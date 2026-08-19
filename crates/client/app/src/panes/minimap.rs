//! The minimap's window component.
//!
//! Terrain is recorded by the radar content pass, not represented as gump art.
//! This pane owns the one rectangle the window layer uses for layout and hits;
//! keeping it here prevents dragging and pointer routing from each inventing a
//! slightly different minimap size.

use openshard_client_render::gump::{GumpArt, GumpPixel};

use crate::panes::{Input, Pane, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

/// Minimap content bounds in its local gump coordinate system.
pub const EXTENT: (i32, i32) = (160, 160);

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
    use super::{EXTENT, Window};
    use openshard_client_render::gump::GumpPixel;

    #[test]
    fn hit_bounds_include_the_first_pixel_and_exclude_the_far_edges() {
        let window = Window { extent: EXTENT };
        assert!(window.contains(GumpPixel::new(0, 0)));
        assert!(window.contains(GumpPixel::new(EXTENT.0 - 1, EXTENT.1 - 1)));
        assert!(!window.contains(GumpPixel::new(EXTENT.0, 0)));
        assert!(!window.contains(GumpPixel::new(0, EXTENT.1)));
    }
}
