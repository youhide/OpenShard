//! `map*.mul` and `statics*.mul`: the ground, and everything standing on it.
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
//! is why [`Map::load_facet`] carries it into the size decision and [`Map::load`]
//! — which has only a path — cannot.

use std::fmt;
use std::path::{Path, PathBuf};

use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;

use crate::grid::{BlockCoord, BlockIndex, LandGrid};

/// Tiles along each side of a map block.
pub const BLOCK_SIZE: u32 = 8;
/// Cells in a block.
pub(crate) const CELLS_PER_BLOCK: usize = (BLOCK_SIZE * BLOCK_SIZE) as usize;
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

/// One cell of ground.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LandCell {
    /// Index into the land table of `tiledata.mul`.
    pub tile: LandTile,
    /// The ground's height here.
    pub z: i8,
}

/// An index into `tiledata.mul`'s land table.
///
/// Land and static entries both look like `u16` in the files, but are indexed
/// into different halves of tiledata. Keeping them distinct prevents a static
/// art graphic from quietly becoming a mountain, or the reverse.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct LandTile(pub u16);

/// One thing standing on the ground.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StaticItem {
    /// Index into the static table of `tiledata.mul`.
    pub tile: Graphic,
    /// Where in the world, not in the block: resolved on load.
    pub x: u16,
    /// Where in the world.
    pub y: u16,
    /// Its base height. What you stand on is this plus the tile's height.
    pub z: i8,
    /// Its colour.
    pub hue: Hue,
}

/// The known sizes of a Britannia facet, in tiles.
///
/// Only used to name what was found; the size itself comes from the file.
fn describe_size(width: u32, height: u32) -> &'static str {
    match (width, height) {
        (6144, 4096) => "Felucca/Trammel (classic)",
        (7168, 4096) => "Felucca/Trammel (post-ML)",
        (2304, 1600) => "Ilshenar",
        (2560, 2048) => "Malas",
        (1448, 1448) => "Tokuno",
        (1280, 4096) => "Ter Mur",
        _ => "unknown facet",
    }
}

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

/// One facet: the ground and the statics on it.
///
/// The whole thing is in memory. That is the design — the database is never
/// touched inside a tick, and a facet is under 100MB.
pub struct Map {
    /// The ground, and the only thing that knows the order it is in.
    land: LandGrid,
    /// Statics per block, indexed by **the same [`BlockIndex`]** the land is —
    /// which is load-bearing and which nothing enforces: the two arrays are
    /// built side by side from one block count and are only ever addressed
    /// through [`LandGrid::index_of`], so a block's cells and a block's statics
    /// cannot come apart without that call being wrong for both.
    ///
    /// **Each block is sorted by the tile its items stand on** — see
    /// [`Map::statics_at`], which is what the order is for, and [`tile_key`],
    /// which is the order.
    ///
    /// The sort is stable, so two statics on one tile stay in the order the file
    /// has them. That is not a nicety: the client draws them in file order and
    /// `client/render`'s `statics::pick` breaks a tie by taking the last, so a
    /// resort that swapped two items on one tile would change which of them is on
    /// top.
    statics: Vec<Vec<StaticItem>>,
}

/// Where a static sorts within its block: by tile, **`y` first**.
///
/// The row before the column, and that is the whole reason this is a named
/// function rather than a tuple written twice. A caller walking a rectangle
/// walks it row by row — `client/render`'s `statics::for_each_static_in`, which
/// is every walk of the map this workspace makes — so a row has to be
/// *contiguous* for [`Map::statics_in_row`] to hand one back as a slice. Sorted
/// the other way it would be eight scattered runs.
///
/// Only the low three bits of each coordinate matter within a block, but the
/// whole of both is used: the items of one block share the same high bits, so
/// the two orders are the same one, and the key is then the same comparison the
/// lookups make.
fn tile_key(item: &StaticItem) -> (u16, u16) {
    (item.y, item.x)
}

impl fmt::Debug for Map {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Map")
            .field("size", &format!("{}x{}", self.width(), self.height()))
            .field("facet", &self.facet_name())
            .field("statics", &self.statics.iter().map(Vec::len).sum::<usize>())
            .finish()
    }
}

impl AsRef<Map> for Map {
    fn as_ref(&self) -> &Map {
        self
    }
}

