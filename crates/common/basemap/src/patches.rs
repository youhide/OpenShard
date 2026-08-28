//! The patch log: every change committed over one base set, in the order it
//! was committed.
//!
//! The base set is the immutable half of a world and this is the other one. A
//! log is **append-only** — a revert is a new patch rather than a rewritten
//! history, so nothing here ever goes back and edits a record.
//!
//! # Where it lives
//!
//! Beside the base set, at the same path with `.ospatch` for an extension.
//! Derived, where `world.base_sets` deliberately derives nothing — and the
//! difference is what is being named. `base_sets` names *which world* a facet
//! is, and guessing that from a convention is a shard silently running the
//! wrong one. A patch log is not another world: it is the rest of the one
//! already named, and a base set without its log is a world missing its edits.
//! Two files that must travel together are better joined by a rule than by a
//! second line of configuration an operator can forget.
//!
//! # The file
//!
//! ```text
//! header, 14 bytes
//!   0  4  magic "OSPL"
//!   4  1  version
//!   5  1  facet
//!   6  8  base revision      u64 -- the base set this log lies over
//! records, in order, each:
//!   0  4  payload length     u32
//!   4  4  FNV-1a of it       u32
//!   8  .. one `openshard_map::codec` patch record
//! ```
//!
//! **The base revision is in the header**, so a log that belongs to another
//! world is refused by two bytes and a compare rather than by the first op that
//! happens to disagree with a tile. [`openshard_map::patch`]'s `was` fields are
//! the second net under that, and the parent revision of the first record is a
//! third — a wrong log is worth catching three times, because the failure it
//! prevents is a shard running a world nobody built.
//!
//! **Each record is framed and checksummed** because a log is written to a
//! little at a time over a shard's life, and the one place a file like that
//! goes wrong is its tail. The checksum is FNV-1a: an integrity check against a
//! torn write, not a defence against anyone — a log an attacker can write to is
//! a world an attacker can rewrite, and no hash here changes that.
//!
//! # A torn tail is refused, not trimmed
//!
//! A crash between the length and the payload leaves a record that is not one.
//! This module refuses the file and names the record, rather than dropping the
//! tail: a dropped tail is an edit an operator was told had been published,
//! silently gone. The safe version of trimming needs the publisher to have
//! flushed the record before acting on it — at which point a torn tail is
//! provably an *unacknowledged* patch — and that discipline belongs with the
//! live publish that C2 builds.

use std::io::Write;
use std::path::{Path, PathBuf};

use openshard_map::codec::{self, PatchDecodeError};
use openshard_map::patch::Patch;
use openshard_map::snapshot::MapRevision;
use openshard_protocol::world::Facet;

/// What every patch log starts with.
const MAGIC: [u8; 4] = *b"OSPL";

/// The layout this module writes and the only one it reads.
const VERSION: u8 = 1;

/// Bytes before the first record.
const HEADER_BYTES: usize = 14;

/// Bytes a record's frame takes: the payload's length and its checksum.
const FRAME_BYTES: usize = 8;

/// The extension a patch log takes, beside the base set it lies over.
pub const EXTENSION: &str = "ospatch";

/// Where the log of a base set is.
///
/// The same path with a different extension, which is the rule the module
/// header argues for. A base set with no extension at all still gets one.
#[must_use]
pub fn log_path(base_set: &Path) -> PathBuf {
    base_set.with_extension(EXTENSION)
}

