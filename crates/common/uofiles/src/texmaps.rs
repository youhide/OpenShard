//! `texmaps.mul`: the square textures the ground is stretched over.
//!
//! A land tile has two pictures and the client picks between them by shape. On
//! level ground it draws the 44×44 diamond out of [`crate::art`]. On a slope the
//! four corners no longer form a diamond, so it draws a *square* texture from
//! here, mapped corner to corner onto whatever quadrilateral the heights make.
//! Ported from ClassicUO's `TexmapsLoader` and `Land.ApplyStretch`.
//!
//! # Which texture belongs to which tile
//!
//! Not the land graphic — a separate id, held in `tiledata.mul`'s land entry as
//! [`openshard_tiles::LandTile::texture`]. Nothing relates the two numbers, so
//! reading that field at the wrong offset produces a picture rather than an
//! error: the ground is textured with somebody else's terrain and looks like a
//! seasonal variant. That is what the shipped-file test compares against the
//! art.
//!
//! # The format
//!
//! The plainest one in the client. `texidx.mul` is 0x4000 twelve-byte entries —
//! offset, length, extra — and `texmaps.mul` is the pixels, [`Color16`] each,
//! row-major, with no header and no compression. The side is not stored: a
//! texture is 64×64 or 128×128, and the *length* is what says which, exactly as
//! ClassicUO reads it (`entry.Length == 0x2000 ? 64 : 128`). The `extra` word
//! agrees on this client — 0 for the small ones and 1 for the large — but it is
//! the length that is checked, because the length is what has to be consistent
//! with the bytes for the read to stay inside the file.
//!
//! # Three quarters of the table is empty
//!
//! A modern client ships 4,116 textures for 16,384 slots, and entry 0 is one of
//! the empty ones — which is why a land tile with texture id 0 means "no texture"
//! without anything having to say so. An absent entry is [`None`], not an error:
//! the client draws such a tile flat even where the ground slopes.

use std::fmt;
use std::path::{
    Path,
    PathBuf,
};

use openshard_tiles::TextureId;

use crate::color::Color16;
use crate::image::Image;

/// How many textures the index has room for. The same 0x4000 as the land half
/// of the art, and not the same index space.
pub const TEXTURE_COUNT: usize = 0x4000;

/// Bytes per `texidx` entry: offset, length, extra.
const INDEX_ENTRY: usize = 12;

/// The two sides a texture can have.
const SMALL_SIDE: u16 = 64;
const LARGE_SIDE: u16 = 128;

/// An entry that says this is where a texture is absent.
const NO_OFFSET: u32 = u32::MAX;

/// The texture maps could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum TexMapError {
    /// A file could not be read.
    Read {
        /// Which file.
        path:   PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// `texidx.mul` is not a whole number of entries.
    NotAnIndex {
        /// Which file.
        path: PathBuf,
        /// How big it is.
        size: usize,
    },
    /// An entry describes bytes the data file does not have, or a length that
    /// is neither of the two square textures.
    Malformed {
        /// Which texture.
        id:     TextureId,
        /// What went wrong.
        detail: String,
    },
}

impl fmt::Display for TexMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotAnIndex { path, size } => {
                write!(
                    f,
                    "{} is {size} bytes, which is not a whole number of {INDEX_ENTRY}-byte entries",
                    path.display()
                )
            }
            Self::Malformed { id, detail } => {
                write!(f, "texture {} is malformed: {detail}", id.0)
            }
        }
    }
}

impl std::error::Error for TexMapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Where one texture's bytes are, as the index describes them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Entry {
    offset: u32,
    length: u32,
}

/// The client's land textures, open and addressable.
pub struct TexMaps {
    entries: Vec<Option<Entry>>,
    /// All of `texmaps.mul`, about 45MB of it. The same shape every reader in
    /// this crate has — see the backlog in `docs/client.md`.
    data:    Vec<u8>,
}

impl fmt::Debug for TexMaps {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TexMaps")
            .field("entries", &self.entries.len())
            .field("present", &self.entries.iter().filter(|e| e.is_some()).count())
            .field("bytes", &self.data.len())
            .finish()
    }
}

impl TexMaps {
    /// Open `texidx.mul` and `texmaps.mul` in a client directory.
    pub fn open(client_dir: impl AsRef<Path>) -> Result<Self, TexMapError> {
        let dir = client_dir.as_ref();
        Self::from_files(dir.join("texidx.mul"), dir.join("texmaps.mul"))
    }

