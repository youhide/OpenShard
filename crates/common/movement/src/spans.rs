//! Where a body may stand, baked once instead of re-derived per step.
//!
//! `docs/map/navigation_spans.md`'s N1. A search spends 85% of its time asking
//! the map what a column holds — walking that column's statics, reading
//! `tiledata` for each, and re-deriving the same platform arithmetic sixteen
//! times per node expansion. This is that answer, computed once at load: a
//! column's standable surfaces, each with the headroom above it.
//!
//! # Three tiers, because the facet has three populations
//!
//! Counted by [`span_census`](../examples/span_census.rs) on facet 0, and every
//! decision here is that census rather than a guess:
//!
//! | | population | what answers it |
//! |---|---:|---|
//! | **the block** | 73.7% of blocks hold no statics | the map's own empty block |
//! | **the column** | 92.1% of columns hold no statics | the land grid, read live |
//! | **the exception table** | 7.9% of columns | a stored span list, CSR |
//!
//! The middle tier is the point. A column with no statics has exactly one
//! standable surface — its `average_land_z` — and nothing above it to duck
//! under, so storing a span for it would be storing what the land grid already
//! answers, for 96.5% of the facet's surfaces. What is expensive today is not
//! *finding* that surface but **proving there is nothing else to consider**,
//! which costs a `statics_at` on every one of a node's sixteen calls. Here that
//! proof is one array read.
//!
//! # Two types, and why they are two
//!
//! [`SpanIndex`] is the bake: owned, no lifetimes, built at facet load and kept
//! beside the map the way `NavigationGraph` is. [`Spans`] is the view a
//! question is asked through — the index, the map it was baked from, and the
//! ability of whoever is asking — built where it is asked, exactly as
//! [`MapTerrain`](crate::MapTerrain) is and for the same reason: the bake
//! deliberately does *not* store the 92% of columns the land grid can answer,
//! so answering needs both halves in hand.

use std::fmt;

use openshard_map::grid::BlockCoord;
use openshard_map::map::{BLOCK_SIZE, CELLS_PER_BLOCK, StaticItem, WorldMap};
use openshard_map::overlay::Cover;
use openshard_tiles::{LAND_TILE_COUNT, TileData};

use crate::terrain::{MAX_STEP_UP, PLAYER_HEIGHT, static_top};

/// One place a body can put its feet, and what is above it.
///
/// Four bytes, which is what makes the whole layer affordable: the exception
/// table is 2.4 million of these.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    /// Where a body's feet rest on this surface — ServUO's `ourZ`.
    pub stand_z: i8,
    /// The edge a step must reach to climb onto it — ServUO's `itemTop`.
    ///
    /// [`stand_z`](Self::stand_z) for everything but a climbable platform,
    /// whose surface is halved and whose base is what you meet. Separate
    /// because [`Cover`] already returns both and they differ on a stair;
    /// folding them would be inventing a rule the step check does not have.
    /// For the land it is the tile's **lowest** corner, which is what
    /// `MapTerrain::check` reaches for, while the body stands at the average.
    pub reach_z: i8,
    /// Free height above [`stand_z`](Self::stand_z) before the map's own
    /// statics are in the way, saturating at 255.
    ///
    /// The other half of what a step asks: `check` calls `is_obstructed` with a
    /// body reaching from the height it *came from*, so the height needed
    /// varies per step and the answer cannot be a bit. It can be a byte: a body
    /// wanting `h` above `stand_z` fits exactly when `h <= clearance`.
    ///
    /// **255 is not by itself "nothing above".** N1 argued that it was, on the
    /// grounds that a base and a `stand_z` are both `i8` and so a gap can never
    /// exceed 255 — which is true, and leaves the *boundary* ambiguous: a
    /// static based at 127 over a surface at −128 is a real gap of exactly 255.
    /// [`SpanFlags::CEILED`] is what separates the two, and it matters for a
    /// body needing more than 255 above its feet, which is a body that walked in
    /// more than 239 above where it is landing.
    pub clearance: u8,
    /// What kind of surface this is. Today: whether only a swimmer stands here.
    pub flags: SpanFlags,
}

/// What a [`Span`] is, past its heights.
///
/// A byte with four bits spare, in the shape
/// [`TileFlags`](openshard_tiles::TileFlags) has: a mask constant per bit and
/// one `has`, so a reader that only wants one bit does not learn the layout.
///
/// Three of the four in use are the step rule's, and they are here rather than
/// in the query for one reason each: they are properties of the *column*, and a
/// query that derived them would be reading the statics this layer exists to
/// stop it reading. See [`Spans::check`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SpanFlags(u8);

impl SpanFlags {
    /// Nothing special: a walker stands here.
    pub const NONE: u8 = 0x00;
    /// Water. A surface only something that swims can use.
    ///
    /// A flag and not a second grid, because a swimmer's surfaces are fifteen
    /// million more than a walker's and every one of them is an ocean column
    /// whose height is the land's — so they cost nothing under the tiers above
    /// and need no storage of their own. The *asker* filters; the structure
    /// offers. See [`Spans::swimming`].
    pub const SWIMMER_ONLY: u8 = 0x01;
    /// The column's own land, rather than something standing on it.
    ///
    /// One span of a column carries it, or none where the land is a mountainside
    /// nobody stands on. What reads it is [`LAND_WINS`](Self::LAND_WINS)'s
    /// residue: the guard needs the tile's *lowest corner*, and the land span is
    /// where the column already keeps it.
    pub const GROUND: u8 = 0x02;
    /// ServUO's `landCheck` guard: the ground pokes through this static, so the
    /// land under it wins and this is not something to climb onto.
    ///
    /// Three of that guard's four conditions are facts about the column — the
    /// static's base against the land's centre, and the land's centre against
    /// where a body would stand on the static — and both of those are settled
    /// here. The fourth is start-dependent and stays in [`Spans::check`]; the
    /// first, whether the land is ground *for the body asking*, is the ability
    /// filter that the [`GROUND`](Self::GROUND) span is subject to anyway.
    ///
    /// Never set on a land span: the guard is about a static the land beats.
    pub const LAND_WINS: u8 = 0x04;
    /// The map put something above this surface, so
    /// [`clearance`](Span::clearance) is a measurement rather than an absence.
    ///
    /// The one bit that keeps the obstruction test exact at its boundary: a gap
    /// of exactly 255 and no gap at all are the same byte, and they answer
    /// differently for a body that needs more than 255 above its feet.
    pub const CEILED: u8 = 0x08;

    /// The flags with `bits` set.
    #[must_use]
    pub const fn new(bits: u8) -> Self {
        Self(bits)
    }

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// The same flags, with every bit of `mask` also set.
    #[must_use]
    pub const fn with(self, mask: u8) -> Self {
        Self(self.0 | mask)
    }

    /// Whether any bit of `mask` is set.
    #[must_use]
    pub const fn has(self, mask: u8) -> bool {
        self.0 & mask != 0
    }

    /// Whether only something that swims stands here.
    #[must_use]
    pub const fn is_swimmer_only(self) -> bool {
        self.has(Self::SWIMMER_ONLY)
    }

