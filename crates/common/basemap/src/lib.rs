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
//! world. It never changes, which is what makes a change describable.
//!
//! The changes are in [`patches`], the append-only log beside it, and nothing
//! in the base set's own format moved to make room for them. **[`load`] is the
//! pair**: a base set plus its log, resolved to the revision the last patch
//! produced. It is the one door to a world of ours, because a shard and an
//! offline bake that resolved a facet differently would stamp a graph against a
//! world nobody built.
//!
//! # The file
//!
//! ```text
//! header, 34 bytes
//!   0  4  magic "OSBS"
//!   4  1  version
//!   5  1  facet
//!   6  8  revision            u64
//!  14  4  blocks wide         u32
//!  18  4  blocks down         u32
//!  22  4  chunk count         u32
//!  26  8  world identity      u64
//! table, (count + 1) x u64 -- where each chunk's bytes start, and where the
//!                             last one ends
//! manifest, count x 12    -- per chunk: the hash of its record, u64, and how
//!                            long that record is once inflated, u32
//! chunks, each `openshard_map::codec`'s canonical bytes, deflated whole, in the
//!         order `openshard_map::chunk::chunks_of` gives them
//! ```
//!
//! The table is redundant today: a chunk's own header says how long it is, so a
//! whole-facet read could walk them. It is here because
//! `docs/map/new_map_representation/plan.md`'s direction G — chunks fetched on
//! approach and dropped behind — is a seek and a read away with it and a full
//! scan away without it, and 57 KiB on a Felucca-sized facet is not a reason to
//! close that door.
//!
//! # What version 2 added, and why all three at once
//!
//! `docs/map/new_map_representation/what_a_change_costs.md`'s S1. A version byte
//! names a layout, so three changes that were each worth a bump are one bump:
//!
//! - **The chunks are deflated.** 107,528,650 bytes of Felucca become
//!   29,698,618 on the same content, through the pair
//!   [`openshard_protocol::chunks::deflate`] already carries the wire's chunks
//!   through — at [`openshard_protocol::chunks::DeflateLevel::BASE_SET`] rather
//!   than the wire's level, and that type is where the measurement that chose it
//!   lives. The client's cache is the caller that wanted it most: it keeps a
//!   whole facet per world it has seen.
//! - **The manifest carries a hash per chunk.** It is what makes "did *this*
//!   square move" answerable without re-encoding a facet, which is what a
//!   product keyed by the chunk it was built from needs (S2). At 64 tiles a
//!   chunk it costs 84 KiB on Felucca; the argument that refused a manifest at
//!   8×8 was 17.5 MiB, a ninth of the set it indexes, and it does not survive
//!   the chunk size that was chosen. It is also read *back*: a chunk whose bytes
//!   do not hash to what the manifest says is refused by name rather than
//!   decoded into a plausible square.
//! - **The header carries the world's identity.** It used to be a hash of the
//!   whole file taken at boot, which answers E3's question and not S4's: a
//!   squash rewrites a world's bytes without making it a different world, and
//!   every client would have refetched a facet nothing moved in. So the identity
//!   is **minted from the content once** — [`Identity::Mint`] — and **carried**
//!   by every later write of that world.
//!
//! The facet's size in blocks is in the header rather than derived from the
//! chunks, for the reason [`openshard_map::chunk::assemble`] asks for it: a set
//! missing its last chunk column would otherwise assemble happily into a
//! narrower world, and a narrower world parses perfectly.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use openshard_map::chunk::{self, AssemblyError, Chunk};
use openshard_map::codec::{self, DecodeError};
use openshard_map::grid::BlockExtent;
use openshard_map::patch::PatchError;
use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_protocol::chunks::InflatedLength;
use openshard_protocol::world::{Facet, WorldId};

pub mod patches;

/// What every base set starts with.
const MAGIC: [u8; 4] = *b"OSBS";

/// The layout this module writes and the only one it reads.
///
/// Version 1 is refused rather than converted, and that is not a hardship: a
/// base set is a bake of an install or an export of a world, so there is nothing
/// in one that is not reproducible by writing it again. A converter would be a
/// second write path with no second caller.
const VERSION: u8 = 2;

/// Bytes before the table.
const HEADER_BYTES: usize = 34;

/// Bytes a table entry takes.
const ENTRY_BYTES: usize = 8;

