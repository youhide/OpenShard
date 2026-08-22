//! The land of one facet, and the one type that owns the order it is in.
//!
//! # The order, in one place
//!
//! [`map`](crate::map)'s header says what the order is and why getting it
//! backwards is silent. This module is the answer to that: the arithmetic lives
//! here once, and nothing outside [`LandGrid`] writes it again.
//!
//! Blocks are **column-major** — `block_x * blocks_down + block_y`, x the outer
//! stride — and the sixty-four cells inside one block are **row-major**,
//! `y_local * BLOCK_SIZE + x_local`. The two orders are opposites, which is
//! exactly why each is worth a type rather than a comment.
//!
//! # A block column is an eight-wide strip
//!
//! The two orders compose into something simpler than either. Fix `block_x`;
//! its blocks are contiguous and run in ascending `block_y`, and each lays its
//! rows out in ascending `y_local`. So within one block column the linear
//! position is
//!
//! ```text
//! block_y * 64 + y_local * 8 + x_local  ==  (block_y * 8 + y_local) * 8 + x_local
//!                                       ==  y * 8 + x_local
//! ```
//!
//! — a block column is one row-major image eight tiles wide and the whole facet
//! tall. That identity is what makes the transitions cheap and is the reason
//! [`LandGrid::south_of`] is `+8` on *every* tile rather than only inside a
//! block: crossing a block's southern edge is one block on (`+64`) less the
//! fifty-six cells the block's own rows ran through, which is the same `+8`.
//! It is also the whole of [`LandGrid::tile_of`], the inverse a loader needs.
//!
//! The identity is derived, not assumed: `the_two_orders_compose_into_a_strip`
//! holds it against the plain block-then-cell spelling on every tile of a
//! fixture.

use std::fmt;

use crate::map::{BLOCK_SIZE, CELLS_PER_BLOCK, LandCell};

/// A block's position on the facet — not a tile, and not a radar chunk.
///
/// A radar chunk is sixty-four tiles square (`client/render`'s
/// `BASE_CHUNK_TILES`), so it addresses a different grid entirely. Collapsing
/// the two is the confusion `docs/pixels.md` exists to prevent, and this type
/// is deliberately not it.
///
/// Its fields are public, unlike [`BlockIndex`]'s: a block coordinate is
/// something a caller has in hand — a rectangle of the world clipped to blocks
/// — where a linear index is only ever derived.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct BlockCoord {
    /// Block column, `tile_x / BLOCK_SIZE`.
    pub x: u32,
    /// Block row, `tile_y / BLOCK_SIZE`.
    pub y: u32,
}

/// A facet's size in map blocks.
///
/// Unlike [`BlockCoord`], this is a size rather than a position: `wide` and
/// `down` say how many eight-tile blocks the facet contains on each axis. The
/// fields are named so a caller cannot silently swap the two bare numbers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct BlockExtent {
    /// Block columns across the facet.
    pub wide: u32,
    /// Block rows down the facet.
    pub down: u32,
}

impl BlockExtent {
    /// How many blocks the extent holds.
    pub const fn count(self) -> u32 {
        self.wide * self.down
    }

    /// Where a block sits in an array laid out in this extent's order, or
    /// `None` for a block the extent has not.
    ///
    /// Column-major, from Sphere's `CServerMap.cpp:445`:
    /// `bx * (SizeY / UO_BLOCK_SIZE) + by`. **This is the only place that
    /// formula is written** — [`LandGrid::index_of`] is this call, and so is a
    /// chunk's own local order, which is what makes a chunk self-similar to the
    /// facet it was cut out of rather than a second layout that happens to
    /// agree.
    pub const fn index_of(self, block: BlockCoord) -> Option<BlockIndex> {
        match block.x < self.wide && block.y < self.down {
            true => Some(BlockIndex(block.x * self.down + block.y)),
            false => None,
        }
    }

