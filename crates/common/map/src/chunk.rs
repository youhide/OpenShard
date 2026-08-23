//! The unit the world is stored, cached, invalidated and transferred in.
//!
//! A [`Chunk`] is a square of the facet with an identity and a revision on it.
//! It is not a second representation of the world: it is cut out of a [`WorldMap`]
//! and it goes back in through [`assemble`], which builds the same `WorldMap` the
//! `.mul` importer does, through the same [`WorldMap::from_parts`]. That is what
//! makes a round trip an assertion about *bytes* rather than about two parallel
//! worlds that agree by inspection.
//!
//! # Why sixty-four tiles
//!
//! Measured on the shipped Felucca — 7168x4096, 2,906,871 statics — for every
//! candidate size, because `mechanics.md` says this is a measurement and not an
//! opinion:
//!
//! | tiles | chunks | non-empty median | manifest | screen pins | one-tile edit |
//! |---|---|---|---|---|---|
//! | 8 | 458,752 | 18 | 17.50 MiB | 625 | 320 B |
//! | 16 | 114,688 | 70 | 4.38 MiB | 169 | 1.2 KiB |
//! | 32 | 28,672 | 267 | 1.09 MiB | 49 | 4.6 KiB |
//! | **64** | **7,168** | **925** | **0.27 MiB** | **16** | **18 KiB** |
//! | 128 | 1,792 | 2,953 | 0.07 MiB | 9 | 74 KiB |
//!
//! The base set's *total* size is flat across all of them — 137 to 151 MiB —
//! so size is not the argument. What is left is overhead against blast radius,
//! and three things decide it:
//!
//! - **UO's own 8x8 block loses on overhead, and not narrowly.** A manifest
//!   with a hash per chunk is 17.5 MiB, a *ninth* of the base set it indexes,
//!   and one widest-zoom rectangle pins 625 chunks — walked three to five times
//!   a frame, per `client_today.md`.
//! - **Blast radius is the argument for a small chunk, and `overview.md`
//!   already refused it by name**: thrift is not a goal, and whole
//!   self-contained chunks over a cache that sometimes re-fetches too much beat
//!   a delta scheme that can leave a client half-patched. Eighteen kilobytes to
//!   move one wall is that trade taken deliberately.
//! - **Sixty-four is the grid every artefact derived from terrain is already
//!   keyed to** — `client/render`'s `BASE_CHUNK_TILES`, and the cache key
//!   `docs/map/minimap_lod_plan.md` asks for. Direction D's invalidation is then
//!   one-to-one instead of a fan-out.
//!
//! It is *not* the same type as a radar chunk, and `docs/pixels.md` is why:
//! sharing a divisor is a decision recorded here, not a licence to collapse two
//! grids into one value.
//!
//! # A chunk is not always whole
//!
//! Felucca divides into 64-tile chunks exactly and Tokuno does not — it is
//! 1,448 tiles square, which is 181 blocks, and 181 is not a multiple of eight.
//! So a chunk carries its own [`BlockExtent`] and an edge chunk is simply
//! smaller. Padding a facet to whole chunks would invent land that is not
//! there, and a reader cannot tell invented ocean from real ocean.
//!
//! # The order inside a chunk is the order outside it
//!
//! A chunk's blocks are laid out by [`BlockExtent`]'s own column-major rule —
//! the *same call* the facet uses, not a second spelling of it. See
//! [`crate::grid`]'s header for what that order is and why getting it backwards
//! is silent.

use openshard_protocol::world::Facet;

use crate::grid::{BlockCoord, BlockExtent, BlockIndex, LandGrid};
use crate::map::{BLOCK_SIZE, CELLS_PER_BLOCK, LandCell, StaticItem, WorldMap};
use crate::snapshot::{MapRevision, MapSnapshot};

/// Tiles along each side of a chunk.
pub const CHUNK_TILES: u32 = 64;

/// Map blocks along each side of a whole chunk.
///
/// A chunk is a whole number of blocks by construction, so no chunk boundary
/// ever splits the block the statics are indexed by.
pub const BLOCKS_PER_CHUNK: u32 = CHUNK_TILES / BLOCK_SIZE;

