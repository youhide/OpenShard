//! The facet-wide map as a normal local gump window.

use openshard_client_render::gump::{self, GumpArt, GumpPixel, Picture};
use openshard_client_render::party::Line;
use openshard_client_render::radar::RadarTile;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::Graphic;

use crate::panes::{Button, Effect, Input, Pane, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

pub const EXTENT: (i32, i32) = (640, 480);
const BACKGROUND: Graphic = Graphic(0x0A28);
const CLOSE: (Graphic, Graphic) = (Graphic(0x00F3), Graphic(0x00F2));
const FALLBACK_INSET: (i32, i32, i32, i32) = (24, 38, 24, 24);
/// Fit the entire facet at the widest view, and allow a close inspection of a
/// small part of it without ever zooming farther out into empty space.
const MIN_ZOOM_STEPS: i8 = 0;
const MAX_ZOOM_STEPS: i8 = 20;

pub fn art_of() -> impl Iterator<Item = GumpArt> {
    (0..9)
        .map(|offset| GumpArt::Gump(Graphic(BACKGROUND.0 + offset)))
        .chain([GumpArt::Gump(CLOSE.0), GumpArt::Gump(CLOSE.1)])
}

#[derive(Clone, PartialEq, Debug)]
pub struct Window {
    pub extent: (i32, i32),
    pub centre: RadarTile,
    pub tiles_per_pixel: f32,
    pub pictures: Vec<Picture>,
    pub lines: Vec<Line>,
    content_at: GumpPixel,
    content_extent: (i32, i32),
    close_at: GumpPixel,
    close_extent: (i32, i32),
}

impl Window {
    #[must_use]
    pub const fn content(&self) -> (GumpPixel, (i32, i32)) {
        (self.content_at, self.content_extent)
    }

    #[must_use]
    pub const fn contains(&self, point: GumpPixel) -> bool {
        point.x >= 0 && point.y >= 0 && point.x < self.extent.0 && point.y < self.extent.1
    }

    #[must_use]
    pub const fn content_contains(&self, point: GumpPixel) -> bool {
        point.x >= self.content_at.x
            && point.y >= self.content_at.y
            && point.x < self.content_at.x + self.content_extent.0
            && point.y < self.content_at.y + self.content_extent.1
    }

    #[must_use]
    pub const fn close_contains(&self, point: GumpPixel) -> bool {
        point.x >= self.close_at.x
            && point.y >= self.close_at.y
            && point.x < self.close_at.x + self.close_extent.0
            && point.y < self.close_at.y + self.close_extent.1
    }
}

#[derive(Debug, Default)]
pub struct WorldMapPane {
    centre: Option<RadarTile>,
    zoom_steps: i8,
    /// The pointer and map centre at the start of a canvas drag.  Both stay
    /// fixed for the gesture so cursor events between frames use one coherent
    /// coordinate system.
    drag_from: Option<GumpPixel>,
    drag_centre: Option<RadarTile>,
    close_held: bool,
}

fn measured_insets(atlas: &gump::GumpAtlas) -> (i32, i32, i32, i32) {
    let size = |offset: u16| {
        atlas
            .sprite(GumpArt::Gump(Graphic(BACKGROUND.0 + offset)))
            .map(|sprite| (i32::from(sprite.width), i32::from(sprite.height)))
            .unwrap_or((0, 0))
    };
    let (tlw, tlh) = size(0);
    let (_, th) = size(1);
    let (trw, trh) = size(2);
    let (blw, blh) = size(6);
    let (brw, brh) = size(8);
    if [tlw, tlh, th, trw, trh, blw, blh, brw, brh].contains(&0) {
        FALLBACK_INSET
    } else {
        (tlw.max(blw), tlh.max(th).max(trh), trw.max(brw), blh.max(brh))
    }
}

fn tpp(map: openshard_protocol::world::MapSize, content: (i32, i32), zoom_steps: i8) -> f32 {
    let fit = (content.0 as f32 / f32::from(map.width)).min(content.1 as f32 / f32::from(map.height));
    1.0 / (fit * 1.25_f32.powi(i32::from(zoom_steps)))
}

fn clamp_centre(
    centre: RadarTile,
    map: openshard_protocol::world::MapSize,
    content: (i32, i32),
    tiles_per_pixel: f32,
) -> RadarTile {
    let clamp_axis = |value: u32, pixels: i32, tiles: u16| {
        let visible = (pixels as f32 * tiles_per_pixel).ceil().min(f32::from(tiles)) as u32;
        let half = visible / 2;
        let max = u32::from(tiles).saturating_sub(visible).saturating_add(half);
        value.clamp(half, max.max(half))
    };
    RadarTile::new(
        clamp_axis(centre.x(), content.0, map.width),
        clamp_axis(centre.y(), content.1, map.height),
    )
}

impl Pane for WorldMapPane {
    fn art(&self, _: &PaneFrame<'_>) -> Vec<GumpArt> {
        art_of().collect()
    }

    fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let (left, top, right, bottom) = measured_insets(frame.files.gump_atlas);
        let content_at = GumpPixel::new(left, top);
        let content_extent = (EXTENT.0 - left - right, EXTENT.1 - top - bottom);
        let tiles_per_pixel = tpp(frame.view.map, content_extent, self.zoom_steps);
        let initial = RadarTile::new(
            u32::from(frame.view.map.width) / 2,
            u32::from(frame.view.map.height) / 2,
        );
        let centre = clamp_centre(
            self.centre.unwrap_or(initial),
            frame.view.map,
            content_extent,
            tiles_per_pixel,
        );
        let close_extent = frame
            .files
            .gump_atlas
            .sprite(GumpArt::Gump(CLOSE.0))
            .map(|sprite| (i32::from(sprite.width), i32::from(sprite.height)))
            .unwrap_or((63, 23));
        let close_at = GumpPixel::new(EXTENT.0 - right - close_extent.0, 10);
        let mut pictures = gump::resize(
            frame.files.gump_atlas,
            BACKGROUND,
            GumpPixel::new(0, 0),
            EXTENT.0,
            EXTENT.1,
        );
        pictures.push(Picture::plain(
            GumpArt::Gump(if self.close_held { CLOSE.1 } else { CLOSE.0 }),
            close_at,
        ));
        Some(Drawn::WorldMap(Window {
            extent: EXTENT,
            centre,
            tiles_per_pixel,
            pictures,
            lines: vec![Line {
                at: GumpPixel::new(left + 8, 12),
                text: "World WorldMap".to_owned(),
                font: Font(2),
                clip: None,
            }],
            content_at,
            content_extent,
            close_at,
            close_extent,
        }))
    }

    fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        let drawn = match ctx.drawn {
            Some(Drawn::WorldMap(window)) => Some(window),
            _ => None,
        };
        match input {
            Input::Wheel(notches) if ctx.under_pointer && notches != 0.0 => {
                let Some(window) = drawn.filter(|window| window.content_contains(ctx.frame.cursor)) else {
                    return Response::ignored();
                };
                let old = window.tiles_per_pixel;
                let offset = (
                    ctx.frame.cursor.x - window.content_at.x - window.content_extent.0 / 2,
                    ctx.frame.cursor.y - window.content_at.y - window.content_extent.1 / 2,
                );
                self.zoom_steps =
                    (self.zoom_steps - notches.signum() as i8).clamp(MIN_ZOOM_STEPS, MAX_ZOOM_STEPS);
                let new = tpp(ctx.frame.view.map, window.content_extent, self.zoom_steps);
                let centre = RadarTile::new(
                    (window.centre.x() as f32 + offset.0 as f32 * (old - new))
                        .round()
                        .max(0.0) as u32,
                    (window.centre.y() as f32 + offset.1 as f32 * (old - new))
                        .round()
                        .max(0.0) as u32,
                );
                self.centre = Some(clamp_centre(
                    centre,
                    ctx.frame.view.map,
                    window.content_extent,
                    new,
                ));
                Response::changed()
            }
            Input::Press(Button::Left) if ctx.under_pointer => {
                let Some(window) = drawn else {
                    return Response::ignored();
                };
                if window.close_contains(ctx.frame.cursor) {
                    self.close_held = true;
                    return Response::changed().with(Effect::Raise);
                }
                if window.content_contains(ctx.frame.cursor) {
                    self.drag_from = Some(ctx.frame.cursor);
                    self.drag_centre = Some(self.centre.unwrap_or(window.centre));
                    return Response::changed().with(Effect::Raise);
                }
                Response::ignored()
            }
            Input::Move => {
                let (Some(from), Some(centre), Some(window)) = (self.drag_from, self.drag_centre, drawn)
                else {
                    return Response::ignored();
                };
                let dx = (ctx.frame.cursor.x - from.x) as f32 * window.tiles_per_pixel;
                let dy = (ctx.frame.cursor.y - from.y) as f32 * window.tiles_per_pixel;
                // `drawn` describes the last rendered frame. Several cursor
                // moves can arrive before there is another frame, so derive
                // every result from the pointer and centre at the start of the
                // drag, rather than a stale frame or rounded intermediate.
                let moved = RadarTile::new(
                    (centre.x() as f32 - dx).round().max(0.0) as u32,
                    (centre.y() as f32 - dy).round().max(0.0) as u32,
                );
                self.centre = Some(clamp_centre(
                    moved,
                    ctx.frame.view.map,
                    window.content_extent,
                    window.tiles_per_pixel,
                ));
                Response::changed()
            }
            Input::Release(Button::Left) if self.close_held => {
                self.close_held = false;
                if drawn.is_some_and(|window| window.close_contains(ctx.frame.cursor)) {
                    Response::changed().with(Effect::Close)
                } else {
                    Response::changed()
                }
            }
            Input::Release(Button::Left) if self.drag_from.take().is_some() => {
                self.drag_centre = None;
                Response::consumed()
            }
            _ => Response::ignored(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panes::fixture;
    use openshard_protocol::serial::Serial;

    #[test]
    fn an_unpressed_move_does_not_start_or_pan_the_canvas() {
        let install = fixture::Install::shipping([]);
        let view = fixture::world(Serial::new(1).unwrap());
        let mut pane = WorldMapPane::default();
        let response = pane.handle(
            Input::Move,
            &install.ctx(&view, None, GumpPixel::new(300, 200), true),
        );
        assert!(!response.taken);
        assert_eq!(pane.centre, None);
        assert_eq!(pane.drag_from, None);
    }

    #[test]
    fn layout_has_a_plate_title_close_button_and_world_centre() {
        let install = fixture::Install::shipping([]);
        let view = fixture::world(Serial::new(1).unwrap());
        let pane = WorldMapPane::default();
        let Some(Drawn::WorldMap(window)) =
            pane.layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
        else {
            panic!("the map lays out");
        };
        assert_eq!(window.centre, RadarTile::new(3072, 2048));
        assert!(!window.pictures.is_empty());
        assert_eq!(window.lines[0].text, "World WorldMap");
    }

    /// The window this pane is: a press inside the canvas takes the drag and
    /// raises, and the move that follows pans the world by exactly the
    /// distance the pointer travelled, in that zoom's own tiles.
    #[test]
    fn a_press_then_a_move_pans_the_canvas_by_the_pointers_own_distance() {
        let install = fixture::Install::shipping([]);
        let view = fixture::world(Serial::new(1).unwrap());
        // Zoomed in far enough that the visible rectangle is smaller than the
        // facet: at the fit-to-window default the whole world is on screen and
        // every pan is clamped away, which would assert nothing.
        let mut pane = WorldMapPane {
            zoom_steps: 6,
            ..WorldMapPane::default()
        };
        let drawn = pane
            .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
            .expect("the map lays out");
        let Drawn::WorldMap(window) = &drawn else {
            panic!("the map lays out");
        };
        let centre = window.centre;
        let tiles_per_pixel = window.tiles_per_pixel;
        let from = GumpPixel::new(
            window.content_at.x + window.content_extent.0 / 2,
            window.content_at.y + window.content_extent.1 / 2,
        );

        let press = pane.handle(
            Input::Press(Button::Left),
            &install.ctx(&view, Some(&drawn), from, true),
        );
        assert!(press.taken);
        assert!(matches!(press.out.as_slice(), [Effect::Raise]));

        let moved = pane.handle(
            Input::Move,
            &install.ctx(&view, Some(&drawn), from.offset(GumpPixel::new(-40, 25)), true),
        );
        assert!(moved.taken);
        // The canvas is dragged, so the world moves the other way: the hand
        // pulls the picture west, which walks the centre east.
        assert_eq!(
            pane.centre,
            Some(RadarTile::new(
                (centre.x() as f32 + 40.0 * tiles_per_pixel).round() as u32,
                (centre.y() as f32 - 25.0 * tiles_per_pixel).round() as u32,
            )),
        );
    }

    /// `clamp_centre`'s whole reason, and R6's own headline claim. Every zoom
    /// step, dragged hard into both corners: the visible rectangle stays
    /// inside the facet, so the canvas can never be pushed off its own frame
    /// into a field of `UNKNOWN`.
    #[test]
    fn the_canvas_cannot_be_dragged_out_of_its_own_frame_at_any_zoom() {
        let install = fixture::Install::shipping([]);
        let view = fixture::world(Serial::new(1).unwrap());
        for steps in MIN_ZOOM_STEPS..=MAX_ZOOM_STEPS {
            for pull in [-4000, 4000] {
                let mut pane = WorldMapPane {
                    zoom_steps: steps,
                    ..WorldMapPane::default()
                };
                let drawn = pane
                    .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
                    .expect("the map lays out");
                let Drawn::WorldMap(window) = &drawn else {
                    panic!("the map lays out");
                };
                let from = GumpPixel::new(
                    window.content_at.x + window.content_extent.0 / 2,
                    window.content_at.y + window.content_extent.1 / 2,
                );
                let _ = pane.handle(
                    Input::Press(Button::Left),
                    &install.ctx(&view, Some(&drawn), from, true),
                );
                let _ = pane.handle(
                    Input::Move,
                    &install.ctx(&view, Some(&drawn), from.offset(GumpPixel::new(pull, pull)), true),
                );

                // Laid out again, because the layout is what a frame draws
                // with — and it clamps a second time, against this zoom.
                let drawn = pane
                    .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
                    .expect("the map lays out");
                let Drawn::WorldMap(window) = &drawn else {
                    panic!("the map lays out");
                };
                let visible = |pixels: i32, tiles: u16| {
                    (pixels as f32 * window.tiles_per_pixel)
                        .ceil()
                        .min(f32::from(tiles)) as u32
                };
                for (centre, pixels, tiles) in [
                    (window.centre.x(), window.content_extent.0, view.map.width),
                    (window.centre.y(), window.content_extent.1, view.map.height),
                ] {
                    let visible = visible(pixels, tiles);
                    let first = centre.saturating_sub(visible / 2);
                    assert!(
                        first + visible <= u32::from(tiles),
                        "zoom {steps}, pull {pull}: {first}..{} leaves a facet {tiles} wide",
                        first + visible,
                    );
                }
            }
        }
    }

    #[test]
    fn successive_moves_accumulate_from_the_current_map_centre() {
        let install = fixture::Install::shipping([]);
        let view = fixture::world(Serial::new(1).unwrap());
        let mut pane = WorldMapPane {
            zoom_steps: 6,
            ..WorldMapPane::default()
        };
        let drawn = pane
            .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
            .expect("the map lays out");
        let Drawn::WorldMap(window) = &drawn else {
            panic!("the map lays out");
        };
        let from = GumpPixel::new(
            window.content_at.x + window.content_extent.0 / 2,
            window.content_at.y + window.content_extent.1 / 2,
        );
        let _ = pane.handle(
            Input::Press(Button::Left),
            &install.ctx(&view, Some(&drawn), from, true),
        );
        let _ = pane.handle(
            Input::Move,
            &install.ctx(&view, Some(&drawn), from.offset(GumpPixel::new(-20, 0)), true),
        );
        let _ = pane.handle(
            Input::Move,
            &install.ctx(&view, Some(&drawn), from.offset(GumpPixel::new(-40, 0)), true),
        );

        assert_eq!(
            pane.centre,
            Some(RadarTile::new(
                (window.centre.x() as f32 + 40.0 * window.tiles_per_pixel).round() as u32,
                window.centre.y(),
            )),
        );
    }

    #[test]
    fn zoom_stays_between_full_facet_and_close_inspection() {
        let install = fixture::Install::shipping([]);
        let view = fixture::world(Serial::new(1).unwrap());
        let mut pane = WorldMapPane::default();
        let drawn = pane
            .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
            .expect("the map lays out");
        let Drawn::WorldMap(window) = &drawn else {
            panic!("the map lays out");
        };
        let cursor = GumpPixel::new(
            window.content_at.x + window.content_extent.0 / 2,
            window.content_at.y + window.content_extent.1 / 2,
        );

        for _ in 0..32 {
            let _ = pane.handle(Input::Wheel(1.0), &install.ctx(&view, Some(&drawn), cursor, true));
        }
        assert_eq!(pane.zoom_steps, MIN_ZOOM_STEPS);

        for _ in 0..32 {
            let _ = pane.handle(
                Input::Wheel(-1.0),
                &install.ctx(&view, Some(&drawn), cursor, true),
            );
        }
        assert_eq!(pane.zoom_steps, MAX_ZOOM_STEPS);
    }

    #[test]
    fn close_button_raises_on_press_and_closes_on_release() {
        let install = fixture::Install::shipping([]);
        let view = fixture::world(Serial::new(1).unwrap());
        let mut pane = WorldMapPane::default();
        let drawn = pane
            .layout(&install.ctx(&view, None, GumpPixel::new(0, 0), true).frame)
            .unwrap();
        let Drawn::WorldMap(window) = &drawn else {
            panic!("the map lays out");
        };
        let cursor = window.close_at.offset(GumpPixel::new(1, 1));
        let press = pane.handle(
            Input::Press(Button::Left),
            &install.ctx(&view, Some(&drawn), cursor, true),
        );
        assert!(press.taken);
        assert!(matches!(press.out.as_slice(), [Effect::Raise]));
        let release = pane.handle(
            Input::Release(Button::Left),
            &install.ctx(&view, Some(&drawn), cursor, true),
        );
        assert!(matches!(release.out.as_slice(), [Effect::Close]));
    }
}
