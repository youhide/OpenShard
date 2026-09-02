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
//! [`Owner`] is that question asked once, and [`Edit`] and [`Hotkey`] are the
//! binding tables of two of the three: what a key does to a line being typed,
//! and what it does to the world. (The third owner, a pane's `{ textentry }`,
//! reads three keys and they are `panes::Key`'s, next to the windows that answer
//! them.)
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
//!   field* inside it is focused. The F1 staff item creator is such a field;
//!   chat and ordinary game windows remain outside egui. It is written as a
//!   live question rather than a panel-specific rule so a future field follows
//!   the same ownership contract.

use winit::keyboard::KeyCode;

/// One pressed-key gesture, including the modifier R2 needs to keep camera pan
/// distinct from structural-floor selection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Gesture {
    code: KeyCode,
    ctrl: bool,
}

impl Gesture {
    pub(crate) const fn new(code: KeyCode, ctrl: bool) -> Self {
        Self { code, ctrl }
    }
}

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
    /// The body. Walk keys, war mode, and [`Hotkey`]'s own table — which is
    /// consulted *only* under this owner, which is the point.
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
    /// **Shift+Tab**, beside the button `chat::channel_button` draws rather than
    /// instead of it: the mouse has the control and the keyboard keeps a way to
    /// turn it without leaving the line. The plain `Tab` it used to be is worth
    /// more as completion — a channel is chosen once a conversation while a
    /// command is completed once a word.
    NextChannel,
    /// Highlight the completion above the current one.
    PreviousCandidate,
    /// Highlight the completion below the current one.
    NextCandidate,
    /// Delete the `char` before the caret.
    Backspace,
    /// Delete the whitespace and word before the caret.
    BackspaceWord,
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
    /// Modifier states are what `ModifiersChanged` last reported — see
    /// [`crate::input::Input`], which is where a held modifier honestly lives;
    /// `KeyEvent` carries no modifiers of its own.
    pub(crate) const fn of(code: KeyCode, shift: bool, ctrl: bool) -> Option<Self> {
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
            KeyCode::Backspace if ctrl => Self::BackspaceWord,
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

/// What a key means to the body, the eye and the picture — [`Owner::World`]'s
/// own bindings.
///
/// [`Edit`]'s twin, one owner along, and written for the same three reasons:
/// until this existed the world's keys were a `match` on a `KeyCode` inside
/// `App::window_event`, so none of them could be **rebound** (there was no table
/// to rebind), none could be **tested** (the arms do things to a window), and
/// nothing could **say what is bound** without reading four hundred lines of
/// event loop. A key is a name here and the doing is at the call site, which is
/// the split that lets all three happen.
///
/// # What is deliberately not in the table
///
/// Three keys the world reads are answered before this one is consulted, and
/// each is a different reason rather than an oversight:
///
/// * **The arrows**, which are held rather than pressed — a step is due every
///   step's length while one is down, on our clock and not the operating
///   system's repeat rate. See [`crate::keys::Held`].
/// * **`Tab`**, war mode, whose press toggles the stance. Its release is still
///   observed solely to make the next press a new toggle.
/// * **`Escape`**, which takes the topmost window down, and so is answered by
///   the window layer before the world is asked at all.
///
/// A bindings window would have to reach all three eventually. It would also
/// have to answer what they *are*, and a held key and a pressed one are not the
/// same kind of binding — which is exactly why they are not filed here as though
/// they were.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Hotkey {
    /// Open the speech line — the reference client's own gesture.
    ///
    /// Costs the body nothing: movement is arrows only, so no key is taken from
    /// it. What it does take is every following letter, which is why the call
    /// site also lets go of whatever was held (see [`Owner::Speech`]).
    Speak,
    /// The dev window, the same switch the status strip's `dev` toggle is.
    ///
    /// Two ways in rather than one because the state is remembered: a window
    /// closed once stays closed across launches, and the strip that reopens it
    /// is itself a thing you have to know is there. A key is what you reach for
    /// without knowing anything.
    DevWindow,
    /// Put the eye back on the body and lock it there.
    Relock,
    /// The character sheet, by the protocol's own paperdoll request rather than
    /// a double click: it must stay reachable when the body is obscured or a
    /// shop window covers it.
    Paperdoll,
    /// The worn backpack, exactly as the paperdoll's own button opens it — that
    /// button can be covered or closed, and this cannot.
    Inventory,
    /// Use the last object the player explicitly used.
    UseLastItem,
    /// Open the shard's craft catalogue. Unlike the skill sheet this is a
    /// server-owned view: it reads the live backpack and nearby workbench.
    CraftCatalogue,
    /// Search permissioned storage in the house currently occupied. Ctrl+I
    /// leaves plain I for the backpack.
    HouseInventory,
    /// The minimap. Local, like the skills and status windows and unlike
    /// [`Paperdoll`](Self::Paperdoll): no round trip.
    ///
    /// Provisional — the minimap's own opening affordance is still an open
    /// product decision (`docs/map/minimap_lod_plan.md` phase 4).
    Minimap,
    /// The full, pannable facet map. Ctrl+M leaves plain M for the radar.
    WorldMap,
    /// Lift the eye rather than the body, which is a pan: the map has no
    /// vertical axis to walk along, only a projection that folds `z` into `y`.
    PanUp,
    /// The same, downward.
    PanDown,
    /// Open the next structural floor of the current building picture.
    FloorUp,
    /// Open the structural floor below it.
    FloorDown,
    /// A fixed mixed-case ASCII line, said without ever going through the
    /// keyboard — no xkb group, no input method, no text field.
    ///
    /// A diagnostic and not a feature: whatever shows up over the head from this
    /// key is exactly what `0xAD` → `0xAE` → `text::collect` do with known-good
    /// bytes, with typing ruled out as a variable.
    SpeechProbe,
    /// Night on and off.
    ///
    /// A key and not a setting because the only honest test of firelight is the
    /// two pictures side by side. The shard now supplies time of day, but F10
    /// remains its separate local lighting switch. The five below are keys for the
    /// same reason, and the
    /// reason is `docs/render/evidence/pitfalls.md`'s: what is being read is the
    /// difference between two pictures of *one instant*, and a hand that has to
    /// find a checkbox has moved the camera by the time it is back.
    Night,
    /// The sun on and off — the only honest test of a shadow.
    Sunlight,
    /// The sky field on and off: what a roof does to the light under it,
    /// against a flat ambient.
    SkyField,
    /// The torch in the player's own hand — also the only way to see what the
    /// map's own fires are doing without a beam swinging across them.
    Lantern,
    /// The occlusion grid drawn as solids.
    Solids,
    /// The world image off underneath them: the box itself, with nothing behind
    /// it arguing about what shape it is.
    SolidsOnly,
    /// How much of the grid either view draws — "is that floor missing, or is it
    /// under my feet".
    SolidsEverything,
    /// The lighting's own values, one after another. See `crate::debug::View`.
    LightView,
    /// What a fragment whose ray met no box is answered with, one after another
    /// — `impostor::Fringe`.
    Fringe,
    /// This frame, written out: every plane the blit can draw of it, plus the
    /// inputs it was assembled from.
    ///
    /// A key and not a setting more than any of the others: what is dumped is
    /// the instant a person is looking at, and anything that had to be switched
    /// on beforehand would dump a different one.
    FrameDump,
    /// Stamp a mark into the combat recorder: *"it stalled here"*.
    ///
    /// A key for [`FrameDump`](Self::FrameDump)'s reason, and more so. What a
    /// mark records is the instant a person decided nothing was happening,
    /// together with what was drawn over their body at it — and a hand that has
    /// to find F1, then a tab, then a button has let that instant go. The panel
    /// has the same button for when there is time to type a note.
    ///
    /// It also **writes the log out**, for the same reason it is a key: a mark
    /// left in a ring that only the panel can read has moved those three steps
    /// from the stamping to the reading rather than removed them, and the ring
    /// dies with the process.
    MarkCombat,
}