impl Map {
    /// Load a facet from a path alone, working out its size from the file.
    ///
    /// `statics` is optional: a map with no statics is bare ground, which is
    /// wrong but not unusable, and being able to load one makes the map testable
    /// on its own.
    ///
    /// # This cannot tell Malas from Ter Mur
    ///
    /// They are the same number of blocks and a path is not a facet number, so
    /// an 81,920-block file loads as Malas. Prefer [`Map::load_facet`], which
    /// knows which facet it asked for; this exists for a file that is not in a
    /// client install at all.
    pub fn load(
        map_path: impl AsRef<Path>,
        statics_paths: Option<(impl AsRef<Path>, impl AsRef<Path>)>,
    ) -> Result<Self, MapError> {
        let map_path = map_path.as_ref();
        let bytes = read(map_path)?;
        Self::from_bytes(map_path, bytes, statics_paths, None)
    }

    /// Load a facet, preferring the UOP container over the `.mul`.
    ///
    /// # Why the `.mul` is the fallback and not the source
    ///
    /// Modern clients ship both, and the `.mul` may be a stub full of zeroes.
    /// It is the right size, it parses perfectly, and it describes a flat empty
    /// world. Reading it produces no error and no map.
    ///
    /// So: if `<name>LegacyMUL.uop` exists next to `<name>.mul`, it wins. That
    /// is the file the client itself reads.
    pub fn load_facet(client_dir: impl AsRef<Path>, facet: u8) -> Result<Self, MapError> {
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

        Self::from_bytes(&source_path, bytes, statics, Some(facet))
    }

    /// Build a facet from cells already in memory, with no file anywhere.
    ///
    /// `cell` is asked for every tile by world coordinate, and where that lands
    /// in the block-ordered array is this function's business rather than the
    /// caller's. That is the point of it: the column-major order this module's
    /// header is about is the easiest thing here to get backwards, and a caller
    /// assembling `cells` itself would be a second place to get it wrong.
    ///
    /// The size is given in **blocks**, so a facet that is not a whole number of
    /// blocks — which no reader in this file could parse — cannot be asked for.
    ///
    /// # What a map built this way does not have
    ///
    /// No statics: [`Map::statics_at`] is empty everywhere and
    /// [`Map::static_count`] is zero, exactly as for a [`Map::load`] that was
    /// given no `statics` paths — until [`Map::place_static`] puts one there.
    ///
    /// And no facet identity. A size nobody ships is an ordinary thing to ask
    /// for here, so [`Map::facet_name`] may honestly answer "unknown facet".
    /// This does not inherit [`Map::load`]'s Malas/Ter Mur ambiguity, and cannot:
    /// there is no block count to deduce a shape from, because the caller said
    /// what the shape is.
    ///
    /// # Panics
    ///
    /// If the facet would be wider or taller than a `u16` coordinate can reach.
    /// Every tile past that is one [`Map::land`] could never be asked about, so
    /// such a map is memory that exists to be unreachable; the largest facet a
    /// client ships is 7,168 tiles across.
    pub fn from_blocks(blocks_wide: u32, blocks_down: u32, cell: impl FnMut(u16, u16) -> LandCell) -> Self {
        let land = LandGrid::from_blocks(blocks_wide, blocks_down, cell);
        let statics = vec![Vec::new(); land.block_count() as usize];
        Self { land, statics }
    }

    /// `facet` is the facet number when the caller knows it, and is what breaks
    /// the Malas/Ter Mur tie described in this module's header.
    fn from_bytes(
        map_path: &Path,
        mut bytes: Vec<u8>,
        statics_paths: Option<(impl AsRef<Path>, impl AsRef<Path>)>,
        facet: Option<u8>,
    ) -> Result<Self, MapError> {
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

        let statics = match statics_paths {
            Some((index_path, data_path)) => {
                Self::load_statics(index_path.as_ref(), data_path.as_ref(), &land)?
            }
            None => vec![Vec::new(); blocks],
        };

        Ok(Self { land, statics })
    }

