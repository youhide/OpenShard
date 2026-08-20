//! The minimap's window component.
//!
//! Terrain is recorded by the radar content pass, not represented as gump art.
//! This pane owns the one rectangle the window layer uses for layout and hits;
//! keeping it here prevents dragging and pointer routing from each inventing a
//! slightly different minimap size.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use openshard_client_render::gump::{GumpArt, GumpPixel, Picture};
use openshard_client_render::radar::{RadarExtent, RadarRegion, RadarTile};
use openshard_protocol::wire::Graphic;
use openshard_protocol::world::{Facet, Point};

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

/// The world-tile rectangle the minimap window shows, centred on where the
/// body stands.
///
/// A region and not a player marker: see `client/render/src/radar.rs`'s own
/// doc for why the two are kept apart. `extent` is the window's own size in
/// world tiles — one pixel a tile, so [`radar_native_extent`]'s answer doubles
/// as both. Centring saturates rather than wrapping, so a body near the map's
/// own edge shows a region clipped to it instead of one that reads from the
/// far side.
#[must_use]
pub(crate) fn radar_region_for(player: Point, extent: RadarExtent) -> RadarRegion {
    RadarRegion::new(
        Facet(crate::FACET),
        RadarTile::new(u32::from(player.x), u32::from(player.y)).saturating_sub(extent),
        extent,
    )
}

/// How many world tiles (native radar texels) the minimap needs fetched for
/// one window, given its content rectangle in gump pixels.
///
/// The one source of truth for this arithmetic: the draw path (`render_passes`,
/// which places the terrain) and the producer's own preparation
/// (`App::draw_from`'s radar block, which requests and builds it) each used to
/// carry their own copy, and a copy is a fork waiting to happen — the two had
/// already drifted, see below.
///
/// The radar cache holds one texel per world tile, and a logical gump pixel
/// can cover several *physical* pixels — HiDPI, or a larger desk scale, or
/// this window's own zoom — so this asks for that many more tiles rather than
/// magnifying the cached texture, which would blur or block up what nearest
/// sampling is for.
///
/// **No `sqrt(2)` factor**, though a whole comment used to argue for one: a
/// window's placed content is a square whose half-side already equals the
/// round frame's own clip radius (`content_extent`'s min dimension over two),
/// and a square that size fully contains its own inscribed circle *at any
/// rotation* — its flat edges are tangent to the circle, never short of it,
/// wherever the rotation happens to put them. Inflating the fetch bought
/// nothing there; what it cost was real. Every extra tile came from the
/// square's own corners, which are the single farthest ground from the
/// player within the region — so under
/// [`region_base_chunks_near`](openshard_client_render::radar::region_base_chunks_near)'s
/// nearest-first production order, that ring is dead last to build and can
/// never earn an LOD stand-in either (an ancestor needs every descendant built
/// at least once, which unvisited edge ground never gets). A window at 2×
/// desk scale asked for exactly twice its needed tile count for no reason,
/// and every one of those needless tiles could starve a real one instead.
///
/// **[`TANGENT_MARGIN_FRACTION`] of real slack, though.** "Tangent" is an
/// exact mathematical answer with zero room in it: `ceil` (never `round`,
/// which can round the exact answer *down*) still lands the fetched edge
/// exactly on the circle's own boundary, and a nearest-sampled chunk texture,
/// drawn as a handful of axis-aligned quads rather than the true circle, does
/// not paint up to a mathematical line with zero-width precision. Zero margin
/// is what a thin ring of backdrop right at the frame reported as.
///
/// The margin is sized in *physical* pixels, not world tiles, and only
/// converted to a tile count last — `margin_fraction * content_extent`,
/// divided by `zoom` the same way the main fetch is. Two reports, in order,
/// are why both halves of that are load-bearing:
///
/// - **A flat tile count** first covered the small classic frame and then
///   measured visibly short on the large one — whatever this slack pays for
///   scales with the window's own size, so it has to be a *fraction of
///   `content_extent`*, not a constant.
/// - **A fraction of `content_extent` alone**, with no `zoom` in it, covered
///   both frames at their default zoom and then thinned to an almost-invisible
///   stripe at maximum zoom-out on the large one. One world tile is `zoom`
///   physical pixels (see the fetch above), so a margin counted in *tiles* and
///   held constant is a margin that shrinks in actual screen pixels as the
///   window zooms out — backwards from what a fixed *visual* seam needs.
///   Dividing by `zoom` keeps the margin's physical size constant instead.
///
/// It must **not** also scale with `magnify` or `device_scale` — the physical
/// margin this pays for doesn't depend on HiDPI or desk scale, and multiplying
/// it by them too would, at a zoomed-out HiDPI window, start reintroducing the
/// corner-ring starvation the `sqrt(2)` factor caused. `TANGENT_MARGIN_FRACTION`
/// is 21% per side.  At a 45° rotation, `(sqrt(2) - 1) / 2` is about 20.7%:
/// that is the geometric expansion needed for the map square to reach the
/// round clip in every direction.  The slight rounding up makes the circle
/// clip, rather than a missing edge tile, decide the visible boundary.
#[must_use]
pub(crate) fn radar_native_extent(
    content_extent: (i32, i32),
    magnify: f32,
    device_scale: f32,
    zoom: f32,
) -> RadarExtent {
    let physical = magnify * device_scale / zoom;
    let margin = (
        2 * (content_extent.0 as f32 * tangent_margin_fraction() / zoom).ceil() as i32,
        2 * (content_extent.1 as f32 * tangent_margin_fraction() / zoom).ceil() as i32,
    );
    let extent = (
        (content_extent.0 as f32 * physical).ceil().max(1.0) as i32 + margin.0,
        (content_extent.1 as f32 * physical).ceil().max(1.0) as i32 + margin.1,
    );
    RadarExtent::new(
        u16::try_from(extent.0).expect("the minimap extent fits UO map coordinates"),
        u16::try_from(extent.1).expect("the minimap extent fits UO map coordinates"),
    )
    .expect("the minimap extent is non-empty")
}