    /// Which block a linear index names — the inverse of [`Self::index_of`],
    /// and the one a static loader needs to recover a block's world origin.
    ///
    /// Open-coded backwards is how a `staidx` walk puts every block after the
    /// first column somewhere else, which parses perfectly.
    pub const fn coord_of(self, index: BlockIndex) -> Option<BlockCoord> {
        // An empty extent counts zero blocks, so the guard rejects every index
        // before either division runs: neither can be by a zero `down`.
        match index.0 < self.count() {
            true => Some(BlockCoord {
                x: index.0 / self.down,
                y: index.0 % self.down,
            }),
            false => None,
        }
    }

    /// Every block of the extent, in the array's own order.
    pub fn blocks(self) -> impl Iterator<Item = BlockIndex> {
        (0..self.count()).map(BlockIndex)
    }
}

impl BlockCoord {
    /// The block a tile falls in.
    ///
    /// Says nothing about whether any facet *has* that block — ask
    /// [`LandGrid::index_of`], which is the thing that knows.
    pub const fn containing(x: u16, y: u16) -> Self {
        Self {
            x: x as u32 / BLOCK_SIZE,
            y: y as u32 / BLOCK_SIZE,
        }
    }

    /// The block's north-west tile.
    ///
    /// In tiles, and in `u32`: a coordinate past `u16` belongs to a block no
    /// facet has, and silently wrapping it is how a caller ends up reading the
    /// wrong corner of the world.
    pub const fn origin(self) -> (u32, u32) {
        (self.x * BLOCK_SIZE, self.y * BLOCK_SIZE)
    }
}

/// A block's position in the linear array.
///
/// Derived, never built by a caller: [`LandGrid::index_of`] is the only way to
/// make one, which is the point. The field is private where
/// [`LandTile`](crate::map::LandTile)'s is not, and the difference is where the
/// value comes from — a land tile is read straight off the wire or the file and
/// has to be constructible, whereas a caller writing out `block_x * blocks_down
/// + block_y` for itself is the precise bug this type exists to prevent.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BlockIndex(u32);

impl BlockIndex {
    /// The linear position, for a caller indexing an array laid out block by
    /// block — `Map`'s statics are the one in this crate.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// A cell's position in the linear array.
///
/// Derived in the same sense [`BlockIndex`] is, and from the same place.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CellIndex(u32);

impl CellIndex {
    /// The linear position.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One facet's land cells, in the order the file has them.
///
/// A `Vec<LandCell>` behind a door that opens only for a [`CellIndex`] or a
/// [`BlockIndex`]. [`LandGrid`] was already the only thing that *derives* those,
/// so what this adds is the other half: the conversion to `usize` happens here
/// and nowhere else, and the array cannot be reached by a loop counter, walked
/// in a caller's own order, or sliced by arithmetic written somewhere new.
///
/// It also gives the **length invariant** a home. [`Cells::of`] is the only way
/// to make one and it takes the block count it is claiming to hold, so
/// "a whole number of blocks' worth" is checked once at construction rather than
/// assumed by every reader — which is what makes [`Cells::block`]'s slice total
/// rather than hopeful.
///
/// Private to this module on purpose: no signature outside it names a cell
/// array, so a public wrapper would be a type nobody could obtain and nobody
/// needs.
struct TerrainCells(Vec<LandCell>);

impl TerrainCells {
    /// The one door in. `blocks` is what the caller says it is handing over.
    ///
    /// # Panics
    ///
    /// If the vector is not exactly `blocks` blocks' worth. Both callers build
    /// the array from the same block count they pass here, so a failure is this
    /// module disagreeing with itself rather than a bad file.
    fn of(cells: Vec<LandCell>, blocks: u32) -> Self {
        let want = blocks as usize * CELLS_PER_BLOCK;
        assert_eq!(cells.len(), want, "{blocks} blocks hold {want} cells");
        Self(cells)
    }

    /// How many cells, for a diagnostic. Not a position anyone can index with.
    fn len(&self) -> usize {
        self.0.len()
    }

    /// The cell at a linear position.
    ///
    /// # Panics
    ///
    /// If `at` came from a different, larger grid — see [`LandGrid::cell`],
    /// which is where that argument is made.
    fn read(&self, at: CellIndex) -> LandCell {
        self.0[at.0 as usize]
    }

    /// Put a cell at a linear position, with the same precondition.
    fn write(&mut self, at: CellIndex, cell: LandCell) {
        self.0[at.0 as usize] = cell;
    }

