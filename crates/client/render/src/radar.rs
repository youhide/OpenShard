//! One pixel per tile: the reduction a radar and a facet map are both made of.
//!
//! No GPU, no window, no camera. It is a function of the map and
//! [`RadarColors`], which is what lets the player's radar and `client.md`'s M3b
//! facet map share it — the two differ only in which rectangle of the baked
//! facet texture they show.
//!
//! # The walk is block-major, and that is not a micro-optimisation
//!
//! `Map::statics_at` binary-searches a block per call, and `map.rs:568`'s own
//! doc records what that costs at scale: asked per tile over a frame it was
//! *the largest single phase of the lighting pass*. A radar covering 256 tiles
//! square would ask it sixty-five thousand times.
//!
//! [`Map::statics_in_block`] hands back the whole block as a slice with no
//! search at all, so the walk asks once per block and buckets what it finds —
//! a thousand slice fetches instead of a hundred and thirty thousand searches,
//! for the same answer.
//!
//! # What a tile's colour is
//!
//! Its land tile, overridden by the **highest static standing on it**. Three
//! details that are each a bug if got wrong:
//!
//! - **`Map::statics_at` is not sorted by z.** Its key is `(y, x)` and nothing
//!   else, so "the last one" is not "the highest one" and the comparison is
//!   explicit.
//! - **The comparison against the land is `>=`, not `>`.** A floor tile lies at
//!   the ground's own height, and `>` would draw grass through a marble floor.
//! - **A tile with no colour at all is [`UNKNOWN`], never transparent.** Zero is
//!   how these files spell *absent*, and a transparent radar pixel would punch a
//!   hole through the window it is drawn in rather than reading as unmapped
//!   ground.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

use openshard_map::grid::BlockCoord;
use openshard_map::map::{BLOCK_SIZE, Map};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::Facet;
use openshard_uofiles::color::Color16;
use openshard_uofiles::radarcol::RadarColors;

use crate::chunk_cache::{LruBudget, WorkQueue};
use crate::radar_pass::Placement;

/// Domain values that name radar space.
///
/// Keeping these distinct is deliberately more than documentation: a chunk
/// coordinate, a world tile, a map-reader tile, and a raster extent all happen
/// to be pairs of integers, but substituting one for another is a real cache
/// or map-edge bug.  Raw integers stay at the UO-file and GPU boundaries;
/// everything inside the radar model speaks these values.
pub mod types {
    use super::Facet;

    /// One level in the radar reduction pyramid.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
    pub struct RadarLod(u8);

    impl RadarLod {
        /// The only grid requested directly from world terrain.
        pub const BASE: Self = Self(0);

        #[must_use]
        pub const fn new(value: u8) -> Self {
            Self(value)
        }

        #[must_use]
        pub const fn value(self) -> u8 {
            self.0
        }

        #[must_use]
        pub const fn is_base(self) -> bool {
            self.0 == Self::BASE.0
        }

        #[must_use]
        pub fn parent(self) -> Option<Self> {
            self.0.checked_add(1).map(Self)
        }

        #[must_use]
        pub fn child(self) -> Option<Self> {
            self.0.checked_sub(1).map(Self)
        }
    }

    impl From<u8> for RadarLod {
        fn from(value: u8) -> Self {
            Self::new(value)
        }
    }

    impl PartialEq<u8> for RadarLod {
        fn eq(&self, other: &u8) -> bool {
            self.0 == *other
        }
    }

    /// A tile in the facet's unbounded world-coordinate space.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
    pub struct RadarTile {
        x: u32,
        y: u32,
    }

    impl RadarTile {
        #[must_use]
        pub const fn new(x: u32, y: u32) -> Self {
            Self { x, y }
        }

        #[must_use]
        pub const fn x(self) -> u32 {
            self.x
        }

        #[must_use]
        pub const fn y(self) -> u32 {
            self.y
        }

        #[must_use]
        pub fn saturating_sub(self, half: RadarExtent) -> Self {
            Self::new(
                self.x.saturating_sub(u32::from(half.width()) / 2),
                self.y.saturating_sub(u32::from(half.height()) / 2),
            )
        }
    }

    impl From<(u32, u32)> for RadarTile {
        fn from((x, y): (u32, u32)) -> Self {
            Self::new(x, y)
        }
    }

    /// A non-empty rectangular extent in native radar tiles.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
    pub struct RadarExtent {
        width: u16,
        height: u16,
    }

    impl RadarExtent {
        #[must_use]
        pub fn new(width: u16, height: u16) -> Option<Self> {
            (width != 0 && height != 0).then_some(Self { width, height })
        }

        #[must_use]
        pub const fn width(self) -> u16 {
            self.width
        }

        #[must_use]
        pub const fn height(self) -> u16 {
            self.height
        }

        #[must_use]
        pub fn last_tile(self, origin: RadarTile) -> RadarTile {
            RadarTile::new(
                origin.x.saturating_add(u32::from(self.width - 1)),
                origin.y.saturating_add(u32::from(self.height - 1)),
            )
        }
    }

    impl PartialEq<(i32, i32)> for RadarExtent {
        fn eq(&self, other: &(i32, i32)) -> bool {
            (i32::from(self.width), i32::from(self.height)) == *other
        }
    }

    impl PartialEq<(u16, u16)> for RadarExtent {
        fn eq(&self, other: &(u16, u16)) -> bool {
            (self.width, self.height) == *other
        }
    }

    impl PartialEq<(u32, u32)> for RadarTile {
        fn eq(&self, other: &(u32, u32)) -> bool {
            (self.x, self.y) == *other
        }
    }

    /// A tile inside one fixed-size base chunk.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
    pub struct RadarChunkLocalTile {
        x: u16,
        y: u16,
    }

    impl RadarChunkLocalTile {
        #[must_use]
        pub const fn new(x: u16, y: u16) -> Self {
            Self { x, y }
        }

        #[must_use]
        pub const fn x(self) -> u16 {
            self.x
        }

        #[must_use]
        pub const fn y(self) -> u16 {
            self.y
        }
    }

    impl PartialEq<(u16, u16)> for RadarChunkLocalTile {
        fn eq(&self, other: &(u16, u16)) -> bool {
            (self.x, self.y) == *other
        }
    }

    /// A coordinate in the fixed-size radar chunk grid.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
    pub struct RadarChunkCoord {
        x: u32,
        y: u32,
    }

    impl RadarChunkCoord {
        #[must_use]
        pub const fn new(x: u32, y: u32) -> Self {
            Self { x, y }
        }

        #[must_use]
        pub const fn x(self) -> u32 {
            self.x
        }

        #[must_use]
        pub const fn y(self) -> u32 {
            self.y
        }

        #[must_use]
        pub fn ancestor_at(self, levels: u8) -> Self {
            Self::new(
                self.x.checked_shr(u32::from(levels)).unwrap_or(0),
                self.y.checked_shr(u32::from(levels)).unwrap_or(0),
            )
        }
    }

    /// A world rectangle sampled through the level-zero chunk grid.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct RadarRegion {
        facet: Facet,
        origin: RadarTile,
        extent: RadarExtent,
    }

    impl RadarRegion {
        #[must_use]
        pub const fn new(facet: Facet, origin: RadarTile, extent: RadarExtent) -> Self {
            Self {
                facet,
                origin,
                extent,
            }
        }

        #[must_use]
        pub const fn facet(self) -> Facet {
            self.facet
        }

        #[must_use]
        pub const fn origin(self) -> RadarTile {
            self.origin
        }

        #[must_use]
        pub const fn extent(self) -> RadarExtent {
            self.extent
        }
    }
}

pub use types::{RadarChunkCoord, RadarChunkLocalTile, RadarExtent, RadarLod, RadarRegion, RadarTile};

/// What a tile with no colour of its own draws as.
///
/// Deliberately non-zero: `Color16(0)` is *absent* in every one of these files,
/// and a pixel that is absent is a hole. Near-black, so unmapped ground reads as
/// unmapped rather than as a mistake.
pub const UNKNOWN: Color16 = Color16(0x0001);

/// How many tiles a map block is across. Re-exported so a caller sizing a buffer
/// does not reach past this module for it.
pub const BLOCK_TILES: u16 = BLOCK_SIZE as u16;

/// The side of a base radar chunk, in world tiles.
///
/// Sixty-four tiles is eight map blocks.  A chunk therefore never needs to
/// split a block walk, while still being small enough to replace independently
/// when a terrain edit arrives.  Every base chunk has this complete size: the
/// east and south border chunks fill cells beyond the facet with [`UNKNOWN`].
/// That fixed shape is what lets a parent be reduced from exactly four child
/// products without a map-edge special case.
pub const BASE_CHUNK_TILES: u16 = BLOCK_TILES * 8;

/// The number of map blocks along a base chunk edge.
pub const BASE_CHUNK_BLOCKS: u16 = BASE_CHUNK_TILES / BLOCK_TILES;
pub const SWEEP_LOD: RadarLod = RadarLod::new(2);
pub const RADAR_CPU_TAIL_BUDGET: u64 = 32 * 1024 * 1024;
const RADAR_CHUNK_CPU_BYTES: u64 = (BASE_CHUNK_TILES as u64) * (BASE_CHUNK_TILES as u64) * 2;

/// One placement of the shared radar raster.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RadarView {
    pub facet: Facet,
    pub centre: RadarTile,
    pub tiles_per_pixel: f32,
    pub placement: Placement,
    facet_extent: RadarExtent,
    device_scale: f32,
    tangent_margin: (u16, u16),
}

impl RadarView {
    #[must_use]
    pub fn new(
        facet: Facet,
        centre: RadarTile,
        facet_extent: RadarExtent,
        tiles_per_pixel: f32,
        placement: Placement,
        device_scale: f32,
    ) -> Self {
        Self {
            facet,
            centre,
            tiles_per_pixel: tiles_per_pixel.max(f32::EPSILON),
            placement,
            facet_extent,
            device_scale: device_scale.max(f32::EPSILON),
            tangent_margin: (0, 0),
        }
    }

    /// Add a round window's measured tangent slack to the fetch.
    ///
    /// `fraction` is a fraction of the window's own *logical* extent and is
    /// converted to a tile count here — the whole of it, divided by `zoom`
    /// once, and multiplied by nothing else. Both halves of that are
    /// load-bearing and both were measured; the caller that owns the number
    /// owns the reasoning with it (`panes::minimap::TANGENT_MARGIN_FRACTION`
    /// in the client, which is the only caller there is). Kept here and not
    /// inside [`Self::region`] because a square window needs none of it: a
    /// view with no margin asked for is a view that fetches exactly its own
    /// pixels.
    #[must_use]
    pub fn with_tangent_margin_fraction(
        mut self,
        logical_extent: (i32, i32),
        zoom: f32,
        fraction: f32,
    ) -> Self {
        self.tangent_margin = (
            (2.0 * (logical_extent.0 as f32 * fraction / zoom).ceil()).clamp(0.0, f32::from(u16::MAX)) as u16,
            (2.0 * (logical_extent.1 as f32 * fraction / zoom).ceil()).clamp(0.0, f32::from(u16::MAX)) as u16,
        );
        self
    }

    /// The world-tile rectangle this view needs fetched, and **the one copy of
    /// that arithmetic**.
    ///
    /// One radar texel is one world tile, and a logical window pixel can cover
    /// several *physical* ones — HiDPI, a larger desk scale, or the window's
    /// own zoom — so the fetch asks for that many more tiles rather than
    /// magnifying a cached texture, which would blur or block up what nearest
    /// sampling is for. `tiles_per_pixel` and `device_scale` are the two
    /// numbers that say how many; `placement.extent` is already in physical
    /// pixels.
    ///
    /// **No `sqrt(2)` factor for a rotated round window**, though a whole
    /// comment used to argue for one: a placed square whose half-side equals
    /// the round frame's clip radius contains its own inscribed circle at
    /// *any* rotation — its flat edges are tangent to the circle, never short
    /// of it, wherever the rotation puts them. Inflating the fetch bought
    /// nothing; what it cost was real, because every extra tile came from the
    /// square's corners, which are the farthest ground from the centre and so
    /// dead last to build under [`region_chunks_near`]'s order — a ring that
    /// can never earn an LOD stand-in either, since an ancestor needs every
    /// descendant built at least once. See [`Self::with_tangent_margin_fraction`]
    /// for the small, measured slack that *is* worth paying.
    ///
    /// The origin is clamped against **both** facet edges, not saturated at
    /// zero: a region wider than the ground left to the east is moved back
    /// inside the facet rather than reading past it, which is what the west
    /// and north edges always did. A body at the map's own corner therefore
    /// sees terrain with its marker off-centre, rather than centred terrain
    /// with a band of [`UNKNOWN`] beside it.
    #[must_use]
    pub fn region(self) -> RadarRegion {
        let width = (self.placement.extent.0 * self.device_scale * self.tiles_per_pixel)
            .ceil()
            .max(1.0) as u32
            + u32::from(self.tangent_margin.0);
        let height = (self.placement.extent.1 * self.device_scale * self.tiles_per_pixel)
            .ceil()
            .max(1.0) as u32
            + u32::from(self.tangent_margin.1);
        let width = width.min(u32::from(self.facet_extent.width())).max(1) as u16;
        let height = height.min(u32::from(self.facet_extent.height())).max(1) as u16;
        let extent = RadarExtent::new(width, height).expect("a view has a non-empty region");
        let max_x = u32::from(self.facet_extent.width() - width);
        let max_y = u32::from(self.facet_extent.height() - height);
        let origin = RadarTile::new(
            self.centre.x().saturating_sub(u32::from(width) / 2).min(max_x),
            self.centre.y().saturating_sub(u32::from(height) / 2).min(max_y),
        );
        RadarRegion::new(self.facet, origin, extent)
    }

    #[must_use]
    pub fn lod(self) -> RadarLod {
        let raw = self.tiles_per_pixel.log2().floor().max(0.0) as u8;
        RadarLod::new(raw.min(max_lod(self.facet_extent).value()))
    }

    /// Screen placement of the fetched region under the view's clip.
    #[must_use]
    pub fn map_placement(self) -> Placement {
        let region = self.region();
        let pixels_per_logical = self.tiles_per_pixel * self.device_scale;
        let extent = (
            f32::from(region.extent().width()) / pixels_per_logical,
            f32::from(region.extent().height()) / pixels_per_logical,
        );
        Placement {
            origin: (
                self.placement.origin.0 + (self.placement.extent.0 - extent.0) / 2.0,
                self.placement.origin.1 + (self.placement.extent.1 - extent.1) / 2.0,
            ),
            extent,
            ..self.placement
        }
    }
}

