//! Stable, validated files containing an already-built [`NavigationGraph`], and
//! **what a file like that was built from**.
//!
//! The second half is why [`FacetWorld`] lives here rather than beside a reader.
//! A facet has two sources now — a client install, or a base set of ours with
//! its patch log — and the difference is not which loader runs: it is which
//! files a derived artifact names in its stamp and which directory it lands in.
//! Getting that wrong produces a bake that validates happily against inputs it
//! was never built from, which is the one failure a stamp exists to stop, so the
//! resolution is one function and not one per caller.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_protocol::world::{Facet, Point};

use crate::NavigationGraph;
use crate::navigation::{Node, Region, Run};

const MAGIC: &[u8; 8] = b"OSNAV\0\r\n";
/// Increment whenever the bytes change shape, whatever the graph in them means.
///
/// 6 is `docs/map/navigation_graph.md`'s G1: the two prefix-sum offset arrays
/// became tables of `base` and `count`, so that a publish can re-lay one
/// region's nodes and one node's edges where they stand. The graph a version 5
/// file holds is still a graph this code would agree with — it is the
/// *addressing* that moved — so a stale artifact is rebaked rather than
/// converted, which is what every other version bump in this repo has decided
/// for a derived file.
const FORMAT_VERSION: u32 = 6;
/// Increment whenever graph construction or static movement semantics change.
///
/// 4 is `docs/map/navigation_spans.md`'s N4: a node is a standing place rather
/// than a tile, and a portal joins two of them in one direction. The bytes did
/// not change shape — a node was always a `Point` and the walkable bitmap was
/// always per tile — so nothing but this number would stop a shard from loading
/// a one-storey graph and believing it.
pub const ROUTING_VERSION: u32 = 4;
const MAX_COLLECTION: usize = 100_000_000;

/// Metadata for one input selected by the client-file loader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputStamp {
    pub name: String,
    pub bytes: u64,
    pub modified_ns: u128,
}

/// Everything cheap to inspect that identifies graph-producing inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub facet: Facet,
    /// The immutable world revision the graph was built from.
    pub revision: MapRevision,
    pub routing_version: u32,
    pub inputs: Vec<InputStamp>,
}

/// Why a baked graph cannot be used.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Missing { path: PathBuf },
    Io { path: PathBuf, source: io::Error },
    Incompatible { path: PathBuf, reason: String },
    Stale { path: PathBuf, reason: String },
    Corrupt { path: PathBuf, reason: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { path } => write!(f, "navigation artifact {} does not exist", path.display()),
            Self::Io { path, source } => write!(f, "navigation artifact {}: {source}", path.display()),
            Self::Incompatible { path, reason } => write!(
                f,
                "navigation artifact {} is incompatible: {reason}",
                path.display()
            ),
            Self::Stale { path, reason } => {
                write!(f, "navigation artifact {} is stale: {reason}", path.display())
            }
            Self::Corrupt { path, reason } => {
                write!(f, "navigation artifact {} is corrupt: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The directory a file sits in, as a directory something can be opened on.
///
/// `Path::parent` has **two** ways of saying "here", and only one of them is
/// `None`: a bare relative name like `felucca.osbase` answers `Some("")`, and an
/// empty path is not a directory anything can open — `File::open("")` is
/// `NotFound`. That is not a hypothetical: it is what the shard's own printed
/// rebake command produces, and it made a successful bake report
/// `navigation artifact  does not exist` after the artifact was already written.
///
/// One rule, three callers: this function, the bake binary and `boot`. Written
/// here because this is the crate that discovered it.
#[must_use]
pub fn beside(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Default destination, overridable for read-only installs.
///
/// **The name says which world the graph is a bake of**, because a directory can
/// hold more than one. A shard's base set and the world a client keeps off the
/// wire live side by side in a working directory — `felucca.osbase` and
/// `openshard-world-<id>-0.osbase` — and under one name per facet the second
/// bake silently overwrites the first. The stamp then does its job and both
/// sides ask for a rebake, in turn, forever: neither artifact is wrong, they
/// just cannot both exist. So the world's own file name goes in front, and an
/// install — which has no one file to be named after — keeps the name it always
/// had.
///
/// `world` is the file the facet was read from, or `None` for the install.
pub fn artifact_path(dir: &Path, world: Option<&Path>, facet: Facet) -> PathBuf {
    if let Some(named) = std::env::var_os("OPENSHARD_NAVIGATION") {
        return PathBuf::from(named);
    }
    match world {
        Some(world) => dir.join(format!("{}-navigation-{}.bin", stem_of(world), facet.0)),
        None => dir.join(format!("openshard-navigation-{}.bin", facet.0)),
    }
}

/// A world file's name without its extension, for naming what is baked from it.
///
/// The file name and not the path, for [`file_name_of`]'s reason: an artifact
/// that had the directory in its name would be invalidated by moving the
/// directory, and it is already in that directory.
fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| file_name_of(path), |stem| stem.to_string_lossy().into_owned())
}

/// Inspect exactly the files `WorldMap::load_facet` selects, plus tile data.
///
/// For a facet loaded out of a UO install. A facet loaded out of a base set has
/// different inputs and a different stamp — [`stamp_of_base_set`].
///
/// `revision` is the revision of the snapshot this graph is built from, or the
/// one a caller is about to check a stored artifact against. It is asked for
/// rather than assumed: a stamp that filled the field in for itself would
/// compare a constant with a constant, and the guard would never fire.
pub fn stamp_of(client_dir: &Path, facet: Facet, revision: MapRevision) -> Result<Stamp, Error> {
    let uop_name = format!("map{}LegacyMUL.uop", facet.0);
    let map_name = if client_dir.join(&uop_name).exists() {
        uop_name
    } else {
        format!("map{}.mul", facet.0)
    };
    let names = [
        map_name,
        format!("staidx{}.mul", facet.0),
        format!("statics{}.mul", facet.0),
        "tiledata.mul".into(),
    ];
    let paths: Vec<(String, PathBuf)> = names
        .into_iter()
        .map(|name| {
            let path = client_dir.join(&name);
            (name, path)
        })
        .collect();
    stamp_over(facet, revision, paths)
}

/// Inspect the base set a facet was read from, plus tile data.
///
/// [`stamp_of`]'s other half, and the reason it has one: a facet loaded from a
/// base set is not derived from `map0LegacyMUL.uop` and `statics0.mul` any
/// more, so stamping those files would validate a graph against inputs that are
/// no longer the source. They still exist and still have those mtimes, so the
/// check would *pass* — a stale bake answering for a world it was never built
/// from, which is exactly what a stamp exists to stop.
///
/// `tiledata` is still an input, because it still is one: a base set holds the
/// map, and what a tile *means* is `tiledata.mul`'s, so a graph built with one
/// tile table is not valid under another.
///
/// `patches` is the log beside the base set, when there is one — and it is an
/// input in exactly the same sense the base set is. A world of ours is the base
/// plus its log, so a graph built before an edit was committed is stale, and a
/// stamp naming only the base set would say it was fine. The `revision` catches
/// it too, and both are recorded for the reason the paragraph below gives.
/// `None` is a world nobody has edited: an absent file is not a zero-length
/// one, and stamping a file that is not there is not a thing this can do.
///
/// This is `docs/map/new_map_representation/plan.md`'s direction D arriving one
/// caller early. D's answer is that the revision is the whole key and the file
/// stamps go away; until then the revision is carried *and* the real inputs are
/// stamped, which is strictly more than either alone.
pub fn stamp_of_base_set(
    base_set: &Path,
    patches: Option<&Path>,
    tiledata: &Path,
    facet: Facet,
    revision: MapRevision,
) -> Result<Stamp, Error> {
    let paths = [Some(base_set), patches, Some(tiledata)]
        .into_iter()
        .flatten()
        .map(|path| (file_name_of(path), path.to_owned()))
        .collect::<Vec<_>>();
    stamp_over(facet, revision, paths)
}

/// The name an input is recorded under: its file name, not its path.
///
/// A path would make moving the shard's directory invalidate every artifact in
/// it, and the length and mtime beside it already separate two different files
/// that happen to share a name.
///
/// Public because the interiors flood is a second artifact keyed to a world and
/// stamps the same base set under the same rule — two spellings of "what is this
/// input called" would be two artifacts that disagree about one file.
#[must_use]
pub fn file_name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Where a facet's world is read from.
///
/// Two arms and not an `Option<&Path>`: the install is a *source*, not the
/// absence of one, and a shard converts one facet at a time — so both answers
/// are ordinary and neither is a value nobody has supplied yet.
#[derive(Clone, Copy, Debug)]
pub enum WorldSource<'a> {
    /// The client install's own `map*` and `statics*`, as every reader before
    /// base sets existed.
    Install,
    /// A base set of ours, and the append-only patch log beside it.
    BaseSet(&'a Path),
}

/// A facet cannot be read from the source it was pointed at.
#[derive(Debug)]
#[non_exhaustive]
pub enum SourceError {
    /// The install's map or statics could not be read.
    Install {
        /// The install directory.
        path: PathBuf,
        /// Why.
        source: openshard_uofiles::map::MapError,
    },
    /// The base set, or the log beside it, could not be resolved.
    BaseSet {
        /// Why.
        source: openshard_basemap::BaseError,
    },
    /// The file is a facet, and not the facet it was named for.
    ///
    /// Two answers to one question is a config that loads Tokuno as Felucca:
    /// every coordinate plausible, every place wrong.
    WrongFacet {
        /// Which file said so.
        path: PathBuf,
        /// The facet the caller asked for.
        wanted: Facet,
        /// The facet the file holds.
        found: Facet,
    },
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Install { path, source } => {
                write!(f, "reading a facet from {}: {source}", path.display())
            }
            Self::BaseSet { source } => source.fmt(f),
            Self::WrongFacet { path, wanted, found } => write!(
                f,
                "{} holds facet {}, and it was named for facet {wanted}",
                path.display(),
                found.0,
            ),
        }
    }
}