    /// One block's sixty-four cells, contiguous because [`Cells::of`] checked
    /// that every block's worth is there.
    fn block(&self, block: BlockIndex) -> &[LandCell] {
        let from = block.0 as usize * CELLS_PER_BLOCK;
        &self.0[from..from + CELLS_PER_BLOCK]
    }
}

/// The land of one facet, in the block order the files are in.
///
/// Every conversion between a tile, a block and a linear position is a method
/// here, and none of them is written anywhere else.
pub struct LandGrid {
    width: u32,
    height: u32,
    /// Blocks column-major, cells row-major within a block. See the module
    /// header — this is the only field in the workspace laid out that way.
    cells: TerrainCells,
}

impl fmt::Debug for LandGrid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LandGrid")
            .field("size", &format!("{}x{}", self.width, self.height))
            .field("cells", &self.cells.len())
            .finish()
    }
}

impl LandGrid {
    /// Build a grid by asking for every tile, by world coordinate.
    ///
    /// Where a cell goes is this function's business rather than the caller's:
    /// the column-major order is the easiest thing here to get backwards, and a
    /// caller assembling the array itself would be a second place to get it
    /// wrong.
    ///
    /// # Panics
    ///
    /// If the facet would be wider or taller than a `u16` coordinate can reach.
    /// Every tile past that is one nothing could ever ask about, so such a grid
    /// is memory that exists to be unreachable; the largest facet a client ships
    /// is 7,168 tiles across.
    pub fn from_blocks(
        blocks_wide: u32,
        blocks_down: u32,
        mut cell: impl FnMut(u16, u16) -> LandCell,
    ) -> Self {
        let (width, height) = (blocks_wide * BLOCK_SIZE, blocks_down * BLOCK_SIZE);
        let reach = u32::from(u16::MAX) + 1;
        assert!(
            width <= reach && height <= reach,
            "a {width}x{height} facet is larger than a u16 coordinate reaches",
        );

        let mut cells = Vec::with_capacity((blocks_wide * blocks_down) as usize * CELLS_PER_BLOCK);
        // The order, written once: blocks column-major, cells within a block
        // row-major. Everything else in this module reads it back.
        for block_x in 0..blocks_wide {
            for block_y in 0..blocks_down {
                for y in 0..BLOCK_SIZE {
                    for x in 0..BLOCK_SIZE {
                        let x = (block_x * BLOCK_SIZE + x) as u16;
                        let y = (block_y * BLOCK_SIZE + y) as u16;
                        cells.push(cell(x, y));
                    }
                }
            }
        }

        Self {
            width,
            height,
            cells: TerrainCells::of(cells, blocks_wide * blocks_down),
        }
    }

    /// Take cells that are already in the file's order, straight down the file.
    ///
    /// The loader's door. It decodes bytes into cells and knows nothing about
    /// where they land, because the file's order **is** this array's order —
    /// which is the one fact worth stating in a signature rather than leaving to
    /// a loop in a decoder.
    ///
    /// # Panics
    ///
    /// If the facet is not a whole number of blocks, or if `cells` is not
    /// exactly one facet's worth. Both are the decoder's own preconditions, so a
    /// failure here is this crate disagreeing with itself rather than a bad
    /// file.
    pub fn from_file_order(width: u32, height: u32, cells: impl Iterator<Item = LandCell>) -> Self {
        assert!(
            width.is_multiple_of(BLOCK_SIZE) && height.is_multiple_of(BLOCK_SIZE),
            "a {width}x{height} facet is not a whole number of {BLOCK_SIZE}-tile blocks",
        );
        let blocks = (width / BLOCK_SIZE) * (height / BLOCK_SIZE);
        Self {
            width,
            height,
            // The length check is `TerrainCells::of`'s, and is the reason the loader
            // hands over a count rather than a bare vector.
            cells: TerrainCells::of(cells.collect(), blocks),
        }
    }

    /// The facet's width in tiles.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// The facet's height in tiles.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The facet's width in blocks.
    pub const fn blocks_wide(&self) -> u32 {
        self.width / BLOCK_SIZE
    }

