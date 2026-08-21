//! Offline artifact for the facet-wide wall topology.
//!
//! This lives beside the art-table reader rather than in `client/render`: the
//! renderer owns the pure graph, while this crate owns client-file I/O and the
//! command that pays the full-facet bake once.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use openshard_client_render::interiors::BuildingMap;
use openshard_map::{MapRevision, MapSnapshot};
use openshard_protocol::world::Facet;
use openshard_uofiles::tiledata::TileData;

use crate::{LoadError, load};

const MAGIC: &[u8; 8] = b"OSINT\0\r\n";
const FORMAT: u32 = 2;
/// Bump when exterior/door/wall semantics change.
pub const TOPOLOGY_VERSION: u32 = 4;
const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputStamp {
    name: String,
    bytes: u64,
    modified_ns: u128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    facet: Facet,
    /// The immutable world revision the flood was run over.
    revision: MapRevision,
    topology_version: u32,
    inputs: Vec<InputStamp>,
}

#[derive(Debug)]
pub enum Error {
    Art(LoadError),
    Missing {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Read {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    Incompatible {
        path: PathBuf,
        reason: String,
    },
    Stale {
        path: PathBuf,
        reason: String,
    },
    Corrupt {
        path: PathBuf,
        reason: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Art(error) => write!(f, "wall catalogue: {error}"),
            Self::Missing { path } => write!(f, "interior artifact {} does not exist", path.display()),
            Self::Io { path, source } => write!(f, "interior artifact {}: {source}", path.display()),
            Self::Read { path, source } => write!(f, "interior input {}: {source}", path.display()),
            Self::Incompatible { path, reason } => {
                write!(
                    f,
                    "interior artifact {} is incompatible: {reason}",
                    path.display()
                )
            }
            Self::Stale { path, reason } => {
                write!(f, "interior artifact {} is stale: {reason}", path.display())
            }
            Self::Corrupt { path, reason } => {
                write!(f, "interior artifact {} is corrupt: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Art(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::Read { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

/// Default artifact location beside the client install.
pub fn artifact_path(client_dir: &Path, facet: Facet) -> PathBuf {
    std::env::var_os("OPENSHARD_INTERIORS")
        .map(PathBuf::from)
        .unwrap_or_else(|| client_dir.join(format!("openshard-interiors-{}.bin", facet.0)))
}

/// Files whose contents determine the map labels, including the authored wall
/// catalogue.  Metadata stamps deliberately match the navigation artifact's
/// policy: inexpensive at startup and enough to distinguish client installs.
pub fn stamp_of(client_dir: &Path, facet: Facet, revision: MapRevision) -> Result<Stamp, Error> {
    let uop_name = format!("map{}LegacyMUL.uop", facet.0);
    let map_name = if client_dir.join(&uop_name).exists() {
        uop_name
    } else {
        format!("map{}.mul", facet.0)
    };
    let table = crate::table_path(client_dir);
    let inputs_at = [
        (map_name.clone(), client_dir.join(map_name)),
        (
            format!("staidx{}.mul", facet.0),
            client_dir.join(format!("staidx{}.mul", facet.0)),
        ),
        (
            format!("statics{}.mul", facet.0),
            client_dir.join(format!("statics{}.mul", facet.0)),
        ),
        ("tiledata.mul".into(), client_dir.join("tiledata.mul")),
        (format!("art-table: {}", table.display()), table),
    ];
    let mut inputs = Vec::with_capacity(inputs_at.len());
    for (name, path) in inputs_at {
        let metadata = fs::metadata(&path).map_err(|source| io_error(path.clone(), source))?;
        let modified_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |time| time.as_nanos());
        inputs.push(InputStamp {
            name,
            bytes: metadata.len(),
            modified_ns,
        });
    }
    Ok(Stamp {
        facet,
        revision,
        topology_version: TOPOLOGY_VERSION,
        inputs,
    })
}

/// Read the wall catalogue, map and tile data, then calculate a whole facet.
///
/// The revision comes back with the graph rather than being asked of the caller
/// afterwards: this function is what decides which world the flood ran over, so
/// it is the only place that can answer honestly. The caller stamps the
/// artifact with what it is handed here.
pub fn build(client_dir: &Path, facet: Facet) -> Result<(BuildingMap, MapRevision), Error> {
    let table = load(client_dir).map_err(Error::Art)?;
    let map = MapSnapshot::load_facet(client_dir, facet).map_err(|source| Error::Read {
        path: client_dir.to_path_buf(),
        source: Box::new(source),
    })?;
    let tiles = TileData::load(client_dir.join("tiledata.mul")).map_err(|source| Error::Read {
        path: client_dir.join("tiledata.mul"),
        source: Box::new(source),
    })?;
    let graph = BuildingMap::bake(map.map(), &tiles, &|graphic| table.shape(graphic));
    Ok((graph, map.revision()))
}

/// Atomically write a fully-built map and its validating stamp.
pub fn save(path: &Path, graph: &BuildingMap, stamp: &Stamp) -> Result<u64, Error> {
    if stamp.topology_version != TOPOLOGY_VERSION {
        return Err(Error::Incompatible {
            path: path.into(),
            reason: "writer received an old topology stamp".into(),
        });
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("interiors");
    let mut attempt = 0_u32;
    let (temp, file) = loop {
        let temp = parent.join(format!(".{stem}.{}.{}.tmp", std::process::id(), attempt));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => attempt += 1,
            Err(source) => return Err(io_error(temp, source)),
        }
    };
    let result: Result<u64, Error> = (|| {
        let mut out = BufWriter::new(file);
        let hash = {
            let mut hash = FNV_OFFSET;
            write_graph(&mut out, graph, stamp, &mut hash)
                .map_err(|source| io_error(temp.clone(), source))?;
            hash
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
            .and_then(|directory| directory.sync_all())
            .map_err(|source| io_error(parent.into(), source))?;
        fs::metadata(path)
            .map(|metadata| metadata.len())
            .map_err(|source| io_error(path.into(), source))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Load a graph without reopening map or art data.  The stamp rejects an
/// artifact made from another client revision or after a table hand edit.
pub fn load_baked(path: &Path, expected: &Stamp) -> Result<BuildingMap, Error> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|source| io_error(path.into(), source))?
        .read_to_end(&mut bytes)
        .map_err(|source| io_error(path.into(), source))?;
    if bytes.len() < 8 {
        return Err(corrupt(path, "truncated checksum"));
    }
    let payload = &bytes[..bytes.len() - 8];
    let recorded = u64::from_le_bytes(bytes[bytes.len() - 8..].try_into().unwrap());
    if hash(payload) != recorded {
        return Err(corrupt(path, "checksum mismatch"));
    }
    read_graph(path, payload, expected)
}

fn write_graph(mut out: impl Write, graph: &BuildingMap, stamp: &Stamp, hash: &mut u64) -> io::Result<()> {
    let mut write = |bytes: &[u8]| -> io::Result<()> {
        out.write_all(bytes)?;
        *hash = hash_continue(*hash, bytes);
        Ok(())
    };
    write(MAGIC)?;
    write(&FORMAT.to_le_bytes())?;
    write(&TOPOLOGY_VERSION.to_le_bytes())?;
    write(&[stamp.facet.0, 0, 0, 0])?;
    write(&stamp.revision.get().to_le_bytes())?;
    let (width, height) = graph.dimensions();
    write(&width.to_le_bytes())?;
    write(&height.to_le_bytes())?;
    write(&(stamp.inputs.len() as u64).to_le_bytes())?;
    for input in &stamp.inputs {
        write(&(input.name.len() as u32).to_le_bytes())?;
        write(input.name.as_bytes())?;
        write(&input.bytes.to_le_bytes())?;
        write(&input.modified_ns.to_le_bytes())?;
    }
    write(&(graph.labels().len() as u64).to_le_bytes())?;
    for &label in graph.labels() {
        write(&label.to_le_bytes())?;
    }
    Ok(())
}

fn read_graph(path: &Path, bytes: &[u8], expected: &Stamp) -> Result<BuildingMap, Error> {
    let mut at = 0;
    let magic = take(bytes, &mut at, MAGIC.len()).ok_or_else(|| corrupt(path, "truncated magic"))?;
    if magic != MAGIC {
        return Err(incompatible(path, "wrong magic"));
    }
    if u32_at(path, bytes, &mut at)? != FORMAT {
        return Err(incompatible(path, "unsupported format"));
    }
    if u32_at(path, bytes, &mut at)? != TOPOLOGY_VERSION {
        return Err(stale(path, "topology algorithm changed"));
    }
    let facet = take(bytes, &mut at, 4).ok_or_else(|| corrupt(path, "truncated facet"))?[0];
    if facet != expected.facet.0 {
        return Err(incompatible(path, "wrong facet"));
    }
    // Alongside the input-file check below, not instead of it: the file stamps
    // say "the same client install", and this says "the same published world".
    let revision = MapRevision::decoded(u64_at(path, bytes, &mut at)?);
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
    let width = u32_at(path, bytes, &mut at)?;
    let height = u32_at(path, bytes, &mut at)?;
    let inputs = read_inputs(path, bytes, &mut at)?;
    if inputs != expected.inputs {
        return Err(stale(path, "client map, tile data, or wall catalogue changed"));
    }
    let label_count =
        usize::try_from(u64_at(path, bytes, &mut at)?).map_err(|_| corrupt(path, "label count too large"))?;
    let expected_labels = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or_else(|| corrupt(path, "map dimensions overflow address space"))?;
    if label_count != expected_labels {
        return Err(corrupt(path, "label dimensions disagree"));
    }
    let mut labels = Vec::with_capacity(label_count);
    for _ in 0..label_count {
        labels.push(u32_at(path, bytes, &mut at)?);
    }
    if at != bytes.len() {
        return Err(corrupt(path, "trailing payload"));
    }
    BuildingMap::from_labels(width, height, labels).ok_or_else(|| corrupt(path, "label dimensions disagree"))
}

fn read_inputs(path: &Path, bytes: &[u8], at: &mut usize) -> Result<Vec<InputStamp>, Error> {
    let count =
        usize::try_from(u64_at(path, bytes, at)?).map_err(|_| corrupt(path, "input count too large"))?;
    let mut inputs = Vec::with_capacity(count);
    for _ in 0..count {
        let len = usize::try_from(u32_at(path, bytes, at)?).unwrap();
        let name =
            std::str::from_utf8(take(bytes, at, len).ok_or_else(|| corrupt(path, "truncated input name"))?)
                .map_err(|_| corrupt(path, "non-utf8 input name"))?
                .to_owned();
        let input_bytes = u64_at(path, bytes, at)?;
        let modified_ns = u128::from_le_bytes(
            take(bytes, at, 16)
                .ok_or_else(|| corrupt(path, "truncated input timestamp"))?
                .try_into()
                .unwrap(),
        );
        inputs.push(InputStamp {
            name,
            bytes: input_bytes,
            modified_ns,
        });
    }
    Ok(inputs)
}

fn take<'a>(bytes: &'a [u8], at: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = at.checked_add(len)?;
    let part = bytes.get(*at..end)?;
    *at = end;
    Some(part)
}

fn u32_at(path: &Path, bytes: &[u8], at: &mut usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        take(bytes, at, 4)
            .ok_or_else(|| corrupt(path, "truncated u32"))?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(path: &Path, bytes: &[u8], at: &mut usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        take(bytes, at, 8)
            .ok_or_else(|| corrupt(path, "truncated u64"))?
            .try_into()
            .unwrap(),
    ))
}

fn io_error(path: PathBuf, source: io::Error) -> Error {
    if source.kind() == io::ErrorKind::NotFound {
        Error::Missing { path }
    } else {
        Error::Io { path, source }
    }
}

fn hash(bytes: &[u8]) -> u64 {
    hash_continue(FNV_OFFSET, bytes)
}

fn hash_continue(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
    }
    hash
}

fn corrupt(path: &Path, reason: impl Into<String>) -> Error {
    Error::Corrupt {
        path: path.into(),
        reason: reason.into(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_baked_building_map_round_trips_with_its_stamp() {
        let graph = BuildingMap::from_labels(3, 2, vec![0, 4, 4, 0, 4, 0]).expect("matching labels");
        let stamp = Stamp {
            facet: Facet(0),
            revision: MapRevision::INITIAL,
            topology_version: TOPOLOGY_VERSION,
            inputs: vec![InputStamp {
                name: "map0.mul".into(),
                bytes: 42,
                modified_ns: 99,
            }],
        };
        let path = std::env::temp_dir().join(format!(
            "openshard-interiors-test-{}-{}.bin",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let _ = fs::remove_file(&path);

        save(&path, &graph, &stamp).expect("save artifact");
        assert_eq!(load_baked(&path, &stamp).expect("load artifact"), graph);

        fs::remove_file(path).expect("remove test artifact");
    }
}