impl std::error::Error for SourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Install { source, .. } => Some(source),
            Self::BaseSet { source } => Some(source),
            Self::WrongFacet { .. } => None,
        }
    }
}

/// One facet as the source it was named from resolved it, plus everything a
/// bake over it has to record about where it came from.
///
/// The fields travel together because a caller that has one wants all of them:
/// the shard's boot, both bake binaries and the client each need the world, the
/// stamp over it and the directory its artifacts belong in, and each of them
/// used to derive all three for itself.
#[derive(Debug)]
pub struct FacetWorld {
    /// The facet, at the revision its source resolved to — the base set's own
    /// if nothing has been committed, or the revision the last patch produced.
    pub snapshot: MapSnapshot,
    /// The base set it came out of, or `None` for a facet read from the install.
    ///
    /// This is what decides the stamp and the artifact directory, which is why
    /// it is kept rather than being consumed by the read.
    pub base_set: Option<PathBuf>,
    /// The patch log beside that base set, when there is one on disk.
    ///
    /// `None` is a world nobody has edited, and it is not the same as an empty
    /// log: an empty log is a file, and a file is an input to a stamp.
    pub log: Option<PathBuf>,
    /// The revision the base set itself is at, before any patch — `None` for
    /// the install. What the log's header names, and what a caller appending a
    /// patch has to name.
    pub base: Option<MapRevision>,
    /// How many patches were applied on the way. Zero for the install.
    pub patches: usize,
}

impl FacetWorld {
    /// Read `facet` from `source`.
    ///
    /// `client_dir` is the install, and it is required either way: a base set
    /// holds the map, and `tiledata.mul` still holds what a tile *is*.
    ///
    /// # Errors
    ///
    /// [`SourceError`] — the source could not be read, or the file it named
    /// turned out to be a different facet.
    pub fn read(client_dir: &Path, source: WorldSource<'_>, facet: Facet) -> Result<Self, SourceError> {
        match source {
            WorldSource::Install => {
                let snapshot = openshard_uofiles::map::load_facet(client_dir, facet).map_err(|source| {
                    SourceError::Install {
                        path: client_dir.to_owned(),
                        source,
                    }
                })?;
                Ok(Self {
                    snapshot,
                    base_set: None,
                    log: None,
                    base: None,
                    patches: 0,
                })
            }
            WorldSource::BaseSet(base_set) => {
                // The base set *and* the log beside it, through the one call
                // every reader of a world of ours resolves it with: a caller
                // that read the base alone would be holding a world the shard
                // is not running.
                let openshard_basemap::Loaded {
                    snapshot,
                    base,
                    log,
                    patches,
                } = openshard_basemap::load(base_set).map_err(|source| SourceError::BaseSet { source })?;
                if snapshot.facet() != facet {
                    return Err(SourceError::WrongFacet {
                        path: base_set.to_owned(),
                        wanted: facet,
                        found: snapshot.facet(),
                    });
                }
                Ok(Self {
                    snapshot,
                    base_set: Some(base_set.to_owned()),
                    log,
                    base: Some(base),
                    patches,
                })
            }
        }
    }