/// Per-window radar LOD hysteresis.
///
/// One selector per *window*, deliberately — two windows showing the same
/// facet at two zooms are two answers, and a shared selector would be one of
/// them dragging the other across its own dead band.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct RadarLodSelector {
    selected: Option<RadarLod>,
}

impl RadarLodSelector {
    #[must_use]
    pub fn update(&mut self, view: RadarView) -> RadarLod {
        let ideal = view.lod();
        let max = max_lod(view.facet_extent).value();
        let Some(selected) = self.selected else {
            self.selected = Some(ideal);
            return ideal;
        };
        // Clamped on entry, because the remembered level was chosen for
        // whichever facet the window showed last. The upward loop only runs
        // while `selected < max` and the downward one stops at zero, so a
        // level carried in from a larger facet would be returned untouched —
        // naming a grid the smaller facet's ladder does not have.
        let mut selected = RadarLod::new(selected.value().min(max));
        while selected.value() < max {
            let boundary = 2_f32.powi(i32::from(selected.value() + 1));
            if view.tiles_per_pixel < boundary * 1.1 {
                break;
            }
            selected = RadarLod::new(selected.value() + 1);
        }
        while selected.value() > 0 {
            let boundary = 2_f32.powi(i32::from(selected.value()));
            if view.tiles_per_pixel >= boundary * 0.9 {
                break;
            }
            selected = RadarLod::new(selected.value() - 1);
        }
        self.selected = Some(selected);
        selected
    }
}

/// Ask for everything the open views need, and answer with the keys this frame
/// must not evict.
///
/// The whole of the per-frame requester, and a function of its three arguments
/// alone. `App::draw_from` owns a device, a shell and a window; none of the
/// three is part of *which chunks a view needs*, and while this loop lived
/// inside that frame the rule could only be read, never asserted. Every view
/// asks for its own region at its own level and for nothing else — which is
/// what makes an open facet map unable to take a chunk away from the minimap,
/// the defect one region standing for both windows used to be
/// (`docs/map/radar.md`, defect 3.3).
///
/// A key the cache already holds is not requested and is still protected: a
/// ready chunk about to be drawn is precisely the one eviction must not take.
/// Order is nearest each view's own centre first, because
/// [`RadarWorkQueue::request`] refuses once its bound is reached, and raster
/// order would then decide whose far rows are never offered a slot at all —
/// see [`region_chunks_near`].
#[must_use]
pub fn request_views(
    views: impl IntoIterator<Item = (RadarView, RadarLod)>,
    cache: &RadarCache,
    queue: &mut RadarWorkQueue,
) -> Vec<RadarChunkKey> {
    let mut protected = Vec::new();
    for (view, lod) in views {
        let region = view.region();
        let (base_centre, _) = world_tile_to_base_chunk(view.centre);
        let centre = base_centre.ancestor_at(lod.value());
        for coord in region_chunks_near(region, lod, centre) {
            let key = cache.key(region.facet(), lod, coord);
            protected.push(key);
            if cache.get(key).is_none() {
                queue.request(key);
            }
        }
    }
    protected
}

/// Convert a world tile to the level-zero chunk and local tile that contain it.
///
/// The conversion is floor division and remainder by [`BASE_CHUNK_TILES`]:
/// tile `(64, 0)` is chunk `(1, 0)`, local tile `(0, 0)`, not the final tile
/// of chunk `(0, 0)`.  It deliberately does not clamp to a facet; callers use
/// the same conversion for an out-of-facet request, whose complete chunk
/// carries [`UNKNOWN`] along its east and south borders.
#[must_use]
pub fn world_tile_to_base_chunk(world: impl Into<RadarTile>) -> (RadarChunkCoord, RadarChunkLocalTile) {
    let world = world.into();
    let side = u32::from(BASE_CHUNK_TILES);
    (
        RadarChunkCoord::new(world.x() / side, world.y() / side),
        RadarChunkLocalTile::new((world.x() % side) as u16, (world.y() % side) as u16),
    )
}

/// Every level-zero chunk coordinate a region's rectangle touches.
///
/// Shared between a producer, which requests exactly these keys, and a
/// content pass, which reads back exactly the ones that are ready — the two
/// must agree on what "the visible chunks" are, or a chunk the queue never
/// saw requested would sit ready and undrawn, or one the pass never asks for
/// would build forever. Invalidation retains this spelling; views use
/// [`region_chunks`] at their selected level.
pub fn region_base_chunks(region: RadarRegion) -> impl Iterator<Item = RadarChunkCoord> {
    region_chunks(region, RadarLod::BASE)
}

/// Every chunk at `lod` whose world rectangle touches `region`.
pub fn region_chunks(region: RadarRegion, lod: impl Into<RadarLod>) -> impl Iterator<Item = RadarChunkCoord> {
    let lod = lod.into();
    let chunk_world = u32::from(BASE_CHUNK_TILES)
        .checked_shl(u32::from(lod.value()))
        .unwrap_or(u32::MAX);
    let last = region.extent().last_tile(region.origin());
    let first_chunk = RadarChunkCoord::new(
        region.origin().x() / chunk_world,
        region.origin().y() / chunk_world,
    );
    let last_chunk = RadarChunkCoord::new(last.x() / chunk_world, last.y() / chunk_world);
    (first_chunk.y()..=last_chunk.y())
        .flat_map(move |y| (first_chunk.x()..=last_chunk.x()).map(move |x| RadarChunkCoord::new(x, y)))
}

/// Every level-zero chunk coordinate a region's rectangle touches, nearest
/// `centre` first.
///
/// [`region_base_chunks`] is enumerated north-to-south, west-to-east within
/// each row — fine for a caller that visits every chunk unconditionally, but
/// not for one with a *bounded* budget: filling that budget in raster order
/// starves whichever chunks are enumerated last, which for a region taller
/// than the budget is every row south of wherever the budget ran out — a
/// zoomed-out minimap reads as terrain that ends abruptly at some latitude and
/// never resumes, however long the window stays open. `take_for_producer_near`
/// already orders its *dequeue* this way for the identical reason (see its own
/// doc); a caller that also *enqueues* under a bound — [`RadarWorkQueue::request`]
/// refuses once `max_queued` is reached — needs the same order on that side
/// too, or raster order still decides who is ever offered a slot at all.
pub fn region_base_chunks_near(
    region: RadarRegion,
    centre: RadarChunkCoord,
) -> impl Iterator<Item = RadarChunkCoord> {
    region_chunks_near(region, RadarLod::BASE, centre)
}

/// [`region_chunks`] in nearest-first producer order.
pub fn region_chunks_near(
    region: RadarRegion,
    lod: impl Into<RadarLod>,
    centre: RadarChunkCoord,
) -> impl Iterator<Item = RadarChunkCoord> {
    let mut chunks: Vec<_> = region_chunks(region, lod).collect();
    chunks.sort_by_key(|chunk| {
        (
            chunk
                .x()
                .abs_diff(centre.x())
                .saturating_add(chunk.y().abs_diff(centre.y())),
            chunk.y(),
            chunk.x(),
        )
    });
    chunks.into_iter()
}

/// One immutable source version of a facet's terrain and statics.
///
/// A revision deliberately cannot be omitted from a [`RadarChunkKey`].  The
/// map/content owner increments it on a mutation; moving a player does not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RadarRevision(pub u64);

/// The complete identity of a cached terrain raster.
///
/// At LOD zero, `chunk` addresses [`BASE_CHUNK_TILES`] square world-tile
/// products.  Each higher LOD addresses a product covering twice as much world
/// space in each direction, but still contains the same number of pixels.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RadarChunkKey {
    facet: Facet,
    lod: RadarLod,
    chunk: RadarChunkCoord,
    revision: RadarRevision,
}

impl RadarChunkKey {
    /// Constructed only by [`RadarCache`], the owner of source revisions.
    ///
    /// Keeping this crate-visible prevents a window or a player marker from
    /// accidentally creating a second, position-keyed terrain cache.
    #[must_use]
    pub(crate) fn new(
        facet: Facet,
        lod: impl Into<RadarLod>,
        chunk: RadarChunkCoord,
        revision: RadarRevision,
    ) -> Self {
        Self {
            facet,
            lod: lod.into(),
            chunk,
            revision,
        }
    }

    #[must_use]
    pub const fn facet(self) -> Facet {
        self.facet
    }

    #[must_use]
    pub const fn lod(self) -> RadarLod {
        self.lod
    }

    #[must_use]
    pub const fn chunk(self) -> RadarChunkCoord {
        self.chunk
    }

    #[must_use]
    pub const fn revision(self) -> RadarRevision {
        self.revision
    }

    /// The north-west world tile a base chunk starts at, if it is representable
    /// by the map reader's `u16` coordinates.
    #[must_use]
    pub fn base_origin(self) -> Option<RadarTile> {
        if !self.lod.is_base() {
            return None;
        }
        let x = self.chunk.x().checked_mul(u32::from(BASE_CHUNK_TILES))?;
        let y = self.chunk.y().checked_mul(u32::from(BASE_CHUNK_TILES))?;
        if x > u32::from(u16::MAX) || y > u32::from(u16::MAX) {
            return None;
        }
        Some(RadarTile::new(x, y))
    }
}

/// An immutable, complete terrain product ready for cache publication.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RadarChunk {
    key: RadarChunkKey,
    pixels: Vec<Color16>,
}

/// How a draw request was satisfied by ready terrain.
///
/// The cache only ever returns a complete raster.  When the requested product
/// is not ready, a current-source ancestor is preferred because it covers the
/// same world area without exposing stale pixels.  If no such ancestor is
/// ready, the newest retained product for the exact chunk remains a safe,
/// complete (though stale) picture while production catches up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RadarReadyKind {
    /// The requested key is ready under the current source revision.
    Exact,
    /// A ready current-source parent at a coarser LOD covers the request.
    CoarserAncestor,
    /// The newest retained revision for the exact requested chunk is ready.
    StaleExact,
}

/// A complete ready chunk selected for a terrain draw, with its fallback mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RadarReadyChunk<'a> {
    chunk: &'a RadarChunk,
    kind: RadarReadyKind,
}

impl<'a> RadarReadyChunk<'a> {
    #[must_use]
    pub const fn chunk(self) -> &'a RadarChunk {
        self.chunk
    }

    #[must_use]
    pub const fn kind(self) -> RadarReadyKind {
        self.kind
    }
}

/// How one frame's demand was answered, by fallback mode.
///
/// The three arms of [`RadarReadyKind`] plus the case it cannot express:
/// nothing ready at all, which is the backdrop a player actually sees. The
/// four are a partition of the requested set, so [`Self::total`] is that set's
/// size.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct RadarDemand {
    /// Ready at the requested key, under the current source revision.
    pub exact: usize,
    /// Stood in for by a ready coarser parent — correct terrain, drawn blurry.
    pub coarser: usize,
    /// Stood in for by the newest retained revision of the same chunk — sharp
    /// terrain that is out of date.
    pub stale: usize,
    /// Nothing ready: this chunk's area is backdrop this frame.
    pub missing: usize,
}

impl RadarDemand {
    /// Every chunk the request set named.
    #[must_use]
    pub const fn total(self) -> usize {
        self.exact + self.coarser + self.stale + self.missing
    }
}

/// What a frame's requested keys resolve to: the picture, and the reading.
#[derive(Clone, Default, Debug)]
pub struct RadarResolved {
    /// How the cache answered each request.
    pub demand: RadarDemand,
    /// The key each answered request will actually be drawn from. A coarse
    /// ancestor standing in for four requests appears once per request, which
    /// is harmless — eviction only needs the set — and is what keeps this a
    /// single walk.
    pub drawn: Vec<RadarChunkKey>,
}

/// Ask the cache what every requested key will be drawn from, and how it
/// answered.
///
/// One walk answering both questions, because they have the same body: a draw
/// looks each key up to find its chunk, and eviction must not take the chunk a
/// draw found. Counting the *kinds* on the way is free, and it is the only
/// thing that distinguishes a radar filling in from a radar starved — both of
/// which look like missing terrain on screen.
///
/// A free function beside [`request_views`] and for its reason: while this
/// loop lived inside `App::draw_from` it needed a window, a device and a
/// shell, none of which is part of *what the cache holds for these keys*.
#[must_use]
pub fn resolve_demand(
    cache: &RadarCache,
    requested: impl IntoIterator<Item = RadarChunkKey>,
) -> RadarResolved {
    let mut resolved = RadarResolved::default();
    for key in requested {
        let Some(ready) = cache.select_ready(key) else {
            resolved.demand.missing += 1;
            continue;
        };
        match ready.kind() {
            RadarReadyKind::Exact => resolved.demand.exact += 1,
            RadarReadyKind::CoarserAncestor => resolved.demand.coarser += 1,
            RadarReadyKind::StaleExact => resolved.demand.stale += 1,
        }
        resolved.drawn.push(ready.chunk().key());
    }
    resolved
}

/// Frame-diagnostic counters for retained CPU terrain products.
///
/// `requested`, `rebuilt` and `evicted` are lifetime event counters; the rest
/// are snapshots. Queue-owned counters are exposed by `RadarWorkQueue`, since
/// a cache does not dispatch or retain pending work.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct RadarCacheCounters {
    pub requested: u64,
    pub ready: usize,
    pub stale: usize,
    pub rebuilt: u64,
    pub evicted: u64,
    /// What every retained product weighs, current and stale together.
    pub retained_bytes: u64,
    /// The tail budget [`RadarCache::evict_to_budget`] measures that weight
    /// against. Reported beside it rather than left as a constant a reader has
    /// to go and look up, because the pair *is* the headroom: a ceiling that
    /// pinned chunks raise cannot be read off `retained_bytes` alone, and
    /// `retained_bytes` above `tail_budget` is the only state in which this
    /// cache evicts at all.
    pub tail_budget: u64,
}

