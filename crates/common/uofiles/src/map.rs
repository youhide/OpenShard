//! `map*.mul` and `statics*.mul`: reading a UO install into a [`WorldMap`].
//!
//! **This module is an importer, not the world.** The world is
//! [`openshard_map::map::WorldMap`] and it has no idea a file exists; what is here is
//! the one thing in the workspace that has heard of `.mul`, `.uop` and `staidx`,
//! and its whole job is to turn those bytes into one of those. A shard that
//! never had a client install is what that split is for — see
//! `docs/map/new_map_representation/overview.md`.
//!
//! # Block order is column-major, and nothing tells you
//!
//! `map0.mul` is a flat array of 8×8 blocks indexed
//! `block_x * (height_in_blocks) + block_y`. Column-major — x is the *outer*
//! stride. Get it the other way round and the file still parses, every block is
//! still 196 bytes, and every read lands somewhere plausible. The map is simply
//! transposed, and you find out when a player walks into an ocean that should be
//! a coastline. Sphere's `CServerMap.cpp:445` is the authority.
//!
//! The order itself is not this module's to know: [`openshard_map::grid`] owns
//! it, and [`LandGrid::from_file_order`] is the door — the file's order **is**
//! that array's order, which is why the decoder below does no index arithmetic
//! at all.
//!
//! # The map size is not in the file either
//!
//! `map0.mul` has no header. The only thing that says how wide a facet is, is
//! the file's own length divided by the block size. A modern map0 is 7168×4096
//! — the post-ML expansion — not the 6144×4096 of every tutorial.
//!
//! # And the block count does not always name one facet
//!
//! Malas is 2560×4096/8² and Ter Mur is 1280×4096/8², and both come to **81,920
//! blocks**. The two files are the same length, neither carries a header, and
//! their `staidx` files are the same length too — so every check the block count
//! can make passes for the wrong one. Loading Ter Mur as Malas is the exact
//! failure the block-order note above describes: 512 blocks per column read as
//! 256, so everything past the first column lands somewhere else, and it parses
//! perfectly. The facet's *number* is the only thing that separates them, which
//! is why [`read_facet`] carries it into the size decision and [`load`] — which
//! has only a path — cannot.

use std::fmt;
use std::path::{Path, PathBuf};

use openshard_map::grid::LandGrid;
use openshard_map::map::{BLOCK_SIZE, CELLS_PER_BLOCK, LandCell, StaticItem, WorldMap};
use openshard_map::snapshot::MapSnapshot;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Facet;
use openshard_tiles::LandTileId;

/// A cell: `u16` tile id and an `i8` height. Sphere's `CUOMapMeter`.
const CELL_BYTES: usize = 3;
/// Every block carries a 4-byte header that nothing reads.
const BLOCK_HEADER: usize = 4;
/// Bytes per block on disk.
pub const BLOCK_BYTES: usize = BLOCK_HEADER + CELLS_PER_BLOCK * CELL_BYTES;
/// Bytes per `staidx` entry: offset, length, extra.
const STAIDX_ENTRY: usize = 12;
/// Bytes per static on disk: tile id, x, y, z, hue.
const STATIC_BYTES: usize = 7;

/// A map file could not be read.
#[derive(Debug)]
#[non_exhaustive]
pub enum MapError {
    /// A file could not be read.
    Read {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The file does not divide into whole blocks, so it is not a map.
    NotABlockMap {
        /// Which file.
        path: PathBuf,
        /// How big it is.
        size: usize,
    },
    /// The block count does not factor into any known facet.
    UnknownFacet {
        /// Which file.
        path: PathBuf,
        /// How many blocks it holds.
        blocks: usize,
    },
    /// The UOP container could not be read.
    Uop {
        /// Which file.
        path: PathBuf,
        /// Why.
        source: Box<crate::uop::UopError>,
    },
    /// `staidx` and `map` disagree about how many blocks there are.
    IndexMismatch {
        /// Blocks in the map.
        map_blocks: usize,
        /// Entries in the index.
        index_entries: usize,
    },
}

impl fmt::Display for MapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::NotABlockMap { path, size } => write!(
                f,
                "{} is {size} bytes, which is not a whole number of {BLOCK_BYTES}-byte blocks",
                path.display()
            ),
            Self::UnknownFacet { path, blocks } => write!(
                f,
                "{} holds {blocks} blocks, which is not the size of any known facet",
                path.display()
            ),
            Self::Uop { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::IndexMismatch {
                map_blocks,
                index_entries,
            } => write!(
                f,
                "the map has {map_blocks} blocks but staidx has {index_entries} entries; \
                 they are from different clients"
            ),
        }
    }
}

