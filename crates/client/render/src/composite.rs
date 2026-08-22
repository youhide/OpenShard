//! Cached, immutable map-block pictures for the far-zoom renderer.
//!
//! A composite is deliberately a *map* resource: its pixels are ground and
//! map statics only.  Server items, mobiles, effects, cursor/selection masks,
//! and UI have no field in [`CompositePixels`] and therefore cannot accidentally
//! become part of a cache entry.  They continue through their existing passes.
//!
//! The module does not decide *when* a block is rebuilt.  Session 2 Work 3
//! supplies that bounded, camera-prioritised queue.  It also does not write a
//! fake depth/G-buffer for one large quad; Work 4 owns that interleaving policy
//! for dynamic objects.  What is complete here is the durable texture cache and
//! its colour-only one-quad draw operation: producers can populate a block
//! asynchronously and a visible block can be drawn without rebuilding its
//! constituent ground/static quads.

use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use openshard_map::grid::BlockCoord;
use openshard_map::map::BLOCK_SIZE;

use crate::blit::WORLD_FORMAT;
use crate::camera::{Camera, TILE_HEIGHT, TILE_WIDTH, TileBounds, WorldPixel, project};
use crate::chunk_cache::{LruBudget, WorkQueue};
use crate::geometry::Rect;
use crate::lod::BlockLod;

/// Virtual-pixel side of the canonical ground image for one map-block
/// composite.
///
/// A cached owner is flat ground only. Map statics stay in the live pass: a
/// roof can rise far above its 8×8 base block, which cannot fit in a bounded
/// immutable source image without making every resident block enormous. LOD1
/// retains this exact source grid; only the disabled LOD2 tier minifies it.
/// It is deliberately neither a window size nor a camera render-target size:
/// a map block must mean the same source pixels when the player pans, resizes,
/// or changes zoom.
pub const COMPOSITE_SOURCE_SIDE: u32 = BLOCK_SIZE * TILE_WIDTH as u32;

/// One cacheable terrain owner: all 64 tiles form the same level surface.
///
/// This is deliberately stronger than "every tile is flat". A block where
/// neighbouring flat tiles have different heights still has overlapping
/// diamonds and must remain in the direct LOD0 ground pass. The common height
/// is the canonical vertical origin for both the offscreen producer and the
/// later restore rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlatGroundBlock {
    block: BlockCoord,
    elevation: i8,
}

impl FlatGroundBlock {
    /// Inspect the authoritative map and return the block only when one cached
    /// texture can own all of its ground pixels without a live overlap.
    pub fn inspect(map: &openshard_map::map::WorldMap, block: BlockCoord) -> Option<Self> {
        // This is the function that decides whether the map has the block at
        // all, so it narrows the origin itself rather than through
        // [`tile_origin`]: a block past a tile coordinate is one no facet has,
        // and saying so here is the same `None` the first `map.land` would give.
        let (first_x, first_y) = block.origin();
        let first_x = u16::try_from(first_x).ok()?;
        let first_y = u16::try_from(first_y).ok()?;
        let mut elevation = None;
        for local_y in 0..BLOCK_SIZE as u16 {
            for local_x in 0..BLOCK_SIZE as u16 {
                let x = first_x.checked_add(local_x)?;
                let y = first_y.checked_add(local_y)?;
                let land = map.land(x, y)?;
                let corners = crate::ground::corner_heights(map, x, y, land.z);
                if !corners.iter().all(|height| *height == corners[0]) {
                    return None;
                }
                // Heights come from the signed map grid. Keeping the native
                // unit is less lossy than caching an already-projected offset.
                let z = corners[0] as i8;
                if corners[0] != f32::from(z) || elevation.is_some_and(|was| was != z) {
                    return None;
                }
                elevation = Some(z);
            }
        }
        Some(Self {
            block,
            elevation: elevation?,
        })
    }

    const fn at(block: BlockCoord, elevation: i8) -> Self {
        Self { block, elevation }
    }

    /// The map block this surface owns.
    pub const fn block(self) -> BlockCoord {
        self.block
    }

    /// The common height of every point on the surface.
    pub const fn elevation(self) -> i8 {
        self.elevation
    }
}

/// The fixed, viewport-independent contract for producing one composite.
///
/// A producer receives only this value and immutable map inputs.  In
/// particular it has no main-frame attachment or camera rectangle to sample.
/// `camera` looks at the centre of the padded source extent at 1:1, so its
/// block rect is exactly `0..COMPOSITE_SOURCE_SIDE` in both axes.  Both LOD
/// tiers consequently derive from the same canonical rasterisation rather
/// than from differently cropped camera frames.
#[derive(Clone, Copy, Debug)]
pub struct CompositeProducerJob {
    key: CompositeKey,
    camera: Camera,
    ground: FlatGroundBlock,
}

impl CompositeProducerJob {
    /// Define the canonical producer for one dispatched cache identity.
    pub fn new(key: CompositeKey) -> Self {
        Self::for_flat_ground(key, FlatGroundBlock::at(key.block, 0))
    }

    /// Define the canonical producer for a wholly flat block at `ground_z`.
    ///
    /// A flat plateau is self-contained, but it is not necessarily at sea
    /// level.  Its local camera must use that same elevation or the fixed
    /// 352-pixel source crops the top (or bottom) of every diamond before it
    /// reaches the cache.
    pub fn for_flat_ground(key: CompositeKey, ground: FlatGroundBlock) -> Self {
        assert_eq!(
            key.block,
            ground.block(),
            "a composite key and its ground owner must name the same map block"
        );
        let ground_z = ground.elevation();
        let (x, y) = tile_origin(key.block);
        // The ground diamond for 8×8 tiles has its vertical centre 22 pixels
        // above the centre of tile `(x + 4, y + 4)`.  Looking there centres
        // the 352-pixel diamond in the fixed 352-pixel producer target.
        let centre = project(openshard_protocol::world::Point::new(x + 4, y + 4, ground_z));
        let mut camera = Camera::new(
            openshard_protocol::world::Point::new(x + 4, y + 4, ground_z),
            COMPOSITE_SOURCE_SIDE,
            COMPOSITE_SOURCE_SIDE,
        );
        camera.look_at_pixel(WorldPixel {
            x: centre.x,
            y: centre.y - TILE_HEIGHT / 2,
        });
        Self { key, camera, ground }
    }

    /// The immutable cache identity this producer is allowed to complete.
    pub const fn key(self) -> CompositeKey {
        self.key
    }

    /// Fixed, local 1:1 camera used for the offscreen map-only draw.
    pub const fn camera(self) -> Camera {
        self.camera
    }

    /// The one immutable terrain owner this job is permitted to publish.
    pub const fn ground(self) -> FlatGroundBlock {
        self.ground
    }

    /// Fixed source attachment dimensions. LOD1 retains this exact grid.
    pub const fn source_size(self) -> CompositeSize {
        CompositeSize {
            width: COMPOSITE_SOURCE_SIDE,
            height: COMPOSITE_SOURCE_SIDE,
        }
    }

    /// The tier's final cached ground texture dimensions.
    pub const fn output_size(self) -> CompositeSize {
        CompositeSize::for_block(self.key.tier, 0)
    }

    /// The ground block footprint in this job's own source attachment.
    pub fn rect_in(self, camera: Camera) -> Rect {
        let (x, y) = tile_origin(self.key.block);
        let top = camera.to_screen(openshard_protocol::world::Point::new(
            x,
            y,
            self.ground.elevation(),
        ));
        let side = COMPOSITE_SOURCE_SIDE as f32;
        Rect {
            x: top.x as f32 - side / 2.0,
            y: top.y as f32 - TILE_WIDTH as f32 / 2.0,
            width: side,
            height: side,
        }
    }

    /// The ground block footprint in this job's own source attachment.
    pub fn source_rect(self) -> Rect {
        self.rect_in(self.camera)
    }
}

/// A block's north-west tile, as a tile coordinate.
///
/// [`BlockCoord::origin`] answers in `u32` because a block coordinate is not
/// promised to be on any facet. A composite is only ever built for a block the
/// map contains — [`FlatGroundBlock::inspect`] asks the map first — so here the
/// origin is a real tile and the narrowing is the place that says so.
pub fn tile_origin(block: BlockCoord) -> (u16, u16) {
    let (x, y) = block.origin();
    (
        u16::try_from(x).expect("a composite's block is on the facet"),
        u16::try_from(y).expect("a composite's block is on the facet"),
    )
}

/// A block column as signed, for the pan arithmetic that steps a rectangle by
/// its own width and can land left of the map before it is clamped.
///
/// A facet is at most 7,168 tiles across — 896 blocks — so the conversion is
/// total in both directions; it is written out rather than cast so that a grid
/// that somehow was not would stop here instead of wrapping into a rectangle
/// on the other side of the world.
fn signed(blocks: u32) -> i32 {
    i32::try_from(blocks).expect("a facet's block count fits i32")
}

/// The inverse, for a clamped result that is back inside the map.
fn unsigned(blocks: i32) -> u32 {
    u32::try_from(blocks).expect("a clamped block column is not negative")
}

/// An inclusive rectangle of map blocks.
///
/// This is the queue's cell range.  It is deliberately separate from
/// [`TileBounds`]: the camera and streaming code work in tiles, while a
/// composite request has exactly one 8×8 map block as its unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapBlockBounds {
    /// Lowest block column, inclusive.
    pub min_x: u32,
    /// Highest block column, inclusive.
    pub max_x: u32,
    /// Lowest block row, inclusive.
    pub min_y: u32,
    /// Highest block row, inclusive.
    pub max_y: u32,
}

impl MapBlockBounds {
    /// Convert camera tile coverage to actual map blocks, clipping off-map
    /// camera slack before it can become a request.
    pub fn from_tiles(bounds: TileBounds, map_width: u32, map_height: u32) -> Option<Self> {
        let (xs, ys) = bounds.clamp_to(map_width, map_height)?;
        let first = BlockCoord::containing(*xs.start(), *ys.start());
        let last = BlockCoord::containing(*xs.end(), *ys.end());
        Some(Self {
            min_x: first.x,
            max_x: last.x,
            min_y: first.y,
            max_y: last.y,
        })
    }

    /// Number of blocks across, inclusive.
    pub const fn width(self) -> u32 {
        self.max_x - self.min_x + 1
    }

    /// Number of blocks down, inclusive.
    pub const fn height(self) -> u32 {
        self.max_y - self.min_y + 1
    }

    fn centre(self) -> (i32, i32) {
        (
            (signed(self.min_x) + signed(self.max_x)) / 2,
            (signed(self.min_y) + signed(self.max_y)) / 2,
        )
    }

    /// Iterate every block in deterministic row-major order.
    pub fn blocks(self) -> impl Iterator<Item = BlockCoord> {
        (self.min_y..=self.max_y)
            .flat_map(move |y| (self.min_x..=self.max_x).map(move |x| BlockCoord { x, y }))
    }

    fn contains(self, block: BlockCoord) -> bool {
        (self.min_x..=self.max_x).contains(&block.x) && (self.min_y..=self.max_y).contains(&block.y)
    }

    /// The rectangle protected from cache eviction while `self` is visible.
    ///
    /// This is deliberately expressed in map blocks rather than pixels.  A
    /// small pan must not immediately discard a just-left composite only to
    /// queue and upload it again on the next pan back.
    pub fn expanded_by(self, margin: u32) -> Self {
        Self {
            min_x: self.min_x.saturating_sub(margin),
            max_x: self.max_x.saturating_add(margin),
            min_y: self.min_y.saturating_sub(margin),
            max_y: self.max_y.saturating_add(margin),
        }
    }

    /// One viewport-sized rectangle immediately in the direction from `was`.
    ///
    /// The result is clamped by `map`; a pan with no block-level movement has
    /// no ahead work.  This leaves the full currently visible rectangle ahead
    /// of tiny one-block pans, which is both deterministic and enough time for
    /// a bounded worker to catch up before the camera arrives.
    fn ahead_of(self, was: Self, map: Self) -> Option<Self> {
        let (old_x, old_y) = was.centre();
        let (new_x, new_y) = self.centre();
        let dx = (new_x - old_x).signum();
        let dy = (new_y - old_y).signum();
        if dx == 0 && dy == 0 {
            return None;
        }
        let shift_x = dx * signed(self.width());
        let shift_y = dy * signed(self.height());
        let min_x = (signed(self.min_x) + shift_x).clamp(signed(map.min_x), signed(map.max_x));
        let max_x = (signed(self.max_x) + shift_x).clamp(signed(map.min_x), signed(map.max_x));
        let min_y = (signed(self.min_y) + shift_y).clamp(signed(map.min_y), signed(map.max_y));
        let max_y = (signed(self.max_y) + shift_y).clamp(signed(map.min_y), signed(map.max_y));
        (min_x <= max_x && min_y <= max_y).then_some(Self {
            min_x: unsigned(min_x),
            max_x: unsigned(max_x),
            min_y: unsigned(min_y),
            max_y: unsigned(max_y),
        })
    }
}

/// The two cached resolutions.  LOD 0 intentionally has no texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompositeTier {
    /// The lossless cached tier: one source pixel per cached texel.
    Lod1,
    /// Four source pixels per cached texel.
    Lod2,
}

impl CompositeTier {
    /// The composite tier corresponding to a selected block LOD.
    pub const fn from_lod(lod: BlockLod) -> Option<Self> {
        match lod {
            BlockLod::Lod0 => None,
            BlockLod::Lod1 => Some(Self::Lod1),
            BlockLod::Lod2 => Some(Self::Lod2),
        }
    }

    /// Source pixels represented by one cache texel in each direction.
    pub const fn source_pixels_per_texel(self) -> u32 {
        match self {
            Self::Lod1 => 1,
            Self::Lod2 => 4,
        }
    }
}

/// A revision of the immutable inputs to a composite.
///
/// A producer increments this for map/static mutation or a change in the
/// rendered composite contract. Static-atlas growth is intentionally not such
/// a change: atlas pages are append-only and a composite has already captured
/// final pixels rather than retaining atlas UVs. Work 3 can then request only
/// stale `(block, tier)` entries; no cache-wide synchronous rebuild is implied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImmutableRevision(pub u64);

/// The full identity of one cached image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompositeKey {
    /// The map block pictured by the texture.
    pub block: BlockCoord,
    /// The cache's intentional sampling resolution.
    pub tier: CompositeTier,
    /// Immutable source revision used to produce its pixels.
    pub revision: ImmutableRevision,
}