/// The sole owner of ready radar terrain products and their source revisions.
///
/// This belongs with the map/content lifetime, not a minimap window. Closing a
/// window therefore leaves ready chunks intact, while a source revision change
/// makes them unreachable without any player movement or UI event. Production
/// and dirty-parent tracking deliberately arrive in the next cache phase; this
/// type establishes the one authoritative identity they operate on.
#[derive(Debug)]
pub struct RadarCache {
    revisions: BTreeMap<Facet, RadarRevision>,
    ready: BTreeMap<RadarChunkKey, RadarChunk>,
    highest_ready_lod: BTreeMap<(Facet, RadarRevision), RadarLod>,
    requested: Cell<u64>,
    rebuilt: u64,
    evicted: u64,
    budget: LruBudget<RadarChunkKey>,
    tail_budget: u64,
    /// Every coarse key a facet's sweep still owes, per facet.
    ///
    /// A set and not a flag. The flag said *the sweep ran*, which is a
    /// different claim from *the floor exists*: the enqueue loop it guarded
    /// called [`RadarWorkQueue::request_sweep`] once per chunk, that call
    /// returns `false` when the queue is at its bound, and a refused key was
    /// never offered again. Nothing was lost loudly — the hole appears weeks
    /// later as a patch of backdrop at some zoom, on ground no window has
    /// ever drawn at level zero. An empty entry is a sweep that finished; a
    /// missing one is a sweep that never started.
    sweep_owed: BTreeMap<Facet, BTreeSet<RadarChunkKey>>,
    /// Current-source products which a terrain/static mutation made unsafe to
    /// reuse.  A producer consumes these keys in a later phase; keeping the
    /// work here makes the content owner, rather than a minimap window, the
    /// sole authority on what needs rebuilding.
    dirty: BTreeSet<RadarChunkKey>,
}

impl Default for RadarCache {
    fn default() -> Self {
        Self {
            revisions: BTreeMap::new(),
            ready: BTreeMap::new(),
            highest_ready_lod: BTreeMap::new(),
            requested: Cell::new(0),
            rebuilt: 0,
            evicted: 0,
            budget: LruBudget::new(RADAR_CPU_TAIL_BUDGET).expect("the shipped radar CPU budget is non-zero"),
            tail_budget: RADAR_CPU_TAIL_BUDGET,
            sweep_owed: BTreeMap::new(),
            dirty: BTreeSet::new(),
        }
    }
}

impl RadarCache {
    #[must_use]
    pub fn with_tail_budget(bytes: u64) -> Option<Self> {
        let budget = LruBudget::new(bytes)?;
        Some(Self {
            budget,
            tail_budget: bytes,
            ..Self::default()
        })
    }

    /// The source revision currently authoritative for a facet.
    #[must_use]
    pub fn revision(&self, facet: Facet) -> RadarRevision {
        self.revisions.get(&facet).copied().unwrap_or(RadarRevision(0))
    }

    /// Construct a cache key under the facet's current source revision.
    #[must_use]
    pub fn key(&self, facet: Facet, lod: impl Into<RadarLod>, chunk: RadarChunkCoord) -> RadarChunkKey {
        RadarChunkKey::new(facet, lod, chunk, self.revision(facet))
    }

    /// Adopt a newer revision named by the map/content owner.
    ///
    /// Existing products are retained only as stale, recreatable storage; all
    /// normal lookup is through [`Self::key`] and therefore cannot return one
    /// for the superseded source revision.
    /// Returns `false` for an old or unchanged revision, so a delayed source
    /// notification cannot make an older ready product current again.
    pub fn set_revision(&mut self, facet: Facet, revision: RadarRevision) -> bool {
        if revision <= self.revision(facet) {
            return false;
        }
        self.revisions.insert(facet, revision);
        // Dirty keys name one exact source snapshot.  A separately announced
        // newer snapshot supersedes any incomplete invalidation for this facet.
        self.dirty.retain(|key| key.facet != facet);
        true
    }

    /// Record a terrain or static mutation at one world tile.
    ///
    /// The mutation creates a new source revision for `facet`, marks the
    /// intersecting level-zero chunk, then marks its parent at every LOD up to
    /// and including `max_lod`.  Thus a one-tile edit requests exactly one
    /// base rebuild and one product at each derived level; sibling chunks are
    /// not raster work merely because they share an ancestor.
    ///
    /// Returns `None` only if the facet revision has exhausted `u64`.  The map
    /// itself must not be changed in that case, because the cache can no longer
    /// name a newer immutable source product.
    pub fn invalidate_tile(
        &mut self,
        facet: Facet,
        tile: impl Into<RadarTile>,
        max_lod: impl Into<RadarLod>,
    ) -> Option<RadarRevision> {
        let max_lod = max_lod.into();
        let revision = RadarRevision(self.revision(facet).0.checked_add(1)?);
        self.revisions.insert(facet, revision);
        self.dirty.retain(|key| key.facet != facet);

        let (base_chunk, _) = world_tile_to_base_chunk(tile);
        for lod in 0..=max_lod.value() {
            let chunk = base_chunk.ancestor_at(lod);
            self.dirty
                .insert(RadarChunkKey::new(facet, RadarLod::new(lod), chunk, revision));
        }
        Some(revision)
    }

    /// Whether this exact current-source product still needs a rebuild.
    #[must_use]
    pub fn is_dirty(&self, key: RadarChunkKey) -> bool {
        key.revision == self.revision(key.facet) && self.dirty.contains(&key)
    }

    /// The current dirty products in deterministic production order.
    ///
    /// This is intentionally a snapshot rather than a consuming queue: Phase
    /// 2.3's bounded producer owns dispatch and removes a key only once it has
    /// published a complete CPU chunk.
    #[must_use]
    pub fn dirty_keys(&self) -> Vec<RadarChunkKey> {
        self.dirty.iter().copied().collect()
    }

    /// Publish one complete CPU product if it still matches the source.
    ///
    /// A producer that finishes after a terrain/static edit loses this race and
    /// cannot make stale pixels ready again.
    pub fn publish(&mut self, chunk: RadarChunk) -> bool {
        let key = chunk.key();
        if key.revision != self.revision(key.facet) {
            return false;
        }
        self.ready.insert(key, chunk);
        self.highest_ready_lod
            .entry((key.facet, key.revision))
            .and_modify(|lod| *lod = (*lod).max(key.lod))
            .or_insert(key.lod);
        self.budget.insert(key, RADAR_CHUNK_CPU_BYTES);
        self.dirty.remove(&key);
        self.rebuilt = self.rebuilt.saturating_add(1);
        true
    }

    /// The complete product ready for a current cache key.
    #[must_use]
    pub fn get(&self, key: RadarChunkKey) -> Option<&RadarChunk> {
        let ready = (key.revision == self.revision(key.facet))
            .then(|| self.ready.get(&key))
            .flatten();
        if ready.is_some() {
            self.budget.touch(key);
        }
        ready
    }

    /// Select complete terrain for a draw request without exposing a hole.
    ///
    /// Selection is ordered as current exact product, nearest current coarser
    /// ancestor, then the newest retained revision of the exact key. `None`
    /// means the cache has never produced terrain covering this request; the
    /// renderer must draw its explicit [`UNKNOWN`] placeholder rather than bind
    /// an uninitialised or blank texture.
    #[must_use]
    pub fn select_ready(&self, key: RadarChunkKey) -> Option<RadarReadyChunk<'_>> {
        self.requested.set(self.requested.get().saturating_add(1));
        let current_revision = self.revision(key.facet);
        if key.revision == current_revision {
            if let Some(chunk) = self.ready.get(&key) {
                self.budget.touch(key);
                return Some(RadarReadyChunk {
                    chunk,
                    kind: RadarReadyKind::Exact,
                });
            }

            let highest = self
                .highest_ready_lod
                .get(&(key.facet, current_revision))
                .copied()
                .unwrap_or(key.lod);
            for ancestor_lod in key.lod.value().saturating_add(1)..=highest.value() {
                let ancestor_key = RadarChunkKey::new(
                    key.facet,
                    RadarLod::new(ancestor_lod),
                    key.chunk.ancestor_at(ancestor_lod - key.lod.value()),
                    current_revision,
                );
                if let Some(chunk) = self.ready.get(&ancestor_key) {
                    self.budget.touch(ancestor_key);
                    return Some(RadarReadyChunk {
                        chunk,
                        kind: RadarReadyKind::CoarserAncestor,
                    });
                }
            }
        }

        let first = RadarChunkKey::new(key.facet, key.lod, key.chunk, RadarRevision(0));
        let last = RadarChunkKey::new(key.facet, key.lod, key.chunk, current_revision);
        let selected = self.ready.range(first..=last).next_back().map(|(key, chunk)| {
            (
                *key,
                RadarReadyChunk {
                    chunk,
                    kind: RadarReadyKind::StaleExact,
                },
            )
        });
        if let Some((key, _)) = selected {
            self.budget.touch(key);
        }
        selected.map(|(_, ready)| ready)
    }

    /// Exhaustive oracle retained for checking the indexed selection path.
    #[cfg(test)]
    fn select_ready_reference(&self, key: RadarChunkKey) -> Option<RadarReadyChunk<'_>> {
        let current_revision = self.revision(key.facet);
        if key.revision == current_revision {
            if let Some(chunk) = self.ready.get(&key) {
                return Some(RadarReadyChunk {
                    chunk,
                    kind: RadarReadyKind::Exact,
                });
            }
            if let Some((_, chunk)) = self
                .ready
                .iter()
                .filter(|(candidate, _)| {
                    candidate.facet == key.facet
                        && candidate.revision == current_revision
                        && candidate.lod > key.lod
                        && key.chunk.ancestor_at(candidate.lod.value() - key.lod.value()) == candidate.chunk
                })
                .min_by_key(|(candidate, _)| candidate.lod)
            {
                return Some(RadarReadyChunk {
                    chunk,
                    kind: RadarReadyKind::CoarserAncestor,
                });
            }
        }
        self.ready
            .iter()
            .filter(|(candidate, _)| {
                candidate.facet == key.facet
                    && candidate.lod == key.lod
                    && candidate.chunk == key.chunk
                    && candidate.revision <= current_revision
            })
            .max_by_key(|(candidate, _)| candidate.revision)
            .map(|(_, chunk)| RadarReadyChunk {
                chunk,
                kind: RadarReadyKind::StaleExact,
            })
    }

    /// Cache-owned diagnostic counters, sampled once per frame by callers.
    #[must_use]
    pub fn counters(&self) -> RadarCacheCounters {
        let current = self
            .ready
            .keys()
            .filter(|key| key.revision == self.revision(key.facet))
            .count();
        RadarCacheCounters {
            requested: self.requested.get(),
            ready: current,
            stale: self.ready.len() - current,
            rebuilt: self.rebuilt,
            evicted: self.evicted,
            retained_bytes: self.budget.retained_bytes(),
            tail_budget: self.tail_budget,
        }
    }

    /// Number of retained complete products, including superseded products
    /// awaiting the cache budget/eviction policy from Phase 2.
    #[must_use]
    pub fn retained_len(&self) -> usize {
        self.ready.len()
    }

    /// Owe a facet its whole coarse floor, once per session.
    ///
    /// Answers `true` on the call that starts one. Every level from
    /// [`SWEEP_LOD`] up to the facet's own [`max_lod`] is enumerated here, at
    /// the revision current when the facet map first opened; keys are handed
    /// out by [`Self::drain_sweep`] and struck off as they land.
    pub fn begin_sweep(&mut self, facet: Facet, extent: RadarExtent) -> bool {
        if self.sweep_owed.contains_key(&facet) {
            return false;
        }
        let whole_facet = RadarRegion::new(facet, RadarTile::new(0, 0), extent);
        let owed: BTreeSet<_> = (SWEEP_LOD.value()..=max_lod(extent).value())
            .flat_map(|lod| {
                let lod = RadarLod::new(lod);
                region_chunks(whole_facet, lod).map(move |chunk| (lod, chunk))
            })
            .map(|(lod, chunk)| self.key(facet, lod, chunk))
            .collect();
        self.sweep_owed.insert(facet, owed);
        true
    }

    #[must_use]
    pub fn sweep_started(&self, facet: Facet) -> bool {
        self.sweep_owed.contains_key(&facet)
    }

    /// How many coarse chunks a facet's sweep still owes.
    #[must_use]
    pub fn sweep_owed_len(&self, facet: Facet) -> usize {
        self.sweep_owed.get(&facet).map_or(0, BTreeSet::len)
    }

    /// Offer every key the sweep still owes, and strike off the ones it does
    /// not owe any more. Answers what is left.
    ///
    /// Called every frame the facet map is open, which is what makes a refused
    /// request harmless: `request` is a *try*, and a key the queue had no room
    /// for this frame is offered again on the next one. A key is struck off
    /// when its product is ready — or when a mutation has moved the facet's
    /// revision past it, because from that moment the chunk is the dirty set's
    /// to rebuild and not the sweep's.
    pub fn drain_sweep(&mut self, facet: Facet, mut request: impl FnMut(RadarChunkKey) -> bool) -> usize {
        let current = self.revisions.get(&facet).copied().unwrap_or(RadarRevision(0));
        let Some(owed) = self.sweep_owed.get_mut(&facet) else {
            return 0;
        };
        let ready = &self.ready;
        owed.retain(|key| {
            if key.revision != current || ready.contains_key(key) {
                return false;
            }
            request(*key);
            true
        });
        owed.len()
    }

    /// Bound the demand-driven tail while retaining the coarse fallback floor
    /// and every key an open view is about to draw.
    pub fn evict_to_budget(&mut self, protected: impl IntoIterator<Item = RadarChunkKey>) -> usize {
        // Nothing pinned can lower the ceiling: `max_bytes` is the tail budget
        // *plus* whatever the pinned set turns out to weigh. So a cache inside
        // the tail budget cannot evict, and the walk below — every ready key,
        // every frame, to rebuild a set that is about to change nothing — is
        // skipped without asking what is in it. This is the ordinary frame:
        // the swept floor is 599 chunks against a budget of four thousand.
        if self.budget.retained_bytes() <= self.tail_budget {
            return 0;
        }
        let mut pinned: BTreeSet<_> = self
            .ready
            .keys()
            .copied()
            .filter(|key| key.lod >= SWEEP_LOD && key.revision == self.revision(key.facet))
            .collect();
        pinned.extend(protected.into_iter().filter(|key| self.ready.contains_key(key)));
        let pinned_bytes = pinned.len() as u64 * RADAR_CHUNK_CPU_BYTES;
        self.budget
            .set_max_bytes(self.tail_budget.saturating_add(pinned_bytes));
        self.budget.set_protected(pinned);
        let revisions = &self.revisions;
        let report = self.budget.evict_to_budget_by(|key| {
            let current = revisions.get(&key.facet).copied().unwrap_or(RadarRevision(0));
            key.revision == current
        });
        for key in &report.keys {
            self.ready
                .remove(key)
                .expect("the CPU LRU decision names a ready radar chunk");
        }
        if !report.keys.is_empty() {
            self.rebuild_highest_ready_lods();
        }
        self.evicted = self.evicted.saturating_add(report.keys.len() as u64);
        report.keys.len()
    }

    fn rebuild_highest_ready_lods(&mut self) {
        self.highest_ready_lod.clear();
        for key in self.ready.keys() {
            self.highest_ready_lod
                .entry((key.facet, key.revision))
                .and_modify(|lod| *lod = (*lod).max(key.lod))
                .or_insert(key.lod);
        }
    }
}