    /// Where artifacts derived from this world live.
    ///
    /// Beside the base set when there is one, and in the install otherwise. An
    /// artifact of a base-set world left in the install directory would be found
    /// by a shard reading the install and refused for reasons it cannot see.
    #[must_use]
    pub fn artifacts<'a>(&'a self, client_dir: &'a Path) -> &'a Path {
        match &self.base_set {
            Some(base_set) => beside(base_set),
            None => client_dir,
        }
    }

    /// The stamp a navigation artifact built over this world records.
    ///
    /// The choice between [`stamp_of`] and [`stamp_of_base_set`] is made here
    /// and nowhere else: a caller that picked for itself could stamp a base-set
    /// world with the install's files, which still exist and still have their
    /// old mtimes — so the check would *pass*.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if one of the inputs cannot be inspected.
    pub fn stamp(&self, client_dir: &Path, facet: Facet) -> Result<Stamp, Error> {
        let revision = self.snapshot.revision();
        match &self.base_set {
            Some(base_set) => stamp_of_base_set(
                base_set,
                self.log.as_deref(),
                &client_dir.join("tiledata.mul"),
                facet,
                revision,
            ),
            None => stamp_of(client_dir, facet, revision),
        }
    }

    /// Where a navigation graph baked over this world belongs.
    ///
    /// The directory and the name in one answer, because they are one decision
    /// and getting either half from somewhere else is how two artifacts of two
    /// worlds end up sharing a path — see [`artifact_path`]. The facet is the
    /// snapshot's own: a world knows which facet it is, and a caller that passed
    /// a second opinion could name a file for a facet the world is not.
    #[must_use]
    pub fn navigation_path(&self, client_dir: &Path) -> PathBuf {
        artifact_path(
            self.artifacts(client_dir),
            self.base_set.as_deref(),
            self.snapshot.facet(),
        )
    }
}

/// Build the coarse graph over one world — the one construction every baker
/// uses.
///
/// The bake binary, the shard's boot and a client that was handed a world off
/// the wire all want this same sequence, and it has two decisions in it that
/// must not be made twice: **nothing live** (a baked graph is a facet's static
/// connectivity, so a door that happened to be shut when the bake ran is not a
/// property of the ground) and **the span index first** (a graph is a flood over
/// `step_allowed`, which since `navigation_spans.md`'s N3 reads spans — 0.16 s
/// against a graph bake measured in seconds).
///
/// `None` is a facet whose dimensions the graph cannot represent, which is
/// [`NavigationGraph::build`]'s own answer and not a failure this can describe
/// any better.
#[must_use]
pub fn build(snapshot: &MapSnapshot, tiles: &openshard_tiles::TileData) -> Option<NavigationGraph> {
    let map = snapshot.map();
    let spans = crate::spans::SpanIndex::build(map, tiles);
    let nothing_placed = openshard_map::overlay::Overlay::default();
    let footing = crate::Footing::new(
        Some(crate::MapTerrain::new(map, tiles, &spans)),
        &nothing_placed,
        openshard_map::overlay::Doors::AsTheyStand,
    );
    NavigationGraph::build(&footing, map.width(), map.height())
}

/// Read length and mtime for each named input, in the order given.
///
/// The order is part of the stamp: [`Stamp`] compares its inputs as a sequence,
/// so two callers stamping the same files in different orders would disagree.
fn stamp_over(facet: Facet, revision: MapRevision, paths: Vec<(String, PathBuf)>) -> Result<Stamp, Error> {
    let mut inputs = Vec::with_capacity(paths.len());
    for (name, path) in paths {
        let metadata = fs::metadata(&path).map_err(|source| io_error(path.clone(), source))?;
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        inputs.push(InputStamp {
            name,
            bytes: metadata.len(),
            modified_ns,
        });
    }
    Ok(Stamp {
        facet,
        revision,
        routing_version: ROUTING_VERSION,
        inputs,
    })
}

