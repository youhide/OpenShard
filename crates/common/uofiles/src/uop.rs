//! UOP containers: where the map actually lives.
//!
//! # `map0.mul` is a lie
//!
//! Modern clients ship both `map0.mul` and `map0LegacyMUL.uop`. The `.mul` is
//! still there and still exactly the right size, and it can be **entirely
//! zeroes** — a stub, kept so that old tools do not fall over. Every byte of
//! the facet is in the `.uop`.
//!
//! This is a nasty way to be wrong. A parser pointed at the stub reads a clean,
//! well-formed, perfectly flat world and reports no error at all. Ours did, and
//! the test that was supposed to prove the block order was correct passed —
//! because a map of all zeroes is perfectly smooth however you index it. See
//! `terrain::tests::the_map_is_not_degenerate`.
//!
//! # The format
//!
//! ```text
//!   header    magic "MYP\0", version, signature, first block, capacity, count
//!   block     entry count, next block offset, then 34-byte entries
//!   entry     data offset, header length, compressed length,
//!             decompressed length, hash, data hash, compression flag
//! ```
//!
//! Entries carry no name — only a 64-bit hash of one. To find file *i* you hash
//! the string the client would have used and look it up.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// `MYP\0`, little-endian.
const MAGIC: u32 = 0x0050_594D;
/// Bytes per file entry in a block.
const ENTRY_BYTES: usize = 34;

/// A UOP file could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum UopError {
    /// The file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file does not start with `MYP\0`.
    NotUop {
        /// Which file.
        path: PathBuf,
    },
    /// The container is structurally broken.
    Malformed {
        /// Which file.
        path: PathBuf,
        /// What went wrong.
        detail: String,
    },
    /// An entry is zlib-compressed and this reader cannot inflate it.
    ///
    /// The map and art containers store their entries uncompressed, so nothing
    /// this crate reads has ever hit this. `gumpartLegacyMUL.uop` is a different
    /// matter — every one of its 5,556 entries is deflated — so whoever reads
    /// gumps first is the one who brings an inflater.
    ///
    /// An error rather than a silent skip: a chunk this reader cannot produce is
    /// a hole in the world, not an absence.
    Compressed {
        /// Which file.
        path: PathBuf,
        /// The name the entry was looked up under.
        name: String,
    },
    /// A file the caller asked for is not in the container.
    MissingEntry {
        /// Which file.
        path: PathBuf,
        /// Which index was expected.
        index: usize,
    },
}

impl fmt::Display for UopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotUop { path } => write!(f, "{} is not a UOP container", path.display()),
            Self::Malformed { path, detail } => {
                write!(f, "{} is malformed: {detail}", path.display())
            }
            Self::Compressed { path, name } => write!(
                f,
                "{} stores {name} zlib-compressed; this reader only handles stored entries",
                path.display()
            ),
            Self::MissingEntry { path, index } => write!(
                f,
                "{} has no entry for index {index}; the container is incomplete",
                path.display()
            ),
        }
    }
}

impl std::error::Error for UopError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// An open UOP container, addressable by the names its entries were hashed
/// from.
///
/// # Why a name and not an index
///
/// A container's entries carry no index and no name — only a 64-bit hash of the
/// path the client would have used. So there is no such thing as "entry 7": there
/// is only "the entry whose name hashes to the same thing as
/// `build/artlegacymul/00000007.tga`". Callers build that string, because only
/// the caller knows the pattern its own file uses.
///
/// # The whole file is held in memory
///
/// A map container is 90MB and the art container is 155MB, and both are read
/// whole. That is the same bargain the rest of this crate makes — see [`Map`] —
/// and it is fine for a shard, which reads the map once and never opens the art.
/// A renderer holding every sprite sheet resident is a different question, and
/// the place to answer it is here, once something is actually drawing.
///
/// [`Map`]: crate::map::Map
pub struct Uop {
    path: PathBuf,
    bytes: Vec<u8>,
    entries: HashMap<u64, Entry>,
    file_count: usize,
}