/// A patch log could not be read or written.
#[derive(Debug)]
#[non_exhaustive]
pub enum LogError {
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
    NotALog {
        /// Which file.
        path: PathBuf,
    },
    /// The file is a log of a layout this build does not read.
    Version {
        /// Which file.
        path: PathBuf,
        /// What it says it is.
        found: u8,
    },
    /// The log is of a different facet than the base set it was found beside.
    WrongFacet {
        /// Which file.
        path: PathBuf,
        /// The facet the base set is.
        wanted: Facet,
        /// The facet the log says it is for.
        found: Facet,
    },
    /// The log was written over a different revision of the world.
    ///
    /// A re-imported facet is revision 1 again, so this catches the *shape* of
    /// the mistake rather than every instance of it — which is why the ops
    /// carry what they replace as well.
    WrongBase {
        /// Which file.
        path: PathBuf,
        /// The revision the base set is at.
        wanted: MapRevision,
        /// The revision the log says it lies over.
        found: MapRevision,
    },
    /// The file ends in the middle of a record.
    Truncated {
        /// Which file.
        path: PathBuf,
        /// Which record, counted from zero.
        at: usize,
        /// How many bytes that record wants.
        wanted: usize,
        /// How many are left.
        found: usize,
    },
    /// A record's bytes are not the bytes its checksum was taken over.
    Corrupt {
        /// Which file.
        path: PathBuf,
        /// Which record.
        at: usize,
    },
    /// A record is not a patch.
    Patch {
        /// Which file.
        path: PathBuf,
        /// Which record.
        at: usize,
        /// Why it is not one.
        source: PatchDecodeError,
    },
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "reading patch log {}: {source}", path.display()),
            Self::Write { path, source } => write!(f, "writing patch log {}: {source}", path.display()),
            Self::NotALog { path } => {
                write!(
                    f,
                    "{} is not a patch log: it does not begin with OSPL",
                    path.display()
                )
            }
            Self::Version { path, found } => write!(
                f,
                "{} is a version {found} patch log, and this build reads version {VERSION}",
                path.display()
            ),
            Self::WrongFacet { path, wanted, found } => write!(
                f,
                "patch log {} is for facet {}, and it lies beside a base set of facet {}",
                path.display(),
                found.0,
                wanted.0
            ),
            Self::WrongBase { path, wanted, found } => write!(
                f,
                "patch log {} was written over revision {} of this facet, and the base set beside \
                 it is revision {}",
                path.display(),
                found.get(),
                wanted.get()
            ),
            Self::Truncated {
                path,
                at,
                wanted,
                found,
            } => write!(
                f,
                "patch log {} ends inside record {at}, which wants {wanted} bytes and has {found}",
                path.display()
            ),
            Self::Corrupt { path, at } => write!(
                f,
                "record {at} of patch log {} does not match its checksum",
                path.display()
            ),
            Self::Patch { path, at, source } => write!(
                f,
                "record {at} of patch log {} is not a patch: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for LogError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::Patch { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Read every patch committed over a base set, in order.
///
/// `facet` and `base` are what the base set beside it said, and the header is
/// checked against both before a record is read. `Ok(Vec::new())` for a file
/// that is not there: a world nobody has edited yet is a world with no log,
/// not a broken one.
///
/// Whether the patches *apply* is not asked here — that is
/// [`MapSnapshot::publish`](openshard_map::snapshot::MapSnapshot::publish)'s
/// question about a world, and this function has only a file.
///
/// # Errors
///
/// [`LogError`], one variant per way a file fails to be one log.
pub fn read(path: &Path, facet: Facet, base: MapRevision) -> Result<Vec<Patch>, LogError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(LogError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };

    check_header(path, &bytes, facet, base)?;
    read_records(path, &bytes[HEADER_BYTES..])
}

/// Check that a header belongs to the base set beside the log.
fn check_header(path: &Path, header: &[u8], facet: Facet, base: MapRevision) -> Result<(), LogError> {
    if header.len() < HEADER_BYTES || header[..4] != MAGIC {
        return Err(LogError::NotALog {
            path: path.to_owned(),
        });
    }
    if header[4] != VERSION {
        return Err(LogError::Version {
            path: path.to_owned(),
            found: header[4],
        });
    }
    let found = Facet(header[5]);
    if found != facet {
        return Err(LogError::WrongFacet {
            path: path.to_owned(),
            wanted: facet,
            found,
        });
    }
    let over = MapRevision::decoded(u64::from_le_bytes(header[6..14].try_into().expect("eight bytes")));
    if over != base {
        return Err(LogError::WrongBase {
            path: path.to_owned(),
            wanted: base,
            found: over,
        });
    }
    Ok(())
}