impl Hotkey {
    /// Every binding there is, in the order a person would read them: what the
    /// game does, then what the diagnostics do.
    ///
    /// The list exists so that "no two actions share a key" is a test rather
    /// than a thing somebody notices — see [`Self::key`], which is also the
    /// question a bindings window asks.
    pub(crate) const ALL: [Self; 26] = [
        Self::Speak,
        Self::DevWindow,
        Self::Relock,
        Self::Paperdoll,
        Self::Inventory,
        Self::UseLastItem,
        Self::CraftCatalogue,
        Self::HouseInventory,
        Self::Minimap,
        Self::WorldMap,
        Self::PanUp,
        Self::PanDown,
        Self::FloorUp,
        Self::FloorDown,
        Self::SpeechProbe,
        Self::Night,
        Self::Sunlight,
        Self::SkyField,
        Self::Lantern,
        Self::Solids,
        Self::SolidsOnly,
        Self::SolidsEverything,
        Self::LightView,
        Self::Fringe,
        Self::FrameDump,
        Self::MarkCombat,
    ];

    /// Which key this is on today.
    ///
    /// The inverse of [`Self::of`], and the half a bindings window would replace
    /// first: a table that can only be read forwards cannot draw itself.
    pub(crate) const fn key(self) -> KeyCode {
        match self {
            Self::Speak => KeyCode::Enter,
            Self::DevWindow => KeyCode::F1,
            Self::Relock => KeyCode::Home,
            Self::Paperdoll => KeyCode::KeyP,
            Self::Inventory | Self::HouseInventory => KeyCode::KeyI,
            // F2 is already Fringe and every other function key is occupied.
            Self::UseLastItem => KeyCode::KeyU,
            Self::CraftCatalogue => KeyCode::KeyC,
            Self::Minimap | Self::WorldMap => KeyCode::KeyM,
            Self::PanUp | Self::FloorUp => KeyCode::PageUp,
            Self::PanDown | Self::FloorDown => KeyCode::PageDown,
            Self::SpeechProbe => KeyCode::F9,
            Self::Night => KeyCode::F10,
            Self::Sunlight => KeyCode::F8,
            Self::SkyField => KeyCode::F6,
            Self::Lantern => KeyCode::F7,
            Self::Solids => KeyCode::F5,
            Self::SolidsOnly => KeyCode::F3,
            Self::SolidsEverything => KeyCode::F4,
            Self::LightView => KeyCode::F11,
            Self::Fringe => KeyCode::F2,
            Self::FrameDump => KeyCode::F12,
            // Every function key is taken, and this one wants to be reachable
            // without looking: `k` for mark, and letters are free while the
            // speech line is shut (it opens on Enter).
            Self::MarkCombat => KeyCode::KeyK,
        }
    }