impl std::error::Error for MapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Uop { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Read a facet from a path alone, working out its size from the file.
///
/// `statics` is optional: a map with no statics is bare ground, which is wrong
/// but not unusable, and being able to read one makes this module testable on
/// its own.
///
/// # This cannot tell Malas from Ter Mur
///
/// They are the same number of blocks and a path is not a facet number, so an
/// 81,920-block file loads as Malas. Prefer [`read_facet`], which knows which
/// facet it asked for; this exists for a file that is not in a client install at
/// all.
pub fn load(
    map_path: impl AsRef<Path>,
    statics_paths: Option<(impl AsRef<Path>, impl AsRef<Path>)>,
) -> Result<WorldMap, MapError> {
    let map_path = map_path.as_ref();
    let bytes = read(map_path)?;
    from_bytes(map_path, bytes, statics_paths, None)
}

/// Import `facet` out of a client install, published as its first revision.
///
/// **The one door.** A process reads a facet here or not at all, so every world
/// in memory arrives knowing which facet it is and which revision it stands at.
/// [`read_facet`] is the same read without the identity, and no production code
/// outside this module calls it — a fixture and a diagnostic do.
pub fn load_facet(client_dir: impl AsRef<Path>, facet: Facet) -> Result<MapSnapshot, MapError> {
    // `facet.0` unwrapped here and nowhere above: the file name and the
    // `FACET_SHAPES` subscript are the two places the number itself is the
    // value, and both are in this module.
    Ok(MapSnapshot::new(facet, read_facet(client_dir, facet.0)?))
}

/// Read a facet out of a client install, preferring the UOP container over the
/// `.mul`.
///
/// # Why the `.mul` is the fallback and not the source
///
/// Modern clients ship both, and the `.mul` may be a stub full of zeroes. It is
/// the right size, it parses perfectly, and it describes a flat empty world.
/// Reading it produces no error and no map.
///
/// So: if `<name>LegacyMUL.uop` exists next to `<name>.mul`, it wins. That is
/// the file the client itself reads.
pub fn read_facet(client_dir: impl AsRef<Path>, facet: u8) -> Result<WorldMap, MapError> {
    let dir = client_dir.as_ref();
    let uop = dir.join(format!("map{facet}LegacyMUL.uop"));
    let statics = Some((
        dir.join(format!("staidx{facet}.mul")),
        dir.join(format!("statics{facet}.mul")),
    ));

    // Whichever file was actually read is the one an error should name.
    let (source_path, bytes) = if uop.exists() {
        let pattern = |index: usize| format!("build/map{facet}legacymul/{index:08}.dat");
        let bytes = crate::uop::read_concatenated(&uop, &pattern).map_err(|source| MapError::Uop {
            path: uop.clone(),
            source: Box::new(source),
        })?;
        (uop, bytes)
    } else {
        let mul = dir.join(format!("map{facet}.mul"));
        let bytes = read(&mul)?;
        (mul, bytes)
    };

    from_bytes(&source_path, bytes, statics, Some(facet))
}

/// `facet` is the facet number when the caller knows it, and is what breaks the
/// Malas/Ter Mur tie described in this module's header.
fn from_bytes(
    map_path: &Path,
    mut bytes: Vec<u8>,
    statics_paths: Option<(impl AsRef<Path>, impl AsRef<Path>)>,
    facet: Option<u8>,
) -> Result<WorldMap, MapError> {
    // A UOP container is allocated in fixed chunks and comes out a block or
    // two longer than the facet. Trim to the largest whole facet that fits
    // rather than refusing: the tail is padding, not data.
    if let Some(size) = largest_facet_within(bytes.len() / BLOCK_BYTES, facet) {
        bytes.truncate(size * BLOCK_BYTES);
    }

    if !bytes.len().is_multiple_of(BLOCK_BYTES) || bytes.is_empty() {
        return Err(MapError::NotABlockMap {
            path: map_path.to_owned(),
            size: bytes.len(),
        });
    }
    let blocks = bytes.len() / BLOCK_BYTES;
    let (width, height) = facet_size(blocks, facet).ok_or_else(|| MapError::UnknownFacet {
        path: map_path.to_owned(),
        blocks,
    })?;
    let land = LandGrid::from_file_order(width, height, cells_of(&bytes));

    let (statics, counts) = match statics_paths {
        Some((index_path, data_path)) => load_statics(index_path.as_ref(), data_path.as_ref(), &land)?,
        None => (Vec::new(), vec![0; blocks]),
    };

    // `from_parts` and not a set of fields: the per-block sort by tile is the
    // map's own invariant, so a decoder cannot forget it and a second importer
    // cannot get it wrong differently from this one.
    Ok(WorldMap::from_parts(land, statics, &counts))
}

/// `land` is what says how many blocks there are and where each one starts
/// in the world — the second of which is the inverse of the block order,
/// and was open-coded here before [`LandGrid`] owned it.
fn load_statics(
    index_path: &Path,
    data_path: &Path,
    land: &LandGrid,
) -> Result<(Vec<StaticItem>, Vec<u32>), MapError> {
    let index = read(index_path)?;
    let data = read(data_path)?;

    let blocks = land.block_count() as usize;
    let entries = index.len() / STAIDX_ENTRY;
    if entries != blocks {
        return Err(MapError::IndexMismatch {
            map_blocks: blocks,
            index_entries: entries,
        });
    }

    // `staidx` entry n describes block n, which is what makes the statics
    // share the land's own [`BlockIndex`]. Read in that order, so the items go
    // straight into the one run `WorldMap` keeps them in and the counts are the
    // lengths this loop already knows.
    let mut out: Vec<StaticItem> = Vec::with_capacity(data.len() / STATIC_BYTES);
    let mut counts: Vec<u32> = Vec::with_capacity(blocks);
    for block in land.blocks() {
        let at = block.get() as usize * STAIDX_ENTRY;
        let offset = u32::from_le_bytes([index[at], index[at + 1], index[at + 2], index[at + 3]]);
        let length = u32::from_le_bytes([index[at + 4], index[at + 5], index[at + 6], index[at + 7]]);

        // 0xFFFFFFFF means "no statics here", and it is the common case —
        // most of Britannia is empty ground. A length that runs past the end
        // of the file means a truncated download, and reading it would
        // panic, so both are simply "nothing here".
        let named = offset != u32::MAX && length != u32::MAX && length != 0;
        let chunk = match named {
            true => match (usize::try_from(offset), usize::try_from(length)) {
                (Ok(offset), Ok(length)) => data.get(offset..offset + length),
                _ => None,
            },
            false => None,
        };

        // The block's own part of the run, measured rather than predicted: a
        // trailing partial entry is not one, and the count has to be what was
        // actually pushed.
        let was = out.len();
        if let Some(chunk) = chunk {
            // The inverse of the block order — the grid's, because getting it
            // backwards here places every block past the first column somewhere
            // else in a file that parses perfectly.
            let (block_x, block_y) = land.origin_of(block).expect("a block of this facet");

            for entry in chunk.chunks_exact(STATIC_BYTES) {
                out.push(StaticItem {
                    tile: Graphic(u16::from_le_bytes([entry[0], entry[1]])),
                    // The file stores an offset within the block; a world
                    // coordinate is more use to everyone downstream.
                    x: (block_x + u32::from(entry[2] & 0x7)) as u16,
                    y: (block_y + u32::from(entry[3] & 0x7)) as u16,
                    z: entry[4] as i8,
                    hue: Hue(u16::from_le_bytes([entry[5], entry[6]])),
                });
            }
        }
        // Handed over in file order. `WorldMap::from_parts` is what sorts a block by
        // tile, **stably**, so two statics on one tile keep the order the file
        // has them in and the last of them stays the one on top.
        counts.push(u32::try_from(out.len() - was).expect("a block of fewer than 4G statics"));
    }
    Ok((out, counts))
}

/// The cells of a map file, straight down it: every block's four-byte header
/// skipped, every three bytes after it a cell.
///
/// No index arithmetic, on purpose — where a cell lands is [`LandGrid`]'s
/// business, and this is only the byte format. The two facts meet in
/// [`LandGrid::from_file_order`]: the file's order **is** the array's order.
fn cells_of(bytes: &[u8]) -> impl Iterator<Item = LandCell> + '_ {
    bytes.chunks_exact(BLOCK_BYTES).flat_map(|block| {
        block[BLOCK_HEADER..]
            .chunks_exact(CELL_BYTES)
            .map(|cell| LandCell {
                // Little-endian: the files are, the network is not.
                tile: LandTileId(u16::from_le_bytes([cell[0], cell[1]])),
                z: cell[2] as i8,
            })
    })
}

