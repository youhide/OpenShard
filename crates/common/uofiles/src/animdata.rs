//! `animdata.mul`: how an animated static cycles through its graphics.
//!
//! A torch, a fire, a water wheel, a spinning wheel — the ones that move without
//! anybody touching them. The file says nothing about *where* they are: it is a
//! table indexed by static graphic, and every copy of that graphic on the map
//! plays the same cycle in the same phase. Which of them animate at all is
//! [`TileFlags::ANIMATION`](openshard_tiles::TileFlags::ANIMATION) in
//! `tiledata.mul` and not anything in here — this file has an entry for every
//! graphic and most of them are zeroed.
//!
//! # A mobile's animation is a different thing entirely
//!
//! `anim.mul` packs frames of a *body*: a group, a direction and a run of
//! pictures that are only that body's. Here there are no frames at all — an
//! entry is a list of signed offsets applied to the graphic id, so a fire cycles
//! by *being* four different statics in turn, each with its own ordinary art
//! entry. That is why the atlas has nothing new to learn: an animated static's
//! frames are statics.
//!
//! # The layout
//!
//! Eight entries to a group, each group behind a four-byte header nothing reads,
//! and an entry is 68 bytes: 64 signed offsets, one unused byte, the number of
//! offsets in use, the interval, and a start index. So the entry for graphic `g`
//! begins at `g * 68 + 4 * ((g >> 3) + 1)` — `AnimDataLoader.CalculateCurrentGraphic`,
//! and the same arithmetic in `AnimatedStaticsManager.Initialize`. A real
//! `animdata.mul` is 4,486,748 bytes, which is 65,499 whole entries and then a
//! group that stops part way; the bound check is not defensive, it is that file.

use std::fmt;
use std::path::{Path, PathBuf};

use openshard_protocol::wire::Graphic;

/// How many bytes one graphic's entry takes.
const ENTRY: usize = 68;

/// How many entries share one header.
const GROUP: usize = 8;

/// The header before each group, which nothing here reads.
const HEADER: usize = 4;

/// How many offsets an entry has room for.
///
/// The `FrameData` array's own length. A real file's longest cycle is a small
/// fraction of it — nothing in `animdata.mul` uses more than eight — but the
/// slot is 64 bytes wide either way and a count is only trusted up to it.
pub const MAX_FRAMES: usize = 64;

/// The number of offsets in an animated static's cycle.
///
/// `animdata.mul` stores this as a byte, but only the non-zero range that fits
/// in an entry describes a playable cycle. Keeping that invariant at the
/// parsed-data boundary prevents callers from treating the file's padding as
/// additional frames.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AnimationFrameCount(u8);

impl AnimationFrameCount {
    /// Construct a count accepted by `animdata.mul`.
    pub const fn new(raw: u8) -> Option<Self> {
        if raw == 0 || raw as usize > MAX_FRAMES {
            None
        } else {
            Some(Self(raw))
        }
    }

    /// Number of offsets in the cycle.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// What one animated static does, read out of the file.
///
/// The offsets are *signed and relative*: the graphic on screen is the base
/// graphic plus the offset, so a cycle of four ordinarily runs across four
/// consecutive art entries and can start below its own base. `0x006D` is
/// `[-3, -2, -1, 0, 1, 2]`, which is the six graphics `0x006A..=0x006F` — and
/// its neighbour `0x006E` holds the same six rotated, which is how the client
/// makes two fires next to each other burn out of step without any per-tile
/// state at all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sequence {
    /// The offsets, of which only the first [`Sequence::count`] are in use.
    frames: [i8; MAX_FRAMES],
    /// How many of them there are. Never zero and never above [`MAX_FRAMES`] —
    /// [`AnimData::sequence`] answers `None` rather than handing back either.
    count: AnimationFrameCount,
    /// How long each offset is shown, in units of [`FRAME_STEP`].
    ///
    /// Zero means one step: `AnimatedStaticsManager.Process` schedules
    /// `Time.Ticks + delay` for it rather than `interval * delay`, which is the
    /// same thing as an interval of one.
    interval: u8,
    /// Which offset the client's own table starts at.
    ///
    /// Kept because it is in the file and read by nothing, here or in the
    /// reference: `AnimatedStaticsManager` starts every cycle at index zero, and
    /// the phase that makes neighbouring fires differ is baked into the offsets
    /// themselves. Named rather than skipped so that the next person to open the
    /// file does not conclude it was missed.
    start: u8,
}

