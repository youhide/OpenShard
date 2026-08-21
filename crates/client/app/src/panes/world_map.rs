//! The facet-wide map window.
//!
//! Unlike the radar, this is a rectangular viewport over one complete facet.
//! Terrain still comes from the shared radar cache; this pane owns only local
//! presentation state (zoom and a dragged canvas offset).

use openshard_client_render::gump::{GumpArt, GumpPixel};

use crate::panes::{Button, Input, Pane, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

/// A useful first size without making a laptop-sized surface unusable.
pub const EXTENT: (i32, i32) = (640, 480);
const TITLE_HEIGHT: i32 = 22;

/// The immutable geometry consumed by input picking and the radar pass.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Window {
    pub extent: (i32, i32),
    pub pan: (i32, i32),
    zoom_steps: i8,
}

impl Window {
    #[must_use]
    pub const fn content(self) -> (GumpPixel, (i32, i32)) {
        (
            GumpPixel::new(0, TITLE_HEIGHT),
            (self.extent.0, self.extent.1 - TITLE_HEIGHT),
        )
    }

    #[must_use]
    pub const fn contains(self, point: GumpPixel) -> bool {
        point.x >= 0 && point.y >= 0 && point.x < self.extent.0 && point.y < self.extent.1
    }

    #[must_use]
    pub fn zoom(self) -> f32 {
        1.25_f32.powi(i32::from(self.zoom_steps))
    }
}

/// A local, pannable viewport.  Its title strip remains a normal window-drag
/// target; dragging the canvas itself moves the map instead.
#[derive(Debug, Default)]
pub struct WorldMapPane {
    pan: (i32, i32),
    zoom_steps: i8,
    drag_from: Option<GumpPixel>,
}

impl Pane for WorldMapPane {
    fn art(&self, _: &PaneFrame<'_>) -> Vec<GumpArt> {
        Vec::new()
    }

    fn layout(&self, _: &PaneFrame<'_>) -> Option<Drawn> {
        Some(Drawn::WorldMap(Window {
            extent: EXTENT,
            pan: self.pan,
            zoom_steps: self.zoom_steps,
        }))
    }

    fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match input {
            Input::Wheel(notches) if ctx.under_pointer && notches != 0.0 => {
                self.zoom_steps = (self.zoom_steps - notches.signum() as i8).clamp(-8, 12);
                Response::changed()
            }
            // Keep the strip free for the window manager's ordinary drag.
            Input::Press(Button::Left) if ctx.under_pointer && ctx.frame.cursor.y >= TITLE_HEIGHT => {
                self.drag_from = Some(ctx.frame.cursor);
                Response::consumed()
            }
            Input::Move => {
                let Some(previous) = self.drag_from.replace(ctx.frame.cursor) else {
                    return Response::ignored();
                };
                self.pan.0 = self.pan.0.saturating_add(ctx.frame.cursor.x - previous.x);
                self.pan.1 = self.pan.1.saturating_add(ctx.frame.cursor.y - previous.y);
                Response::changed()
            }
            Input::Release(Button::Left) if self.drag_from.take().is_some() => Response::consumed(),
            _ => Response::ignored(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_window_has_a_drag_strip_and_a_canvas() {
        let window = Window {
            extent: EXTENT,
            pan: (0, 0),
            zoom_steps: 0,
        };
        assert_eq!(window.content(), (GumpPixel::new(0, TITLE_HEIGHT), (640, 458)));
        assert!(window.contains(GumpPixel::new(639, 479)));
        assert!(!window.contains(GumpPixel::new(640, 479)));
    }
}