    /// The facet's height in blocks — the column-major stride, and the number
    /// this module exists to stop anyone else spelling out.
    pub const fn blocks_down(&self) -> u32 {
        self.height / BLOCK_SIZE
    }

    /// The facet's size in blocks.
    ///
    /// The order arithmetic itself is [`BlockExtent`]'s — a facet and a chunk
    /// cut out of one are laid out by the same rule, and this is where the
    /// facet borrows it.
    pub const fn extent(&self) -> BlockExtent {
        BlockExtent {
            wide: self.blocks_wide(),
            down: self.blocks_down(),
        }
    }

    /// How many blocks the facet holds.
    pub const fn block_count(&self) -> u32 {
        self.extent().count()
    }

    /// Whether a point is on the facet at all.
    pub const fn contains(&self, x: u16, y: u16) -> bool {
        (x as u32) < self.width && (y as u32) < self.height
    }

    /// Cells in one block column: a whole column of blocks is contiguous, and
    /// this is how long it is. See the module header's strip identity.
    const fn column_stride(&self) -> u32 {
        self.blocks_down() * CELLS_PER_BLOCK as u32
    }

    /// The block a tile falls in, or `None` off the facet.
    pub fn block_of(&self, x: u16, y: u16) -> Option<BlockCoord> {
        match self.contains(x, y) {
            true => Some(BlockCoord::containing(x, y)),
            false => None,
        }
    }

    /// Where a block starts in the linear array, or `None` for a block the
    /// facet has not.
    pub const fn index_of(&self, block: BlockCoord) -> Option<BlockIndex> {
        self.extent().index_of(block)
    }

    /// Which block a linear index names — the inverse of [`Self::index_of`].
    pub const fn coord_of(&self, block: BlockIndex) -> Option<BlockCoord> {
        self.extent().coord_of(block)
    }

    /// The north-west tile of the block at a linear index, in tiles.
    pub fn origin_of(&self, block: BlockIndex) -> Option<(u32, u32)> {
        self.coord_of(block).map(BlockCoord::origin)
    }

    /// Every block of the facet, in the array's own order.
    ///
    /// What a `staidx` walk wants: entry n of that file describes block n of
    /// this grid, and this is the only way to say so without a caller building
    /// a [`BlockIndex`] out of a loop counter.
    pub fn blocks(&self) -> impl Iterator<Item = BlockIndex> {
        self.extent().blocks()
    }

    /// Where a tile's cell is in the linear array, or `None` off the facet.
    pub fn cell_index(&self, x: u16, y: u16) -> Option<CellIndex> {
        let block = self.index_of(self.block_of(x, y)?)?;
        // Within the block, Sphere reads `m_Meter[yo * UO_BLOCK_SIZE + xo]` —
        // row-major, the opposite of the block order above it.
        let within = (u32::from(y) % BLOCK_SIZE) * BLOCK_SIZE + (u32::from(x) % BLOCK_SIZE);
        Some(CellIndex(block.0 * CELLS_PER_BLOCK as u32 + within))
    }

    /// Which tile a cell index names — the full inverse of
    /// [`Self::cell_index`].
    ///
    /// The module header's strip identity written out: the column is the index
    /// divided by a column's length, and what is left over is a row-major image
    /// eight tiles wide.
    pub fn tile_of(&self, at: CellIndex) -> Option<(u16, u16)> {
        if at.0 >= self.block_count() * CELLS_PER_BLOCK as u32 {
            return None;
        }
        let column = at.0 / self.column_stride();
        let within = at.0 % self.column_stride();
        let x = column * BLOCK_SIZE + within % BLOCK_SIZE;
        let y = within / BLOCK_SIZE;
        // Both fit: `from_blocks` refuses a facet a `u16` cannot reach, and a
        // loaded one is at most 7,168 tiles across.
        Some((x as u16, y as u16))
    }

    /// The ground at a point, or `None` off the facet.
    pub fn get(&self, x: u16, y: u16) -> Option<LandCell> {
        // No second bounds check: `cell_index` already answered `None` off the
        // facet, and `Cells` holds every block's worth of what is on it.
        Some(self.cells.read(self.cell_index(x, y)?))
    }

