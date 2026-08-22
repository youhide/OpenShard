//! Choosing a map block's rendering level of detail from its screen footprint.
//!
//! A map block is a fixed `BLOCK_SIZE`-by-`BLOCK_SIZE` square of tiles.  In the
//! isometric projection its ground footprint is a diamond whose horizontal and
//! vertical extents are both `BLOCK_SIZE * TILE_WIDTH` virtual pixels.  The
//! camera's zoom is the only transform between that footprint and physical
//! viewport pixels, so this module deliberately does not name a zoom rung.
//! A future camera scale, device scale, or continuous zoom can feed the same
//! measured footprint to [`LodThresholds::next`] without changing the policy.
//!
//! This is selection policy only.  Until the composite renderer exists, the
//! caller keeps using [`BlockLod::Lod0`]; the state and thresholds live here now
//! so that introducing cached composites cannot quietly couple their lifetime
//! to the current seven-rung camera ladder.

use openshard_map::map::BLOCK_SIZE;

use crate::camera::{Camera, TILE_WIDTH};

/// The physical-pixel width and height of one map block's ground footprint.
///
/// A block's corner lattice spans eight tile widths in either screen axis:
/// eight tiles in `x` and eight in `y` produce a diamond 352 virtual pixels
/// wide and high.  Heights and static art may overhang it, but they must not
/// decide which representation owns the *map block*; composite padding is a
/// renderer concern.
pub const BLOCK_FOOTPRINT_VIRTUAL_PIXELS: f32 = BLOCK_SIZE as f32 * TILE_WIDTH as f32;

/// The representation selected for immutable map ground and map statics.
///
/// Dynamic items, mobiles, effects, picking, selection, and UI remain outside
/// this choice.  They are never part of a map-block composite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockLod {
    /// The current per-tile ground and static collectors.
    Lod0,
    /// The first cached map-block composite tier.
    Lod1,
    /// The coarsest cached map-block composite tier.
    Lod2,
}

impl BlockLod {
    /// Whether this level draws a cached composite instead of LOD 0 geometry.
    pub const fn is_composite(self) -> bool {
        !matches!(self, Self::Lod0)
    }

    /// The representation immediately more detailed than this one.
    ///
    /// A block whose selected composite is still being built uses this if it
    /// is already cached.  Falling through from LOD 1 to LOD 0 is intentional:
    /// it is the existing renderer and needs no cache miss to be repaired in a
    /// camera frame.
    pub const fn next_more_detailed(self) -> Option<Self> {
        match self {
            Self::Lod0 => None,
            Self::Lod1 => Some(Self::Lod0),
            Self::Lod2 => Some(Self::Lod1),
        }
    }
}

/// A map block's projected footprint in physical viewport pixels.
///
/// The type prevents a caller from handing the LOD policy virtual render-target
/// pixels at a minifying zoom.  It is the on-screen size that matters: that is
/// what bounds both visual detail and the amount of work a composite saves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProjectedBlockSize(f32);

impl ProjectedBlockSize {
    /// A finite, positive physical-pixel footprint.
    pub fn new(pixels: f32) -> Option<Self> {
        (pixels.is_finite() && pixels > 0.0).then_some(Self(pixels))
    }

    /// The footprint in physical viewport pixels.
    pub const fn pixels(self) -> f32 {
        self.0
    }

    /// Measure one map block through this camera's world-to-viewport scale.
    ///
    /// Translation and viewport dimensions do not change an isometric block's
    /// footprint, but accepting the camera keeps the unit at the real crossing
    /// and prevents callers from treating `render_width` as screen pixels when
    /// the image is minified.
    pub fn from_camera(camera: &Camera) -> Self {
        let zoom = camera.zoom();
        let scale = zoom.numerator() as f32 / zoom.denominator() as f32;
        Self(BLOCK_FOOTPRINT_VIRTUAL_PIXELS * scale)
    }
}

