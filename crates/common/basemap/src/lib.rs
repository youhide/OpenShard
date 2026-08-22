//! The base set: one imported facet, as chunks, in one file.
//!
//! **This is where our own format meets a path.** `openshard-map` holds the
//! world and encodes a chunk to bytes; it opens nothing. This crate is the
//! other half — the file those bytes go in, the table that says where each one
//! starts, and the read that turns the whole thing back into a facet.
//!
//! It has never heard of a UO install. `openshard_uofiles::map` is what turns a
//! client's files into a facet, and `openshard-map-import` is the binary that
//! chains the two: read a facet from an install, write it here. After that the
//! shard needs neither the install nor that importer.
//!
//! # Base, in `mechanics.md`'s sense
//!
//! The world as imported, immutable. One bake of a UO facet, or one generated
//! world. It never changes, which is what makes a change describable — the
//! patches that will lie over it are direction C's, and nothing in this file
//! format has to move to make room for them.
//!
//! # The file
//!
//! ```text
//! header, 26 bytes
//!   0  4  magic "OSBS"
//!   4  1  version
//!   5  1  facet
//!   6  8  revision            u64
//!  14  4  blocks wide         u32
//!  18  4  blocks down         u32
//!  22  4  chunk count         u32
//! table, (count + 1) x u64 -- where each chunk's bytes start, and where the
//!                             last one ends
//! chunks, each `openshard_map::codec`'s canonical bytes, in the order
//!         `openshard_map::chunk::chunks_of` gives them
//! ```
//!
//! The table is redundant today: a chunk's own header says how long it is, so a
//! whole-facet read could walk them. It is here because
//! `docs/map/new_map_representation/plan.md`'s direction G — chunks fetched on
//! approach and dropped behind — is a seek and a read away with it and a full
//! scan away without it, and 57 KiB on a Felucca-sized facet is not a reason to
//! close that door.
//!
//! The facet's size in blocks is in the header rather than derived from the
//! chunks, for the reason [`openshard_map::chunk::assemble`] asks for it: a set
//! missing its last chunk column would otherwise assemble happily into a
//! narrower world, and a narrower world parses perfectly.

use std::io::Write;
use std::path::{Path, PathBuf};

use openshard_map::chunk::{self, AssemblyError, Chunk};
use openshard_map::codec::{self, DecodeError};
use openshard_map::grid::BlockExtent;
use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_protocol::world::Facet;

/// What every base set starts with.
const MAGIC: [u8; 4] = *b"OSBS";

/// The layout this module writes and the only one it reads.
const VERSION: u8 = 1;

/// Bytes before the table.
const HEADER_BYTES: usize = 26;

/// Bytes a table entry takes.
const ENTRY_BYTES: usize = 8;

/// Where a facet's base set lives by default.
///
/// Beside the shard rather than beside the client's files, unlike the
/// navigation bake: a base set is **the world**, not something derived from an
/// install that a rebuild could throw away.
#[must_use]
pub fn default_path(facet: Facet) -> PathBuf {
    PathBuf::from(format!("openshard-map-{}.osbase", facet.0))
}