/// A chunk's position on the facet — not a tile, not a map block, and not a
/// radar chunk.
///
/// It happens to share a divisor with a radar chunk, which the module header
/// records as a decision. It is still a different type, because the two answer
/// different questions and `docs/pixels.md` is a list of what happens when a
/// grid is used to index another one that merely lines up with it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ChunkCoord {
    /// Chunk column, `tile_x / CHUNK_TILES`.
    pub x: u32,
    /// Chunk row, `tile_y / CHUNK_TILES`.
    pub y: u32,
}

impl ChunkCoord {
    /// The chunk a tile falls in.
    ///
    /// Says nothing about whether any facet *has* that chunk.
    pub const fn containing(x: u16, y: u16) -> Self {
        Self {
            x: x as u32 / CHUNK_TILES,
            y: y as u32 / CHUNK_TILES,
        }
    }

    /// The chunk's north-west map block.
    pub const fn block_origin(self) -> BlockCoord {
        BlockCoord {
            x: self.x * BLOCKS_PER_CHUNK,
            y: self.y * BLOCKS_PER_CHUNK,
        }
    }

    /// The chunk's north-west tile.
    pub const fn origin(self) -> (u32, u32) {
        (self.x * CHUNK_TILES, self.y * CHUNK_TILES)
    }
}

/// What names one chunk of one world.
///
/// No `map_id` above [`Facet`]: `mechanics.md` makes that conditional on ever
/// running two worlds whose facet numbers collide, and we do not. The encoding
/// carries a version byte, which is what the door back in looks like if that
/// ever changes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ChunkKey {
    /// Which facet.
    pub facet: Facet,
    /// Where on it.
    pub at: ChunkCoord,
}

/// One square of the world, self-contained.
///
/// Self-contained is the property that matters: a chunk names the facet, the
/// position and the revision it was cut at, so a blob that arrived over a wire
/// or came off a disk cache can be checked against what was asked for rather
/// than trusted because of where it was found.
///
/// Statics are held **CSR over the chunk's blocks** — one flat run and a count
/// per block — rather than a vector per block. That is the layout
/// `client_today.md`'s finding 6 measured `WorldMap` against: 120,744 allocations
/// facet-wide become one per chunk, and a block's items stay contiguous, which
/// is what [`WorldMap::statics_in_row`] needs them to be.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Chunk {
    key: ChunkKey,
    revision: MapRevision,
    /// How many blocks this chunk covers — [`BLOCKS_PER_CHUNK`] square, except
    /// at a facet's eastern or southern edge where the facet simply stops.
    extent: BlockExtent,
    /// `extent.count()` blocks of [`CELLS_PER_BLOCK`] cells, in the order
    /// [`BlockExtent::index_of`] gives them.
    land: Vec<LandCell>,
    /// Where each block's items start, plus a final entry holding the total —
    /// `extent.count() + 1` of them, non-decreasing. The CSR half of the
    /// layout: block `i` owns `items[offsets[i]..offsets[i + 1]]`.
    ///
    /// The *encoding* carries a count per block instead, with no redundant
    /// leading zero, and this prefix sum is built from it on the way in. Which
    /// is the same division of labour as [`WorldMap::from_parts`]' sort: the decoder
    /// says what is there, the type says what shape it is in.
    offsets: Vec<u32>,
    /// Every static in the chunk, its blocks in order and each block's items in
    /// the `(y, x)` stable order [`WorldMap::from_parts`] imposes.
    ///
    /// **Carrying absolute world coordinates, as [`StaticItem`] always does.**
    /// Packing them against the block is the encoding's business and happens at
    /// that boundary — a decoded chunk hands `WorldMap` exactly what the `.mul`
    /// importer hands it.
    items: Vec<StaticItem>,
}

