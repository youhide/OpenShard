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
use openshard_tiles::LandTileId;

use crate::grid::{BlockCoord, BlockExtent, BlockIndex, LandGrid};

/// Tiles along each side of a map block.
pub const BLOCK_SIZE: u32 = 8;
/// Cells in a block.
pub const CELLS_PER_BLOCK: usize = (BLOCK_SIZE * BLOCK_SIZE) as usize;

/// One cell of ground.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LandCell {
    /// Index into the land table of `tiledata.mul`.
    pub tile: LandTileId,
    /// The ground's height here.
    pub z: i8,
}

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

/// One block of a facet as it is to become: its ground and everything standing
/// on it, whole.
///
/// What [`WorldMap::replace_blocks`] takes, and it borrows rather than owns —
/// the arrays it points into belong to whatever the block arrived in, which is
/// a [`Chunk`](crate::chunk::Chunk) at the only caller.
///
/// **A block and not a tile**, because that is the unit the two arrays agree in:
/// a block's cells are a fixed slice of the land and its items are one run with
/// a count, so replacing *part* of one would leave the count describing
/// something that is no longer there. A caller with one tile to change wants
/// [`crate::patch`], which is the other kind of change entirely.
#[derive(Debug)]
pub struct BlockPatch<'a> {
    /// Which block of the facet, in the land's own order.
    at: BlockIndex,
    /// Its sixty-four cells, row-major within the block — [`LandGrid::block`]'s
    /// order, because that is where it will go.
    land: &'a [LandCell],
    /// Everything standing in it. Any order: [`WorldMap::replace_blocks`]
    /// imposes the sort, for the reason [`WorldMap::from_parts`] does.
    statics: &'a [StaticItem],
}

impl<'a> BlockPatch<'a> {
    /// Name a block and what is to be in it.
    ///
    /// # Panics
    ///
    /// If `land` is not exactly one block's cells. The check is here rather than
    /// at the write so that a caller assembling a list of these finds out which
    /// block it got wrong.
    #[must_use]
    pub fn new(at: BlockIndex, land: &'a [LandCell], statics: &'a [StaticItem]) -> Self {
        assert_eq!(land.len(), CELLS_PER_BLOCK, "a block is {CELLS_PER_BLOCK} cells");
        Self { at, land, statics }
    }

    /// Which block of the facet this replaces.
    #[must_use]
    pub const fn at(&self) -> BlockIndex {
        self.at
    }
}

/// One facet: the ground and the statics on it.
///
/// The whole thing is in memory. That is the design — the database is never
/// touched inside a tick, and a facet is under 100MB.
pub struct WorldMap {
    /// The ground, and the only thing that knows the order it is in.
    land: LandGrid,
    /// Every static on the facet, its blocks in the order the land's own
    /// [`BlockIndex`] gives them — **one run**, not one vector per block.
    ///
    /// Felucca is 2,906,871 statics over 120,744 non-empty blocks. Per block
    /// that was 120,745 allocations and 38.2 MiB, most of it the `Vec` headers
    /// and the slack of a hundred thousand tiny vectors; as one run it is two
    /// allocations and 29.6 MiB, and every accessor still hands back a
    /// `&[StaticItem]` because a block is still contiguous.
    ///
    /// **Each block is sorted by the tile its items stand on** — see
    /// [`WorldMap::statics_at`], which is what the order is for, and [`tile_key`],
    /// which is the order.
    ///
    /// The sort is stable, so two statics on one tile stay in the order the file
    /// has them. That is not a nicety: the client draws them in file order and
    /// `client/render`'s `statics::pick` breaks a tie by taking the last, so a
    /// resort that swapped two items on one tile would change which of them is on
    /// top.
    statics: Vec<StaticItem>,
    /// Where each block's items are: one entry per block of the land, in the
    /// land's own [`BlockIndex`] order. Block `i` owns
    /// `statics[blocks[i].base..][..blocks[i].count]`.
    ///
    /// Indexed by **the same [`BlockIndex`]** the land is — which is
    /// load-bearing and which nothing enforces: the two arrays are built side by
    /// side from one block count and are only ever addressed through
    /// [`LandGrid::index_of`], so a block's cells and a block's statics cannot
    /// come apart without that call being wrong for both.
    ///
    /// **A table and not a prefix sum, since `what_a_change_costs.md`'s S3.** The
    /// two are the same thing to a *reader* — both answer a block's run in two
    /// reads — and the difference is what happens when a block's item count
    /// moves. A prefix sum *is* the ordering, so re-laying one block in place
    /// pushes every run after it and repairs 458,752 offsets; a table lets the
    /// block be written at the **end** of the run and its entry repointed, which
    /// is O(the block). The price is one extra `u32` a block, 1.75 MiB on a
    /// 150 MiB world, and the runs the repointing orphans — see
    /// [`dead`](Self::dead).
    blocks: Vec<BlockRun>,
    /// How many of [`statics`](Self::statics) no block addresses any more.
    ///
    /// Zero for a facet built by an importer, and it grows only through the
    /// three writers — [`place_static`](Self::place_static),
    /// [`remove_static`](Self::remove_static) and
    /// [`replace_blocks`](Self::replace_blocks). The rule is the span layer's,
    /// because it is the same trade: never compact while a session is editing,
    /// except that dead items exceeding live ones repack the run — at which
    /// point the facet is laid out in block order again and the count is zero.
    dead: usize,
}