    /// `land` is what says how many blocks there are and where each one starts
    /// in the world — the second of which is the inverse of the block order,
    /// and was open-coded here before [`LandGrid`] owned it.
    fn load_statics(
        index_path: &Path,
        data_path: &Path,
        land: &LandGrid,
    ) -> Result<Vec<Vec<StaticItem>>, MapError> {
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
        // share the land's own [`BlockIndex`].
        let mut out: Vec<Vec<StaticItem>> = vec![Vec::new(); blocks];
        for (slot, block) in out.iter_mut().zip(land.blocks()) {
            let at = block.get() as usize * STAIDX_ENTRY;
            let offset = u32::from_le_bytes([index[at], index[at + 1], index[at + 2], index[at + 3]]);
            let length = u32::from_le_bytes([index[at + 4], index[at + 5], index[at + 6], index[at + 7]]);

            // 0xFFFFFFFF means "no statics here", and it is the common case —
            // most of Britannia is empty ground. A length that runs past the end
            // of the file means a truncated download, and reading it would
            // panic, so both are simply "nothing here".
            if offset == u32::MAX || length == u32::MAX || length == 0 {
                continue;
            }
            let (Ok(offset), Ok(length)) = (usize::try_from(offset), usize::try_from(length)) else {
                continue;
            };
            let Some(chunk) = data.get(offset..offset + length) else {
                continue;
            };

            // The inverse of the block order — the grid's, because getting it
            // backwards here places every block past the first column somewhere
            // else in a file that parses perfectly.
            let (block_x, block_y) = land.origin_of(block).expect("a block of this facet");

            let mut items = Vec::with_capacity(chunk.len() / STATIC_BYTES);
            for entry in chunk.chunks_exact(STATIC_BYTES) {
                items.push(StaticItem {
                    tile: Graphic(u16::from_le_bytes([entry[0], entry[1]])),
                    // The file stores an offset within the block; a world
                    // coordinate is more use to everyone downstream.
                    x: (block_x + u32::from(entry[2] & 0x7)) as u16,
                    y: (block_y + u32::from(entry[3] & 0x7)) as u16,
                    z: entry[4] as i8,
                    hue: Hue(u16::from_le_bytes([entry[5], entry[6]])),
                });
            }
            // Sorted by tile, which is [`Map::statics_at`]'s index — and
            // **stably**, because two statics on one tile are drawn in the order
            // the file has them and the last of them is the one on top.
            items.sort_by_key(tile_key);
            *slot = items;
        }
        Ok(out)
    }

    /// The facet's width in tiles.
    pub const fn width(&self) -> u32 {
        self.land.width()
    }

    /// The facet's height in tiles.
    pub const fn height(&self) -> u32 {
        self.land.height()
    }

