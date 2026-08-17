//! The skill sheet as a component: the second window kind to own its state,
//! its layout and its input.
//!
//! Step 2 of `docs/window_components.md`, and the step where one field stops
//! meaning two things. `Windows::skills` was an `Option<skills::Tree>` whose
//! `Some` *was* "the window is open", so closing the window and forgetting
//! which headings were shut were the same write, and four files did it —
//! `close_window`, `sync_own_windows`, the disconnect arm of `net_command` and
//! the paperdoll's Skills button. Now the tree is a field of this pane, the
//! pane lives in the window's record, and the window being in
//! [`Windows::own_windows`](crate::windows::Windows::own_windows) is the whole
//! of "it is open" — the same answer every other kind already gives.
//!
//! # What the sheet is, on the wire
//!
//! Almost nothing. A `0x3A` carries the numbers and no window, which is why
//! this kind's *existence* is local (see [`LocalWindow`](crate::panes::LocalWindow)):
//! the shard sends a skill list at world entry and the player is not shown a
//! window for it. What goes out from here is two packets — a lock arrow's
//! `0x3A` reply and a use button's `0x12` — and both are ordinary
//! [`Effect::Net`]s.

use openshard_client_net::action::Outgoing;
use openshard_client_net::view::WorldView;
use openshard_client_render::gump::{GumpArt, GumpPixel};
use openshard_client_render::skills;
use openshard_protocol::skill::SkillLock;
use openshard_protocol::wire::RawSkillId;
use openshard_uofiles::skills::SkillId;

use crate::panes::{Button, Effect, Input, Pane, PaneCtx, PaneFrame, Response};
use crate::resources::Resources;
use crate::windows::Drawn;

/// This character's skill sheet, open.
#[derive(Debug, Default)]
pub struct SkillsPane {
    /// Which headings are shut, how far down the list is scrolled, and which
    /// lock arrows the player has clicked since it opened.
    ///
    /// Everything about this window that no packet carries. It is thrown away
    /// with the window, which is what closing one has always meant — the
    /// reference client does not remember either, and "a window with no
    /// memory" is the backlog entry every kind here shares.
    tree: skills::Tree,
    /// What the mouse went down on, if anything.
    ///
    /// The same "still on the same picture" rule a dialog's buttons and a
    /// paperdoll's follow: a press that slid off a control is not a click on
    /// it. Keyed by *what* was pressed and not by which picture, because the
    /// sheet is laid out afresh every frame and an index would name a different
    /// row by the time the button came up.
    ///
    /// A held [`skills::Hit::Thumb`] is also what makes the bar follow the
    /// pointer — see [`SkillsPane::drag_thumb`].
    held: Option<skills::Hit>,
}

impl SkillsPane {
    /// How tall the list is with the headings the player has shut, which is
    /// what every scroll is clamped against.
    ///
    /// Measured rather than remembered: a heading toggled, a skill the shard
    /// has just named for the first time, or a different set of client files
    /// all change it, and a cached height would clamp a scroll against a list
    /// that is no longer there.
    fn content(&self, resources: &Resources) -> i32 {
        skills::content_height(&resources.skill_names, &resources.skill_groups, &self.tree)
    }

    /// One notch over the window: a row up or down, and what that was worth.
    ///
    /// **The notch is taken either way**, the same as a shop's catalogue — see
    /// [`VendorPane::wheel`](crate::panes::vendor). A sheet already at either
    /// end still swallows the wheel, which is what every list in every client
    /// does, and handing it on because nothing moved is how a wheel over a
    /// window became a zoom of the map behind it.
    ///
    /// This is the one behaviour step 2 *changes* rather than moves.
    /// `App::scroll_skills` answered one `bool` for both questions and answered
    /// it `true`, so a sheet at its last row asked for a frame that had nothing
    /// new on it — the harmless half of the same conflation the vendor's half
    /// was a visible defect of.
    fn wheel(&mut self, notches: f32, content: i32) -> Response {
        let step = if notches > 0.0 {
            -skills::STEP
        } else {
            skills::STEP
        };
        let before = self.tree.offset();
        self.tree.scroll_by(step, content);
        if self.tree.offset() == before {
            Response::consumed()
        } else {
            Response::changed()
        }
    }