fn read(path: &Path) -> Result<Vec<u8>, MapError> {
    std::fs::read(path).map_err(|source| MapError::Read {
        path: path.to_owned(),
        source,
    })
}

/// How many blocks a facet of this shape holds.
const fn blocks_in(width: u32, height: u32) -> usize {
    ((width / BLOCK_SIZE) * (height / BLOCK_SIZE)) as usize
}

/// The largest whole facet that fits in `blocks`, so a padded UOP can be
/// trimmed to it.
///
/// Bounded by `facet`'s own shapes when the caller knows which facet this is:
/// trimming to a shape this facet could never be would be padding removed by
/// coincidence.
fn largest_facet_within(blocks: usize, facet: Option<u8>) -> Option<usize> {
    candidate_shapes(facet)
        .iter()
        .map(|(width, height)| blocks_in(*width, *height))
        .filter(|size| *size <= blocks)
        .max()
}

/// Every facet shape a client ships, largest first.
const KNOWN_FACETS: [(u32, u32); 6] = [
    (7168, 4096),
    (6144, 4096),
    (2560, 2048),
    (2304, 1600),
    (1448, 1448),
    (1280, 4096),
];

/// The shapes each facet number is allowed to be.
///
/// Britannia is listed twice because it grew: map0 and map1 were 6144 wide until
/// Mondain's Legacy widened them to 7168, and a client of either age is a client
/// somebody runs. Every other facet has exactly one shape, and that is what
/// makes this table able to answer a question the block count cannot — Malas and
/// Ter Mur are both 81,920 blocks, and only their *number* tells them apart.
const FACET_SHAPES: [&[(u32, u32)]; 6] = [
    &[(7168, 4096), (6144, 4096)], // 0 Felucca
    &[(7168, 4096), (6144, 4096)], // 1 Trammel
    &[(2304, 1600)],               // 2 Ilshenar
    &[(2560, 2048)],               // 3 Malas
    &[(1448, 1448)],               // 4 Tokuno
    &[(1280, 4096)],               // 5 Ter Mur
];