/// Why a block was deliberately held at direct LOD0 for this session.
///
/// This is a cache safety decision, not an error-recovery queue: recording it
/// alongside the owning source proof makes a field dump actionable without
/// asking someone to reconstruct the camera frame that exposed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositeQuarantineReason {
    /// The map inspection found slopes or mixed elevations in this block.
    NonFlatGround,
    /// The full LOD0 oracle found a missing immutable-map pixel after restore.
    OracleMissingGroundCoverage,
}

/// The compact immutable owner record retained for a quarantined block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompositeQuarantine {
    pub block: BlockCoord,
    pub key: CompositeKey,
    pub ground: Option<FlatGroundBlock>,
    pub reason: CompositeQuarantineReason,
}

/// Dimensions of one already-rasterised composite image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompositeSize {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
}

impl CompositeSize {
    /// A non-empty texture extent.
    pub const fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            None
        } else {
            Some(Self { width, height })
        }
    }

    /// A square extent for the ground diamond plus a caller-provided static
    /// overhang, represented at the exact resolution of `tier`.
    ///
    /// `overhang_source_pixels` is the cache format's fixed static-overhang
    /// allowance. Keeping it in the extent makes a tall tree at a block edge
    /// part of exactly one cached image instead of being clipped or forcing its
    /// neighbours to rebuild.
    pub const fn for_block(tier: CompositeTier, overhang_source_pixels: u32) -> Self {
        let source = BLOCK_SIZE * TILE_WIDTH as u32 + overhang_source_pixels * 2;
        let divisor = tier.source_pixels_per_texel();
        // ceil(source / divisor), preserving the right/bottom edge.
        let side = source.div_ceil(divisor);
        Self {
            width: side,
            height: side,
        }
    }

    /// RGBA8 upload length, or `None` when an input has overflowed `usize`.
    pub fn rgba_bytes(self) -> Option<usize> {
        usize::try_from(self.width)
            .ok()?
            .checked_mul(usize::try_from(self.height).ok()?)?
            .checked_mul(4)
    }
}

/// The already composed RGBA8 image of immutable map data.
///
/// Construction verifies the exact texture byte length.  That check keeps a
/// failed or partial worker result from becoming a drawable cache entry.
#[derive(Clone, Debug, PartialEq)]
pub struct CompositePixels {
    size: CompositeSize,
    rgba: Vec<u8>,
    /// The cache starts colour-only while Work 2 is being built.  A composite
    /// is eligible to replace map geometry only once its producer supplied the
    /// deferred planes below; otherwise using it would leave dynamic sprites
    /// testing against a made-up depth/G-buffer.
    deferred: Option<DeferredPixels>,
}

/// The per-texel facts a cached map block must retain to participate in the
/// ordinary deferred world pass.
///
/// `ids` contains the normal `gbuffer::IDS_FORMAT` word except that producers
/// reserve the high bit of its row id for the cached-map route.  The eventual
/// blit branch reads `position` directly for that route, rather than indexing
/// the current frame's transient ground/static instance buffers.  `depth` is
/// the producer's depth value and `depth_base` is the camera tile depth it was
/// based on; a draw adjusts the value by the current camera base before writing
/// fragment depth.  This is deliberately source data, not a lossy screenshot.
#[derive(Clone, Debug, PartialEq)]
pub struct DeferredPixels {
    ids: Vec<u32>,
    position: Vec<f32>,
    normal: Vec<u32>,
    depth: Vec<f32>,
    depth_base: i32,
}

impl DeferredPixels {
    /// Validate the four exact per-texel planes from a completed producer.
    pub fn new(
        size: CompositeSize,
        ids: Vec<u32>,
        position: Vec<f32>,
        normal: Vec<u32>,
        depth: Vec<f32>,
        depth_base: i32,
    ) -> Option<Self> {
        let texels = size.width.checked_mul(size.height)? as usize;
        (ids.len() == texels
            && position.len() == texels.checked_mul(4)?
            && normal.len() == texels
            && depth.len() == texels)
            .then_some(Self {
                ids,
                position,
                normal,
                depth,
                depth_base,
            })
    }

    pub fn ids(&self) -> &[u32] {
        &self.ids
    }
    pub fn position(&self) -> &[f32] {
        &self.position
    }
    pub fn normal(&self) -> &[u32] {
        &self.normal
    }
    pub fn depth(&self) -> &[f32] {
        &self.depth
    }
    pub const fn depth_base(&self) -> i32 {
        self.depth_base
    }
}

impl CompositePixels {
    /// Validate one RGBA8 composite result.
    pub fn new(size: CompositeSize, rgba: Vec<u8>) -> Option<Self> {
        (rgba.len() == size.rgba_bytes()?).then_some(Self {
            size,
            rgba,
            deferred: None,
        })
    }

    /// Texture dimensions.
    pub const fn size(&self) -> CompositeSize {
        self.size
    }

    /// Pixels in row-major RGBA8 order.
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// Attach the deferred facts produced from the same immutable source.
    /// A size mismatch is impossible because [`DeferredPixels::new`] validates
    /// it against the exact size passed here, but taking the size again keeps a
    /// caller from pairing two unrelated completed jobs by accident.
    pub fn with_deferred(mut self, deferred: DeferredPixels) -> Option<Self> {
        let texels = self.size.width.checked_mul(self.size.height)? as usize;
        (deferred.ids.len() == texels).then(|| {
            self.deferred = Some(deferred);
            self
        })
    }

    /// Deferred data makes this a candidate for geometry replacement.  Plain
    /// RGBA work remains drawable only as a diagnostic overlay, never as the
    /// authoritative map representation beneath mobiles or server items.
    pub fn deferred(&self) -> Option<&DeferredPixels> {
        self.deferred.as_ref()
    }
}

/// A GPU-resident cached composite.
#[derive(Debug)]
pub struct CompositeTexture {
    key: CompositeKey,
    /// The source contract carried through producer and restore unchanged.
    ground: FlatGroundBlock,
    /// CPU pixels are retained for worker-produced entries.  GPU captures do
    /// not read the image back merely to upload it again, so they have no CPU
    /// copy here.
    pixels: Option<CompositePixels>,
    size: CompositeSize,
    depth_base: i32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    deferred: Option<DeferredTextures>,
}

/// GPU planes for a completed deferred composite.  Keeping the owning textures
/// beside their views makes this a real cache entry rather than a frame-local
/// bind group with dangling sources.
#[derive(Debug)]
struct DeferredTextures {
    _ids: wgpu::Texture,
    ids_view: wgpu::TextureView,
    _position: wgpu::Texture,
    position_view: wgpu::TextureView,
    _normal: wgpu::Texture,
    normal_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
}

impl CompositeTexture {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, key: CompositeKey, pixels: CompositePixels) -> Self {
        let size = pixels.size();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("map block composite"),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORLD_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels.rgba(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * size.width),
                rows_per_image: Some(size.height),
            },
            wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let deferred = pixels
            .deferred()
            .map(|planes| DeferredTextures::new(device, queue, size, planes));
        Self {
            key,
            ground: FlatGroundBlock::at(key.block, 0),
            size,
            depth_base: pixels.deferred().map_or(0, DeferredPixels::depth_base),
            pixels: Some(pixels),
            texture,
            view,
            deferred,
        }
    }

    /// Allocate an entry whose planes are filled by a GPU copy from the
    /// map-only portion of a normal frame.  This deliberately never maps a
    /// buffer: a queue job becomes useful on a later frame without inserting a
    /// CPU readback stall between the source draw and the cache upload.
    fn capture(
        device: &wgpu::Device,
        key: CompositeKey,
        size: CompositeSize,
        depth_base: i32,
        ground: FlatGroundBlock,
    ) -> Self {
        assert_eq!(
            key.block,
            ground.block(),
            "a captured texture must retain the producer's own ground block"
        );
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("captured map block composite"),
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORLD_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            key,
            ground,
            pixels: None,
            size,
            depth_base,
            texture,
            view,
            deferred: Some(DeferredTextures::capture(device, size)),
        }
    }

    /// Immutable identity of the image.
    pub const fn key(&self) -> CompositeKey {
        self.key
    }

    /// The immutable ground owner captured into this texture.
    pub const fn ground(&self) -> FlatGroundBlock {
        self.ground
    }

    /// The exact current-frame destination rectangle for this cached plateau.
    pub fn rect_in(&self, camera: Camera) -> crate::geometry::Rect {
        CompositeProducerJob::for_flat_ground(self.key, self.ground).rect_in(camera)
    }

    /// Camera depth base used when this entry's stored depths were written.
    pub const fn depth_base(&self) -> i32 {
        self.depth_base
    }

    /// Whether this entry owns all planes required to replace map geometry.
    pub fn has_deferred(&self) -> bool {
        self.deferred.is_some()
    }

    /// Texture size and source pixels retained for deterministic replacement.
    pub fn pixels(&self) -> Option<&CompositePixels> {
        self.pixels.as_ref()
    }

    /// The texture view bound by [`CompositeRenderer`].
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    fn deferred_views(
        &self,
    ) -> Option<(
        &wgpu::TextureView,
        &wgpu::TextureView,
        &wgpu::TextureView,
        &wgpu::TextureView,
    )> {
        let planes = self.deferred.as_ref()?;
        Some((
            &planes.ids_view,
            &planes.position_view,
            &planes.normal_view,
            &planes.depth_view,
        ))
    }

    fn deferred_textures(&self) -> Option<(&wgpu::Texture, &wgpu::Texture, &wgpu::Texture, &wgpu::Texture)> {
        let planes = self.deferred.as_ref()?;
        Some((&planes._ids, &planes._position, &planes._normal, &planes._depth))
    }

    /// The deferred attachment textures, for an explicit diagnostic readback.
    ///
    /// Normal composition must use [`Self::deferred_views`] and stays entirely
    /// on the GPU.  This accessor exists so an opt-in field scenario can
    /// inspect a completed cache entry itself, before that entry is restored
    /// into a camera frame.
    pub fn deferred_textures_for_audit(
        &self,
    ) -> Option<(&wgpu::Texture, &wgpu::Texture, &wgpu::Texture, &wgpu::Texture)> {
        self.deferred_textures()
    }

    /// The underlying texture, for diagnostics and GPU-memory accounting.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// GPU bytes retained by this composite.
    pub fn gpu_bytes(&self) -> u64 {
        let rgba = self.size.rgba_bytes().unwrap_or(0) as u64;
        rgba + self.deferred.as_ref().map_or(0, |_| rgba * 7)
    }
}

impl DeferredTextures {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: CompositeSize, planes: &DeferredPixels) -> Self {
        let texture = |label, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.width,
                    height: size.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let ids = texture("map composite ids", crate::gbuffer::IDS_FORMAT);
        let position = texture("map composite position", crate::gbuffer::POSITION_FORMAT);
        let normal = texture("map composite normal", crate::gbuffer::NORMAL_FORMAT);
        let depth = texture("map composite depth", wgpu::TextureFormat::R32Float);
        let write = |texture: &wgpu::Texture, bytes: &[u8], bytes_per_texel: u32| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.width * bytes_per_texel),
                    rows_per_image: Some(size.height),
                },
                wgpu::Extent3d {
                    width: size.width,
                    height: size.height,
                    depth_or_array_layers: 1,
                },
            );
        };
        let words = |values: &[u32]| {
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        };
        let floats = |values: &[f32]| {
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>()
        };
        write(&ids, &words(planes.ids()), 4);
        write(&position, &floats(planes.position()), 16);
        write(&normal, &words(planes.normal()), 4);
        write(&depth, &floats(planes.depth()), 4);
        Self {
            ids_view: ids.create_view(&wgpu::TextureViewDescriptor::default()),
            _ids: ids,
            position_view: position.create_view(&wgpu::TextureViewDescriptor::default()),
            _position: position,
            normal_view: normal.create_view(&wgpu::TextureViewDescriptor::default()),
            _normal: normal,
            depth_view: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            _depth: depth,
        }
    }

    /// Empty GPU planes for a captured entry.  Colour, ids, position and
    /// normal arrive through `copy_texture_to_texture`; depth is rasterised by
    /// [`CompositeRenderer::capture`] because a depth attachment cannot be
    /// copied into the `R32Float` sampling plane used by the restore shader.
    fn capture(device: &wgpu::Device, size: CompositeSize) -> Self {
        let texture = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.width,
                    height: size.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let sampled = wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC;
        let ids = texture("captured map composite ids", crate::gbuffer::IDS_FORMAT, sampled);
        let position = texture(
            "captured map composite position",
            crate::gbuffer::POSITION_FORMAT,
            sampled,
        );
        let normal = texture(
            "captured map composite normal",
            crate::gbuffer::NORMAL_FORMAT,
            sampled,
        );
        let depth = texture(
            "captured map composite depth",
            wgpu::TextureFormat::R32Float,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
        );
        Self {
            ids_view: ids.create_view(&wgpu::TextureViewDescriptor::default()),
            _ids: ids,
            position_view: position.create_view(&wgpu::TextureViewDescriptor::default()),
            _position: position,
            normal_view: normal.create_view(&wgpu::TextureViewDescriptor::default()),
            _normal: normal,
            depth_view: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            _depth: depth,
        }
    }
}

/// The hard cache retention policy.
///
/// The default is 128 MiB for the colour and deferred planes together.  It is
/// independent of the static atlas's 128 MiB page limit: a deferred composite
/// has eight RGBA-sized planes, so conflating the two would silently retain
/// much more GPU memory than its number suggests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompositeCacheLimits {
    /// Maximum bytes retained for entries outside the protected viewport
    /// margin.  Visible and near-visible entries are never evicted merely to
    /// satisfy this limit; they are the working set, not the cache tail.
    pub max_gpu_bytes: u64,
    /// Number of map blocks kept on every side of the visible rectangle.
    pub viewport_margin_blocks: u32,
}

impl CompositeCacheLimits {
    /// The shipped 128 MiB tail budget and one-block pan hysteresis margin.
    pub const DEFAULT_MAX_GPU_BYTES: u64 = 128 * 1024 * 1024;
    pub const DEFAULT_VIEWPORT_MARGIN_BLOCKS: u32 = 1;

    /// A non-zero tail budget and its viewport hysteresis margin.
    pub const fn new(max_gpu_bytes: u64, viewport_margin_blocks: u32) -> Option<Self> {
        if max_gpu_bytes == 0 {
            None
        } else {
            Some(Self {
                max_gpu_bytes,
                viewport_margin_blocks,
            })
        }
    }
}

impl Default for CompositeCacheLimits {
    fn default() -> Self {
        Self {
            max_gpu_bytes: Self::DEFAULT_MAX_GPU_BYTES,
            viewport_margin_blocks: Self::DEFAULT_VIEWPORT_MARGIN_BLOCKS,
        }
    }
}