impl Sequence {
    /// How many graphics this cycles through.
    pub const fn count(self) -> AnimationFrameCount {
        self.count
    }

    /// How long one of them is shown, in units of [`FRAME_STEP`].
    pub const fn interval(self) -> u8 {
        self.interval
    }

    /// The start index the file records and nothing acts on. See the field.
    pub const fn start(self) -> u8 {
        self.start
    }

    /// The offsets in use, in the order they are played.
    pub fn offsets(self) -> impl Iterator<Item = i8> {
        (0..usize::from(self.count.get())).map(move |index| self.frames[index])
    }
}

/// The table, or an empty one when the client ships no `animdata.mul`.
///
/// Held as the file's own bytes rather than as parsed entries: 64,296 of the
/// 65,499 graphics in a real file animate nothing at all, so parsing every one
/// of them up front would build a megabyte of zeroes to answer questions nobody
/// asks. The caller that *does* want them all — a renderer building the set of
/// animated graphics once — walks the flags in `tiledata.mul` and asks for those.
#[derive(Clone, Default)]
pub struct AnimData {
    bytes: Vec<u8>,
}

impl fmt::Debug for AnimData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnimData")
            .field("bytes", &self.bytes.len())
            .field("graphics", &self.graphics())
            .finish()
    }
}

impl AnimData {
    /// Read `animdata.mul` from a client directory.
    ///
    /// **A missing file is not an error.** `animdata.mul` is one of the few the
    /// client will start without, and a client directory that has no fires in it
    /// is a client directory with no fires — not a failure to open a map. The
    /// empty table answers `None` to everything, which is what a static that does
    /// not animate looks like anyway.
    pub fn load(client_dir: impl AsRef<Path>) -> Result<Self, AnimDataError> {
        let path = client_dir.as_ref().join("animdata.mul");
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Self { bytes }),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(AnimDataError::Read { path, source }),
        }
    }

    /// Parse bytes that are already in memory.
    pub fn parse(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
        }
    }

    /// How many graphics this file has a whole entry for.
    ///
    /// The end of a real file lands inside a group, so this is not
    /// `bytes.len() / 68`: it is however many entries fit completely.
    pub fn graphics(&self) -> usize {
        // `pos(g) + ENTRY <= len` solved for `g`, and solved by counting rather
        // than by algebra because the group header makes the stride uneven at
        // every eighth entry and an off-by-one here reads the next entry's
        // offsets as this one's.
        let stride = HEADER + GROUP * ENTRY;
        let groups = self.bytes.len() / stride;
        let rest = self.bytes.len() % stride;
        groups * GROUP + rest.saturating_sub(HEADER) / ENTRY
    }

    /// What `graphic` cycles through, or `None` if it cycles through nothing.
    ///
    /// `None` covers three cases that are one answer: the file is shorter than
    /// this graphic, the entry is zeroed — which is 98% of them — or its count is
    /// past the 64 offsets an entry holds, which is a corrupt entry and not a
    /// cycle anybody can play. A caller draws the base graphic in all three.
    pub fn sequence(&self, graphic: Graphic) -> Option<Sequence> {
        let index = usize::from(graphic.0);
        let at = index * ENTRY + HEADER * ((index / GROUP) + 1);
        let raw = self.bytes.get(at..at + ENTRY)?;

        let count = AnimationFrameCount::new(raw[MAX_FRAMES + 1])?;
        let mut frames = [0i8; MAX_FRAMES];
        for (slot, byte) in frames.iter_mut().zip(&raw[..MAX_FRAMES]) {
            *slot = *byte as i8;
        }
        Some(Sequence {
            frames,
            count,
            interval: raw[MAX_FRAMES + 2],
            start: raw[MAX_FRAMES + 3],
        })
    }
}

/// Why `animdata.mul` could not be read. A missing file is not one of these.
#[derive(Debug)]
pub enum AnimDataError {
    /// The file is there and unreadable.
    Read {
        /// What was being opened.
        path: PathBuf,
        /// What the operating system said.
        source: std::io::Error,
    },
}