    /// What this facet appears to be.
    pub fn facet_name(&self) -> &'static str {
        describe_size(self.width(), self.height())
    }

    /// Whether a point is on the map at all.
    pub const fn contains(&self, x: u16, y: u16) -> bool {
        self.land.contains(x, y)
    }

    /// The ground at a point, or `None` off the map.
    pub fn land(&self, x: u16, y: u16) -> Option<LandCell> {
        self.land.get(x, y)
    }

    /// The ground of one row of tiles, from `from_x` to `to_x` inclusive, in
    /// ascending `x`.
    ///
    /// [`Map::statics_in_row`]'s other half, and it exists for the same reason:
    /// a rectangle is walked row by row, and a row is where the cost is. Each
    /// cell here is one step east of the last — see
    /// [`LandGrid::cells_in_row`] — rather than a fresh derivation of a block
    /// index and an offset inside it per tile.
    ///
    /// It yields cells and not positions: a caller walking a rectangle already
    /// knows where it is, and the row simply ends where the facet does. So a
    /// row that starts off the map is empty, a row that runs off its eastern
    /// edge stops there, and **a caller counting tiles must not assume it got
    /// one cell per tile it asked for** — the tiles past the end are the ones
    /// [`Map::land`] answers `None` for.
    ///
    /// A range that runs backwards is empty.
    pub fn land_in_row(&self, y: u16, from_x: u16, to_x: u16) -> impl Iterator<Item = LandCell> + '_ {
        self.land
            .cells_in_row(y, from_x, to_x)
            .map(|at| self.land.cell(at))
    }

    /// Change the ground at one tile, for a map built by [`Map::from_blocks`].
    ///
    /// The other half of building a scene — see [`Map::place_static`] for what
    /// that is, why both of these are `pub`, and why nothing in the engine may
    /// call either. Off the map it does nothing, for the same reason.
    pub fn set_land(&mut self, x: u16, y: u16, cell: LandCell) {
        self.land.set(x, y, cell);
    }

    /// Put a static on the map, at the coordinates the item itself carries.
    ///
    /// For building a *scene*: a handful of tiles with known geometry — ground
    /// at a stated height, a stair, a band of wall — that a movement test can
    /// walk over and know the right answer for in advance. The map this
    /// repository can otherwise test against is a real client install, which is
    /// not on every machine and is not something a test can shape; a scene is
    /// both, and it is what `openshard_movement::scene` is built on.
    ///
    /// It is `pub` and not `#[cfg(test)]` for the reason
    /// [`crate::tiledata::TileData::set_static_tile`] is: the tests that want it
    /// are in other crates, and this repository ships no client files to build a
    /// fixture from.
    ///
    /// Nothing in the engine calls it, and nothing should — what stands on the
    /// ground is the client's own file talking, and a static written in at
    /// runtime is one end of the wire disagreeing with the other about a tile
    /// they both drew.
    ///
    /// A static outside the map is dropped: there is no block to keep it in, and
    /// [`Map::statics_at`] could never return it.
    ///
    /// It goes in after everything already standing on its own tile — where a
    /// push used to put it — which is what keeps the sort an invariant of the
    /// type rather than of the loader.
    pub fn place_static(&mut self, item: StaticItem) {
        let Some(block) = self.block_index(item.x, item.y) else {
            return;
        };
        let slot = &mut self.statics[block.get() as usize];
        let at = slot.partition_point(|had| tile_key(had) <= tile_key(&item));
        slot.insert(at, item);
    }

    /// Every static standing on a point.
    ///
    /// A block's items are sorted by tile, so this is two binary searches and a
    /// slice rather than a scan. **The scan was measured and it was the largest
    /// single phase of the lighting pass**: a block holds a few dozen items and
    /// `client/render`'s `statics::for_each_static_in` asks about all 64 of its
    /// tiles, so every block of a widest-zoom frame was read sixty-four times —
    /// 0.98ms of the 2.30ms that frame spends building its occlusion grid, before
    /// a single occluder is looked at. Every walk of the map pays it: what is
    /// drawn, what is picked, which graphics the atlas wants, and where the
    /// flames are.
    pub fn statics_at(&self, x: u16, y: u16) -> impl Iterator<Item = &StaticItem> + '_ {
        let Some(block) = self.land.block_of(x, y) else {
            return NO_STATICS.iter();
        };
        let block = self.statics_of(block);
        let from = block.partition_point(|item| tile_key(item) < (y, x));
        let count = block[from..].partition_point(|item| tile_key(item) == (y, x));
        block[from..from + count].iter()
    }

    /// Every static standing on one row of tiles, from `from_x` to `to_x`
    /// inclusive, in ascending `x`.
    ///
    /// What a walk of a rectangle is made of, and it exists because the walk is
    /// the expensive thing rather than the lookup. A block's items are sorted by
    /// `(y, x)`, so one row of one block is a **contiguous run** — one binary
    /// search a block instead of one a tile, which over a widest-zoom frame is
    /// four and a half thousand searches rather than thirty-five thousand, most
    /// of which found nothing because most of a map is open ground.
    ///
    /// The order is exactly [`Map::statics_at`]'s, row by row: `client/render`
    /// resolves a tie between two statics at one depth by taking the last one
    /// walked, so the order of this walk is a fact the picture depends on and not
    /// an implementation detail. See [`tile_key`].
    ///
    /// A row off the map, or a range that runs backwards, is empty.
    pub fn statics_in_row(&self, y: u16, from_x: u16, to_x: u16) -> impl Iterator<Item = &StaticItem> + '_ {
        let last = self.width().saturating_sub(1).min(u32::from(u16::MAX)) as u16;
        let (from_x, to_x) = (from_x.min(last), to_x.min(last));
        // The row's own block row, which every block below shares.
        let row = BlockCoord::containing(from_x, y);
        // An empty range for a row the map does not have, rather than a `None`
        // the caller would have to flatten: there is no static there either way.
        let columns = match u32::from(y) < self.height() && from_x <= to_x {
            true => row.x..BlockCoord::containing(to_x, y).x + 1,
            false => 0..0,
        };
        columns
            .flat_map(move |x| {
                let items = self.statics_of(BlockCoord { x, y: row.y });
                let from = items.partition_point(|item| item.y < y);
                let count = items[from..].partition_point(|item| item.y == y);
                &items[from..from + count]
            })
            .filter(move |item| item.x >= from_x && item.x <= to_x)
    }

    /// Every static in one block, in the block's own order.
    ///
    /// The whole slice and no search at all, which is what a *per-block* reader
    /// wants: `client/render`'s occlusion bake derives one block's surfaces once
    /// and keeps them, so it asks about a block rather than about a rectangle,
    /// and eight calls to [`Map::statics_in_row`] would be eight binary searches
    /// for a run that is already contiguous.
    ///
    /// The order is [`tile_key`]'s — `(y, x)` — which is [`Map::statics_in_row`]'s
    /// own order restricted to the block, so a reader that walks a block sees one
    /// tile's statics in exactly the order a reader walking rows sees them. That
    /// is what lets the two build the same grid, and `client/render`'s
    /// `occlusion::tests::a_baked_grid_is_the_one_the_walk_builds` is what says
    /// so.
    ///
    /// A block the facet does not have is empty.
    pub fn statics_in_block(&self, block_x: u32, block_y: u32) -> &[StaticItem] {
        self.statics_of(BlockCoord {
            x: block_x,
            y: block_y,
        })
    }

    /// One block's statics, empty for a block the facet does not have.
    ///
    /// The one place `statics` is subscripted, and it goes through the land's
    /// own [`LandGrid::index_of`] — which is what the field's doc comment means
    /// by the two arrays sharing an index.
    fn statics_of(&self, block: BlockCoord) -> &[StaticItem] {
        self.land
            .index_of(block)
            .and_then(|block| self.statics.get(block.get() as usize))
            .map_or(NO_STATICS, Vec::as_slice)
    }

    /// Which block a tile's statics are in, or `None` off the map.
    fn block_index(&self, x: u16, y: u16) -> Option<BlockIndex> {
        self.land.index_of(self.land.block_of(x, y)?)
    }

    /// How many statics the facet holds.
    pub fn static_count(&self) -> usize {
        self.statics.iter().map(Vec::len).sum()
    }

    /// A point on the ground, for a caller that only has x and y.
    pub fn ground(&self, x: u16, y: u16) -> Option<Point> {
        self.land(x, y).map(|cell| Point::new(x, y, cell.z))
    }

    /// The heights of a land tile's four corners: top, right, left, bottom —
    /// `(x, y)`, `(x+1, y)`, `(x, y+1)`, `(x+1, y+1)`.
    ///
    /// A land cell stores *one* height, and it is the corner the tile shares
    /// with the tiles north of it. The other three belong to the neighbours,
    /// which is why the ground has no seams: adjacent tiles do not merely abut,
    /// they are stretched over *the same* vertices, so a gap between them is not
    /// expressible.
    ///
    /// Off the edge of the map there is no neighbour and the tile's own height
    /// stands in, which flattens the border rather than dropping it off a cliff
    /// into `z = 0`.
    pub fn land_corners(&self, x: u16, y: u16) -> Option<[i8; 4]> {
        let own = self.land(x, y)?.z;
        let at = |x: Option<u16>, y: Option<u16>| match (x, y) {
            (Some(x), Some(y)) => self.land(x, y).map_or(own, |cell| cell.z),
            _ => own,
        };
        let (east, south) = (x.checked_add(1), y.checked_add(1));
        Some([own, at(east, Some(y)), at(Some(x), south), at(east, south)])
    }

    /// The height a body stands at on the land tile at `(x, y)` — the *average*
    /// of the four corners, not the raw north corner the cell stores.
    ///
    /// **Whoever asks where a character is standing asks this, on both sides of
    /// the wire.** A land tile is a sloped diamond and you stand in the middle
    /// of one; the raw corner is up to a tile's whole relief away from that. The
    /// walk ack (`0x22`) carries no `z`, so the server and the client each
    /// compute one, and a client that used the corner would draw its own body
    /// buried in the hillside — the ground's draw order is this same average,
    /// less two (`crate::tiledata` has no say in it; see the client's
    /// `depth::land_priority_z`), so the tile is not merely near the body, it is
    /// *in front of* it.
    ///
    /// Ported from RunUO's `Map.GetAverageZ`, which ClassicUO's `Land.AverageZ`
    /// agrees with: average the pair spanning the *gentler* slope, so a body
    /// stands level along the shallow axis.
    pub fn average_land_z(&self, x: u16, y: u16) -> Option<i8> {
        self.land_corners(x, y).map(average_corner_z)
    }
}

