//! One facet of the world: the ground, and everything standing on it.
//!
//! This is the type every reader of the world holds. It is *not* a file format
//! and it has never opened one — `openshard_uofiles::map` is the importer that
//! reads a UO install and hands one of these back, and it is the only thing in
//! the workspace that knows a `.mul` exists.
//!
//! The order the land is in is [`crate::grid`]'s and only [`crate::grid`]'s;
//! read that module's header for what the order is and why getting it backwards
//! is silent.

use std::fmt;

use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Point;

use crate::grid::{BlockCoord, BlockExtent, BlockIndex, LandGrid};

/// Tiles along each side of a map block.
pub const BLOCK_SIZE: u32 = 8;
/// Cells in a block.
pub const CELLS_PER_BLOCK: usize = (BLOCK_SIZE * BLOCK_SIZE) as usize;

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
    /// Build a facet from cells already in memory, with no file anywhere.
    ///
    /// `cell` is asked for every tile by world coordinate, and where that lands
    /// in the block-ordered array is this function's business rather than the
    /// caller's. That is the point of it: the column-major order this module's
    /// header is about is the easiest thing here to get backwards, and a caller
    /// assembling `cells` itself would be a second place to get it wrong.
    ///
    /// The size is given in **blocks**, so a facet that is not a whole number of
    /// blocks — which no importer could produce — cannot be asked for.
    ///
    /// # What a map built this way does not have
    ///
    /// No statics: [`Map::statics_at`] is empty everywhere and
    /// [`Map::static_count`] is zero — until [`Map::place_static`] puts one
    /// there. [`Map::from_parts`] is the other way in, and the one an importer
    /// takes.
    ///
    /// And no facet identity. A size nobody ships is an ordinary thing to ask
    /// for here, so [`Map::facet_name`] may honestly answer "unknown facet".
    /// This does not inherit an importer's Malas/Ter Mur ambiguity, and cannot:
    /// there is no block count to deduce a shape from, because the caller said
    /// what the shape is.
    ///
    /// # Panics
    ///
    /// If the facet would be wider or taller than a `u16` coordinate can reach.
    /// Every tile past that is one [`Map::land`] could never be asked about, so
    /// such a map is memory that exists to be unreachable; the largest facet a
    /// client ships is 7,168 tiles across.
    pub fn from_blocks(extent: BlockExtent, cell: impl FnMut(u16, u16) -> LandCell) -> Self {
        let land = LandGrid::from_blocks(extent.wide, extent.down, cell);
        let statics = vec![Vec::new(); land.block_count() as usize];
        Self { land, statics }
    }

    /// Build a facet from land and statics an importer already decoded.
    ///
    /// The door every importer comes through, and the reason it exists rather
    /// than a pair of public fields: **the sort is this type's invariant, not
    /// the decoder's.** [`Map::statics_at`] and [`Map::statics_in_row`] are
    /// binary searches over `(y, x)` order — see [`tile_key`] — so a decoder
    /// that handed over an unsorted block would not fail here, it would make
    /// every later lookup quietly find nothing. Sorting here means no importer
    /// can get it wrong, and a second importer cannot get it wrong *differently*
    /// from the first.
    ///
    /// The sort is **stable**, which is the half with a consequence a player can
    /// see: two statics on one tile keep the order the importer produced them
    /// in, and `client/render`'s `statics::pick` breaks a tie by taking the
    /// last, so reordering them would change which one is on top and which one
    /// a click holds.
    ///
    /// # Panics
    ///
    /// If `statics` does not hold exactly one entry per block of `land`. The two
    /// arrays share [`LandGrid`]'s own [`BlockIndex`], which is what lets a
    /// block's cells and a block's items be found by one number; a length that
    /// disagrees is an importer disagreeing with itself, and every lookup past
    /// the short end would silently be a block with nothing on it.
    #[must_use]
    pub fn from_parts(land: LandGrid, mut statics: Vec<Vec<StaticItem>>) -> Self {
        assert_eq!(
            statics.len(),
            land.block_count() as usize,
            "an importer handed over statics for a facet of a different size",
        );
        for block in &mut statics {
            block.sort_by_key(tile_key);
        }
        Self { land, statics }
    }

    /// The facet's width in tiles.
    pub const fn width(&self) -> u32 {
        self.land.width()
    }

    /// The facet's height in tiles.
    pub const fn height(&self) -> u32 {
        self.land.height()
    }

    /// The facet's size in blocks.
    ///
    /// What a caller cutting the facet into pieces asks first — see
    /// [`crate::chunk`] — and it is the land's own, so a piece is measured
    /// against the same extent that indexes it.
    pub const fn extent(&self) -> BlockExtent {
        self.land.extent()
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
    /// `openshard_uofiles::tiledata::TileData::set_static_tile` is: the tests that want it
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

    /// One block's sixty-four cells, row-major within the block.
    ///
    /// [`Map::statics_in_block`]'s other half, and it exists for the same
    /// caller: something that takes a whole block at a time rather than a
    /// rectangle — [`crate::chunk::Chunk::of`] is the one in this crate. A
    /// block the facet has not is empty, which is the same answer
    /// [`Map::statics_in_block`] gives.
    pub fn land_in_block(&self, block: BlockCoord) -> &[LandCell] {
        self.land
            .index_of(block)
            .map_or(NO_LAND, |block| self.land.block(block))
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
    /// less two (`openshard_uofiles::tiledata` has no say in it; see the client's
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

/// The ground of a block the facet does not have.
const NO_LAND: &[LandCell] = &[];

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the order itself is tested: [`crate::grid`], which owns it.
    ///
    /// `block_order_is_column_major`, `cells_within_a_block_are_row_major` and
    /// `every_cell_is_reachable_exactly_once` are tests of [`LandGrid`], and
    /// what is left here is what a `Map` adds to it: the statics, and the
    /// heights a body stands at. The byte format is the importer's, and is
    /// tested where it lives.
    #[test]
    fn off_the_map_is_none_not_a_panic() {
        let map = Map::from_blocks(BlockExtent { wide: 2, down: 2 }, |x, y| LandCell {
            tile: LandTile(x + y),
            z: 0,
        });
        assert_eq!(map.land(16, 0), None);
        assert_eq!(map.land(0, 16), None);
        assert_eq!(map.land(u16::MAX, u16::MAX), None);
        assert!(!map.contains(16, 15));
        assert!(map.contains(15, 15));
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
        let map = Map::from_blocks(BlockExtent { wide: 3, down: 2 }, |x, y| LandCell {
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
        let map = Map::from_blocks(BlockExtent { wide: 1, down: 1 }, |x, y| LandCell {
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
        let map = Map::from_blocks(BlockExtent { wide: 1, down: 1 }, |x, y| LandCell {
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
        let map = Map::from_blocks(BlockExtent { wide: 3, down: 2 }, |_, _| LandCell::default());
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
        let mut map = Map::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell::default());
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
        let mut map = Map::from_blocks(BlockExtent { wide: 3, down: 2 }, |_, _| LandCell::default());
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
        let mut map = Map::from_blocks(BlockExtent { wide: 3, down: 2 }, |_, _| LandCell::default());
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