/// Bytes a manifest entry takes: the record's hash, and its inflated length.
const MANIFEST_BYTES: usize = 12;

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
    /// One of the chunks is not the deflate stream the manifest says it is.
    NotDeflated {
        /// Which file.
        path: PathBuf,
        /// Which chunk of it, counted in the file's own order.
        at: usize,
    },
    /// One of the chunks does not hash to what the manifest says it does.
    ///
    /// The manifest is written from the same bytes the chunk is, so this is a
    /// file that changed under itself — a torn write, a half-finished copy, two
    /// writers interleaved. It is refused here rather than decoded, because a
    /// chunk record with a byte moved in it decodes into a *plausible* square
    /// and there is nothing downstream that could tell.
    HashMismatch {
        /// Which file.
        path: PathBuf,
        /// Which chunk of it, counted in the file's own order.
        at: usize,
        /// What the manifest claims the record hashes to.
        wanted: u64,
        /// What it hashes to.
        found: u64,
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
    /// The patch log beside the base set could not be read.
    Log {
        /// Why.
        source: patches::LogError,
    },
    /// A patch in the log does not apply to the world the ones before it made.
    ///
    /// The log is ordered, so this is a *chain* that does not hold: a record
    /// out of order, a record made against a world that was never published, or
    /// a log that belongs to some other base set the header check let through.
    NotApplied {
        /// Which log.
        path: PathBuf,
        /// Which record of it, counted from zero.
        at: usize,
        /// Why the patch was refused.
        source: PatchError,
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
            Self::NotDeflated { path, at } => write!(
                f,
                "{}: chunk {at} did not inflate to the length its manifest entry declares",
                path.display()
            ),
            Self::HashMismatch {
                path,
                at,
                wanted,
                found,
            } => write!(
                f,
                "{}: chunk {at} hashes to {found:016x} and its manifest entry says {wanted:016x}",
                path.display()
            ),
            Self::Chunk { path, at, source } => {
                write!(f, "{}: chunk {at} is not one: {source}", path.display())
            }
            Self::Assembly { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
            Self::Log { source } => write!(f, "{source}"),
            Self::NotApplied { path, at, source } => {
                write!(f, "patch {at} of {} does not apply: {source}", path.display())
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
            Self::Log { source } => Some(source),
            Self::NotApplied { source, .. } => Some(source),
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
    /// The identity the file went out under — minted here, or the one the
    /// caller carried in.
    pub world: WorldId,
}

/// Whose world the file about to be written is.
///
/// **The one thing a base set cannot work out for itself**, and the reason it is
/// asked rather than derived: a world's identity has to survive the world being
/// rewritten. A squash rewrites every byte of a set without making it a
/// different world, and a client's cache is a copy of a world that belongs to
/// the shard that served it. Both would be a new world under an identity taken
/// from the file that happens to hold them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Identity {
    /// A world being written for the first time: mint one from its content.
    ///
    /// `openshard-map-import` is the caller — an import is where a world
    /// begins — and a generator would be the other one.
    Mint,
    /// A world that already has an identity, being written again.
    ///
    /// The client's chunk cache, and the squash that folds a patch log into a
    /// new base set.
    Keep(WorldId),
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
pub fn write(
    path: impl AsRef<Path>,
    snapshot: &MapSnapshot,
    identity: Identity,
) -> Result<Written, BaseError> {
    let path = path.as_ref();
    let extent = snapshot.map().extent();

    // Encoded up front rather than streamed: the table has to be written before
    // the chunks, and it cannot be filled in until every length is known. A
    // facet is about 110 MiB of records, which is less than loading one costs —
    // and about 22 MiB of them once deflated, which is what is held here.
    let mut blobs = Vec::new();
    let mut manifest = Vec::new();
    let mut statics = 0;
    for at in chunk::chunks_of(extent) {
        let chunk = Chunk::of(snapshot, at).expect("a chunk of the facet it was cut from");
        statics += chunk.static_count();
        let record = codec::encode(&chunk);
        // The hash is of the *record*, not of the deflate stream: what a reader
        // downstream keys on is the chunk's content, and two builds of a
        // compressor are allowed to disagree about the bytes it packs into.
        manifest.push((
            fnv1a64(&record),
            u32::try_from(record.len()).expect("a chunk record of fewer than four billion bytes"),
        ));
        blobs.push(openshard_protocol::chunks::deflate(
            &record,
            openshard_protocol::chunks::DeflateLevel::BASE_SET,
        ));
    }

    let world = match identity {
        Identity::Keep(world) => world,
        Identity::Mint => mint(&manifest),
    };

    let table_bytes = (blobs.len() + 1) * ENTRY_BYTES + blobs.len() * MANIFEST_BYTES;
    let mut out = Vec::with_capacity(HEADER_BYTES + table_bytes);
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(snapshot.facet().0);
    out.extend_from_slice(&snapshot.revision().get().to_le_bytes());
    out.extend_from_slice(&extent.wide.to_le_bytes());
    out.extend_from_slice(&extent.down.to_le_bytes());
    out.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
    out.extend_from_slice(&world.0.to_le_bytes());

    let mut offset = (HEADER_BYTES + table_bytes) as u64;
    out.extend_from_slice(&offset.to_le_bytes());
    for blob in &blobs {
        offset += blob.len() as u64;
        out.extend_from_slice(&offset.to_le_bytes());
    }
    for (hash, inflated) in &manifest {
        out.extend_from_slice(&hash.to_le_bytes());
        out.extend_from_slice(&inflated.to_le_bytes());
    }

    let bytes = offset as usize;
    let write = |header: &[u8], encoded_chunks: &[Vec<u8>]| -> std::io::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut file = std::io::BufWriter::new(file);
        file.write_all(header)?;
        for blob in encoded_chunks {
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
        world,
    })
}

/// Mint a world's identity out of what its chunks hash to.
///
/// **Over the manifest rather than over the file**, and the difference is the
/// whole point: the file holds a compressor's output and a header that carries
/// this very number, so a hash of it could not be minted from inside it and
/// would move with a compressor upgrade. The manifest is content — the same
/// facet imported twice on two machines produces the same hashes in the same
/// order, and therefore the same world.
fn mint(manifest: &[(u64, u32)]) -> WorldId {
    let mut bytes = Vec::with_capacity(manifest.len() * 8);
    for (hash, _) in manifest {
        bytes.extend_from_slice(&hash.to_le_bytes());
    }
    WorldId(fnv1a64(&bytes))
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

    let manifest_at = HEADER_BYTES + (count + 1) * ENTRY_BYTES;
    let table_end = manifest_at + count * MANIFEST_BYTES;
    if bytes.len() < table_end {
        return Err(BaseError::Truncated {
            path: path.to_owned(),
            wanted: table_end,
            found: bytes.len(),
        });
    }
    let table: Vec<u64> = bytes[HEADER_BYTES..manifest_at]
        .chunks_exact(ENTRY_BYTES)
        .map(|entry| u64::from_le_bytes(entry.try_into().expect("eight bytes")))
        .collect();
    let manifest: Vec<(u64, InflatedLength)> = bytes[manifest_at..table_end]
        .chunks_exact(MANIFEST_BYTES)
        .map(|entry| {
            (
                u64::from_le_bytes(entry[..8].try_into().expect("eight bytes")),
                InflatedLength(u32::from_le_bytes(entry[8..].try_into().expect("four bytes"))),
            )
        })
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
        let (wanted, inflated) = manifest[at];
        let Some(record) = openshard_protocol::chunks::inflate(blob, inflated) else {
            return Err(BaseError::NotDeflated {
                path: path.to_owned(),
                at,
            });
        };
        // Before the decode rather than after it: a record with a byte moved in
        // it decodes into a square that is wrong and looks like every other one.
        let found = fnv1a64(&record);
        if found != wanted {
            return Err(BaseError::HashMismatch {
                path: path.to_owned(),
                at,
                wanted,
                found,
            });
        }
        chunks.push(codec::decode(&record).map_err(|source| BaseError::Chunk {
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

/// Which world a base set *is*, by the identity in its header.
///
/// **The question a facet number cannot answer.** Two shards both serving facet
/// 0 serve two different Feluccas and both call the first revision of it 1, so a
/// client that kept a copy of one and compared revisions with the other would
/// draw a world nobody built. A shard tells a client this in its
/// `WorldNotice`, and the client files what it keeps under it — see
/// [`WorldId`], and `docs/map/new_map_representation/to_the_client.md`'s E3.
///
/// **Of the base set alone, and not of the log beside it.** A base set never
/// changes, so this never does; where the world has got to since is the
/// revision, and the two are asked together. The pair separates every world any
/// shard of ours can serve except two logs forked from one base at the same
/// revision — which is a log taken apart by hand, and the append-only rule in
/// [`patches`] is what makes that not a thing that happens.
///
/// **Minted once and carried, rather than taken off the file each time.** It
/// used to be a hash of the whole file, which says the same thing right up until
/// a world is written again: a squash folds a log into a new base set without
/// making it a different world, and a client's cache is somebody else's world in
/// a file of ours. Both would come back under a name nobody had served. What
/// mints it is [`Identity::Mint`] and it is FNV-1a over what the chunks
/// hash to — [`patches`]'s own checksum one width up, and for the same reason:
/// an identity that has to be the same on two machines and in every future build
/// of this engine, not a defence against anybody. A hash from a crate whose
/// value changes with its version would silently orphan every cache on an
/// upgrade.
///
/// Reads the header alone, so it costs a seek where [`read`] costs a facet.
///
/// # Errors
///
/// [`BaseError::Read`] if the file cannot be read,
/// [`BaseError::NotABaseSet`] for a file that is not one — the magic is checked
/// here rather than trusted, because an identity taken over somebody else's file
/// is a name for a world this shard is not serving — and [`BaseError::Version`]
/// for a layout whose header this one cannot read.
pub fn identity_of(base_set: impl AsRef<Path>) -> Result<WorldId, BaseError> {
    let path = base_set.as_ref();
    let mut header = [0_u8; HEADER_BYTES];
    let mut file = std::fs::File::open(path).map_err(|source| BaseError::Read {
        path: path.to_owned(),
        source,
    })?;
    if let Err(source) = file.read_exact(&mut header) {
        // A file too short to hold a header is not one, whatever else it is;
        // anything else went wrong at the disk and says so.
        return Err(if source.kind() == std::io::ErrorKind::UnexpectedEof {
            BaseError::NotABaseSet {
                path: path.to_owned(),
            }
        } else {
            BaseError::Read {
                path: path.to_owned(),
                source,
            }
        });
    }
    if header[..4] != MAGIC {
        return Err(BaseError::NotABaseSet {
            path: path.to_owned(),
        });
    }
    if header[4] != VERSION {
        return Err(BaseError::Version {
            path: path.to_owned(),
            found: header[4],
        });
    }
    Ok(WorldId(u64::from_le_bytes(
        header[26..34].try_into().expect("eight bytes"),
    )))
}

/// FNV-1a, 64 bits.
///
/// The 32-bit twin in [`patches`] is a torn-write check over one record; this is
/// an identity over a whole file, and it is written out here for that module's
/// reason — it is six lines, it has no configuration, and both are pinned to a
/// spelling that cannot change under us.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A facet as it stands: the base set, and every patch committed over it.
///
/// The three fields travel together because a caller that has one wants all
/// three. `openshard-server`'s boot and `openshard-navigation-bake` both stamp
/// what they built over, and a stamp that named the base set alone would
/// validate a graph against a world that has been edited since — the same trap
/// `openshard_movement::bake::stamp_of_base_set` exists to keep a base-set
/// world out of.
#[derive(Debug)]
pub struct Loaded {
    /// The facet, at the revision the last patch produced — or at the base
    /// set's own, if nothing has been committed.
    pub snapshot: MapSnapshot,
    /// The revision the base set itself is at, before any patch.
    ///
    /// What the log's header says it lies over, and what a caller appending a
    /// patch has to name. Carried rather than derived by counting the patches
    /// back off the snapshot's revision: that arithmetic is only right while
    /// one patch means one revision, and it is not a property worth depending
    /// on from outside.
    pub base: MapRevision,
    /// The log the patches came out of, if there is one on disk. `None` is a
    /// world nobody has edited, and it is not the same as an empty log: an
    /// empty log is a file, and a file is an input to stamp.
    pub log: Option<PathBuf>,
    /// How many patches were applied.
    pub patches: usize,
}

/// Read a base set and everything committed over it.
///
/// **The one door to a world of ours**, and the reason it is one: the shard and
/// the navigation bake must resolve a facet identically, down to the revision,
/// or a graph is stamped against a world it was not built from. Two call sites
/// spelling out read-then-apply would be two chances to disagree.
///
/// The patch log is beside the base set — [`patches::log_path`] is the rule and
/// its module header is the argument. A facet with no log comes back at the
/// base set's own revision.
///
/// # Errors
///
/// [`BaseError`] — the base set is not one, the log is not one, or a patch in
/// the log does not apply to the world the patches before it made.
pub fn load(base_set: impl AsRef<Path>) -> Result<Loaded, BaseError> {
    let base_set = base_set.as_ref();
    let mut snapshot = read(base_set)?;
    let base = snapshot.revision();

    let path = patches::log_path(base_set);
    let log = path.exists().then(|| path.clone());
    let committed =
        patches::read(&path, snapshot.facet(), base).map_err(|source| BaseError::Log { source })?;

    for (at, patch) in committed.iter().enumerate() {
        snapshot.publish(patch).map_err(|source| BaseError::NotApplied {
            path: path.clone(),
            at,
            source,
        })?;
    }
    Ok(Loaded {
        snapshot,
        base,
        log,
        patches: committed.len(),
    })
}