/// Where one block's statics are in the facet's run.
///
/// Two `u32`s rather than a start and the next block's start: a count is a fact
/// about *this* block, and reading it out of the neighbour's base is what makes
/// a prefix sum an ordering rather than an index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct BlockRun {
    /// The first item.
    base: u32,
    /// How many there are. Zero for the 73.7% of Britannia's blocks that hold
    /// nothing, whose `base` is then never read.
    count: u32,
}

/// A static's sortable coordinate within its block: **`y` first**, then `x`.
///
/// This is deliberately not a bare coordinate tuple: it is valid only for the
/// order `WorldMap` keeps its statics in.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct StaticTileKey(u16, u16);

/// Where a static sorts within its block: by tile, **`y` first**.
///
/// The row before the column, and that is the whole reason this is a named
/// function rather than a tuple written twice. A caller walking a rectangle
/// walks it row by row — `client/render`'s `statics::for_each_static_in`, which
/// is every walk of the map this workspace makes — so a row has to be
/// *contiguous* for [`WorldMap::statics_in_row`] to hand one back as a slice. Sorted
/// the other way it would be eight scattered runs.
///
/// Only the low three bits of each coordinate matter within a block, but the
/// whole of both is used: the items of one block share the same high bits, so
/// the two orders are the same one, and the key is then the same comparison the
/// lookups make.
fn tile_key(item: &StaticItem) -> StaticTileKey {
    StaticTileKey(item.y, item.x)
}

impl fmt::Debug for WorldMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorldMap")
            .field("size", &format!("{}x{}", self.width(), self.height()))
            .field("facet", &self.facet_name())
            .field("statics", &self.statics.len())
            .finish()
    }
}

impl WorldMap {
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
    /// No statics: [`WorldMap::statics_at`] is empty everywhere and
    /// [`WorldMap::static_count`] is zero — until [`WorldMap::place_static`] puts one
    /// there. [`WorldMap::from_parts`] is the other way in, and the one an importer
    /// takes.
    ///
    /// And no facet identity. A size nobody ships is an ordinary thing to ask
    /// for here, so [`WorldMap::facet_name`] may honestly answer "unknown facet".
    /// This does not inherit an importer's Malas/Ter Mur ambiguity, and cannot:
    /// there is no block count to deduce a shape from, because the caller said
    /// what the shape is.
    ///
    /// # Panics
    ///
    /// If the facet would be wider or taller than a `u16` coordinate can reach.
    /// Every tile past that is one [`WorldMap::land`] could never be asked about, so
    /// such a map is memory that exists to be unreachable; the largest facet a
    /// client ships is 7,168 tiles across.
    pub fn from_blocks(extent: BlockExtent, cell: impl FnMut(u16, u16) -> LandCell) -> Self {
        let land = LandGrid::from_blocks(extent, cell);
        let blocks = vec![BlockRun { base: 0, count: 0 }; land.block_count() as usize];
        Self {
            land,
            statics: Vec::new(),
            blocks,
            dead: 0,
        }
    }

    /// Build a facet from land and statics an importer already decoded.
    ///
    /// **`statics` is one run, block by block in [`LandGrid::blocks`]' order**,
    /// and `counts` says how long each block's part of it is — the layout the
    /// map keeps them in, so an importer that already reads a facet block by
    /// block hands over what it built rather than a vector per block that would
    /// be flattened here. It is [`crate::chunk::Chunk::from_parts`]' shape, and
    /// the prefix sum is this type's for that function's reason: a second
    /// decoder cannot accumulate it differently from the first.
    ///
    /// The other half of why this exists rather than a set of public fields:
    /// **the sort is this type's invariant, not the decoder's.**
    /// [`WorldMap::statics_at`] and [`WorldMap::statics_in_row`] are binary
    /// searches over `(y, x)` order — see [`tile_key`] — so a decoder that
    /// handed over an unsorted block would not fail here, it would make every
    /// later lookup quietly find nothing. Sorting here means no importer can get
    /// it wrong, and a second importer cannot get it wrong *differently* from
    /// the first.
    ///
    /// The sort is **stable**, which is the half with a consequence a player can
    /// see: two statics on one tile keep the order the importer produced them
    /// in, and `client/render`'s `statics::pick` breaks a tie by taking the
    /// last, so reordering them would change which one is on top and which one
    /// a click holds.
    ///
    /// # Panics
    ///
    /// If `counts` does not hold exactly one entry per block of `land`, or if
    /// the counts do not add up to `statics`. The counts and the land share
    /// [`LandGrid`]'s own [`BlockIndex`], which is what lets a block's cells and
    /// a block's items be found by one number; a length that disagrees is an
    /// importer disagreeing with itself, and every lookup past the short end
    /// would silently be a block with nothing on it.
    #[must_use]
    pub fn from_parts(land: LandGrid, mut statics: Vec<StaticItem>, counts: &[u32]) -> Self {
        assert_eq!(
            counts.len(),
            land.block_count() as usize,
            "an importer handed over statics for a facet of a different size",
        );
        let mut blocks = Vec::with_capacity(counts.len());
        let mut total: u32 = 0;
        for count in counts {
            blocks.push(BlockRun {
                base: total,
                count: *count,
            });
            total = total
                .checked_add(*count)
                .expect("a facet of fewer than 4G statics");
        }
        assert_eq!(
            total as usize,
            statics.len(),
            "an importer's block counts do not add up to the statics it handed over",
        );

        for block in &blocks {
            let from = block.base as usize;
            statics[from..from + block.count as usize].sort_by_key(tile_key);
        }
        // The base layer is one run and it is done growing: a loader that
        // pushed its way to three million items is holding up to twice the
        // memory it needs, and this is the one place that knows the growing is
        // over.
        statics.shrink_to_fit();
        Self {
            land,
            statics,
            blocks,
            dead: 0,
        }
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
    /// [`WorldMap::statics_in_row`]'s other half, and it exists for the same reason:
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
    /// [`WorldMap::land`] answers `None` for.
    ///
    /// A range that runs backwards is empty.
    pub fn land_in_row(&self, y: u16, from_x: u16, to_x: u16) -> impl Iterator<Item = LandCell> + '_ {
        self.land
            .cells_in_row(y, from_x, to_x)
            .map(|at| self.land.cell(at))
    }

