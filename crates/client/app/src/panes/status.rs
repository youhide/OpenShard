//! The status frame as a component: the third window kind to move in, and the
//! one that proves a pane with no state and no input at all is still a pane.
//!
//! Step 3 of `docs/window_components.md`, and the smallest of the six. There is
//! nothing here to remember between frames — every number on the frame is read
//! out of the view as it is drawn — and nothing on it to press: no arrow, no
//! button, no list. What is left is a layout and the fact that the window is
//! open, and step 2 had already made the second of those a place in
//! [`Windows::own_windows`](crate::windows::Windows::own_windows) rather than a
//! `bool` beside it.
//!
//! # What the frame is, on the wire
//!
//! A `0x11` and nothing else. It carries the name, the three stats, the armour,
//! the gold and the weight, and it carries **no window**: the shard sends one at
//! world entry, so a client that opened a frame on the data would open one at
//! every login. That is why this kind's *existence* is local — see
//! [`LocalWindow`](crate::panes::LocalWindow) — and why the Status button both
//! asks for a fresh `0x11` and opens the window, in that order.
//!
//! The health line is not in that packet's copy of the numbers:
//! [`Player::hits`](openshard_client_net::view::Player::hits) is shared with
//! `0xA1`, which refreshes it between status replies, and the layout below reads
//! it from there so the bar over the player's head and the line on this frame
//! cannot disagree.

use openshard_client_render::gump::{GumpArt, GumpPixel};
use openshard_client_render::status;

use crate::panes::{Input, Pane, PaneCtx, PaneFrame, Response};
use crate::windows::Drawn;

/// This character's status frame, open.
///
/// A unit struct, and that is the whole statement of the step: the two kinds
/// before it moved a scroll position, a set of shut headings and a held control
/// out of `Windows`, and this one has nothing of the sort to move. What it
/// gains by being a pane anyway is that it is laid out by the same call as its
/// neighbours, and that its openness is the same fact as theirs.
#[derive(Debug, Default)]
pub struct StatusPane;

impl Pane for StatusPane {
    /// Nothing of its own, for [`SkillsPane`](crate::panes::skills)'s reason:
    /// the frame art and the glyphs over it are offered to the atlas by the
    /// sweep over every laid-out window at the end of
    /// `render_passes::draw_gump_windows`, and naming them here would be a
    /// second answer to "what does this window draw", worked out before the
    /// layout that decides it.
    fn art(&self, _frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        Vec::new()
    }

    /// The frame, and eleven numbers written over it out of the view.
    ///
    /// `None` is **reachable here**, unlike the skill sheet's: the Status button
    /// opens the window and asks for a `0x11` in the same press, so there is a
    /// frame or two in which the window is open and this client has never been
    /// told a single one of the values. Drawing the empty frame then would be
    /// drawing a status window belonging to nobody; drawing nothing is the
    /// honest picture, and the window becomes visible with its numbers on the
    /// frame the reply lands.
    fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let player = &frame.view.player;
        Some(Drawn::Status(status::window(
            player.status.as_ref()?,
            player.hits?,
            // Window-local — see `PaneFrame::cursor`'s doc.
            GumpPixel::new(0, 0),
        )))
    }

    /// Nothing. There is no control on the old status frame — the reference
    /// client's own `StatusGumpOld` has none either — so every input a player
    /// aims at this window is one of the manager's: raising it, picking it up by
    /// any pixel of it, and the right button that closes it. Decision 2 is what
    /// makes that the manager's rather than something repeated here, and this is
    /// the kind where the whole of a pane's input side is that sentence.
    fn handle(&mut self, _input: Input, _ctx: &PaneCtx<'_>) -> Response {
        Response::ignored()
    }
}