    /// Open a named pair, for a client whose files are not laid out as one.
    pub fn from_files(index: impl AsRef<Path>, data: impl AsRef<Path>) -> Result<Self, TexMapError> {
        let index_path = index.as_ref();
        let index_bytes = read(index_path)?;
        let data = read(data.as_ref())?;
        if index_bytes.len() % INDEX_ENTRY != 0 {
            return Err(TexMapError::NotAnIndex {
                path: index_path.to_owned(),
                size: index_bytes.len(),
            });
        }

        // Shorter than the full table is not an error: what it does not describe
        // is absent, which is the same answer as an empty entry. Longer is not
        // one either — nothing here looks past the ids `tiledata` can hold.
        let mut entries = Vec::with_capacity(TEXTURE_COUNT);
        for id in 0..TEXTURE_COUNT {
            let at = id * INDEX_ENTRY;
            entries.push(match index_bytes.get(at..at + INDEX_ENTRY) {
                Some(raw) => {
                    let offset = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                    let length = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
                    // `extra` — raw[8..12] — is 0 for a 64 and 1 for a 128 on
                    // this client, and is deliberately not read: the length has
                    // to be right for the read to stay inside the file, so the
                    // length is the one number worth believing.
                    (offset != NO_OFFSET && length != 0).then_some(Entry { offset, length })
                }
                None => None,
            });
        }

        Ok(Self { entries, data })
    }

    /// How many of the table's slots the client actually ships a texture for.
    pub fn present(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }

    /// Whether this id has a texture at all.
    ///
    /// The client asks exactly this before deciding to stretch a tile: without a
    /// texture it draws the flat diamond however the ground slopes.
    pub fn has(&self, id: TextureId) -> bool {
        self.entries.get(id.0 as usize).is_some_and(Option::is_some)
    }

    /// The texture for an id, or `None` if the client ships none.
    ///
    /// Always square, and always [`SMALL_SIDE`] or [`LARGE_SIDE`] on a side.
    /// Every pixel is a colour: a texture has no transparency, so nothing here
    /// treats a zero word as absent.
    pub fn texture(&self, id: TextureId) -> Result<Option<Image>, TexMapError> {
        let Some(Some(entry)) = self.entries.get(id.0 as usize) else {
            return Ok(None);
        };
        let malformed = |detail: String| TexMapError::Malformed { id, detail };

        let side = match entry.length {
            len if len == u32::from(SMALL_SIDE) * u32::from(SMALL_SIDE) * 2 => SMALL_SIDE,
            len if len == u32::from(LARGE_SIDE) * u32::from(LARGE_SIDE) * 2 => LARGE_SIDE,
            len => {
                return Err(malformed(format!(
                    "{len} bytes is neither a {SMALL_SIDE} nor a {LARGE_SIDE} square"
                )));
            }
        };

        let start = entry.offset as usize;
        let raw = self
            .data
            .get(start..start + entry.length as usize)
            .ok_or_else(|| {
                malformed(format!(
                    "its {} bytes at {start} run past the end of a {}-byte file",
                    entry.length,
                    self.data.len()
                ))
            })?;

        let pixels = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|word| Color16(u16::from_le_bytes([word[0], word[1]])))
            .collect();
        Ok(Some(Image::new(side, side, pixels)))
    }
}