impl fmt::Display for AnimDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for AnimDataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_frame_count_rejects_empty_and_overflowing_cycles() {
        assert_eq!(AnimationFrameCount::new(0), None);
        assert_eq!(AnimationFrameCount::new((MAX_FRAMES + 1) as u8), None);
        assert_eq!(AnimationFrameCount::new(6).map(AnimationFrameCount::get), Some(6));
    }

    /// A file of `groups` whole groups, where entry `g` has `g % 7` offsets, each
    /// offset being its own index, and an interval of `g % 5`.
    ///
    /// Synthetic, and it is checked for what a fixture can honestly be checked
    /// for: that this module's own arithmetic lands on the entry it meant to.
    /// Whether that arithmetic matches the *client's* is not something a fixture
    /// can say, and the test at the foot of this module reads a real file for it.
    fn synthetic(groups: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for group in 0..groups {
            out.extend_from_slice(&[0xAA; HEADER]);
            for slot in 0..GROUP {
                let g = group * GROUP + slot;
                let mut entry = [0u8; ENTRY];
                let count = (g % 7) as u8;
                for (index, slot) in entry.iter_mut().take(usize::from(count)).enumerate() {
                    *slot = index as u8;
                }
                entry[MAX_FRAMES + 1] = count;
                entry[MAX_FRAMES + 2] = (g % 5) as u8;
                entry[MAX_FRAMES + 3] = (g % 3) as u8;
                out.extend_from_slice(&entry);
            }
        }
        out
    }

    /// The stride, the header and the entry, all three at once: read the entry
    /// this module says it is reading, at every position in a group and across a
    /// group boundary.
    #[test]
    fn an_entry_is_found_behind_its_own_groups_header() {
        let data = AnimData::parse(&synthetic(3));
        assert_eq!(data.graphics(), 24);
        for graphic in 0..24u16 {
            let count = (usize::from(graphic) % 7) as u8;
            match data.sequence(Graphic(graphic)) {
                None => assert_eq!(count, 0, "{graphic} has {count} offsets and was not read"),
                Some(sequence) => {
                    assert_eq!(sequence.count().get(), count, "{graphic}");
                    assert_eq!(sequence.interval(), (usize::from(graphic) % 5) as u8, "{graphic}");
                    assert_eq!(sequence.start(), (usize::from(graphic) % 3) as u8, "{graphic}");
                    let offsets: Vec<i8> = sequence.offsets().collect();
                    assert_eq!(offsets, (0..count as i8).collect::<Vec<_>>(), "{graphic}");
                }
            }
        }
    }

    /// Past the end of the file is the same answer as an empty entry, and a
    /// client with no `animdata.mul` at all is a client where nothing animates
    /// rather than a client that fails to start.
    #[test]
    fn a_graphic_past_the_end_and_an_absent_file_both_animate_nothing() {
        let data = AnimData::parse(&synthetic(1));
        assert_eq!(data.graphics(), 8);
        assert_eq!(data.sequence(Graphic(8)), None);
        assert_eq!(data.sequence(Graphic(u16::MAX)), None);

        let empty = AnimData::default();
        assert_eq!(empty.graphics(), 0);
        assert_eq!(empty.sequence(Graphic(0)), None);
    }

    /// A group that stops part way through is worth however many whole entries
    /// it holds, and not one more: the last entry of a truncated file would
    /// otherwise be read with somebody else's bytes in its tail.
    #[test]
    fn a_truncated_group_is_worth_its_whole_entries() {
        let mut bytes = synthetic(2);
        bytes.extend_from_slice(&[0xAA; HEADER]);
        bytes.extend_from_slice(&[0u8; ENTRY * 2 + 3]);
        let data = AnimData::parse(&bytes);
        assert_eq!(data.graphics(), 18, "sixteen whole, then two of a third group");
        assert_eq!(
            data.sequence(Graphic(18)),
            None,
            "and the one cut short is not read"
        );
    }

    /// The real file, and the two things a fixture cannot say: that the entry
    /// arithmetic lands where the client's does, and that what comes out is a
    /// cycle rather than noise.
    ///
    /// `0x006D` is pinned by hand. It is a fire, its six offsets are the six
    /// consecutive graphics `0x006A..=0x006F`, and its neighbour `0x006E` holds
    /// the same six rotated — which is the phase, and is the observation that
    /// says these bytes are being read as the client reads them rather than
    /// merely being read consistently.
    #[test]
    fn a_real_animdata_reads_as_cycles_of_neighbouring_graphics() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(PathBuf::from) else {
            return;
        };
        let data = AnimData::load(&dir).expect("animdata.mul");
        assert!(data.graphics() > 0x4000, "only {} entries", data.graphics());

        let fire = data.sequence(Graphic(0x006D)).expect("0x006D animates");
        assert_eq!(fire.count().get(), 6);
        assert_eq!(fire.offsets().collect::<Vec<_>>(), [-3, -2, -1, 0, 1, 2]);
        let next = data.sequence(Graphic(0x006E)).expect("0x006E animates");
        assert_eq!(next.offsets().collect::<Vec<_>>(), [-2, -1, 0, 1, -4, -3]);

        // Every graphic in every cycle is a graphic: an offset that took a
        // sequence below zero or past `u16::MAX` would be an entry read at the
        // wrong place, and it would draw a fire as somebody's floor tile.
        let mut animated = 0;
        for graphic in 0..u16::MAX {
            let Some(sequence) = data.sequence(Graphic(graphic)) else {
                continue;
            };
            animated += 1;
            for offset in sequence.offsets() {
                let frame = i32::from(graphic) + i32::from(offset);
                assert!(
                    (0..=i32::from(u16::MAX)).contains(&frame),
                    "0x{graphic:04X} steps to {frame}",
                );
            }
        }
        assert!(animated > 1_000, "only {animated} animated graphics in the file");
    }

    /// The animation bit, *re-derived* from the two files rather than taken from
    /// the reference that named it.
    ///
    /// `TileFlags::ANIMATION` is `0x0100_0000` because ClassicUO and ServUO both
    /// say so, and two references agreeing is still one claim — they are the same
    /// table copied twice. What makes this test worth its runtime is that the two
    /// *files* are independent: `tiledata.mul` says which graphics animate,
    /// `animdata.mul` says what they animate to, and nothing in either points at
    /// the other. So the bit can be found instead of trusted: score every bit in
    /// the static flag word by how well the graphics it selects overlap the
    /// graphics `animdata.mul` actually describes, and the winner has to be this
    /// one.
    ///
    /// It wins by a distance rather than narrowly — 0.28 against 0.15 for the
    /// runner-up on a 7.0.24 install — so the assertion is the ranking and not a
    /// threshold somebody tuned.
    ///
    /// The overlap being far from perfect is not a defect and is the reason this
    /// is a ranking: the two files are maintained by hand and disagree at the
    /// edges. A 7.0.24 install flags 3,691 statics and holds 1,203 non-empty
    /// entries, of which 1,068 are flagged — animations removed and left flagged,
    /// data left behind for graphics no longer placed. The reference lives with
    /// exactly the same gap: `AnimatedStaticsManager` registers every flagged
    /// graphic and then draws the base one for those whose `FrameCount` is zero.
    #[test]
    fn the_animation_flag_is_the_bit_that_predicts_an_animdata_entry() {
        let Some(dir) = std::env::var_os("OPENSHARD_CLIENT").map(PathBuf::from) else {
            return;
        };
        let data = AnimData::load(&dir).expect("animdata.mul");
        let tiledata = crate::tiledata::load(dir.join("tiledata.mul"))
            .expect("tiledata.mul")
            .tiles;

        let described: Vec<bool> = (0..=u16::MAX)
            .map(|graphic| data.sequence(Graphic(graphic)).is_some())
            .collect();
        let total = described.iter().filter(|yes| **yes).count();
        assert!(total > 1_000, "only {total} entries in animdata.mul");

        // Jaccard: the intersection over the union, which is the score that
        // punishes a bit for being *broad* as well as for missing. Plain
        // "how many of mine are described" would be won by any rare bit that
        // happens to fall inside the animated set.
        let score = |mask: u64| {
            let (mut both, mut either) = (0usize, 0usize);
            for (graphic, described) in described.iter().enumerate() {
                let flagged = tiledata.static_tile(graphic as u16).flags.has(mask);
                both += usize::from(flagged && *described);
                either += usize::from(flagged || *described);
            }
            match either {
                0 => 0.0,
                _ => both as f64 / either as f64,
            }
        };

        let mut ranked: Vec<(u64, f64)> = (0..u64::BITS)
            .map(|bit| (1u64 << bit, score(1u64 << bit)))
            .filter(|(_, score)| *score > 0.0)
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let (best, best_score) = ranked[0];
        let (_, runner_up) = ranked[1];
        assert_eq!(
            best,
            openshard_tiles::TileFlags::ANIMATION,
            "0x{best:08X} predicts animdata.mul better ({best_score:.3}) than the bit \
             this crate calls ANIMATION",
        );
        assert!(
            best_score > runner_up * 1.5,
            "0x{best:08X} scores {best_score:.3} against {runner_up:.3}: the bit is not \
             distinguished by the data, and this test is not evidence",
        );
    }
}