/// What one cache-maintenance pass discarded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositeEviction {
    /// Entries discarded from the least-recently-used tail.
    pub entries: usize,
    /// GPU bytes released by those entries.
    pub freed_gpu_bytes: u64,
    /// Bytes still retained after the pass.
    pub retained_gpu_bytes: u64,
    /// Bytes above the configured tail budget that are protected by the
    /// visible viewport margin.  This is reported rather than evicting a
    /// near-visible image and defeating the hysteresis guarantee.
    pub protected_over_budget_bytes: u64,
}

/// A cache of immutable block pictures with a bounded LRU tail.
#[derive(Debug)]
pub struct CompositeCache {
    entries: BTreeMap<CompositeKey, CompositeTexture>,
    /// Blocks the full-frame oracle has proved unsafe to restore. They stay at
    /// LOD0 for the rest of the session: a correct fallback is preferable to
    /// repeatedly rebuilding a known-bad cache image every frame.
    rejected: BTreeMap<BlockCoord, CompositeQuarantine>,
    latest_quarantine: Option<CompositeQuarantine>,
    limits: CompositeCacheLimits,
    budget: LruBudget<CompositeKey>,
}

impl Default for CompositeCache {
    fn default() -> Self {
        Self::with_limits(CompositeCacheLimits::default())
    }
}

impl CompositeCache {
    /// Create a cache with an explicit GPU-tail budget.
    pub fn with_limits(limits: CompositeCacheLimits) -> Self {
        Self {
            entries: BTreeMap::new(),
            rejected: BTreeMap::new(),
            latest_quarantine: None,
            limits,
            budget: LruBudget::new(limits.max_gpu_bytes)
                .expect("composite cache limits require a non-zero budget"),
        }
    }

    /// The configured cache retention policy.
    pub const fn limits(&self) -> CompositeCacheLimits {
        self.limits
    }

    /// Number of ready textures.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no texture has been produced yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A ready composite for the exact immutable revision.
    pub fn get(&self, key: CompositeKey) -> Option<&CompositeTexture> {
        if self.rejected.contains_key(&key.block) {
            return None;
        }
        let entry = self.entries.get(&key)?;
        self.budget.touch(key);
        Some(entry)
    }

    /// The selected cached representation, or its immediate detailed fallback.
    ///
    /// A newly visible LOD 2 block may keep drawing a ready LOD 1 texture while
    /// its LOD 2 job waits.  A LOD 1 miss falls through to LOD 0 (`None`), so a
    /// caller keeps its established detailed geometry instead of composing a
    /// full map block synchronously in the camera frame.
    pub fn selected_or_more_detailed(
        &self,
        block: BlockCoord,
        selected: BlockLod,
        revision: ImmutableRevision,
    ) -> Option<&CompositeTexture> {
        let tier = CompositeTier::from_lod(selected)?;
        let key = CompositeKey {
            block,
            tier,
            revision,
        };
        self.get(key).or_else(|| {
            let detailed = selected.next_more_detailed()?;
            let tier = CompositeTier::from_lod(detailed)?;
            self.get(CompositeKey {
                block,
                tier,
                revision,
            })
        })
    }

    /// Upload a completed immutable map composite.
    ///
    /// Replacing the exact same key is intentional: a worker may retry after a
    /// device loss, and revision equality says its source content still is the
    /// same.  Revision changes create a distinct entry so Work 3 can choose
    /// when the old image becomes unreachable rather than flashing a block.
    pub fn insert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: CompositeKey,
        pixels: CompositePixels,
    ) -> &CompositeTexture {
        let composite = CompositeTexture::new(device, queue, key, pixels);
        let bytes = composite.gpu_bytes();
        self.entries.insert(key, composite);
        self.budget.insert(key, bytes);
        self.get(key).expect("the cache has just inserted this key")
    }

    /// Allocate a GPU-only capture of one already-drawn map-only rectangle.
    ///
    /// `source` belongs to the current frame and the entry becomes visible to
    /// the next one.  No caller can hand in dynamic attachment textures: the
    /// source is the world target at the exact point before server items and
    /// mobiles are rendered. [`CompositeRenderer`] fills the returned planes
    /// with a GPU capture in the same command encoder.
    fn capture(
        &mut self,
        device: &wgpu::Device,
        key: CompositeKey,
        source: CaptureSource<'_>,
        ground: FlatGroundBlock,
    ) -> &CompositeTexture {
        let divisor = key.tier.source_pixels_per_texel();
        let size = CompositeSize::new(
            source.rect.width.div_ceil(divisor),
            source.rect.height.div_ceil(divisor),
        )
        .expect("a non-empty capture rectangle has a non-empty tier");
        let composite = CompositeTexture::capture(device, key, size, source.depth_base, ground);
        let bytes = composite.gpu_bytes();
        self.entries.insert(key, composite);
        self.budget.insert(key, bytes);
        self.get(key).expect("the captured entry was just inserted")
    }

    /// Forget one exact entry.  This is intentionally narrow: mutation code
    /// can invalidate affected block/tier pairs without a global cache clear.
    pub fn remove(&mut self, key: CompositeKey) -> Option<CompositeTexture> {
        let removed = self.entries.remove(&key);
        if removed.is_some() {
            self.budget.remove(key);
        }
        removed
    }

    /// Permanently fall back to direct LOD0 rendering for one block after a
    /// full-frame oracle found a missing map pixel in its cached replacement.
    ///
    /// This is deliberately a block-level circuit breaker rather than a retry:
    /// retrying the same deterministic producer would merely reintroduce the
    /// hole after its queue comes round again. Map terrain is immutable for the
    /// lifetime of this cache, so the direct path remains the authoritative
    /// safe representation until the underlying producer is fixed.
    pub fn quarantine(&mut self, quarantine: CompositeQuarantine) -> usize {
        self.latest_quarantine = Some(quarantine);
        self.rejected.insert(quarantine.block, quarantine);
        self.invalidate_block(quarantine.block)
    }

    /// Permanently fall back to LOD0 and retain the source owner that proved
    /// unsafe. The optional ground proof distinguishes a map-inspection
    /// decision from an oracle failure after a producer/restore attempt.
    pub fn reject_block(
        &mut self,
        key: CompositeKey,
        ground: Option<FlatGroundBlock>,
        reason: CompositeQuarantineReason,
    ) -> usize {
        self.quarantine(CompositeQuarantine {
            block: key.block,
            key,
            ground,
            reason,
        })
    }

    /// A rejected block acts as ready to the scheduler so it does not consume
    /// an atlas/preparation slot every frame only to be discarded again.
    pub fn is_rejected(&self, block: BlockCoord) -> bool {
        self.rejected.contains_key(&block)
    }

    /// Number of blocks permanently using the safe direct path.
    pub fn quarantined_len(&self) -> usize {
        self.rejected.len()
    }

    /// The most recent safety decision, including its block/key/owner proof.
    pub const fn latest_quarantine(&self) -> Option<CompositeQuarantine> {
        self.latest_quarantine
    }

    /// Forget every cached resolution and revision of one changed map block.
    ///
    /// A map/static mutation changes the source pixels for both cached LODs;
    /// keeping another revision around would make it too easy for a fallback
    /// lookup to show stale map state.
    pub fn invalidate_block(&mut self, block: BlockCoord) -> usize {
        self.invalidate_matching(|key| key.block == block)
    }

    /// Forget selected cached tiers of one changed map block.
    pub fn invalidate_block_tiers(&mut self, block: BlockCoord, tiers: &[CompositeTier]) -> usize {
        self.invalidate_matching(|key| key.block == block && tiers.contains(&key.tier))
    }

    /// Forget every cached resolution/revision in an affected block rectangle.
    pub fn invalidate_blocks(&mut self, blocks: MapBlockBounds) -> usize {
        self.invalidate_matching(|key| blocks.contains(key.block))
    }

    /// Forget selected cached tiers in an affected block rectangle.
    pub fn invalidate_block_tiers_in(&mut self, blocks: MapBlockBounds, tiers: &[CompositeTier]) -> usize {
        self.invalidate_matching(|key| blocks.contains(key.block) && tiers.contains(&key.tier))
    }

    /// Forget every entry whose immutable input changed globally, such as a
    /// world-output format change.  Callers should use the block variants for
    /// ordinary map/static or newly packed-art changes.
    pub fn clear(&mut self) -> usize {
        let removed = self.entries.len();
        self.entries.clear();
        self.budget.clear();
        removed
    }

    fn invalidate_matching(&mut self, mut stale: impl FnMut(&CompositeKey) -> bool) -> usize {
        let keys: Vec<_> = self.entries.keys().copied().filter(|key| stale(key)).collect();
        for key in &keys {
            self.entries.remove(key);
            self.budget.remove(*key);
        }
        keys.len()
    }

    /// Enforce the configured GPU-tail budget by evicting LRU entries outside
    /// the viewport's hysteresis margin.  Call once per rendered frame after
    /// the cache's completed captures have been accepted.
    pub fn evict_lru_outside_viewport(&mut self, visible: Option<MapBlockBounds>) -> CompositeEviction {
        let protected = visible.map(|bounds| bounds.expanded_by(self.limits.viewport_margin_blocks));
        self.budget.set_protected(
            self.entries
                .keys()
                .copied()
                .filter(|key| protected.is_some_and(|bounds| bounds.contains(key.block))),
        );
        let report = self.budget.evict_to_budget();
        for key in &report.keys {
            self.entries
                .remove(key)
                .expect("the LRU decision names a composite cache entry");
        }
        CompositeEviction {
            entries: report.keys.len(),
            freed_gpu_bytes: report.freed_bytes,
            retained_gpu_bytes: report.retained_bytes,
            protected_over_budget_bytes: report.protected_over_budget_bytes,
        }
    }

    /// Total retained RGBA8 texture bytes.
    pub fn gpu_bytes(&self) -> u64 {
        self.budget.retained_bytes()
    }
}

/// The immutable attachments captured at the map/dynamic boundary.
#[derive(Clone, Copy, Debug)]
pub struct CaptureSource<'a> {
    pub color: &'a wgpu::Texture,
    pub ids: &'a wgpu::Texture,
    pub position: &'a wgpu::Texture,
    pub normal: &'a wgpu::Texture,
    pub depth: &'a wgpu::TextureView,
    pub depth_base: i32,
    pub rect: crate::blit::ViewportRect,
}

/// Why one composite job is waiting.
///
/// The order is intentional: every visible block is dispatched before a block
/// merely predicted to enter the view.  The queue then sorts by distance and
/// stable key, so its output does not depend on `HashMap` iteration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompositePriority {
    /// The camera can see the block in this frame.
    Visible,
    /// The block is one viewport ahead of the camera's block-level motion.
    Ahead,
}

/// One bounded asynchronous composition request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompositeWork {
    /// The immutable image to build or refresh.
    pub key: CompositeKey,
    /// Why the work was scheduled.
    pub priority: CompositePriority,
}

/// A producer request whose map-derived ownership proof has been accepted.
///
/// `ground` is deliberately carried with the work instead of rediscovering
/// eligibility at render time.  It freezes the block and shared elevation that
/// define both the producer camera and the later restore transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedCompositeWork {
    pub work: CompositeWork,
    pub ground: FlatGroundBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct QueueOrder {
    priority: CompositePriority,
    distance: u32,
    key: CompositeKey,
}

/// The bounded map-cell queue shared by streamed map work and composites.
///
/// This value never composes pixels.  `take_for_frame` hands a small fixed
/// number of jobs to the background/idle producer, and `finished` marks a job
/// available for another revision.  That separation is the guarantee that a
/// newly exposed large block never becomes a synchronous camera-frame build.
#[derive(Debug)]
pub struct CompositeWorkQueue {
    queue: WorkQueue<CompositeKey>,
    orders: BTreeMap<CompositeKey, QueueOrder>,
    prepared: BTreeMap<CompositeKey, FlatGroundBlock>,
    previous_visible: Option<MapBlockBounds>,
}

impl Default for CompositeWorkQueue {
    fn default() -> Self {
        Self::new(128, 1).expect("the shipped composite queue limits are non-zero")
    }
}

impl CompositeWorkQueue {
    /// Construct a queue with explicit pending and per-frame bounds.
    pub fn new(max_pending: usize, builds_per_frame: usize) -> Option<Self> {
        (max_pending != 0 && builds_per_frame != 0).then_some(Self {
            queue: WorkQueue::new(max_pending, builds_per_frame)
                .expect("the composite wrapper has checked its limits"),
            orders: BTreeMap::new(),
            prepared: BTreeMap::new(),
            previous_visible: None,
        })
    }

    /// Requests waiting to be handed to a producer.
    pub fn pending_len(&self) -> usize {
        self.queue.pending_len()
    }

    /// Requests a producer currently owns.
    pub fn in_flight_len(&self) -> usize {
        self.queue.in_flight_len()
    }

    /// Pending jobs whose immutable atlas inputs are ready for the producer.
    pub fn prepared_len(&self) -> usize {
        self.prepared.len()
    }