    /// Whether this is the column's own land.
    #[must_use]
    pub const fn is_ground(self) -> bool {
        self.has(Self::GROUND)
    }

    /// Whether the land under this static beats it as somewhere to stand.
    #[must_use]
    pub const fn land_wins(self) -> bool {
        self.has(Self::LAND_WINS)
    }

    /// Whether anything of the map's own is above this surface.
    #[must_use]
    pub const fn is_ceiled(self) -> bool {
        self.has(Self::CEILED)
    }
}

/// What one land tile is to a body walking over it.
///
/// Baked per land graphic rather than read from `tiledata` per column: there
/// are 16,384 land tiles and 29 million columns, so this is sixteen kilobytes
/// that take the tile table off the query path entirely. It is also what lets
/// [`SpanIndex`] answer a bare column without holding a `TileData` — the bake
/// is a snapshot of one map *and* one tile table, and this is the half of the
/// table it needs to keep.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LandKind {
    /// Ground a walker stands on.
    Ground,
    /// Water: a surface for a swimmer and nothing for a walker.
    Water,
    /// Impassable — a mountainside, a lava field. Not a surface for anyone.
    Blocked,
}

/// One block's column index into the span run.
///
/// **Sixteen bytes, and a count only where there is something to count.** The
/// plan's first choice was a `[u8; 64]` per block — one cache line, addressed by
/// a byte-wise prefix sum — and N1 measured what that costs: 8.2 MB of tables
/// against 6.5 MB of spans, because a block with any static at all carries
/// sixty-four bytes whether or not sixty-four of its columns hold anything, and
/// on facet 0 most of them do not. The mask is that finding taken: one bit per
/// cell says whether the column owns a run, and the counts live packed in
/// [`SpanIndex::counts`], one byte per *set* bit.
///
/// So the prefix sum is over the occupied columns of the block rather than over
/// all sixty-four of them, and it is reached through a `count_ones` on a word
/// the lookup has already loaded. The census caps a column at twelve spans, so
/// a count is still a byte — but the builder asserts that rather than
/// truncating, because a base set is a world nobody has counted.
///
/// **Measured on facet 0:** the addressing is 3.3 MB where it was 8.2 MB, the
/// whole bake 11.2 MiB where it was 15.8, and a landing off the bake 158 ns
/// where it was 180 — smaller *and* fewer bytes read, which is what the finding
/// predicted. The spans it addresses did not move: 1,635,392 of them, and the
/// whole-facet oracle in `span_index` agrees with the walk on all 29,360,128
/// columns for both abilities.
struct BlockTable {
    /// Where this block's columns begin in [`SpanIndex::spans`].
    base: u32,
    /// Which of the block's sixty-four columns own a run, bit per cell in the
    /// block's own row-major cell order.
    ///
    /// A column with statics over it can still own nothing — a wall standing on
    /// a mountainside is a block with a table and a cell with no bit — so this
    /// is not "has statics"; it is "has spans stored", which is the question
    /// [`SpanIndex::stored`] is asking.
    occupied: u64,
    /// Where this block's counts begin in [`SpanIndex::counts`]: one byte per
    /// set bit of [`occupied`](Self::occupied), in ascending cell.
    counts: u32,
}

/// The occupancy mask is one bit per cell, so a block's cells must fit a word.
///
/// Pinned at compile time rather than assumed: [`BLOCK_SIZE`] is the map's to
/// choose, and a map that widened its block would otherwise index spans by a
/// mask that had quietly stopped covering it.
const _: () = assert!(
    CELLS_PER_BLOCK == u64::BITS as usize,
    "a block's cells must fit the occupancy mask"
);

/// A block with no statics at all: every column of it is bare ground.
///
/// A sentinel rather than an `Option<u32>`, which would double the index to
/// 3.6 MB for a bit that a `u32` has 458,751 spare values for.
const BARE: u32 = u32::MAX;

/// Every standable surface of one facet that the land grid cannot answer for
/// itself.
///
/// Built at load and never mutated: it is a projection of the two *lower*
/// layers — the ground and the statics — and deliberately not of the live one.
/// A door, a crate and a house floor are invisible here by construction rather
/// than by each builder remembering, which is what makes the overlay's veto the
/// only thing that has to be applied per tick.
pub struct SpanIndex {
    /// Per map block, in the land grid's own
    /// [`BlockIndex`](openshard_map::grid::BlockIndex) order: which entry
    /// of [`tables`](Self::tables) holds it, or [`BARE`].
    ///
    /// Indexed by the map's own block index and not by a second one of this
    /// module's — a parallel block addressing is a second thing to keep in step
    /// with the map, and `docs/map/` is a catalogue of what that costs.
    blocks: Vec<u32>,
    /// One per block that holds any static — 120,744 of 458,752 on facet 0.
    tables: Vec<BlockTable>,
    /// How many spans each *occupied* column owns: one byte per set bit of the
    /// tables' masks, tables in [`tables`](Self::tables) order and columns in
    /// each block's row-major cell order.
    ///
    /// Packed rather than sixty-four bytes a block because the occupancy is
    /// what the map turned out to be: 1,388,743 columns of facet 0 own a run,
    /// against the 7,727,616 cells the 120,744 dense tables addressed — so 82%
    /// of every table was a zero, and the tables were half the bake.
    counts: Vec<u8>,
    /// Every stored span, blocks in [`blocks`](Self::blocks) order and columns
    /// in each block's row-major cell order.
    spans: Vec<Span>,
    /// What each land graphic is, indexed by
    /// [`LandTileId`](openshard_tiles::LandTileId).
    land: Vec<LandKind>,
}

impl fmt::Debug for SpanIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpanIndex")
            .field("blocks", &self.blocks.len())
            .field("tables", &self.tables.len())
            .field("columns", &self.counts.len())
            .field("spans", &self.spans.len())
            .field("bytes", &self.resident_bytes())
            .finish()
    }
}