    /// Change the ground at one tile.
    ///
    /// The other half of building a scene — see [`WorldMap::place_static`] for what
    /// that is and why both of these are `pub`. Off the map it does nothing;
    /// [`crate::patch`] checks the tile is there before it calls, so that a
    /// change nobody could see is an error rather than a silent nothing.
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
    /// `openshard_tiles::TileData::set_static_tile` is: the tests that want it
    /// are in other crates, and this repository ships no client files to build a
    /// fixture from.
    ///
    /// **The engine's one other caller is [`crate::patch`]**, and the
    /// difference is the whole point of that module: a static written in at
    /// runtime by a *system* is one end of the wire disagreeing with the other
    /// about a tile they both drew, and a static written in by a published
    /// patch is a change to the world that both ends can be told about.
    ///
    /// A static outside the map is dropped: there is no block to keep it in, and
    /// [`WorldMap::statics_at`] could never return it. The patch applier checks
    /// first, for the reason [`WorldMap::set_land`] gives.
    ///
    /// It goes in after everything already standing on its own tile — where a
    /// push used to put it — which is what keeps the sort an invariant of the
    /// type rather than of the loader, and what makes the ordinal of a
    /// just-added static knowable without a search.
    ///
    /// # What it costs, and why that is not the way to load a facet
    ///
    /// The block's run is **rewritten at the end of the statics** with the new
    /// item in its place, and the block's entry repointed at it: O(the block),
    /// which at Britannia's median is eighteen items. Nothing else on the facet
    /// moves and no other block's entry is touched. What it leaves behind is the
    /// run that was there, counted as garbage — see the type's `dead` field for
    /// the rule that eventually repacks it.
    ///
    /// It is still not the way to load a facet: three million calls would be
    /// three million copies of a growing run and a repack every time the garbage
    /// caught up. [`WorldMap::from_parts`] is the door that assembles a facet,
    /// and the two `.mul`/base-set importers are both through it.
    pub fn place_static(&mut self, item: StaticItem) {
        let Some(block) = self.block_index(item.x, item.y) else {
            return;
        };
        let (from, to) = self.span(block);
        let at = self.statics[from..to].partition_point(|had| tile_key(had) <= tile_key(&item));
        let mut run = Vec::with_capacity(to - from + 1);
        run.extend_from_slice(&self.statics[from..from + at]);
        run.push(item);
        run.extend_from_slice(&self.statics[from + at..to]);
        self.relocate(block, &run);
        self.repack_if_mostly_garbage();
    }

    /// Take the `nth` static standing on a tile off the map, and hand it back.
    ///
    /// [`WorldMap::place_static`]'s inverse, and [`crate::patch`]'s alone: `nth`
    /// counts in [`WorldMap::statics_at`]'s order, which is what makes it the
    /// identity that module's header argues a static needs. `None` for a tile
    /// off the map or an `nth` past what stands there — and in both cases
    /// nothing was removed.
    ///
    /// A removal shifts the ordinals of everything after it on the tile. That
    /// is not a defect to be designed away: an ordinal is only ever read
    /// against a stated revision, and taking a static out is what produces the
    /// next one.
    ///
    /// **Cheaper than [`WorldMap::place_static`], and this is where the two
    /// differ.** A run that loses an item still fits where it stands, so the
    /// items after it inside *this block* close the gap and the count drops by
    /// one: nothing is relocated and the last slot of the run becomes the
    /// garbage. An addition has nowhere to put its item without moving the whole
    /// facet, which is why it is the one that goes to the end.
    pub fn remove_static(&mut self, x: u16, y: u16, nth: usize) -> Option<StaticItem> {
        let block = self.block_index(x, y)?;
        let (start, end) = self.span(block);
        let slot = &self.statics[start..end];
        // The same two searches [`WorldMap::statics_at`] makes, over the same sorted
        // run: the first item of the tile, and how many of them there are.
        let key = StaticTileKey(y, x);
        let from = slot.partition_point(|item| tile_key(item) < key);
        let count = slot[from..].partition_point(|item| tile_key(item) == key);
        if nth >= count {
            return None;
        }
        let at = start + from + nth;
        let gone = self.statics[at];
        self.statics.copy_within(at + 1..end, at);
        self.blocks[block.get() as usize].count -= 1;
        self.dead += 1;
        self.repack_if_mostly_garbage();
        Some(gone)
    }