/// A bounded hand-off from cache invalidation to a radar chunk producer.
///
/// This queue deliberately owns scheduling only: it never walks a [`Map`],
/// allocates pixels, or uploads a texture.  A presentation frame may refresh
/// its dirty view cheaply, while an idle worker takes at most
/// a fixed cost in base-chunk units and returns complete [`RadarChunk`]s through
/// [`Self::finish`].  Keeping those paths separate is what prevents a newly
/// exposed minimap area from becoming a synchronous rasterisation burst.
#[derive(Debug)]
pub struct RadarWorkQueue {
    queue: WorkQueue<RadarChunkKey>,
    priorities: BTreeMap<RadarChunkKey, RadarWorkPriority>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum RadarWorkPriority {
    View,
    Sweep,
}

/// Queue-owned frame diagnostics, separate from retained cache products.
///
/// There is deliberately no *refused* counter here. [`RadarWorkQueue::request`]
/// answering `false` is an ordinary event for sweep work — [`RadarCache::drain_sweep`]
/// offers every owed key again the next frame precisely because a refusal is
/// expected — so a refusal total would climb through a healthy session and read
/// as an alarm. What a reader needs is the headroom, and that is `max_queued`
/// against the two lengths above.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct RadarWorkCounters {
    /// Work waiting to be handed to a producer.
    pub queued: usize,
    /// Work a producer has accepted but not yet returned.
    pub in_flight: usize,
    /// The bound `queued + in_flight` is refused at.
    pub max_queued: usize,
}

impl Default for RadarWorkQueue {
    fn default() -> Self {
        // Eight level-zero units preserve the original production rate. A
        // coarse product spends 4^lod units, so zoom cannot silently multiply
        // the synchronous map walk by hundreds.
        Self::new(1024, 8).expect("the shipped radar queue limits are non-zero")
    }
}

impl RadarWorkQueue {
    /// Construct a queue with explicit total and producer-turn bounds.
    ///
    /// `max_queued` includes work already handed to a producer.  A stalled
    /// producer therefore cannot make the amount of outstanding map work grow
    /// without bound.
    #[must_use]
    pub fn new(max_queued: usize, units_per_turn: usize) -> Option<Self> {
        (max_queued != 0 && units_per_turn != 0).then_some(Self {
            queue: WorkQueue::new(max_queued, units_per_turn)
                .expect("the radar wrapper has checked its limits"),
            priorities: BTreeMap::new(),
        })
    }

    /// Number of requests waiting for a producer.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.queue.pending_len()
    }

    /// Number of requests currently owned by a producer.
    #[must_use]
    pub fn in_flight_len(&self) -> usize {
        self.queue.in_flight_len()
    }

    /// The total outstanding bound [`Self::request`] refuses at.
    #[must_use]
    pub const fn max_queued(&self) -> usize {
        self.queue.max_outstanding()
    }

    /// Queue state suitable for a frame diagnostic report.
    #[must_use]
    pub fn counters(&self) -> RadarWorkCounters {
        RadarWorkCounters {
            queued: self.pending_len(),
            in_flight: self.in_flight_len(),
            max_queued: self.max_queued(),
        }
    }

    /// Enqueue one immutable product, coalescing an identical request.
    ///
    /// Returns `false` only when this is a new key and the total outstanding
    /// work is at its explicit bound.  Re-requesting pending or in-flight work
    /// always succeeds without consuming another slot.
    pub fn request(&mut self, key: RadarChunkKey) -> bool {
        self.request_with_priority(key, RadarWorkPriority::View)
    }

    /// Enqueue background pyramid work below every open view's demand.
    pub fn request_sweep(&mut self, key: RadarChunkKey) -> bool {
        self.request_with_priority(key, RadarWorkPriority::Sweep)
    }

    fn request_with_priority(&mut self, key: RadarChunkKey, priority: RadarWorkPriority) -> bool {
        if self.queue.request(key) {
            self.priorities
                .entry(key)
                .and_modify(|was| *was = (*was).min(priority))
                .or_insert(priority);
            true
        } else {
            false
        }
    }

    /// Reconcile pending work with what the cache already holds.
    ///
    /// Two demands feed this queue, and one rule prunes both: work a mutation
    /// named through [`RadarCache::invalidate_tile`], and work a window asked
    /// for through [`Self::request`] because it is about to draw ground no
    /// product has ever covered. A pending key survives only while it still
    /// names something worth building — the facet's current source revision,
    /// and no complete product published for it yet.
    ///
    /// Both halves of that test are load-bearing. Keeping only dirty keys
    /// would discard every request a window made, since demand for unbuilt
    /// ground is never an invalidation, and the minimap would wait forever on
    /// a queue that emptied itself each frame. Keeping every current key would
    /// rebuild ready terrain for as long as the window stayed open, because a
    /// window re-asks for all of its visible chunks every frame.
    ///
    /// An already-running old revision is left to finish safely: publication
    /// rejects it against the cache revision, rather than attempting to cancel
    /// a producer that may already be reading its immutable source snapshot.
    pub fn reconcile(&mut self, cache: &RadarCache) {
        // `get` answers `None` for a superseded revision as well as for a key
        // never built, so the revision is tested on its own: a stale pending
        // key has to go, not be kept because no product answers to it.
        self.queue
            .reconcile(|key| key.revision == cache.revision(key.facet) && cache.get(key).is_none());
        self.priorities.retain(|key, _| self.queue.contains_pending(*key));
        for key in cache.dirty_keys() {
            self.request(key);
        }
    }

    /// Hand a bounded, deterministic batch to an idle/background producer.
    ///
    /// This only transfers ownership between queue sets.  It does not build
    /// pixels and is consequently safe to call from presentation bookkeeping.
    #[must_use]
    pub fn take_for_producer(&mut self) -> Vec<RadarChunkKey> {
        let priorities = &self.priorities;
        // Read with a default rather than indexed. Every path that makes a key
        // pending inserts its priority too, so a missing one is an invariant
        // violation — but a panic sited inside an `Ord` comparator, in a
        // frame, is a bad place to spend one, and `View` is the answer the
        // invariant says it would have found.
        let priority = |key: &RadarChunkKey| priorities.get(key).copied().unwrap_or(RadarWorkPriority::View);
        let keys = self.queue.take_for_producer_by_cost(
            |left, right| (priority(left), *left).cmp(&(priority(right), *right)),
            |key| lod_cost(key.lod()),
        );
        for key in &keys {
            self.priorities.remove(key);
        }
        keys
    }

    /// Like [`Self::take_for_producer`], but starts with chunks closest to the
    /// player.  A map viewport can expose more chunks than fit in one bounded
    /// production turn; coordinate order would otherwise fill its north-west
    /// corner first, which becomes a visibly displaced wedge after rotation.
    #[must_use]
    pub fn take_for_producer_near(&mut self, centre: RadarChunkCoord) -> Vec<RadarChunkKey> {
        let priorities = &self.priorities;
        // Defaulted for [`Self::take_for_producer`]'s reason.
        let priority = |key: &RadarChunkKey| priorities.get(key).copied().unwrap_or(RadarWorkPriority::View);
        let keys = self.queue.take_for_producer_by_cost(
            |left, right| {
                let distance_key = |key: &RadarChunkKey| {
                    let chunk = key.chunk();
                    (
                        priority(key),
                        std::cmp::Reverse(key.lod().value()),
                        chunk
                            .x()
                            .abs_diff(centre.x())
                            .saturating_add(chunk.y().abs_diff(centre.y())),
                        chunk.y(),
                        chunk.x(),
                    )
                };
                distance_key(left).cmp(&distance_key(right))
            },
            |key| lod_cost(key.lod()),
        );
        for key in &keys {
            self.priorities.remove(key);
        }
        keys
    }

    /// Release a dispatched job and publish its complete result if current.
    ///
    /// Results not dispatched by this queue are refused.  A result made stale
    /// by a later mutation still releases its slot, but [`RadarCache::publish`]
    /// rejects the pixels; a later [`Self::reconcile`] queues the newer
    /// source key.
    pub fn finish(&mut self, cache: &mut RadarCache, chunk: RadarChunk) -> bool {
        let key = chunk.key();
        if !self.queue.finish(key) {
            return false;
        }
        cache.publish(chunk)
    }

    /// Release a dispatched job the producer could not build at all.
    ///
    /// Not the same as a stale result, which [`Self::finish`] handles: this is
    /// work whose *source* was unavailable — a key naming a rectangle the map
    /// reader cannot address, or a derived product whose children are not all
    /// ready. Without it a slot handed out is a slot lost, and enough of them
    /// silently fill `max_queued` until no terrain is ever requested again.
    /// A key still named by the cache's dirty set is re-queued by the next
    /// [`Self::reconcile`].
    pub fn abandon(&mut self, key: RadarChunkKey) -> bool {
        self.queue.abandon(key)
    }
}

impl RadarChunk {
    /// Construct a complete fixed-size raster.  A partial chunk is never a
    /// cache value: map borders are represented by [`UNKNOWN`] pixels instead.
    #[must_use]
    pub fn new(key: RadarChunkKey, pixels: Vec<Color16>) -> Option<Self> {
        (pixels.len() == chunk_pixel_count()).then_some(Self { key, pixels })
    }

    #[must_use]
    pub const fn key(&self) -> RadarChunkKey {
        self.key
    }

    #[must_use]
    pub fn pixels(&self) -> &[Color16] {
        &self.pixels
    }
}

/// Reusable storage for a producer turn that builds differently-sized LODs.
#[derive(Debug, Default)]
pub struct RadarBuildScratch {
    pixels: Vec<Color16>,
}

/// Build any LOD directly from the authoritative map and colour table.
#[must_use]
pub fn build_chunk(map: &Map, colors: &RadarColors, key: RadarChunkKey) -> Option<RadarChunk> {
    build_chunk_reusing(map, colors, key, &mut RadarBuildScratch::default())
}

/// [`build_chunk`] with storage shared by all jobs in one producer turn.
#[must_use]
pub fn build_chunk_reusing(
    map: &Map,
    colors: &RadarColors,
    key: RadarChunkKey,
    scratch: &mut RadarBuildScratch,
) -> Option<RadarChunk> {
    let scale = 1_u32.checked_shl(u32::from(key.lod.value()))?;
    let side_u32 = u32::from(BASE_CHUNK_TILES).checked_mul(scale)?;
    let side = u16::try_from(side_u32).ok()?;
    let origin_x = key.chunk.x().checked_mul(side_u32)?;
    let origin_y = key.chunk.y().checked_mul(side_u32)?;
    let origin = (u16::try_from(origin_x).ok()?, u16::try_from(origin_y).ok()?);
    let len = usize::from(side).checked_mul(usize::from(side))?;
    scratch.pixels.resize(len, UNKNOWN);
    fill(map, colors, origin, side, side, &mut scratch.pixels[..len]);

    let mut source_side = usize::from(side);
    for _ in 0..key.lod.value() {
        let target_side = source_side / 2;
        for y in 0..target_side {
            for x in 0..target_side {
                let source = (y * 2) * source_side + x * 2;
                scratch.pixels[y * target_side + x] = reduce_lod_pixel([
                    scratch.pixels[source],
                    scratch.pixels[source + 1],
                    scratch.pixels[source + source_side],
                    scratch.pixels[source + source_side + 1],
                ]);
            }
        }
        source_side = target_side;
    }
    RadarChunk::new(key, scratch.pixels[..chunk_pixel_count()].to_vec())
}

/// The level-zero compatibility spelling used by invalidation tests/callers.
#[must_use]
pub fn build_base_chunk(map: &Map, colors: &RadarColors, key: RadarChunkKey) -> Option<RadarChunk> {
    key.lod.is_base().then(|| build_chunk(map, colors, key)).flatten()
}

/// Reduce four categorical colours to one categorical parent pixel.
///
/// The most frequent colour wins.  A tie is resolved by the first sample in
/// north-west, north-east, south-west, south-east order.  [`UNKNOWN`] is an
/// ordinary candidate, rather than transparency or black, so an unmapped area
/// remains visibly unmapped at every LOD.  This is intentionally not RGB
/// averaging: radar colours name terrain categories, not light values.
#[must_use]
pub fn reduce_lod_pixel(samples: [Color16; 4]) -> Color16 {
    let mut winner = 0;
    let mut winner_count = 0;
    for candidate in 0..samples.len() {
        let count = samples
            .iter()
            .filter(|&&colour| colour == samples[candidate])
            .count();
        if count > winner_count {
            winner = candidate;
            winner_count = count;
        }
    }
    samples[winner]
}

/// Build one LOD parent from its four complete children.
///
/// Children are ordered north-west, north-east, south-west, south-east.  Their
/// keys must be the direct children of `key`, on the same facet and revision.
/// Refusing a mismatched family prevents a cache from publishing mixed-source
/// terrain after an invalidation.
#[must_use]
pub fn build_lod_parent(key: RadarChunkKey, children: [&RadarChunk; 4]) -> Option<RadarChunk> {
    let child_lod = key.lod.child()?;
    let child_x = key.chunk.x().checked_mul(2)?;
    let child_y = key.chunk.y().checked_mul(2)?;
    let expected = [
        RadarChunkCoord::new(child_x, child_y),
        RadarChunkCoord::new(child_x.checked_add(1)?, child_y),
        RadarChunkCoord::new(child_x, child_y.checked_add(1)?),
        RadarChunkCoord::new(child_x.checked_add(1)?, child_y.checked_add(1)?),
    ];
    if children.iter().zip(expected).any(|(child, chunk)| {
        let child_key = child.key();
        child_key.facet != key.facet
            || child_key.lod != child_lod
            || child_key.chunk != chunk
            || child_key.revision != key.revision
    }) {
        return None;
    }

    let side = usize::from(BASE_CHUNK_TILES);
    let mut pixels = vec![UNKNOWN; chunk_pixel_count()];
    for y in 0..side {
        for x in 0..side {
            let index = |x: usize, y: usize| y * side + x;
            let sample = |x: usize, y: usize| {
                let child = (y / side) * 2 + x / side;
                children[child].pixels[index(x % side, y % side)]
            };
            let source_x = x * 2;
            let source_y = y * 2;
            pixels[index(x, y)] = reduce_lod_pixel([
                sample(source_x, source_y),
                sample(source_x + 1, source_y),
                sample(source_x, source_y + 1),
                sample(source_x + 1, source_y + 1),
            ]);
        }
    }
    RadarChunk::new(key, pixels)
}