fn read(path: &Path) -> Result<Vec<u8>, TexMapError> {
    std::fs::read(path).map_err(|source| {
        TexMapError::Read {
            path: path.to_owned(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that takes its files with it.
    struct ScratchDir(PathBuf);

    impl ScratchDir {
        fn new(name: &str) -> Self {
            // The pid is not decoration. `name` being unique makes the
            // directories unique *within* one run, which is what the helper's
            // own doc is about — but two `cargo test` processes at once (a
            // workspace run beside a focused one, or CI's) both compute the same
            // path, and the second's `remove_dir_all` deletes the first's files
            // between its `create_dir_all` and its `write`. `map.rs`'s
            // `ScratchDir` had already worked this out and guarded it; this one
            // was written from the same idea and missed the process half.
            let dir = std::env::temp_dir().join(format!("openshard-texmaps-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("a writable temp directory");
            Self(dir)
        }

        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).expect("writing to our own temp directory");
            path
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An index of `entries`, each `(offset, length)`, and the data to match.
    ///
    /// `name` is the caller's, and it has to be unique: the tests run in
    /// parallel, and two of them sharing a scratch directory means one deletes
    /// the other's files halfway through — which fails as "the index says the
    /// texture is there and the data file disagrees", the exact shape of the bug
    /// these tests are about. Uniqueness *across processes* is
    /// [`ScratchDir::new`]'s own, and for the same reason one process further
    /// down: it failed only on a full workspace run, never on the test alone.
    fn synthetic(name: &str, entries: &[(u32, u32)], data: Vec<u8>) -> (ScratchDir, TexMaps) {
        let dir = ScratchDir::new(name);
        let mut index = Vec::new();
        for (offset, length) in entries {
            index.extend_from_slice(&offset.to_le_bytes());
            index.extend_from_slice(&length.to_le_bytes());
            index.extend_from_slice(&0u32.to_le_bytes());
        }
        let index_path = dir.write("texidx.mul", &index);
        let data_path = dir.write("texmaps.mul", &data);
        let maps = TexMaps::from_files(&index_path, &data_path).expect("a well-formed pair");
        (dir, maps)
    }

    /// A 64x64 texture whose pixels count up, so a misread row shows up as a
    /// value rather than as a plausible colour.
    fn small_texture() -> Vec<u8> {
        (0..SMALL_SIDE as u32 * SMALL_SIDE as u32)
            .flat_map(|i| (i as u16).to_le_bytes())
            .collect()
    }

    #[test]
    fn a_texture_is_read_row_major_at_the_side_its_length_names() {
        let data = small_texture();
        let (_dir, maps) = synthetic("row-major", &[(0, 0), (0, data.len() as u32)], data);

        // Entry 0 is empty on the real client too, which is what makes "texture
        // id 0" mean "no texture" without anything saying so.
        assert!(!maps.has(TextureId(0)));
        assert_eq!(maps.texture(TextureId(0)).unwrap(), None);

        let image = maps.texture(TextureId(1)).unwrap().expect("entry 1 is present");
        assert_eq!((image.width(), image.height()), (SMALL_SIDE, SMALL_SIDE));
        assert_eq!(image.pixel(0, 0), Some(Color16(0)));
        // Row-major: the second row starts at pixel 64, not at pixel 1.
        assert_eq!(image.pixel(0, 1), Some(Color16(SMALL_SIDE)));
        assert_eq!(image.pixel(1, 0), Some(Color16(1)));
    }

    #[test]
    fn the_two_sides_are_told_apart_by_length_alone() {
        let large = LARGE_SIDE as usize * LARGE_SIDE as usize * 2;
        let data = vec![0u8; large];
        let (_dir, maps) = synthetic("two-sides", &[(0, 0), (0, large as u32)], data);
        let image = maps.texture(TextureId(1)).unwrap().expect("present");
        assert_eq!(image.width(), LARGE_SIDE);
    }

    #[test]
    fn a_length_that_is_neither_square_is_refused_rather_than_guessed() {
        // ClassicUO reads anything that is not 0x2000 as a 128 square, which on
        // a truncated file walks off the end of it. Neither size is a guess here.
        let (_dir, maps) = synthetic("odd-length", &[(0, 1024)], vec![0u8; 1024]);
        assert!(maps.texture(TextureId(0)).is_err());
    }

    #[test]
    fn an_entry_pointing_past_the_data_is_refused_rather_than_panicking() {
        let small = SMALL_SIDE as usize * SMALL_SIDE as usize * 2;
        let (_dir, maps) = synthetic("past-the-end", &[(8, small as u32)], vec![0u8; small]);
        assert!(maps.has(TextureId(0)), "the index claims it is there");
        assert!(maps.texture(TextureId(0)).is_err(), "and the data is not");
    }

    #[test]
    fn an_index_that_is_not_whole_entries_is_refused() {
        let dir = ScratchDir::new("ragged");
        let index = dir.write("texidx.mul", &[0u8; INDEX_ENTRY + 3]);
        let data = dir.write("texmaps.mul", &[]);
        assert!(matches!(
            TexMaps::from_files(&index, &data),
            Err(TexMapError::NotAnIndex { .. })
        ));
    }

    #[test]
    fn an_id_past_the_end_of_the_index_is_absent_and_not_a_panic() {
        // Every id came out of `tiledata`, which is a `u16` and describes a
        // client that may not be this one.
        let (_dir, maps) = synthetic("out-of-range", &[(0, 0)], Vec::new());
        assert!(!maps.has(TextureId(u16::MAX)));
        assert_eq!(maps.texture(TextureId(u16::MAX)).unwrap(), None);
        assert_eq!(maps.texture(TextureId(TEXTURE_COUNT as u16)).unwrap(), None);
    }

    #[test]
    fn a_marked_absent_entry_is_absent_however_it_is_marked() {
        // Both spellings the files use: the sentinel offset, and a zero length.
        let (_dir, maps) = synthetic("absent", &[(NO_OFFSET, 8192), (0, 0)], vec![0u8; 8192]);
        assert!(!maps.has(TextureId(0)));
        assert!(!maps.has(TextureId(1)));
        assert_eq!(maps.present(), 0);
    }
}