impl fmt::Debug for Uop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Uop")
            .field("path", &self.path)
            .field("file_count", &self.file_count)
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl Uop {
    /// Read a container and index its entries by name hash.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, UopError> {
        let path = path.as_ref().to_owned();
        let bytes = std::fs::read(&path).map_err(|source| UopError::Read {
            path: path.clone(),
            source,
        })?;
        let header = Header::parse(&bytes).ok_or_else(|| UopError::NotUop { path: path.clone() })?;
        let entries = header.entries(&bytes, &path)?;
        Ok(Self {
            path,
            bytes,
            entries,
            file_count: header.file_count,
        })
    }

    /// A container with no entries at all.
    ///
    /// For a caller that needs the *shape* of a container and not an install's
    /// — [`TileData::empty`](crate::tiledata::TileData::empty)'s reason, one
    /// file over. Every name resolves to `Ok(None)`, which is the answer this
    /// reader already gives for an index a real container does not ship, so
    /// nothing here is a guess at what a file says: it says nothing at all.
    ///
    /// The path is the empty one, because there is no file behind it. Anything
    /// asserting on real entries opens a real install.
    pub fn empty() -> Self {
        Self {
            path: PathBuf::new(),
            bytes: Vec::new(),
            entries: HashMap::new(),
            file_count: 0,
        }
    }

    /// How many files the container's header claims.
    ///
    /// Not the same as how many entries resolve: `art` reserves 0x4000 land
    /// slots and ships about a quarter of them, so a name that is absent is a
    /// tile that does not exist, not a broken file.
    pub const fn file_count(&self) -> usize {
        self.file_count
    }

    /// The bytes stored under `name`, or `None` if the container has no such
    /// entry.
    ///
    /// `Ok(None)` and `Err` say different things on purpose. A missing entry is
    /// ordinary — most art indices are empty. A *compressed* entry is not
    /// something to skip: it is data this reader cannot produce, and silently
    /// returning "nothing here" would draw a hole where a wall is.
    pub fn entry(&self, name: &str) -> Result<Option<&[u8]>, UopError> {
        let Some(entry) = self.entries.get(&hash_file_name(name.as_bytes())) else {
            return Ok(None);
        };
        if entry.compression != UopCompression::STORED {
            return Err(UopError::Compressed {
                path: self.path.clone(),
                name: name.to_owned(),
            });
        }
        let start = entry.data_offset + entry.header_length;
        self.bytes
            .get(start..start + entry.compressed_length)
            .map(Some)
            .ok_or_else(|| UopError::Malformed {
                path: self.path.clone(),
                detail: format!("the entry for {name} runs past the end of the file"),
            })
    }

    /// The entry `name` resolves to, without [`entry`]'s refusal to hand back
    /// something compressed.
    ///
    /// `entry` exists because the map and art containers this crate reads are
    /// never compressed, so a caller silently getting "no entry" for one would
    /// be drawing a hole where a wall is. Gump art breaks that assumption —
    /// every entry in `gumpartLegacyMUL.uop` carries a nonzero compression
    /// flag — and it is [`gumpart`] that knows what that flag means and how to
    /// invert it, not `Uop`. This is the seam between the two: `Uop` hands back
    /// exactly what its own header says about the entry, and stays ignorant of
    /// what any particular compression scheme does with the bytes.
    ///
    /// [`entry`]: Uop::entry
    /// [`gumpart`]: crate::gumpart
    pub fn raw_entry(&self, name: &str) -> Result<Option<RawEntry<'_>>, UopError> {
        let Some(entry) = self.entries.get(&hash_file_name(name.as_bytes())) else {
            return Ok(None);
        };
        let start = entry.data_offset + entry.header_length;
        let bytes = self
            .bytes
            .get(start..start + entry.compressed_length)
            .ok_or_else(|| UopError::Malformed {
                path: self.path.clone(),
                detail: format!("the entry for {name} runs past the end of the file"),
            })?;
        Ok(Some(RawEntry {
            compression: entry.compression,
            decompressed_length: entry.decompressed_length,
            bytes,
        }))
    }
}

/// What the container's own header says about one entry, before `Uop` decides
/// whether it can produce a picture, a map chunk, or anything else from it.
///
/// Returned by [`Uop::raw_entry`] rather than [`Uop::entry`]'s plain `&[u8]`:
/// a caller that has to invert its own container-specific compression needs
/// the flag that says which scheme was used and the length the first stage
/// (always zlib, on every entry this reader has seen) should produce, not
/// just the bytes.
#[derive(Debug)]
pub struct RawEntry<'a> {
    /// The compression flag the container recorded for this entry: `0` for
    /// stored, `3` for every entry `gumpartLegacyMUL.uop` ships. This reader
    /// has never seen another value; a caller seeing something else here is
    /// looking at a format this crate has no reader for yet.
    pub compression: UopCompression,
    /// What zlib alone should produce from `bytes`. For a flag-3 entry this is
    /// an intermediate length, not the final picture data: see
    /// `gumpart`'s module documentation for the second pass that sits on top.
    pub decompressed_length: usize,
    /// The entry's stored bytes, exactly as the container holds them.
    pub bytes: &'a [u8],
}