const fn chunk_pixel_count() -> usize {
    (BASE_CHUNK_TILES as usize) * (BASE_CHUNK_TILES as usize)
}

/// The product one LOD coarser that covers this one.
///
/// A chunk's parent is its coordinates halved, which is why the ladder is a
/// shift rather than a lookup: every product at every level names exactly one
/// rectangle of world tiles, and the four that share a parent tile it exactly.
#[must_use]
pub fn parent_key(key: RadarChunkKey) -> Option<RadarChunkKey> {
    Some(RadarChunkKey::new(
        key.facet,
        key.lod.parent()?,
        RadarChunkCoord::new(key.chunk.x() / 2, key.chunk.y() / 2),
        key.revision,
    ))
}

/// The four direct children of a product, in the order [`build_lod_parent`]
/// requires: north-west, north-east, south-west, south-east.
///
/// `None` at LOD zero, which has no children — a base chunk is built from the
/// map rather than reduced from anything.
#[must_use]
pub fn child_keys(key: RadarChunkKey) -> Option<[RadarChunkKey; 4]> {
    let lod = key.lod.child()?;
    let x = key.chunk.x().checked_mul(2)?;
    let y = key.chunk.y().checked_mul(2)?;
    let child = |x, y| RadarChunkKey::new(key.facet, lod, RadarChunkCoord::new(x, y), key.revision);
    Some([
        child(x, y),
        child(x.checked_add(1)?, y),
        child(x, y.checked_add(1)?),
        child(x.checked_add(1)?, y.checked_add(1)?),
    ])
}

/// The level at which one chunk covers the facet's longer axis.
#[must_use]
pub fn max_lod(extent: RadarExtent) -> RadarLod {
    let longest = u32::from(extent.width().max(extent.height()));
    let chunks = longest.div_ceil(u32::from(BASE_CHUNK_TILES)).max(1);
    RadarLod::new((u32::BITS - (chunks - 1).leading_zeros()) as u8)
}

/// Producer cost in level-zero chunk units.
#[must_use]
pub fn lod_cost(lod: RadarLod) -> usize {
    1usize
        .checked_shl(u32::from(lod.value()) * 2)
        .unwrap_or(usize::MAX)
}

/// Build every ancestor that publishing one chunk has just completed, and
/// publish them too.  Returns how many were built.
///
/// The parent of a chunk is built when — and only when — its fourth child
/// lands, so this walks up until it meets a level whose family is incomplete.
/// That is what makes a coarse fallback exist without anybody scheduling one:
/// no ancestor is ever requested, and none is ever built from terrain that is
/// partly missing.
///
/// Work is bounded by the facet's [`max_lod`] and by the arithmetic: one reduction is four
/// complete children into one product of the same pixel count, and it happens
/// on one child in four.
pub fn build_ready_ancestors(
    cache: &mut RadarCache,
    key: RadarChunkKey,
    max_lod: impl Into<RadarLod>,
) -> usize {
    let max_lod = max_lod.into();
    let mut built = 0;
    let mut child = key;
    while child.lod < max_lod {
        let Some(parent) = parent_key(child) else {
            break;
        };
        // Scoped so the four borrows of the cache end before the parent —
        // which owns its own pixels — is handed back to it to publish.
        let Some(chunk) = ({
            let Some(family) = child_keys(parent) else {
                break;
            };
            let ready: Option<Vec<&RadarChunk>> = family.iter().map(|key| cache.get(*key)).collect();
            match ready {
                Some(ready) => build_lod_parent(parent, [ready[0], ready[1], ready[2], ready[3]]),
                None => None,
            }
        }) else {
            break;
        };
        if !cache.publish(chunk) {
            break;
        }
        built += 1;
        child = parent;
    }
    built
}

/// Bake one colour for every tile in a facet snapshot.
///
/// This is the building block for a radar cache, not a promise that a facet can
/// never change.  Its caller owns the snapshot's revision and uses it to build
/// or rebuild an invalidated LOD level.  Walking only changes which rectangle
/// is sampled; it must not rebuild this raster.  A live marker belongs in a
/// small overlay, not in the cached terrain pixels.
///
/// UO map coordinates and [`fill`] are `u16`; a map outside that format has no
/// representable radar image and returns an empty vector rather than wrapping
/// its dimensions into a different facet.
#[must_use]
pub fn bake(map: &Map, colors: &RadarColors) -> Vec<Color16> {
    let (Ok(width), Ok(height)) = (u16::try_from(map.width()), u16::try_from(map.height())) else {
        return Vec::new();
    };
    let Some(len) = usize::from(width).checked_mul(usize::from(height)) else {
        return Vec::new();
    };
    let mut pixels = vec![UNKNOWN; len];
    fill(map, colors, (0, 0), width, height, &mut pixels);
    pixels
}

/// The colour of one tile.
///
/// The whole rule in one place, so the block walk below and any caller asking
/// about a single tile cannot come to disagree. Off the map is [`UNKNOWN`].
#[must_use]
pub fn tile_color(map: &Map, colors: &RadarColors, x: u16, y: u16) -> Color16 {
    let Some(land) = map.land(x, y) else {
        return UNKNOWN;
    };
    let mut best = colors.land(land.tile);
    let mut best_z = land.z;
    for item in map.statics_at(x, y) {
        if item.z < best_z {
            continue;
        }
        let color = colors.statik(item.tile);
        if color == Color16::TRANSPARENT {
            continue;
        }
        best = color;
        best_z = item.z;
    }
    if best == Color16::TRANSPARENT {
        UNKNOWN
    } else {
        best
    }
}

/// Fill `into` with the colours of a `width` × `height` rectangle of tiles whose
/// north-west corner is `origin`.
///
/// Row-major, `width` per row, so a caller uploading it as a texture needs no
/// stride of its own. `into` must hold `width * height` colours; anything it
/// cannot reach is left alone, which is the safe half of a caller's arithmetic
/// error rather than a panic in a render path.
///
/// See the module header for why this walks blocks rather than tiles.
pub fn fill(
    map: &Map,
    colors: &RadarColors,
    origin: (u16, u16),
    width: u16,
    height: u16,
    into: &mut [Color16],
) {
    let (origin_x, origin_y) = origin;
    // The land, and the statics laid over it block by block. Two passes rather
    // than one because the land is a direct lookup and the statics are not:
    // interleaving them would put a binary search back in the inner loop, which
    // is the whole thing this avoids.
    //
    // `best_z` starts at the land's own height so a static below the ground is
    // skipped, and a floor *at* it is not — see the module header. It is filled
    // by the same walk as the colours: what a tile's land decides is one lookup
    // answering two questions, not two lookups of the same cell.
    let mut best_z = vec![i8::MIN; into.len().min(usize::from(width) * usize::from(height))];
    let last_column = origin_x.saturating_add(width.saturating_sub(1));
    for row in 0..height {
        // One row of land, each cell one step east of the last rather than a
        // block index derived per tile — see [`Map::land_in_row`]. It ends
        // where the facet does, so a column with no cell is a column off the
        // map, and `None` for the whole row is a row past the coordinate space
        // — off the map in the same sense, and [`UNKNOWN`] the same way.
        let mut cells = origin_y
            .checked_add(row)
            .map(|y| map.land_in_row(y, origin_x, last_column));
        for column in 0..width {
            let index = usize::from(row) * usize::from(width) + usize::from(column);
            let land = cells.as_mut().and_then(Iterator::next);
            if let (Some(land), Some(z)) = (land, best_z.get_mut(index)) {
                *z = land.z;
            }
            let Some(cell) = into.get_mut(index) else {
                continue;
            };
            *cell = match land {
                Some(land) => {
                    let color = colors.land(land.tile);
                    if color == Color16::TRANSPARENT {
                        UNKNOWN
                    } else {
                        color
                    }
                }
                None => UNKNOWN,
            };
        }
    }

    let last_x = origin_x.saturating_add(width.saturating_sub(1));
    let last_y = origin_y.saturating_add(height.saturating_sub(1));
    let (first_block, last_block) = (
        BlockCoord::containing(origin_x, origin_y),
        BlockCoord::containing(last_x, last_y),
    );
    for block_x in first_block.x..=last_block.x {
        for block_y in first_block.y..=last_block.y {
            for item in map.statics_in_block(block_x, block_y) {
                let (Some(column), Some(row)) = (item.x.checked_sub(origin_x), item.y.checked_sub(origin_y))
                else {
                    continue;
                };
                if column >= width || row >= height {
                    continue;
                }
                let index = usize::from(row) * usize::from(width) + usize::from(column);
                let (Some(cell), Some(z)) = (into.get_mut(index), best_z.get_mut(index)) else {
                    continue;
                };
                if item.z < *z {
                    continue;
                }
                let color = colors.statik(item.tile);
                if color == Color16::TRANSPARENT {
                    continue;
                }
                *cell = color;
                *z = item.z;
            }
        }
    }
}

/// The tiles a marker covers, relative to the one it stands on.
///
/// The centre and its four neighbours: a cross, and that is not decoration. At
/// one pixel a tile a single dot is a single pixel — indistinguishable from a
/// lamp post, and invisible against ground of a similar colour. Five pixels in
/// a shape nothing in `radarcol.mul` produces is the smallest thing a person
/// can actually find.
///
/// One constant because a marker is drawn two ways — stamped into a transient
/// bitmap by [`mark`], and recorded as overlay quads over cached terrain by the
/// radar pass — and a player who is a cross in one picture and a plus-sign in
/// the other would be two markers rather than one.
pub const MARKER_ARMS: [(i32, i32); 5] = [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)];

/// What the body this client is looking through draws as on the radar.
///
/// White, which `radarcol.mul` does contain: the shape is what makes the marker
/// findable ([`MARKER_ARMS`]), so the colour only has to be bright rather than
/// unique. A colour no terrain uses would still be one pixel wide.
pub const PLAYER_MARKER: Color16 = Color16(0x7FFF);

/// Stamp a marker over a filled buffer, at the tile `column`, `row` in from its
/// north-west corner.
///
/// A cross rather than a pixel — see [`MARKER_ARMS`] for why, and for the other
/// place that same shape is drawn.
///
/// This is a small bitmap utility for callers that own a transient image.  A
/// [`RadarChunk`] must never be passed here: player and waypoint markers are
/// overlays and do not change cached terrain products.  The arms clip at the
/// edges, so a marker on the first row keeps the pixels that are on the map
/// instead of wrapping to the last.
pub fn mark(into: &mut [Color16], width: u16, height: u16, at: (u16, u16), color: Color16) {
    let (column, row) = at;
    if column >= width || row >= height {
        return;
    }
    for (dx, dy) in MARKER_ARMS {
        let (Ok(x), Ok(y)) = (
            u16::try_from(i32::from(column) + dx),
            u16::try_from(i32::from(row) + dy),
        ) else {
            continue;
        };
        if x >= width || y >= height {
            continue;
        }
        if let Some(cell) = into.get_mut(usize::from(y) * usize::from(width) + usize::from(x)) {
            *cell = color;
        }
    }
}