/// A base set could not be read or written.
#[derive(Debug)]
#[non_exhaustive]
pub enum BaseError {
    /// The file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file could not be written.
    Write {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file does not begin with the magic.
    NotABaseSet {
        /// Which file.
        path: PathBuf,
    },
    /// The file is a base set of a layout this build does not read.
    Version {
        /// Which file.
        path: PathBuf,
        /// What it says it is.
        found: u8,
    },
    /// The file ends before the header or the table it describes does.
    Truncated {
        /// Which file.
        path: PathBuf,
        /// How long it has to be to hold what its header describes.
        wanted: usize,
        /// How long it is.
        found: usize,
    },
    /// The table does not describe a run of chunks inside the file.
    ///
    /// Offsets have to rise and stay within the file. One that does not is a
    /// truncated download or a file two writers interleaved, and following it
    /// would hand a decoder somebody else's bytes.
    BadTable {
        /// Which file.
        path: PathBuf,
        /// The entry that broke the run.
        at: usize,
    },
    /// The header says the facet holds a number of chunks its size does not.
    CountMismatch {
        /// Which file.
        path: PathBuf,
        /// What the facet's size in blocks comes to.
        wanted: usize,
        /// What the header claims.
        found: usize,
    },
    /// One of the chunks is not a chunk.
    Chunk {
        /// Which file.
        path: PathBuf,
        /// Which chunk of it, counted in the file's own order.
        at: usize,
        /// Why.
        source: DecodeError,
    },
    /// The chunks do not make one facet.
    Assembly {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: AssemblyError,
    },
}

impl std::fmt::Display for BaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Write { path, source } => write!(f, "cannot write {}: {source}", path.display()),
            Self::NotABaseSet { path } => {
                write!(
                    f,
                    "{} does not begin with OSBS, so it is not a base set",
                    path.display()
                )
            }
            Self::Version { path, found } => write!(
                f,
                "{} is a version {found} base set, and this build reads version {VERSION}",
                path.display()
            ),
            Self::Truncated { path, wanted, found } => {
                write!(f, "{} describes {wanted} bytes and is {found}", path.display())
            }
            Self::BadTable { path, at } => write!(
                f,
                "{}: table entry {at} does not point inside the file after the one before it",
                path.display()
            ),
            Self::CountMismatch { path, wanted, found } => write!(
                f,
                "{} holds {found} chunks, and a facet of the size in its header has {wanted}",
                path.display()
            ),
            Self::Chunk { path, at, source } => {
                write!(f, "{}: chunk {at} is not one: {source}", path.display())
            }
            Self::Assembly { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for BaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Chunk { source, .. } => Some(source),
            Self::Assembly { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// What a write turned out to be, for a caller that wants to report it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Written {
    /// How many chunks the facet cut into.
    pub chunks: usize,
    /// How many statics they hold between them.
    pub statics: usize,
    /// How long the file is.
    pub bytes: usize,
}

/// Write a published facet out as a base set.
///
/// The chunks go in [`chunk::chunks_of`]'s order, which is the order a reader
/// walks them in — so the table and the file agree without either having to
/// say so.
///
/// # Errors
///
/// [`BaseError::Write`] if the file cannot be written.
pub fn write(path: impl AsRef<Path>, snapshot: &MapSnapshot) -> Result<Written, BaseError> {
    let path = path.as_ref();
    let extent = snapshot.map().extent();

    // Encoded up front rather than streamed: the table has to be written before
    // the chunks, and it cannot be filled in until every length is known. A
    // facet is about 110 MiB of blobs, which is less than loading one costs.
    let mut blobs = Vec::new();
    let mut statics = 0;
    for at in chunk::chunks_of(extent) {
        let chunk = Chunk::of(snapshot, at).expect("a chunk of the facet it was cut from");
        statics += chunk.static_count();
        blobs.push(codec::encode(&chunk));
    }

    let table_bytes = (blobs.len() + 1) * ENTRY_BYTES;
    let mut out = Vec::with_capacity(HEADER_BYTES + table_bytes);
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(snapshot.facet().0);
    out.extend_from_slice(&snapshot.revision().get().to_le_bytes());
    out.extend_from_slice(&extent.wide.to_le_bytes());
    out.extend_from_slice(&extent.down.to_le_bytes());
    out.extend_from_slice(&(blobs.len() as u32).to_le_bytes());

    let mut offset = (HEADER_BYTES + table_bytes) as u64;
    out.extend_from_slice(&offset.to_le_bytes());
    for blob in &blobs {
        offset += blob.len() as u64;
        out.extend_from_slice(&offset.to_le_bytes());
    }

    let bytes = offset as usize;
    let write = |out: &[u8], blobs: &[Vec<u8>]| -> std::io::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut file = std::io::BufWriter::new(file);
        file.write_all(out)?;
        for blob in blobs {
            file.write_all(blob)?;
        }
        file.flush()
    };
    write(&out, &blobs).map_err(|source| BaseError::Write {
        path: path.to_owned(),
        source,
    })?;

    Ok(Written {
        chunks: blobs.len(),
        statics,
        bytes,
    })
}

/// Read a base set back into the facet it was written from.
///
/// The facet arrives at the revision the file recorded, not at a fresh one:
/// this is a world being read back, and a reader that minted its own revision
/// could claim agreement with a snapshot it never saw.
///
/// # Errors
///
/// [`BaseError`], one variant per way a file fails to be one facet.
pub fn read(path: impl AsRef<Path>) -> Result<MapSnapshot, BaseError> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|source| BaseError::Read {
        path: path.to_owned(),
        source,
    })?;

    if bytes.len() < HEADER_BYTES || bytes[..4] != MAGIC {
        return Err(BaseError::NotABaseSet {
            path: path.to_owned(),
        });
    }
    if bytes[4] != VERSION {
        return Err(BaseError::Version {
            path: path.to_owned(),
            found: bytes[4],
        });
    }

    let facet = Facet(bytes[5]);
    let revision = MapRevision::decoded(u64::from_le_bytes(bytes[6..14].try_into().expect("eight bytes")));
    let extent = BlockExtent {
        wide: u32::from_le_bytes(bytes[14..18].try_into().expect("four bytes")),
        down: u32::from_le_bytes(bytes[18..22].try_into().expect("four bytes")),
    };
    let count = u32::from_le_bytes(bytes[22..26].try_into().expect("four bytes")) as usize;

    // The count the facet's own size implies, checked before the table is read:
    // a header that disagrees with itself is not a file to go looking inside.
    let wanted = chunk::chunks_of(extent).count();
    if wanted != count {
        return Err(BaseError::CountMismatch {
            path: path.to_owned(),
            wanted,
            found: count,
        });
    }

    let table_end = HEADER_BYTES + (count + 1) * ENTRY_BYTES;
    if bytes.len() < table_end {
        return Err(BaseError::Truncated {
            path: path.to_owned(),
            wanted: table_end,
            found: bytes.len(),
        });
    }
    let table: Vec<u64> = bytes[HEADER_BYTES..table_end]
        .chunks_exact(ENTRY_BYTES)
        .map(|entry| u64::from_le_bytes(entry.try_into().expect("eight bytes")))
        .collect();

    // Every entry rises and stays inside the file, checked before any of them
    // is used to slice: an offset that ran backwards would hand a decoder the
    // bytes of the chunk before it, which decode perfectly.
    for at in 0..table.len() {
        let ok = table[at] >= table_end as u64
            && table[at] <= bytes.len() as u64
            && (at == 0 || table[at] >= table[at - 1]);
        if !ok {
            return Err(BaseError::BadTable {
                path: path.to_owned(),
                at,
            });
        }
    }

    let mut chunks = Vec::with_capacity(count);
    for at in 0..count {
        let blob = &bytes[table[at] as usize..table[at + 1] as usize];
        chunks.push(codec::decode(blob).map_err(|source| BaseError::Chunk {
            path: path.to_owned(),
            at,
            source,
        })?);
    }

    let map = chunk::assemble(facet, extent, &chunks).map_err(|source| BaseError::Assembly {
        path: path.to_owned(),
        source,
    })?;
    Ok(MapSnapshot::restored(facet, revision, map))
}