impl Chunk {
    /// Cut one chunk out of a published facet.
    ///
    /// `None` for a chunk the facet has not — past its eastern or southern
    /// edge. A chunk that merely *overhangs* the edge is not that: it comes
    /// back with the blocks that exist and a smaller extent.
    #[must_use]
    pub fn of(snapshot: &MapSnapshot, at: ChunkCoord) -> Option<Self> {
        let map = snapshot.map();
        let origin = at.block_origin();
        let extent = chunk_extent(map.extent(), origin)?;

        let mut land = Vec::with_capacity(extent.count() as usize * CELLS_PER_BLOCK);
        let mut offsets = Vec::with_capacity(extent.count() as usize + 1);
        let mut items = Vec::new();
        offsets.push(0);
        for local in extent.blocks() {
            let block = world_block(origin, extent, local);
            land.extend_from_slice(map.land_in_block(block));
            items.extend_from_slice(map.statics_in_block(block.x, block.y));
            // A total that does not fit a `u32` would need four billion statics
            // on 4,096 tiles. The cast is the format's own width, and this is
            // where it is checked rather than at the encoder.
            offsets.push(u32::try_from(items.len()).expect("a chunk of fewer than 4G statics"));
        }

        Some(Self {
            key: ChunkKey {
                facet: snapshot.facet(),
                at,
            },
            revision: snapshot.revision(),
            extent,
            land,
            offsets,
            items,
        })
    }

    /// Build a chunk from parts a decoder already checked.
    ///
    /// The door the encoding comes back through, and the reason it is not a set
    /// of public fields: the three arrays have to agree about how many blocks
    /// there are, and a decoder that got that wrong would produce a chunk whose
    /// every later lookup silently addressed the wrong block. The prefix sum
    /// over `counts` is this type's, for the same reason [`WorldMap::from_parts`]
    /// owns the sort — a second decoder cannot accumulate it differently from
    /// the first.
    ///
    /// # Panics
    ///
    /// If `land` is not exactly `extent.count()` blocks' worth, if `counts` is
    /// not one entry per block, or if the counts do not add up to `items`.
    /// Every one of those is this crate disagreeing with itself — the decoder
    /// checks them against the bytes and reports a bad blob as an error long
    /// before here.
    #[must_use]
    pub fn from_parts(
        key: ChunkKey,
        revision: MapRevision,
        extent: BlockExtent,
        land: Vec<LandCell>,
        counts: &[u32],
        items: Vec<StaticItem>,
    ) -> Self {
        let blocks = extent.count() as usize;
        assert_eq!(
            land.len(),
            blocks * CELLS_PER_BLOCK,
            "land for a different extent"
        );
        assert_eq!(counts.len(), blocks, "one count per block");

        let mut offsets = Vec::with_capacity(blocks + 1);
        let mut running: u32 = 0;
        offsets.push(running);
        for count in counts {
            running = running
                .checked_add(*count)
                .expect("a chunk of fewer than 4G statics");
            offsets.push(running);
        }
        assert_eq!(
            running as usize,
            items.len(),
            "the counts and the items disagree about how many statics there are",
        );

        Self {
            key,
            revision,
            extent,
            land,
            offsets,
            items,
        }
    }

    /// Which chunk of which facet this is.
    #[must_use]
    pub const fn key(&self) -> ChunkKey {
        self.key
    }

    /// Which published revision of the facet it was cut at.
    #[must_use]
    pub const fn revision(&self) -> MapRevision {
        self.revision
    }

    /// How many blocks it covers — square except at a facet's edge.
    #[must_use]
    pub const fn extent(&self) -> BlockExtent {
        self.extent
    }

    /// How many statics stand in it.
    #[must_use]
    pub fn static_count(&self) -> usize {
        self.items.len()
    }

    /// One block's cells, row-major within the block.
    ///
    /// `local` is the block's position *inside the chunk*, which is what
    /// [`Chunk::extent`] indexes.
    ///
    /// # Panics
    ///
    /// If `local` is not a block of this chunk — a [`BlockIndex`] from a
    /// different, larger extent.
    #[must_use]
    pub fn land_in_block(&self, local: BlockIndex) -> &[LandCell] {
        let from = local.get() as usize * CELLS_PER_BLOCK;
        &self.land[from..from + CELLS_PER_BLOCK]
    }

