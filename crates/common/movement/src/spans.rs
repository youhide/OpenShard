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

use crate::terrain::static_top;

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
    /// wanting `h` above `stand_z` fits exactly when `h <= clearance`. 255
    /// means nothing above at all rather than "255 units" — see
    /// [`Spans::surfaces`], where a bare column is synthesised with it.
    pub clearance: u8,
    /// What kind of surface this is. Today: whether only a swimmer stands here.
    pub flags: SpanFlags,
}

/// What a [`Span`] is, past its heights.
///
/// A byte with seven bits spare, in the shape
/// [`TileFlags`](openshard_tiles::TileFlags) has: a mask constant per bit and
/// one `has`, so a reader that only wants one bit does not learn the layout.
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
/// The `[u8; 64]` is the plan's own choice and it is a cache line: a column's
/// spans start at `base` plus the counts of the columns before it in the block,
/// which is a byte-wise prefix sum over one line rather than a 256-byte table of
/// offsets. The census caps a column at twelve spans, so a count is a byte —
/// but the builder asserts that rather than truncating, because a base set is a
/// world nobody has counted.
struct BlockTable {
    /// Where this block's columns begin in [`SpanIndex::spans`].
    base: u32,
    /// How many spans each of the block's sixty-four columns owns, in the
    /// block's own row-major cell order.
    counts: [u8; CELLS_PER_BLOCK],
}

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
                counts: [0; CELLS_PER_BLOCK],
            };
            let (origin_x, origin_y) = coord.origin();
            // A block's items are sorted by `(y, x)`, which *is* its row-major
            // cell order — so grouping them by tile walks the block's columns in
            // ascending cell, and the counts can be laid down in one pass with
            // no second sort. The assertion below is what says so out loud: get
            // this wrong and every column after the first reads a run belonging
            // to somebody else, silently.
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
                table.counts[cell] = u8::try_from(column.len()).unwrap_or_else(|_| {
                    panic!(
                        "({x}, {y}) holds {} standable surfaces; a count is a byte",
                        column.len()
                    )
                });
                index.spans.append(&mut column);
                at += run;
            }
            index.blocks[block.get() as usize] =
                u32::try_from(index.tables.len()).expect("a facet's blocks fit a u32");
            index.tables.push(table);
        }
        index.spans.shrink_to_fit();
        index.tables.shrink_to_fit();
        index
    }

    /// How much memory the bake holds, in bytes — the number
    /// `docs/map/navigation_spans.md` estimated before it was built.
    #[must_use]
    pub fn resident_bytes(&self) -> usize {
        self.blocks.capacity() * size_of::<u32>()
            + self.tables.capacity() * size_of::<BlockTable>()
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
        // The prefix sum is over one cache line, and it is the price of storing
        // counts rather than offsets — 64 bytes a block instead of 256.
        let from = table.base as usize
            + table.counts[..cell]
                .iter()
                .copied()
                .map(usize::from)
                .sum::<usize>();
        &self.spans[from..from + usize::from(table.counts[cell])]
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
const fn kind_flags(kind: LandKind) -> Option<SpanFlags> {
    match kind {
        LandKind::Ground => Some(SpanFlags(SpanFlags::NONE)),
        LandKind::Water => Some(SpanFlags(SpanFlags::SWIMMER_ONLY)),
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
    if let Some(flags) = map
        .land(x, y)
        .and_then(|cell| kind_flags(land[usize::from(cell.tile.0)]))
    {
        let corners = map.land_corners(x, y).expect("land was just present");
        let stand_z = openshard_map::map::average_corner_z(corners);
        out.push(Span {
            stand_z,
            reach_z: corners.into_iter().min().expect("four corners"),
            clearance: clearance(tiles, items, i32::from(stand_z)),
            flags,
        });
    }
    for item in items {
        // `Cover::of_static` and not a second reading of the platform bit: the
        // halved climbable lives there, both ends of the wire lay a *placed*
        // static's surface with it, and `stand_surfaces` — the oracle this has
        // to match — asks the very same question.
        let Some(cover) = Cover::of_static(tiles.static_tile(item.tile.0))
            .based_at(item.z)
            .stands()
        else {
            continue;
        };
        out.push(Span {
            stand_z: fits(cover.surface(), "a surface", x, y),
            reach_z: fits(cover.reach(), "a step's reach", x, y),
            clearance: clearance(tiles, items, cover.surface()),
            flags: SpanFlags(SpanFlags::NONE),
        });
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

/// How much room there is above `z` on a column holding `items`, saturating at
/// 255.
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
fn clearance(tiles: &TileData, items: &[StaticItem], z: i32) -> u8 {
    let mut free = i32::from(u8::MAX);
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
        free = free.min((base - z).max(0));
    }
    // Every branch above kept `free` inside `0..=255`.
    free as u8
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::BlockExtent;
    use openshard_tiles::TileFlags;

    use super::*;
    use crate::scene::Scene;
    use crate::surfaces::stand_surfaces;

    /// A land graphic the scenes below declare, so it can be made water or
    /// mountainside. Anything else is [`Scene`]'s own tile 0: plain ground.
    const OTHER_GROUND: u16 = 7;

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

    #[test]
    fn each_column_of_a_block_reads_its_own_run() {
        // Four columns of one block, each with a different number of surfaces
        // over it. The CSR addressing is a prefix sum of the counts before the
        // column, so a run read one item out of place is a floor at the wrong
        // height rather than a panic — which is what this pins.
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
}