/// Pixel thresholds for entering and leaving the two composite levels.
///
/// The `enter` bounds apply while zooming out (the footprint shrinks); the
/// corresponding `leave` bounds apply while zooming in.  The gaps are
/// hysteresis bands, so a resize or fractional-scale rounding around a boundary
/// does not alternate a block's cache representation every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodThresholds {
    /// Enter LOD 1 from detailed geometry at or below this footprint.
    lod1_enter_pixels: f32,
    /// Return from LOD 1 to detailed geometry at or above this footprint.
    lod1_leave_pixels: f32,
    /// Enter LOD 2 from LOD 1 at or below this footprint.
    lod2_enter_pixels: f32,
    /// Return from LOD 2 to LOD 1 at or above this footprint.
    lod2_leave_pixels: f32,
}

impl LodThresholds {
    /// The shipped policy: a 352-pixel block stays detailed until it becomes
    /// smaller than roughly two hundred physical pixels, then reaches the
    /// coarsest tier only below one hundred pixels.
    ///
    /// The 32-pixel LOD 0/1 and 16-pixel LOD 1/2 gaps are intentionally in
    /// physical pixels.  They survive device scale and do not depend on the
    /// camera's current set of legal zoom rungs.
    pub const DEFAULT: Self = Self {
        lod1_enter_pixels: 192.0,
        lod1_leave_pixels: 224.0,
        lod2_enter_pixels: 96.0,
        lod2_leave_pixels: 112.0,
    };

    /// Construct a threshold policy when the tiers and both hysteresis bands
    /// are strictly ordered.
    ///
    /// `None` refuses inverted or non-finite policies rather than making an
    /// every-frame LOD flip expressible.
    pub fn new(
        lod1_enter_pixels: f32,
        lod1_leave_pixels: f32,
        lod2_enter_pixels: f32,
        lod2_leave_pixels: f32,
    ) -> Option<Self> {
        let values = [
            lod1_enter_pixels,
            lod1_leave_pixels,
            lod2_enter_pixels,
            lod2_leave_pixels,
        ];
        if !values.iter().all(|value| value.is_finite() && *value > 0.0)
            || lod2_enter_pixels >= lod2_leave_pixels
            || lod2_leave_pixels >= lod1_enter_pixels
            || lod1_enter_pixels >= lod1_leave_pixels
        {
            return None;
        }
        Some(Self {
            lod1_enter_pixels,
            lod1_leave_pixels,
            lod2_enter_pixels,
            lod2_leave_pixels,
        })
    }

    /// Choose the next level from the measured footprint and the prior level.
    ///
    /// A large zoom jump may cross more than one band and lands directly in its
    /// settled level.  Within a hysteresis band this returns `was` exactly,
    /// which is the property the composite cache depends on.
    pub fn next(self, was: BlockLod, size: ProjectedBlockSize) -> BlockLod {
        let pixels = size.pixels();
        match was {
            BlockLod::Lod0 if pixels <= self.lod2_enter_pixels => BlockLod::Lod2,
            BlockLod::Lod0 if pixels <= self.lod1_enter_pixels => BlockLod::Lod1,
            BlockLod::Lod0 => BlockLod::Lod0,
            BlockLod::Lod1 if pixels >= self.lod1_leave_pixels => BlockLod::Lod0,
            BlockLod::Lod1 if pixels <= self.lod2_enter_pixels => BlockLod::Lod2,
            BlockLod::Lod1 => BlockLod::Lod1,
            BlockLod::Lod2 if pixels >= self.lod1_leave_pixels => BlockLod::Lod0,
            BlockLod::Lod2 if pixels >= self.lod2_leave_pixels => BlockLod::Lod1,
            BlockLod::Lod2 => BlockLod::Lod2,
        }
    }
}

/// Persistent LOD state for one map block.
///
/// A composite cache owns one of these per block (or per visible block key),
/// updates it once when camera output scale changes, and uses the returned
/// value to choose work.  The selector itself owns no GPU resource and does no
/// rendering or queueing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlockLodSelector {
    thresholds: LodThresholds,
    current: BlockLod,
}

impl BlockLodSelector {
    /// Start at the existing detailed renderer.
    pub const fn new(thresholds: LodThresholds) -> Self {
        Self {
            thresholds,
            current: BlockLod::Lod0,
        }
    }

