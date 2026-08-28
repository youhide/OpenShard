//! `soundidx.mul`/`sound.mul`: the short sounds played at world locations.
//!
//! Classic installs have the ordinary three-word `soundidx.mul` entries;
//! modern ones put the same entries in `soundLegacyMUL.uop` under
//! `build/soundlegacymul/{id:08}.dat`.  The normal entry is a forty-byte name
//! followed by mono, signed 16-bit PCM at 22,050 Hz; a few patched packs wrap
//! their audio in RIFF/WAVE instead. [`SoundArchive`] accepts both shapes and
//! presents ready-to-mix samples. A RIFF wrapper is authoritative: its channel
//! count and sample width are used rather than treating every payload as the
//! classic form.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::num::{NonZeroU16, NonZeroU32};
use std::path::{Path, PathBuf};

use openshard_protocol::wire::SoundId;

const INDEX_ENTRY: usize = 12;
const NO_ENTRY: u32 = u32::MAX;
const LEGACY_HEADER: usize = 40;
const DEFAULT_RATE: NonZeroU32 = NonZeroU32::new(22_050).expect("the classic sample rate is nonzero");

/// One decoded, mono sound effect.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    /// Samples in Rodio's normal -1.0 to 1.0 range.
    pub samples: Vec<f32>,
    /// Number of interleaved channels in [`samples`](Self::samples).
    pub channels: NonZeroU16,
    /// Samples per second.
    pub sample_rate: NonZeroU32,
}

/// An open pair of UO sound files.
#[derive(Debug)]
pub struct SoundArchive {
    backing: Backing,
}

#[derive(Debug)]
enum Backing {
    Mul {
        index: Vec<Entry>,
        data: File,
        data_path: PathBuf,
    },
    Uop(crate::uop::Uop),
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    offset: u32,
    length: u32,
}

/// A sound archive could not be opened or read.
#[derive(Debug)]
#[non_exhaustive]
pub enum SoundError {
    /// One of the two files could not be read.
    Read { path: PathBuf, source: std::io::Error },
    /// The index is not made of twelve-byte entries.
    NotAnIndex { path: PathBuf, size: usize },
    /// A present index entry points outside `sound.mul`.
    Malformed { sound: SoundId, detail: String },
    /// The modern UOP archive could not be opened or read.
    Uop(crate::uop::UopError),
}

impl fmt::Display for SoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotAnIndex { path, size } => write!(
                f,
                "{} is {size} bytes, not a whole number of {INDEX_ENTRY}-byte sound entries",
                path.display()
            ),
            Self::Malformed { sound, detail } => write!(f, "sound {:04X}: {detail}", sound.0),
            Self::Uop(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SoundError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::NotAnIndex { .. } | Self::Malformed { .. } => None,
            Self::Uop(error) => error.source(),
        }
    }
}

impl SoundArchive {
    /// Open the sound pair in a client installation.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, SoundError> {
        let dir = client_dir.as_ref();
        let uop_path = dir.join("soundLegacyMUL.uop");
        if uop_path.is_file() {
            return Ok(Self {
                backing: Backing::Uop(crate::uop::Uop::open(uop_path).map_err(SoundError::Uop)?),
            });
        }
        Self::from_files(dir.join("soundidx.mul"), dir.join("sound.mul"))
    }

    /// Open a named pair, principally for tests.
    pub fn from_files(idx_path: impl AsRef<Path>, data_path: impl AsRef<Path>) -> Result<Self, SoundError> {
        let idx_path = idx_path.as_ref();
        let bytes = std::fs::read(idx_path).map_err(|source| SoundError::Read {
            path: idx_path.to_owned(),
            source,
        })?;
        if !bytes.len().is_multiple_of(INDEX_ENTRY) {
            return Err(SoundError::NotAnIndex {
                path: idx_path.to_owned(),
                size: bytes.len(),
            });
        }
        let index = bytes
            .chunks_exact(INDEX_ENTRY)
            .map(|entry| Entry {
                offset: u32::from_le_bytes(entry[0..4].try_into().expect("four bytes")),
                length: u32::from_le_bytes(entry[4..8].try_into().expect("four bytes")),
            })
            .collect();
        let data_path = data_path.as_ref().to_owned();
        let data = File::open(&data_path).map_err(|source| SoundError::Read {
            path: data_path.clone(),
            source,
        })?;
        Ok(Self {
            backing: Backing::Mul {
                index,
                data,
                data_path,
            },
        })
    }

    /// Decode `sound`, or return `None` for an empty/out-of-range index slot.
    pub fn sound(&mut self, sound: SoundId) -> Result<Option<Sound>, SoundError> {
        let bytes = match &mut self.backing {
            Backing::Mul {
                index,
                data,
                data_path,
            } => {
                let Some(entry) = index.get(usize::from(sound.0)).copied() else {
                    return Ok(None);
                };
                if entry.offset == NO_ENTRY || entry.length == NO_ENTRY || entry.length == 0 {
                    return Ok(None);
                }
                let length = usize::try_from(entry.length).map_err(|_| SoundError::Malformed {
                    sound,
                    detail: "entry length does not fit this platform".to_owned(),
                })?;
                data.seek(SeekFrom::Start(u64::from(entry.offset)))
                    .map_err(|source| SoundError::Read {
                        path: data_path.clone(),
                        source,
                    })?;
                let mut bytes = vec![0; length];
                data.read_exact(&mut bytes).map_err(|source| SoundError::Read {
                    path: data_path.clone(),
                    source,
                })?;
                bytes
            }
            Backing::Uop(uop) => {
                let name = format!("build/soundlegacymul/{:08}.dat", sound.0);
                let Some(bytes) = uop.entry(&name).map_err(SoundError::Uop)? else {
                    return Ok(None);
                };
                bytes.to_vec()
            }
        };
        decode(sound, &bytes).map(Some)
    }
}