    /// Record the exact pending job's immutable source proof and atlas inputs.
    ///
    /// A stale/cancelled key cannot be made ready again: it has to be refreshed
    /// and selected as a new pending request first.
    pub fn mark_prepared(&mut self, key: CompositeKey, ground: FlatGroundBlock) -> bool {
        assert_eq!(
            key.block,
            ground.block(),
            "a composite job may only carry the source proof for its own block"
        );
        if !self.queue.contains_pending(key) {
            return false;
        }
        match self.prepared.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(ground);
                true
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                debug_assert_eq!(
                    *entry.get(),
                    ground,
                    "a pending composite key cannot change source transform"
                );
                false
            }
        }
    }

    /// The next bounded set of requests whose immutable inputs should be
    /// prepared.
    ///
    /// This does not dispatch work or alter queue ownership.  It lets an app
    /// append the map art required by a block before it calls
    /// [`take_prepared_for_frame`](Self::take_prepared_for_frame); a failed
    /// preparation leaves the exact request pending for a later retry.
    pub fn preparation_candidates(&self) -> Vec<CompositeWork> {
        let mut ordered: Vec<_> = self
            .orders
            .values()
            .copied()
            .filter(|order| self.queue.contains_pending(order.key))
            .filter(|order| !self.prepared.contains_key(&order.key))
            .collect();
        ordered.sort();
        ordered
            .into_iter()
            .take(self.queue.work_per_turn())
            .map(|order| CompositeWork {
                key: order.key,
                priority: order.priority,
            })
            .collect()
    }

    /// Enqueue the selected tier for every visible block, then one viewport of
    /// prefetch work in the camera's movement direction.
    ///
    /// `ready` is normally the composite cache's exact-key lookup.  It keeps a
    /// completed composite out of both the pending and in-flight sets, while a
    /// different immutable revision is naturally a new request.
    pub fn refresh(
        &mut self,
        visible: MapBlockBounds,
        map: MapBlockBounds,
        selected: BlockLod,
        revision: ImmutableRevision,
        mut ready: impl FnMut(CompositeKey) -> bool,
    ) {
        let Some(tier) = CompositeTier::from_lod(selected) else {
            self.queue.reconcile(|_| false);
            self.orders.clear();
            self.prepared.clear();
            self.previous_visible = Some(visible);
            return;
        };
        let centre = visible.centre();
        let ahead = self.previous_visible.and_then(|was| visible.ahead_of(was, map));
        // A camera can reverse before its prefetch has started.  Pending work
        // for the old direction is neither visible nor ahead now, so retaining
        // it would let stale prefetch starve the entered blocks.  In-flight
        // work is left to its producer: completion is cheap and cannot be
        // cancelled safely after it has begun.
        self.queue.reconcile(|key| {
            key.tier == tier
                && key.revision == revision
                && (visible.contains(key.block) || ahead.is_some_and(|bounds| bounds.contains(key.block)))
                // A preparation gate may have concluded that this immutable
                // block deliberately stays LOD0 (for example it contains a
                // slope). Drop the existing pending record as well as refusing
                // a new request below, otherwise that conclusion would leave
                // a never-preparable entry resident forever.
                && !ready(key)
        });
        self.orders.retain(|key, _| self.queue.contains_pending(*key));
        self.prepared.retain(|key, _| self.queue.contains_pending(*key));
        for block in visible.blocks() {
            self.request(
                block,
                tier,
                revision,
                CompositePriority::Visible,
                centre,
                &mut ready,
            );
        }
        if let Some(ahead) = ahead {
            for block in ahead.blocks() {
                self.request(
                    block,
                    tier,
                    revision,
                    CompositePriority::Ahead,
                    centre,
                    &mut ready,
                );
            }
        }
        self.previous_visible = Some(visible);
    }

    fn request(
        &mut self,
        block: BlockCoord,
        tier: CompositeTier,
        revision: ImmutableRevision,
        priority: CompositePriority,
        centre: (i32, i32),
        ready: &mut impl FnMut(CompositeKey) -> bool,
    ) {
        let key = CompositeKey {
            block,
            tier,
            revision,
        };
        if ready(key) || self.queue.contains_in_flight(key) {
            return;
        }
        let distance =
            i32::abs(signed(block.x) - centre.0) as u32 + i32::abs(signed(block.y) - centre.1) as u32;
        let order = QueueOrder {
            priority,
            distance,
            key,
        };
        if let Some(existing) = self.orders.get_mut(&key) {
            *existing = (*existing).min(order);
            return;
        }
        if self.queue.request(key) {
            self.orders.insert(key, order);
            return;
        }
        let Some(worst) = self.orders.values().copied().max() else {
            return;
        };
        if order >= worst || !self.queue.drop_pending(worst.key) {
            return;
        }
        self.orders.remove(&worst.key);
        self.prepared.remove(&worst.key);
        if self.queue.request(key) {
            self.orders.insert(key, order);
        }
    }

    /// Gives at most the configured work budget to an asynchronous producer.
    ///
    /// Calling this does no rasterisation or upload.  A caller that has no
    /// idle/worker producer leaves the requests pending; it must not call a
    /// large compose operation from its camera frame to empty this queue.
    pub fn take_for_frame(&mut self) -> Vec<CompositeWork> {
        self.take_prepared_for_frame(|_| true)
    }

    /// Dispatch only jobs previously accepted by [`mark_prepared`](Self::mark_prepared).
    ///
    /// This is the producer path for map-block LOD.  It makes the preparation
    /// gate structural: an atlas-page-limit or other preparation failure cannot
    /// move the job into `in_flight`, so the visible renderer has only its LOD0
    /// fallback for that block.
    pub fn take_marked_prepared_for_frame(&mut self) -> Vec<PreparedCompositeWork> {
        let queue = &mut self.queue;
        let orders = &self.orders;
        let prepared = &self.prepared;
        let keys = queue.take_for_producer_if(
            |key| prepared.contains_key(&key),
            |left, right| orders[left].cmp(&orders[right]),
        );
        let mut work = Vec::with_capacity(keys.len());
        for key in keys {
            let order = self.orders.remove(&key).expect("a queued key has an order");
            let ground = self
                .prepared
                .remove(&key)
                .expect("selected prepared job retains its source proof");
            work.push(PreparedCompositeWork {
                work: CompositeWork {
                    key,
                    priority: order.priority,
                },
                ground,
            });
        }
        work
    }

    /// Dispatch only work whose immutable inputs are ready to be rendered.
    ///
    /// Unlike a cancellation, a `false` answer leaves the request pending in
    /// its stable queue position.  The caller can therefore prefetch map art
    /// and append atlas pages before it hands a job to the offscreen producer;
    /// a visible block that is not ready simply continues through LOD 0.
    ///
    /// `prepared` is deliberately checked before a key enters `in_flight`.
    /// Once dispatched, a producer owns its source data and the queue may no
    /// longer safely assume that abandoning it is free.
    pub fn take_prepared_for_frame(
        &mut self,
        mut prepared: impl FnMut(CompositeWork) -> bool,
    ) -> Vec<CompositeWork> {
        let queue = &mut self.queue;
        let orders = &self.orders;
        let keys = queue.take_for_producer_if(
            |key| {
                let order = orders[&key];
                prepared(CompositeWork {
                    key,
                    priority: order.priority,
                })
            },
            |left, right| orders[left].cmp(&orders[right]),
        );
        keys.into_iter()
            .map(|key| {
                let order = self.orders.remove(&key).expect("a queued key has an order");
                self.prepared.remove(&key);
                CompositeWork {
                    key,
                    priority: order.priority,
                }
            })
            .collect()
    }

    /// Releases an asynchronous job after its result has been accepted or
    /// discarded.  The next `refresh` can request a retry if its exact key is
    /// still not in the cache.
    pub fn finished(&mut self, key: CompositeKey) {
        self.queue.finish(key);
    }

    /// Cancel all pending and dispatched work for one changed map block.
    ///
    /// Removing an in-flight key is intentional: a producer that completes
    /// after the mutation reaches [`finish_into_cache`](Self::finish_into_cache)
    /// or [`finish_capture`](Self::finish_capture), sees that its reservation
    /// is gone, and discards its stale result instead of reviving old pixels.
    pub fn invalidate_block(&mut self, block: BlockCoord) -> usize {
        self.invalidate_matching(|key| key.block == block)
    }

    /// Cancel selected cached LOD jobs for one changed map block.
    pub fn invalidate_block_tiers(&mut self, block: BlockCoord, tiers: &[CompositeTier]) -> usize {
        self.invalidate_matching(|key| key.block == block && tiers.contains(&key.tier))
    }

    /// Cancel all work in an affected map-block rectangle.
    pub fn invalidate_blocks(&mut self, blocks: MapBlockBounds) -> usize {
        self.invalidate_matching(|key| blocks.contains(key.block))
    }

    /// Cancel selected LOD work in an affected map-block rectangle.
    pub fn invalidate_block_tiers_in(&mut self, blocks: MapBlockBounds, tiers: &[CompositeTier]) -> usize {
        self.invalidate_matching(|key| blocks.contains(key.block) && tiers.contains(&key.tier))
    }

    /// Cancel every queued and dispatched job, for a global source change such
    /// as a world-output-format reconfiguration.
    pub fn clear(&mut self) -> usize {
        let removed = self.queue.clear();
        self.orders.clear();
        self.prepared.clear();
        removed
    }

    fn invalidate_matching(&mut self, mut stale: impl FnMut(&CompositeKey) -> bool) -> usize {
        let removed = self.queue.invalidate_matching(|key| stale(key));
        self.orders.retain(|key, _| !stale(key));
        self.prepared.retain(|key, _| !stale(key));
        removed
    }

    /// Accept a completed asynchronous image into the cache and release its
    /// queue slot.
    ///
    /// The exact key must have been handed out by [`Self::take_for_frame`].
    /// This prevents a late result for a cancelled/stale request from quietly
    /// replacing a newer cache entry.  Rasterising `pixels` is deliberately
    /// outside this method: producers may use an idle worker or a streamed map
    /// cell budget, while this small upload is the one atomic completion step.
    pub fn finish_into_cache<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cache: &'a mut CompositeCache,
        key: CompositeKey,
        pixels: CompositePixels,
    ) -> Option<&'a CompositeTexture> {
        self.queue
            .finish(key)
            .then(|| cache.insert(device, queue, key, pixels))
    }

    /// Complete one dispatched job by copying the immutable map portion of the
    /// current frame into GPU-resident cache planes.  This is intentionally a
    /// no-op for a key that was not dispatched: a late capture must not make a
    /// stale block authoritative.
    #[allow(clippy::too_many_arguments)] // GPU capture must name its queue, encoder and cache ownership separately.
    pub fn finish_capture<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut CompositeRenderer,
        cache: &'a mut CompositeCache,
        key: CompositeKey,
        source: CaptureSource<'_>,
        ground: FlatGroundBlock,
    ) -> Option<&'a CompositeTexture> {
        if !self.queue.finish(key) {
            return None;
        }
        if cache.is_rejected(key.block) {
            return None;
        }
        let captured = cache.capture(device, key, source, ground);
        renderer.capture_planes(device, queue, encoder, source, captured);
        renderer.capture_depth(device, queue, encoder, source, captured);
        Some(captured)
    }
}

/// One cached image placed in the current world target.
#[derive(Clone, Copy, Debug)]
pub struct CompositeQuad<'a> {
    /// The cache image.  A caller obtains this only after a background producer
    /// has completed it; requesting or composing work does not happen here.
    pub texture: &'a CompositeTexture,
    /// The image's full screen-space rectangle in virtual target pixels.
    pub rect: Rect,
}

/// Draws each cached map block as one textured quad.
///
/// This pass writes only the colour image and uses source-over alpha blending.
/// The main world's G-buffer and depth are deliberately not guessed at a block
/// granularity; the next work item owns the policy that lets dynamic objects
/// interleave with cached map pixels.
#[derive(Debug)]
pub struct CompositeRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    deferred_pipeline: wgpu::RenderPipeline,
    deferred_layout: wgpu::BindGroupLayout,
    capture_depth_pipeline: wgpu::RenderPipeline,
    capture_depth_layout: wgpu::BindGroupLayout,
    capture_planes_pipeline: wgpu::RenderPipeline,
    capture_planes_layout: wgpu::BindGroupLayout,
    capture_uniform: wgpu::Buffer,
    viewport: wgpu::Buffer,
    quad: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: u64,
    /// One immutable binding pair per deferred call encoded into the current
    /// submission. Source-depth adjustment is per instance, so the camera
    /// needs one call for all source depths; separate calls still cannot share
    /// a buffer before the encoder has been submitted.
    deferred_batches: Vec<DeferredBatch>,
    deferred_batch_cursor: usize,
    deferred_bindings_created: usize,
    deferred_bindings_reused: usize,
    deferred_cpu: DeferredCpuCosts,
    sampler: wgpu::Sampler,
}

/// CPU time spent in the deferred composite restore itself.
///
/// This intentionally splits command recording from the surrounding world
/// pass: cached terrain may use a multi-attachment pass, so a high aggregate
/// `encode_composites` needs to say whether uploads, binding work, or wgpu pass
/// encoding is responsible before its representation is changed.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeferredCpuCosts {
    pub upload: Duration,
    pub bindings: Duration,
    pub pass: Duration,
}

/// Buffers held immutable by one deferred call in an encoded frame.
#[derive(Debug)]
struct DeferredBatch {
    viewport: wgpu::Buffer,
    instances: wgpu::Buffer,
    capacity: u64,
    /// Bind groups retain their source textures. Retain only this call's
    /// visible blocks so an evicted composite image is not kept alive here.
    bindings: Vec<(CompositeKey, wgpu::BindGroup)>,
}

fn write_capture_uniform(
    queue: &wgpu::Queue,
    buffer: &wgpu::Buffer,
    size: CompositeSize,
    source: crate::blit::ViewportRect,
    block: BlockCoord,
) {
    let values = [
        size.width as f32,
        size.height as f32,
        source.x as f32,
        source.y as f32,
        source.width as f32,
        source.height as f32,
        f32::from(tile_origin(block).0),
        f32::from(tile_origin(block).1),
        0.0,
        0.0,
    ];
    let bytes: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
    queue.write_buffer(buffer, 0, &bytes);
}