/// The shapes worth considering for `facet`.
///
/// A facet number past the table is one no client ships. It falls back to every
/// known shape rather than refusing, because the caller has a file in hand and
/// the block count may well still identify it.
fn candidate_shapes(facet: Option<u8>) -> &'static [(u32, u32)] {
    facet
        .and_then(|facet| FACET_SHAPES.get(facet as usize).copied())
        .unwrap_or(&KNOWN_FACETS)
}

/// Work out a facet's dimensions from its block count, and from which facet it
/// is when the count alone cannot say.
///
/// The file has no header, so this is the only source of truth. Anything that
/// matches no candidate is refused rather than guessed, because a wrong guess
/// transposes the map silently.
fn facet_size(blocks: usize, facet: Option<u8>) -> Option<(u32, u32)> {
    candidate_shapes(facet)
        .iter()
        .copied()
        .find(|(width, height)| blocks == blocks_in(*width, *height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of one test's own, removed when the test ends.
    ///
    /// The fixtures below are written to disk because `WorldMap::load` takes a path,
    /// and a fixed name under `temp_dir()` is shared state: two runs of this
    /// suite at once — a second `cargo test`, or CI's — write and delete each
    /// other's file, and the loser fails on a file that was whole a moment ago.
    /// It happened once on a full workspace run and never on the test alone,
    /// which is exactly how that class of flake presents. The pid and the
    /// counter make the name unique across processes and within one; `Drop`
    /// does the cleanup so a failing assertion still leaves no litter.
    struct ScratchDir(std::path::PathBuf);

    impl ScratchDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let n = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("openshard-map-test-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn join(&self, name: &str) -> std::path::PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_block_is_the_size_sphere_says() {
        assert_eq!(BLOCK_BYTES, 196, "4-byte header plus 64 cells of 3 bytes");
    }

    #[test]
    fn the_real_clients_map0_is_the_expanded_facet() {
        // 89,915,392 bytes. Every tutorial says Felucca is 6144x4096; a modern
        // one is the post-ML 7168x4096, and assuming the classic size would put
        // every block after the first column in the wrong place.
        let blocks = 89_915_392 / BLOCK_BYTES;
        assert_eq!(blocks, 458_752);
        assert_eq!(facet_size(blocks, Some(0)), Some((7168, 4096)));
        assert_eq!(facet_size(blocks, None), Some((7168, 4096)));
        // That the shape is then *named* "Felucca/Trammel (post-ML)" is
        // `WorldMap::facet_name`'s, and is tested where that lives.
    }

    #[test]
    fn the_classic_facet_is_still_recognised() {
        let blocks = blocks_in(6144, 4096);
        assert_eq!(facet_size(blocks, Some(0)), Some((6144, 4096)));
        assert_eq!(facet_size(blocks, None), Some((6144, 4096)));
    }

    #[test]
    fn an_unknown_block_count_is_refused_rather_than_guessed() {
        // Guessing would transpose the map, and it would parse cleanly.
        assert_eq!(facet_size(0, None), None);
        assert_eq!(facet_size(1, None), None);
        assert_eq!(facet_size(458_751, None), None);
    }

    #[test]
    fn malas_and_ter_mur_are_the_same_size_and_only_the_facet_number_parts_them() {
        // The collision itself, in arithmetic: 320x256 and 160x512.
        assert_eq!(blocks_in(2560, 2048), 81_920);
        assert_eq!(blocks_in(1280, 4096), 81_920);

        // With the facet number, each file is what it says it is.
        assert_eq!(facet_size(81_920, Some(3)), Some((2560, 2048)), "Malas");
        assert_eq!(facet_size(81_920, Some(5)), Some((1280, 4096)), "Ter Mur");

        // Without it there is no honest answer, and the documented one is Malas.
        // Pinned so that changing it is a decision rather than a reordering of
        // `KNOWN_FACETS`.
        assert_eq!(facet_size(81_920, None), Some((2560, 2048)));
    }

    #[test]
    fn a_facet_number_no_client_ships_falls_back_to_every_shape() {
        // Not an error: the caller has a file, and the block count may still
        // identify it. It must not silently become "no known facet".
        assert_eq!(facet_size(blocks_in(1448, 1448), Some(9)), Some((1448, 1448)));
        assert_eq!(facet_size(7, Some(9)), None);
    }

    #[test]
    fn a_padded_container_is_trimmed_to_the_shape_its_facet_can_be() {
        // A UOP is allocated in chunks and runs past the facet. Ter Mur's
        // container holds a few blocks more than 81,920; trimming must land on
        // Ter Mur's own size and not on some larger facet's.
        assert_eq!(largest_facet_within(81_920 + 5, Some(5)), Some(81_920));
        assert_eq!(largest_facet_within(81_920 + 5, Some(3)), Some(81_920));
        // Tokuno is smaller than either, and its own shape is the right answer
        // even though bigger shapes exist.
        assert_eq!(largest_facet_within(81_920, Some(4)), Some(blocks_in(1448, 1448)));
        // Nothing fits inside a file smaller than the smallest facet.
        assert_eq!(largest_facet_within(3, Some(4)), None);
    }

    /// The decoder reads its bytes straight down the file, and the grid puts
    /// them where the block order says.
    ///
    /// The two halves meet in [`cells_of`], which does no index arithmetic at
    /// all — so the thing worth testing is that a byte at a hand-computed offset
    /// comes back at the tile the format says it is. A transposed read finds it
    /// somewhere else, and finds *something* everywhere, which is the failure
    /// this module's header is about.
    #[test]
    fn a_loaded_facet_reads_its_bytes_where_the_block_order_says() {
        // Tokuno: 181 blocks square, the smallest facet a client ships, and one
        // whose block count names it on its own.
        let blocks_down = 1448 / BLOCK_SIZE;
        let mut bytes = vec![0u8; blocks_in(1448, 1448) * BLOCK_BYTES];

        // One marked cell, placed at the byte offset the file format says:
        // block (2, 3) is `2 * 181 + 3` blocks in — column-major — and cell
        // (5, 6) of it is `6 * 8 + 5` cells into that block, row-major.
        let block = (2 * blocks_down + 3) as usize;
        let cell = 6 * BLOCK_SIZE as usize + 5;
        let at = block * BLOCK_BYTES + BLOCK_HEADER + cell * CELL_BYTES;
        bytes[at..at + 2].copy_from_slice(&0xBEEF_u16.to_le_bytes());
        bytes[at + 2] = -3i8 as u8;

        let map = from_bytes(Path::new("tokuno"), bytes, None::<(&Path, &Path)>, Some(4)).unwrap();
        assert_eq!(
            map.land(2 * 8 + 5, 3 * 8 + 6),
            Some(LandCell {
                tile: LandTileId(0xBEEF),
                z: -3,
            }),
        );
        // And a facet read with the two orders swapped would have found it at
        // the transposed tile instead.
        assert_eq!(map.land(3 * 8 + 6, 2 * 8 + 5).unwrap().tile, LandTileId(0));
        assert_eq!(map.static_count(), 0);
    }

    #[test]
    fn a_map_that_is_not_whole_blocks_is_refused() {
        let dir = ScratchDir::new();
        let path = dir.join("ragged.mul");
        std::fs::write(&path, [0u8; BLOCK_BYTES + 1]).unwrap();

        let result = load(&path, None::<(&Path, &Path)>);
        assert!(matches!(result, Err(MapError::NotABlockMap { .. })));
    }

    #[test]
    fn a_map_with_no_statics_loads_as_bare_ground() {
        let dir = ScratchDir::new();
        let path = dir.join("tiny.mul");
        // A whole facet's worth of blocks would be 90MB; use Tokuno's shape.
        let blocks = ((1448 / BLOCK_SIZE) * (1448 / BLOCK_SIZE)) as usize;
        std::fs::write(&path, vec![0u8; blocks * BLOCK_BYTES]).unwrap();

        let map = load(&path, None::<(&Path, &Path)>).unwrap();
        assert_eq!((map.width(), map.height()), (1448, 1448));
        assert_eq!(map.facet_name(), "Tokuno");
        assert_eq!(map.static_count(), 0);
    }
}