/// The compression scheme recorded in a UOP entry header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UopCompression(u16);

impl UopCompression {
    /// An entry stored without a second compression pass.
    pub const STORED: Self = Self(0);
    /// zlib followed by the container's gump decompression pass.
    pub const ZLIB_THEN_BWT: Self = Self(3);

    /// The value as it appears in the UOP header.
    pub const fn raw(self) -> u16 {
        self.0
    }
}

/// Read a UOP container and concatenate its entries in index order.
///
/// `path_pattern` is the name the client would have given file `i` — for a map,
/// `"build/map0legacymul/{:08}.dat"`. The container stores only hashes of these,
/// so the name has to be reconstructed and hashed to find anything.
///
/// # Index order is not offset order
///
/// The obvious shortcut is to sort entries by their data offset and skip the
/// hashing entirely. That is **wrong**: a map container's entries need not be
/// written in index order, and concatenating them by offset produces a map that
/// parses cleanly and is scrambled. On a shipped `map0LegacyMUL.uop` index 0
/// lives at byte 17,699,812 and index 1 at byte 512 — see
/// `tests::a_real_containers_entries_are_not_in_offset_order`. The hash is not
/// optional.
///
/// Unlike [`Uop::entry`], a missing index is an error here: the caller is
/// building one continuous file out of the pieces, and a gap in it is a hole in
/// the world.
pub fn read_concatenated(
    path: impl AsRef<Path>,
    path_pattern: &dyn Fn(usize) -> String,
) -> Result<Vec<u8>, UopError> {
    let uop = Uop::open(path)?;

    let mut out = Vec::new();
    for index in 0..uop.file_count {
        let name = path_pattern(index);
        let chunk = uop.entry(&name)?.ok_or_else(|| UopError::MissingEntry {
            path: uop.path.clone(),
            index,
        })?;
        out.extend_from_slice(chunk);
    }
    Ok(out)
}

struct Header {
    first_block: usize,
    file_count: usize,
}

struct Entry {
    data_offset: usize,
    header_length: usize,
    compressed_length: usize,
    /// What zlib alone produces from `compressed_length` bytes at
    /// `data_offset + header_length`. Unread until `raw_entry` needed it —
    /// `entry` never looks past `compressed_length`, since a stored entry's
    /// compressed and decompressed lengths are the same number.
    decompressed_length: usize,
    compression: UopCompression,
}

impl Header {
    fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 28 || read_u32(bytes, 0)? != MAGIC {
            return None;
        }
        Some(Self {
            first_block: usize::try_from(read_u64(bytes, 12)?).ok()?,
            file_count: usize::try_from(read_u32(bytes, 24)?).ok()?,
        })
    }

    /// Walk the block chain and collect every entry, keyed by its name hash.
    fn entries(&self, bytes: &[u8], path: &Path) -> Result<HashMap<u64, Entry>, UopError> {
        let malformed = |detail: &str| UopError::Malformed {
            path: path.to_owned(),
            detail: detail.to_owned(),
        };

        let mut entries = HashMap::with_capacity(self.file_count);
        let mut block = self.first_block;
        // A corrupt file can point a block at itself. Bound the walk by the
        // file's own size rather than trusting it to terminate.
        let mut budget = bytes.len() / ENTRY_BYTES + 1;

        while block != 0 {
            budget = budget
                .checked_sub(1)
                .ok_or_else(|| malformed("the block chain does not terminate"))?;

            let count = read_u32(bytes, block).ok_or_else(|| malformed("truncated block"))? as usize;
            let next = read_u64(bytes, block + 4).ok_or_else(|| malformed("truncated block"))?;

            for index in 0..count {
                let at = block + 12 + index * ENTRY_BYTES;
                let data_offset = read_u64(bytes, at).ok_or_else(|| malformed("truncated entry"))?;
                // A zero offset is a free slot, not a file. Every container has
                // them: blocks are allocated in fixed-size chunks.
                if data_offset == 0 {
                    continue;
                }
                let entry = Entry {
                    data_offset: usize::try_from(data_offset)
                        .map_err(|_| malformed("entry offset does not fit in memory"))?,
                    header_length: read_u32(bytes, at + 8).ok_or_else(|| malformed("truncated entry"))?
                        as usize,
                    compressed_length: read_u32(bytes, at + 12).ok_or_else(|| malformed("truncated entry"))?
                        as usize,
                    decompressed_length: read_u32(bytes, at + 16)
                        .ok_or_else(|| malformed("truncated entry"))?
                        as usize,
                    compression: UopCompression(
                        read_u16(bytes, at + 32).ok_or_else(|| malformed("truncated entry"))?,
                    ),
                };
                let hash = read_u64(bytes, at + 20).ok_or_else(|| malformed("truncated entry"))?;
                entries.insert(hash, entry);
            }

            block = usize::try_from(next).map_err(|_| malformed("block offset out of range"))?;
        }
        Ok(entries)
    }
}