impl SpanIndex {
    /// Bake a facet.
    ///
    /// One pass over the statics — which is why there is no artifact and no
    /// cache: the census walked twice as much in 3.5 s, and the thing that is
    /// actually expensive to bake is the region graph over this.
    ///
    /// # Panics
    ///
    /// If a column holds more than 255 standable surfaces, or one of them sits
    /// outside `i8`. Both are impossible on Britannia — the deepest column on
    /// facet 0 holds twelve — and both are a world this layer cannot describe,
    /// so it says so rather than storing a truncation that would read as a
    /// missing floor a thousand columns later.
    #[must_use]
    pub fn build(map: &WorldMap, tiles: &TileData) -> Self {
        let extent = map.extent();
        let mut index = Self {
            blocks: vec![BARE; extent.count() as usize],
            tables: Vec::new(),
            counts: Vec::new(),
            spans: Vec::new(),
            land: land_kinds(tiles),
        };
        let mut column = Vec::new();
        for block in extent.blocks() {
            let coord = extent.coord_of(block).expect("the extent named this block");
            let items = map.statics_in_block(coord.x, coord.y);
            if items.is_empty() {
                // The block tier: nothing here to duck under and nothing to
                // stand on, so every column of it is the land grid's answer.
                continue;
            }
            let mut table = BlockTable {
                base: u32::try_from(index.spans.len()).expect("a facet's spans fit a u32"),
                occupied: 0,
                counts: u32::try_from(index.counts.len()).expect("a facet's columns fit a u32"),
            };
            let (origin_x, origin_y) = coord.origin();
            // A block's items are sorted by `(y, x)`, which *is* its row-major
            // cell order — so grouping them by tile walks the block's columns in
            // ascending cell, and the counts can be laid down in one pass with
            // no second sort. It is what the packed counts are addressed by as
            // well as the spans: a count belongs to the `n`th set bit of the
            // mask, so laying them down in any other order would hand a column
            // its neighbour's length. The assertion below is what says so out
            // loud: get this wrong and every column after the first reads a run
            // belonging to somebody else, silently.
            let mut at = 0;
            let mut last_cell = None;
            while at < items.len() {
                let (x, y) = (items[at].x, items[at].y);
                let run = items[at..].partition_point(|item| (item.y, item.x) == (y, x));
                let cell = usize::try_from(u32::from(y) - origin_y).expect("a tile inside its own block")
                    * BLOCK_SIZE as usize
                    + usize::try_from(u32::from(x) - origin_x).expect("a tile inside its own block");
                assert!(
                    last_cell.is_none_or(|last| cell > last),
                    "block {coord:?} hands its statics out of cell order at ({x}, {y}): \
                     the CSR layout addresses a column by the counts of the columns before it"
                );
                last_cell = Some(cell);
                surfaces_of(map, tiles, &index.land, x, y, &items[at..at + run], &mut column);
                let held = u8::try_from(column.len()).unwrap_or_else(|_| {
                    panic!(
                        "({x}, {y}) holds {} standable surfaces; a count is a byte",
                        column.len()
                    )
                });
                // A column whose statics leave nothing to stand on gets no bit
                // and no count. Not for correctness — a stored zero would sum
                // to the same offsets and hand back the same empty slice — but
                // because it is the population the packed counts are sized by,
                // and a byte spent saying "nothing" is what the dense table was
                // spending 82% of itself on.
                if held > 0 {
                    table.occupied |= 1 << cell;
                    index.counts.push(held);
                }
                index.spans.append(&mut column);
                at += run;
            }
            index.blocks[block.get() as usize] =
                u32::try_from(index.tables.len()).expect("a facet's blocks fit a u32");
            index.tables.push(table);
        }
        index.spans.shrink_to_fit();
        index.tables.shrink_to_fit();
        index.counts.shrink_to_fit();
        index
    }

    /// How much memory the bake holds, in bytes — the number
    /// `docs/map/navigation_spans.md` estimated before it was built.
    #[must_use]
    pub fn resident_bytes(&self) -> usize {
        self.blocks.capacity() * size_of::<u32>()
            + self.tables.capacity() * size_of::<BlockTable>()
            + self.counts.capacity()
            + self.spans.capacity() * size_of::<Span>()
            + self.land.capacity() * size_of::<LandKind>()
    }

    /// How many spans the exception table holds.
    #[must_use]
    pub fn span_count(&self) -> usize {
        self.spans.len()
    }

    /// How many blocks hold any static at all.
    #[must_use]
    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// How many columns own a stored run — the population the packed counts are
    /// sized by, and the one the dense tables paid sixty-four bytes a block to
    /// address.
    #[must_use]
    pub fn column_count(&self) -> usize {
        self.counts.len()
    }

    /// Whether this one column owns a stored run, rather than being answered by
    /// the land grid.
    ///
    /// [`column_count`](Self::column_count) for a single column, and it exists
    /// for the same reason: the two tiers are answered by different code at
    /// different costs, so *which tier a column is* is a thing a measurement has
    /// to be able to ask. It is the population any per-span structure could ever
    /// address — a bare column has no span to hang anything on — and
    /// `step_cost` splits its sample by it.
    ///
    /// Not the same question as "does this column hold statics": a column whose
    /// statics leave nothing to stand on owns no run.
    #[must_use]
    pub fn stores(&self, map: &WorldMap, x: u16, y: u16) -> bool {
        !self.stored(map, x, y).is_empty()
    }

    /// One column's stored spans, empty for a column with no statics — which
    /// is not the same as a column with nothing to stand on. See
    /// [`Spans::surfaces`], which is where the land answers for the empty case.
    fn stored(&self, map: &WorldMap, x: u16, y: u16) -> &[Span] {
        let Some(block) = map.extent().index_of(BlockCoord::containing(x, y)) else {
            return &[];
        };
        let table = self.blocks[block.get() as usize];
        if table == BARE {
            return &[];
        }
        let table = &self.tables[table as usize];
        let cell = (usize::from(y) % BLOCK_SIZE as usize) * BLOCK_SIZE as usize
            + (usize::from(x) % BLOCK_SIZE as usize);
        let bit = 1_u64 << cell;
        if table.occupied & bit == 0 {
            // The column tier, reached without touching the counts at all: the
            // mask is in the same sixteen bytes as the base, so a column with
            // nothing stored costs one word and one test. That is 82% of the
            // columns of a block with statics in it.
            return &[];
        }
        // Which of the block's occupied columns this one is. The prefix sum is
        // still a prefix sum — a count is a length, not an offset — but it runs
        // over the occupied columns before this one rather than over all
        // sixty-four cells, and `count_ones` finds where they start.
        let at = table.counts as usize;
        let rank = (table.occupied & (bit - 1)).count_ones() as usize;
        let from = table.base as usize
            + self.counts[at..at + rank]
                .iter()
                .copied()
                .map(usize::from)
                .sum::<usize>();
        &self.spans[from..from + usize::from(self.counts[at + rank])]
    }
}

/// A body's view of one facet's spans: the bake, the map it was baked from, and
/// what the body can do.
///
/// Two borrows and a flag, built where it is asked —
/// [`MapTerrain`](crate::MapTerrain)'s own shape, because it is the same kind
/// of thing. `Copy`: a caller that wants to keep one beside its own data copies
/// it rather than borrowing the borrow.
#[derive(Clone, Copy, Debug)]
pub struct Spans<'a> {
    /// The ground. Read for the 92% of columns the bake deliberately does not
    /// store, and for the block addressing both halves share.
    map: &'a WorldMap,
    /// The bake.
    index: &'a SpanIndex,
    /// Whether water counts as somewhere to stand. A property of the *body*
    /// asking and never of the world, which is why one facet's bake serves a
    /// walker and a fish at once.
    swimming: bool,
}

impl<'a> Spans<'a> {
    /// Read a baked facet, as something that walks.
    #[must_use]
    pub const fn new(map: &'a WorldMap, index: &'a SpanIndex) -> Self {
        Self {
            map,
            index,
            swimming: false,
        }
    }

    /// Ask as something that swims: water becomes a surface.
    #[must_use]
    pub const fn swimming(mut self, swimming: bool) -> Self {
        self.swimming = swimming;
        self
    }