impl CompositeRenderer {
    /// Create the colour-only cached-composite pass.
    pub fn new(device: &wgpu::Device) -> Self {
        let viewport = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite viewport"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map block composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("map block composite sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("map block composite"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!(concat!(env!("OUT_DIR"), "/composite.wgsl")).into(),
            ),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map block composite"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("map block composite"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 16,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 0,
                            shader_location: 1,
                        }],
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let deferred_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map block deferred composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let deferred_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("map block deferred composite"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!(concat!(env!("OUT_DIR"), "/composite_deferred.wgsl")).into(),
            ),
        });
        let deferred_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map block deferred composite"),
            bind_group_layouts: &[Some(&deferred_layout)],
            immediate_size: 0,
        });
        let vertex_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                }],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 20,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 0,
                        shader_location: 1,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32,
                        offset: 16,
                        shader_location: 2,
                    },
                ],
            }),
        ];
        let deferred_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("map block deferred composite"),
            layout: Some(&deferred_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &deferred_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &vertex_buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &deferred_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: WORLD_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(crate::renderer::IDS_TARGET),
                    Some(crate::renderer::POSITION_TARGET),
                    Some(crate::renderer::NORMAL_TARGET),
                ],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: Some(crate::renderer::depth_state()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let capture_depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map composite depth capture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let capture_depth_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("map composite depth capture"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!(concat!(env!("OUT_DIR"), "/composite_capture_depth.wgsl")).into(),
            ),
        });
        let capture_depth_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map composite depth capture"),
            bind_group_layouts: &[Some(&capture_depth_layout)],
            immediate_size: 0,
        });
        let capture_depth_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("map composite depth capture"),
            layout: Some(&capture_depth_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &capture_depth_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &capture_depth_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::R32Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let capture_planes_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map composite plane capture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let capture_planes_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("map composite plane capture"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!(concat!(env!("OUT_DIR"), "/composite_capture_planes.wgsl")).into(),
            ),
        });
        let capture_planes_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map composite plane capture"),
            bind_group_layouts: &[Some(&capture_planes_layout)],
            immediate_size: 0,
        });
        let capture_planes_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("map composite plane capture"),
            layout: Some(&capture_planes_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &capture_planes_shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    }],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &capture_planes_shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: WORLD_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(crate::renderer::IDS_TARGET),
                    Some(crate::renderer::POSITION_TARGET),
                    Some(crate::renderer::NORMAL_TARGET),
                ],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let capture_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map composite depth capture"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let quad = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map block composite unit quad"),
            size: 4 * 2 * 4,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: true,
        });
        let mut bytes = Vec::with_capacity(4 * 2 * 4);
        for value in [0.0_f32, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        quad.slice(..)
            .get_mapped_range_mut()
            .expect("a freshly mapped buffer has its whole range")
            .copy_from_slice(&bytes);
        quad.unmap();
        let instances = Self::instance_buffer(device, 1);
        Self {
            pipeline,
            layout,
            deferred_pipeline,
            deferred_layout,
            capture_depth_pipeline,
            capture_depth_layout,
            capture_planes_pipeline,
            capture_planes_layout,
            capture_uniform,
            viewport,
            quad,
            instances,
            capacity: 1,
            deferred_batches: Vec::new(),
            deferred_batch_cursor: 0,
            deferred_bindings_created: 0,
            deferred_bindings_reused: 0,
            deferred_cpu: DeferredCpuCosts::default(),
            sampler,
        }
    }

    fn instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map block composite instances"),
            size: capacity * 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn deferred_instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("map block deferred composite instances"),
            size: capacity * 20,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn deferred_batch(device: &wgpu::Device, capacity: u64) -> DeferredBatch {
        DeferredBatch {
            viewport: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("map block deferred composite viewport"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            instances: Self::deferred_instance_buffer(device, capacity),
            capacity,
            bindings: Vec::new(),
        }
    }

    /// Start recording a fresh camera frame.
    pub fn begin_frame(&mut self) {
        self.deferred_batch_cursor = 0;
        self.deferred_bindings_created = 0;
        self.deferred_bindings_reused = 0;
        self.deferred_cpu = DeferredCpuCosts::default();
    }

    /// Bindings created and reused by deferred restoration in this frame.
    pub const fn deferred_binding_stats(&self) -> (usize, usize) {
        (self.deferred_bindings_created, self.deferred_bindings_reused)
    }

    /// Per-frame CPU cost of the deferred composite restore.
    pub const fn deferred_cpu_costs(&self) -> DeferredCpuCosts {
        self.deferred_cpu
    }

    fn next_deferred_batch(&mut self, device: &wgpu::Device, instances: u64) -> usize {
        let index = self.deferred_batch_cursor;
        self.deferred_batch_cursor += 1;
        if self.deferred_batches.len() == index {
            self.deferred_batches
                .push(Self::deferred_batch(device, instances));
        } else if instances > self.deferred_batches[index].capacity {
            self.deferred_batches[index] = Self::deferred_batch(device, instances.next_power_of_two());
        }
        index
    }

    /// Draw all ready blocks as one quad each over an already-cleared colour
    /// target.  No rebuild, upload, or cache lookup occurs in this method.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        target_size: CompositeSize,
        blocks: &[CompositeQuad<'_>],
    ) {
        if blocks.is_empty() {
            return;
        }
        let mut viewport = Vec::with_capacity(16);
        for value in [target_size.width as f32, target_size.height as f32, 0.0, 0.0] {
            viewport.extend_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.viewport, 0, &viewport);
        if blocks.len() as u64 > self.capacity {
            self.capacity = (blocks.len() as u64).next_power_of_two();
            self.instances = Self::instance_buffer(device, self.capacity);
        }
        let mut instances = Vec::with_capacity(blocks.len() * 16);
        for block in blocks {
            for value in [block.rect.x, block.rect.y, block.rect.width, block.rect.height] {
                instances.extend_from_slice(&value.to_le_bytes());
            }
        }
        queue.write_buffer(&self.instances, 0, &instances);

        // A render pass borrows every bind group it uses until the pass ends.
        // Build all groups first so a short-lived loop variable cannot leave a
        // texture binding dangling between two block draws.
        let bind_groups: Vec<_> = blocks
            .iter()
            .map(|block| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("map block composite"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.viewport.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(block.texture.view()),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                })
            })
            .collect();

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map block composites"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, self.instances.slice(..));
        for (index, bind_group) in bind_groups.iter().enumerate() {
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..4, index as u32..index as u32 + 1);
        }
    }

    /// Restore completed map composites into the normal world attachments.
    ///
    /// This is deliberately a depth-writing pass, not an overlay: callers run
    /// it before server items and mobiles, so those live producers still test
    /// against exactly the map surface they would have met at LOD 0.
    pub fn render_deferred(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: crate::renderer::Target<'_>,
        depth_adjust: f32,
        blocks: &[CompositeQuad<'_>],
    ) {
        self.render_deferred_with(device, queue, encoder, target, blocks, |_| depth_adjust);
    }

    /// Restore blocks captured from potentially different source eyes in one
    /// deferred batch. The correction is serialized into each instance, so
    /// callers avoid one command-encoding group per source depth base.
    pub fn render_deferred_rebased(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: crate::renderer::Target<'_>,
        current_depth_base: i32,
        blocks: &[CompositeQuad<'_>],
    ) {
        self.render_deferred_with(device, queue, encoder, target, blocks, |block| {
            crate::depth::rebase_adjust(block.texture.depth_base(), current_depth_base)
        });
    }

    fn render_deferred_with(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: crate::renderer::Target<'_>,
        blocks: &[CompositeQuad<'_>],
        depth_adjust: impl Fn(&CompositeQuad<'_>) -> f32,
    ) {
        let blocks: Vec<_> = blocks
            .iter()
            .filter(|block| block.texture.deferred_views().is_some())
            .collect();
        if blocks.is_empty() {
            return;
        }
        let upload_started = Instant::now();
        let mut viewport = Vec::with_capacity(16);
        for value in [target.width as f32, target.height as f32, 0.0, 0.0] {
            viewport.extend_from_slice(&value.to_le_bytes());
        }
        // A source block's depth base differs from its neighbours'.  The old
        // path split those bases into calls because this adjustment lived in a
        // uniform, creating one queue write per group.  It now travels in the
        // block's instance row, so one call can restore every ready block.
        // The batch itself remains distinct per call: queue writes made before
        // one submit are visible to all commands in it, which is the artifact
        // this isolation was introduced to prevent.
        let batch_index = self.next_deferred_batch(device, blocks.len() as u64);
        let batch = &self.deferred_batches[batch_index];
        queue.write_buffer(&batch.viewport, 0, &viewport);
        let mut instances = Vec::with_capacity(blocks.len() * 20);
        for block in &blocks {
            for value in [block.rect.x, block.rect.y, block.rect.width, block.rect.height] {
                instances.extend_from_slice(&value.to_le_bytes());
            }
            instances.extend_from_slice(&depth_adjust(block).to_le_bytes());
        }
        queue.write_buffer(&batch.instances, 0, &instances);
        self.deferred_cpu.upload += upload_started.elapsed();
        let bindings_started = Instant::now();
        let batch = &mut self.deferred_batches[batch_index];
        batch
            .bindings
            .retain(|(key, _)| blocks.iter().any(|block| block.texture.key() == *key));
        let mut bindings_created = 0;
        let mut bindings_reused = 0;
        for block in &blocks {
            if batch.bindings.iter().any(|(key, _)| *key == block.texture.key()) {
                bindings_reused += 1;
                continue;
            }
            bindings_created += 1;
            let (ids, position, normal, depth) = block.texture.deferred_views().expect("filtered above");
            batch.bindings.push((
                block.texture.key(),
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("map block deferred composite"),
                    layout: &self.deferred_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: batch.viewport.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(block.texture.view()),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(ids),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::TextureView(position),
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(normal),
                        },
                        wgpu::BindGroupEntry {
                            binding: 5,
                            resource: wgpu::BindingResource::TextureView(depth),
                        },
                    ],
                }),
            ));
        }
        let bindings_cost = bindings_started.elapsed();
        let pass_started = Instant::now();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map block deferred composites"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &target.gbuffer.ids,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &target.gbuffer.position,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: &target.gbuffer.normal,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.deferred_pipeline);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.set_vertex_buffer(1, batch.instances.slice(..));
        for (index, block) in blocks.iter().enumerate() {
            let (_, bind_group) = batch
                .bindings
                .iter()
                .find(|(key, _)| *key == block.texture.key())
                .expect("every deferred block has a cached binding");
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..4, index as u32..index as u32 + 1);
        }
        drop(pass);
        self.deferred_bindings_created += bindings_created;
        self.deferred_bindings_reused += bindings_reused;
        self.deferred_cpu.bindings += bindings_cost;
        self.deferred_cpu.pass += pass_started.elapsed();
    }

    /// Capture colour and the three G-buffer planes into one cached texture.
    fn capture_planes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: CaptureSource<'_>,
        captured: &CompositeTexture,
    ) {
        let Some((ids, position, normal, _)) = captured.deferred_views() else {
            return;
        };
        let size = captured.size;
        write_capture_uniform(
            queue,
            &self.capture_uniform,
            size,
            source.rect,
            captured.key().block,
        );
        let color = source.color.create_view(&wgpu::TextureViewDescriptor::default());
        let source_ids = source.ids.create_view(&wgpu::TextureViewDescriptor::default());
        let source_position = source
            .position
            .create_view(&wgpu::TextureViewDescriptor::default());
        let source_normal = source.normal.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map composite plane capture"),
            layout: &self.capture_planes_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.capture_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&source_ids),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&source_position),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&source_normal),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map composite plane capture"),
            color_attachments: &[
                Some(wgpu::RenderPassColorAttachment {
                    view: captured.view(),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: ids,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(crate::gbuffer::IDS_CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: position,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
                Some(wgpu::RenderPassColorAttachment {
                    view: normal,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                }),
            ],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.capture_planes_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.draw(0..4, 0..1);
    }

    /// Write the source depth rectangle into a captured entry's float plane.
    /// The colour and G-buffer planes were captured by [`Self::capture_planes`];
    /// this pass exists solely because WebGPU
    /// does not permit a direct `Depth24Plus` to `R32Float` texture copy.
    fn capture_depth(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: CaptureSource<'_>,
        captured: &CompositeTexture,
    ) {
        let Some((_, _, _, depth)) = captured.deferred_textures() else {
            return;
        };
        let size = captured.size;
        write_capture_uniform(
            queue,
            &self.capture_uniform,
            size,
            source.rect,
            captured.key().block,
        );
        let source_ids = source.ids.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map composite depth capture"),
            layout: &self.capture_depth_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.capture_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(source.depth),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&source_ids),
                },
            ],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("map composite depth capture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &depth_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.capture_depth_pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad.slice(..));
        pass.draw(0..4, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::BlockExtent;

    use super::*;

    /// A GPU suitable for the complete capture-and-restore oracle, or `None`
    /// on a headless machine whose adapter cannot expose the client's G-buffer
    /// attachments.
    fn renderable_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
        if !adapter
            .get_texture_format_features(crate::gbuffer::POSITION_FORMAT)
            .allowed_usages
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
        {
            return None;
        }
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: crate::gbuffer::required_limits(),
            ..Default::default()
        }))
        .ok()
    }

    /// Make a source attachment for the capture half of the oracle.  Unlike a
    /// normal world attachment it only needs to be written by the fixture and
    /// sampled by the capture shader.
    fn source_texture(
        device: &wgpu::Device,
        label: &'static str,
        format: wgpu::TextureFormat,
        size: u32,
    ) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    /// Read one four-byte attachment, padding rows when its width does not
    /// happen to meet WebGPU's copy alignment.
    fn read_attachment(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
        read_attachment_with_texel_bytes(device, queue, texture, 4)
    }

    /// Read one attachment with an explicitly supplied packed texel size.
    fn read_attachment_with_texel_bytes(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        bytes_per_texel: u32,
    ) -> Vec<u8> {
        let row = texture.width() * bytes_per_texel;
        let stride = row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let bytes = u64::from(stride) * u64::from(texture.height());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("composite test readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: Some(texture.height()),
                },
            },
            wgpu::Extent3d {
                width: texture.width(),
                height: texture.height(),
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |result| {
            result.expect("the test submitted the copy")
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the test's readback completes");
        let padded = slice
            .get_mapped_range()
            .expect("the completed mapping has bytes")
            .to_vec();
        readback.unmap();
        padded
            .chunks_exact(stride as usize)
            .flat_map(|source_row| source_row[..row as usize].iter().copied())
            .collect()
    }

    /// A deliberately tiny stand-in for the ordinary item/mobile pass.  It
    /// exercises the one contract composites must preserve for every later
    /// dynamic renderer: the restored map depth remains a normal depth target.
    fn dynamic_depth_pipeline(device: &wgpu::Device, depth: f32, color: [f32; 4]) -> wgpu::RenderPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("composite dynamic-depth test"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "
                    struct VertexOut {{ @builtin(position) position: vec4<f32> }};

                    @vertex
                    fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {{
                        var points = array<vec2<f32>, 4>(
                            vec2(-1.0, -1.0), vec2(1.0, -1.0),
                            vec2(-1.0, 1.0), vec2(1.0, 1.0));
                        var out: VertexOut;
                        out.position = vec4(points[index], {depth}, 1.0);
                        return out;
                    }}

                    @fragment
                    fn fs_main() -> @location(0) vec4<f32> {{
                        return vec4({}, {}, {}, {});
                    }}",
                    color[0], color[1], color[2], color[3]
                )
                .into(),
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite dynamic-depth test"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("composite dynamic-depth test"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::blit::WORLD_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::renderer::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    }

    fn render_dynamic_depth_test(
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        world: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("composite dynamic-depth test"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: world,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            multiview_mask: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipeline);
        pass.draw(0..4, 0..1);
    }

    #[test]
    fn deferred_groups_in_one_submit_keep_their_own_rectangles() {
        let Some((device, queue)) = renderable_device() else {
            return;
        };
        const SIZE: u32 = 64;
        const BLOCK: u32 = SIZE / 2;
        let block_size = CompositeSize::new(BLOCK, SIZE).unwrap();
        let deferred = |color: [u8; 4], depth_base| {
            let mut rgba = Vec::with_capacity((BLOCK * SIZE * 4) as usize);
            for _ in 0..BLOCK * SIZE {
                rgba.extend_from_slice(&color);
            }
            let texels = (BLOCK * SIZE) as usize;
            let pixels = CompositePixels::new(block_size, rgba).unwrap();
            pixels
                .with_deferred(
                    DeferredPixels::new(
                        block_size,
                        vec![crate::gbuffer::pack_ids(
                            0,
                            crate::place::Stance::Flat,
                            crate::place::Kind::Land,
                        ); texels],
                        vec![0.0; texels * 4],
                        vec![crate::gbuffer::NORMAL_DRAWN; texels],
                        vec![0.5; texels],
                        depth_base,
                    )
                    .unwrap(),
                )
                .unwrap()
        };
        let left_key = CompositeKey {
            block: BlockCoord { x: 0, y: 0 },
            tier: CompositeTier::Lod1,
            revision: ImmutableRevision::default(),
        };
        let right_key = CompositeKey {
            block: BlockCoord { x: 1, y: 0 },
            tier: CompositeTier::Lod1,
            revision: ImmutableRevision::default(),
        };
        let mut cache = CompositeCache::default();
        cache.insert(&device, &queue, left_key, deferred([213, 29, 17, u8::MAX], 0));
        cache.insert(&device, &queue, right_key, deferred([17, 71, 213, u8::MAX], 8));

        let restored = crate::blit::world_texture(&device, SIZE, SIZE);
        let restored_view = restored.create_view(&wgpu::TextureViewDescriptor::default());
        let restored_gbuffer = crate::gbuffer::Gbuffer::new(&device, SIZE, SIZE);
        let restored_views = restored_gbuffer.views();
        let restored_depth = crate::renderer::depth_texture(&device, SIZE, SIZE);
        let restored_depth_view = restored_depth.create_view(&wgpu::TextureViewDescriptor::default());
        let target =
            crate::renderer::Target::whole(&restored_view, &restored_depth_view, &restored_views, SIZE, SIZE);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("deferred composite group test clear"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &restored_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &restored_views.ids,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(crate::gbuffer::IDS_CLEAR),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &restored_views.position,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(crate::gbuffer::POSITION_CLEAR),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &restored_views.normal,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(crate::gbuffer::NORMAL_CLEAR),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &restored_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        let mut composite = CompositeRenderer::new(&device);
        composite.begin_frame();
        let left = cache.get(left_key).unwrap();
        composite.render_deferred(
            &device,
            &queue,
            &mut encoder,
            target,
            0.0,
            &[CompositeQuad {
                texture: left,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: BLOCK as f32,
                    height: SIZE as f32,
                },
            }],
        );
        let right = cache.get(right_key).unwrap();
        composite.render_deferred(
            &device,
            &queue,
            &mut encoder,
            target,
            0.01,
            &[CompositeQuad {
                texture: right,
                rect: Rect {
                    x: BLOCK as f32,
                    y: 0.0,
                    width: BLOCK as f32,
                    height: SIZE as f32,
                },
            }],
        );
        queue.submit([encoder.finish()]);
        assert_eq!(composite.deferred_batches.len(), 2);
        composite.begin_frame();
        assert_eq!(composite.next_deferred_batch(&device, 1), 0);
        assert_eq!(
            composite.deferred_batches.len(),
            2,
            "a new submitted frame reuses its first deferred binding slot"
        );

        let colors = read_attachment(&device, &queue, &restored);
        let pixel = |x, y| {
            let at = ((y * SIZE + x) * 4) as usize;
            [colors[at], colors[at + 1], colors[at + 2], colors[at + 3]]
        };
        assert_eq!(pixel(BLOCK / 2, SIZE / 2), [213, 29, 17, u8::MAX]);
        assert_eq!(pixel(BLOCK + BLOCK / 2, SIZE / 2), [17, 71, 213, u8::MAX]);

        // The camera path uses one call even when its blocks were captured
        // from different depth bases.  This is deliberately after the two-call
        // check above: both the old artifact guard and the coalesced far-zoom
        // path must remain valid.
        let mut rebased = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        composite.begin_frame();
        composite.render_deferred_rebased(
            &device,
            &queue,
            &mut rebased,
            target,
            0,
            &[
                CompositeQuad {
                    texture: left,
                    rect: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: BLOCK as f32,
                        height: SIZE as f32,
                    },
                },
                CompositeQuad {
                    texture: right,
                    rect: Rect {
                        x: BLOCK as f32,
                        y: 0.0,
                        width: BLOCK as f32,
                        height: SIZE as f32,
                    },
                },
            ],
        );
        queue.submit([rebased.finish()]);
        assert_eq!(
            composite.deferred_batches.len(),
            2,
            "one rebased call reuses one submitted-frame slot"
        );
        assert_eq!(
            composite.deferred_batches[0].bindings.len(),
            2,
            "each visible source has one reusable deferred binding"
        );
        let mut trimmed = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        composite.begin_frame();
        composite.render_deferred_rebased(
            &device,
            &queue,
            &mut trimmed,
            target,
            0,
            &[CompositeQuad {
                texture: left,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: BLOCK as f32,
                    height: SIZE as f32,
                },
            }],
        );
        queue.submit([trimmed.finish()]);
        assert_eq!(
            composite.deferred_batches[0].bindings.len(),
            1,
            "a block outside the next frame cannot retain an evictable texture"
        );
        let colors = read_attachment(&device, &queue, &restored);
        let pixel = |x, y| {
            let at = ((y * SIZE + x) * 4) as usize;
            [colors[at], colors[at + 1], colors[at + 2], colors[at + 3]]
        };
        assert_eq!(pixel(BLOCK / 2, SIZE / 2), [213, 29, 17, u8::MAX]);
        assert_eq!(pixel(BLOCK + BLOCK / 2, SIZE / 2), [17, 71, 213, u8::MAX]);
    }

    #[test]
    fn gpu_capture_pipelines_construct_when_a_renderable_adapter_is_available() {
        let Some((device, _)) = renderable_device() else {
            return;
        };
        let _ = CompositeRenderer::new(&device);
    }

    /// Producer geometry is the single ownership authority. This fixture
    /// models an 8x8 producer that emitted its left half only; capture and
    /// restore must preserve that exact coverage without consulting the
    /// interpolated G-buffer position a second time.
    #[test]
    fn captured_block_restores_only_its_producer_geometry() {
        let Some((device, queue)) = renderable_device() else {
            return;
        };
        const SIZE: u32 = 64;
        let color = source_texture(&device, "composite test color", crate::blit::WORLD_FORMAT, SIZE);
        let ids = source_texture(&device, "composite test ids", crate::gbuffer::IDS_FORMAT, SIZE);
        let position = source_texture(
            &device,
            "composite test position",
            crate::gbuffer::POSITION_FORMAT,
            SIZE,
        );
        let normal = source_texture(
            &device,
            "composite test normal",
            crate::gbuffer::NORMAL_FORMAT,
            SIZE,
        );

        let mut colors = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        let mut source_ids = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        let mut positions = Vec::with_capacity((SIZE * SIZE * 16) as usize);
        let mut normals = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        let map_id = crate::gbuffer::pack_ids(7, crate::place::Stance::Flat, crate::place::Kind::Land);
        for _y in 0..SIZE {
            for x in 0..SIZE {
                let owned = x < SIZE / 2;
                colors.extend_from_slice(if owned {
                    &[219, 31, 17, u8::MAX]
                } else {
                    &[17, 49, 211, u8::MAX]
                });
                let id = if owned { map_id } else { 0 };
                source_ids.extend_from_slice(&id.to_le_bytes());
                // Position may cross an apparent tile boundary. It must not
                // turn a valid producer pixel into a cache hole: IDs encode
                // source coverage, while tile ownership was settled before
                // rasterisation by the producer's geometry collection.
                for value in [if owned { 3.0_f32 } else { 8.0 }, 3.0, 0.0, 0.0] {
                    positions.extend_from_slice(&value.to_le_bytes());
                }
                normals.extend_from_slice(&crate::gbuffer::NORMAL_DRAWN.to_le_bytes());
            }
        }
        let write = |texture: &wgpu::Texture, bytes: &[u8], bytes_per_row| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(SIZE),
                },
                wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
            );
        };
        write(&color, &colors, 4 * SIZE);
        write(&ids, &source_ids, 4 * SIZE);
        write(&position, &positions, 16 * SIZE);
        write(&normal, &normals, 4 * SIZE);

        let source_depth = crate::renderer::depth_texture(&device, SIZE, SIZE);
        let source_depth_view = source_depth.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite test source depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &source_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.5),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        let block = BlockCoord { x: 0, y: 0 };
        let key = CompositeKey {
            block,
            tier: CompositeTier::Lod1,
            revision: ImmutableRevision::default(),
        };
        let bounds = MapBlockBounds {
            min_x: 0,
            max_x: 0,
            min_y: 0,
            max_y: 0,
        };
        let mut work = CompositeWorkQueue::new(1, 1).unwrap();
        work.refresh(bounds, bounds, BlockLod::Lod1, key.revision, |_| false);
        assert_eq!(
            work.take_for_frame(),
            vec![CompositeWork {
                key,
                priority: CompositePriority::Visible
            }]
        );
        let mut cache = CompositeCache::default();
        let mut composite = CompositeRenderer::new(&device);
        let source = CaptureSource {
            color: &color,
            ids: &ids,
            position: &position,
            normal: &normal,
            depth: &source_depth_view,
            depth_base: 0,
            rect: crate::blit::ViewportRect {
                x: 0,
                y: 0,
                width: SIZE,
                height: SIZE,
            },
        };
        assert!(
            work.finish_capture(
                &device,
                &queue,
                &mut encoder,
                &mut composite,
                &mut cache,
                key,
                source,
                CompositeProducerJob::new(key).ground(),
            )
            .is_some()
        );

        let restored = crate::blit::world_texture(&device, SIZE, SIZE);
        let restored_view = restored.create_view(&wgpu::TextureViewDescriptor::default());
        let restored_gbuffer = crate::gbuffer::Gbuffer::new(&device, SIZE, SIZE);
        let restored_views = restored_gbuffer.views();
        let restored_depth = crate::renderer::depth_texture(&device, SIZE, SIZE);
        let restored_depth_view = restored_depth.create_view(&wgpu::TextureViewDescriptor::default());
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite test restore clear"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &restored_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &restored_views.ids,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(crate::gbuffer::IDS_CLEAR),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &restored_views.position,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(crate::gbuffer::POSITION_CLEAR),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &restored_views.normal,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(crate::gbuffer::NORMAL_CLEAR),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &restored_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        let texture = cache.get(key).expect("the dispatched capture completed");
        composite.render_deferred(
            &device,
            &queue,
            &mut encoder,
            crate::renderer::Target::whole(&restored_view, &restored_depth_view, &restored_views, SIZE, SIZE),
            0.0,
            &[CompositeQuad {
                texture,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: SIZE as f32,
                    height: SIZE as f32,
                },
            }],
        );
        queue.submit([encoder.finish()]);

        let colors = read_attachment(&device, &queue, &restored);
        let restored_ids = read_attachment(&device, &queue, restored_gbuffer.ids());
        let pixel = |bytes: &[u8], x, y| {
            let at = ((y * SIZE + x) * 4) as usize;
            [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]
        };
        assert_eq!(pixel(&colors, 16, 32), [219, 31, 17, u8::MAX]);
        assert_eq!(pixel(&colors, 48, 32), [0, 0, 0, 0]);
        let owner_id = u32::from_le_bytes(pixel(&restored_ids, 16, 32));
        assert_eq!(crate::gbuffer::ids_kind(owner_id), Some(crate::place::Kind::Land));
        assert_ne!(
            crate::gbuffer::ids_id(owner_id) & crate::gbuffer::IDS_COMPOSITE_MAP,
            0
        );
        assert_eq!(u32::from_le_bytes(pixel(&restored_ids, 48, 32)), 0);

        // The same source depth that prevents a real item/mobile behind map
        // geometry must still reject it after the cached block restores.  The
        // neighbour half has no composite depth, so that very same dynamic
        // quad is visible there.  This catches the subtle failure where colour
        // and G-buffer planes are restored but depth is left cleared.
        let behind = dynamic_depth_pipeline(&device, 0.6, [0.0, 1.0, 0.0, 1.0]);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        render_dynamic_depth_test(&mut encoder, &behind, &restored_view, &restored_depth_view);
        queue.submit([encoder.finish()]);
        let behind_colors = read_attachment(&device, &queue, &restored);
        assert_eq!(pixel(&behind_colors, 16, 32), [219, 31, 17, u8::MAX]);
        assert_eq!(pixel(&behind_colors, 48, 32), [0, 255, 0, 255]);

        // Conversely, a nearer dynamic producer wins the normal depth test
        // over the restored map just as it does over LOD0 geometry.
        let in_front = dynamic_depth_pipeline(&device, 0.4, [1.0, 0.0, 1.0, 1.0]);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        render_dynamic_depth_test(&mut encoder, &in_front, &restored_view, &restored_depth_view);
        queue.submit([encoder.finish()]);
        let front_colors = read_attachment(&device, &queue, &restored);
        assert_eq!(pixel(&front_colors, 16, 32), [255, 0, 255, 255]);

        // The producer target is deliberately reusable. Capture another key
        // after replacing *every* source plane, then read the first cached
        // planes themselves. This catches an attachment alias that a screen
        // picture could only suggest after the fact.
        let first = cache.get(key).unwrap();
        let (_, first_ids, first_position, first_normal, first_depth) = (
            read_attachment(&device, &queue, first.texture()),
            read_attachment(
                &device,
                &queue,
                first.deferred_textures().expect("captured planes").0,
            ),
            read_attachment_with_texel_bytes(
                &device,
                &queue,
                first.deferred_textures().expect("captured planes").1,
                16,
            ),
            read_attachment(
                &device,
                &queue,
                first.deferred_textures().expect("captured planes").2,
            ),
            read_attachment(
                &device,
                &queue,
                first.deferred_textures().expect("captured planes").3,
            ),
        );
        let first_color = read_attachment(&device, &queue, first.texture());
        let overwritten: Vec<_> = (0..SIZE * SIZE).flat_map(|_| [0, 255, 0, 255]).collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &overwritten,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * SIZE),
                rows_per_image: Some(SIZE),
            },
            wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            },
        );
        let overwritten_ids: Vec<_> = (0..SIZE * SIZE)
            .flat_map(|_| {
                crate::gbuffer::pack_ids(31, crate::place::Stance::Upright, crate::place::Kind::Static)
                    .to_le_bytes()
            })
            .collect();
        write(&ids, &overwritten_ids, 4 * SIZE);
        let overwritten_position: Vec<_> = (0..SIZE * SIZE)
            .flat_map(|_| [3.5_f32, 3.5, 17.0, 1.0])
            .flat_map(f32::to_le_bytes)
            .collect();
        write(&position, &overwritten_position, 16 * SIZE);
        let overwritten_normals: Vec<_> = (0..SIZE * SIZE).flat_map(|_| 0_u32.to_le_bytes()).collect();
        write(&normal, &overwritten_normals, 4 * SIZE);
        let second_key = CompositeKey {
            revision: ImmutableRevision(1),
            ..key
        };
        let mut second_work = CompositeWorkQueue::new(1, 1).unwrap();
        second_work.refresh(bounds, bounds, BlockLod::Lod1, second_key.revision, |_| false);
        assert_eq!(second_work.take_for_frame().len(), 1);
        let mut second_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _clear = second_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite test second source depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &source_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.25),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        assert!(
            second_work
                .finish_capture(
                    &device,
                    &queue,
                    &mut second_encoder,
                    &mut composite,
                    &mut cache,
                    second_key,
                    CaptureSource {
                        color: &color,
                        ids: &ids,
                        position: &position,
                        normal: &normal,
                        depth: &source_depth_view,
                        depth_base: 0,
                        rect: crate::blit::ViewportRect {
                            x: 0,
                            y: 0,
                            width: SIZE,
                            height: SIZE,
                        },
                    },
                    CompositeProducerJob::new(second_key).ground(),
                )
                .is_some()
        );
        queue.submit([second_encoder.finish()]);
        let first_after = cache.get(key).unwrap();
        let (_, after_ids, after_position, after_normal, after_depth) = (
            read_attachment(&device, &queue, first_after.texture()),
            read_attachment(
                &device,
                &queue,
                first_after.deferred_textures().expect("captured planes").0,
            ),
            read_attachment_with_texel_bytes(
                &device,
                &queue,
                first_after.deferred_textures().expect("captured planes").1,
                16,
            ),
            read_attachment(
                &device,
                &queue,
                first_after.deferred_textures().expect("captured planes").2,
            ),
            read_attachment(
                &device,
                &queue,
                first_after.deferred_textures().expect("captured planes").3,
            ),
        );
        assert_eq!(
            read_attachment(&device, &queue, first_after.texture()),
            first_color
        );
        assert_eq!(after_ids, first_ids);
        assert_eq!(after_position, first_position);
        assert_eq!(after_normal, first_normal);
        assert_eq!(after_depth, first_depth);
    }

    /// At LOD2 a cache texel stands for a 4x4 source footprint. A valid land
    /// fragment in any one of those sixteen pixels must keep the cache texel
    /// alive; sampling just one fixed source pixel recreates visible holes at
    /// the 8x8 block boundary when the cache progressively replaces LOD0.
    #[test]
    fn lod2_capture_conservatively_keeps_sparse_source_coverage() {
        let Some((device, queue)) = renderable_device() else {
            return;
        };
        const SOURCE: u32 = 864;
        let color = source_texture(
            &device,
            "sparse composite color",
            crate::blit::WORLD_FORMAT,
            SOURCE,
        );
        let ids = source_texture(
            &device,
            "sparse composite ids",
            crate::gbuffer::IDS_FORMAT,
            SOURCE,
        );
        let position = source_texture(
            &device,
            "sparse composite position",
            crate::gbuffer::POSITION_FORMAT,
            SOURCE,
        );
        let normal = source_texture(
            &device,
            "sparse composite normal",
            crate::gbuffer::NORMAL_FORMAT,
            SOURCE,
        );
        let map_id = crate::gbuffer::pack_ids(7, crate::place::Stance::Flat, crate::place::Kind::Land);
        let texels = (SOURCE * SOURCE) as usize;
        let mut colors = vec![0_u8; texels * 4];
        let mut source_ids = vec![0_u8; texels * 4];
        for y in 0..SOURCE {
            for x in 0..SOURCE {
                // Exactly one valid source pixel per 4x4 cache footprint.
                if x % 4 != 3 || y % 4 != 3 {
                    continue;
                }
                let at = (y * SOURCE + x) as usize * 4;
                colors[at..at + 4].copy_from_slice(&[219, 31, 17, u8::MAX]);
                source_ids[at..at + 4].copy_from_slice(&map_id.to_le_bytes());
            }
        }
        let write = |texture: &wgpu::Texture, bytes: &[u8], bytes_per_texel| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_texel * SOURCE),
                    rows_per_image: Some(SOURCE),
                },
                wgpu::Extent3d {
                    width: SOURCE,
                    height: SOURCE,
                    depth_or_array_layers: 1,
                },
            );
        };
        write(&color, &colors, 4);
        write(&ids, &source_ids, 4);
        write(&position, &vec![0_u8; texels * 16], 16);
        write(&normal, &vec![0_u8; texels * 4], 4);

        let source_depth = crate::renderer::depth_texture(&device, SOURCE, SOURCE);
        let source_depth_view = source_depth.create_view(&wgpu::TextureViewDescriptor::default());
        let key = CompositeKey {
            block: BlockCoord { x: 0, y: 0 },
            tier: CompositeTier::Lod2,
            revision: ImmutableRevision::default(),
        };
        let bounds = MapBlockBounds {
            min_x: 0,
            max_x: 0,
            min_y: 0,
            max_y: 0,
        };
        let mut work = CompositeWorkQueue::new(1, 1).unwrap();
        work.refresh(bounds, bounds, BlockLod::Lod2, key.revision, |_| false);
        assert_eq!(work.take_for_frame().len(), 1);
        let mut cache = CompositeCache::default();
        let mut composite = CompositeRenderer::new(&device);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sparse composite source depth"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &source_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.5),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                multiview_mask: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        assert!(
            work.finish_capture(
                &device,
                &queue,
                &mut encoder,
                &mut composite,
                &mut cache,
                key,
                CaptureSource {
                    color: &color,
                    ids: &ids,
                    position: &position,
                    normal: &normal,
                    depth: &source_depth_view,
                    depth_base: 0,
                    rect: crate::blit::ViewportRect {
                        x: 0,
                        y: 0,
                        width: SOURCE,
                        height: SOURCE,
                    },
                },
                CompositeProducerJob::new(key).ground(),
            )
            .is_some()
        );
        queue.submit([encoder.finish()]);
        let ids = read_attachment(
            &device,
            &queue,
            cache
                .get(key)
                .unwrap()
                .deferred_textures()
                .expect("captured planes")
                .0,
        );
        assert!(ids.chunks_exact(4).all(|word| {
            crate::gbuffer::ids_kind(u32::from_le_bytes(word.try_into().unwrap()))
                == Some(crate::place::Kind::Land)
        }));
    }

    #[test]
    fn lod_zero_cannot_become_a_composite_key() {
        assert_eq!(CompositeTier::from_lod(BlockLod::Lod0), None);
        assert_eq!(CompositeTier::from_lod(BlockLod::Lod1), Some(CompositeTier::Lod1));
        assert_eq!(CompositeTier::from_lod(BlockLod::Lod2), Some(CompositeTier::Lod2));
    }

    #[test]
    fn block_ownership_excludes_neighbouring_capture_pixels() {
        let block = BlockCoord { x: 2, y: 3 };
        assert_eq!(BlockCoord::containing(16, 24), block);
        assert_eq!(BlockCoord::containing(23, 31), block);
        assert_ne!(BlockCoord::containing(15, 24), block);
        assert_ne!(BlockCoord::containing(24, 31), block);
        assert_ne!(BlockCoord::containing(16, 32), block);
    }

    #[test]
    fn tiers_represent_the_same_padded_source_extent() {
        let lod1 = CompositeSize::for_block(CompositeTier::Lod1, 64);
        let lod2 = CompositeSize::for_block(CompositeTier::Lod2, 64);
        assert_eq!(lod1, CompositeSize::new(480, 480).unwrap());
        assert_eq!(lod2, CompositeSize::new(120, 120).unwrap());
    }

    #[test]
    fn canonical_ground_block_extent_does_not_depend_on_current_atlas_contents() {
        let lod1 = CompositeSize::for_block(CompositeTier::Lod1, 0);
        let lod2 = CompositeSize::for_block(CompositeTier::Lod2, 0);
        assert_eq!(lod1, CompositeSize::new(352, 352).unwrap());
        assert_eq!(lod2, CompositeSize::new(88, 88).unwrap());
    }

    #[test]
    fn producer_job_has_a_fixed_local_camera_and_canonical_source_extent() {
        let key = CompositeKey {
            block: BlockCoord { x: 12, y: 19 },
            tier: CompositeTier::Lod2,
            revision: ImmutableRevision(41),
        };
        let job = CompositeProducerJob::new(key);
        assert_eq!(job.key(), key);
        assert_eq!(job.source_size(), CompositeSize::new(352, 352).unwrap());
        assert_eq!(job.output_size(), CompositeSize::new(88, 88).unwrap());
        assert_eq!(job.camera().width, COMPOSITE_SOURCE_SIDE);
        assert_eq!(job.camera().height, COMPOSITE_SOURCE_SIDE);
        assert_eq!(
            job.source_rect(),
            Rect {
                x: 0.0,
                y: 0.0,
                width: COMPOSITE_SOURCE_SIDE as f32,
                height: COMPOSITE_SOURCE_SIDE as f32,
            }
        );
    }

    #[test]
    fn elevated_flat_plateau_keeps_its_source_rect_at_the_full_attachment() {
        let key = CompositeKey {
            block: BlockCoord { x: 12, y: 19 },
            tier: CompositeTier::Lod1,
            revision: ImmutableRevision(41),
        };
        let elevated = CompositeProducerJob::for_flat_ground(key, FlatGroundBlock::at(key.block, 20));
        assert_eq!(elevated.source_rect().x, 0.0);
        assert_eq!(elevated.source_rect().y, 0.0);
        assert_eq!(elevated.source_rect().width, COMPOSITE_SOURCE_SIDE as f32);
        assert_eq!(elevated.source_rect().height, COMPOSITE_SOURCE_SIDE as f32);

        let level = CompositeProducerJob::new(key);
        let camera = Camera::new(openshard_protocol::world::Point::new(100, 100, 0), 640, 480);
        assert_eq!(
            elevated.rect_in(camera).y,
            level.rect_in(camera).y - 20.0 * crate::camera::Z_STEP as f32
        );
    }

    #[test]
    fn flat_ground_block_accepts_only_one_common_surface_height() {
        use openshard_map::map::{LandCell, LandTile, WorldMap};

        let mut map = WorldMap::from_blocks(BlockExtent { wide: 2, down: 2 }, |_, _| LandCell {
            tile: LandTile(7),
            z: 20,
        });
        let block = BlockCoord { x: 0, y: 0 };
        let plateau = FlatGroundBlock::inspect(&map, block).expect("one level 8x8 plateau");
        assert_eq!(plateau.block(), block);
        assert_eq!(plateau.elevation(), 20);

        // One altered source height makes the generated corner field sloped.
        // The cache must reject the whole block rather than trying to keep the
        // other 63 tiles in a different ownership domain.
        map.set_land(
            4,
            4,
            LandCell {
                tile: LandTile(7),
                z: 21,
            },
        );
        assert_eq!(FlatGroundBlock::inspect(&map, block), None);
    }

    #[test]
    fn prepared_work_preserves_the_verified_plateau_for_producer_and_restore() {
        use openshard_map::map::{LandCell, LandTile, WorldMap};

        let map = WorldMap::from_blocks(BlockExtent { wide: 2, down: 2 }, |_, _| LandCell {
            tile: LandTile(7),
            z: 20,
        });
        let key = CompositeKey {
            block: BlockCoord { x: 1, y: 1 },
            tier: CompositeTier::Lod1,
            revision: ImmutableRevision(9),
        };
        let plateau = FlatGroundBlock::inspect(&map, key.block).expect("the map supplies one plateau proof");
        let mut queue = CompositeWorkQueue::new(1, 1).expect("one bounded prepared job");
        let bounds = MapBlockBounds {
            min_x: 1,
            max_x: 1,
            min_y: 1,
            max_y: 1,
        };
        queue.refresh(bounds, bounds, BlockLod::Lod1, key.revision, |_| false);
        assert!(queue.mark_prepared(key, plateau));

        let prepared = queue
            .take_marked_prepared_for_frame()
            .pop()
            .expect("prepared work must retain its source proof");
        assert_eq!(prepared.work.key, key);
        assert_eq!(prepared.ground, plateau);
        let job = CompositeProducerJob::for_flat_ground(prepared.work.key, prepared.ground);
        assert_eq!(job.ground(), plateau);
        assert_eq!(job.source_rect(), job.rect_in(job.camera()));
        let level = CompositeProducerJob::new(key);
        let camera = Camera::new(openshard_protocol::world::Point::new(100, 100, 0), 640, 480);
        assert_eq!(
            job.rect_in(camera).y,
            level.rect_in(camera).y - 20.0 * crate::camera::Z_STEP as f32
        );
    }

    #[test]
    #[should_panic(expected = "source proof for its own block")]
    fn prepared_work_rejects_a_source_proof_for_another_block() {
        let key = CompositeKey {
            block: BlockCoord { x: 0, y: 0 },
            tier: CompositeTier::Lod1,
            revision: ImmutableRevision::default(),
        };
        let bounds = MapBlockBounds {
            min_x: 0,
            max_x: 0,
            min_y: 0,
            max_y: 0,
        };
        let mut queue = CompositeWorkQueue::new(1, 1).expect("one bounded prepared job");
        queue.refresh(bounds, bounds, BlockLod::Lod1, key.revision, |_| false);
        queue.mark_prepared(key, FlatGroundBlock::at(BlockCoord { x: 1, y: 0 }, 0));
    }

    #[test]
    fn producer_source_and_runtime_rects_share_one_canonical_transform() {
        let key = CompositeKey {
            block: BlockCoord { x: 12, y: 19 },
            tier: CompositeTier::Lod1,
            revision: ImmutableRevision::default(),
        };
        let job = CompositeProducerJob::new(key);
        assert_eq!(job.source_rect(), job.rect_in(job.camera()));

        let east = CompositeProducerJob::new(CompositeKey {
            block: BlockCoord { x: 13, y: 19 },
            ..key
        });
        let south = CompositeProducerJob::new(CompositeKey {
            block: BlockCoord { x: 12, y: 20 },
            ..key
        });
        let mut camera = Camera::new(openshard_protocol::world::Point::new(100, 100, 0), 640, 480);
        for zoom in [
            crate::camera::Zoom::ONE.scale_down(),
            crate::camera::Zoom::ONE,
            crate::camera::Zoom::ONE.scale_up(),
        ] {
            camera.zoom_about(crate::camera::RealPixel::new(320, 240), zoom);
            let here = job.rect_in(camera);
            let east_rect = east.rect_in(camera);
            let south_rect = south.rect_in(camera);
            assert_eq!(here.width, COMPOSITE_SOURCE_SIDE as f32);
            assert_eq!(here.height, COMPOSITE_SOURCE_SIDE as f32);
            assert_eq!(east_rect.x - here.x, 4.0 * TILE_WIDTH as f32);
            assert_eq!(east_rect.y - here.y, 4.0 * TILE_HEIGHT as f32);
            assert_eq!(south_rect.x - here.x, -4.0 * TILE_WIDTH as f32);
            assert_eq!(south_rect.y - here.y, 4.0 * TILE_HEIGHT as f32);
        }
    }

    #[test]
    fn pixels_require_an_exact_rgba_image() {
        let size = CompositeSize::new(3, 2).unwrap();
        assert!(CompositePixels::new(size, vec![0; 23]).is_none());
        let pixels = CompositePixels::new(size, vec![7; 24]).unwrap();
        assert_eq!(pixels.size(), size);
        assert_eq!(pixels.rgba(), vec![7; 24]);
    }

    #[test]
    fn only_a_complete_deferred_result_can_replace_map_geometry() {
        let size = CompositeSize::new(2, 1).unwrap();
        let plain = CompositePixels::new(size, vec![0; 8]).unwrap();
        assert!(plain.deferred().is_none());
        assert!(DeferredPixels::new(size, vec![0; 1], vec![0.0; 8], vec![0; 2], vec![1.0; 2], 17).is_none());
        let deferred =
            DeferredPixels::new(size, vec![1; 2], vec![0.0; 8], vec![2; 2], vec![0.5; 2], 17).unwrap();
        let ready = plain.with_deferred(deferred).unwrap();
        assert_eq!(ready.deferred().unwrap().depth_base(), 17);
    }

    fn blocks(min_x: u32, max_x: u32, min_y: u32, max_y: u32) -> MapBlockBounds {
        MapBlockBounds {
            min_x,
            max_x,
            min_y,
            max_y,
        }
    }

    #[test]
    fn queue_dispatches_visible_blocks_before_blocks_ahead_of_the_camera() {
        let mut queue = CompositeWorkQueue::new(32, 32).unwrap();
        let map = blocks(0, 9, 0, 9);
        let revision = ImmutableRevision(7);
        queue.refresh(blocks(1, 2, 1, 2), map, BlockLod::Lod2, revision, |_| false);
        // A one-block pan right predicts a full viewport to the right.  The
        // first four jobs are still the newly visible rectangle, regardless of
        // the ahead range's nearer distance.
        queue.refresh(blocks(2, 3, 1, 2), map, BlockLod::Lod2, revision, |_| false);
        let work = queue.take_for_frame();
        let visible: BTreeSet<_> = blocks(2, 3, 1, 2).blocks().collect();
        let first_visible = work.iter().take(visible.len()).collect::<Vec<_>>();
        assert!(
            first_visible
                .iter()
                .all(|job| job.priority == CompositePriority::Visible)
        );
        assert!(first_visible.iter().all(|job| visible.contains(&job.key.block)));
        assert!(work.iter().skip(visible.len()).all(|job| {
            job.priority == CompositePriority::Visible || job.priority == CompositePriority::Ahead
        }));
    }

    #[test]
    fn an_unprepared_job_stays_pending_until_its_map_inputs_are_available() {
        let mut queue = CompositeWorkQueue::new(8, 1).unwrap();
        let visible = blocks(3, 3, 4, 4);
        queue.refresh(visible, visible, BlockLod::Lod1, ImmutableRevision(9), |_| false);

        assert!(queue.take_prepared_for_frame(|_| false).is_empty());
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(queue.in_flight_len(), 0);

        let work = queue.take_prepared_for_frame(|_| true);
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].key.block, BlockCoord { x: 3, y: 4 });
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.in_flight_len(), 1);
    }

    #[test]
    fn preparation_uses_the_same_bounded_priority_order_without_dispatching() {
        let mut queue = CompositeWorkQueue::new(8, 1).unwrap();
        let map = blocks(0, 9, 0, 9);
        queue.refresh(
            blocks(1, 1, 1, 1),
            map,
            BlockLod::Lod1,
            ImmutableRevision(4),
            |_| false,
        );
        queue.refresh(
            blocks(2, 2, 1, 1),
            map,
            BlockLod::Lod1,
            ImmutableRevision(4),
            |_| false,
        );

        let candidates = queue.preparation_candidates();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].priority, CompositePriority::Visible);
        let ground = CompositeProducerJob::new(candidates[0].key).ground();
        assert!(queue.mark_prepared(candidates[0].key, ground));
        let next = queue.preparation_candidates();
        assert_eq!(next.len(), 1);
        assert_ne!(next[0].key, candidates[0].key);
        assert_eq!(
            queue.take_marked_prepared_for_frame(),
            vec![PreparedCompositeWork {
                work: candidates[0],
                ground,
            }]
        );
        assert_eq!(queue.pending_len(), 1);
        assert_eq!(queue.in_flight_len(), 1);
    }

    /// A deterministic far-zoom pan benchmark: every entered block can add
    /// work, but the producer receives no more than its one-job frame budget.
    /// It deliberately uses the preparation gate the app uses, so this guards
    /// against a future shortcut that turns a long pan into synchronous bursts.
    #[test]
    fn steady_far_zoom_pan_benchmark_keeps_producer_work_bounded() {
        let mut queue = CompositeWorkQueue::new(128, 1).unwrap();
        let map = blocks(0, 511, 0, 3);
        let revision = ImmutableRevision(12);
        let mut ready = BTreeSet::new();
        let mut produced = 0;

        for x in 0..256 {
            let visible = blocks(x, x, 1, 1);
            queue.refresh(visible, map, BlockLod::Lod2, revision, |key| ready.contains(&key));
            for candidate in queue.preparation_candidates() {
                assert!(
                    queue.mark_prepared(candidate.key, CompositeProducerJob::new(candidate.key).ground())
                );
            }
            let frame = queue.take_marked_prepared_for_frame();
            assert!(
                frame.len() <= 1,
                "pan frame {x} exceeded the configured producer budget: {frame:?}"
            );
            for work in frame {
                produced += 1;
                ready.insert(work.work.key);
                queue.finished(work.work.key);
            }
            assert!(queue.pending_len() <= 128);
        }

        assert!(produced > 0, "the benchmark must exercise newly entered blocks");
        assert_eq!(queue.in_flight_len(), 0);
    }

    /// Returning to the detailed renderer at a max zoom must stop composite
    /// preparation immediately.  A producer already handed a job keeps its
    /// reservation until it finishes, but repeated camera pans at LOD0 cannot
    /// create another atlas-preparation or producer pass.
    #[test]
    fn detailed_max_zoom_pan_never_requeues_composite_work() {
        let mut queue = CompositeWorkQueue::new(128, 1).unwrap();
        let map = blocks(0, 63, 0, 63);
        let first = blocks(20, 21, 20, 21);
        queue.refresh(first, map, BlockLod::Lod1, ImmutableRevision::default(), |_| {
            false
        });
        assert!(!queue.preparation_candidates().is_empty());
        let dispatched = queue.take_for_frame();
        assert_eq!(dispatched.len(), 1, "one earlier producer may finish safely");

        for offset in 0..256 {
            let at = blocks(
                20 + offset % 8,
                20 + (offset / 8) % 8,
                20 + offset % 8,
                20 + (offset / 8) % 8,
            );
            queue.refresh(at, map, BlockLod::Lod0, ImmutableRevision::default(), |_| false);
            assert_eq!(queue.pending_len(), 0, "LOD0 pan {offset} queued composite work");
            assert_eq!(
                queue.prepared_len(),
                0,
                "LOD0 pan {offset} prepared composite work"
            );
            assert!(queue.preparation_candidates().is_empty());
            assert!(
                queue.take_for_frame().is_empty(),
                "LOD0 pan {offset} dispatched composite work"
            );
        }
        assert_eq!(
            queue.in_flight_len(),
            1,
            "only the pre-existing job remains to finish"
        );
    }

    #[test]
    fn queue_is_bounded_and_visible_work_evicts_the_furthest_prefetch() {
        let mut queue = CompositeWorkQueue::new(2, 1).unwrap();
        let map = blocks(0, 9, 0, 9);
        queue.refresh(
            blocks(0, 0, 0, 0),
            map,
            BlockLod::Lod1,
            ImmutableRevision(0),
            |_| false,
        );
        queue.refresh(
            blocks(1, 2, 0, 0),
            map,
            BlockLod::Lod1,
            ImmutableRevision(0),
            |_| false,
        );
        assert_eq!(queue.pending_len(), 2);
        let work = queue.take_for_frame();
        assert_eq!(work[0].priority, CompositePriority::Visible);
        assert_eq!(work[0].key.block, BlockCoord { x: 1, y: 0 });
    }

    #[test]
    fn ready_or_in_flight_work_is_not_composed_again() {
        let mut queue = CompositeWorkQueue::new(8, 1).unwrap();
        let visible = blocks(1, 1, 1, 1);
        let map = blocks(0, 9, 0, 9);
        let revision = ImmutableRevision(4);
        queue.refresh(visible, map, BlockLod::Lod1, revision, |_| false);
        let first = queue.take_for_frame();
        assert_eq!(first.len(), 1);
        queue.refresh(visible, map, BlockLod::Lod1, revision, |_| false);
        assert_eq!(queue.pending_len(), 0, "in-flight work is deduplicated");
        queue.finished(first[0].key);
        queue.refresh(visible, map, BlockLod::Lod1, revision, |key| key == first[0].key);
        assert_eq!(queue.pending_len(), 0, "ready work is not re-requested");
    }

    #[test]
    fn a_pending_block_that_becomes_a_lod0_fallback_is_discarded() {
        let mut queue = CompositeWorkQueue::new(8, 1).unwrap();
        let visible = blocks(1, 1, 1, 1);
        let map = blocks(0, 9, 0, 9);
        let revision = ImmutableRevision(4);
        queue.refresh(visible, map, BlockLod::Lod1, revision, |_| false);
        assert_eq!(queue.pending_len(), 1);

        // The cache callback also represents a permanent LOD0 decision. A
        // slope-containing block must leave the queue rather than remain an
        // unpreparable visible request.
        queue.refresh(visible, map, BlockLod::Lod1, revision, |_| true);
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(queue.prepared_len(), 0);
    }

    #[test]
    fn tile_bounds_are_clipped_before_becoming_block_requests() {
        let bounds = MapBlockBounds::from_tiles(
            TileBounds {
                min_x: -8,
                max_x: 17,
                min_y: -1,
                max_y: 8,
            },
            16,
            16,
        )
        .unwrap();
        assert_eq!(bounds, blocks(0, 1, 0, 1));
    }

    #[test]
    fn invalidating_a_block_cancels_its_pending_and_in_flight_lods_only() {
        let mut queue = CompositeWorkQueue::new(8, 2).unwrap();
        let map = blocks(0, 9, 0, 9);
        queue.refresh(
            blocks(2, 3, 2, 2),
            map,
            BlockLod::Lod1,
            ImmutableRevision(9),
            |_| false,
        );
        let dispatched = queue.take_for_frame();
        assert_eq!(dispatched.len(), 2);
        let changed = dispatched[0].key.block;
        assert!(queue.invalidate_block(changed) >= 1);
        assert!(
            !queue.queue.contains_in_flight(dispatched[0].key),
            "a late result for a changed block must no longer own a cache slot"
        );
        assert!(queue.queue.in_flight_keys().all(|key| key.block != changed));
        assert!(queue.queue.pending_keys().all(|key| key.block != changed));
    }

    #[test]
    fn cache_eviction_keeps_the_viewport_margin_and_discards_the_lru_tail() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let Some(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()
        else {
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            return;
        };
        let limits = CompositeCacheLimits::new(32, 0).unwrap();
        let mut cache = CompositeCache::with_limits(limits);
        let size = CompositeSize::new(2, 2).unwrap();
        let key = |x| CompositeKey {
            block: BlockCoord { x, y: 0 },
            tier: CompositeTier::Lod1,
            revision: ImmutableRevision(0),
        };
        for x in 0..3 {
            cache.insert(
                &device,
                &queue,
                key(x),
                CompositePixels::new(size, vec![x as u8; 16]).unwrap(),
            );
        }
        // Make block zero newer than block one.  Block two is visible and so
        // protected even though it is the oldest entry after these insertions.
        cache.get(key(0));
        let evicted = cache.evict_lru_outside_viewport(Some(blocks(2, 2, 0, 0)));
        assert_eq!(evicted.entries, 1);
        assert!(cache.get(key(0)).is_some());
        assert!(
            cache.get(key(1)).is_none(),
            "the oldest non-visible entry is the LRU tail"
        );
        assert!(cache.get(key(2)).is_some(), "the visible block is protected");
        assert_eq!(evicted.retained_gpu_bytes, 32);
    }

    #[test]
    fn viewport_margin_is_hysteresis_not_an_eager_eviction_target() {
        let bounds = blocks(10, 11, 20, 21);
        assert!(bounds.expanded_by(1).contains(BlockCoord { x: 9, y: 19 }));
        assert!(bounds.expanded_by(1).contains(BlockCoord { x: 12, y: 22 }));
        assert!(!bounds.expanded_by(1).contains(BlockCoord { x: 8, y: 20 }));
    }

    #[test]
    fn quarantine_retains_the_latest_owner_and_reason() {
        let mut cache = CompositeCache::default();
        let key = CompositeKey {
            block: BlockCoord { x: 3, y: 7 },
            tier: CompositeTier::Lod1,
            revision: ImmutableRevision(12),
        };
        cache.reject_block(key, None, CompositeQuarantineReason::NonFlatGround);

        assert!(cache.is_rejected(key.block));
        assert_eq!(cache.quarantined_len(), 1);
        assert_eq!(
            cache.latest_quarantine(),
            Some(CompositeQuarantine {
                block: key.block,
                key,
                ground: None,
                reason: CompositeQuarantineReason::NonFlatGround,
            })
        );
    }
}