    /// The full key-plus-modifier binding. Page Up/Down are intentionally the
    /// only chords today: plain selects floors; Ctrl keeps the camera pan.
    pub(crate) const fn gesture(self) -> Gesture {
        Gesture::new(
            self.key(),
            matches!(
                self,
                Self::PanUp | Self::PanDown | Self::WorldMap | Self::HouseInventory
            ),
        )
    }

    /// What a pressed key does in the world, or `None` for one that does
    /// nothing.
    ///
    /// **Answered out of [`ALL`](Self::ALL) rather than by a second `match`.** A
    /// forward table and a backward one are two statements of the same fact, and
    /// two statements of one fact are a pair that can disagree — `docs/render/design_frame_assembly.md`
    /// one rung down from a frame. The scan is nineteen comparisons on a
    /// keystroke, which is not a cost worth a second table to avoid.
    ///
    /// `NumpadEnter` is the one key with two spellings, and it is answered
    /// before the table rather than given a variant of its own: it is the same
    /// action on the same keyboard, and [`Self::key`] answers with the one a
    /// legend would print.
    pub(crate) fn of(gesture: Gesture) -> Option<Self> {
        if matches!(gesture.code, KeyCode::NumpadEnter) && !gesture.ctrl {
            return Some(Self::Speak);
        }
        Self::ALL.into_iter().find(|hotkey| hotkey.gesture() == gesture)
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

    use super::{
        Edit,
        Gesture,
        Hotkey,
        Owner,
        egui_may_see,
    };

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
        assert_eq!(Edit::of(KeyCode::Tab, false, false), Some(Edit::Complete));
        assert_eq!(Edit::of(KeyCode::Tab, true, false), Some(Edit::NextChannel));
    }

