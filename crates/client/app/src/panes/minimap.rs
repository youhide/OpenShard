//! The minimap's window component.
//!
//! Terrain is recorded by the radar content pass, not represented as gump art.
//! This pane owns the one rectangle the window layer uses for layout and hits;
//! keeping it here prevents dragging and pointer routing from each inventing a
//! slightly different minimap size.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use openshard_client_render::gump::{GumpArt, GumpPixel, Picture};
use openshard_protocol::wire::Graphic;

use crate::panes::{Input, Pane, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

/// The two radar gumps used by the classic client.  The art supplies the
/// decorative rim; the generated terrain is clipped to the same circle below.
const SMALL_FRAME: Graphic = Graphic(5010);
const LARGE_FRAME: Graphic = Graphic(5011);

/// Fallbacks for an install that lacks the radar gumps.  Shipping installs
/// measure the packed art instead, so custom clients keep their own dimensions.
pub const SMALL_EXTENT: (i32, i32) = (120, 120);
pub const LARGE_EXTENT: (i32, i32) = (200, 200);

/// The tangent margin's *physical* size, as a fraction of the window's content
/// rectangle — turned into a world-tile count by
/// [`RadarView::with_tangent_margin_fraction`](openshard_client_render::radar::RadarView::with_tangent_margin_fraction),
/// which is where it is divided by `zoom` and by nothing else.
///
/// **Why there is a margin at all.** "Tangent" is an exact mathematical answer
/// with zero room in it: `ceil` (never `round`, which can round the exact
/// answer *down*) still lands the fetched edge exactly on the circle's own
/// boundary, and a nearest-sampled chunk texture, drawn as a handful of
/// axis-aligned quads rather than the true circle, does not paint up to a
/// mathematical line with zero-width precision. Zero margin is what a thin
/// ring of backdrop right at the frame reported as.
///
/// **Why it is a fraction of the window, and why it is divided by `zoom`.**
/// Two reports, in order:
///
/// - **A flat tile count** first covered the small classic frame and then
///   measured visibly short on the large one — whatever this slack pays for
///   scales with the window's own size, so it has to be a fraction of the
///   content rectangle, not a constant.
/// - **A fraction alone**, with no `zoom` in it, covered both frames at their
///   default zoom and then thinned to an almost-invisible stripe at maximum
///   zoom-out on the large one. One world tile is `zoom` physical pixels, so a
///   margin counted in *tiles* and held constant is a margin that shrinks in
///   actual screen pixels as the window zooms out — backwards from what a
///   fixed *visual* seam needs. Dividing by `zoom` keeps its physical size
///   constant instead.
///
/// It must **not** also scale with the desk's magnification or the device
/// scale: the physical seam this pays for does not depend on HiDPI or desk
/// scale, and multiplying by them too — on top of the whole fetch already
/// scaling by them — is how a zoomed-out HiDPI window would reintroduce the
/// corner-ring starvation `RadarView::region`'s doc describes.
///
/// 21% per side. At a 45° rotation `(sqrt(2) - 1) / 2` is about 20.7%: that is
/// the geometric expansion needed for the map square to reach the round clip
/// in every direction, and the slight rounding up makes the circle clip —
/// rather than a missing edge tile — decide the visible boundary.
const TANGENT_MARGIN_FRACTION: f32 = 0.21;

/// A deliberately runtime-only diagnostic lever.  It changes both the source
/// region and the map rectangle that uses it, so a visible change proves the
/// minimap is running this geometry path rather than a stale executable or a
/// different draw pass.
///
/// Set `OPENSHARD_MINIMAP_MARGIN_FRACTION=0.50` for an intentionally excessive
/// margin. Invalid values, and values outside `0.0..=1.0`, keep the ordinary
/// 21% default.
pub(crate) fn tangent_margin_fraction() -> f32 {
    static VALUE: OnceLock<f32> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("OPENSHARD_MINIMAP_MARGIN_FRACTION")
            .ok()
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
            .unwrap_or(TANGENT_MARGIN_FRACTION)
    })
}

/// The immutable layout data remembered for the next frame's hit test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Window {
    pub extent: (i32, i32),
    pub frame: Picture,
    zoom_steps: i8,
}

impl Window {
    /// The radar sits inside the metal ring, not across the whole gump.  The
    /// classic frames reserve about fifteen percent on every side for their
    /// rim and the four ornaments.
    #[must_use]
    pub const fn content(self) -> (GumpPixel, (i32, i32)) {
        let inset_x = self.extent.0 * 3 / 20;
        let inset_y = self.extent.1 * 3 / 20;
        (
            GumpPixel::new(inset_x, inset_y),
            (self.extent.0 - inset_x * 2, self.extent.1 - inset_y * 2),
        )
    }

    #[must_use]
    pub fn contains(self, point: GumpPixel) -> bool {
        let radius = self.extent.0.min(self.extent.1) / 2;
        let dx = point.x - self.extent.0 / 2;
        let dy = point.y - self.extent.1 / 2;
        radius > 0 && dx * dx + dy * dy <= radius * radius
    }

    #[must_use]
    pub fn zoom(self) -> f32 {
        1.25_f32.powi(i32::from(self.zoom_steps))
    }
}