    /// Write one block's items at the end of the run and point the block at
    /// them, leaving what was there as garbage.
    ///
    /// The move S3 is: a block's run is addressed by a table entry rather than
    /// by a prefix sum, so a block whose length changed is written somewhere it
    /// fits instead of pushing every run after it along. Shared by the two
    /// writers that can lengthen a block — a placed static and an arriving chunk.
    ///
    /// A run that is exactly as long as the one it replaces is written **where it
    /// stands**, which is every edit to the ground: a `.setland` sends a chunk
    /// holding the same items it replaces, and relocating them would be
    /// manufacturing garbage out of an edit that moved no statics at all.
    fn relocate(&mut self, block: BlockIndex, run: &[StaticItem]) {
        let entry = self.blocks[block.get() as usize];
        let count = u32::try_from(run.len()).expect("a block of fewer than 4G statics");
        if count == entry.count {
            let at = entry.base as usize;
            self.statics[at..at + run.len()].copy_from_slice(run);
            return;
        }
        self.dead += entry.count as usize;
        self.blocks[block.get() as usize] = BlockRun {
            base: u32::try_from(self.statics.len()).expect("a facet of fewer than 4G statics"),
            count,
        };
        self.statics.extend_from_slice(run);
    }

    /// Lay the statics out in block order again, once the garbage outweighs what
    /// is reachable.
    ///
    /// The span layer's rule, one crate down and for the same reason: a publish
    /// is an operator typing, so compacting after each one would pay a facet-wide
    /// pass for a block-sized edit — but garbage that has grown past the live
    /// items is memory nothing can reach and a run whose next `extend` reallocates
    /// twice what it needs. Between the two, doing it rarely and completely.
    fn repack_if_mostly_garbage(&mut self) {
        if self.dead <= self.statics.len() - self.dead {
            return;
        }
        let mut packed = Vec::with_capacity(self.statics.len() - self.dead);
        for block in &mut self.blocks {
            let from = block.base as usize;
            block.base = u32::try_from(packed.len()).expect("a facet of fewer than 4G statics");
            packed.extend_from_slice(&self.statics[from..from + block.count as usize]);
        }
        self.statics = packed;
        self.dead = 0;
    }

