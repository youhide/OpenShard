//! The client-side window for an opened spellbook.
//!
//! A spellbook is not a generic container: `0x24` says it looks like a book,
//! while `0xBF 0x1B` says which spell rows exist.  This pane joins that latter
//! packet to the local scroll position and turns a chosen row into a cast.

use openshard_client_net::action::Outgoing;
use openshard_client_render::gump::{GumpArt, GumpPixel};
use openshard_client_render::spellbook::{self, Hit};
use openshard_protocol::serial::Serial;

use crate::panes::{Button, Effect, Input, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

/// One opened book and the state no packet carries.
#[derive(Debug)]
pub struct SpellbookPane {
    book: Serial,
    scroll: i32,
    held: Option<Hit>,
}

impl SpellbookPane {
    pub const fn new(book: Serial) -> Self {
        Self {
            book,
            scroll: 0,
            held: None,
        }
    }

    fn content(&self, frame: &PaneFrame<'_>) -> i32 {
        frame.view.spellbooks.get(&self.book).map_or(0, |book| {
            spellbook::entries(book.offset, book.content).len() as i32 * spellbook::ROW_HEIGHT
        })
    }

    fn clamp_scroll(&mut self, content: i32) {
        self.scroll = self
            .scroll
            .clamp(0, (content - spellbook::Window::viewport_height()).max(0));
    }

    fn press(&mut self, window: &spellbook::Window, ctx: &PaneCtx<'_>) -> Response {
        let raised = Response::changed().with(Effect::Raise);
        if let Some(hit) = window.hit(ctx.frame.cursor, ctx.frame.files.gump_atlas) {
            self.held = Some(hit);
            raised
        } else {
            raised.with(Effect::Grab)
        }
    }

    fn release(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let Some(held) = self.held.take() else {
            return Response::ignored();
        };
        let Some(Drawn::Spellbook(window)) = ctx.drawn else {
            return Response::changed();
        };
        if window.hit(ctx.frame.cursor, ctx.frame.files.gump_atlas) != Some(held) {
            return Response::changed();
        }
        match held {
            Hit::Cast(spell) => Response::changed().with(Effect::Net(Outgoing::CastSpell {
                spellbook: self.book,
                spell,
            })),
        }
    }

    fn wheel(&mut self, notches: f32, ctx: &PaneCtx<'_>) -> Response {
        if notches == 0.0 {
            return Response::ignored();
        }
        let before = self.scroll;
        let delta = if notches > 0.0 {
            -spellbook::ROW_HEIGHT
        } else {
            spellbook::ROW_HEIGHT
        };
        self.scroll += delta;
        self.clamp_scroll(self.content(&ctx.frame));
        if self.scroll == before {
            Response::consumed()
        } else {
            Response::changed()
        }
    }
}

impl SpellbookPane {
    pub(super) fn art(&self, _: &PaneFrame<'_>) -> Vec<GumpArt> {
        Vec::new()
    }

    pub(super) fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let book = frame.view.spellbooks.get(&self.book)?;
        Some(Drawn::Spellbook(spellbook::window(
            book.offset,
            book.content,
            self.scroll,
            GumpPixel::new(0, 0),
        )))
    }

    pub(super) fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match input {
            Input::Press(Button::Left) if ctx.under_pointer => {
                let Some(Drawn::Spellbook(window)) = ctx.drawn else {
                    return Response::ignored();
                };
                self.press(window, ctx)
            }
            Input::Release(Button::Left) => self.release(ctx),
            Input::Wheel(notches) if ctx.under_pointer => self.wheel(notches, ctx),
            Input::Move
            | Input::Press(Button::Right)
            | Input::Release(Button::Right)
            | Input::Key(_)
            | Input::Answered(_)
            | Input::Press(Button::Left)
            | Input::Wheel(_) => Response::ignored(),
        }
    }
}