/// Atomically write a complete artifact in the destination directory.
pub fn save(path: &Path, graph: &NavigationGraph, stamp: &Stamp) -> Result<u64, Error> {
    if stamp.routing_version != ROUTING_VERSION {
        return Err(Error::Incompatible {
            path: path.into(),
            reason: "writer received an old routing stamp".into(),
        });
    }
    let parent = beside(path);
    let stem = path.file_name().and_then(|s| s.to_str()).unwrap_or("navigation");
    let mut attempt = 0u32;
    let (temp, file) = loop {
        let temp = parent.join(format!(".{stem}.{}.{}.tmp", std::process::id(), attempt));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => attempt += 1,
            Err(source) => return Err(io_error(temp, source)),
        }
    };
    let result = (|| {
        let mut out = BufWriter::new(file);
        let hash = {
            let mut hashed = HashWriter {
                inner: &mut out,
                hash: FNV_OFFSET,
            };
            encode(&mut hashed, graph, stamp).map_err(|source| io_error(temp.clone(), source))?;
            hashed.hash
        };
        out.write_all(&hash.to_le_bytes())
            .map_err(|source| io_error(temp.clone(), source))?;
        out.flush().map_err(|source| io_error(temp.clone(), source))?;
        out.get_ref()
            .sync_all()
            .map_err(|source| io_error(temp.clone(), source))?;
        drop(out);
        fs::rename(&temp, path).map_err(|source| io_error(path.into(), source))?;
        File::open(parent)
            .and_then(|d| d.sync_all())
            .map_err(|source| io_error(parent.into(), source))?;
        fs::metadata(path)
            .map(|m| m.len())
            .map_err(|source| io_error(path.into(), source))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// An artifact as it was read, and the world it was actually built from.
///
/// [`load_behind`]'s answer. The revision is the point of it: a caller that can
/// carry a graph forward has to know **how far**, and the file is the only thing
/// that knows.
#[derive(Debug)]
pub struct Loaded {
    /// The graph the file holds.
    pub graph: NavigationGraph,
    /// The revision it was built from — the world's own, or an ancestor of it.
    pub revision: MapRevision,
}

/// How much of a mismatch between an artifact and the world it names a loader
/// will still answer a graph for.
///
/// Two arms because there are two kinds of caller, not two levels of strictness:
/// a tool that wants *the* graph of a world, and a shard that holds the ground
/// and the log and can rebake the difference. Neither is allowed a changed base
/// set or a changed tile table — nothing replays those.
#[derive(Clone, Copy, Debug)]
enum Accept<'a> {
    /// The world exactly: every input, and the revision.
    Current,
    /// The world, or a world **behind** it that the patch log carries forward.
    ///
    /// `log` is that log's file name in the stamp, and it is the one input the
    /// two are allowed to disagree about: an artifact baked at revision 7 was
    /// stamped over a shorter log than the world at revision 9 has. It may also
    /// be absent from the older stamp altogether — a world nobody had edited had
    /// no log file to stamp — which is why the entry is dropped from *both*
    /// sides rather than compared leniently.
    OrBehind { log: &'a str },
}

/// Read and validate an artifact without consulting terrain or pathfinding.
///
/// The world exactly as it stands: see [`load_behind`] for the caller that can
/// take an older one.
pub fn load(path: &Path, expected: &Stamp) -> Result<NavigationGraph, Error> {
    read_artifact(path, expected, Accept::Current).map(|loaded| loaded.graph)
}

/// Read an artifact that may have been built from an **earlier revision of the
/// same world**, for a caller that can carry it forward.
///
/// [`load`]'s other half, and the reason it has one. The shard's coarse graph
/// follows a patch on the tick that commits it — `FacetState::publish` rebakes
/// the chunks the patch named — but the *file* is only ever as new as the last
/// bake, so a shard that was edited and then restarted meets an artifact one or
/// more revisions behind the world its log rebuilds. Refusing that is refusing
/// to boot over work the log already knows how to redo, and the answer is 80 ms
/// of `NavigationGraph::rebake_chunks` per edit rather than a whole-facet bake
/// measured in half-minutes.
///
/// `log` is the patch log's file name — [`file_name_of`] over
/// `openshard_basemap::patches::log_path` — and it names the one input an older
/// artifact is allowed to disagree about. Everything else still has to match
/// byte for byte: a base set that was re-imported or a tile table that moved is
/// a world no log can carry a graph across, and it is refused exactly as before.
///
/// **This does not check ancestry, and cannot.** That the recorded revision is
/// *below* the world's is all a file can say; whether the log actually holds the
/// patches between them is a question for the log, and the caller is the one
/// holding it.
///
/// # Errors
///
/// [`Error`] as [`load`], and [`Error::Stale`] for an artifact *ahead* of the
/// world it names — a log that was truncated or rolled back under a graph, which
/// is not a gap anything can replay forward.
pub fn load_behind(path: &Path, expected: &Stamp, log: &str) -> Result<Loaded, Error> {
    read_artifact(path, expected, Accept::OrBehind { log })
}

fn read_artifact(path: &Path, expected: &Stamp, accept: Accept<'_>) -> Result<Loaded, Error> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|source| io_error(path.into(), source))?
        .read_to_end(&mut bytes)
        .map_err(|source| io_error(path.into(), source))?;
    if bytes.len() < 8 {
        return Err(corrupt(path, "truncated checksum"));
    }
    let payload_len = bytes.len() - 8;
    let recorded = u64::from_le_bytes(bytes[payload_len..].try_into().unwrap());
    if hash(&bytes[..payload_len]) != recorded {
        return Err(corrupt(path, "checksum mismatch"));
    }
    decode(path, &bytes[..payload_len], expected, accept).map_err(|error| match error {
        Error::Corrupt { path: empty, reason } if empty.as_os_str().is_empty() => Error::Corrupt {
            path: path.into(),
            reason,
        },
        other => other,
    })
}

fn io_error(path: PathBuf, source: io::Error) -> Error {
    if source.kind() == io::ErrorKind::NotFound {
        Error::Missing { path }
    } else {
        Error::Io { path, source }
    }
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

struct HashWriter<W> {
    inner: W,
    hash: u64,
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hash = hash_continue(self.hash, &bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn hash_continue(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
    }
    hash
}

fn encode(mut w: impl Write, g: &NavigationGraph, stamp: &Stamp) -> io::Result<()> {
    w.write_all(MAGIC)?;
    put_u32(&mut w, FORMAT_VERSION)?;
    put_u32(&mut w, ROUTING_VERSION)?;
    w.write_all(&[stamp.facet.0, 0, 0, 0])?;
    put_u64(&mut w, stamp.revision.get())?;
    put_u32(&mut w, g.width)?;
    put_u32(&mut w, g.height)?;
    put_u64(&mut w, stamp.inputs.len() as u64)?;
    for input in &stamp.inputs {
        put_u32(&mut w, input.name.len() as u32)?;
        w.write_all(input.name.as_bytes())?;
        put_u64(&mut w, input.bytes)?;
        w.write_all(&input.modified_ns.to_le_bytes())?;
    }
    put_u64(&mut w, g.regions.len() as u64)?;
    put_u64(&mut w, g.walkable.len() as u64)?;
    put_u64(&mut w, g.nodes.len() as u64)?;
    put_u64(&mut w, g.region_nodes.len() as u64)?;
    put_u64(&mut w, g.edge_targets.len() as u64)?;
    for r in &g.regions {
        put_u16(&mut w, r.left)?;
        put_u16(&mut w, r.top)?;
        put_u16(&mut w, r.width)?;
        put_u16(&mut w, r.height)?;
    }
    w.write_all(&g.walkable)?;
    for n in &g.nodes {
        put_u16(&mut w, n.point.x)?;
        put_u16(&mut w, n.point.y)?;
        w.write_all(&[n.point.z as u8])?;
    }
    // The tables, and they are what the file carries now: a run is written as it
    // stands, dead entries and all, so that saving a graph a publish has already
    // moved is the same operation as saving one straight off a bake. See
    // `NavigationGraph::Run`, and `docs/map/navigation_graph.md`'s G1.
    for run in &g.region_runs {
        put_u32(&mut w, run.base)?;
        put_u32(&mut w, run.count)?;
    }
    for &node in &g.region_nodes {
        put_u32(&mut w, node)?;
    }
    for run in &g.edge_runs {
        put_u32(&mut w, run.base)?;
        put_u32(&mut w, run.count)?;
    }
    for &target in &g.edge_targets {
        put_u32(&mut w, target)?;
    }
    for &cost in &g.edge_costs {
        put_u16(&mut w, cost)?;
    }
    Ok(())
}

fn decode(path: &Path, bytes: &[u8], expected: &Stamp, accept: Accept<'_>) -> Result<Loaded, Error> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(8)? != MAGIC {
        return Err(incompatible(path, "wrong magic"));
    }
    let format = r.u32()?;
    if format != FORMAT_VERSION {
        return Err(incompatible(
            path,
            format!("format version {format}, expected {FORMAT_VERSION}"),
        ));
    }
    let routing = r.u32()?;
    if routing != ROUTING_VERSION {
        return Err(stale(
            path,
            format!("routing version {routing}, expected {ROUTING_VERSION}"),
        ));
    }
    let facet = Facet(r.take(4)?[0]);
    let revision = MapRevision::decoded(r.u64()?);
    let width = r.u32()?;
    let height = r.u32()?;
    if facet != expected.facet {
        return Err(incompatible(
            path,
            format!("facet {}, expected {}", facet.0, expected.facet.0),
        ));
    }
    // Alongside the file-metadata check below, not instead of it: mtime and
    // length answer "are these the same client files", and the revision answers
    // "is this the same world we published". A world edited in place would keep
    // every input file's stamp and change only this number.
    match accept {
        Accept::Current if revision != expected.revision => {
            return Err(stale(
                path,
                format!(
                    "built from map revision {}, expected {}",
                    revision.get(),
                    expected.revision.get()
                ),
            ));
        }
        // Behind is what the caller asked to be handed; ahead is a log that lost
        // records under a graph, and there is no direction to replay that in.
        Accept::OrBehind { .. } if revision.get() > expected.revision.get() => {
            return Err(stale(
                path,
                format!(
                    "built from map revision {}, and the world is at {}: an artifact cannot be \
                     ahead of the world it names",
                    revision.get(),
                    expected.revision.get()
                ),
            ));
        }
        _ => {}
    }
    if width == 0 || height == 0 || width > u16::MAX as u32 || height > u16::MAX as u32 {
        return Err(incompatible(
            path,
            format!("invalid map dimensions {width}x{height}"),
        ));
    }
    let count = r.count()?;
    if count > r.remaining() / 28 {
        return Err(corrupt(path, "input count exceeds the payload"));
    }
    let mut inputs = Vec::with_capacity(count);
    for _ in 0..count {
        let len = r.u32()? as usize;
        let name =
            String::from_utf8(r.take(len)?.to_vec()).map_err(|_| corrupt(path, "non-UTF-8 input name"))?;
        let bytes = r.u64()?;
        let modified_ns = u128::from_le_bytes(r.take(16)?.try_into().unwrap());
        inputs.push(InputStamp {
            name,
            bytes,
            modified_ns,
        });
    }
    let actual = Stamp {
        facet,
        revision,
        routing_version: routing,
        inputs,
    };
    let agrees = match accept {
        Accept::Current => &actual == expected,
        // The revision is already decided above, and the log is the input the
        // revision *is*: what is left to check is that the world underneath both
        // is the same one.
        Accept::OrBehind { log } => {
            actual.facet == expected.facet
                && actual.routing_version == expected.routing_version
                && inputs_besides(&actual.inputs, log) == inputs_besides(&expected.inputs, log)
        }
    };
    if !agrees {
        return Err(stale(path, "client-file metadata changed"));
    }
    let nr = r.count()?;
    let nw = r.count()?;
    let nn = r.count()?;
    let nrn = r.count()?;
    let ne = r.count()?;
    // Two `u32`s per run, where an offsets array carried one per owner plus a
    // terminator — the eight bytes a region and a node each cost the file, and
    // 0.4 MB on Britannia's 7.4.
    let minimum = nr
        .checked_mul(16)
        .and_then(|n| n.checked_add(nw))
        .and_then(|n| n.checked_add(nn.checked_mul(13)?))
        .and_then(|n| n.checked_add(nrn.checked_mul(4)?))
        .and_then(|n| n.checked_add(ne.checked_mul(6)?))
        .ok_or_else(|| corrupt(path, "collection sizes overflow"))?;
    if minimum > r.remaining() {
        return Err(corrupt(path, "collection sizes exceed the payload"));
    }
    let cells = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| corrupt(path, "dimension overflow"))?;
    let regions_across = (width as usize).div_ceil(32);
    let expected_regions = regions_across * (height as usize).div_ceil(32);
    if nr != expected_regions || nw != cells.div_ceil(8) {
        return Err(corrupt(path, "inconsistent collection lengths"));
    }
    let mut regions = Vec::with_capacity(nr);
    for _ in 0..nr {
        regions.push(Region {
            left: r.u16()?,
            top: r.u16()?,
            width: r.u16()?,
            height: r.u16()?,
        });
    }
    let walkable = r.take(nw)?.to_vec();
    if cells % 8 != 0 && walkable.last().is_some_and(|last| last >> (cells % 8) != 0) {
        return Err(corrupt(path, "walkability bitset has nonzero padding"));
    }
    let mut nodes = Vec::with_capacity(nn);
    for _ in 0..nn {
        let x = r.u16()?;
        let y = r.u16()?;
        let z = r.take(1)?[0] as i8;
        if u32::from(x) >= width || u32::from(y) >= height {
            return Err(corrupt(path, "node is outside the map"));
        }
        let tile = usize::from(y) * width as usize + usize::from(x);
        if walkable[tile / 8] & (1 << (tile % 8)) == 0 {
            return Err(corrupt(path, "node is not on a walkable tile"));
        }
        nodes.push(Node {
            point: Point::new(x, y, z),
        });
    }
    let region_runs = take_runs(&mut r, nr)?;
    let region_nodes = take_u32s(&mut r, nrn)?;
    let edge_runs = take_runs(&mut r, nn)?;
    let edge_targets = take_u32s(&mut r, ne)?;
    let mut edge_costs = Vec::with_capacity(ne);
    for _ in 0..ne {
        edge_costs.push(r.u16()?);
    }
    if r.at != bytes.len() {
        return Err(corrupt(path, "trailing payload bytes"));
    }
    for (i, region) in regions.iter().enumerate() {
        let left = (i % regions_across) * 32;
        let top = (i / regions_across) * 32;
        if region.left != left as u16
            || region.top != top as u16
            || region.width != (width as usize - left).min(32) as u16
            || region.height != (height as usize - top).min(32) as u16
        {
            return Err(corrupt(path, format!("region {i} is outside the map")));
        }
    }
    let listed = valid_runs(path, &region_runs, nrn, "region")?;
    let reachable = valid_runs(path, &edge_runs, ne, "edge")?;
    if region_nodes.iter().any(|&node| node as usize >= nn)
        || edge_targets.iter().any(|&node| node as usize >= nn)
        || edge_costs.iter().any(|&cost| cost > 1023)
    {
        return Err(corrupt(path, "node index is out of range"));
    }
    // A node stands in exactly one region, and a file that lists one twice is a
    // file whose regions disagree about where a place is.
    let mut named = vec![false; nn];
    for (region, run) in region_runs.iter().enumerate() {
        for &node in &region_nodes[run.base as usize..run.base as usize + run.count as usize] {
            if named[node as usize] {
                return Err(corrupt(path, "a node is listed by two regions"));
            }
            named[node as usize] = true;
            let point = nodes[node as usize].point;
            let actual = usize::from(point.y) / 32 * regions_across + usize::from(point.x) / 32;
            if actual != region {
                return Err(corrupt(path, "region membership does not match node coordinates"));
            }
        }
    }
    Ok(Loaded {
        revision,
        graph: NavigationGraph {
            width,
            height,
            regions,
            walkable,
            // What no run points at is garbage a publish left behind, and it is
            // counted rather than refused: a saved graph is allowed to be one an
            // edit has already moved. See `NavigationGraph::repack`.
            dead_nodes: (nn - named.iter().filter(|listed| **listed).count()) as u32,
            dead_region_nodes: (nrn - listed) as u32,
            dead_edges: (ne - reachable) as u32,
            nodes,
            region_runs,
            region_nodes,
            edge_runs,
            edge_targets,
            edge_costs,
        },
    })
}

/// Every input but the patch log, in order.
///
/// The log is the one input two stamps over the same world may honestly
/// disagree about — see [`Accept::OrBehind`] — and it is dropped from both sides
/// rather than compared, because an artifact old enough may not list it at all.
fn inputs_besides<'a>(inputs: &'a [InputStamp], log: &str) -> Vec<&'a InputStamp> {
    inputs.iter().filter(|input| input.name != log).collect()
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }

    fn require_items(&self, count: usize, bytes: usize) -> Result<(), Error> {
        if count
            .checked_mul(bytes)
            .is_some_and(|size| size <= self.remaining())
        {
            Ok(())
        } else {
            Err(Error::Corrupt {
                path: PathBuf::new(),
                reason: "collection size exceeds the payload".into(),
            })
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| Error::Corrupt {
                path: PathBuf::new(),
                reason: "truncated payload".into(),
            })?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }
    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn count(&mut self) -> Result<usize, Error> {
        let n = usize::try_from(self.u64()?).map_err(|_| Error::Corrupt {
            path: PathBuf::new(),
            reason: "collection length overflow".into(),
        })?;
        if n > MAX_COLLECTION {
            Err(Error::Corrupt {
                path: PathBuf::new(),
                reason: "unreasonable collection length".into(),
            })
        } else {
            Ok(n)
        }
    }
}
fn take_u32s(r: &mut Reader<'_>, count: usize) -> Result<Vec<u32>, Error> {
    r.require_items(count, 4)?;
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(r.u32()?);
    }
    Ok(values)
}