    /// Put whole blocks of the facet back, in one pass.
    ///
    /// **What a square of the world that arrived whole goes in through**, and the
    /// third door into the statics after [`WorldMap::from_parts`] and
    /// [`WorldMap::place_static`]. The difference from both is the unit: a patch
    /// names one *tile*, an importer builds a *facet*, and this replaces some
    /// number of *blocks* — which is what a chunk is made of, and what
    /// [`crate::chunk::apply`] hands over.
    ///
    /// # What it costs
    ///
    /// **O(the blocks that arrived**, since S3, and the shape of the answer is
    /// the same one at both ends of the wire — see
    /// `docs/map/new_map_representation/what_a_change_costs.md`.
    ///
    /// Land is free of the question entirely: a block is
    /// [`CELLS_PER_BLOCK`] cells wherever it sits, so each one is written where
    /// the old one was and nothing else moves — see [`LandGrid::set_block`].
    ///
    /// The statics were the part with a cost. A block's items are one run in a
    /// facet-wide vector, and while that run was addressed by a **prefix sum** a
    /// block whose item count changed moved every static after it — one memmove
    /// of the tail, plus an arithmetic pass over 1.8 MiB of offsets. Measured on
    /// Felucca, that was 0.1 ms for a block that kept its count, and between
    /// 0.02 ms and **1.3 ms** for one that did not, in proportion to how much of
    /// the facet stood after it. The table replaced the prefix sum for exactly
    /// this: a block that grew or shrank is written at the end of the run and its
    /// entry repointed, so no block but the ones named is read or written at all.
    ///
    /// Two costs remain and both are the type's rather than this call's: the
    /// reallocation the first *addition* into an importer-built facet pays, since
    /// [`WorldMap::from_parts`] shrinks the run to fit (**7.05 ms** on Felucca,
    /// then 0.36, then 0.04); and the repack that eventually reclaims what the
    /// repointing orphaned, which is a facet-wide pass made once per doubling
    /// rather than once per publish.
    ///
    /// Blocks still arrive as a set rather than one call each — a chunk is
    /// sixty-four of them and a publish is atomic — but scattered blocks no
    /// longer cost anything extra: there is no span between them to rebuild.
    ///
    /// The sort inside each arriving block is imposed here, for
    /// [`WorldMap::from_parts`]' reason: the `(y, x)` order is this type's
    /// invariant and not its callers', and a block handed over unsorted would not
    /// fail, it would make every later binary search quietly find nothing.
    ///
    /// # Panics
    ///
    /// If `blocks` is empty, if they are not in strictly ascending
    /// [`BlockIndex`] order — which is what makes them one span and each block
    /// at most once — or if any of them is a block this facet has not.
    pub fn replace_blocks(&mut self, blocks: &[BlockPatch<'_>]) {
        assert!(!blocks.is_empty(), "replacing no blocks is not a change");
        assert!(
            blocks.windows(2).all(|pair| pair[0].at < pair[1].at),
            "blocks must arrive in the facet's own order, each of them once",
        );
        let last = blocks[blocks.len() - 1].at.get() as usize;
        assert!(
            last < self.blocks.len(),
            "a block this facet has not: {last} of {}",
            self.blocks.len(),
        );

        // One block at a time, and each one is its own answer: nothing between
        // two named blocks is read, because nothing between them moved.
        let mut run: Vec<StaticItem> = Vec::new();
        for patch in blocks {
            run.clear();
            run.extend_from_slice(patch.statics);
            // The sort inside each arriving block is imposed here, for
            // `from_parts`' reason: the `(y, x)` order is this type's invariant
            // and not its callers'.
            run.sort_by_key(tile_key);
            self.land.set_block(patch.at, patch.land);
            self.relocate(patch.at, &run);
        }
        self.repack_if_mostly_garbage();
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
        let key = StaticTileKey(y, x);
        let from = block.partition_point(|item| tile_key(item) < key);
        let count = block[from..].partition_point(|item| tile_key(item) == key);
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
    /// The order is exactly [`WorldMap::statics_at`]'s, row by row: `client/render`
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
    /// [`WorldMap::statics_in_block`]'s other half, and it exists for the same
    /// caller: something that takes a whole block at a time rather than a
    /// rectangle — [`crate::chunk::Chunk::of`] is the one in this crate. A
    /// block the facet has not is empty, which is the same answer
    /// [`WorldMap::statics_in_block`] gives.
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
    /// and eight calls to [`WorldMap::statics_in_row`] would be eight binary searches
    /// for a run that is already contiguous.
    ///
    /// The order is [`tile_key`]'s — `(y, x)` — which is [`WorldMap::statics_in_row`]'s
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
    /// Two of the three places `statics` is sliced, and it goes through the
    /// land's own [`LandGrid::index_of`] — which is what the offsets' doc
    /// comment means by the two arrays sharing an index.
    fn statics_of(&self, block: BlockCoord) -> &[StaticItem] {
        self.land.index_of(block).map_or(NO_STATICS, |block| {
            let (from, to) = self.span(block);
            &self.statics[from..to]
        })
    }

    /// Where one block's items begin and end in the run.
    ///
    /// The table holds an entry for every block a [`BlockIndex`] can name, which
    /// is what makes this infallible where [`WorldMap::statics_of`] is not. Two
    /// reads, exactly as the prefix sum it replaced was — the difference between
    /// the two is at the writing end and not here.
    fn span(&self, block: BlockIndex) -> (usize, usize) {
        let run = self.blocks[block.get() as usize];
        let from = run.base as usize;
        (from, from + run.count as usize)
    }

    /// Which block a tile's statics are in, or `None` off the map.
    fn block_index(&self, x: u16, y: u16) -> Option<BlockIndex> {
        self.land.index_of(self.land.block_of(x, y)?)
    }

    /// How many statics the facet holds.
    ///
    /// What is *reachable*, not what the run is long: a facet that has been
    /// edited carries the runs its repointing orphaned until they are repacked,
    /// and those are not statics anybody can find.
    pub fn static_count(&self) -> usize {
        self.statics.len() - self.dead
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
        self.land_and_corners(x, y).map(|(_, corners)| corners)
    }

    /// The tile's own cell **and** its four corner heights, from one walk.
    ///
    /// The pair, because the two callers that matter want both and asking
    /// separately reads `(x, y)` twice: `Spans::ground` needs the graphic to
    /// know whether the land is a surface at all and the corners to know where
    /// its middle is, and the bake's column builder needs the same. A land read
    /// is ~1.2 ns and a node expansion makes about forty of them, so the
    /// duplicate is a twentieth of the expansion for nothing.
    ///
    /// Inside a block this is [`LandGrid::corner_quad`] — one address
    /// derivation for four cells, which is 76.6% of the facet. On a block's far
    /// edge it is the walk it always was, and there the off-facet fallback
    /// matters: a corner past the edge is the tile's *own* height, so the world
    /// does not fall away at its border.
    pub fn land_and_corners(&self, x: u16, y: u16) -> Option<(LandCell, [i8; 4])> {
        if let Some(quad) = self.land.corner_quad(x, y) {
            return Some((quad[0], quad.map(|cell| cell.z)));
        }
        let own = self.land(x, y)?;
        let at = |corner_x: Option<u16>, corner_y: Option<u16>| match (corner_x, corner_y) {
            (Some(corner_x), Some(corner_y)) => self.land(corner_x, corner_y).map_or(own.z, |cell| cell.z),
            _ => own.z,
        };
        let (east, south) = (x.checked_add(1), y.checked_add(1));
        Some((
            own,
            [own.z, at(east, Some(y)), at(Some(x), south), at(east, south)],
        ))
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

/// [`WorldMap::average_land_z`]'s arithmetic, for a caller that already has the four
/// corners and would otherwise read them a second time.
///
/// `corners` is [`WorldMap::land_corners`] order: top, right, left, bottom.
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
    /// what is left here is what a `WorldMap` adds to it: the statics, and the
    /// heights a body stands at. The byte format is the importer's, and is
    /// tested where it lives.
    #[test]
    fn off_the_map_is_none_not_a_panic() {
        let map = WorldMap::from_blocks(BlockExtent { wide: 2, down: 2 }, |x, y| LandCell {
            tile: LandTileId(x + y),
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
        let map = WorldMap::from_blocks(BlockExtent { wide: 3, down: 2 }, |x, y| LandCell {
            tile: LandTileId(x),
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
        let map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |x, y| LandCell {
            tile: LandTileId(3),
            z: (x + y) as i8,
        });
        assert_eq!(map.land_corners(2, 3), Some([5, 6, 6, 7]));
        // The far corner of the facet has no eastern or southern neighbour, so
        // all four corners are its own height.
        assert_eq!(map.land_corners(7, 7), Some([14; 4]));
        assert_eq!(map.land_corners(8, 0), None, "off the map is not a tile");
    }

    /// The one-block-derivation path answers exactly what four separate reads
    /// do, over every tile of a facet several blocks across.
    ///
    /// **The oracle is the walk it replaces**, written out here rather than
    /// called, because the point is that the fast path and the slow one are two
    /// pieces of arithmetic that must agree — and the fast one is only taken on
    /// 76.6% of tiles, so a run that never leaves a block's interior would prove
    /// nothing about the seam.
    ///
    /// A ramp whose height is a function of *both* coordinates is what makes it
    /// a test: on flat ground, or on ground sloping one way, three of the four
    /// corners coincide and a transposed pair would pass.
    #[test]
    fn a_tiles_corner_quad_is_the_four_reads_it_replaces() {
        let map = WorldMap::from_blocks(BlockExtent { wide: 3, down: 2 }, |x, y| LandCell {
            tile: LandTileId(3),
            z: ((x * 5) as i8).wrapping_sub((y * 3) as i8),
        });
        let slowly = |x: u16, y: u16| {
            let own = map.land(x, y)?.z;
            let at = |x: Option<u16>, y: Option<u16>| match (x, y) {
                (Some(x), Some(y)) => map.land(x, y).map_or(own, |cell| cell.z),
                _ => own,
            };
            let (east, south) = (x.checked_add(1), y.checked_add(1));
            Some([own, at(east, Some(y)), at(Some(x), south), at(east, south)])
        };
        let mut fast = 0;
        for y in 0..map.height() as u16 {
            for x in 0..map.width() as u16 {
                assert_eq!(map.land_corners(x, y), slowly(x, y), "({x}, {y})");
                // And the pair reads back the same cell the tile has, which is
                // the half `Spans::ground` stopped reading twice for.
                assert_eq!(
                    map.land_and_corners(x, y).map(|(cell, _)| cell.z),
                    map.land(x, y).map(|cell| cell.z),
                    "({x}, {y})",
                );
                fast += usize::from(map.land.corner_quad(x, y).is_some());
            }
        }
        // Seven eighths each way — the tiles on a block's eastern or southern
        // edge are the ones that fall back, and there must be some of both.
        assert_eq!(
            fast,
            7 * 7 * 3 * 2,
            "the interior of every block and nothing else"
        );
        assert!(map.land.corner_quad(7, 0).is_none(), "a block's eastern edge");
        assert!(map.land.corner_quad(0, 7).is_none(), "a block's southern edge");
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
        let map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |x, y| LandCell {
            tile: LandTileId(3),
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
    /// what is left here is the statics half a `WorldMap` adds.
    #[test]
    fn a_map_built_in_memory_is_bare_ground_of_the_size_asked_for() {
        let map = WorldMap::from_blocks(BlockExtent { wide: 3, down: 2 }, |_, _| LandCell::default());
        assert_eq!((map.width(), map.height()), (24, 16));
        assert_eq!(map.facet_name(), "unknown facet");
        assert_eq!(map.static_count(), 0);
        assert_eq!(map.statics_at(0, 0).count(), 0);
    }

    /// A block is kept sorted by tile, and two statics on one tile keep the
    /// order they arrived in.
    ///
    /// [`WorldMap::statics_at`] is a binary search over that order, so this is the
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
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell::default());
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
    /// [`WorldMap::statics_in_row`] is a faster spelling of the tile walk and nothing
    /// else, so the tile walk is its oracle — and the assertion is on the whole
    /// sequence rather than on a set, because the order is what a tie between two
    /// statics at one depth is broken by. It crosses three block columns and runs
    /// off both ends of the map, which is where a partial block and a clamp are.
    #[test]
    fn a_row_is_the_tile_walk_written_faster() {
        // Three blocks across, two down, and statics scattered over it in an
        // order that is neither the sort's nor the walk's.
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 3, down: 2 }, |_, _| LandCell::default());
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
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 3, down: 2 }, |_, _| LandCell::default());
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

    /// An edit in one block moves every block after it, and none of them notices.
    ///
    /// The failure mode the one-run layout adds and the per-block vectors could
    /// not have: a block's items are found through an offset now, so putting a
    /// static into block 0 shifts block 1's items along the run and *every*
    /// offset past it has to move with them. Miss one and the neighbour's
    /// lookups read a slice one item out of place — which is not a panic, it is
    /// a wall that is silently the item next to it.
    ///
    /// So the assertion is over the whole facet after each edit, and the edits
    /// are in the earliest block on purpose: an edit in the last one would pass
    /// with the offsets left alone entirely.
    #[test]
    fn an_edit_in_one_block_leaves_every_other_block_where_it_was() {
        let mut map = WorldMap::from_blocks(BlockExtent { wide: 3, down: 2 }, |_, _| LandCell::default());
        // One item per block, named by its block, and one extra pair in block 0
        // for the removal below to have an ordinal to name.
        let mut expected: Vec<(Graphic, u16, u16)> = Vec::new();
        let mut tile = 0;
        for block_x in 0..3u16 {
            for block_y in 0..2u16 {
                tile += 1;
                let (x, y) = (block_x * 8 + 2, block_y * 8 + 3);
                map.place_static(StaticItem {
                    tile: Graphic(tile),
                    x,
                    y,
                    z: 0,
                    hue: Hue(0),
                });
                expected.push((Graphic(tile), x, y));
            }
        }
        let whole_facet = |map: &WorldMap| {
            (0..16u16)
                .flat_map(|y| {
                    map.statics_in_row(y, 0, 23)
                        .map(|item| (item.tile, item.x, item.y))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        };
        let sorted = |mut items: Vec<(Graphic, u16, u16)>| {
            items.sort_by_key(|(_, x, y)| (*y, *x));
            items
        };
        assert_eq!(whole_facet(&map), sorted(expected.clone()));

        // Into the first block, which is the one every other block's offset is
        // downstream of.
        map.place_static(StaticItem {
            tile: Graphic(100),
            x: 1,
            y: 3,
            z: 0,
            hue: Hue(0),
        });
        expected.push((Graphic(100), 1, 3));
        assert_eq!(whole_facet(&map), sorted(expected.clone()));
        assert_eq!(map.static_count(), 7);

        // And back out again: the first of the two standing in block 0's row 3.
        let gone = map.remove_static(1, 3, 0).expect("the static just placed");
        assert_eq!(gone.tile, Graphic(100));
        expected.retain(|(tile, ..)| *tile != Graphic(100));
        assert_eq!(whole_facet(&map), sorted(expected));
        assert_eq!(map.static_count(), 6);
        assert_eq!(map.remove_static(1, 3, 0), None, "nothing stands there now");
    }

    /// What the base layer costs is its count times this, and nothing else.
    ///
    /// Nine bytes of fields in ten of storage — the padding is the alignment of
    /// the three `u16`s. Felucca's 2,906,871 statics are 29,068,710 bytes of run
    /// and 3,670,016 of block table (458,752 blocks × 8, where the prefix sum it
    /// replaced was 1,835,012 — the 1.75 MiB S3 named as the price of a block
    /// being replaceable where it stands). A field added here is 2.9 MiB of
    /// resident memory per byte it adds, which is the measurement the one-run
    /// layout was for.
    #[test]
    fn a_static_is_ten_bytes_in_the_run() {
        assert_eq!(size_of::<StaticItem>(), 10);
    }

    /// The importer's door sorts each block's own part of the run, and only it.
    ///
    /// [`WorldMap::from_parts`] takes one run and a count per block, so the sort
    /// it owes is per block rather than over the whole facet — a global sort by
    /// `(y, x)` would interleave two blocks that share a row and leave every
    /// block's slice holding somebody else's items. Two blocks of one row are
    /// where that shows.
    #[test]
    fn from_parts_sorts_each_blocks_own_part_of_the_run() {
        let land = LandGrid::from_blocks(BlockExtent { wide: 2, down: 1 }, |_, _| LandCell::default());
        let item = |tile, x, y| StaticItem {
            tile: Graphic(tile),
            x,
            y,
            z: 0,
            hue: Hue(0),
        };
        // In file order, which is not tile order: block 0's three items and then
        // block 1's two, the two on (3, 5) in the order the file has them.
        let statics = vec![
            item(10, 3, 5),
            item(20, 1, 2),
            item(30, 3, 5),
            item(40, 9, 7),
            item(50, 12, 0),
        ];
        let map = WorldMap::from_parts(land, statics, &[3, 2]);

        let named = |items: &[StaticItem]| items.iter().map(|item| item.tile).collect::<Vec<_>>();
        assert_eq!(
            named(map.statics_in_block(0, 0)),
            vec![Graphic(20), Graphic(10), Graphic(30)],
            "block 0 is sorted by tile, stably",
        );
        assert_eq!(
            named(map.statics_in_block(1, 0)),
            vec![Graphic(50), Graphic(40)],
            "block 1 is sorted within itself",
        );
        assert_eq!(
            map.statics_at(3, 5).map(|item| item.tile).collect::<Vec<_>>(),
            vec![Graphic(10), Graphic(30)],
        );
        assert_eq!(map.static_count(), 5);
    }

    // ---- S3: a block is replaced where it stands ---------------------------

    /// A facet of `wide`×`down` blocks with `each` items in every block, named
    /// by where they stand so a run read out of the wrong block says which.
    fn facet_with_statics(wide: u32, down: u32, each: u16) -> WorldMap {
        let land = LandGrid::from_blocks(BlockExtent { wide, down }, |_, _| LandCell::default());
        let mut statics = Vec::new();
        let extent = land.extent();
        for block in extent.blocks() {
            let coord = extent.coord_of(block).expect("the extent named this block");
            let (origin_x, origin_y) = coord.origin();
            for n in 0..each {
                statics.push(StaticItem {
                    tile: Graphic(block.get() as u16 * 100 + n),
                    x: origin_x as u16 + n % 8,
                    y: origin_y as u16,
                    z: 0,
                    hue: Hue(0),
                });
            }
        }
        let counts = vec![u32::from(each); extent.count() as usize];
        WorldMap::from_parts(land, statics, &counts)
    }

    /// Every static of the facet, block by block, as a value two maps can be
    /// compared by.
    fn every_block(map: &WorldMap) -> Vec<Vec<StaticItem>> {
        map.land
            .extent()
            .blocks()
            .map(|block| {
                let coord = map.land.coord_of(block).expect("a block of this facet");
                map.statics_in_block(coord.x, coord.y).to_vec()
            })
            .collect()
    }

    /// The point of the table: a block that grew is written at the end of the
    /// run, and no other block's entry moves — where a prefix sum would have had
    /// to repair every one of them.
    #[test]
    fn a_block_that_grew_is_the_only_entry_that_moved() {
        let mut map = facet_with_statics(3, 2, 2);
        let before = map.blocks.clone();
        let contents = every_block(&map);

        // Into the *first* block, which every later entry of a prefix sum would
        // be downstream of.
        map.place_static(StaticItem {
            tile: Graphic(999),
            x: 1,
            y: 1,
            z: 0,
            hue: Hue(0),
        });

        assert_eq!(
            map.blocks[1..],
            before[1..],
            "no block but the one that grew was repointed"
        );
        assert_eq!(
            map.blocks[0].base as usize, 12,
            "and it went to the end of the run"
        );
        assert_eq!(map.blocks[0].count, 3);
        assert_eq!(map.dead, 2, "what it left behind is the run it had");
        assert_eq!(map.static_count(), 13, "which is not counted as a static");

        // And every block still reads its own items.
        for (block, was) in every_block(&map).iter().zip(&contents).skip(1) {
            assert_eq!(block, was);
        }
        assert_eq!(
            map.statics_at(1, 1).map(|item| item.tile).collect::<Vec<_>>(),
            vec![Graphic(999)],
        );
    }

    /// A run that kept its length is written where it stands: an edit to the
    /// *ground* moves no statics, and relocating them would manufacture garbage
    /// out of a publish that changed none.
    #[test]
    fn a_block_that_kept_its_count_stays_where_it_is() {
        let mut map = facet_with_statics(3, 2, 2);
        let before = map.blocks.clone();
        let at = map.land.index_of(BlockCoord { x: 1, y: 0 }).expect("a block");
        let land = vec![
            LandCell {
                tile: LandTileId(4),
                z: 12,
            };
            CELLS_PER_BLOCK
        ];
        let statics = map.statics_in_block(1, 0).to_vec();

        map.replace_blocks(&[BlockPatch::new(at, &land, &statics)]);

        assert_eq!(map.blocks, before, "nothing was repointed");
        assert_eq!(map.dead, 0, "and nothing was orphaned");
        assert_eq!(map.land(8, 0).expect("a cell of this facet").z, 12);
    }

    /// The garbage rule: orphaned runs are left where they are until they
    /// outweigh what is reachable, and then the run is laid out in block order
    /// again — with every block still reading its own items.
    #[test]
    fn garbage_past_the_live_items_repacks_the_run() {
        let mut map = facet_with_statics(2, 1, 4);
        let mut placed = 0;
        // Each addition orphans the block's whole run, so the garbage catches up
        // fast on a facet this small — which is the point of the fixture, not of
        // the rule: on Felucca it is thousands of publishes away.
        while map.dead > 0 || placed == 0 {
            placed += 1;
            map.place_static(StaticItem {
                tile: Graphic(500 + placed),
                x: 1,
                y: 1,
                z: placed as i8,
                hue: Hue(0),
            });
            if map.dead == 0 {
                break;
            }
        }
        assert!(placed > 1, "the fixture never reached a repack");
        assert_eq!(map.dead, 0, "a repacked run holds no garbage");
        assert_eq!(map.statics.len(), map.static_count(), "and nothing unreachable");
        assert_eq!(map.static_count(), 8 + placed as usize);
        assert_eq!(
            map.statics_at(1, 1).count(),
            placed as usize,
            "every item placed is still where it was put"
        );
        assert_eq!(
            map.statics_in_block(1, 0).len(),
            4,
            "and the other block is untouched"
        );
    }

    /// **The oracle for the whole layout:** a facet edited into shape holds what
    /// the same facet built by an importer holds, block for block.
    ///
    /// A repointed table and a prefix sum are the same thing to a reader, so the
    /// failure this catches is the one that is not visible in any single edit —
    /// a block reading a run that belongs to whoever was relocated after it.
    #[test]
    fn an_edited_facet_holds_what_an_imported_one_does() {
        let mut edited = facet_with_statics(3, 3, 2);
        let extent = edited.land.extent();

        // Grow one block, shrink another, replace a third wholesale, and do it
        // in an order that leaves the run out of block order.
        edited.place_static(StaticItem {
            tile: Graphic(901),
            x: 2,
            y: 2,
            z: 0,
            hue: Hue(0),
        });
        edited.remove_static(8, 8, 0).expect("block (1, 1) holds two");
        let at = extent.index_of(BlockCoord { x: 2, y: 2 }).expect("a block");
        let land = vec![LandCell::default(); CELLS_PER_BLOCK];
        let arrived = [
            StaticItem {
                tile: Graphic(902),
                x: 19,
                y: 17,
                z: 0,
                hue: Hue(0),
            },
            StaticItem {
                tile: Graphic(903),
                x: 17,
                y: 17,
                z: 0,
                hue: Hue(0),
            },
        ];
        edited.replace_blocks(&[BlockPatch::new(at, &land, &arrived)]);

        // The same world, assembled by the importer's door out of what the
        // edited one now holds.
        let contents = every_block(&edited);
        let counts: Vec<u32> = contents
            .iter()
            .map(|block| u32::try_from(block.len()).expect("a block of fewer than 4G statics"))
            .collect();
        let imported = WorldMap::from_parts(
            LandGrid::from_blocks(BlockExtent { wide: 3, down: 3 }, |_, _| LandCell::default()),
            contents.concat(),
            &counts,
        );

        assert_eq!(every_block(&edited), every_block(&imported));
        assert_eq!(edited.static_count(), imported.static_count());
        for y in 0..24u16 {
            for x in 0..24u16 {
                assert_eq!(
                    edited.statics_at(x, y).collect::<Vec<_>>(),
                    imported.statics_at(x, y).collect::<Vec<_>>(),
                    "({x}, {y})"
                );
            }
        }
    }
}
