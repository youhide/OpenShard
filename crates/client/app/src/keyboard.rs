//! Who owns the keyboard this instant, and what a key means to them.
//!
//! # Why this is a layer and not a ladder
//!
//! Every keystroke has to answer one question before it can mean anything:
//! *whose is it*. The body's, so `Tab` enters war mode and an arrow walks? The
//! speech line's, so a letter is a letter and an arrow moves the caret? A
//! `{ textentry }` in one of the client's own gump windows? The three read the
//! same keys and mean different things by them, and until this module existed
//! the answer was an implicit ladder of early `return`s inside
//! `App::window_event` — which meant it could not be tested, could not be
//! stated, and could be got wrong by adding an arm in the wrong place.
//!
//! [`Owner`] is that question asked once, and [`Edit`] is the binding table for
//! the one owner whose keys are not the body's.
//!
//! # The fourth owner, and the bug it was
//!
//! Above all three sits egui, and it is a special case worth writing down
//! because it cost a working key. `egui::Context::egui_wants_keyboard_input` is
//! literally `memory.focused().is_some()` — *any* widget focused, not a widget
//! that reads text — and `Tab` is what hands out that focus. So the first `Tab`
//! entered war mode and, with the same event, gave egui's first button the
//! focus; from the next frame egui claimed the whole keyboard, and `Tab`,
//! `Enter` and the arrows all stopped reaching the game. A self-arming trap: the
//! key that broke the keyboard was the key that could no longer be pressed.
//!
//! Two rules kill it, and both are here rather than in `shell.rs` so that the
//! keyboard's owners are described in one file:
//!
//! * [`egui_may_see`] — `Tab` is never given to egui, not even to be recorded.
//!   With no `Tab` there is no focus to hand out, and its focus navigation is
//!   dead by construction.
//! * `Shell::holds_keyboard` — egui may claim the keyboard only while a *text
//!   field* inside it is focused, which this client has none of (there is not
//!   one `egui::TextEdit` in the tree — every box a player types into is drawn
//!   by `chat.rs` or by `panes.rs`). It is written as a live question rather
//!   than as `false` because the day one appears is the day it must work.

use winit::keyboard::KeyCode;

/// Who a keystroke belongs to.
///
/// Ordered by precedence, and the order is the whole content of the type:
/// something being typed into wins over the body, because a player who has
/// opened a line to speak did not ask to walk.
///
/// The UI is not a variant — see the module docs. It is answered before this,
/// by `Shell::on_window_event`, and it can only ever be a focused text field in
/// egui.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Owner {
    /// The speech line: [`crate::chat::Chat`], opened with Enter.
    Speech,
    /// A `{ textentry }` in one of the client's own gump windows, clicked into.
    Pane,
    /// The body. Walk keys, war mode, and the hotkey ladder in
    /// `event_loop.rs` — which runs *only* under this owner, which is the point.
    World,
}

impl Owner {
    /// Who has the keyboard, given what is open.
    ///
    /// The two flags cannot both be true today — Enter opens the speech line and
    /// a click opens a pane's field, and each takes the keyboard from the other
    /// — but the precedence is stated anyway rather than left to whichever
    /// `if` was written first.
    pub(crate) const fn of(speech_line: bool, pane_field: bool) -> Self {
        match (speech_line, pane_field) {
            (true, _) => Self::Speech,
            (false, true) => Self::Pane,
            (false, false) => Self::World,
        }
    }
}

/// What a key means to a line being typed.
///
/// Only the keys that are *not* text: a letter is not here, because a letter is
/// whatever the window system says it is (`KeyEvent::text`, which is what an
/// input method and a keyboard layout speak through) and this table would have
/// to be a keyboard layout to answer it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Edit {
    /// Send the line.
    Submit,
    /// Put the completion popup away, or — with nothing to put away — the line.
    Cancel,
    /// Take the highlighted completion, if one is offered.
    ///
    /// **Tab**, and the reason `Tab` is worth taking off egui: the one place a
    /// player would reach for it is a half-typed `.` command, and it was going
    /// to a focus ring in a debug panel instead.
    Complete,
    /// The next channel round — say, guild, alliance, party.
    ///
    /// **Shift+Tab**, until the channel is a button on screen: the plain `Tab`
    /// it used to be is worth more as completion, and a channel is chosen once a
    /// conversation while a command is completed once a word.
    NextChannel,
    /// Highlight the completion above the current one.
    PreviousCandidate,
    /// Highlight the completion below the current one.
    NextCandidate,
    /// Delete the `char` before the caret.
    Backspace,
    /// Delete the `char` after the caret.
    Delete,
    /// Caret one `char` left.
    Left,
    /// Caret one `char` right.
    Right,
    /// Caret to the start of the line.
    Start,
    /// Caret to the end of the line.
    End,
}