    /// One block's statics, in the `(y, x)` order [`WorldMap::statics_in_block`]
    /// hands them out in.
    ///
    /// # Panics
    ///
    /// If `local` is not a block of this chunk.
    #[must_use]
    pub fn statics_in_block(&self, local: BlockIndex) -> &[StaticItem] {
        let at = local.get() as usize;
        &self.items[self.offsets[at] as usize..self.offsets[at + 1] as usize]
    }

    /// Where a block of this chunk sits on the facet.
    ///
    /// # Panics
    ///
    /// If `local` is not a block of this chunk.
    #[must_use]
    pub fn world_block(&self, local: BlockIndex) -> BlockCoord {
        world_block(self.key.at.block_origin(), self.extent, local)
    }

    /// Every block of the chunk, in the chunk's own order.
    pub fn blocks(&self) -> impl Iterator<Item = BlockIndex> {
        self.extent.blocks()
    }

    /// The land, blocks in the chunk's order and cells row-major within each.
    ///
    /// For an encoder, which writes the array it is given rather than deriving
    /// a position per cell.
    #[must_use]
    pub fn land(&self) -> &[LandCell] {
        &self.land
    }

    /// How many statics stand in each block, in the chunk's order.
    ///
    /// What the encoding carries, derived back off the prefix sum: a count is
    /// the shorter thing to write and the offsets are the faster thing to read,
    /// and neither has to be the other.
    pub fn counts(&self) -> impl Iterator<Item = u32> + '_ {
        self.offsets.windows(2).map(|pair| pair[1] - pair[0])
    }

    /// Every static in the chunk, its blocks in order.
    #[must_use]
    pub fn statics(&self) -> &[StaticItem] {
        &self.items
    }
}

/// How much of a chunk a facet actually has, or `None` for a chunk it has not.
fn chunk_extent(facet: BlockExtent, origin: BlockCoord) -> Option<BlockExtent> {
    if origin.x >= facet.wide || origin.y >= facet.down {
        return None;
    }
    Some(BlockExtent {
        wide: BLOCKS_PER_CHUNK.min(facet.wide - origin.x),
        down: BLOCKS_PER_CHUNK.min(facet.down - origin.y),
    })
}

/// A chunk-local block's position on the facet.
///
/// # Panics
///
/// If `local` is not a block of `extent`.
fn world_block(origin: BlockCoord, extent: BlockExtent, local: BlockIndex) -> BlockCoord {
    let at = extent.coord_of(local).expect("a block of this chunk");
    BlockCoord {
        x: origin.x + at.x,
        y: origin.y + at.y,
    }
}

/// Every chunk coordinate a facet of this size has, in column-major order.
///
/// The same rule the blocks are in, one level up: a base set written in this
/// order is a base set a reader can walk without a manifest saying what order
/// it is in.
pub fn chunks_of(facet: BlockExtent) -> impl Iterator<Item = ChunkCoord> {
    let wide = facet.wide.div_ceil(BLOCKS_PER_CHUNK);
    let down = facet.down.div_ceil(BLOCKS_PER_CHUNK);
    (0..wide).flat_map(move |x| (0..down).map(move |y| ChunkCoord { x, y }))
}

/// A set of chunks does not describe one facet.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AssemblyError {
    /// A chunk belongs to a different facet than the one being assembled.
    WrongFacet {
        /// What was asked for.
        wanted: Facet,
        /// What the chunk says it is.
        found: Facet,
    },
    /// Two chunks claim to be different revisions of the world.
    ///
    /// A facet assembled out of two revisions is a world that never existed —
    /// half of it before an edit and half after — which is exactly the
    /// half-patched state `overview.md` refuses whole chunks in order to avoid.
    MixedRevisions {
        /// The revision the first chunk carried.
        first: MapRevision,
        /// The one that disagreed.
        found: MapRevision,
    },
    /// A chunk lies outside the facet it claims to be part of.
    OutsideFacet {
        /// Where it says it is.
        at: ChunkCoord,
    },
    /// A chunk covers a different number of blocks than its position allows.
    WrongExtent {
        /// Where it says it is.
        at: ChunkCoord,
        /// What the facet's size makes of that position.
        wanted: BlockExtent,
        /// What the chunk claims.
        found: BlockExtent,
    },
    /// Two chunks cover the same block.
    Overlap {
        /// The chunk that arrived second.
        at: ChunkCoord,
    },
    /// The set does not cover the whole facet.
    Incomplete {
        /// How many of the facet's blocks nothing covered.
        missing: u32,
    },
}