/// A local window with no pane-private interaction yet. Drag/raise/close are
/// manager gestures shared by every [`crate::windows::OwnWindow`].
#[derive(Debug, Default)]
pub struct MinimapPane {
    large: bool,
    last_left_press: Option<Instant>,
    zoom_steps: i8,
}

impl MinimapPane {
    /// Switch between ClassicUO's small and large radar frames.
    pub fn toggle_size(&mut self) {
        self.large = !self.large;
    }

    const fn frame_art(&self) -> GumpArt {
        GumpArt::Gump(if self.large { LARGE_FRAME } else { SMALL_FRAME })
    }

    const fn fallback_extent(&self) -> (i32, i32) {
        if self.large { LARGE_EXTENT } else { SMALL_EXTENT }
    }
}

impl Pane for MinimapPane {
    fn art(&self, _: &PaneFrame<'_>) -> Vec<GumpArt> {
        vec![self.frame_art()]
    }

    fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let extent = frame
            .files
            .gump_atlas
            .sprite(self.frame_art())
            .map(|sprite| (i32::from(sprite.width), i32::from(sprite.height)))
            .unwrap_or_else(|| self.fallback_extent());
        Some(Drawn::Minimap(Window {
            extent,
            frame: Picture::plain(self.frame_art(), GumpPixel::new(0, 0)),
            zoom_steps: self.zoom_steps,
        }))
    }

    fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        if !ctx.under_pointer {
            return Response::ignored();
        }
        if let Input::Wheel(notches) = input {
            if !ctx.modifiers.ctrl || notches == 0.0 {
                return Response::ignored();
            }
            self.zoom_steps = (self.zoom_steps - notches.signum() as i8).clamp(-6, 12);
            return Response::changed();
        }
        let Input::Press(crate::panes::Button::Left) = input else {
            return Response::ignored();
        };
        let paired = self
            .last_left_press
            .is_some_and(|last| ctx.now.saturating_duration_since(last) <= Duration::from_millis(500));
        self.last_left_press = (!paired).then_some(ctx.now);
        if paired {
            self.toggle_size();
            Response::changed()
        } else {
            // A single press is still the manager's drag gesture.
            Response::ignored()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SMALL_EXTENT, SMALL_FRAME, TANGENT_MARGIN_FRACTION, Window};
    use openshard_client_render::gump::{GumpArt, GumpPixel, Picture};
    use openshard_client_render::radar::{RadarExtent, RadarTile, RadarView};
    use openshard_client_render::radar_pass::Placement;
    use openshard_protocol::world::{Facet, Point};

    /// Britannia, so nothing below is clipped by a toy facet — except the two
    /// edge cases that mean to be.
    fn facet() -> RadarExtent {
        RadarExtent::new(7168, 4096).expect("Britannia has an extent")
    }

    /// The view this pane's window produces, built exactly the way
    /// `App::draw_from` builds it — the *one* construction, which is the whole
    /// point of asserting against it here. Placement origin plays no part in a
    /// region, so it is left at the corner.
    fn view(
        content_extent: (i32, i32),
        magnify: f32,
        device_scale: f32,
        zoom: f32,
        player: Point,
    ) -> RadarView {
        RadarView::new(
            Facet(crate::FACET),
            RadarTile::new(u32::from(player.x), u32::from(player.y)),
            facet(),
            1.0 / zoom,
            Placement {
                origin: (0.0, 0.0),
                extent: (
                    content_extent.0 as f32 * magnify,
                    content_extent.1 as f32 * magnify,
                ),
                circle: true,
                rotation: std::f32::consts::FRAC_PI_4,
            },
            device_scale,
        )
        .with_tangent_margin_fraction(content_extent, zoom, TANGENT_MARGIN_FRACTION)
    }

    /// The fetched extent for a window well away from every map edge.
    fn native_extent(content_extent: (i32, i32), magnify: f32, device_scale: f32, zoom: f32) -> RadarExtent {
        view(
            content_extent,
            magnify,
            device_scale,
            zoom,
            Point::new(3000, 2000, 0),
        )
        .region()
        .extent()
    }

    #[test]
    fn hit_bounds_include_the_first_pixel_and_exclude_the_far_edges() {
        let window = Window {
            extent: SMALL_EXTENT,
            frame: Picture::plain(GumpArt::Gump(SMALL_FRAME), GumpPixel::new(0, 0)),
            zoom_steps: 0,
        };
        assert!(window.contains(GumpPixel::new(SMALL_EXTENT.0 / 2, SMALL_EXTENT.1 / 2)));
        assert!(window.contains(GumpPixel::new(SMALL_EXTENT.0 / 2, 0)));
        assert!(!window.contains(GumpPixel::new(0, 0)));
        assert!(!window.contains(GumpPixel::new(SMALL_EXTENT.0 - 1, SMALL_EXTENT.1 - 1)));
        assert!(!window.contains(GumpPixel::new(SMALL_EXTENT.0, 0)));
    }

    #[test]
    fn a_region_is_centred_on_the_player_and_pushed_back_inside_both_map_edges() {
        // 100 content pixels, no scaling, no zoom: 100 tiles plus a margin of
        // 2 * ceil(100 * 0.21) = 42, so a 142-tile square around the body.
        let middle = view((100, 100), 1.0, 1.0, 1.0, Point::new(3000, 2000, 0)).region();
        assert_eq!(middle.facet(), Facet(crate::FACET));
        assert_eq!(middle.extent(), (142, 142));
        assert_eq!(middle.origin(), (3000 - 71, 2000 - 71));

        // Half the window falls off the west and north edges, not off the u32.
        let north_west = view((100, 100), 1.0, 1.0, 1.0, Point::new(10, 10, 0)).region();
        assert_eq!(north_west.origin(), (0, 0));

        // **And off the east and south edges too**, which is the half that
        // used to saturate at zero and nowhere else: a region wider than the
        // ground left in front of it is moved back inside the facet, so the
        // window shows terrain with the marker off-centre rather than centred
        // terrain with a band of `UNKNOWN` beside it. Deliberate, and asserted
        // here so the next reader does not "fix" it back to a hole.
        let south_east = view((100, 100), 1.0, 1.0, 1.0, Point::new(7160, 4090, 0)).region();
        assert_eq!(south_east.origin(), (7168 - 142, 4096 - 142));
    }

    #[test]
    fn the_native_fetch_is_one_tile_a_physical_pixel_plus_a_tangent_margin() {
        // At every scale at 1.0 and no zoom, one world tile is asked for per
        // window pixel — not `sqrt(2)` more, which used to buy a rotated
        // square nothing it did not already have — plus a small margin that
        // keeps the fetch from landing exactly, zero-slack, on the circle's
        // own boundary. 140 * 0.21 / 1.0 = 29.4, ceil 30, doubled: 60 tiles.
        // This is just wider than the square a 45°-rotated circle requires.
        assert_eq!(native_extent((140, 140), 1.0, 1.0, 1.0), (200, 200));
        // HiDPI and desk scale both widen the physical footprint, so both
        // widen the *scaled* part of the fetch by the same factor — the
        // margin does not widen again with them (see the dedicated test
        // below for why it must not).
        assert_eq!(native_extent((140, 140), 2.0, 1.0, 1.0), (340, 340));
        assert_eq!(native_extent((140, 140), 1.0, 2.0, 1.0), (340, 340));
    }

    #[test]
    fn the_tangent_margin_scales_with_the_windows_own_size() {
        // A flat tile count covered the small classic frame and then measured
        // short on the large one — the margin has to grow with the window's
        // own logical size, not stay a constant.
        let small = native_extent((84, 84), 1.0, 1.0, 1.0);
        let large = native_extent((140, 140), 1.0, 1.0, 1.0);
        assert_eq!(small, (120, 120), "84 scaled tiles plus a 36-tile margin");
        assert_eq!(
            large,
            (200, 200),
            "140 scaled tiles plus a *larger*, 60-tile margin"
        );
    }

    #[test]
    fn the_tangent_margin_grows_at_low_zoom_so_its_physical_size_stays_put() {
        // One world tile is `zoom` physical pixels, so a margin held constant
        // *in tiles* shrinks in actual screen pixels as the window zooms
        // out — measured live as an almost-invisible stripe on the large
        // frame at maximum zoom-out. Dividing the margin by `zoom` keeps its
        // *physical* size roughly constant instead: at a quarter zoom the
        // scaled fetch quadruples (140 * 4 = 560), and the margin grows from
        // 60 tiles to 2 * ceil(140 * 0.21 / 0.25) = 2 * 118 = 236 — the same
        // order of magnitude as the fetch's own growth, not the flat 60 tiles
        // a full-zoom window gets.
        let at_zoom_one = native_extent((140, 140), 1.0, 1.0, 1.0);
        let zoomed_out = native_extent((140, 140), 1.0, 1.0, 0.25);
        assert_eq!(at_zoom_one, (200, 200));
        assert_eq!(zoomed_out, (796, 796), "the margin grew from 60 tiles to 236");
    }

    #[test]
    fn the_tangent_margin_does_not_scale_with_hidpi_or_desk_scale() {
        // Unlike `zoom`, `magnify` and `device_scale` must not multiply the
        // margin: the physical seam this pays for does not depend on HiDPI or
        // desk scale, and multiplying it by them too — on top of already
        // scaling the whole fetch by them — is exactly how a zoomed-out,
        // HiDPI window would start reintroducing the `sqrt(2)` factor's own
        // corner-ring starvation.
        let baseline = native_extent((140, 140), 1.0, 1.0, 1.0);
        let hidpi = native_extent((140, 140), 1.0, 2.0, 1.0);
        let desk_scaled = native_extent((140, 140), 2.0, 1.0, 1.0);
        assert_eq!(baseline, (200, 200), "140 scaled tiles plus a 60-tile margin");
        assert_eq!(
            hidpi,
            (340, 340),
            "280 scaled tiles plus the *same* 60-tile margin"
        );
        assert_eq!(
            desk_scaled,
            (340, 340),
            "280 scaled tiles plus the *same* 60-tile margin"
        );
    }
}