    /// Follow the pointer with a held thumb.
    ///
    /// Not `taken`: a move is not an exclusive event (see [`Input::Move`]), and
    /// the window being dragged and the item on its way out of a bag are moving
    /// on the same pointer.
    fn drag_thumb(&mut self, ctx: &PaneCtx<'_>) -> Response {
        if self.held != Some(skills::Hit::Thumb) {
            return Response::ignored();
        }
        let Some(Drawn::Skills(sheet)) = ctx.drawn else {
            return Response::ignored();
        };
        let content = self.content(ctx.frame.resources);
        let offset = sheet.offset_at(ctx.frame.cursor, content);
        let before = self.tree.offset();
        self.tree.scroll_to(offset, content);
        if self.tree.offset() == before {
            Response::ignored()
        } else {
            Response::stale()
        }
    }

    /// A press somewhere in the window, against the layout it was drawn as.
    ///
    /// Every arm raises. A press that landed on none of the window's furniture
    /// is the one that moves it, and that is [`Effect::Grab`] rather than a
    /// position this pane writes — the bar runs down the inside of the scroll,
    /// and a thumb that also picked the window up would move both at once.
    fn press(&mut self, sheet: &skills::Sheet, ctx: &PaneCtx<'_>) -> Response {
        let raised = Response::changed().with(Effect::Raise);
        match sheet.hit(ctx.frame.cursor, &ctx.frame.resources.gump_atlas) {
            Some(hit) => {
                self.held = Some(hit);
                raised
            }
            None => raised.with(Effect::Grab(GumpPixel::new(
                ctx.frame.cursor.x - ctx.frame.at.x,
                ctx.frame.cursor.y - ctx.frame.at.y,
            ))),
        }
    }

    /// The release that finishes a press on one of the window's controls.
    ///
    /// Always [`Response::changed`] once a press is being held: the control was
    /// drawn pressed and has to come back up, whether or not the pointer is
    /// still on it.
    fn release(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let Some(held) = self.held.take() else {
            return Response::ignored();
        };
        let Some(Drawn::Skills(sheet)) = ctx.drawn else {
            return Response::changed();
        };
        if sheet.hit(ctx.frame.cursor, &ctx.frame.resources.gump_atlas) != Some(held) {
            return Response::changed();
        }
        self.clicked(held, sheet, ctx)
    }

    /// What one completed click means.
    fn clicked(&mut self, hit: skills::Hit, sheet: &skills::Sheet, ctx: &PaneCtx<'_>) -> Response {
        let content = self.content(ctx.frame.resources);
        match hit {
            skills::Hit::Heading(group) => {
                self.tree.toggle(group);
                // Re-measured after the toggle and not before it: shutting the
                // last heading shortens the list under a scroll that was valid
                // a line ago, and the clamp is what puts the viewport back on
                // rows that exist.
                let content = self.content(ctx.frame.resources);
                self.tree.scroll_to(self.tree.offset(), content);
                Response::changed()
            }
            skills::Hit::Up => {
                let before = self.tree.offset();
                self.tree.scroll_by(-skills::STEP, content);
                if self.tree.offset() == before {
                    Response::consumed()
                } else {
                    Response::changed()
                }
            }
            skills::Hit::Down => {
                let before = self.tree.offset();
                self.tree.scroll_by(skills::STEP, content);
                if self.tree.offset() == before {
                    Response::consumed()
                } else {
                    Response::changed()
                }
            }
            skills::Hit::Track => {
                let before = self.tree.offset();
                self.tree
                    .scroll_to(sheet.offset_at(ctx.frame.cursor, content), content);
                if self.tree.offset() == before {
                    Response::consumed()
                } else {
                    Response::changed()
                }
            }
            // Already done, on every move since the press.
            skills::Hit::Thumb => Response::changed(),
            skills::Hit::Lock(id) => {
                let next = next_lock(self.tree.lock_of(id, shard_lock(ctx.frame.view, id)));
                // Written here as well as sent, and that is deliberate: the
                // shard answers a lock with nothing at all (see
                // `World::set_skill_lock`), so a window that waited for a reply
                // would show the old arrow until the skill happened to change
                // some other way.
                self.tree.set_lock(id, next);
                Response::changed().with(Effect::Net(Outgoing::SkillLock {
                    skill: RawSkillId(id.0),
                    lock: next,
                }))
            }
            skills::Hit::Use(id) => {
                Response::changed().with(Effect::Net(Outgoing::UseSkill(RawSkillId(id.0))))
            }
        }
    }
}