fn decode(sound: SoundId, bytes: &[u8]) -> Result<Sound, SoundError> {
    let wave = if bytes.get(0..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        Some(wave_data(bytes).ok_or_else(|| SoundError::Malformed {
            sound,
            detail: "invalid RIFF/WAVE header".to_owned(),
        })?)
    } else {
        None
    };
    let (sample_rate, channels, bits_per_sample, pcm) = wave
        .map(|wave| (wave.sample_rate, wave.channels, wave.bits_per_sample, wave.data))
        .unwrap_or((
            DEFAULT_RATE,
            NonZeroU16::MIN,
            16,
            bytes.get(LEGACY_HEADER..).unwrap_or_default(),
        ));
    if pcm.is_empty() {
        return Err(SoundError::Malformed {
            sound,
            detail: "entry has no PCM samples after its header".to_owned(),
        });
    }
    let samples = pcm_samples(pcm, bits_per_sample).ok_or_else(|| SoundError::Malformed {
        sound,
        detail: format!("unsupported PCM sample width {bits_per_sample}"),
    })?;
    Ok(Sound {
        samples,
        channels,
        sample_rate,
    })
}

/// Find the `data` chunk in a standard RIFF/WAVE wrapper.
fn wave_data(bytes: &[u8]) -> Option<Wave<'_>> {
    (bytes.get(0..4)? == b"RIFF" && bytes.get(8..12)? == b"WAVE").then_some(())?;
    let mut format = None;
    let mut at: usize = 12;
    while at.checked_add(8)? <= bytes.len() {
        let kind = bytes.get(at..at + 4)?;
        let length = u32::from_le_bytes(bytes.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        let data = at.checked_add(8)?;
        let end = data.checked_add(length)?;
        let chunk = bytes.get(data..end)?;
        if kind == b"fmt " && chunk.len() >= 16 {
            format = Some(WaveFormat {
                encoding: u16::from_le_bytes(chunk[0..2].try_into().ok()?),
                channels: u16::from_le_bytes(chunk[2..4].try_into().ok()?),
                sample_rate: u32::from_le_bytes(chunk[4..8].try_into().ok()?),
                bits_per_sample: u16::from_le_bytes(chunk[14..16].try_into().ok()?),
            });
        }
        if kind == b"data" {
            let format = format?;
            if format.encoding != 1 {
                return None;
            }
            return Some(Wave {
                data: chunk,
                channels: NonZeroU16::new(format.channels)?,
                sample_rate: NonZeroU32::new(format.sample_rate)?,
                bits_per_sample: format.bits_per_sample,
            });
        }
        at = end.checked_add(length % 2)?;
    }
    None
}

struct Wave<'a> {
    data: &'a [u8],
    channels: NonZeroU16,
    sample_rate: NonZeroU32,
    bits_per_sample: u16,
}

struct WaveFormat {
    encoding: u16,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
}

