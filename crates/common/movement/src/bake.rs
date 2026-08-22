//! Stable, validated files containing an already-built [`NavigationGraph`].

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use openshard_map::snapshot::MapRevision;
use openshard_protocol::world::{Facet, Point};

use crate::NavigationGraph;
use crate::navigation::{Node, Region};

const MAGIC: &[u8; 8] = b"OSNAV\0\r\n";
const FORMAT_VERSION: u32 = 5;
/// Increment whenever graph construction or static movement semantics change.
pub const ROUTING_VERSION: u32 = 3;
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

/// Default destination, overridable for read-only installs.
pub fn artifact_path(client_dir: &Path, facet: Facet) -> PathBuf {
    std::env::var_os("OPENSHARD_NAVIGATION")
        .map(PathBuf::from)
        .unwrap_or_else(|| client_dir.join(format!("openshard-navigation-{}.bin", facet.0)))
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
fn file_name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
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
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
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

/// Read and validate an artifact without consulting terrain or pathfinding.
pub fn load(path: &Path, expected: &Stamp) -> Result<NavigationGraph, Error> {
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
    decode(path, &bytes[..payload_len], expected).map_err(|error| match error {
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
    put_u64(&mut w, g.region_offsets.len() as u64)?;
    put_u64(&mut w, g.region_nodes.len() as u64)?;
    put_u64(&mut w, g.edge_offsets.len() as u64)?;
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
    for &offset in &g.region_offsets {
        put_u32(&mut w, offset)?;
    }
    for &node in &g.region_nodes {
        put_u32(&mut w, node)?;
    }
    for &offset in &g.edge_offsets {
        put_u32(&mut w, offset)?;
    }
    for &target in &g.edge_targets {
        put_u32(&mut w, target)?;
    }
    for &cost in &g.edge_costs {
        put_u16(&mut w, cost)?;
    }
    Ok(())
}

fn decode(path: &Path, bytes: &[u8], expected: &Stamp) -> Result<NavigationGraph, Error> {
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
    if revision != expected.revision {
        return Err(stale(
            path,
            format!(
                "built from map revision {}, expected {}",
                revision.get(),
                expected.revision.get()
            ),
        ));
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
    if &actual != expected {
        return Err(stale(path, "client-file metadata changed"));
    }
    let nr = r.count()?;
    let nw = r.count()?;
    let nn = r.count()?;
    let nro = r.count()?;
    let nrn = r.count()?;
    let neo = r.count()?;
    let ne = r.count()?;
    let minimum = nr
        .checked_mul(8)
        .and_then(|n| n.checked_add(nw))
        .and_then(|n| n.checked_add(nn.checked_mul(5)?))
        .and_then(|n| n.checked_add(nro.checked_mul(4)?))
        .and_then(|n| n.checked_add(nrn.checked_mul(4)?))
        .and_then(|n| n.checked_add(neo.checked_mul(4)?))
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
    if nr != expected_regions || nw != cells.div_ceil(8) || nro != nr + 1 || neo != nn + 1 {
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
    let region_offsets = take_u32s(&mut r, nro)?;
    let region_nodes = take_u32s(&mut r, nrn)?;
    let edge_offsets = take_u32s(&mut r, neo)?;
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
    valid_offsets(path, &region_offsets, nrn, "region")?;
    valid_offsets(path, &edge_offsets, ne, "edge")?;
    if region_nodes.iter().any(|&node| node as usize >= nn)
        || edge_targets.iter().any(|&node| node as usize >= nn)
        || edge_costs.iter().any(|&cost| cost > 1023)
    {
        return Err(corrupt(path, "node index is out of range"));
    }
    for region in 0..nr {
        for &node in &region_nodes[region_offsets[region] as usize..region_offsets[region + 1] as usize] {
            let point = nodes[node as usize].point;
            let actual = usize::from(point.y) / 32 * regions_across + usize::from(point.x) / 32;
            if actual != region {
                return Err(corrupt(path, "region membership does not match node coordinates"));
            }
        }
    }
    Ok(NavigationGraph {
        width,
        height,
        regions,
        walkable,
        nodes,
        region_offsets,
        region_nodes,
        edge_offsets,
        edge_targets,
        edge_costs,
        build_region_nodes: Vec::new(),
        build_edges: Vec::new(),
    })
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

fn valid_offsets(path: &Path, offsets: &[u32], items: usize, name: &str) -> Result<(), Error> {
    if offsets.first() != Some(&0)
        || offsets.last().copied() != Some(items as u32)
        || offsets.windows(2).any(|pair| pair[0] > pair[1])
    {
        Err(corrupt(
            path,
            format!("{name} offsets are not monotonic and bounded"),
        ))
    } else {
        Ok(())
    }
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
    use crate::overlay::{Cover, Doors, Overlay};
    use crate::scene::Scene;
    use crate::{Footing, Tile, find_long_path};

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
            find_long_path(&terrain.footing(), &terrain.footing(), &graph, from, to, 100),
            find_long_path(&terrain.footing(), &terrain.footing(), &loaded, from, to, 100),
        );
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

    #[test]
    fn an_absent_artifact_is_reported_as_absent() {
        let path = temp("absent.bin");
        let _ = fs::remove_file(&path);
        assert!(matches!(load(&path, &stamp()), Err(Error::Missing { .. })));
    }
}
