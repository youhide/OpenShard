//! The minimap's window component.
//!
//! Terrain is recorded by the radar content pass, not represented as gump art.
//! This pane owns the one rectangle the window layer uses for layout and hits;
//! keeping it here prevents dragging and pointer routing from each inventing a
//! slightly different minimap size.

use openshard_client_render::gump::{GumpArt, GumpPixel};
use openshard_client_render::radar::RadarRegion;
use openshard_protocol::world::{Facet, Point};

use crate::panes::{Input, Pane, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

/// Minimap content bounds in its local gump coordinate system.
pub const EXTENT: (i32, i32) = (160, 160);

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
}

impl Window {
    #[must_use]
    pub const fn contains(self, point: GumpPixel) -> bool {
        point.x >= 0 && point.y >= 0 && point.x < self.extent.0 && point.y < self.extent.1
    }
}

/// A local window with no pane-private interaction yet. Drag/raise/close are
/// manager gestures shared by every [`crate::windows::OwnWindow`].
#[derive(Clone, Copy, Debug, Default)]
pub struct MinimapPane;

impl Pane for MinimapPane {
    fn art(&self, _: &PaneFrame<'_>) -> Vec<GumpArt> {
        Vec::new()
    }

    fn layout(&self, _: &PaneFrame<'_>) -> Option<Drawn> {
        Some(Drawn::Minimap(Window { extent: EXTENT }))
    }

    fn handle(&mut self, _: Input, _: &PaneCtx<'_>) -> Response {
        Response::ignored()
    }
}

#[cfg(test)]
mod tests {
    use super::{EXTENT, Window, radar_region_for};
    use openshard_client_render::gump::GumpPixel;
    use openshard_protocol::world::{Facet, Point};

    #[test]
    fn hit_bounds_include_the_first_pixel_and_exclude_the_far_edges() {
        let window = Window { extent: EXTENT };
        assert!(window.contains(GumpPixel::new(0, 0)));
        assert!(window.contains(GumpPixel::new(EXTENT.0 - 1, EXTENT.1 - 1)));
        assert!(!window.contains(GumpPixel::new(EXTENT.0, 0)));
        assert!(!window.contains(GumpPixel::new(0, EXTENT.1)));
    }

    #[test]
    fn a_region_is_centred_on_the_player_and_clips_at_the_map_edge() {
        let region = radar_region_for(Point::new(100, 100, 0), EXTENT);
        assert_eq!(region.facet, Facet(crate::FACET));
        assert_eq!(region.lod, 0);
        assert_eq!(region.origin, (20, 20));
        assert_eq!(region.extent, (160, 160));

        let clipped = radar_region_for(Point::new(10, 10, 0), EXTENT);
        assert_eq!(
            clipped.origin,
            (0, 0),
            "half the window falls off the map, not off the u32"
        );
    }
}