/// [`Map::average_land_z`]'s arithmetic, for a caller that already has the four
/// corners and would otherwise read them a second time.
///
/// `corners` is [`Map::land_corners`] order: top, right, left, bottom.
pub fn average_corner_z(corners: [i8; 4]) -> i8 {
    let [top, right, left, bottom] = corners.map(i32::from);
    let average = if (top - bottom).abs() > (left - right).abs() {
        floor_average(left, right)
    } else {
        floor_average(top, bottom)
    };
    // Every input is an `i8` and the mean of two of them is one: no branch here
    // can leave the range, so the conversion cannot fail.
    i8::try_from(average).unwrap()
}

/// The mean of two heights, floored towards minus infinity.
///
/// `>> 1` and not `/ 2`: they differ for an odd negative sum, which is every
/// other tile of a dungeon floor, and the client floors. Getting this wrong puts
/// a body one unit — four pixels — off the surface it is standing on, on half
/// the tiles underground.
const fn floor_average(a: i32, b: i32) -> i32 {
    (a + b) >> 1
}

/// A block the facet does not have, and a tile nothing stands on.
const NO_STATICS: &[StaticItem] = &[];

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
                tile: LandTile(u16::from_le_bytes([cell[0], cell[1]])),
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
    /// The fixtures below are written to disk because `Map::load` takes a path,
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
        assert_eq!(describe_size(7168, 4096), "Felucca/Trammel (post-ML)");
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

    /// Where the order itself is tested: [`crate::grid`], which owns it.
    ///
    /// `block_order_is_column_major`, `cells_within_a_block_are_row_major` and
    /// `every_cell_is_reachable_exactly_once` are tests of [`LandGrid`] now, and
    /// what is left here is what a `Map` adds to it — the byte format below, and
    /// the statics above it.
    #[test]
    fn off_the_map_is_none_not_a_panic() {
        let map = Map::from_blocks(2, 2, |x, y| LandCell {
            tile: LandTile(x + y),
            z: 0,
        });
        assert_eq!(map.land(16, 0), None);
        assert_eq!(map.land(0, 16), None);
        assert_eq!(map.land(u16::MAX, u16::MAX), None);
        assert!(!map.contains(16, 15));
        assert!(map.contains(15, 15));
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

        let map = Map::from_bytes(Path::new("tokuno"), bytes, None::<(&Path, &Path)>, Some(4)).unwrap();
        assert_eq!(
            map.land(2 * 8 + 5, 3 * 8 + 6),
            Some(LandCell {
                tile: LandTile(0xBEEF),
                z: -3,
            }),
        );
        // And a facet read with the two orders swapped would have found it at
        // the transposed tile instead.
        assert_eq!(map.land(3 * 8 + 6, 2 * 8 + 5).unwrap().tile, LandTile(0));
        assert_eq!(map.static_count(), 0);
    }

    #[test]
    fn a_map_that_is_not_whole_blocks_is_refused() {
        let dir = ScratchDir::new();
        let path = dir.join("ragged.mul");
        std::fs::write(&path, [0u8; BLOCK_BYTES + 1]).unwrap();

        let result = Map::load(&path, None::<(&Path, &Path)>);
        assert!(matches!(result, Err(MapError::NotABlockMap { .. })));
    }

    #[test]
    fn a_map_with_no_statics_loads_as_bare_ground() {
        let dir = ScratchDir::new();
        let path = dir.join("tiny.mul");
        // A whole facet's worth of blocks would be 90MB; use Tokuno's shape.
        let blocks = ((1448 / BLOCK_SIZE) * (1448 / BLOCK_SIZE)) as usize;
        std::fs::write(&path, vec![0u8; blocks * BLOCK_BYTES]).unwrap();

        let map = Map::load(&path, None::<(&Path, &Path)>).unwrap();
        assert_eq!((map.width(), map.height()), (1448, 1448));
        assert_eq!(map.facet_name(), "Tokuno");
        assert_eq!(map.static_count(), 0);
    }

    /// A stepped row is the same row a per-tile lookup builds — and it stops
    /// where the facet does rather than wrapping onto the next row.
    ///
    /// The two failures worth naming, because both leave a plausible picture:
    /// a step that crosses a block's eastern edge by `+1` reads the tile eight
    /// rows down of the *same* block, and a row that runs past the last column
    /// and keeps stepping reads the western edge of the row below.
    #[test]
    fn a_stepped_row_is_the_row_a_lookup_builds() {
        // Three blocks across, two down, and every tile carries its own
        // position — so a cell that came from the wrong tile says which.
        let map = Map::from_blocks(3, 2, |x, y| LandCell {
            tile: LandTile(x),
            z: y as i8,
        });

        for y in [0u16, 7, 8, 15] {
            let stepped: Vec<LandCell> = map.land_in_row(y, 0, 23).collect();
            let looked_up: Vec<LandCell> = (0..24).map(|x| map.land(x, y).unwrap()).collect();
            assert_eq!(stepped, looked_up, "row {y}, across three blocks");
        }

        // Past the eastern edge the row ends; it does not wrap.
        let over = map.land_in_row(5, 20, 40);
        assert_eq!(over.count(), 4, "four tiles left of a 24-wide facet");

        // A row the facet has not, and a range that runs backwards.
        assert_eq!(map.land_in_row(16, 0, 23).count(), 0);
        assert_eq!(map.land_in_row(0, 5, 4).count(), 0);
    }

    /// A tile's corners are its own height and its three neighbours', and the
    /// map's edge is flat rather than a cliff into zero.
    #[test]
    fn a_tiles_corners_are_its_neighbours_own_heights() {
        // A ramp running south-east: z is x + y.
        let map = Map::from_blocks(1, 1, |x, y| LandCell {
            tile: LandTile(3),
            z: (x + y) as i8,
        });
        assert_eq!(map.land_corners(2, 3), Some([5, 6, 6, 7]));
        // The far corner of the facet has no eastern or southern neighbour, so
        // all four corners are its own height.
        assert_eq!(map.land_corners(7, 7), Some([14; 4]));
        assert_eq!(map.land_corners(8, 0), None, "off the map is not a tile");
    }

    /// The height a body stands at, and the two halves of it that are easy to
    /// get wrong: which axis is averaged, and which way the halving rounds.
    ///
    /// Both sides of the wire compute this and neither is told the other's
    /// answer — the walk ack carries no `z` — so a unit of disagreement is a
    /// step the server refuses for no reason the player can see.
    #[test]
    fn a_body_stands_at_the_average_of_the_gentler_axis() {
        // Steep top-to-bottom (10), gentle left-to-right (2): the gentle pair is
        // the one averaged.
        assert_eq!(average_corner_z([0, 4, 6, 10]), 5);
        // And the other way round, with the same numbers transposed.
        assert_eq!(average_corner_z([0, 10, 0, 2]), 1);
        // Flat ground is its own height whichever branch is taken.
        assert_eq!(average_corner_z([-7; 4]), -7);
        // RunUO's `FloorAverage`: a truncating divide would give -3 here, half a
        // unit above where the client draws the floor. Every other tile of a
        // dungeon is an odd negative pair.
        assert_eq!(average_corner_z([-3, 10, 0, -4]), -4);
        assert_eq!(average_corner_z([0, 9, 3, -1]), -1);
        assert_eq!(average_corner_z([-10, 0, 0, 10]), 0);
    }

    /// And the map's own accessor is that formula over its own corners, so a
    /// caller that has coordinates and a caller that has heights cannot drift.
    #[test]
    fn the_maps_average_is_the_average_of_its_corners() {
        let map = Map::from_blocks(1, 1, |x, y| LandCell {
            tile: LandTile(3),
            z: ((x * 3) as i8).wrapping_sub((y * 2) as i8),
        });
        for y in 0..8u16 {
            for x in 0..8u16 {
                assert_eq!(
                    map.average_land_z(x, y),
                    map.land_corners(x, y).map(average_corner_z),
                    "({x}, {y})",
                );
            }
        }
        assert_eq!(map.average_land_z(99, 0), None);
    }

    /// A map built in memory is bare ground of the size that was asked for,
    /// whether or not any facet is that shape.
    ///
    /// That every tile is asked for exactly once is [`LandGrid`]'s own test;
    /// what is left here is the statics half a `Map` adds.
    #[test]
    fn a_map_built_in_memory_is_bare_ground_of_the_size_asked_for() {
        let map = Map::from_blocks(3, 2, |_, _| LandCell::default());
        assert_eq!((map.width(), map.height()), (24, 16));
        assert_eq!(map.facet_name(), "unknown facet");
        assert_eq!(map.static_count(), 0);
        assert_eq!(map.statics_at(0, 0).count(), 0);
    }

    /// A block is kept sorted by tile, and two statics on one tile keep the
    /// order they arrived in.
    ///
    /// [`Map::statics_at`] is a binary search over that order, so this is the
    /// invariant it rests on — and the second half is the one with a consequence
    /// a player can see. The client draws a tile's statics in file order and
    /// `client/render`'s `statics::pick` breaks a tie by taking the last, so a
    /// sort that reordered two items on one tile would change which of them is on
    /// top and which one a click holds. `place_static` is the other way in and
    /// takes the same rule: it goes in *after* what is already on its tile,
    /// which is where a push put it.
    ///
    /// The tiles are placed out of order and out of one block on purpose: in
    /// order, an unsorted list would pass.
    #[test]
    fn a_blocks_statics_are_sorted_by_tile_and_stable_within_one() {
        let mut map = Map::from_blocks(1, 1, |_, _| LandCell::default());
        // Three tiles of one block, none of them in order, and two items on the
        // middle one. The `tile` is what identifies each below.
        for (tile, x, y) in [(10, 3, 5), (20, 1, 2), (30, 3, 5), (40, 0, 7), (50, 3, 4)] {
            map.place_static(StaticItem {
                tile: Graphic(tile),
                x,
                y,
                z: 0,
                hue: Hue(0),
            });
        }

        let at = |x, y| map.statics_at(x, y).map(|item| item.tile).collect::<Vec<_>>();
        assert_eq!(
            at(3, 5),
            vec![Graphic(10), Graphic(30)],
            "the two on one tile lost their order"
        );
        assert_eq!(at(1, 2), vec![Graphic(20)]);
        assert_eq!(at(0, 7), vec![Graphic(40)]);
        assert_eq!(at(3, 4), vec![Graphic(50)]);
        assert_eq!(at(2, 2), Vec::<Graphic>::new(), "a tile nothing stands on");
        assert_eq!(map.static_count(), 5, "the sort dropped or duplicated one");
    }

    /// A row hands back exactly what asking its tiles one at a time does, in
    /// exactly that order.
    ///
    /// [`Map::statics_in_row`] is a faster spelling of the tile walk and nothing
    /// else, so the tile walk is its oracle — and the assertion is on the whole
    /// sequence rather than on a set, because the order is what a tie between two
    /// statics at one depth is broken by. It crosses three block columns and runs
    /// off both ends of the map, which is where a partial block and a clamp are.
    #[test]
    fn a_row_is_the_tile_walk_written_faster() {
        // Three blocks across, two down, and statics scattered over it in an
        // order that is neither the sort's nor the walk's.
        let mut map = Map::from_blocks(3, 2, |_, _| LandCell::default());
        let mut tile = 0;
        for y in [5u16, 0, 12, 5, 5, 7] {
            for x in [23u16, 0, 8, 15, 7, 16, 9] {
                tile += 1;
                map.place_static(StaticItem {
                    tile: Graphic(tile),
                    x,
                    y,
                    z: 0,
                    hue: Hue(0),
                });
            }
        }

        let named = |item: &StaticItem| (item.tile, item.x, item.y);
        for y in 0..16u16 {
            // Wider than the map on both sides, so the clamp is exercised as
            // well as the partial block at either end.
            for (from_x, to_x) in [(0u16, 23u16), (7, 16), (9, 9), (16, 7), (0, 100)] {
                let by_tile: Vec<_> = (from_x..=to_x.min(23))
                    .flat_map(|x| map.statics_at(x, y))
                    .map(named)
                    .collect();
                let by_row: Vec<_> = map.statics_in_row(y, from_x, to_x).map(named).collect();
                assert_eq!(by_row, by_tile, "row {y}, {from_x}..={to_x}");
            }
        }

        // The sweep found something: an empty map would agree with itself.
        assert_eq!(map.statics_in_row(5, 0, 23).count(), 21);
        assert_eq!(map.statics_in_row(1, 0, 23).count(), 0, "a row nothing stands on");
    }

    /// A block hands back its eight rows, in the order a reader of rows sees
    /// them.
    ///
    /// The property `client/render`'s occlusion bake rests on, and the one that
    /// would be silent if it broke: a per-block reader and a per-row reader build
    /// the same grid only because a tile's statics come out in the same order in
    /// both, and a resort of the block would change which of two statics on one
    /// tile is on top without failing anything else here.
    #[test]
    fn a_block_is_its_own_rows_end_to_end() {
        let mut map = Map::from_blocks(3, 2, |_, _| LandCell::default());
        let mut tile = 0;
        for y in [5u16, 0, 12, 5, 7] {
            for x in [23u16, 0, 8, 15, 7, 9] {
                tile += 1;
                map.place_static(StaticItem {
                    tile: Graphic(tile),
                    x,
                    y,
                    z: 0,
                    hue: Hue(0),
                });
            }
        }

        let named = |item: &StaticItem| (item.tile, item.x, item.y);
        for block_x in 0..3u32 {
            for block_y in 0..2u32 {
                let (from_x, to_x) = (block_x as u16 * 8, block_x as u16 * 8 + 7);
                let by_row: Vec<_> = (block_y as u16 * 8..block_y as u16 * 8 + 8)
                    .flat_map(|y| map.statics_in_row(y, from_x, to_x))
                    .map(named)
                    .collect();
                let by_block: Vec<_> = map.statics_in_block(block_x, block_y).iter().map(named).collect();
                assert_eq!(by_block, by_row, "block ({block_x}, {block_y})");
            }
        }

        // The sweep found something, and a block off the facet is empty rather
        // than a panic or a neighbour's contents.
        // Four of the five rows are in the first block row and two of the six
        // columns are in the first block column.
        assert_eq!(map.statics_in_block(0, 0).len(), 8);
        assert!(
            map.statics_in_block(3, 0).is_empty(),
            "a column the facet has not"
        );
        assert!(map.statics_in_block(0, 2).is_empty(), "a row the facet has not");
    }
}