    /// The map the spans were baked from.
    #[must_use]
    pub const fn map(&self) -> &'a WorldMap {
        self.map
    }

    /// Every surface a body of this ability could stand on at `(x, y)`, highest
    /// first.
    ///
    /// Exactly what
    /// [`stand_surfaces`](crate::surfaces::stand_surfaces) returns for the same
    /// column and the same ability — that equivalence over the whole of facet 0
    /// is what N1 is done when, and `spans_are_the_surfaces_the_walk_derives`
    /// is where it is asserted.
    ///
    /// **Highest first**, where `stand_surfaces` is in the map file's own order.
    /// The step rule wants the highest surface within reach (Sphere's
    /// `GetFixPoint`), so the order it walks in is the order it is stored in and
    /// the first candidate that passes is the answer.
    #[must_use]
    pub fn surfaces(&self, x: u16, y: u16) -> Surfaces<'a> {
        let stored = self.index.stored(self.map, x, y);
        Surfaces {
            // A column with no statics is not stored at all: the land grid
            // answers it, and nothing can be in the way of a surface with
            // nothing above it. This is the column tier, and it is 92% of the
            // facet.
            bare: match stored.is_empty() {
                true => self.ground(x, y).filter(|span| self.wants(*span)),
                false => None,
            },
            listed: stored.iter(),
            swimming: self.swimming,
        }
    }

    /// The land's own surface at `(x, y)`, for a column the bake does not
    /// store.
    ///
    /// The same two reads `stand_surfaces` makes — the average of the four
    /// corners for the height, the land graphic for whether it is a surface at
    /// all — and the reason the middle tier exists: about twelve nanoseconds,
    /// against the `statics_at` it replaces.
    fn ground(&self, x: u16, y: u16) -> Option<Span> {
        let cell = self.map.land(x, y)?;
        let corners = self.map.land_corners(x, y).expect("land was just present");
        Some(Span {
            stand_z: openshard_map::map::average_corner_z(corners),
            // What a step has to reach is the tile's lowest corner, not the
            // height a body ends up at: `MapTerrain::check` compares `step_top`
            // against `land_z`, and a body stands at the average.
            reach_z: corners.into_iter().min().expect("four corners"),
            // Nothing is stored for this column because nothing stands on it.
            clearance: u8::MAX,
            flags: kind_flags(self.index.land[usize::from(cell.tile.0)])?,
        })
    }

    /// Whether a body of this ability stands on `span`.
    const fn wants(&self, span: Span) -> bool {
        self.swimming || !span.flags.is_swimmer_only()
    }

    /// The height a body whose feet are at `start_z`, standing on a surface
    /// topping out at `start_top`, lands at stepping onto `(x, y)` — or `None`
    /// where it may not step there at all.
    ///
    /// [`MapTerrain::check`](crate::MapTerrain::check) with the column already
    /// resolved: the same rule, choosing among surfaces this layer stored rather
    /// than deriving them from `tiledata` per step. `docs/map/navigation_spans.md`'s
    /// N2, and its whole content is that the answer did not change — the
    /// `span_check` example is where that is proved over the facet.
    ///
    /// **First accepted wins**, because the spans are stored highest first and
    /// the rule wants the highest surface in reach (Sphere's `GetFixPoint`).
    /// `check` expresses the same choice as a running maximum over the map file's
    /// own order, and a candidate that is refused never suppresses a lower one,
    /// so a descending walk that stops at the first acceptance is the same
    /// answer. On a tie between the land and a static at one height the two
    /// disagree about *which* surface won and agree about the number, which is
    /// the whole of what either returns.
    #[must_use]
    pub fn check(&self, x: u16, y: u16, start_z: i32, start_top: i32) -> Option<i32> {
        // How high a step reaches, and where the head of a body that walked in
        // from `start_z` is — the two scalars the whole rule reaches its source
        // through.
        let step_top = start_top + MAX_STEP_UP;
        let check_top = start_z + PLAYER_HEIGHT;
        let stored = self.index.stored(self.map, x, y);
        if stored.is_empty() {
            // The column tier. Nothing was stored because nothing stands here
            // and nothing is in the way, so the reach is the whole question —
            // and there is no `landCheck` guard without a static to guard
            // against.
            let ground = self.ground(x, y).filter(|span| self.wants(*span))?;
            return (step_top >= i32::from(ground.reach_z)).then_some(i32::from(ground.stand_z));
        }
        // The column's own land, for the one clause that reaches past a single
        // span. `None` here is exactly `land_is_ground` saying no: the land is a
        // mountainside, or it is water and this body walks.
        let ground = stored
            .iter()
            .copied()
            .find(|span| span.flags.is_ground() && self.wants(*span));
        stored
            .iter()
            .copied()
            .filter(|span| self.wants(*span))
            .find(|&span| self.admits(span, ground, step_top, check_top))
            .map(|span| i32::from(span.stand_z))
    }

    /// Whether a body reaching `step_top` with its head at `check_top` may take
    /// `span`, given the column's own `ground`.
    ///
    /// The three clauses of `MapTerrain::check`'s loop, in its order:
    ///
    /// - **Reach.** `step_top >= item_top`, and `item_top` is
    ///   [`reach_z`](Span::reach_z).
    /// - **The `landCheck` guard.** [`SpanFlags::LAND_WINS`] carries three of its
    ///   four conditions; the fourth is `test_top > land_z`, and the land's
    ///   lowest corner is the ground span's own reach.
    /// - **The body fits.** `is_obstructed(our_z, test_top)` read off
    ///   [`clearance`](Span::clearance) — obstructed exactly when the free height
    ///   is less than the height wanted, which is the same comparison
    ///   `is_obstructed` makes against every static at once.
    fn admits(&self, span: Span, ground: Option<Span>, step_top: i32, check_top: i32) -> bool {
        let stand = i32::from(span.stand_z);
        if step_top < i32::from(span.reach_z) {
            return false;
        }
        // `test_top`, not `stand + PLAYER_HEIGHT`: a body walks in at the height
        // it left, and dropping that half is the hole `is_obstructed` documents.
        let test_top = check_top.max(stand + PLAYER_HEIGHT);
        if span.flags.land_wins() && ground.is_some_and(|land| test_top > i32::from(land.reach_z)) {
            return false;
        }
        // Nothing above at all admits any body. Where there is something, the
        // byte *is* the gap and not a saturation of it — a base and a `stand_z`
        // are both `i8`, so no gap exceeds 255 — and the comparison is therefore
        // exact for every height a body could ask for. See `Span::clearance` for
        // why the flag is what carries the difference.
        !span.flags.is_ceiled() || test_top - stand <= i32::from(span.clearance)
    }
}

/// [`Spans::surfaces`]'s walk: at most one synthesised land surface, or a run
/// of stored ones, with the asker's own ability applied.
#[derive(Clone, Debug)]
pub struct Surfaces<'a> {
    /// The land surface of an unstored column, already filtered.
    bare: Option<Span>,
    /// A stored column's run, filtered as it is walked.
    listed: std::slice::Iter<'a, Span>,
    /// Whether water counts.
    swimming: bool,
}