    /// The selected representation before or after an update.
    pub const fn current(self) -> BlockLod {
        self.current
    }

    /// Update the level from a projected block footprint.
    pub fn update(&mut self, size: ProjectedBlockSize) -> BlockLod {
        self.current = self.thresholds.next(self.current, size);
        self.current
    }

    /// Update directly from a camera's physical projection scale.
    pub fn update_camera(&mut self, camera: &Camera) -> BlockLod {
        self.update(ProjectedBlockSize::from_camera(camera))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{RealPixel, Zoom};
    use openshard_protocol::world::Point;

    fn size(pixels: f32) -> ProjectedBlockSize {
        ProjectedBlockSize::new(pixels).expect("a positive test size")
    }

    #[test]
    fn block_footprint_is_measured_in_viewport_not_render_target_pixels() {
        let mut camera = Camera::new(Point::new(100, 100, 0), 800, 600);
        assert_eq!(ProjectedBlockSize::from_camera(&camera).pixels(), 352.0);

        let half = Zoom::ONE.scale_down().scale_down().scale_down();
        camera.zoom_about(RealPixel { x: 400, y: 300 }, half);
        assert_eq!(camera.render_width(), 1600, "the minified image doubled");
        assert_eq!(ProjectedBlockSize::from_camera(&camera).pixels(), 176.0);
    }

    #[test]
    fn each_boundary_has_a_hysteresis_band() {
        let thresholds = LodThresholds::DEFAULT;

        assert_eq!(thresholds.next(BlockLod::Lod0, size(193.0)), BlockLod::Lod0);
        assert_eq!(thresholds.next(BlockLod::Lod0, size(192.0)), BlockLod::Lod1);
        assert_eq!(
            thresholds.next(BlockLod::Lod1, size(223.0)),
            BlockLod::Lod1,
            "LOD 1 holds through the LOD 0/1 band"
        );
        assert_eq!(thresholds.next(BlockLod::Lod1, size(224.0)), BlockLod::Lod0);

        assert_eq!(thresholds.next(BlockLod::Lod1, size(97.0)), BlockLod::Lod1);
        assert_eq!(thresholds.next(BlockLod::Lod1, size(96.0)), BlockLod::Lod2);
        assert_eq!(
            thresholds.next(BlockLod::Lod2, size(111.0)),
            BlockLod::Lod2,
            "LOD 2 holds through the LOD 1/2 band"
        );
        assert_eq!(thresholds.next(BlockLod::Lod2, size(112.0)), BlockLod::Lod1);
    }

    #[test]
    fn selector_never_flips_inside_a_band_and_settles_large_jumps() {
        let mut selector = BlockLodSelector::new(LodThresholds::DEFAULT);
        assert_eq!(selector.update(size(192.0)), BlockLod::Lod1);
        for pixels in [193.0, 205.0, 223.0, 200.0] {
            assert_eq!(selector.update(size(pixels)), BlockLod::Lod1);
        }

        assert_eq!(selector.update(size(80.0)), BlockLod::Lod2);
        assert_eq!(selector.update(size(240.0)), BlockLod::Lod0);
    }

    #[test]
    fn malformed_thresholds_are_refused() {
        assert!(LodThresholds::new(192.0, 224.0, 96.0, 112.0).is_some());
        assert!(LodThresholds::new(224.0, 192.0, 96.0, 112.0).is_none());
        assert!(LodThresholds::new(192.0, 224.0, 112.0, 96.0).is_none());
        assert!(LodThresholds::new(192.0, 224.0, 96.0, f32::NAN).is_none());
    }

    #[test]
    fn a_missing_composite_falls_back_exactly_one_level() {
        assert_eq!(BlockLod::Lod2.next_more_detailed(), Some(BlockLod::Lod1));
        assert_eq!(BlockLod::Lod1.next_more_detailed(), Some(BlockLod::Lod0));
        assert_eq!(BlockLod::Lod0.next_more_detailed(), None);
    }
}