    /// Change the ground at one tile. Off the facet it does nothing.
    pub fn set(&mut self, x: u16, y: u16, cell: LandCell) {
        if let Some(at) = self.cell_index(x, y) {
            self.cells.write(at, cell);
        }
    }

    /// One block's sixty-four cells, row-major within the block.
    ///
    /// Never more than one block: `docs/map/new_map_representation/plan.md`'s
    /// direction G holds chunks lazily, and a slice spanning two of them is the
    /// one thing that would make that expensive to reach.
    ///
    /// # Panics
    ///
    /// If `block` came from a different, larger grid. A [`BlockIndex`] is only
    /// ever made by [`Self::index_of`], so that is a caller mixing up two
    /// facets rather than a value it could have got wrong.
    pub fn block(&self, block: BlockIndex) -> &[LandCell] {
        self.cells.block(block)
    }

    /// The cell at a linear index.
    ///
    /// What a walk that stepped its way to an index reads through — see
    /// [`Self::cells_in_row`]. A tile-shaped caller wants [`Self::get`]
    /// instead; this one is for a caller that already has the position.
    ///
    /// # Panics
    ///
    /// If `at` came from a different, larger grid, which is the same argument
    /// as [`Self::block`]: a [`CellIndex`] is only ever made here, so that is a
    /// caller mixing up two facets rather than a value it could have got wrong.
    pub fn cell(&self, at: CellIndex) -> LandCell {
        self.cells.read(at)
    }

    /// The cell one tile east, or `None` at the facet's eastern edge.
    ///
    /// A step, not a fresh derivation. Inside a block east is the next cell;
    /// across the block's eastern edge it is the same row of the block one
    /// column over — a whole block column further on, less the seven cells this
    /// row ran through.
    pub fn east_of(&self, at: CellIndex) -> Option<CellIndex> {
        let (x, _) = self.tile_of(at)?;
        if u32::from(x) + 1 >= self.width {
            return None;
        }
        match u32::from(x) % BLOCK_SIZE == BLOCK_SIZE - 1 {
            true => Some(CellIndex(at.0 + self.column_stride() - (BLOCK_SIZE - 1))),
            false => Some(CellIndex(at.0 + 1)),
        }
    }

    /// The cell one tile south, or `None` at the facet's southern edge.
    ///
    /// Always `+BLOCK_SIZE`, inside a block and across one — the strip identity
    /// in the module header is what makes the two cases the same.
    pub fn south_of(&self, at: CellIndex) -> Option<CellIndex> {
        let (_, y) = self.tile_of(at)?;
        if u32::from(y) + 1 >= self.height {
            return None;
        }
        Some(CellIndex(at.0 + BLOCK_SIZE))
    }