impl Edit {
    /// What a key does to the line, or `None` for one that is text (or nothing).
    ///
    /// `shift` is the modifier state as `ModifiersChanged` last reported it —
    /// see `crate::input::Input::shift_held`, which is where a held modifier
    /// honestly lives; `KeyEvent` carries no modifiers of its own.
    pub(crate) const fn of(code: KeyCode, shift: bool) -> Option<Self> {
        Some(match code {
            KeyCode::Enter | KeyCode::NumpadEnter => Self::Submit,
            KeyCode::Escape => Self::Cancel,
            // The one binding that reads the modifier. Both halves are one
            // key on purpose: whichever a hand reaches for, the other is a
            // thumb away.
            KeyCode::Tab if shift => Self::NextChannel,
            KeyCode::Tab => Self::Complete,
            KeyCode::ArrowUp => Self::PreviousCandidate,
            KeyCode::ArrowDown => Self::NextCandidate,
            KeyCode::Backspace => Self::Backspace,
            KeyCode::Delete => Self::Delete,
            KeyCode::ArrowLeft => Self::Left,
            KeyCode::ArrowRight => Self::Right,
            KeyCode::Home => Self::Start,
            KeyCode::End => Self::End,
            _ => return None,
        })
    }
}

/// Whether egui is allowed to so much as *see* this key.
///
/// `false` for `Tab`, in either direction, and for nothing else — see the module
/// docs for the trap that rule exists to spring. Dropping the event rather than
/// dropping egui's *answer* is deliberate: egui's focus navigation runs off the
/// recorded event, so an event it never receives is a focus it never moves.
///
/// A `KeyCode` and not the `WindowEvent` it arrived in, because `winit`'s
/// `KeyEvent` cannot be constructed outside `winit` (its `platform_specific`
/// field is crate-private) and a rule with no test is a rule that comes back.
pub(crate) const fn egui_may_see(code: KeyCode) -> bool {
    !matches!(code, KeyCode::Tab)
}

#[cfg(test)]
mod tests {
    use winit::keyboard::KeyCode;

    use super::{Edit, Owner, egui_may_see};

    #[test]
    fn something_being_typed_into_outranks_the_body() {
        assert_eq!(Owner::of(true, false), Owner::Speech);
        assert_eq!(Owner::of(false, true), Owner::Pane);
        assert_eq!(Owner::of(false, false), Owner::World);
        assert_eq!(
            Owner::of(true, true),
            Owner::Speech,
            "the two cannot both be open, but the precedence is stated anyway"
        );
    }

    /// The defect this module was written for, at the level it can be tested at:
    /// egui never receives a `Tab`, so it never hands out the focus that made it
    /// claim the whole keyboard.
    #[test]
    fn egui_never_sees_a_tab() {
        assert!(!egui_may_see(KeyCode::Tab));
        assert!(egui_may_see(KeyCode::KeyA));
        assert!(egui_may_see(KeyCode::Enter));
        assert!(egui_may_see(KeyCode::Escape));
    }

    #[test]
    fn tab_completes_and_shift_tab_turns_the_channel() {
        assert_eq!(Edit::of(KeyCode::Tab, false), Some(Edit::Complete));
        assert_eq!(Edit::of(KeyCode::Tab, true), Some(Edit::NextChannel));
    }

    /// An arrow means the caret or the popup here — never a step. The body is a
    /// different owner, and this is the table that says so.
    #[test]
    fn arrows_are_the_lines_own_and_a_letter_is_not_in_the_table() {
        assert_eq!(Edit::of(KeyCode::ArrowLeft, false), Some(Edit::Left));
        assert_eq!(Edit::of(KeyCode::ArrowUp, false), Some(Edit::PreviousCandidate));
        assert_eq!(
            Edit::of(KeyCode::KeyA, false),
            None,
            "a letter is text, not a binding"
        );
        assert_eq!(
            Edit::of(KeyCode::F5, false),
            None,
            "a hotkey is the world's, not the line's"
        );
    }
}