/// What the shard says a skill's lock is, or the default for one it has never
/// mentioned.
fn shard_lock(view: &WorldView, id: SkillId) -> SkillLock {
    view.player
        .skills
        .get(&id.0)
        .map_or_else(SkillLock::default, |line| line.lock)
}

/// The lock arrow's cycle, which is the reference client's: raise, lower, hold.
const fn next_lock(lock: SkillLock) -> SkillLock {
    match lock {
        SkillLock::Up => SkillLock::Down,
        SkillLock::Down => SkillLock::Locked,
        SkillLock::Locked => SkillLock::Up,
    }
}

impl Pane for SkillsPane {
    /// Nothing of its own. The sheet's frame, its arrows and its lock faces are
    /// all ordinary gump art, and every picture of every window is offered to
    /// the atlas once the frame has been laid out — the sweep at the end of
    /// `render_passes::draw_gump_windows`. Naming them here would be a second
    /// answer to "what does this window draw", worked out before the layout
    /// that decides which rows are showing.
    fn art(&self, _frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        Vec::new()
    }

    /// The three sources meet here and nowhere else: the names and the grouping
    /// out of the client's own files, the numbers out of the view, and how wide
    /// a string came out of the font atlas — which is what puts a value's right
    /// edge where it belongs and starts a heading's rule at the end of its name.
    ///
    /// The wire's `u8` becomes a [`SkillId`] at this seam, which is the one
    /// place that holds both: `view::Player::skills` is keyed by what the shard
    /// said, and the files' numbering is what the window is written against. A
    /// skill the files do not name has no row to put a number on, and is
    /// dropped inside the layout rather than here.
    fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let resources = frame.resources;
        Some(Drawn::Skills(skills::window(
            &resources.skill_names,
            &resources.skill_groups,
            &self.tree,
            |id| {
                frame.view.player.skills.get(&id.0).map(|line| skills::Standing {
                    skill: *line,
                    // The player's own click, held over the shard's line — see
                    // `Tree::lock_of`.
                    lock: self.tree.lock_of(id, line.lock),
                })
            },
            |text, font| openshard_client_render::text::gump_width(text, font, &resources.font_atlas),
            frame.at,
        )))
    }

    /// A press on the window's furniture, the release that completes it, the
    /// thumb following the pointer, and a notch anywhere over the frame.
    ///
    /// The press and the notch are *located*, so both begin by asking whether
    /// this window is the one the pointer is on — see [`PaneCtx::under_pointer`].
    /// The release is not: it finishes a press this pane started, wherever the
    /// pointer has got to since.
    ///
    /// **A notch anywhere over the frame is the sheet's**, where a shop's is
    /// only its catalogue's. The two kinds disagree, they always have, and the
    /// plan's Backlog is where that is written down rather than quietly settled
    /// here — moving the input is this step, choosing between them is not.
    fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match input {
            // Offered to every window, and answered only by the one whose thumb
            // is being held.
            Input::Move => self.drag_thumb(ctx),
            Input::Release(Button::Left) => self.release(ctx),
            Input::Press(Button::Left) => {
                if !ctx.under_pointer {
                    return Response::ignored();
                }
                // Never drawn, so there is nothing to hit-test against and no
                // pixels the player can have meant. The window is still raised
                // and moved by the manager's own path.
                let Some(Drawn::Skills(sheet)) = ctx.drawn else {
                    return Response::ignored();
                };
                self.press(sheet, ctx)
            }
            Input::Wheel(notches) => {
                if !ctx.under_pointer {
                    return Response::ignored();
                }
                let content = self.content(ctx.frame.resources);
                self.wheel(notches, content)
            }
            // The right button is the manager's close, and a keystroke is a
            // dialog's: this sheet has nothing to type into, so it never asks
            // for the keyboard and is never offered one. A modal's answer is
            // the same shape — this window asks for none, so none arrives.
            Input::Press(Button::Right)
            | Input::Release(Button::Right)
            | Input::Key(_)
            | Input::Answered(_) => Response::ignored(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six rows' worth of list in a viewport that holds fewer, so there is
    /// something to scroll. The number itself is arbitrary — what the
    /// assertions are about is the *end* of the travel, which
    /// [`skills::Tree::scroll_to`] clamps.
    const CONTENT: i32 = skills::STEP * 40;

    /// **The wheel defect, on the second window kind.** A sheet at the bottom
    /// of its list takes the notch and has nothing new to draw.
    ///
    /// `App::scroll_skills` answered `true` here — one `bool` for both
    /// questions — so the notch was taken *and* a frame was asked for. Taking
    /// it is the half that mattered, and it is why the skill window never had
    /// the vendor's visible defect; asking for the frame is the half this
    /// assertion pins.
    #[test]
    fn a_sheet_at_the_bottom_takes_the_notch_and_draws_nothing() {
        let mut pane = SkillsPane::default();
        let mut notches = 0;
        // Down until it stops moving, which is the clamp rather than a count
        // this test would have to keep in step with the layout's constants.
        while pane.wheel(-1.0, CONTENT).redraw {
            notches += 1;
            assert!(notches < 100, "the list is not this long");
        }
        assert!(notches > 0, "there was something to scroll");

        let answer = pane.wheel(-1.0, CONTENT);
        assert!(answer.taken, "the notch is still the window's at the bottom");
        assert!(!answer.redraw, "and there is nothing new to draw");
    }

    /// The top is the other end of the same rule, and it is where a fresh
    /// window starts.
    #[test]
    fn a_sheet_at_the_top_takes_the_notch_too() {
        let mut pane = SkillsPane::default();
        let answer = pane.wheel(1.0, CONTENT);
        assert!(answer.taken, "already at the top, and still the window's");
        assert!(!answer.redraw);
        assert_eq!(pane.tree.offset(), 0);

        assert!(pane.wheel(-1.0, CONTENT).redraw, "and down still moves");
        assert!(pane.wheel(1.0, CONTENT).redraw);
        assert_eq!(pane.tree.offset(), 0, "back where it started");
    }

    /// A list that fits its viewport has nothing to scroll, and the notch is
    /// *still* not the camera's.
    #[test]
    fn a_short_list_does_not_scroll_and_does_not_let_the_notch_past() {
        let mut pane = SkillsPane::default();
        let answer = pane.wheel(-1.0, 0);
        assert!(answer.taken);
        assert!(!answer.redraw);
        assert_eq!(pane.tree.offset(), 0);
    }

    /// The lock arrow cycles the reference client's way, and a full turn is
    /// three clicks.
    #[test]
    fn a_lock_cycles_up_down_held_and_round() {
        assert_eq!(next_lock(SkillLock::Up), SkillLock::Down);
        assert_eq!(next_lock(SkillLock::Down), SkillLock::Locked);
        assert_eq!(next_lock(SkillLock::Locked), SkillLock::Up);
    }
}