impl Iterator for Surfaces<'_> {
    type Item = Span;

    fn next(&mut self) -> Option<Span> {
        if let Some(span) = self.bare.take() {
            return Some(span);
        }
        let swimming = self.swimming;
        self.listed
            .by_ref()
            .copied()
            .find(|span| swimming || !span.flags.is_swimmer_only())
    }
}

/// What each land graphic is to a body, read off the tile table once.
fn land_kinds(tiles: &TileData) -> Vec<LandKind> {
    (0..LAND_TILE_COUNT)
        .map(|id| {
            let flags = tiles.land(id as u16).flags;
            match (flags.is_water(), flags.is_blocking()) {
                (true, _) => LandKind::Water,
                (false, true) => LandKind::Blocked,
                (false, false) => LandKind::Ground,
            }
        })
        .collect()
}

/// The flags a land surface of this kind carries, or `None` where it is not a
/// surface at all.
///
/// [`SpanFlags::GROUND`] is here rather than at the two call sites because this
/// is the only thing that builds a land span, and a column whose ground forgot
/// to say so is a column the `landCheck` guard silently stops applying to.
const fn kind_flags(kind: LandKind) -> Option<SpanFlags> {
    match kind {
        LandKind::Ground => Some(SpanFlags(SpanFlags::GROUND)),
        LandKind::Water => Some(SpanFlags(SpanFlags::GROUND | SpanFlags::SWIMMER_ONLY)),
        LandKind::Blocked => None,
    }
}

/// One exception column's whole span list, into `out`.
///
/// `items` is every static on `(x, y)` — the caller has them as a run of the
/// block's slice, which is what keeps this off `statics_at` and its two binary
/// searches.
///
/// The land is included here where a bare column's is not, and that is the
/// difference between the two tiers rather than an inconsistency: a column with
/// statics has a *headroom* over its ground, and the byte that says so has to be
/// stored beside the height it is a headroom above.
fn surfaces_of(
    map: &WorldMap,
    tiles: &TileData,
    land: &[LandKind],
    x: u16,
    y: u16,
    items: &[StaticItem],
    out: &mut Vec<Span>,
) {
    debug_assert!(out.is_empty(), "the scratch column is drained by its caller");
    // A static outside the facet's own width or height — a block the extent has
    // and the map does not fill — is nowhere a body can be, and `statics_at`
    // does not hand it out either.
    if !map.contains(x, y) {
        return;
    }
    // The land, read once. Its *centre* is wanted even where it is no surface at
    // all — a mountainside still decides `landCheck` for the statics standing on
    // it — so this is the height and not the span.
    let land_center = map.land(x, y).map(|_| {
        let corners = map.land_corners(x, y).expect("land was just present");
        i32::from(openshard_map::map::average_corner_z(corners))
    });
    if let Some(flags) = map
        .land(x, y)
        .and_then(|cell| kind_flags(land[usize::from(cell.tile.0)]))
    {
        let corners = map.land_corners(x, y).expect("land was just present");
        out.push(span_over(
            tiles,
            items,
            openshard_map::map::average_corner_z(corners),
            corners.into_iter().min().expect("four corners"),
            flags,
        ));
    }
    for item in items {
        // `Cover::of_static` and not a second reading of the platform bit: the
        // halved climbable lives there, both ends of the wire lay a *placed*
        // static's surface with it, and `stand_surfaces` — the oracle this has
        // to match — asks the very same question.
        let tile = tiles.static_tile(item.tile.0);
        let Some(cover) = Cover::of_static(tile).based_at(item.z).stands() else {
            continue;
        };
        let stand_z = fits(cover.surface(), "a surface", x, y);
        out.push(span_over(
            tiles,
            items,
            stand_z,
            fits(cover.reach(), "a step's reach", x, y),
            SpanFlags(match land_wins(land_center, item.z, tile.height, stand_z) {
                true => SpanFlags::LAND_WINS,
                false => SpanFlags::NONE,
            }),
        ));
    }
    // Highest first — see `Spans::surfaces`. Stable, so two statics at one
    // height keep the file's order, which is the order that decides which of
    // them the client draws on top.
    out.sort_by_key(|span| std::cmp::Reverse(span.stand_z));
}

/// A height this layer can hold, or a panic naming where it could not.
///
/// The engine itself cannot carry a body above `i8` — `walk::can_step` refuses
/// a landing that does not fit one — so a span that does not fit is a surface
/// nothing could ever stand on, and storing a wrapped one would be a floor in
/// the wrong place rather than a missing one.
fn fits(z: i32, what: &str, x: u16, y: u16) -> i8 {
    i8::try_from(z).unwrap_or_else(|_| panic!("{what} at ({x}, {y}) is z={z}, which is not a height"))
}

/// One span standing at `stand_z` and met at `reach_z`, with the headroom over
/// it measured off the column it belongs to.
///
/// The one place a [`Span`] is built from a height, so the pairing of
/// [`clearance`](Span::clearance) with [`SpanFlags::CEILED`] cannot be got right
/// at one call site and wrong at the other.
fn span_over(tiles: &TileData, items: &[StaticItem], stand_z: i8, reach_z: i8, flags: SpanFlags) -> Span {
    let above = headroom(tiles, items, i32::from(stand_z));
    Span {
        stand_z,
        reach_z,
        clearance: above.unwrap_or(u8::MAX),
        flags: match above.is_some() {
            true => flags.with(SpanFlags::CEILED),
            false => flags,
        },
    }
}

/// How much room there is above `z` on a column holding `items`, or `None` where
/// the map put nothing above it at all.
///
/// `MapTerrain::is_obstructed` asked once per surface instead of once per step:
/// a static is in the way when its body overlaps the body of whoever is
/// standing here, so the free height is the distance up to the lowest thing
/// whose top is above `z`. Something already straddling `z` leaves zero, which
/// is a surface nothing fits on rather than one that does not exist — the
/// candidate is still a candidate, and the step rule is what refuses it.
///
/// A surface does **not** block the body standing on it: its top is exactly at
/// the feet, and `top > z` is false there. That is the same property
/// `is_obstructed` documents, arrived at the same way.
///
/// `None` and not 255: they are the same byte for a static based at 127 over a
/// surface at −128, and they are different answers for a body that needs more
/// than 255 above its feet.
fn headroom(tiles: &TileData, items: &[StaticItem], z: i32) -> Option<u8> {
    let mut free = None;
    for item in items {
        let tile = tiles.static_tile(item.tile.0);
        // What `is_obstructed` counts: a wall, and a surface too — a stair or an
        // upper floor is exactly as solid to a body beside it as a wall is.
        if !tile.flags.is_blocking() && !tile.flags.is_platform() {
            continue;
        }
        let base = i32::from(item.z);
        if static_top(tile, base) <= z {
            continue;
        }
        // Both ends are `i8`, so the gap is inside `0..=255` and the cast is
        // total — which is the whole reason a byte can carry this at all.
        let gap = (base - z).max(0) as u8;
        free = Some(free.map_or(gap, |free: u8| free.min(gap)));
    }
    free
}

