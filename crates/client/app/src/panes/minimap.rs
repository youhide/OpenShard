//! The minimap's window component.
//!
//! Terrain is recorded by the radar content pass, not represented as gump art.
//! This pane owns the one rectangle the window layer uses for layout and hits;
//! keeping it here prevents dragging and pointer routing from each inventing a
//! slightly different minimap size.

use std::time::{Duration, Instant};

use openshard_client_render::gump::{GumpArt, GumpPixel, Picture};
use openshard_client_render::radar::RadarRegion;
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
/// world tiles — one pixel a tile, so [`EXTENT`] doubles as both. Centring
/// saturates rather than wrapping, so a body near the map's own edge shows a
/// region clipped to it instead of one that reads from the far side.
#[must_use]
pub(crate) fn radar_region_for(player: Point, extent: (i32, i32)) -> RadarRegion {
    let half_x = u32::from(u16::try_from(extent.0).unwrap_or(0)) / 2;
    let half_y = u32::from(u16::try_from(extent.1).unwrap_or(0)) / 2;
    RadarRegion {
        facet: Facet(crate::FACET),
        lod: 0,
        origin: (
            u32::from(player.x).saturating_sub(half_x),
            u32::from(player.y).saturating_sub(half_y),
        ),
        extent: (extent.0.try_into().unwrap_or(0), extent.1.try_into().unwrap_or(0)),
    }
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
    use super::{SMALL_EXTENT, SMALL_FRAME, Window, radar_region_for};
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
        let region = radar_region_for(Point::new(100, 100, 0), SMALL_EXTENT);
        assert_eq!(region.facet, Facet(crate::FACET));
        assert_eq!(region.lod, 0);
        assert_eq!(region.origin, (40, 40));
        assert_eq!(region.extent, (120, 120));

        let clipped = radar_region_for(Point::new(10, 10, 0), SMALL_EXTENT);
        assert_eq!(
            clipped.origin,
            (0, 0),
            "half the window falls off the map, not off the u32"
        );
    }
}