/// The hash UOP uses to name its entries.
///
/// Bob Jenkins' `hashlittle2` from lookup3, with one twist that is not written
/// down anywhere: the two 32-bit outputs are packed as `(b << 32) | c`.
/// Jenkins' own signature is `hashlittle2(key, len, &pc, &pb)` — `pc` first —
/// so the natural reading is `(c << 32) | b`, and that matches zero entries.
///
/// The mix is verbatim lookup3 and is checked against Jenkins' own test vector
/// in the tests below, which is what separates "the hash is wrong" from "the
/// packing is wrong". They look identical from the outside: nothing matches.
fn hash_file_name(name: &[u8]) -> u64 {
    let (b, c) = hash_little2(name);
    (u64::from(b) << 32) | u64::from(c)
}

/// lookup3's `hashlittle2`, returning `(b, c)`.
fn hash_little2(key: &[u8]) -> (u32, u32) {
    let mut a = 0xDEAD_BEEFu32.wrapping_add(key.len() as u32);
    let mut b = a;
    let mut c = a;

    let mut rest = key;
    while rest.len() > 12 {
        a = a.wrapping_add(le_u32(rest, 0));
        b = b.wrapping_add(le_u32(rest, 4));
        c = c.wrapping_add(le_u32(rest, 8));

        // lookup3's mix().
        a = a.wrapping_sub(c);
        a ^= c.rotate_left(4);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= a.rotate_left(6);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= b.rotate_left(8);
        b = b.wrapping_add(a);
        a = a.wrapping_sub(c);
        a ^= c.rotate_left(16);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= a.rotate_left(19);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= b.rotate_left(4);
        b = b.wrapping_add(a);

        rest = &rest[12..];
    }

    if !rest.is_empty() {
        // The reference switches on the remaining length and ORs bytes in.
        // Zero-padding to twelve is the same thing on a little-endian read, and
        // it is a great deal harder to get wrong.
        let mut tail = [0u8; 12];
        tail[..rest.len()].copy_from_slice(rest);
        a = a.wrapping_add(le_u32(&tail, 0));
        b = b.wrapping_add(le_u32(&tail, 4));
        c = c.wrapping_add(le_u32(&tail, 8));

        // lookup3's final().
        c ^= b;
        c = c.wrapping_sub(b.rotate_left(14));
        a ^= c;
        a = a.wrapping_sub(c.rotate_left(11));
        b ^= a;
        b = b.wrapping_sub(a.rotate_left(25));
        c ^= b;
        c = c.wrapping_sub(b.rotate_left(16));
        a ^= c;
        a = a.wrapping_sub(c.rotate_left(4));
        b ^= a;
        b = b.wrapping_sub(a.rotate_left(14));
        c ^= b;
        c = c.wrapping_sub(b.rotate_left(24));
    }

    (b, c)
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mix_matches_jenkins_own_test_vector() {
        // This is what tells "my lookup3 is wrong" apart from "my packing is
        // wrong". Both present as: nothing matches, at all, ever.
        let (_, c) = hash_little2(b"Four score and seven years ago");
        assert_eq!(c, 0x1777_0551, "lookup3's published vector");

        let (_, c) = hash_little2(b"");
        assert_eq!(c, 0xDEAD_BEEF, "an empty key is the seed untouched");
    }

    #[test]
    fn keys_at_every_length_around_the_block_boundary_hash() {
        // The loop runs while len > 12, so 12 and 13 take different paths.
        // A key of exactly 12 skipping the final mix would be silently wrong.
        for length in 0..40usize {
            let key = vec![b'a'; length];
            let (b, c) = hash_little2(&key);
            let packed = hash_file_name(&key);
            assert_eq!(packed, (u64::from(b) << 32) | u64::from(c));
        }
    }

    #[test]
    fn the_packing_is_b_then_c() {
        // Documented because it is the one thing about UOP that is not lookup3
        // and not written down. `(c << 32) | b` — the reading Jenkins'
        // parameter order suggests — matches nothing in a real container.
        let (b, c) = hash_little2(b"build/map0legacymul/00000000.dat");
        assert_eq!(
            hash_file_name(b"build/map0legacymul/00000000.dat"),
            (u64::from(b) << 32) | u64::from(c)
        );
        assert_ne!(b, c, "the two halves differ, so the order is observable");
    }

    #[test]
    fn a_file_that_is_not_uop_is_refused() {
        assert!(Header::parse(&[]).is_none());
        assert!(Header::parse(&[0u8; 28]).is_none());
        assert!(Header::parse(b"NOPE").is_none());

        let mut good = vec![0u8; 28];
        good[..4].copy_from_slice(&MAGIC.to_le_bytes());
        assert!(Header::parse(&good).is_some());
    }

    /// Point `OPENSHARD_CLIENT` at a UO client install to run the tests below.
    ///
    /// They skip when it is unset: a test that needs two gigabytes of
    /// copyrighted files cannot be a test everyone runs, and there is no path
    /// that is correct on two machines.
    fn client_dir() -> Option<PathBuf> {
        let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
        dir.join("map0LegacyMUL.uop").exists().then_some(dir)
    }

    #[test]
    fn a_real_containers_entries_are_not_in_offset_order() {
        // The claim `read_concatenated` is built on, checked against a shipped
        // container rather than reasoning. Sorting entries by offset and
        // skipping the hash is the obvious shortcut, and on this file it is
        // catastrophic: index 0 lives at byte 17,699,812 and index 1 at byte
        // 512, so the shortcut would put the second chunk of Felucca first and
        // the map would parse perfectly and be scrambled.
        //
        // If a future client ever did write its entries in index order, this
        // test failing is the good outcome: it would mean the shortcut had
        // become tempting again on the very file that disproves it.
        let Some(dir) = client_dir() else {
            return;
        };
        let path = dir.join("map0LegacyMUL.uop");
        let bytes = std::fs::read(&path).unwrap();
        let header = Header::parse(&bytes).expect("a shipped map container is a UOP");
        let entries = header.entries(&bytes, &path).unwrap();

        assert!(header.file_count > 1, "a facet is more than one chunk");
        assert_eq!(
            entries.len(),
            header.file_count,
            "every index resolves to exactly one entry, so no chunk is missing or doubled"
        );

        let offsets: Vec<usize> = (0..header.file_count)
            .map(|index| {
                let hash = hash_file_name(format!("build/map0legacymul/{index:08}.dat").as_bytes());
                entries
                    .get(&hash)
                    .unwrap_or_else(|| panic!("index {index} hashes to no entry"))
                    .data_offset
            })
            .collect();

        assert!(
            !offsets.windows(2).all(|pair| pair[0] < pair[1]),
            "entries came out in offset order, which would make the name hash look optional"
        );
    }

    #[test]
    fn every_chunk_of_a_real_facet_is_stored_uncompressed() {
        // `UopError::Compressed` exists because a chunk we cannot inflate is a
        // hole in the world rather than something to skip. It has never fired,
        // and this is what says so about a client someone actually runs — not
        // about the one whose behaviour got the error written.
        let Some(dir) = client_dir() else {
            return;
        };
        let mut checked = 0;
        for facet in 0..=5u8 {
            let path = dir.join(format!("map{facet}LegacyMUL.uop"));
            if !path.exists() {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let header = Header::parse(&bytes).unwrap();
            for entry in header.entries(&bytes, &path).unwrap().values() {
                assert_eq!(
                    entry.compression,
                    UopCompression::STORED,
                    "{} has a compressed chunk",
                    path.display()
                );
                checked += 1;
            }
        }
        // Without this the test passes just as happily on a directory of no
        // containers at all.
        assert!(checked > 100, "only {checked} chunks were examined");
    }

    #[test]
    fn a_block_chain_that_loops_is_refused_rather_than_hanging() {
        // A corrupt container can point a block at itself. Without a bound this
        // is an infinite loop inside a server's startup.
        let mut bytes = vec![0u8; 128];
        bytes[..4].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[12..20].copy_from_slice(&64u64.to_le_bytes()); // first block at 64
        bytes[24..28].copy_from_slice(&1u32.to_le_bytes()); // one file
        // Block at 64: zero entries, next block = itself.
        bytes[64..68].copy_from_slice(&0u32.to_le_bytes());
        bytes[68..76].copy_from_slice(&64u64.to_le_bytes());

        let header = Header::parse(&bytes).unwrap();
        let result = header.entries(&bytes, Path::new("loop.uop"));
        assert!(matches!(result, Err(UopError::Malformed { .. })));
    }
}