impl std::fmt::Display for AssemblyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongFacet { wanted, found } => write!(
                f,
                "a chunk of facet {} was handed to an assembly of facet {}",
                found.0, wanted.0
            ),
            Self::MixedRevisions { first, found } => write!(
                f,
                "chunks from revisions {} and {} cannot make one world",
                first.get(),
                found.get()
            ),
            Self::OutsideFacet { at } => {
                write!(f, "chunk ({}, {}) is not on this facet", at.x, at.y)
            }
            Self::WrongExtent { at, wanted, found } => write!(
                f,
                "chunk ({}, {}) covers {}x{} blocks where the facet leaves room for {}x{}",
                at.x, at.y, found.wide, found.down, wanted.wide, wanted.down
            ),
            Self::Overlap { at } => write!(f, "chunk ({}, {}) arrived twice", at.x, at.y),
            Self::Incomplete { missing } => {
                write!(f, "{missing} blocks of the facet were not covered by any chunk")
            }
        }
    }
}

impl std::error::Error for AssemblyError {}

/// Build a facet out of a complete set of chunks.
///
/// **The second importer, and deliberately not a second world.** It ends in
/// [`WorldMap::from_parts`] — the same call `openshard_uofiles::map` ends in — so the
/// per-block sort that every later lookup binary-searches over is imposed by
/// the type either way, and a chunk decoder cannot get it wrong *differently*
/// from the `.mul` decoder.
///
/// `facet_extent` is the facet's size in blocks, which a base set records
/// beside its chunks. It is asked for rather than derived from the chunks: a
/// set missing its last chunk column would otherwise assemble happily into a
/// narrower world, and a narrower world parses perfectly.
///
/// The chunks may arrive in any order. What is checked is that they are all the
/// same facet at the same revision, that each is where and how big it says it
/// is, and that together they cover every block of the facet exactly once.
///
/// # Errors
///
/// [`AssemblyError`], one variant per way a set of chunks fails to be one world.
pub fn assemble(
    facet: Facet,
    facet_extent: BlockExtent,
    chunks: &[Chunk],
) -> Result<WorldMap, AssemblyError> {
    let blocks = facet_extent.count() as usize;
    let mut cells = vec![LandCell::default(); blocks * CELLS_PER_BLOCK];
    let mut statics: Vec<Vec<StaticItem>> = vec![Vec::new(); blocks];
    let mut covered = vec![false; blocks];
    let mut revision: Option<MapRevision> = None;

    for chunk in chunks {
        if chunk.key.facet != facet {
            return Err(AssemblyError::WrongFacet {
                wanted: facet,
                found: chunk.key.facet,
            });
        }
        match revision {
            Some(first) if first != chunk.revision => {
                return Err(AssemblyError::MixedRevisions {
                    first,
                    found: chunk.revision,
                });
            }
            _ => revision = Some(chunk.revision),
        }

        let at = chunk.key.at;
        let origin = at.block_origin();
        let wanted = chunk_extent(facet_extent, origin).ok_or(AssemblyError::OutsideFacet { at })?;
        if wanted != chunk.extent {
            return Err(AssemblyError::WrongExtent {
                at,
                wanted,
                found: chunk.extent,
            });
        }

        for local in chunk.blocks() {
            let block = world_block(origin, chunk.extent, local);
            // The facet's own index, and the chunk's is a different one: the
            // whole point of `chunk_extent` above is that the two agree about
            // which blocks exist before either is used to address an array.
            let index = facet_extent
                .index_of(block)
                .expect("a block inside the facet")
                .get() as usize;
            if std::mem::replace(&mut covered[index], true) {
                return Err(AssemblyError::Overlap { at });
            }
            let from = index * CELLS_PER_BLOCK;
            cells[from..from + CELLS_PER_BLOCK].copy_from_slice(chunk.land_in_block(local));
            statics[index] = chunk.statics_in_block(local).to_vec();
        }
    }

    let missing = covered.iter().filter(|seen| !**seen).count() as u32;
    if missing != 0 {
        return Err(AssemblyError::Incomplete { missing });
    }

    let land = LandGrid::from_file_order(
        facet_extent.wide * BLOCK_SIZE,
        facet_extent.down * BLOCK_SIZE,
        cells.into_iter(),
    );
    // `from_parts`, and not a pair of fields: the sort is the map's invariant,
    // so this importer cannot forget it and cannot get it wrong differently
    // from the `.mul` one.
    Ok(WorldMap::from_parts(land, statics))
}