/// Decode each length-and-checksum frame after a validated header.
fn read_records(path: &Path, bytes: &[u8]) -> Result<Vec<Patch>, LogError> {
    let mut patches = Vec::new();
    let mut read = 0;
    while read < bytes.len() {
        let at = patches.len();
        let frame = bytes
            .get(read..read + FRAME_BYTES)
            .ok_or_else(|| LogError::Truncated {
                path: path.to_owned(),
                at,
                wanted: FRAME_BYTES,
                found: bytes.len() - read,
            })?;
        let length = u32::from_le_bytes(frame[..4].try_into().expect("four bytes")) as usize;
        let checksum = u32::from_le_bytes(frame[4..].try_into().expect("four bytes"));

        let from = read + FRAME_BYTES;
        let payload = bytes
            .get(from..from + length)
            .ok_or_else(|| LogError::Truncated {
                path: path.to_owned(),
                at,
                wanted: FRAME_BYTES + length,
                found: bytes.len() - read,
            })?;
        if fnv1a(payload) != checksum {
            return Err(LogError::Corrupt {
                path: path.to_owned(),
                at,
            });
        }
        patches.push(codec::decode_patch(payload).map_err(|source| LogError::Patch {
            path: path.to_owned(),
            at,
            source,
        })?);
        read = from + length;
    }
    Ok(patches)
}

/// Commit one patch to the end of a log, creating the log if it is not there.
///
/// The header is written from `facet` and `base` on creation and checked
/// against them on every later append, so a patch cannot be committed to the
/// log of another world. What is *not* checked here is that the patch follows
/// the one before it: a caller appends a patch it has already published to the
/// snapshot in hand, and publishing is what proved the parent.
///
/// # Errors
///
/// [`LogError`] — the log could not be written, or the file already there is
/// not this world's log.
pub fn append(path: &Path, facet: Facet, base: MapRevision, patch: &Patch) -> Result<(), LogError> {
    if !path.exists() {
        let mut header = Vec::with_capacity(HEADER_BYTES);
        header.extend_from_slice(&MAGIC);
        header.push(VERSION);
        header.push(facet.0);
        header.extend_from_slice(&base.get().to_le_bytes());
        std::fs::write(path, &header).map_err(|source| LogError::Write {
            path: path.to_owned(),
            source,
        })?;
    } else {
        // The header of the file we are about to write into, checked against
        // the world in hand. Reading the whole log back would be the same check
        // and a hundred records of work; the header is what carries the
        // identity.
        read_header(path, facet, base)?;
    }

    let payload = codec::encode_patch(patch);
    let mut record = Vec::with_capacity(FRAME_BYTES + payload.len());
    record.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    record.extend_from_slice(&fnv1a(&payload).to_le_bytes());
    record.extend_from_slice(&payload);

    let append = |frame: &[u8]| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new().append(true).open(path)?;
        // One `write_all` and one flush, so the frame and its payload go out
        // together as far as anything below here allows.
        file.write_all(frame)?;
        file.flush()
    };
    append(&record).map_err(|source| LogError::Write {
        path: path.to_owned(),
        source,
    })
}

/// The identity in a log's header, checked against the world it is being
/// appended to.
fn read_header(path: &Path, facet: Facet, base: MapRevision) -> Result<(), LogError> {
    let mut header = [0u8; HEADER_BYTES];
    read_exact(path, &mut header)?;
    check_header(path, &header, facet, base)
}

/// The first bytes of a file, or the error of a file too short to have them.
fn read_exact(path: &Path, into: &mut [u8]) -> Result<(), LogError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|source| LogError::Read {
        path: path.to_owned(),
        source,
    })?;
    file.read_exact(into).map_err(|source| {
        if source.kind() == std::io::ErrorKind::UnexpectedEof {
            LogError::NotALog {
                path: path.to_owned(),
            }
        } else {
            LogError::Read {
                path: path.to_owned(),
                source,
            }
        }
    })
}

/// FNV-1a, 32 bits.
///
/// An integrity check over one record, written out here rather than pulled in:
/// it is six lines, it has no configuration, and a log is not a place where a
/// hash has to be resistant to anybody. See the module header.
fn fnv1a(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}