/// The tangent margin's *physical* size, as a fraction of `content_extent` —
/// converted to a world-tile count, divided by `zoom`, only inside
/// [`radar_native_extent`]. See that function's own doc for why the exact
/// circumscribing answer still leaves a visible seam, why the margin has to
/// scale with the window rather than staying a flat tile count, and why it
/// has to scale with `zoom` too or it thins to nothing at maximum zoom-out.
const TANGENT_MARGIN_FRACTION: f32 = 0.21;

/// A deliberately runtime-only diagnostic lever.  It changes both the source
/// region and the map rectangle that uses it, so a visible change proves the
/// minimap is running this geometry path rather than a stale executable or a
/// different draw pass.
///
/// Set `OPENSHARD_MINIMAP_MARGIN_FRACTION=0.50` for an intentionally excessive
/// margin. Invalid values, and values outside `0.0..=1.0`, keep the ordinary
/// 21% default.
fn tangent_margin_fraction() -> f32 {
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
    use super::{RadarExtent, SMALL_EXTENT, SMALL_FRAME, Window, radar_native_extent, radar_region_for};
    use openshard_client_render::gump::{GumpArt, GumpPixel, Picture};
    use openshard_protocol::world::{Facet, Point};

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
    fn a_region_is_centred_on_the_player_and_clips_at_the_map_edge() {
        let extent = RadarExtent::new(120, 120).unwrap();
        let region = radar_region_for(Point::new(100, 100, 0), extent);
        assert_eq!(region.facet(), Facet(crate::FACET));
        assert_eq!(region.origin(), (40, 40));
        assert_eq!(region.extent(), (120, 120));

        let clipped = radar_region_for(Point::new(10, 10, 0), extent);
        assert_eq!(
            clipped.origin(),
            (0, 0),
            "half the window falls off the map, not off the u32"
        );
    }

    #[test]
    fn the_native_fetch_is_one_tile_a_physical_pixel_plus_a_tangent_margin() {
        // At every scale at 1.0 and no zoom, one world tile is asked for per
        // window pixel — not `sqrt(2)` more, which used to buy a rotated
        // square nothing it did not already have — plus a small margin that
        // keeps the fetch from landing exactly, zero-slack, on the circle's
        // own boundary. 140 * 0.21 / 1.0 = 29.4, ceil 30, doubled: 60 tiles.
        // This is just wider than the square a 45°-rotated circle requires.
        assert_eq!(radar_native_extent((140, 140), 1.0, 1.0, 1.0), (200, 200));
        // HiDPI and desk scale both widen the physical footprint, so both
        // widen the *scaled* part of the fetch by the same factor — the
        // margin does not widen again with them (see the dedicated test
        // below for why it must not).
        assert_eq!(radar_native_extent((140, 140), 2.0, 1.0, 1.0), (340, 340));
        assert_eq!(radar_native_extent((140, 140), 1.0, 2.0, 1.0), (340, 340));
    }

    #[test]
    fn the_tangent_margin_scales_with_the_windows_own_size() {
        // A flat tile count covered the small classic frame and then measured
        // short on the large one — the margin has to grow with the window's
        // own logical size, not stay a constant.
        let small = radar_native_extent((84, 84), 1.0, 1.0, 1.0);
        let large = radar_native_extent((140, 140), 1.0, 1.0, 1.0);
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
        let at_zoom_one = radar_native_extent((140, 140), 1.0, 1.0, 1.0);
        let zoomed_out = radar_native_extent((140, 140), 1.0, 1.0, 0.25);
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
        let baseline = radar_native_extent((140, 140), 1.0, 1.0, 1.0);
        let hidpi = radar_native_extent((140, 140), 1.0, 2.0, 1.0);
        let desk_scaled = radar_native_extent((140, 140), 2.0, 1.0, 1.0);
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