fn take_runs(r: &mut Reader<'_>, count: usize) -> Result<Vec<Run>, Error> {
    r.require_items(count, 8)?;
    let mut runs = Vec::with_capacity(count);
    for _ in 0..count {
        runs.push(Run {
            base: r.u32()?,
            count: r.u32()?,
        });
    }
    Ok(runs)
}

/// Check that every run lies inside the array it addresses, and answer how many
/// of that array's entries are reachable through one.
///
/// The rest is garbage a publish left behind — legal, counted, and repacked once
/// it outweighs the live. What is *not* legal is a run reaching past the end of
/// what it addresses, which is a file the reader would follow into somebody
/// else's numbers.
fn valid_runs(path: &Path, runs: &[Run], items: usize, name: &str) -> Result<usize, Error> {
    let mut reachable = 0_usize;
    for run in runs {
        let end = (run.base as usize)
            .checked_add(run.count as usize)
            .filter(|&end| end <= items)
            .ok_or_else(|| corrupt(path, format!("a {name} run reaches past its array")))?;
        let _ = end;
        reachable += run.count as usize;
    }
    if reachable > items {
        return Err(corrupt(path, format!("{name} runs overlap")));
    }
    Ok(reachable)
}
fn put_u16(w: &mut impl Write, n: u16) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}
fn put_u32(w: &mut impl Write, n: u32) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}
fn put_u64(w: &mut impl Write, n: u64) -> io::Result<()> {
    w.write_all(&n.to_le_bytes())
}
fn incompatible(path: &Path, reason: impl Into<String>) -> Error {
    Error::Incompatible {
        path: path.into(),
        reason: reason.into(),
    }
}
fn stale(path: &Path, reason: impl Into<String>) -> Error {
    Error::Stale {
        path: path.into(),
        reason: reason.into(),
    }
}
fn corrupt(path: &Path, reason: impl Into<String>) -> Error {
    Error::Corrupt {
        path: path.into(),
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::scene::Scene;
    use crate::{Footing, Weight, find_long_path};
    use openshard_map::grid::Tile;
    use openshard_map::overlay::{Cover, Doors, Overlay};

    /// A bounded open grid with some tiles blocked: a real map for the ground,
    /// an overlay for what is in the way. See `navigation`'s twin of this — the
    /// same fixture, because both are the same world.
    struct Grid {
        scene: Scene,
        blocked: Overlay,
    }

    impl Grid {
        fn new(width: u16, height: u16, blocked: &BTreeSet<(u16, u16)>) -> Self {
            let scene = Scene::flat_holding(width - 1, height - 1, 0);
            let mut overlay = Overlay::default();
            for y in 0..scene.height() {
                for x in 0..scene.width() {
                    // The scene rounds up to whole blocks; fence off what the
                    // fixture did not ask for, so its edge refuses a step.
                    if x >= width || y >= height || blocked.contains(&(x, y)) {
                        overlay.set(Tile::new(x, y), vec![Cover::blocking(0, 20)]);
                    }
                }
            }
            Self {
                scene,
                blocked: overlay,
            }
        }

        fn footing(&self) -> Footing<'_> {
            Footing::new(Some(self.scene.terrain()), &self.blocked, Doors::AsTheyStand)
        }
    }

    fn stamp() -> Stamp {
        Stamp {
            facet: Facet(0),
            revision: MapRevision::INITIAL,
            routing_version: ROUTING_VERSION,
            inputs: vec![InputStamp {
                name: "map0.mul".into(),
                bytes: 42,
                modified_ns: 7,
            }],
        }
    }
    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("openshard-nav-{}-{name}", std::process::id()))
    }
    #[test]
    fn round_trip_and_route_parity() {
        let mut blocked = BTreeSet::new();
        for y in 0..64 {
            if y != 40 {
                blocked.insert((48, y));
            }
        }
        let terrain = Grid::new(96, 64, &blocked);
        let graph = NavigationGraph::build(&terrain.footing(), 96, 64).unwrap();
        assert!(graph.counts().1 > 0, "the payload must exercise graph nodes");
        let path = temp("round.bin");
        let s = stamp();
        save(&path, &graph, &s).unwrap();
        let loaded = load(&path, &s).unwrap();
        assert_eq!(loaded, graph);
        let from = Point::new(2, 2, 0);
        let to = Point::new(93, 2, 0);
        assert_eq!(
            find_long_path(
                &terrain.footing(),
                &terrain.footing(),
                &graph,
                from,
                to,
                100,
                Weight::EXACT
            ),
            find_long_path(
                &terrain.footing(),
                &terrain.footing(),
                &loaded,
                from,
                to,
                100,
                Weight::EXACT
            ),
        );
        let _ = fs::remove_file(path);
    }

    /// A graph a publish has already moved is a graph the file has to carry as
    /// it stands — runs out of order, garbage between them and all.
    ///
    /// The alternative would be to pack it on the way out, and that would make
    /// `save` an operation whose result depends on how the graph in hand was
    /// arrived at. What a file holds is the graph, not the history of the graph.
    #[test]
    fn a_rebaked_graph_round_trips_with_its_garbage() {
        let mut blocked = BTreeSet::new();
        for y in 0..64 {
            if y != 40 {
                blocked.insert((48, y));
            }
        }
        let terrain = Grid::new(96, 64, &blocked);
        let mut graph = NavigationGraph::build(&terrain.footing(), 96, 64).unwrap();
        // A rebake over the same ground: nothing about the world moved, so the
        // graph is the same graph — but it has been through the write-back, and
        // this is the file's side of that.
        graph.rebake_chunks(
            &terrain.footing(),
            &[openshard_map::chunk::ChunkCoord::containing(48, 40)],
        );
        let path = temp("rebaked.bin");
        let s = stamp();
        save(&path, &graph, &s).unwrap();
        assert_eq!(load(&path, &s).unwrap(), graph);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn incompatible_stale_and_corrupt_files_are_distinct() {
        let terrain = Grid::new(8, 8, &BTreeSet::new());
        let graph = NavigationGraph::build(&terrain.footing(), 8, 8).unwrap();
        let path = temp("reject.bin");
        let s = stamp();
        save(&path, &graph, &s).unwrap();
        let original = fs::read(&path).unwrap();
        let resign = |data: &mut Vec<u8>| {
            let payload_len = data.len() - 8;
            let checksum = hash(&data[..payload_len]);
            data[payload_len..].copy_from_slice(&checksum.to_le_bytes());
        };

        let mut wrong = s.clone();
        wrong.inputs[0].bytes += 1;
        assert!(matches!(load(&path, &wrong), Err(Error::Stale { .. })));

        // The guard the revision field exists for, and the one that would never
        // fire if a stamp filled the field in for itself: every input file is
        // byte-for-byte what it was, and only the world's revision moved.
        let mut republished = s.clone();
        republished.revision = MapRevision::decoded(s.revision.get() + 1);
        let refusal = load(&path, &republished);
        assert!(
            matches!(&refusal, Err(Error::Stale { reason, .. }) if reason.contains("map revision")),
            "a graph built from another revision is stale, and says so: {refusal:?}"
        );

        let mut data = original.clone();
        data[0] ^= 1;
        resign(&mut data);
        fs::write(&path, &data).unwrap();
        assert!(matches!(load(&path, &s), Err(Error::Incompatible { .. })));
        data = original.clone();
        data[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        resign(&mut data);
        fs::write(&path, &data).unwrap();
        assert!(matches!(load(&path, &s), Err(Error::Incompatible { .. })));
        data = original.clone();
        data[12..16].copy_from_slice(&(ROUTING_VERSION + 1).to_le_bytes());
        resign(&mut data);
        fs::write(&path, &data).unwrap();
        assert!(matches!(load(&path, &s), Err(Error::Stale { .. })));
        data = original.clone();
        data[16] = 1;
        resign(&mut data);
        fs::write(&path, &data).unwrap();
        assert!(matches!(load(&path, &s), Err(Error::Incompatible { .. })));
        data = original.clone();
        data[28..32].copy_from_slice(&0u32.to_le_bytes());
        resign(&mut data);
        fs::write(&path, &data).unwrap();
        assert!(matches!(load(&path, &s), Err(Error::Incompatible { .. })));
        data = original;
        data.truncate(data.len() - 3);
        fs::write(&path, &data).unwrap();
        assert!(matches!(load(&path, &s), Err(Error::Corrupt { .. })));
        let _ = fs::remove_file(path);
    }

    /// A stamp over a world of ours, in `stamp_of_base_set`'s order: the base
    /// set, the log beside it when there is one, and the tile table.
    fn base_set_stamp(revision: u64, log: Option<u64>) -> Stamp {
        let mut inputs = vec![InputStamp {
            name: "felucca.osbase".into(),
            bytes: 4096,
            modified_ns: 11,
        }];
        if let Some(bytes) = log {
            inputs.push(InputStamp {
                name: "felucca.ospatch".into(),
                bytes,
                modified_ns: 12,
            });
        }
        inputs.push(InputStamp {
            name: "tiledata.mul".into(),
            bytes: 512,
            modified_ns: 13,
        });
        Stamp {
            facet: Facet(0),
            revision: MapRevision::decoded(revision),
            routing_version: ROUTING_VERSION,
            inputs,
        }
    }

    /// An artifact one or more revisions behind the world is handed to
    /// [`load_behind`] and refused by [`load`], and it says how far behind it is.
    ///
    /// The case is the ordinary life of a shard that can be edited: the graph
    /// follows a patch on the tick that commits it and nothing writes the file,
    /// so every restart after an edit meets exactly this. What must *not* be
    /// forgiven is anything else — a base set that moved is a world no log
    /// carries a graph across.
    #[test]
    fn an_artifact_behind_the_world_is_offered_to_a_caller_that_can_carry_it_forward() {
        let terrain = Grid::new(8, 8, &BTreeSet::new());
        let graph = NavigationGraph::build(&terrain.footing(), 8, 8).unwrap();
        // The world two edits on: the revision moved, and so did the log it moved
        // by. Nothing else did.
        let world = base_set_stamp(9, Some(480));

        let path = temp("behind.bin");
        save(&path, &graph, &base_set_stamp(7, Some(300))).unwrap();
        assert!(
            matches!(load(&path, &world), Err(Error::Stale { .. })),
            "the loader that wants the world exactly still refuses it"
        );
        let loaded = load_behind(&path, &world, "felucca.ospatch")
            .expect("an older revision of the same world is what the log carries forward");
        assert_eq!(loaded.revision.get(), 7, "how far behind is the file's to say");
        assert_eq!(loaded.graph, graph);

        // Baked before anything was ever committed: there was no log file to
        // stamp, so the older stamp is one input shorter rather than different.
        let never_edited = temp("behind-unlogged.bin");
        save(&never_edited, &graph, &base_set_stamp(1, None)).unwrap();
        assert_eq!(
            load_behind(&never_edited, &world, "felucca.ospatch")
                .expect("a world nobody had edited is behind one that has been")
                .revision
                .get(),
            1
        );

        // The base set itself moved: a re-import, and a world nothing replays.
        let mut reimported = world.clone();
        reimported.inputs[0].modified_ns += 1;
        assert!(
            matches!(
                load_behind(&path, &reimported, "felucca.ospatch"),
                Err(Error::Stale { .. })
            ),
            "only the log is forgiven"
        );

        // And the other direction, which is a log that lost records under a
        // graph rather than a graph that missed some.
        let ahead = load_behind(&path, &base_set_stamp(5, Some(200)), "felucca.ospatch");
        assert!(
            matches!(&ahead, Err(Error::Stale { reason, .. }) if reason.contains("ahead of the world")),
            "an artifact newer than the world it names is refused, and says why: {ahead:?}"
        );

        let _ = fs::remove_file(path);
        let _ = fs::remove_file(never_edited);
    }

    #[test]
    fn an_absent_artifact_is_reported_as_absent() {
        let path = temp("absent.bin");
        let _ = fs::remove_file(&path);
        assert!(matches!(load(&path, &stamp()), Err(Error::Missing { .. })));
    }

    /// A bare relative name has a parent, and it is the empty path — which is
    /// not a directory anything can open. It cost a successful bake reporting
    /// `navigation artifact  does not exist` after writing the artifact, from
    /// the rebake command the shard itself prints.
    #[test]
    fn a_bare_relative_name_sits_in_this_directory() {
        assert_eq!(beside(Path::new("felucca.osbase")), Path::new("."));
        assert_eq!(beside(Path::new("worlds/felucca.osbase")), Path::new("worlds"));
        assert_eq!(beside(Path::new("/srv/felucca.osbase")), Path::new("/srv"));
        // `File::open` is what `save` does with it, and it is the reason the
        // empty answer is not good enough.
        assert!(File::open(beside(Path::new("felucca.osbase"))).is_ok());
        assert!(File::open(Path::new("")).is_err());
    }

    /// Two worlds in one directory are two artifacts.
    ///
    /// The pair that made this necessary is the real one: a shard runs
    /// `felucca.osbase` out of its working directory, and a client that took
    /// that same world off the wire keeps it beside it as
    /// `openshard-world-<id>-0.osbase`. Under one name per facet, whichever
    /// baked last owned the file and the other side asked for a rebake — each
    /// one correct, each one undoing the other.
    #[test]
    fn a_bake_is_named_after_the_world_it_is_a_bake_of() {
        // The environment overrides everything, and a test that inherited one
        // would agree with itself no matter what this function does.
        assert!(
            std::env::var_os("OPENSHARD_NAVIGATION").is_none(),
            "OPENSHARD_NAVIGATION is set, so this test would be reading it instead",
        );
        let here = Path::new(".");
        let shard = artifact_path(here, Some(Path::new("felucca.osbase")), Facet(0));
        let client = artifact_path(
            here,
            Some(Path::new("openshard-world-688b7d838063f8c4-0.osbase")),
            Facet(0),
        );
        assert_ne!(shard, client, "two worlds shared one artifact path");
        assert_eq!(shard, Path::new("./felucca-navigation-0.bin"));
        assert_eq!(
            client,
            Path::new("./openshard-world-688b7d838063f8c4-0-navigation-0.bin"),
        );
        // And a facet is a facet: one world's two facets are two artifacts as
        // they always were.
        assert_ne!(
            artifact_path(here, Some(Path::new("felucca.osbase")), Facet(1)),
            shard,
        );
        // An install has no one file to be named after, and keeps the name
        // every bake had before base sets existed.
        assert_eq!(
            artifact_path(here, None, Facet(0)),
            Path::new("./openshard-navigation-0.bin"),
        );
    }
}