/// The colour a static of this graphic draws as, or [`UNKNOWN`] when the table
/// has none. Named so a caller drawing a marker over the map uses the same
/// widening as the map itself.
#[must_use]
pub fn static_color(colors: &RadarColors, graphic: Graphic) -> Color16 {
    match colors.statik(graphic) {
        Color16::TRANSPARENT => UNKNOWN,
        color => color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_map::grid::BlockExtent;
    use openshard_map::map::{LandCell, LandTile, StaticItem};
    use openshard_protocol::wire::Hue;
    use openshard_uofiles::tiledata::LAND_TILE_COUNT;
    use std::collections::BTreeSet;

    /// Land id 1 is green, land id 2 is blue; static 1 is red, static 2 is
    /// white, static 3 has no colour at all.
    fn colors() -> RadarColors {
        let mut bytes = vec![0u8; LAND_TILE_COUNT * 2];
        bytes[2..4].copy_from_slice(&0x03E0u16.to_le_bytes()); // land 1: green
        bytes[4..6].copy_from_slice(&0x001Fu16.to_le_bytes()); // land 2: blue
        bytes.extend_from_slice(&0u16.to_le_bytes()); // static 0: absent
        bytes.extend_from_slice(&0x7C00u16.to_le_bytes()); // static 1: red
        bytes.extend_from_slice(&0x7FFFu16.to_le_bytes()); // static 2: white
        bytes.extend_from_slice(&0u16.to_le_bytes()); // static 3: absent
        RadarColors::parse(&bytes).expect("a whole table")
    }

    const GREEN: Color16 = Color16(0x03E0);
    const BLUE: Color16 = Color16(0x001F);
    const RED: Color16 = Color16(0x7C00);
    const WHITE: Color16 = Color16(0x7FFF);

    #[test]
    fn radar_space_keeps_tiles_extents_and_lods_distinct() {
        assert!(RadarExtent::new(0, 1).is_none());
        assert!(RadarExtent::new(1, 0).is_none());

        let extent = RadarExtent::new(64, 32).expect("a non-empty rectangle");
        let region = RadarRegion::new(Facet(0), RadarTile::new(80, 40).saturating_sub(extent), extent);
        assert_eq!(region.origin(), RadarTile::new(48, 24));
        assert_eq!(RadarLod::BASE.parent(), Some(RadarLod::new(1)));
        assert_eq!(RadarLod::BASE.child(), None);
    }

    /// A one-block facet, every tile land id 1 at z 0.
    fn a_field() -> Map {
        Map::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| LandCell {
            tile: LandTile(1),
            z: 0,
        })
    }

    fn put(map: &mut Map, graphic: u16, x: u16, y: u16, z: i8) {
        map.place_static(StaticItem {
            tile: Graphic(graphic),
            x,
            y,
            z,
            hue: Hue(0),
        });
    }

    #[test]
    fn bake_is_the_whole_facet_once_in_row_major_order() {
        let mut map = a_field();
        put(&mut map, 1, 7, 7, 1);

        let baked = bake(&map, &colors());

        assert_eq!(baked.len(), 64);
        assert_eq!(baked[0], GREEN);
        assert_eq!(baked[63], RED);
    }

    #[test]
    fn base_chunks_near_a_centre_visit_the_closest_first_not_in_raster_order() {
        let region = RadarRegion::new(
            Facet(0),
            RadarTile::new(0, 0),
            RadarExtent::new(BASE_CHUNK_TILES * 3, BASE_CHUNK_TILES * 3).unwrap(),
        );
        let centre = RadarChunkCoord::new(2, 2);

        let ordered: Vec<_> = region_base_chunks_near(region, centre).collect();

        assert_eq!(ordered[0], centre, "a chunk is its own nearest chunk");
        assert_ne!(
            ordered[0],
            RadarChunkCoord::new(0, 0),
            "raster order would visit the north-west corner first; distance order must not"
        );
        let mut raster: Vec<_> = region_base_chunks(region).collect();
        let mut near = ordered;
        raster.sort_by_key(|chunk| (chunk.y(), chunk.x()));
        near.sort_by_key(|chunk| (chunk.y(), chunk.x()));
        assert_eq!(
            raster, near,
            "distance order is a permutation of the raster walk, not a different set"
        );
    }

    #[test]
    fn world_tiles_use_one_block_aligned_base_chunk_conversion() {
        assert_eq!(BASE_CHUNK_BLOCKS, 8);
        assert_eq!(
            world_tile_to_base_chunk((0, 0)),
            (RadarChunkCoord::new(0, 0), RadarChunkLocalTile::new(0, 0))
        );
        assert_eq!(
            world_tile_to_base_chunk((u32::from(BASE_CHUNK_TILES) - 1, 63)),
            (RadarChunkCoord::new(0, 0), RadarChunkLocalTile::new(63, 63))
        );
        assert_eq!(
            world_tile_to_base_chunk((u32::from(BASE_CHUNK_TILES), 64)),
            (RadarChunkCoord::new(1, 1), RadarChunkLocalTile::new(0, 0))
        );
    }

    #[test]
    fn a_base_chunk_is_block_aligned_and_keeps_the_map_edge_complete() {
        let mut map = a_field();
        put(&mut map, 1, 7, 7, 1);
        let key = RadarChunkKey::new(Facet(0), 0, RadarChunkCoord::new(0, 0), RadarRevision(7));

        let chunk = build_base_chunk(&map, &colors(), key).expect("a level-zero key");
        let mut reference = vec![UNKNOWN; chunk_pixel_count()];
        fill(
            &map,
            &colors(),
            (0, 0),
            BASE_CHUNK_TILES,
            BASE_CHUNK_TILES,
            &mut reference,
        );

        assert_eq!(chunk.key(), key);
        assert_eq!(
            chunk.pixels(),
            reference,
            "the chunk builder is the rectangle walk"
        );
        assert_eq!(chunk.pixels().len(), usize::from(BASE_CHUNK_TILES).pow(2));
        assert_eq!(chunk.pixels()[7 * usize::from(BASE_CHUNK_TILES) + 7], RED);
        assert_eq!(chunk.pixels()[8], UNKNOWN, "the first tile beyond the east edge");
        assert_eq!(
            chunk.pixels()[usize::from(BASE_CHUNK_TILES) * 8],
            UNKNOWN,
            "the first tile beyond the south edge"
        );
    }

    #[test]
    fn a_direct_lod_build_is_identical_to_climbing_complete_child_families() {
        let map = a_field();
        let colors = colors();
        for target_lod in 0..=3_u8 {
            let revision = RadarRevision(9);
            let direct_key = RadarChunkKey::new(
                Facet(0),
                RadarLod::new(target_lod),
                RadarChunkCoord::new(0, 0),
                revision,
            );
            let direct = build_chunk(&map, &colors, direct_key).expect("the direct product is addressable");
            let side = 1_u32 << target_lod;
            let mut level: BTreeMap<RadarChunkCoord, RadarChunk> = (0..side)
                .flat_map(|y| (0..side).map(move |x| RadarChunkCoord::new(x, y)))
                .map(|coord| {
                    let key = RadarChunkKey::new(Facet(0), RadarLod::BASE, coord, revision);
                    (coord, build_chunk(&map, &colors, key).expect("a base child"))
                })
                .collect();
            for lod in 1..=target_lod {
                let parent_side = 1_u32 << (target_lod - lod);
                let mut parents = BTreeMap::new();
                for y in 0..parent_side {
                    for x in 0..parent_side {
                        let child = |dx, dy| &level[&RadarChunkCoord::new(x * 2 + dx, y * 2 + dy)];
                        let key = RadarChunkKey::new(
                            Facet(0),
                            RadarLod::new(lod),
                            RadarChunkCoord::new(x, y),
                            revision,
                        );
                        parents.insert(
                            RadarChunkCoord::new(x, y),
                            build_lod_parent(key, [child(0, 0), child(1, 0), child(0, 1), child(1, 1)])
                                .expect("a complete family"),
                        );
                    }
                }
                level = parents;
            }
            assert_eq!(direct, level.remove(&RadarChunkCoord::new(0, 0)).unwrap());
        }
    }

    #[test]
    fn britannias_extent_owns_a_seven_level_ladder() {
        let extent = RadarExtent::new(7168, 4096).unwrap();
        assert_eq!(max_lod(extent), RadarLod::new(7));
        assert_eq!(max_lod(RadarExtent::new(64, 64).unwrap()), RadarLod::BASE);
    }

    fn test_view(tiles_per_pixel: f32) -> RadarView {
        RadarView::new(
            Facet(0),
            RadarTile::new(32_500, 32_500),
            RadarExtent::new(65_000, 65_000).unwrap(),
            tiles_per_pixel,
            Placement {
                origin: (0.0, 0.0),
                extent: (640.0, 480.0),
                circle: false,
                rotation: 0.0,
            },
            1.0,
        )
    }

    #[test]
    fn view_chunk_demand_is_bounded_by_pixels_not_zoom() {
        let fine = test_view(1.0);
        let coarse = test_view(64.0);
        assert_eq!(fine.lod(), RadarLod::BASE);
        assert_eq!(coarse.lod(), RadarLod::new(6));
        assert_eq!(
            region_chunks(fine.region(), fine.lod()).count(),
            region_chunks(coarse.region(), coarse.lod()).count(),
        );
    }

    /// R3's own acceptance test. Defect 3.3 was one region standing for both
    /// windows: opening the facet map moved that single region to the whole
    /// world, and the minimap — still drawing a circle around the player —
    /// was left asking for chunks nothing requested. Two views ask for two
    /// regions at two levels, and neither can spend the other's slots.
    #[test]
    fn an_open_facet_map_adds_its_own_demand_and_takes_none_of_the_minimaps() {
        let extent = RadarExtent::new(7168, 4096).expect("Britannia");
        let placement = |width: f32, height: f32, circle: bool| Placement {
            origin: (0.0, 0.0),
            extent: (width, height),
            circle,
            rotation: 0.0,
        };
        // The minimap, drawn small and unzoomed around a player in Britain.
        let minimap = RadarView::new(
            Facet(0),
            RadarTile::new(1400, 1600),
            extent,
            1.0,
            placement(256.0, 256.0, true),
            1.0,
        );
        // The facet map, drawn wide and fully zoomed out around the middle of
        // the world — sixteen tiles to a pixel, so level four.
        let facet_map = RadarView::new(
            Facet(0),
            RadarTile::new(3584, 2048),
            extent,
            16.0,
            placement(592.0, 418.0, false),
            1.0,
        );
        assert_eq!(minimap.lod(), RadarLod::BASE);
        assert_eq!(facet_map.lod(), RadarLod::new(4));

        let cache = RadarCache::default();
        let mut alone = RadarWorkQueue::default();
        let by_itself = request_views([(minimap, minimap.lod())], &cache, &mut alone);

        let mut together = RadarWorkQueue::default();
        // The facet map first, which is the order that starves the minimap if
        // anything at all is shared between the two.
        let both = request_views(
            [(facet_map, facet_map.lod()), (minimap, minimap.lod())],
            &cache,
            &mut together,
        );

        assert!(!by_itself.is_empty());
        for key in &by_itself {
            assert!(
                both.contains(key),
                "the minimap keeps {key:?} with the facet map open"
            );
        }
        assert!(
            both.len() > by_itself.len(),
            "the facet map adds a demand of its own"
        );
        let distinct: BTreeSet<_> = both.iter().collect();
        assert_eq!(
            together.pending_len(),
            distinct.len(),
            "every key either window named reached the queue",
        );
    }

    #[test]
    fn radar_lod_has_a_ten_percent_dead_band() {
        let mut selector = RadarLodSelector::default();
        assert_eq!(selector.update(test_view(1.9)), RadarLod::BASE);
        assert_eq!(selector.update(test_view(2.05)), RadarLod::BASE);
        assert_eq!(selector.update(test_view(2.21)), RadarLod::new(1));
        assert_eq!(selector.update(test_view(1.85)), RadarLod::new(1));
        assert_eq!(selector.update(test_view(1.79)), RadarLod::BASE);
    }

    /// A selector is per window and follows that window across facets. The
    /// level it remembers was chosen against the ladder of the facet it was
    /// looking at, and a smaller facet has a shorter one — a remembered level
    /// above it names a grid that does not exist.
    #[test]
    fn a_remembered_level_is_clamped_to_the_facet_the_window_moved_to() {
        let facet_view = |extent: RadarExtent, tiles_per_pixel: f32| {
            RadarView::new(
                Facet(0),
                RadarTile::new(0, 0),
                extent,
                tiles_per_pixel,
                Placement {
                    origin: (0.0, 0.0),
                    extent: (640.0, 480.0),
                    circle: false,
                    rotation: 0.0,
                },
                1.0,
            )
        };
        let britannia = RadarExtent::new(7168, 4096).expect("Britannia");
        let small = RadarExtent::new(BASE_CHUNK_TILES * 2, BASE_CHUNK_TILES * 2).expect("a two-chunk facet");
        assert_eq!(max_lod(britannia), RadarLod::new(7));
        assert_eq!(max_lod(small), RadarLod::new(1));

        let mut selector = RadarLodSelector::default();
        assert_eq!(selector.update(facet_view(britannia, 32.0)), RadarLod::new(5));
        // The same window, now looking at a facet whose ladder ends at one.
        // Without the clamp the dead band's upward loop never runs (five is
        // already above the maximum) and its downward loop stops where the
        // zoom says, so level five would be returned for a two-chunk world.
        assert_eq!(selector.update(facet_view(small, 32.0)), RadarLod::new(1));
    }

    #[test]
    fn cpu_budget_evicts_the_unpinned_tail_and_keeps_the_sweep_floor() {
        let facet = Facet(0);
        let mut cache = RadarCache::with_tail_budget(RADAR_CHUNK_CPU_BYTES * 2).unwrap();
        for x in 0..4 {
            let key = cache.key(facet, RadarLod::BASE, RadarChunkCoord::new(x, 0));
            assert!(cache.publish(RadarChunk::new(key, vec![GREEN; chunk_pixel_count()]).unwrap()));
        }
        let coarse = cache.key(facet, SWEEP_LOD, RadarChunkCoord::new(0, 0));
        assert!(cache.publish(RadarChunk::new(coarse, vec![BLUE; chunk_pixel_count()]).unwrap()));
        assert_eq!(cache.evict_to_budget([]), 2);
        assert!(cache.get(coarse).is_some(), "the sweep floor is pinned");
        assert_eq!(cache.counters().evicted, 2);
    }

    #[test]
    fn sweep_is_once_per_facet_and_never_outranks_view_work() {
        let facet = Facet(0);
        let extent = RadarExtent::new(7168, 4096).expect("Britannia");
        let mut cache = RadarCache::default();
        assert!(cache.begin_sweep(facet, extent));
        assert!(!cache.begin_sweep(facet, extent));
        assert!(cache.sweep_started(facet));
        // Levels two through seven of the shipped facet: 448 + 112 + 28 + 8 +
        // 2 + 1.
        assert_eq!(cache.sweep_owed_len(facet), 599);

        let mut queue = RadarWorkQueue::new(8, 1).unwrap();
        let visible = cache.key(facet, RadarLod::BASE, RadarChunkCoord::new(20, 20));
        assert!(queue.request(visible));
        assert_eq!(
            cache.drain_sweep(facet, |key| queue.request_sweep(key)),
            599,
            "the floor is owed until its products land, not until it was asked for",
        );
        assert_eq!(
            queue.take_for_producer_near(RadarChunkCoord::new(0, 0)),
            vec![visible],
            "an open window's own terrain still outranks the whole floor",
        );
    }

    /// The defect the flag had: `request_sweep` refuses at the queue's bound,
    /// a refused key was never offered again, and the flag already said the
    /// sweep had run. Today's arithmetic hides it — 599 keys against a bound
    /// of 1024 — so this drives the sweep through a queue far too small for
    /// it and asks for the floor afterwards.
    #[test]
    fn a_sweep_through_a_queue_it_does_not_fit_in_still_builds_the_whole_floor() {
        let facet = Facet(0);
        let extent = RadarExtent::new(7168, 4096).expect("Britannia");
        let mut cache = RadarCache::default();
        assert!(cache.begin_sweep(facet, extent));

        // Eight slots for 599 chunks, one unit a turn: every frame refuses
        // most of what it is offered.
        let mut queue = RadarWorkQueue::new(8, 1).unwrap();
        let mut turns = 0;
        // One frame a turn: offer what is still owed, build what the queue
        // had room for. A frame that is owed nothing is the sweep finished.
        while cache.drain_sweep(facet, |key| queue.request_sweep(key)) != 0 {
            let batch = queue.take_for_producer_near(RadarChunkCoord::new(0, 0));
            assert!(!batch.is_empty(), "a turn always hands out at least one job");
            for key in batch {
                let chunk = RadarChunk::new(key, vec![GREEN; chunk_pixel_count()]).expect("a complete chunk");
                assert!(queue.finish(&mut cache, chunk));
            }
            turns += 1;
            assert!(turns < 5_000, "the sweep drains rather than spinning");
        }

        let whole_facet = RadarRegion::new(facet, RadarTile::new(0, 0), extent);
        for lod in SWEEP_LOD.value()..=max_lod(extent).value() {
            let lod = RadarLod::new(lod);
            for chunk in region_chunks(whole_facet, lod) {
                assert!(
                    cache.get(cache.key(facet, lod, chunk)).is_some(),
                    "level {} chunk {chunk:?} is part of the floor",
                    lod.value(),
                );
            }
        }
    }

    #[test]
    fn a_lod_parent_reduces_its_four_children_without_blending_colours() {
        let revision = RadarRevision(11);
        let child = |x, y, colour| {
            RadarChunk::new(
                RadarChunkKey::new(Facet(0), 0, RadarChunkCoord::new(x, y), revision),
                vec![colour; chunk_pixel_count()],
            )
            .expect("a complete child")
        };
        let northwest = child(0, 0, RED);
        let northeast = child(1, 0, WHITE);
        let southwest = child(0, 1, UNKNOWN);
        let southeast = child(1, 1, BLUE);
        let key = RadarChunkKey::new(Facet(0), 1, RadarChunkCoord::new(0, 0), revision);

        let parent = build_lod_parent(key, [&northwest, &northeast, &southwest, &southeast])
            .expect("four direct children");
        let side = usize::from(BASE_CHUNK_TILES);

        assert_eq!(parent.pixels()[0], RED, "ties prefer the north-west sample");
        assert_eq!(
            parent.pixels()[side - 1],
            WHITE,
            "the north-east child is sampled"
        );
        assert_eq!(parent.pixels()[(side - 1) * side], UNKNOWN, "unknown is retained");
        assert_eq!(
            parent.pixels()[side * side - 1],
            BLUE,
            "the south-east child is sampled"
        );
        assert_eq!(reduce_lod_pixel([RED, WHITE, RED, WHITE]), RED);
    }

    #[test]
    fn an_ancestor_is_built_by_the_child_that_completes_its_family() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        let child = |cache: &RadarCache, x, y| {
            RadarChunk::new(
                cache.key(facet, 0, RadarChunkCoord::new(x, y)),
                vec![RED; chunk_pixel_count()],
            )
            .expect("a complete child")
        };
        let parent = cache.key(facet, 1, RadarChunkCoord::new(0, 0));
        let grandparent = cache.key(facet, 2, RadarChunkCoord::new(0, 0));

        // Three of the four: nothing above them can be built from a family with
        // a hole in it, and a reduction over one is not a coarser picture of
        // the ground — it is a picture of three quarters of it.
        for (x, y) in [(0, 0), (1, 0), (0, 1)] {
            let key = child(&cache, x, y).key();
            assert!(cache.publish(child(&cache, x, y)));
            assert_eq!(build_ready_ancestors(&mut cache, key, RadarLod::new(4)), 0);
        }
        assert!(cache.get(parent).is_none());

        let last = child(&cache, 1, 1);
        let key = last.key();
        assert!(cache.publish(last));
        assert_eq!(
            build_ready_ancestors(&mut cache, key, RadarLod::new(4)),
            1,
            "the fourth child completes exactly one level — the level above it \
             still has three quarters missing"
        );
        assert!(cache.get(parent).is_some());
        assert!(cache.get(grandparent).is_none());
        assert_eq!(
            cache.get(parent).expect("just built").pixels()[0],
            RED,
            "four red children reduce to red, not to an average of one colour",
        );
    }

    #[test]
    fn the_ladder_is_climbed_no_further_than_it_was_asked_for() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let chunk = RadarChunk::new(
                cache.key(facet, 0, RadarChunkCoord::new(x, y)),
                vec![RED; chunk_pixel_count()],
            )
            .expect("a complete child");
            assert!(cache.publish(chunk));
        }
        let last = cache.key(facet, 0, RadarChunkCoord::new(1, 1));
        assert_eq!(
            build_ready_ancestors(&mut cache, last, 0),
            0,
            "a ladder of no levels builds nothing"
        );
        assert!(
            cache
                .get(cache.key(facet, 1, RadarChunkCoord::new(0, 0)))
                .is_none()
        );
    }

    #[test]
    fn an_abandoned_job_returns_its_slot_instead_of_losing_it() {
        let facet = Facet(0);
        let cache = RadarCache::default();
        // One slot, so a lost one is the difference between a queue that works
        // and a queue that never accepts another request.
        let mut queue = RadarWorkQueue::new(1, 1).expect("non-zero limits");
        let key = cache.key(facet, 0, RadarChunkCoord::new(0, 0));
        assert!(queue.request(key));
        assert_eq!(queue.take_for_producer(), vec![key]);
        assert!(!queue.request(cache.key(facet, 0, RadarChunkCoord::new(1, 0))));

        assert!(queue.abandon(key));
        assert!(!queue.abandon(key), "a job is released once");
        assert!(queue.request(cache.key(facet, 0, RadarChunkCoord::new(1, 0))));
    }

    #[test]
    fn lod_reduction_keeps_unknown_and_resolves_all_ties_in_sample_order() {
        assert_eq!(
            reduce_lod_pixel([UNKNOWN, RED, UNKNOWN, WHITE]),
            UNKNOWN,
            "unknown is a categorical map colour, not transparent"
        );
        assert_eq!(
            reduce_lod_pixel([BLUE, RED, WHITE, UNKNOWN]),
            BLUE,
            "four-way ties retain the north-west sample"
        );
    }

    #[test]
    fn cache_keys_and_publication_follow_the_content_revision_not_a_window() {
        let facet = Facet(0);
        let coord = RadarChunkCoord::new(3, 4);
        let mut cache = RadarCache::default();
        let first_key = cache.key(facet, 0, coord);
        let first = RadarChunk::new(first_key, vec![GREEN; chunk_pixel_count()]).expect("a complete chunk");

        assert!(cache.publish(first));
        assert!(cache.get(first_key).is_some());
        assert_eq!(cache.retained_len(), 1);

        assert!(cache.set_revision(facet, RadarRevision(1)));
        let current_key = cache.key(facet, 0, coord);
        assert_ne!(current_key, first_key);
        assert!(cache.get(first_key).is_none(), "old source is not ready");
        assert!(cache.get(current_key).is_none(), "no new product was built yet");

        let late_old =
            RadarChunk::new(first_key, vec![RED; chunk_pixel_count()]).expect("a complete old chunk");
        assert!(!cache.publish(late_old), "an edit rejects a late worker result");
        assert_eq!(cache.retained_len(), 1, "the stale result was not published");
        assert!(
            !cache.set_revision(facet, RadarRevision(0)),
            "a delayed source notification cannot revive the old product"
        );
    }

    #[test]
    fn ready_selection_prefers_exact_then_nearest_current_ancestor() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        let requested = cache.key(facet, 0, RadarChunkCoord::new(7, 5));
        let ancestor = cache.key(facet, 2, RadarChunkCoord::new(1, 1));
        let ancestor_chunk =
            RadarChunk::new(ancestor, vec![WHITE; chunk_pixel_count()]).expect("a complete ancestor");
        assert!(cache.publish(ancestor_chunk));

        let fallback = cache
            .select_ready(requested)
            .expect("the ancestor covers the request");
        assert_eq!(fallback.kind(), RadarReadyKind::CoarserAncestor);
        assert_eq!(fallback.chunk().key(), ancestor);

        let exact_chunk =
            RadarChunk::new(requested, vec![GREEN; chunk_pixel_count()]).expect("a complete exact chunk");
        assert!(cache.publish(exact_chunk));
        let exact = cache.select_ready(requested).expect("the exact product is ready");
        assert_eq!(exact.kind(), RadarReadyKind::Exact);
        assert_eq!(exact.chunk().key(), requested);
    }

    #[test]
    fn ready_selection_uses_the_newest_exact_stale_product_when_current_is_missing() {
        let facet = Facet(0);
        let coord = RadarChunkCoord::new(2, 3);
        let mut cache = RadarCache::default();
        let old = cache.key(facet, 1, coord);
        assert!(
            cache.publish(
                RadarChunk::new(old, vec![RED; chunk_pixel_count()]).expect("a complete old product")
            )
        );
        assert!(cache.set_revision(facet, RadarRevision(1)));
        let current = cache.key(facet, 1, coord);

        let fallback = cache
            .select_ready(current)
            .expect("the old complete raster remains safe");
        assert_eq!(fallback.kind(), RadarReadyKind::StaleExact);
        assert_eq!(fallback.chunk().key(), old);

        let counters = cache.counters();
        assert_eq!(counters.requested, 1);
        assert_eq!(counters.ready, 0);
        assert_eq!(counters.stale, 1);
        assert_eq!(counters.rebuilt, 1);
        assert_eq!(counters.evicted, 0, "eviction is not implemented yet");
    }

    /// **The four ways a frame's demand can be answered, in one walk.**
    ///
    /// R7's "chunks requested/ready/fallen-back" is not one number: a chunk
    /// drawn from a coarse ancestor is the ladder working, a chunk drawn from
    /// a superseded revision is terrain that has since changed, and a chunk
    /// with nothing ready is backdrop. All three look alike on screen, which
    /// is why they are counted apart here.
    #[test]
    fn resolved_demand_partitions_the_request_by_how_the_cache_answered() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        // Stale first: a product under the old revision, and nothing else for
        // its chunk afterwards. The bump is per facet, so everything else in
        // this test is published *after* it.
        let stale_coord = RadarChunkCoord::new(40, 40);
        let old = cache.key(facet, 0, stale_coord);
        assert!(
            cache.publish(RadarChunk::new(old, vec![RED; chunk_pixel_count()]).expect("a complete product"))
        );
        assert!(cache.set_revision(facet, RadarRevision(1)));
        let stale = cache.key(facet, 0, stale_coord);
        // Exact: the requested key itself, at the current revision.
        let exact = cache.key(facet, 0, RadarChunkCoord::new(0, 0));
        assert!(
            cache.publish(
                RadarChunk::new(exact, vec![GREEN; chunk_pixel_count()]).expect("a complete product")
            )
        );
        // Coarser: only a level-2 parent of (7, 5) is ready. The stale ladder
        // is exact-key only, which is why this arm needs a *current* ancestor
        // and a stale one would count as missing instead.
        let ancestor = cache.key(facet, 2, RadarChunkCoord::new(1, 1));
        assert!(cache.publish(
            RadarChunk::new(ancestor, vec![WHITE; chunk_pixel_count()]).expect("a complete ancestor")
        ));
        let coarser = cache.key(facet, 0, RadarChunkCoord::new(7, 5));
        // Missing: never asked for, never built.
        let missing = cache.key(facet, 0, RadarChunkCoord::new(99, 99));

        let resolved = resolve_demand(&cache, [exact, coarser, stale, missing]);
        assert_eq!(
            resolved.demand,
            RadarDemand {
                exact: 1,
                coarser: 1,
                stale: 1,
                missing: 1,
            }
        );
        assert_eq!(resolved.demand.total(), 4, "the four arms partition the request");
        assert_eq!(
            resolved.drawn.len(),
            3,
            "only an answered request names a chunk eviction must keep"
        );
        assert!(!resolved.drawn.contains(&missing));
    }

    /// The same walk against a cache nothing has invalidated, which is the
    /// ordinary frame: exact where a product landed, coarser where only a
    /// parent has, missing where neither.
    #[test]
    fn resolved_demand_reports_the_ladder_on_an_undisturbed_cache() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        let exact = cache.key(facet, 0, RadarChunkCoord::new(0, 0));
        assert!(
            cache.publish(
                RadarChunk::new(exact, vec![GREEN; chunk_pixel_count()]).expect("a complete product")
            )
        );
        let ancestor = cache.key(facet, 2, RadarChunkCoord::new(1, 1));
        assert!(cache.publish(
            RadarChunk::new(ancestor, vec![WHITE; chunk_pixel_count()]).expect("a complete ancestor")
        ));
        let coarser = cache.key(facet, 0, RadarChunkCoord::new(7, 5));
        let missing = cache.key(facet, 0, RadarChunkCoord::new(99, 99));

        let resolved = resolve_demand(&cache, [exact, coarser, missing]);
        assert_eq!(
            resolved.demand,
            RadarDemand {
                exact: 1,
                coarser: 1,
                stale: 0,
                missing: 1,
            }
        );
        assert_eq!(
            resolved.drawn,
            vec![exact, ancestor],
            "the coarse request draws from the parent, not from itself"
        );
    }

    /// **The CPU budget's headroom is two numbers, and both are reported.**
    ///
    /// `retained_bytes` alone cannot be read: this cache evicts only above
    /// `tail_budget`, and pinned chunks raise the ceiling above that, so a
    /// reader with one number and a constant in another file would conclude
    /// the wrong thing about a cache sitting over its tail.
    #[test]
    fn cache_counters_report_the_weight_and_the_budget_it_is_measured_against() {
        let facet = Facet(0);
        let mut cache = RadarCache::with_tail_budget(RADAR_CHUNK_CPU_BYTES * 2).expect("a budget");
        assert_eq!(cache.counters().retained_bytes, 0);
        assert_eq!(cache.counters().tail_budget, RADAR_CHUNK_CPU_BYTES * 2);
        for x in 0..3 {
            let key = cache.key(facet, 0, RadarChunkCoord::new(x, 0));
            assert!(cache.publish(
                RadarChunk::new(key, vec![GREEN; chunk_pixel_count()]).expect("a complete product")
            ));
        }
        assert_eq!(
            cache.counters().retained_bytes,
            RADAR_CHUNK_CPU_BYTES * 3,
            "every retained product weighs, whether or not it is pinned"
        );
        // Nothing protected and nothing at or above `SWEEP_LOD`, so the walk
        // is free to take the cache back down to its tail.
        assert_eq!(cache.evict_to_budget([]), 1);
        assert_eq!(cache.counters().retained_bytes, RADAR_CHUNK_CPU_BYTES * 2);
        assert_eq!(cache.counters().evicted, 1);
    }

    /// The queue reports the bound it refuses at, because the two lengths
    /// beside it are meaningless without it.
    #[test]
    fn queue_counters_report_the_bound_they_are_measured_against() {
        let queue = RadarWorkQueue::default();
        let counters = queue.counters();
        assert_eq!(counters.max_queued, queue.max_queued());
        assert_eq!(counters.queued + counters.in_flight, 0);
        assert_eq!(
            RadarWorkQueue::new(7, 1)
                .expect("non-zero limits")
                .counters()
                .max_queued,
            7
        );
    }

    #[test]
    fn indexed_ready_selection_matches_the_exhaustive_oracle() {
        let mut cache = RadarCache::default();
        for facet in 0..2 {
            cache.revisions.insert(Facet(facet), RadarRevision(7));
            for index in 0..500_u32 {
                let lod = RadarLod::new((index % 8) as u8);
                let coord = RadarChunkCoord::new(index % 31, (index / 31) % 19);
                let revision = RadarRevision(u64::from(index % 8));
                let key = RadarChunkKey::new(Facet(facet), lod, coord, revision);
                cache.ready.insert(
                    key,
                    RadarChunk::new(key, vec![Color16(index as u16); chunk_pixel_count()])
                        .expect("a complete generated product"),
                );
            }
        }
        cache.rebuild_highest_ready_lods();

        for facet in 0..2 {
            for lod in 0..8 {
                for x in 0..35 {
                    let key = RadarChunkKey::new(
                        Facet(facet),
                        RadarLod::new(lod),
                        RadarChunkCoord::new(x, (x * 7) % 23),
                        RadarRevision(7),
                    );
                    let fast = cache
                        .select_ready(key)
                        .map(|ready| (ready.chunk().key(), ready.kind()));
                    let reference = cache
                        .select_ready_reference(key)
                        .map(|ready| (ready.chunk().key(), ready.kind()));
                    assert_eq!(fast, reference, "selection differs for {key:?}");
                }
            }
        }
    }

    #[test]
    fn a_tile_mutation_marks_only_its_base_chunk_and_lod_ancestors_dirty() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        let tile = (
            u32::from(BASE_CHUNK_TILES) * 6 + 9,
            u32::from(BASE_CHUNK_TILES) * 5 + 12,
        );

        let revision = cache
            .invalidate_tile(facet, tile, 3)
            .expect("the first revision is representable");
        assert_eq!(revision, RadarRevision(1));
        assert_eq!(
            cache.dirty_keys(),
            vec![
                cache.key(facet, 0, RadarChunkCoord::new(6, 5)),
                cache.key(facet, 1, RadarChunkCoord::new(3, 2)),
                cache.key(facet, 2, RadarChunkCoord::new(1, 1)),
                cache.key(facet, 3, RadarChunkCoord::new(0, 0)),
            ],
            "one tile has one base product and one ancestor at each LOD"
        );

        let base = cache.key(facet, 0, RadarChunkCoord::new(6, 5));
        let other = cache.key(facet, 0, RadarChunkCoord::new(7, 5));
        assert!(cache.is_dirty(base));
        assert!(!cache.is_dirty(other), "an adjacent base chunk is unaffected");

        let complete = RadarChunk::new(base, vec![GREEN; chunk_pixel_count()]).expect("a complete chunk");
        assert!(cache.publish(complete));
        assert!(!cache.is_dirty(base), "publication settles only that product");
        assert!(cache.is_dirty(cache.key(facet, 1, RadarChunkCoord::new(3, 2))));
    }

    #[test]
    fn region_base_chunks_covers_every_chunk_a_rectangle_touches() {
        let aligned = RadarRegion::new(
            Facet(0),
            RadarTile::new(0, 0),
            RadarExtent::new(BASE_CHUNK_TILES, BASE_CHUNK_TILES).unwrap(),
        );
        assert_eq!(
            region_base_chunks(aligned).collect::<Vec<_>>(),
            vec![RadarChunkCoord::new(0, 0)],
            "one chunk exactly fills one chunk-aligned region"
        );

        let straddling = RadarRegion::new(
            Facet(0),
            RadarTile::new(32, 32),
            RadarExtent::new(BASE_CHUNK_TILES, BASE_CHUNK_TILES).unwrap(),
        );
        assert_eq!(
            region_base_chunks(straddling).collect::<Vec<_>>(),
            vec![
                RadarChunkCoord::new(0, 0),
                RadarChunkCoord::new(1, 0),
                RadarChunkCoord::new(0, 1),
                RadarChunkCoord::new(1, 1),
            ],
            "a region straddling all four neighbours touches all four"
        );
    }

    #[test]
    fn a_new_mutation_supersedes_unfinished_dirty_work_for_its_facet() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        let first = cache.invalidate_tile(facet, (0, 0), 1).expect("a first revision");
        let second = cache
            .invalidate_tile(facet, (u32::from(BASE_CHUNK_TILES), 0), 1)
            .expect("a second revision");

        assert_eq!(first, RadarRevision(1));
        assert_eq!(second, RadarRevision(2));
        assert_eq!(
            cache.dirty_keys(),
            vec![
                cache.key(facet, 0, RadarChunkCoord::new(1, 0)),
                cache.key(facet, 1, RadarChunkCoord::new(0, 0)),
            ],
            "a worker can never be asked to build an obsolete source revision"
        );
    }

    #[test]
    fn a_bare_tile_is_its_land() {
        let map = Map::from_blocks(BlockExtent { wide: 1, down: 1 }, |x, _| LandCell {
            tile: LandTile(if x < 4 { 1 } else { 2 }),
            z: 0,
        });
        let colors = colors();

        assert_eq!(tile_color(&map, &colors, 3, 3), GREEN);
        assert_eq!(
            tile_color(&map, &colors, 5, 3),
            BLUE,
            "a different land tile is a different colour, so the land id is the key"
        );
    }

    /// Off the map is unmapped, not transparent — a hole in the window is worse
    /// than a dark tile.
    #[test]
    fn off_the_map_is_unknown() {
        let map = a_field();
        assert_eq!(tile_color(&map, &colors(), 99, 99), UNKNOWN);
    }

    /// **The comparison is `>=`, not `>`.** A floor lies at the ground's own
    /// height, and `>` would draw grass through it.
    #[test]
    fn a_static_at_the_grounds_own_height_covers_it() {
        let mut map = a_field();
        put(&mut map, 1, 2, 2, 0);
        assert_eq!(tile_color(&map, &colors(), 2, 2), RED);
    }

    /// And one below the ground does not.
    #[test]
    fn a_static_under_the_ground_does_not_cover_it() {
        let mut map = a_field();
        put(&mut map, 1, 2, 2, -5);
        assert_eq!(tile_color(&map, &colors(), 2, 2), GREEN);
    }

    /// **`statics_at` is keyed by `(y, x)` and not by z**, so the highest is
    /// picked by comparing rather than by taking the last.
    #[test]
    fn the_highest_static_wins_whatever_order_they_are_in() {
        let mut map = a_field();
        put(&mut map, 2, 4, 4, 20); // white, high
        put(&mut map, 1, 4, 4, 5); // red, low
        assert_eq!(
            tile_color(&map, &colors(), 4, 4),
            WHITE,
            "the lower static was drawn over the higher one"
        );
    }

    /// A static the radar table has no colour for falls through to what is under
    /// it. Zero is *absent* in these files, not black.
    #[test]
    fn a_static_with_no_colour_falls_through() {
        let mut map = a_field();
        put(&mut map, 3, 6, 6, 20);
        assert_eq!(tile_color(&map, &colors(), 6, 6), GREEN);
    }

    /// The rectangle walk agrees with the single-tile rule, tile for tile. They
    /// are two readers of one answer and the block walk is the one that could
    /// drift.
    #[test]
    fn the_block_walk_agrees_with_the_single_tile_rule() {
        let mut map = a_field();
        put(&mut map, 1, 1, 1, 10);
        put(&mut map, 2, 5, 6, 30);
        put(&mut map, 3, 3, 4, 30); // no colour: falls through
        put(&mut map, 1, 7, 0, -1); // below ground: ignored
        let colors = colors();

        let mut pixels = vec![Color16::TRANSPARENT; 64];
        fill(&map, &colors, (0, 0), 8, 8, &mut pixels);

        for y in 0..8u16 {
            for x in 0..8u16 {
                assert_eq!(
                    pixels[usize::from(y) * 8 + usize::from(x)],
                    tile_color(&map, &colors, x, y),
                    "the block walk and the tile rule disagree at ({x}, {y})"
                );
            }
        }
        assert_eq!(pixels[8 + 1], RED, "(1, 1)");
        assert_eq!(pixels[6 * 8 + 5], WHITE, "(5, 6)");
    }

    /// A marker is five pixels, and its arms clip rather than wrap.
    #[test]
    fn a_marker_at_a_corner_keeps_only_the_arms_on_the_map() {
        let mut pixels = vec![Color16::TRANSPARENT; 16];
        mark(&mut pixels, 4, 4, (0, 0), RED);

        assert_eq!(pixels[0], RED, "the centre");
        assert_eq!(pixels[1], RED, "the arm east of it");
        assert_eq!(pixels[4], RED, "and the one south");
        assert_eq!(
            pixels.iter().filter(|&&c| c == RED).count(),
            3,
            "the west and north arms wrapped instead of clipping",
        );
    }

    /// Away from an edge it is the whole cross, and nothing else.
    #[test]
    fn a_marker_in_the_middle_is_a_cross() {
        let mut pixels = vec![Color16::TRANSPARENT; 25];
        mark(&mut pixels, 5, 5, (2, 2), WHITE);

        for index in [12, 11, 13, 7, 17] {
            assert_eq!(pixels[index], WHITE, "pixel {index} is part of the cross");
        }
        assert_eq!(pixels.iter().filter(|&&c| c == WHITE).count(), 5);
    }

    /// Off the buffer entirely is nothing, not a wrapped pixel on the far side.
    #[test]
    fn a_marker_off_the_map_draws_nothing() {
        let mut pixels = vec![Color16::TRANSPARENT; 16];
        mark(&mut pixels, 4, 4, (4, 0), RED);
        mark(&mut pixels, 4, 4, (0, 9), RED);
        assert!(pixels.iter().all(|&c| c == Color16::TRANSPARENT));
    }

    /// A rectangle that runs off the map is filled to its edge and unmapped
    /// past it, rather than short or panicking.
    #[test]
    fn a_rectangle_past_the_edge_is_unmapped_not_missing() {
        let map = a_field();
        let mut pixels = vec![Color16::TRANSPARENT; 16];
        fill(&map, &colors(), (6, 6), 4, 4, &mut pixels);

        assert_eq!(pixels[0], GREEN, "(6, 6) is on the map");
        assert_eq!(pixels[3], UNKNOWN, "(9, 6) is not");
        assert!(
            pixels.iter().all(|&color| color != Color16::TRANSPARENT),
            "a transparent pixel would be a hole in the window"
        );
    }

    /// Nothing is written past the buffer a caller supplied. A render path
    /// should not panic over a caller's arithmetic.
    #[test]
    fn a_buffer_too_small_is_filled_as_far_as_it_goes() {
        let map = a_field();
        let mut pixels = vec![Color16::TRANSPARENT; 4];
        fill(&map, &colors(), (0, 0), 8, 8, &mut pixels);
        assert!(pixels.iter().all(|&color| color == GREEN));
    }

    #[test]
    fn producer_queue_coalesces_keys_and_never_exceeds_its_bound() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        let mut queue = RadarWorkQueue::new(2, 1).expect("non-zero limits");
        cache.invalidate_tile(facet, (0, 0), 0).expect("a revision");

        // Add three keys from the current source snapshot to exercise the
        // queue's capacity independently of cache mutation scheduling.
        let first = cache.key(facet, 0, RadarChunkCoord::new(0, 0));
        let second = cache.key(facet, 0, RadarChunkCoord::new(1, 0));
        let third = cache.key(facet, 0, RadarChunkCoord::new(2, 0));
        assert!(queue.request(first));
        assert!(queue.request(first), "an equal request is coalesced");
        assert!(queue.request(second));
        assert!(!queue.request(third), "a distinct request observes the bound");
        assert_eq!(
            queue.counters(),
            RadarWorkCounters {
                queued: 2,
                in_flight: 0,
                max_queued: 2
            }
        );

        assert_eq!(queue.take_for_producer(), vec![first]);
        assert_eq!(
            queue.counters(),
            RadarWorkCounters {
                queued: 1,
                in_flight: 1,
                max_queued: 2
            }
        );
        assert!(queue.request(first), "an in-flight request is also coalesced");
        assert!(!queue.request(third), "in-flight work counts toward the bound");
    }

    #[test]
    fn producer_queue_publishes_only_dispatched_complete_current_products() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        let mut queue = RadarWorkQueue::new(4, 1).expect("non-zero limits");
        cache.invalidate_tile(facet, (0, 0), 0).expect("a revision");
        queue.reconcile(&cache);
        let key = cache.key(facet, 0, RadarChunkCoord::new(0, 0));
        assert_eq!(queue.take_for_producer(), vec![key]);

        let complete = RadarChunk::new(key, vec![GREEN; chunk_pixel_count()]).expect("complete product");
        assert!(queue.finish(&mut cache, complete));
        assert!(cache.get(key).is_some());
        assert_eq!(
            queue.counters(),
            RadarWorkCounters {
                queued: 0,
                in_flight: 0,
                max_queued: 4
            }
        );

        let undispatched = RadarChunk::new(key, vec![RED; chunk_pixel_count()]).expect("complete product");
        assert!(!queue.finish(&mut cache, undispatched));

        cache.invalidate_tile(facet, (0, 0), 0).expect("new revision");
        queue.reconcile(&cache);
        let stale_key = cache.key(facet, 0, RadarChunkCoord::new(0, 0));
        assert_eq!(queue.take_for_producer(), vec![stale_key]);
        cache.invalidate_tile(facet, (0, 0), 0).expect("newer revision");
        let stale = RadarChunk::new(stale_key, vec![RED; chunk_pixel_count()]).expect("complete product");
        assert!(
            !queue.finish(&mut cache, stale),
            "a dispatched result from a superseded revision is rejected"
        );
        assert_eq!(
            queue.counters(),
            RadarWorkCounters {
                queued: 0,
                in_flight: 0,
                max_queued: 4
            },
            "the rejected job released its slot"
        );

        queue.reconcile(&cache);
        let current_key = cache.key(facet, 0, RadarChunkCoord::new(0, 0));
        assert_eq!(queue.take_for_producer(), vec![current_key]);
        let current =
            RadarChunk::new(current_key, vec![WHITE; chunk_pixel_count()]).expect("complete product");
        assert!(queue.finish(&mut cache, current));
        assert_eq!(
            cache.get(current_key).expect("current product").pixels()[0],
            WHITE
        );
    }

    /// The exact sequence one minimap frame runs: ask for every chunk the
    /// window is about to draw, reconcile, dispatch what is left.
    ///
    /// Regression. Reconciliation used to keep only the cache's dirty keys,
    /// and demand for never-built ground is not an invalidation — so a
    /// window's own request was thrown away between being made and being
    /// dispatched, the producer was handed nothing on every frame, and the
    /// minimap drew its `UNKNOWN` backdrop forever.
    #[test]
    fn a_window_request_survives_reconciliation_until_its_product_lands() {
        let facet = Facet(0);
        let mut cache = RadarCache::default();
        let mut queue = RadarWorkQueue::new(4, 1).expect("non-zero limits");
        let key = cache.key(facet, 0, RadarChunkCoord::new(0, 0));

        assert!(queue.request(key));
        queue.reconcile(&cache);
        assert_eq!(
            queue.take_for_producer(),
            vec![key],
            "ground no product covers is work to do, not a superseded key"
        );

        let built = RadarChunk::new(key, vec![GREEN; chunk_pixel_count()]).expect("complete product");
        assert!(queue.finish(&mut cache, built));

        // The frame after: the window asks for the same chunk again, because
        // it asks for all of its visible chunks every frame.
        assert!(queue.request(key));
        queue.reconcile(&cache);
        assert!(
            queue.take_for_producer().is_empty(),
            "terrain already ready is not rebuilt for as long as the window stays open"
        );
    }
}