    #[test]
    fn ctrl_backspace_deletes_a_word_and_plain_backspace_a_character() {
        assert_eq!(
            Edit::of(KeyCode::Backspace, false, true),
            Some(Edit::BackspaceWord)
        );
        assert_eq!(Edit::of(KeyCode::Backspace, false, false), Some(Edit::Backspace));
    }

    /// The world's table, read both ways: every binding answers for its own key
    /// and no two of them answer for one. Neither half was checkable while the
    /// bindings were arms of a `match` inside the event loop.
    #[test]
    fn every_hotkey_owns_exactly_one_key_and_no_key_is_owned_twice() {
        let mut keys: Vec<Gesture> = Vec::new();
        for hotkey in Hotkey::ALL {
            let key = hotkey.gesture();
            assert_eq!(
                Hotkey::of(key),
                Some(hotkey),
                "{hotkey:?} says it is on {key:?}, which answers with something else"
            );
            assert!(
                !keys.contains(&key),
                "{key:?} is bound twice, the second to {hotkey:?}"
            );
            keys.push(key);
        }
        assert_eq!(keys.len(), Hotkey::ALL.len());
    }

    #[test]
    fn page_keys_select_floors_and_ctrl_keeps_camera_pan() {
        assert_eq!(
            Hotkey::of(Gesture::new(KeyCode::PageUp, false)),
            Some(Hotkey::FloorUp)
        );
        assert_eq!(
            Hotkey::of(Gesture::new(KeyCode::PageDown, false)),
            Some(Hotkey::FloorDown)
        );
        assert_eq!(
            Hotkey::of(Gesture::new(KeyCode::PageUp, true)),
            Some(Hotkey::PanUp)
        );
        assert_eq!(
            Hotkey::of(Gesture::new(KeyCode::PageDown, true)),
            Some(Hotkey::PanDown)
        );
    }

    #[test]
    fn use_last_item_is_bound_to_the_free_u_key() {
        assert_eq!(
            Hotkey::of(Gesture::new(KeyCode::KeyU, false)),
            Some(Hotkey::UseLastItem)
        );
    }

    /// The three keys the world reads and this table deliberately does not hold
    /// — see [`Hotkey`]'s own doc for why each is answered earlier. A binding
    /// that appeared here for one of them would be a second answer, and the
    /// first one is the one that runs.
    #[test]
    fn the_held_keys_and_the_window_key_are_not_bindings_here() {
        assert_eq!(
            Hotkey::of(Gesture::new(KeyCode::Tab, false)),
            None,
            "war mode is held, not pressed"
        );
        assert_eq!(
            Hotkey::of(Gesture::new(KeyCode::ArrowUp, false)),
            None,
            "an arrow is a step, held"
        );
        assert_eq!(
            Hotkey::of(Gesture::new(KeyCode::Escape, false)),
            None,
            "the topmost window's"
        );
    }

    /// A letter the body walks on is not a hotkey, and a letter that opens a
    /// window is — which is only safe because a letter reaching the world at all
    /// means the speech line does not have the keyboard (see [`Owner`]).
    #[test]
    fn a_letter_is_a_hotkey_only_where_the_world_owns_the_keyboard() {
        assert_eq!(
            Hotkey::of(Gesture::new(KeyCode::KeyP, false)),
            Some(Hotkey::Paperdoll)
        );
        assert_eq!(
            Edit::of(KeyCode::KeyP, false, false),
            None,
            "the same key, typed, is text"
        );
        assert_eq!(Hotkey::of(Gesture::new(KeyCode::KeyZ, false)), None);
    }

    /// An arrow means the caret or the popup here — never a step. The body is a
    /// different owner, and this is the table that says so.
    #[test]
    fn arrows_are_the_lines_own_and_a_letter_is_not_in_the_table() {
        assert_eq!(Edit::of(KeyCode::ArrowLeft, false, false), Some(Edit::Left));
        assert_eq!(
            Edit::of(KeyCode::ArrowUp, false, false),
            Some(Edit::PreviousCandidate)
        );
        assert_eq!(
            Edit::of(KeyCode::KeyA, false, false),
            None,
            "a letter is text, not a binding"
        );
        assert_eq!(
            Edit::of(KeyCode::F5, false, false),
            None,
            "a hotkey is the world's, not the line's"
        );
    }
}