/// Convert interleaved little-endian PCM samples to Rodio's normalized range.
fn pcm_samples(bytes: &[u8], bits_per_sample: u16) -> Option<Vec<f32>> {
    match bits_per_sample {
        8 => Some(
            bytes
                .iter()
                .map(|sample| (f32::from(*sample) - 128.0) / 128.0)
                .collect(),
        ),
        16 if bytes.len().is_multiple_of(2) => Some(
            bytes
                .chunks_exact(2)
                .map(|sample| f32::from(i16::from_le_bytes(sample.try_into().expect("two bytes"))) / 32768.0)
                .collect(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_data_uses_the_rate_and_data_chunk() {
        let mut wave =
            b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x22\x56\0\0\x22\x56\0\0\x01\0\x08\0data\x03\0\0\0"
                .to_vec();
        wave.extend([0, 128, 255]);
        let wave = wave_data(&wave).unwrap();
        assert_eq!(wave.sample_rate.get(), 22_050);
        assert_eq!(wave.channels.get(), 1);
        assert_eq!(wave.bits_per_sample, 8);
        assert_eq!(wave.data, [0, 128, 255]);
    }

    #[test]
    fn legacy_name_is_skipped_and_signed_pcm_is_normalized() {
        let mut bytes = vec![0; LEGACY_HEADER];
        bytes.extend(i16::MIN.to_le_bytes());
        bytes.extend(0i16.to_le_bytes());
        bytes.extend(i16::MAX.to_le_bytes());
        let sound = decode(SoundId(7), &bytes).unwrap();
        assert_eq!(sound.channels.get(), 1);
        assert_eq!(sound.sample_rate, DEFAULT_RATE);
        assert_eq!(sound.samples, [-1.0, 0.0, i16::MAX as f32 / 32768.0]);
    }

    #[test]
    fn wave_16_bit_samples_keep_their_sample_boundaries() {
        let mut wave =
            b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0\x88X\x01\0\x02\0\x10\0data\x06\0\0\0"
                .to_vec();
        wave.extend(i16::MIN.to_le_bytes());
        wave.extend(0i16.to_le_bytes());
        wave.extend(i16::MAX.to_le_bytes());

        let sound = decode(SoundId(7), &wave).unwrap();
        assert_eq!(sound.channels.get(), 1);
        assert_eq!(sound.sample_rate.get(), 44_100);
        assert_eq!(sound.samples, [-1.0, 0.0, i16::MAX as f32 / 32768.0]);
    }

    #[test]
    fn wave_stereo_keeps_its_channel_layout() {
        let mut wave =
            b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x02\0\x22V\0\0\x88X\x01\0\x02\0\x08\0data\x04\0\0\0"
                .to_vec();
        wave.extend([0, 255, 128, 64]);

        let sound = decode(SoundId(7), &wave).unwrap();
        assert_eq!(sound.channels.get(), 2);
        assert_eq!(sound.samples, [-1.0, 127.0 / 128.0, 0.0, -0.5]);
    }

    #[test]
    fn wave_rejects_zero_audio_dimensions_instead_of_inventing_them() {
        let valid =
            b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x22\x56\0\0\x22\x56\0\0\x01\0\x08\0data\x01\0\0\0\x80";
        let mut zero_channels = valid.to_vec();
        zero_channels[22..24].copy_from_slice(&0u16.to_le_bytes());
        let mut zero_rate = valid.to_vec();
        zero_rate[24..28].copy_from_slice(&0u32.to_le_bytes());

        for bytes in [&zero_channels, &zero_rate] {
            let error = decode(SoundId(7), bytes).expect_err("zero is not valid audio metadata");
            assert!(error.to_string().contains("invalid RIFF/WAVE header"));
        }
    }

    #[test]
    fn real_skeleton_attack_sound_is_a_valid_mono_source() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(std::path::PathBuf::from) else {
            return;
        };
        let Ok(mut archive) = SoundArchive::open(&dir) else {
            return;
        };
        // The skeleton's BaseSoundID is 0x048D; an attack is BaseSoundID + 2.
        let sound = archive.sound(SoundId(0x048F)).unwrap().unwrap();
        assert_eq!(sound.sample_rate.get(), 22_050);
        assert_eq!(sound.channels.get(), 1);
        assert_eq!(sound.samples.len(), 15_347);
        assert!(
            sound
                .samples
                .iter()
                .all(|sample| sample.is_finite() && (-1.0..=1.0).contains(sample))
        );
    }
}