/// Whether ServUO's `landCheck` guard's two column-shaped conditions hold for a
/// static based at `base` with art `height`, standing a body at `stand_z`.
///
/// The guard is the rule that a low static the ground pokes through is not
/// something you climb onto. Its four conditions split three ways, and this is
/// the middle pair — the ones that are neither a property of the *body* asking
/// nor of where it came from:
///
/// - `land_check < land_center`: the static's near edge is under the ground.
/// - `land_center > our_z`: standing on it would put you under the ground too.
///
/// The first condition, `land_is_ground`, is an ability question and is answered
/// by whether the column's [`SpanFlags::GROUND`] span survives the asker's
/// filter. The fourth, `test_top > land_z`, is start-dependent and lives in
/// [`Spans::check`]. `land_center` is `None` only off the map, where there is no
/// land to win.
fn land_wins(land_center: Option<i32>, base: i8, height: u8, stand_z: i8) -> bool {
    let Some(land_center) = land_center else {
        return false;
    };
    let land_check = i32::from(base) + MAX_STEP_UP.min(i32::from(height));
    land_check < land_center && land_center > i32::from(stand_z)
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::{BlockExtent, Tile};
    use openshard_tiles::TileFlags;

    use super::*;
    use crate::scene::Scene;
    use crate::surfaces::stand_surfaces;
    use crate::terrain::MapTerrain;

    /// A land graphic the scenes below declare, so it can be made water or
    /// mountainside. Anything else is [`Scene`]'s own tile 0: plain ground.
    const OTHER_GROUND: u16 = 7;

    /// The eight tiles one step away, in no particular order — a node
    /// expansion's whole neighbourhood.
    const NEIGHBOURS: [(i32, i32); 8] = [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ];

    /// A second one, so a scene can carry water and a mountainside at once.
    const BLOCKED_GROUND: u16 = 8;

    /// The bake of a scene, and a walker's view of it.
    ///
    /// A scene owns its map and its tile table, so this is the same pairing the
    /// shard makes at facet load, written small.
    fn bake(scene: &Scene) -> SpanIndex {
        SpanIndex::build(scene.terrain().map(), scene.terrain().tiles())
    }

    fn view<'a>(scene: &'a Scene, index: &'a SpanIndex) -> Spans<'a> {
        Spans::new(scene.terrain().map(), index)
    }

    /// Every surface the view offers, as plain heights.
    fn heights(spans: &Spans<'_>, x: u16, y: u16) -> Vec<i32> {
        spans.surfaces(x, y).map(|span| i32::from(span.stand_z)).collect()
    }

    /// What the walk derives, sorted the way the bake stores it so the two are
    /// comparable as lists.
    fn derived(scene: &Scene, x: u16, y: u16, swimming: bool) -> Vec<i32> {
        let terrain = scene.terrain();
        let mut surfaces = stand_surfaces(terrain.map(), terrain.tiles(), x, y, swimming);
        surfaces.sort_unstable_by(|left, right| right.cmp(left));
        surfaces
    }

    #[test]
    fn a_column_with_no_statics_is_the_land_and_is_not_stored() {
        let scene = Scene::flat(12);
        let index = bake(&scene);
        assert_eq!(index.span_count(), 0, "nothing on the facet needs a span");
        assert_eq!(index.table_count(), 0, "and no block needs a table");
        let spans = view(&scene, &index);
        assert_eq!(heights(&spans, 3, 3), vec![12]);
        let surface = spans.surfaces(3, 3).next().expect("flat ground is standable");
        assert_eq!(surface.clearance, u8::MAX, "nothing is above a bare column");
    }

    #[test]
    fn water_is_a_surface_only_to_a_swimmer() {
        let mut scene = Scene::flat(-5);
        scene
            .land_everywhere(OTHER_GROUND)
            .land_art(OTHER_GROUND, TileFlags::WATER | TileFlags::BLOCK);
        let index = bake(&scene);
        let walker = view(&scene, &index);
        assert!(
            heights(&walker, 3, 3).is_empty(),
            "a walker cannot stand on the sea"
        );
        assert_eq!(heights(&walker.swimming(true), 3, 3), vec![-5]);
    }

    #[test]
    fn impassable_land_is_nowhere_to_stand_for_either_body() {
        let mut scene = Scene::flat(30);
        scene
            .land_everywhere(OTHER_GROUND)
            .land_art(OTHER_GROUND, TileFlags::BLOCK);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        assert!(heights(&spans, 3, 3).is_empty());
        assert!(heights(&spans.swimming(true), 3, 3).is_empty());
    }

    #[test]
    fn a_platform_is_a_second_surface_over_the_ground_it_stands_on() {
        let mut scene = Scene::flat(0);
        scene.floor(3, 3, 20, 4);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        // Highest first: the floor's top, then the ground under it.
        assert_eq!(heights(&spans, 3, 3), vec![24, 0]);
        assert_eq!(
            heights(&spans, 4, 3),
            vec![0],
            "the column next door is untouched"
        );
        assert_eq!(index.span_count(), 2, "only the column with a static is stored");
    }

    #[test]
    fn a_stair_is_stood_on_half_way_up_and_met_at_its_base() {
        let mut scene = Scene::flat(0);
        scene.stair(3, 3, 10, 10);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        let stair = spans
            .surfaces(3, 3)
            .next()
            .expect("the stair is the higher surface");
        assert_eq!(stair.stand_z, 15, "ten tall, climbable: you stand half way up");
        assert_eq!(stair.reach_z, 10, "and you step onto it at its base");
    }

    #[test]
    fn a_wall_over_a_surface_is_the_headroom_above_it() {
        // A wall based at 16 over ground at 0: a body on the ground has sixteen
        // units of room, which is exactly a person.
        let mut scene = Scene::flat(0);
        scene.wall(3, 3, 16, 20);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        let ground = spans
            .surfaces(3, 3)
            .next()
            .expect("the ground is still a surface");
        assert_eq!(ground.stand_z, 0);
        assert_eq!(ground.clearance, 16);
    }

    #[test]
    fn a_wall_through_a_surface_leaves_no_room_at_all() {
        let mut scene = Scene::flat(0);
        scene.wall(3, 3, -4, 20);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        let ground = spans
            .surfaces(3, 3)
            .next()
            .expect("the surface is still a candidate");
        assert_eq!(
            ground.clearance, 0,
            "the wall straddles the ground: a candidate nothing fits on"
        );
    }

    #[test]
    fn a_body_on_a_platform_is_not_blocked_by_the_platform() {
        let mut scene = Scene::flat(0);
        scene.floor(3, 3, 20, 4);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        let floor = spans.surfaces(3, 3).next().expect("the floor is a surface");
        assert_eq!(floor.stand_z, 24);
        assert_eq!(
            floor.clearance,
            u8::MAX,
            "its own top is at the feet standing on it"
        );
        let ground = spans.surfaces(3, 3).nth(1).expect("the ground under it");
        assert_eq!(ground.clearance, 20, "and it is a ceiling to the ground below");
    }

    #[test]
    fn the_bake_agrees_with_the_walk_on_a_column_of_everything() {
        let mut scene = Scene::flat(-3);
        scene
            .land_everywhere(OTHER_GROUND)
            .land_art(OTHER_GROUND, TileFlags::WATER | TileFlags::BLOCK)
            .floor(3, 3, 0, 4)
            .wall(3, 3, 4, 20)
            .stair(3, 3, 24, 10)
            .floor(4, 3, 8, 4);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        for (x, y) in [(3, 3), (4, 3), (5, 3)] {
            for swimming in [false, true] {
                assert_eq!(
                    heights(&spans.swimming(swimming), x, y),
                    derived(&scene, x, y, swimming),
                    "({x}, {y}), swimming={swimming}"
                );
            }
        }
    }

    /// The step rule over the bake is the step rule over the map, for every
    /// column of `scene`, every start height in `-20..=60` and three shapes of
    /// surface underfoot.
    ///
    /// The sweep and not a case, because N2's whole content is that the answer
    /// did not change: a case proves the clause somebody thought of.
    fn check_agrees(scene: &Scene, index: &SpanIndex, swimming: bool) {
        let terrain = scene.terrain().swimming(swimming);
        let spans = view(scene, index).swimming(swimming);
        // A sweep that refused everything would agree perfectly and prove
        // nothing, and so would one that never landed anywhere but the ground.
        let mut landed = 0_u32;
        let mut refused = 0_u32;
        let mut above_the_ground = 0_u32;
        for y in 0..scene.height() {
            for x in 0..scene.width() {
                let ground = terrain.ground_z(Tile::new(x, y));
                for start_z in -20..=60 {
                    // Feet and top together: flat ground, a slope, and a body
                    // standing half way up a tall tread — the three shapes
                    // `start_surface` hands `check`.
                    for lift in [0, 4, 20] {
                        let start_top = start_z + lift;
                        let baked = spans.check(x, y, start_z, start_top);
                        assert_eq!(
                            baked,
                            terrain.check(x, y, start_z, start_top),
                            "({x}, {y}) from z={start_z} top={start_top}, swimming={swimming}"
                        );
                        match baked {
                            Some(z) => {
                                landed += 1;
                                above_the_ground +=
                                    u32::from(ground.is_none_or(|ground| z > i32::from(ground)));
                            }
                            None => refused += 1,
                        }
                    }
                }
            }
        }
        assert!(landed > 1000, "only {landed} of the sweep's steps landed");
        assert!(refused > 100, "only {refused} of the sweep's steps were refused");
        assert!(
            above_the_ground > 100,
            "only {above_the_ground} landings were on something over the land"
        );
    }

    #[test]
    fn check_is_the_map_rule_over_a_scene_of_everything() {
        let mut scene = Scene::flat(0);
        scene
            // A slope, so the tile's lowest corner and the height a body stands
            // at are different numbers — which is the whole of what `reach_z`
            // is for.
            .ground(1, 1, 6)
            .ground(2, 1, 12)
            .ground(2, 2, 18)
            // Water on one column and a mountainside on another, so both
            // abilities take a different path through the tiers.
            .land(5, 5, OTHER_GROUND)
            .land_art(OTHER_GROUND, TileFlags::WATER | TileFlags::BLOCK)
            .land(6, 6, BLOCKED_GROUND)
            .land_art(BLOCKED_GROUND, TileFlags::BLOCK)
            // A storey with a wall on it, a flight of treads, and a floor over
            // open water.
            .floor(3, 3, 20, 4)
            .wall(3, 3, 24, 20)
            .stair(4, 3, 0, 10)
            .stair(4, 3, 10, 10)
            .floor(5, 5, 0, 4)
            // ServUO's `landCheck`, in the shape where it is the only thing
            // deciding: a tread whose low end is within a step of a body down at
            // z=-5, buried in ground the same body cannot reach. Without the
            // guard the tread is a landing at z=7; with it there is nowhere to
            // stand on the column at all. The whole four-tile corner is raised
            // so the tile's *lowest* corner is 20 too — that is the height the
            // guard's start-dependent clause is measured against.
            .ground(2, 6, 20)
            .ground(3, 6, 20)
            .ground(2, 7, 20)
            .ground(3, 7, 20)
            .stair(2, 6, 0, 14);
        let index = bake(&scene);
        for swimming in [false, true] {
            check_agrees(&scene, &index, swimming);
        }
    }

    #[test]
    fn the_ground_poking_through_a_low_static_is_flagged_at_bake_time() {
        // The plank at z=0..4 under ground standing at 20: you do not climb onto
        // it, because the land it is buried in is higher than it is.
        let mut scene = Scene::flat(20);
        scene.floor(3, 3, 0, 4);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        let plank = spans
            .surfaces(3, 3)
            .find(|span| !span.flags.is_ground())
            .expect("the plank is a surface of the column");
        assert!(plank.flags.land_wins(), "the ground stands over the plank");
        assert_eq!(
            spans.check(3, 3, 20, 20),
            Some(20),
            "a body walking in at ground level stays on the ground"
        );
        // Far enough below that the guard's fourth condition fails — the body's
        // head is under the tile's lowest corner — and the plank is a candidate
        // again, exactly as `MapTerrain::check` has it.
        assert_eq!(spans.check(3, 3, 2, 2), scene.terrain().check(3, 3, 2, 2));
    }

    #[test]
    fn a_surface_with_nothing_over_it_is_not_ceiled() {
        let mut scene = Scene::flat(0);
        scene.wall(3, 3, 16, 20);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        let ground = spans.surfaces(3, 3).next().expect("the ground is a surface");
        assert!(ground.flags.is_ceiled(), "the wall is above it");
        assert_eq!(ground.clearance, 16);
        let bare = spans.surfaces(4, 3).next().expect("the column next door");
        assert!(!bare.flags.is_ceiled(), "nothing at all is above it");
        assert_eq!(bare.clearance, u8::MAX);
    }

    #[test]
    fn each_column_of_a_block_reads_its_own_run() {
        // Four columns of one block, each with a different number of surfaces
        // over it. The CSR addressing is a prefix sum of the counts of the
        // occupied columns before this one, so a run read one item out of place
        // is a floor at the wrong height rather than a panic — which is what
        // this pins.
        let mut scene = Scene::flat(0);
        scene
            .floor(1, 0, 10, 4)
            .floor(5, 2, 20, 4)
            .floor(5, 2, 30, 4)
            .floor(2, 6, 40, 4);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        assert_eq!(index.table_count(), 1, "one block holds all four");
        assert_eq!(heights(&spans, 1, 0), vec![14, 0]);
        assert_eq!(heights(&spans, 5, 2), vec![34, 24, 0]);
        assert_eq!(heights(&spans, 2, 6), vec![44, 0]);
        assert_eq!(
            heights(&spans, 7, 7),
            vec![0],
            "and a column of its own between them"
        );
    }

    #[test]
    fn the_rank_is_over_occupied_columns_and_not_over_cells() {
        // The counts are packed one byte per occupied column, so a column finds
        // its own by how many *set bits* stand before it — not by how many
        // cells do. Three columns spread across one block, the last of them in
        // the block's last cell, is the shape that tells the two apart: a rank
        // taken over cells would send cell 63 sixty-one bytes past the end of a
        // three-byte run.
        let mut scene = Scene::flat(0);
        scene
            .floor(0, 0, 10, 4)
            .floor(4, 3, 20, 4)
            .floor(7, 7, 30, 4)
            .floor(7, 7, 40, 4);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        assert_eq!(index.table_count(), 1, "one block holds all three");
        assert_eq!(
            index.column_count(),
            3,
            "and three of its sixty-four columns own a run"
        );
        assert_eq!(heights(&spans, 0, 0), vec![14, 0], "the first cell");
        assert_eq!(heights(&spans, 4, 3), vec![24, 0], "one in the middle");
        assert_eq!(heights(&spans, 7, 7), vec![44, 34, 0], "and the last");
    }

    #[test]
    fn a_column_whose_statics_leave_nothing_to_stand_on_owns_no_run() {
        // The mask says "has spans stored", not "has statics": a wall on a
        // mountainside is a cell with items and no surface. What it asserts is
        // the *size* rather than the addressing — a stored zero would answer
        // the same, since it sums to the same offset and yields the same empty
        // slice — and the size is the whole point of the packing.
        let mut scene = Scene::flat(30);
        scene
            .land_everywhere(OTHER_GROUND)
            .land_art(OTHER_GROUND, TileFlags::BLOCK)
            .wall(0, 0, 30, 20)
            .floor(5, 5, 40, 4);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        assert!(
            heights(&spans, 0, 0).is_empty(),
            "the wall stands on ground nobody stands on"
        );
        assert_eq!(index.table_count(), 1, "the block holds both statics");
        assert_eq!(index.column_count(), 1, "and one of them is a surface");
        assert_eq!(heights(&spans, 5, 5), vec![44], "which is the floor's top");
    }

    #[test]
    fn a_column_in_a_block_with_statics_but_none_of_its_own_reads_the_land() {
        let mut scene = Scene::flat_over(BlockExtent { wide: 2, down: 1 }, 7);
        scene.wall(0, 0, 0, 20);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        // The last cell of the same block, and the first of the next: neither
        // stores anything, and both are ground.
        assert_eq!(heights(&spans, 7, 7), vec![7]);
        assert_eq!(heights(&spans, 8, 0), vec![7]);
        assert_eq!(
            index.table_count(),
            1,
            "only one of the two blocks holds a static"
        );
    }

    #[test]
    fn off_the_map_holds_nothing() {
        let scene = Scene::flat(0);
        let index = bake(&scene);
        let spans = view(&scene, &index);
        assert!(heights(&spans, 8, 0).is_empty(), "past the eastern edge");
        assert!(heights(&spans, 0, 8).is_empty(), "past the southern edge");
    }

    /// Point `OPENSHARD_CLIENT` at a UO client install to run the one test
    /// below, the way `terrain.rs` does. No client files enter this repository,
    /// so it skips where they are not.
    fn real_install() -> Option<(WorldMap, TileData)> {
        let dir = std::path::PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
        if !dir.join("tiledata.mul").exists() {
            return None;
        }
        let map = openshard_uofiles::map::read_facet(&dir, 0).expect("the client's map0 should load");
        let tiles =
            openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata should load");
        Some((map, tiles))
    }

    /// The bake says exactly what the walk derives, over a box of the real
    /// facet and for both abilities.
    ///
    /// **The whole-facet form of this is the `span_index` example**, which is
    /// where N1's done-when actually lives: 29.4 million columns twice over is
    /// two seconds in release and minutes in debug. What is here is the same
    /// oracle over Britain and the water west of it — enough that a suite run on
    /// a machine with an install would notice a tier answering for the wrong
    /// column, without turning `cargo test` into a map walk.
    #[test]
    fn spans_are_the_surfaces_the_walk_derives() {
        let Some((map, tiles)) = real_install() else {
            return;
        };
        let index = SpanIndex::build(&map, &tiles);
        let mut stored = 0_u64;
        let mut deep = 0_u64;
        for y in 1600..1900_u16 {
            for x in 1350..1600_u16 {
                for swimming in [false, true] {
                    let spans = Spans::new(&map, &index).swimming(swimming);
                    let mut baked: Vec<i32> =
                        spans.surfaces(x, y).map(|span| i32::from(span.stand_z)).collect();
                    baked.sort_unstable();
                    let mut walked = stand_surfaces(&map, &tiles, x, y, swimming);
                    walked.sort_unstable();
                    assert_eq!(baked, walked, "({x}, {y}), swimming={swimming}");
                    stored += u64::from(map.statics_at(x, y).next().is_some());
                    deep += u64::from(baked.len() > 1);
                }
            }
        }
        // A box of open water would agree perfectly and prove nothing: the
        // point of the three tiers is that different columns take different
        // paths through them, so the run has to have taken all three.
        assert!(stored > 1000, "only {stored} columns in the box hold a static");
        assert!(
            deep > 100,
            "only {deep} columns in the box hold more than one surface"
        );
    }

    /// The step rule over the bake is the step rule over the map, for a node
    /// expansion out of every surface of every column in a box of the real
    /// facet.
    ///
    /// N2's suite-sized oracle. **The whole-facet form is the `span_check`
    /// example**, which also floods the facet through both rules; this is the
    /// same comparison over Britain and the water west of it, and it is the one
    /// that runs on a machine with an install and no arguments.
    #[test]
    fn check_is_the_map_rule_over_a_box_of_britain() {
        let Some((map, tiles)) = real_install() else {
            return;
        };
        let index = SpanIndex::build(&map, &tiles);
        for swimming in [false, true] {
            let terrain = MapTerrain::new(&map, &tiles, &index).swimming(swimming);
            let spans = Spans::new(&map, &index).swimming(swimming);
            let mut compared = 0_u64;
            let mut landed = 0_u64;
            for y in 1600..1900_u16 {
                for x in 1350..1600_u16 {
                    // Every height a body could be standing at here, and the top
                    // of the surface under each of them: a node expansion's
                    // start half, exactly as `can_step` computes it.
                    for start_z in terrain.surfaces(x, y) {
                        let (_, start_top) = terrain.start_surface(x, y, start_z);
                        for (dx, dy) in NEIGHBOURS {
                            let (Ok(to_x), Ok(to_y)) =
                                (u16::try_from(i32::from(x) + dx), u16::try_from(i32::from(y) + dy))
                            else {
                                continue;
                            };
                            let baked = spans.check(to_x, to_y, start_z, start_top);
                            assert_eq!(
                                baked,
                                terrain.check(to_x, to_y, start_z, start_top),
                                "({to_x}, {to_y}) from ({x}, {y}) z={start_z} top={start_top}, \
                                 swimming={swimming}"
                            );
                            compared += 1;
                            landed += u64::from(baked.is_some());
                        }
                    }
                }
            }
            assert!(compared > 100_000, "only {compared} steps compared");
            assert!(landed > 10_000, "only {landed} of them landed anywhere");
        }
    }
}