#[cfg(test)]
pub(crate) mod fixture {
    use openshard_protocol::wire::{Graphic, Hue};

    use crate::grid::BlockExtent;
    use crate::map::{LandCell, StaticItem, WorldMap};
    use openshard_tiles::LandTileId;

    /// A facet that is **not** a whole number of chunks on either axis.
    ///
    /// Nine blocks square is 72 tiles, so it cuts into two chunks each way and
    /// three of the four are edge chunks — eight by eight, eight by one, one by
    /// eight and one by one. Tokuno is the real facet with this shape (181
    /// blocks square), and a fixture that divided evenly would let a decoder
    /// that assumed a whole chunk pass.
    pub const BLOCKS: u32 = 9;
    /// The fixture's side in tiles.
    pub const TILES: u32 = BLOCKS * crate::map::BLOCK_SIZE;

    /// The land of the fixture: every tile's own coordinates, so a transposed
    /// read lands on a cell that names where it should have been.
    pub fn cell(x: u16, y: u16) -> LandCell {
        LandCell {
            tile: LandTileId(u16::try_from(u32::from(x) * TILES + u32::from(y)).unwrap()),
            z: (i32::from(x) - i32::from(y)) as i8,
        }
    }

    /// A facet with land that names itself and statics in the places worth
    /// getting wrong: a chunk's corners, both sides of both chunk seams, the
    /// facet's far corner, and two items on one tile.
    pub fn map() -> WorldMap {
        let mut map = WorldMap::from_blocks(
            BlockExtent {
                wide: BLOCKS,
                down: BLOCKS,
            },
            cell,
        );
        let tiles = u16::try_from(TILES).unwrap();
        let seam = u16::try_from(super::CHUNK_TILES).unwrap();
        let at = [
            (0, 0),
            (7, 7),
            // Both sides of the eastern chunk seam, and of the southern one.
            (seam - 1, 30),
            (seam, 30),
            (30, seam - 1),
            (30, seam),
            // The one-by-one corner chunk, and the last tile of the facet.
            (seam + 3, seam + 3),
            (tiles - 1, tiles - 1),
        ];
        for (n, (x, y)) in at.into_iter().enumerate() {
            let n = u16::try_from(n).unwrap();
            map.place_static(StaticItem {
                tile: Graphic(0x100 + n),
                x,
                y,
                z: n as i8,
                hue: Hue(0),
            });
        }
        // Two on one tile, and a third under them: what the stable sort is for.
        // They must come back in this order or the client draws a different one
        // of them on top.
        for n in 0..3u16 {
            map.place_static(StaticItem {
                tile: Graphic(0x200 + n),
                x: 20,
                y: 21,
                z: 5,
                hue: Hue(n),
            });
        }
        map
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_tiles::LandTileId;

    const FACET: Facet = Facet(0);

    fn snapshot() -> MapSnapshot {
        MapSnapshot::new(FACET, fixture::map())
    }

    fn extent() -> BlockExtent {
        BlockExtent {
            wide: fixture::BLOCKS,
            down: fixture::BLOCKS,
        }
    }

    fn cut(snapshot: &MapSnapshot) -> Vec<Chunk> {
        chunks_of(extent())
            .map(|at| Chunk::of(snapshot, at).expect("a chunk of this facet"))
            .collect()
    }

    /// Every tile of the rebuilt facet answers what the original did — the
    /// ground and everything standing on it, in the same order.
    ///
    /// The order half is not decoration: `client/render` breaks a tie between
    /// two statics on one tile by taking the last, so a rebuild that returned
    /// them the other way round would draw a different item on top.
    fn assert_same_world(original: &WorldMap, rebuilt: &WorldMap) {
        assert_eq!(
            (original.width(), original.height()),
            (rebuilt.width(), rebuilt.height())
        );
        assert_eq!(original.static_count(), rebuilt.static_count());
        for y in 0..u16::try_from(fixture::TILES).unwrap() {
            for x in 0..u16::try_from(fixture::TILES).unwrap() {
                assert_eq!(
                    original.land(x, y),
                    rebuilt.land(x, y),
                    "the ground at ({x}, {y})"
                );
                let was: Vec<_> = original.statics_at(x, y).collect();
                let is: Vec<_> = rebuilt.statics_at(x, y).collect();
                assert_eq!(was, is, "the statics at ({x}, {y})");
            }
        }
    }

    #[test]
    fn a_facet_cut_into_chunks_and_assembled_is_the_same_facet() {
        let snapshot = snapshot();
        let rebuilt = assemble(FACET, extent(), &cut(&snapshot)).expect("a complete set");
        assert_same_world(snapshot.map(), &rebuilt);
    }

    #[test]
    fn the_chunks_may_arrive_in_any_order() {
        let snapshot = snapshot();
        let mut chunks = cut(&snapshot);
        chunks.reverse();
        let rebuilt = assemble(FACET, extent(), &chunks).expect("a complete set");
        assert_same_world(snapshot.map(), &rebuilt);
    }

    /// A facet that is not a whole number of chunks has edge chunks, and they
    /// are smaller rather than padded — padding would invent ocean a reader
    /// could not tell from real ocean.
    #[test]
    fn an_edge_chunk_covers_only_the_blocks_the_facet_has() {
        let snapshot = snapshot();
        let sizes: Vec<_> = cut(&snapshot)
            .iter()
            .map(|chunk| (chunk.key().at, chunk.extent()))
            .collect();
        assert_eq!(
            sizes,
            vec![
                (ChunkCoord { x: 0, y: 0 }, BlockExtent { wide: 8, down: 8 }),
                (ChunkCoord { x: 0, y: 1 }, BlockExtent { wide: 8, down: 1 }),
                (ChunkCoord { x: 1, y: 0 }, BlockExtent { wide: 1, down: 8 }),
                (ChunkCoord { x: 1, y: 1 }, BlockExtent { wide: 1, down: 1 }),
            ]
        );
    }

    #[test]
    fn a_chunk_past_the_facet_does_not_exist() {
        let snapshot = snapshot();
        assert!(Chunk::of(&snapshot, ChunkCoord { x: 2, y: 0 }).is_none());
        assert!(Chunk::of(&snapshot, ChunkCoord { x: 0, y: 2 }).is_none());
    }

    /// A chunk knows which blocks of the facet it holds, and the chunk-local
    /// order is the facet's own rule rather than a second one that agrees.
    #[test]
    fn a_chunks_blocks_are_the_facets_blocks() {
        let snapshot = snapshot();
        let chunk = Chunk::of(&snapshot, ChunkCoord { x: 1, y: 0 }).expect("a chunk");
        let blocks: Vec<_> = chunk.blocks().map(|local| chunk.world_block(local)).collect();
        assert_eq!(blocks.len(), 8);
        assert!(blocks.iter().all(|block| block.x == 8));
        assert_eq!(blocks.first().unwrap().y, 0);
        assert_eq!(blocks.last().unwrap().y, 7);

        // And the land under those blocks is the land the map has there.
        for local in chunk.blocks() {
            let block = chunk.world_block(local);
            assert_eq!(chunk.land_in_block(local), snapshot.map().land_in_block(block));
        }
    }

    #[test]
    fn a_chunk_of_another_facet_is_refused() {
        let snapshot = snapshot();
        let chunks = cut(&snapshot);
        assert_eq!(
            assemble(Facet(1), extent(), &chunks).err(),
            Some(AssemblyError::WrongFacet {
                wanted: Facet(1),
                found: FACET
            })
        );
    }

    /// Half a facet before an edit and half after is a world that never
    /// existed, which is the state whole chunks exist to make unreachable.
    #[test]
    fn chunks_from_two_revisions_do_not_make_one_world() {
        let snapshot = snapshot();
        let mut chunks = cut(&snapshot);
        let first = chunks[0].clone();
        let later = Chunk::from_parts(
            first.key(),
            MapRevision::decoded(first.revision().get() + 1),
            first.extent(),
            first.land().to_vec(),
            &first.counts().collect::<Vec<_>>(),
            first.statics().to_vec(),
        );
        chunks[0] = later;
        assert!(matches!(
            assemble(FACET, extent(), &chunks),
            Err(AssemblyError::MixedRevisions { .. })
        ));
    }

    #[test]
    fn a_set_missing_a_chunk_is_not_a_facet() {
        let snapshot = snapshot();
        let mut chunks = cut(&snapshot);
        chunks.pop();
        // The one-by-one corner chunk, which is one block of sixty-four cells.
        assert_eq!(
            assemble(FACET, extent(), &chunks).err(),
            Some(AssemblyError::Incomplete { missing: 1 })
        );
    }

    #[test]
    fn a_chunk_that_arrives_twice_is_refused() {
        let snapshot = snapshot();
        let mut chunks = cut(&snapshot);
        chunks.push(chunks[0].clone());
        assert_eq!(
            assemble(FACET, extent(), &chunks).err(),
            Some(AssemblyError::Overlap {
                at: ChunkCoord { x: 0, y: 0 }
            })
        );
    }

    /// A chunk assembled into a *narrower* facet than it was cut from is the
    /// silent transposition this crate's grid header is about, and the extent
    /// check is what stops it.
    #[test]
    fn a_chunk_is_refused_by_a_facet_it_does_not_fit() {
        let snapshot = snapshot();
        let chunks = cut(&snapshot);
        let narrow = BlockExtent { wide: 8, down: 9 };
        assert!(matches!(
            assemble(FACET, narrow, &chunks),
            Err(AssemblyError::WrongExtent { .. } | AssemblyError::OutsideFacet { .. })
        ));
    }

    /// The statics of a block come out of a chunk in the order `WorldMap` holds
    /// them, which is the order the picture depends on.
    #[test]
    fn a_blocks_statics_keep_the_maps_order() {
        let snapshot = snapshot();
        let chunk = Chunk::of(&snapshot, ChunkCoord { x: 0, y: 0 }).expect("a chunk");
        let block = BlockCoord::containing(20, 21);
        let local = chunk
            .extent()
            .index_of(BlockCoord {
                x: block.x,
                y: block.y,
            })
            .expect("a block of this chunk");
        let stacked: Vec<_> = chunk
            .statics_in_block(local)
            .iter()
            .filter(|item| (item.x, item.y) == (20, 21))
            .map(|item| (item.tile, item.hue))
            .collect();
        assert_eq!(
            stacked,
            vec![
                (Graphic(0x200), Hue(0)),
                (Graphic(0x201), Hue(1)),
                (Graphic(0x202), Hue(2)),
            ]
        );
    }

    /// The fixture's land names its own tile, so a chunk read transposed comes
    /// back holding a cell that says where it should have been.
    #[test]
    fn the_land_of_a_chunk_is_the_land_of_those_tiles() {
        let snapshot = snapshot();
        let chunk = Chunk::of(&snapshot, ChunkCoord { x: 1, y: 1 }).expect("a chunk");
        let block = chunk.world_block(chunk.blocks().next().expect("a block"));
        let (origin_x, origin_y) = block.origin();
        let first = chunk.land_in_block(chunk.blocks().next().expect("a block"))[0];
        assert_eq!(
            first.tile,
            LandTileId(u16::try_from(origin_x * fixture::TILES + origin_y).unwrap())
        );
    }
}