    /// The cells of one row of tiles, from `from_x` to `to_x` inclusive, east
    /// to west — stepping, rather than deriving an index per tile.
    ///
    /// The walk order becomes a property of this iterator rather than of every
    /// caller's loop nesting, which is the half of the point that outlives the
    /// arithmetic: `client/render`'s `depth::Order` gives every tile on one
    /// anti-diagonal the same key and breaks the tie by what was walked last,
    /// so the order a rectangle is walked in is currently visible in the
    /// picture. One iterator is where a later direction can change that.
    ///
    /// A row off the facet, or a range that runs backwards, is empty; a range
    /// running off the eastern edge stops there.
    pub fn cells_in_row(&self, y: u16, from_x: u16, to_x: u16) -> impl Iterator<Item = CellIndex> + '_ {
        let wanted = match from_x <= to_x {
            true => usize::from(to_x - from_x) + 1,
            false => 0,
        };
        std::iter::successors(self.cell_index(from_x, y), move |at| self.east_of(*at)).take(wanted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::LandTile;

    /// A 2x2-block grid (16x16 tiles) whose cells number themselves straight
    /// down the file — cell `n` of the array carries tile id `n`.
    ///
    /// So an assertion about a tile id here is an assertion about a *position in
    /// the file*, which is what every test below is really about.
    fn grid_16x16() -> LandGrid {
        LandGrid::from_file_order(
            16,
            16,
            (0..4 * CELLS_PER_BLOCK).map(|at| LandCell {
                tile: LandTile(at as u16),
                z: 0,
            }),
        )
    }

    #[test]
    fn block_order_is_column_major() {
        // The one that silently transposes the world. With a 16x16 grid there
        // are two blocks down, so block (1,0) is index 2 — not 1.
        let grid = grid_16x16();

        assert_eq!(
            grid.get(0, 0).unwrap().tile,
            LandTile(0),
            "(0,0) is block 0, cell 0"
        );
        assert_eq!(
            grid.get(8, 0).unwrap().tile,
            LandTile((2 * CELLS_PER_BLOCK) as u16),
            "(8,0) is block *2*: bx=1, by=0, blocks_down=2 -> 1*2+0",
        );
        assert_eq!(
            grid.get(0, 8).unwrap().tile,
            LandTile(CELLS_PER_BLOCK as u16),
            "(0,8) is block 1: bx=0, by=1 -> 0*2+1",
        );

        // And the same statement about the index itself, without going through
        // a cell: it is `index_of` that everything else is built on.
        let index = |x, y| grid.index_of(BlockCoord { x, y }).unwrap().get();
        assert_eq!((index(0, 0), index(0, 1), index(1, 0), index(1, 1)), (0, 1, 2, 3));
    }

    /// The length invariant has one home. A loader handing over the wrong number
    /// of cells is refused at the door rather than found later as a block whose
    /// slice ran off the end.
    #[test]
    #[should_panic(expected = "4 blocks hold 256 cells")]
    fn a_short_facet_is_refused_where_the_cells_are_taken() {
        LandGrid::from_file_order(16, 16, std::iter::empty());
    }

    #[test]
    fn cells_within_a_block_are_row_major() {
        // Sphere: `m_Meter[yo * UO_BLOCK_SIZE + xo]`. The opposite of the block
        // order, which is exactly why it is worth a test.
        let grid = grid_16x16();
        assert_eq!(grid.get(1, 0).unwrap().tile, LandTile(1), "x moves by one");
        assert_eq!(grid.get(0, 1).unwrap().tile, LandTile(8), "y moves by a row");
        assert_eq!(
            grid.get(7, 7).unwrap().tile,
            LandTile(63),
            "the block's far corner"
        );
    }

    /// The inverse [`crate::map`]'s static loader used to open-code backwards.
    ///
    /// A block index that came from a tile, turned back into the tile the block
    /// starts at. Get this wrong — swap the divide and the remainder — and every
    /// static past the first block column is placed somewhere else, in a file
    /// that parses perfectly.
    #[test]
    fn a_blocks_origin_is_the_tile_it_started_from() {
        let grid = LandGrid::from_blocks(5, 3, |_, _| LandCell::default());
        for y in 0..24u16 {
            for x in 0..40u16 {
                let block = grid.index_of(grid.block_of(x, y).unwrap()).unwrap();
                let origin = grid.origin_of(block).unwrap();
                assert_eq!(
                    origin,
                    (
                        u32::from(x) / BLOCK_SIZE * BLOCK_SIZE,
                        u32::from(y) / BLOCK_SIZE * BLOCK_SIZE
                    ),
                    "({x}, {y}) came back to the wrong block corner",
                );
            }
        }

        // Every block, once, and nothing past the facet.
        let seen: std::collections::HashSet<(u32, u32)> = (0..grid.block_count())
            .map(|at| grid.origin_of(BlockIndex(at)).unwrap())
            .collect();
        assert_eq!(seen.len(), 15);
        assert_eq!(grid.origin_of(BlockIndex(15)), None, "a block the facet has not");
        assert_eq!(
            grid.index_of(BlockCoord { x: 5, y: 0 }),
            None,
            "a column it has not"
        );
        assert_eq!(grid.index_of(BlockCoord { x: 0, y: 3 }), None, "a row it has not");
    }

    #[test]
    fn every_cell_is_reachable_exactly_once() {
        // If the indexing were wrong in a way the spot-checks missed, two points
        // would map to one cell and some cell would be unreachable.
        let grid = grid_16x16();
        let mut seen = std::collections::HashSet::new();
        for y in 0..16u16 {
            for x in 0..16u16 {
                let at = grid.cell_index(x, y).expect("on the facet");
                assert!(seen.insert(at), "({x},{y}) collides with another point");
                assert_eq!(grid.tile_of(at), Some((x, y)), "the inverse disagrees");
            }
        }
        assert_eq!(seen.len(), 16 * 16);
    }

    /// The module header's derivation, held against the plain spelling.
    ///
    /// `tile_of`, `east_of` and `south_of` are all written in terms of "a block
    /// column is an eight-wide strip", which is *derived* from the two orders
    /// rather than being one of them. If it were wrong, those three would be
    /// wrong together and consistently — so it is checked against the
    /// block-then-cell arithmetic itself, on a facet that is neither square nor
    /// one block wide.
    #[test]
    fn the_two_orders_compose_into_a_strip() {
        let grid = LandGrid::from_blocks(5, 3, |_, _| LandCell::default());
        for y in 0..24u32 {
            for x in 0..40u32 {
                let plain = ((x / BLOCK_SIZE) * grid.blocks_down() + y / BLOCK_SIZE) * CELLS_PER_BLOCK as u32
                    + (y % BLOCK_SIZE) * BLOCK_SIZE
                    + x % BLOCK_SIZE;
                let strip = (x / BLOCK_SIZE) * grid.column_stride() + y * BLOCK_SIZE + x % BLOCK_SIZE;
                assert_eq!(plain, strip, "({x}, {y})");
                assert_eq!(grid.cell_index(x as u16, y as u16).unwrap().get(), plain);
            }
        }
    }

    /// A step east or south lands where a fresh derivation would, everywhere.
    ///
    /// The transitions are the arithmetic's payoff and its risk: `+1` inside a
    /// block and a whole column across its edge, against `cell_index` written
    /// out per tile. The facet is five blocks by three so that both crossings
    /// happen many times and the two are not the same number.
    #[test]
    fn a_step_lands_where_a_fresh_index_would() {
        let grid = LandGrid::from_blocks(5, 3, |_, _| LandCell::default());
        for y in 0..24u16 {
            for x in 0..40u16 {
                let at = grid.cell_index(x, y).unwrap();
                assert_eq!(grid.east_of(at), grid.cell_index(x + 1, y), "east of ({x}, {y})");
                assert_eq!(
                    grid.south_of(at),
                    grid.cell_index(x, y + 1),
                    "south of ({x}, {y})"
                );
            }
        }

        // The edges are `None` rather than the next row's first tile, which is
        // where a strip walk would wrap without the bound.
        let east_edge = grid.cell_index(39, 5).unwrap();
        assert_eq!(grid.east_of(east_edge), None, "the facet's eastern edge");
        let south_edge = grid.cell_index(5, 23).unwrap();
        assert_eq!(grid.south_of(south_edge), None, "the facet's southern edge");
    }

    /// The row iterator is the tile walk written as steps, and nothing else.
    #[test]
    fn a_row_is_the_tile_walk_written_as_steps() {
        let grid = LandGrid::from_blocks(5, 3, |_, _| LandCell::default());
        for y in 0..24u16 {
            for (from_x, to_x) in [(0u16, 39u16), (7, 16), (9, 9), (16, 7), (0, 100), (38, 45)] {
                let walked: Vec<CellIndex> = grid.cells_in_row(y, from_x, to_x).collect();
                let derived: Vec<CellIndex> = (from_x..=to_x.min(39))
                    .map(|x| grid.cell_index(x, y).unwrap())
                    .collect();
                assert_eq!(walked, derived, "row {y}, {from_x}..={to_x}");
            }
        }
        assert_eq!(grid.cells_in_row(24, 0, 39).count(), 0, "a row the facet has not");
    }

    #[test]
    fn off_the_facet_is_none_not_a_panic() {
        let grid = grid_16x16();
        assert_eq!(grid.get(16, 0), None);
        assert_eq!(grid.get(0, 16), None);
        assert_eq!(grid.get(u16::MAX, u16::MAX), None);
        assert!(!grid.contains(16, 15));
        assert!(grid.contains(15, 15));
    }

    /// A grid built tile by tile holds what a grid taken straight from the file
    /// holds.
    ///
    /// The round trip is the whole point: [`LandGrid::from_blocks`] decides
    /// where a cell goes and [`LandGrid::cell_index`] decides where it is read
    /// back from, and they are two separate walks of the same column-major
    /// order. Get either nesting backwards and the facet is transposed — which
    /// parses perfectly, draws plausibly, and is the failure this module is
    /// about.
    #[test]
    fn building_by_tile_matches_the_files_own_order() {
        let from_file = grid_16x16();
        let built = LandGrid::from_blocks(2, 2, |x, y| from_file.get(x, y).expect("inside the fixture"));

        assert_eq!((built.width(), built.height()), (16, 16));
        for y in 0..16u16 {
            for x in 0..16u16 {
                assert_eq!(
                    built.get(x, y),
                    from_file.get(x, y),
                    "({x}, {y}) is a different cell"
                );
            }
        }
        // The fixture's tiles are all distinct, so the comparison above could
        // not have passed on a grid that happens to be uniform.
        let distinct: std::collections::HashSet<LandTile> = (0..16u16)
            .flat_map(|y| (0..16u16).map(move |x| (x, y)))
            .map(|(x, y)| built.get(x, y).unwrap().tile)
            .collect();
        assert_eq!(distinct.len(), 16 * 16);
    }

    /// Every tile is asked for, once, by world coordinate — the contract the
    /// callback rests on and the reason a caller never sees a block.
    #[test]
    fn building_asks_for_each_tile_exactly_once() {
        let mut asked = Vec::new();
        let grid = LandGrid::from_blocks(3, 2, |x, y| {
            asked.push((x, y));
            LandCell::default()
        });

        assert_eq!(asked.len(), 24 * 16);
        let unique: std::collections::HashSet<(u16, u16)> = asked.iter().copied().collect();
        assert_eq!(unique.len(), asked.len(), "a tile was asked for twice");
        for y in 0..16u16 {
            for x in 0..24u16 {
                assert!(unique.contains(&(x, y)), "({x}, {y}) was never asked for");
            }
        }
        assert_eq!((grid.width(), grid.height()), (24, 16));
        assert_eq!((grid.blocks_wide(), grid.blocks_down()), (3, 2));
        assert_eq!(grid.block_count(), 6);
    }

    /// A block hands back its own sixty-four cells and no neighbour's.
    #[test]
    fn a_block_is_its_own_sixty_four_cells() {
        let grid = grid_16x16();
        for (bx, by) in [(0u32, 0u32), (0, 1), (1, 0), (1, 1)] {
            let block = grid.index_of(BlockCoord { x: bx, y: by }).unwrap();
            let cells = grid.block(block);
            assert_eq!(cells.len(), CELLS_PER_BLOCK);
            let (origin_x, origin_y) = BlockCoord { x: bx, y: by }.origin();
            for local_y in 0..BLOCK_SIZE {
                for local_x in 0..BLOCK_SIZE {
                    let want = grid.get((origin_x + local_x) as u16, (origin_y + local_y) as u16);
                    let got = cells[(local_y * BLOCK_SIZE + local_x) as usize];
                    assert_eq!(want, Some(got), "block ({bx}, {by}) cell ({local_x}, {local_y})");
                }
            }
        }
    }

    #[test]
    fn a_written_cell_is_the_one_read_back() {
        let mut grid = grid_16x16();
        let cell = LandCell {
            tile: LandTile(999),
            z: -12,
        };
        grid.set(9, 3, cell);
        assert_eq!(grid.get(9, 3), Some(cell));
        // And nothing else moved: the neighbour that shares a block, and the
        // one in the block column next door.
        assert_eq!(
            grid.get(8, 3).unwrap().tile,
            LandTile((2 * CELLS_PER_BLOCK + 3 * 8) as u16)
        );
        assert_eq!(grid.get(1, 3).unwrap().tile, LandTile(3 * 8 + 1));

        grid.set(16, 0, cell);
        assert_eq!(grid.get(16, 0), None, "off the facet writes nothing");
    }
}
