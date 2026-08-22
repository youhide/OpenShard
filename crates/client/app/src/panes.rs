//! The client's own windows as components: one type per kind, each owning the
//! state that belongs to that window alone and answering the input that lands
//! on it.
//!
//! `docs/window_components.md` is the plan this implements and its decisions
//! are the vocabulary here. A pane **takes readonly context in ([`PaneCtx`])
//! and hands mutations out ([`Effect`])**: it never holds an `&mut App`, never
//! reaches the shard, and never decides where on the screen it sits. Which
//! windows exist, in what order, and at what coordinate stays with the manager
//! — see [`crate::windows`] and decisions 2 and 8.
//!
//! # What is here
//!
//! **All six kinds have moved in** — the vendor (step 1), the skill sheet
//! (step 2), the status frame (step 3), the `0xB0` dialog (step 4), the
//! paperdoll (step 5) and the container (step 6) — and each owns its state,
//! its layout and its input. `App` no longer knows what any of them is, and no
//! window's input is answered anywhere but in its own pane.
//!
//! What [`crate::app::App::deliver`] still reaches *after* the panes, in
//! `App::fallback_gestures`, is not a window kind. It is the press that picks a
//! window up when no pane wanted it, and the two gestures over the world that
//! look like a window's and are not: the press on an item lying on the ground,
//! and the drop of a held item onto it. Step 7 of the plan gave those that rung.
//!
//! # Two names that differ from the plan's
//!
//! The plan writes `trait Pane` and `enum Pane` in the same breath, which
//! cannot both exist. The trait keeps the name — it is the concept — and the
//! static-dispatch enum over its implementors is [`AnyPane`], the ordinary Rust
//! spelling of that. The plan's `Panes::deliver` is [`crate::app::App::deliver`],
//! because the router still has to reach `App::fallback_gestures`, which needs
//! the whole `App`; the loop over the panes itself already takes nothing but the
//! pane list.

use std::time::Instant;

use openshard_client_net::action::{GumpReply, Outgoing};
use openshard_client_net::view::WorldView;
use openshard_client_render::gump::{GumpArt, GumpPixel};
use openshard_client_render::text::GumpLabel;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::Hue;

use crate::hand::{Hand, ItemDrag, PendingDrop};
use crate::resources::Resources;
use crate::windows::{Drawn, WindowSubject};

pub(crate) mod confirm;
pub(crate) mod container;
pub(crate) mod dialog;
#[cfg(test)]
pub(crate) mod fixture;
pub(crate) mod minimap;
pub(crate) mod paperdoll;
pub(crate) mod party;
mod route;
mod skills;
mod spellbook;
pub(crate) mod split;
mod status;
mod vendor;
pub(crate) mod world_map;

/// Which mouse button an input is about.
///
/// No middle button: it pans the camera (`event_loop.rs`'s `MouseInput` arm)
/// and no window kind has ever wanted it, so a pane cannot be offered one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Left,
    Right,
}

/// One input, as a pane is offered it.
///
/// Named `Input` after the plan, and always reached as `panes::Input`: the
/// crate already has an [`Input`](crate::input::Input) which is the *state* of
/// the pointer and the modifier keys rather than one event out of it. This is
/// the event; that is what the event is measured against.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Input {
    /// A button went down at [`PaneCtx::cursor`].
    Press(Button),
    /// And came back up. Where it came up is [`PaneCtx::cursor`] again, which
    /// is not where it went down — every "still on the same picture" rule in
    /// the window layer is that difference.
    Release(Button),
    /// The pointer moved to [`PaneCtx::cursor`].
    ///
    /// **Not an exclusive event.** A move is offered to panes so that a hover
    /// tint can go stale and a pane-private gesture can follow the pointer, but
    /// the manager's own gestures — the window being dragged, the item on its
    /// way out of a bag — run whether a pane took it or not, and the camera
    /// reads it too. `taken` on a move therefore says only "the world should
    /// not also act on this", and nothing in the client asks that yet.
    Move,
    /// A wheel notch, signed: positive is away from the player.
    ///
    /// A line on a wheel and a fraction of one on a touchpad — only the sign is
    /// meaningful, and how far a notch goes is the pane's own business.
    Wheel(f32),
    /// One keystroke, for the window that holds the keyboard.
    ///
    /// **The one input that is not routed by the pointer.** Every other arm here
    /// is offered to windows from the top down and stops at the one the cursor
    /// is over; this one is offered to the window named by
    /// [`Windows::keyboard`](crate::windows::Windows::keyboard) and to no other,
    /// because a player typing into a field can raise a second window over it
    /// without the keys following the pointer. That is the plan's Backlog entry
    /// about *who a modal's answer is addressed to*, answered for the keyboard:
    /// by identity, not by z-order.
    Key(Key),
    /// The client's own modal has been answered, for the window that asked.
    ///
    /// [`Input::Key`]'s routing rule for a second exclusive device: the manager
    /// remembers which press a prompt was opened over
    /// ([`Windows::prompt`](crate::windows::Windows::prompt)) and delivers the
    /// answer to that window and no other. A player may raise a bag over the
    /// prompt while it stands, so "whoever is on top" would hand the number to
    /// the wrong presser.
    ///
    /// It arrives at the top of a frame rather than from the event loop — the
    /// shell's answers are applied in `App::apply` — which changes nothing
    /// about the walk: an input is an input, and this one is addressed.
    Answered(Answer),
}

/// What the player said to the client's own modal.
///
/// One kind for now, because there is one modal: the amount picker a
/// Shift-drag opens over a stack. The dismissal is an arm rather than an
/// `Option` around the number, because "no amount" is a decision the presser
/// acts on — it puts the press down — and not a missing value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    /// How many of the stack to take.
    Split(u16),
    /// The prompt was dismissed, and the press it suspended goes with it.
    Cancelled,
}

/// One keystroke, as a window sees it.
///
/// Four arms and not a key code, because only four things can happen to a text
/// box: a character goes in, the last one comes out, the player is done with
/// it, or the player never mind. Which physical key means which is the event
/// loop's business — see `event_loop.rs`, where Enter becomes [`Key::Done`] and
/// Escape [`Key::Cancel`].
///
/// A character rather than the `&str` the keyboard produced: a `&str` would put
/// a lifetime on [`Input`], and an input method that produces two characters at
/// once produces two of these in order, which appends the same text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// One character, as the keyboard layout and the input method decided it —
    /// `winit`'s own `KeyEvent::text`, which is why this is a character and not
    /// a key code.
    Typed(char),
    /// Rub out the last one.
    Backspace,
    /// The player is finished with the box: give the keyboard back to the world.
    Done,
    /// The player never mind: whatever the box was being filled in for does not
    /// happen.
    ///
    /// The two are one key to a `{ textentry }` — a dialog's field puts itself
    /// down either way, which is what Escape means there — and **opposite
    /// answers to a modal**, which is the window kind that made the arm
    /// necessary: Enter on the amount picker takes the number, and Escape takes
    /// the press down with the prompt. One enum with two arms rather than a
    /// window kind reaching for the key code the event loop already threw away.
    Cancel,
}

/// The modifier keys, as a pane sees them.
///
/// Two of them, because two of them mean something to a window: Shift splits a
/// stack being dragged out of a bag, and Ctrl is a move order rather than a
/// heading. Passed in rather than read off `App::input`, for the same reason
/// [`PaneCtx::now`] is passed in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
}

/// The client's own files, narrowed to what a window actually reads.
///
/// Nine borrows rather than the whole of [`Resources`], and the difference is
/// not tidiness: `Resources` holds the facet, the navigation graph and the
/// 195MB animation file, none of which can be built without an install on
/// disk — so a `PaneFrame` carrying it made [`Pane::handle`] reachable from a
/// test only on a machine with a client in it. That is the whole of step 8 in
/// `docs/window_components.md`, and the answer the step's own last paragraph
/// proposed: *the two or three fields a pane actually reads instead of the
/// whole `Resources`*. It turned out to be nine, and every one of them can be
/// built from nothing — `GumpAtlas::pack`, `FontAtlas::pack`,
/// `TileData::empty`, `Art::empty`, `Default` for the three tables, and `None`
/// for the two an install may legitimately not ship.
///
/// It is also a statement of what a window is allowed to know about the
/// install: a pane that wanted the map or the animations would have to widen
/// this struct, which is a decision somebody makes rather than a field that was
/// already in reach.
#[derive(Clone, Copy)]
pub struct PaneFiles<'a> {
    /// The gump pictures packed so far. Every kind reads it: it is what a hit
    /// test picks against and what a layout measures a window's frame by.
    pub gump_atlas: &'a openshard_client_render::gump::GumpAtlas,
    /// The `fonts.mul` glyphs, for the two kinds that measure their own text —
    /// a dialog's caret and a skill row's columns.
    pub font_atlas: &'a openshard_client_render::atlas::FontAtlas,
    /// The item art, read for one thing only: the centre of an icon that is
    /// grabbed somewhere other than its own picture — see
    /// [`centre_of`](crate::hand::centre_of).
    pub art: &'a openshard_uofiles::art::Art,
    /// What a graphic *is*: its name for a label, and its layer for a drop onto
    /// a body.
    pub tiledata: &'a openshard_uofiles::tiledata::TileData,
    /// The client's gump art, or `None` for an install this build could not
    /// open it from — the paperdoll is the one kind that reads the file itself,
    /// because a doll's body is drawn from a gump rather than from the atlas.
    pub gumps: Option<&'a openshard_uofiles::gumpart::Gumps>,
    /// The client's own text table, or `None` when `Cliloc.enu` could not be
    /// read. A dialog line that named a cliloc draws nothing without it, which
    /// is the tolerance a missing gump or a missing glyph already gets.
    pub cliloc: Option<&'a openshard_uofiles::cliloc::Cliloc>,
    /// What a worn item's graphic resolves to for drawing, for the paperdoll.
    pub equip_conv: &'a openshard_uofiles::equipconv::EquipConv,
    /// What the client's files call each skill, for the sheet.
    pub skill_names: &'a openshard_uofiles::skills::Skills,
    /// Which heading each skill is filed under, for the sheet.
    pub skill_groups: &'a openshard_uofiles::skillgrp::SkillGroups,
}

impl<'a> PaneFiles<'a> {
    /// The window layer's view of an install.
    ///
    /// The one place [`Resources`] is narrowed, so that what a pane may read is
    /// a list in one place rather than whatever the next author happens to
    /// reach for through a `&Resources`.
    pub fn of(resources: &'a Resources) -> Self {
        Self {
            gump_atlas: &resources.gump_atlas,
            font_atlas: &resources.font_atlas,
            art: &resources.art,
            tiledata: &resources.tiledata,
            gumps: resources.gumps.as_ref(),
            cliloc: resources.cliloc.as_ref(),
            equip_conv: &resources.equip_conv,
            skill_names: &resources.skill_names,
            skill_groups: &resources.skill_groups,
        }
    }
}

/// What a pane may read while it is being packed and laid out.
///
/// Decision 3's context, minus the four things that only mean something while
/// an *input* is being answered — see [`PaneCtx`], which is this plus those.
/// Split in two because the two callers are two different places: a frame is
/// laid out from `render_passes::draw_gump_windows`, which has the view, the
/// files and the pointer and has no business inventing a clock, a modifier
/// state or a z-order answer to put in a field a layout never reads.
///
/// No `&mut App`, no [`Link`](crate::link::Link), no `&mut WorldView`: a pane
/// that could reach any of the three would be able to answer a click by
/// changing the world, and the whole point of [`Effect`] is that the answer is
/// *returned* instead — where the manager can order it, log it, refuse it, or a
/// test can read it.
pub struct PaneFrame<'a> {
    /// The authoritative picture. A pane holds no copy of what is in the bag or
    /// on the body; it looks both up here every time it is asked, which is why
    /// a window can never draw or click a stale item.
    pub view: &'a WorldView,
    /// The client's own files, narrowed to the nine a window reads — see
    /// [`PaneFiles`], and step 8 of `docs/window_components.md` for why that
    /// narrowing is the step's whole content.
    pub files: PaneFiles<'a>,
    /// The pointer, in this window's own gump pixels — the origin is the
    /// window's own top-left corner, not the screen's.
    ///
    /// **Window-local, not absolute.** `docs/window_components.md`'s Backlog
    /// entry of that name is what this field closes: every window kind lays
    /// its own pictures out as if it sat at `(0, 0)` (see each pane's
    /// `layout`, which always passes [`GumpPixel::new(0, 0)`] to the render
    /// crate's layout functions), so a pane's own hit test needs no
    /// subtraction to match what it drew — it reads `cursor` and compares it
    /// straight against its own pictures. The one place [`GumpPixel::new(0,
    /// 0)`] is not enough is where a picture or a pointer position has to
    /// leave the window's own frame of reference: `render_passes.rs`'s draw
    /// pass and `own_windows.rs`'s [`App::window_under_pointer`] are the two
    /// places that add the window's own absolute placement
    /// ([`OwnWindow::at`](crate::windows::OwnWindow::at), the manager's) back
    /// in — once each, which is decision 2's "one place that measures a
    /// cursor" made real. A pane itself never sees that placement: it has no
    /// `at` field to read or forget to subtract.
    pub cursor: GumpPixel,
    /// What is on the cursor, if anything, and where it is on its way to.
    ///
    /// Readonly, and the whole of decision 7: the hand is a **slot** and not a
    /// gesture — one per client because the shard has one per connection — so a
    /// pane reads it to answer "was something dropped on me", to draw a
    /// wearable's preview, and to subtract a lifted icon from its own list, and
    /// no pane may fill or empty it. The two halves of a transfer are an
    /// [`Effect::Lift`] and an [`Effect::Drop`], possibly from two different
    /// panes, possibly with a walk between them.
    ///
    /// The whole [`Hand`] and not just what is in it, because a bag has to draw
    /// a drop it is *waiting* on: the item sits where it was let go until the
    /// shard's answer lands, and which bag that is, is the pending drop's.
    pub hand: Option<Hand>,
    /// The keys are coming to this window.
    ///
    /// The manager's answer and not the pane's, for
    /// [`PaneCtx::under_pointer`]'s reason one resource over: a keyboard is
    /// exclusive across every window on the screen, and a pane can no more know
    /// what another pane has taken than it can know what is drawn over it. A
    /// pane pairs it with whichever of *its own* controls the keys would land
    /// in — see [`dialog::DialogPane`], the only kind that has one — so that a
    /// field left focused in a window the player clicked away from draws no
    /// caret.
    pub has_keyboard: bool,
    /// The client's own modal is up, and its answer is coming to this window.
    ///
    /// [`PaneFrame::has_keyboard`]'s shape for the prompt a Shift-drag opens:
    /// the manager owns which press the modal suspended
    /// ([`Asking`](crate::windows::Asking)) and the pane owns the press itself.
    /// A pane reads this to leave that press alone — a move must not lift it
    /// out from under the number the player is choosing, and the release that
    /// dismissed the prompt's own window must not put it down.
    pub has_prompt: bool,
}

/// Everything a pane may read while it answers one input, and nothing it may
/// write.
///
/// [`PaneFrame`] and the four things that are true of an *event* rather than of
/// a frame.
pub struct PaneCtx<'a> {
    /// What this pane would be packed and laid out with, were the frame being
    /// drawn now.
    pub frame: PaneFrame<'a>,
    /// How this window was laid out on the last frame, or `None` for a window
    /// that has not been drawn yet.
    ///
    /// **What is clicked is what was drawn.** A pane could lay itself out again
    /// from [`PaneCtx::frame`] and hit-test that, and it would be asking the
    /// atlas and the view a second question whose answer is free to differ from
    /// the picture the player is pointing at — the rule
    /// [`Windows::drawn_windows`](crate::windows::Windows::drawn_windows)
    /// exists for, and the one that used to make a paperdoll whose two answers
    /// disagreed a window that could not be closed.
    pub drawn: Option<&'a Drawn>,
    /// This window is the one the pointer is on: it covers the cursor and no
    /// window above it does.
    ///
    /// The manager's answer and not the pane's, because **z-order is the
    /// manager's** (decision 2) — a pane knows where the cursor is inside
    /// itself and cannot know what is drawn over it. It is the same question
    /// `App::window_under_pointer` has always asked first, handed to the pane
    /// instead of asked again.
    ///
    /// A pane checks it before taking a *located* input — a press or a notch —
    /// and ignores it for the two that are not located that way: a release
    /// finishes a press this pane started, wherever the pointer has got to
    /// since, and a move is offered to every window.
    pub under_pointer: bool,
    /// Shift and Ctrl, as of this event.
    ///
    /// One reader, which is the one the field was added for: a bag's Shift-drag
    /// divides the stack it is pulling out instead of lifting the whole pile.
    pub modifiers: Modifiers,
    /// Now, for every double-click pair.
    ///
    /// Passed rather than read from `Instant::now()` inside a pane, for the
    /// reason the tick's `Rng` is owned rather than ambient: a pair of clicks
    /// is a *timing* rule, and a rule that reads an ambient clock cannot be
    /// exercised by a test.
    pub now: Instant,
}

/// What a pane says about one input.
///
/// Decision 4, and the wheel defect stated as a type instead of as a
/// convention: `taken` and `redraw` are **two fields because they are two
/// questions**, and every one of the four combinations happens. A catalogue
/// scrolled to its last row took the notch and has nothing new to draw
/// ([`Response::consumed`]); a hover tint that changed took nothing and is
/// stale ([`Response::stale`]). The `bool` these replace could say neither, and
/// the `||` chain in `event_loop.rs` read it as whichever of the two the last
/// author had in mind — which is how a wheel over a shop window became a map
/// zoom.
#[derive(Debug, Default)]
#[must_use]
pub struct Response {
    /// The event stops here: neither the camera, nor the world, nor a window
    /// under this one ever sees it.
    pub taken: bool,
    /// The frame is stale and has to be drawn again.
    pub redraw: bool,
    /// What the pane is asking the manager to do, in order.
    pub out: Vec<Effect>,
}

impl Response {
    /// Not mine, and nothing changed. The pointer is somewhere else, or this is
    /// an input this kind of window has no use for.
    pub const fn ignored() -> Self {
        Self {
            taken: false,
            redraw: false,
            out: Vec::new(),
        }
    }

    /// Mine, and the frame still stands. The end-of-list wheel notch.
    pub const fn consumed() -> Self {
        Self {
            taken: true,
            redraw: false,
            out: Vec::new(),
        }
    }

    /// Mine, and it changed what is on the screen. The ordinary click.
    pub const fn changed() -> Self {
        Self {
            taken: true,
            redraw: true,
            out: Vec::new(),
        }
    }

    /// Not mine, but it changed what is on the screen. A hover tint.
    pub const fn stale() -> Self {
        Self {
            taken: false,
            redraw: true,
            out: Vec::new(),
        }
    }

    /// Ask for one thing, on the way out.
    pub fn with(mut self, effect: Effect) -> Self {
        self.out.push(effect);
        self
    }

    /// Fold another answer into this one: either says stale, either says taken,
    /// and the effects keep their order.
    ///
    /// For the two places one input has more than one answer — the manager's
    /// own gestures beside a pane's, and the fallback rung beside both.
    pub fn absorb(&mut self, other: Self) {
        self.taken |= other.taken;
        self.redraw |= other.redraw;
        self.out.extend(other.out);
    }
}

/// One thing a pane asks the manager to do.
///
/// Decision 5. Most of what a window does is ask the shard for something, and
/// [`Outgoing`] is already the complete vocabulary of that — so a pane returns
/// [`Effect::Net`] and never touches the link. A second enum meaning the same
/// things would be a second place to add a packet to, and translating between
/// them would be a `match` whose arms are all identities.
///
/// The rest is what only the manager can do, because it is about the window
/// rather than about what the window is over.
///
/// Every arm is performed by `panes::route`'s `App::perform`.
#[derive(Debug)]
pub enum Effect {
    /// This window to the top of the pile.
    Raise,
    /// This window off the list. What that means per kind — an overlay entry, a
    /// `0xB1` for a dialog, nothing at all on the wire for a bag — stays with
    /// the manager: see `App::close_window`.
    Close,
    /// Start moving this window, grabbed this far into it.
    Grab(GumpPixel),
    /// The shard's half, for everything that is not the hand.
    ///
    /// A `0x07` and a `0x08` can travel through here too, and one pair does:
    /// the wield a double-click on a weapon is, and the sweep "Take all" is.
    /// Neither of those ever puts anything on the *cursor* — the player sees
    /// no icon under the pointer at any point — so neither is
    /// [`Effect::Lift`]. That is the whole difference between the two.
    Net(Outgoing),
    /// This goes onto the cursor: the `0x07` **and** the hand it fills.
    ///
    /// One effect and not a `Net` beside a hand-write, for [`Effect::Answer`]'s
    /// reason: they are one act. A pane that could send the lift without
    /// filling the hand would leave the shard holding an item this client draws
    /// in a bag, and one that could fill the hand without sending would draw an
    /// item on a cursor the shard has never heard of.
    ///
    /// Decision 7 is what makes this *not* half of a pair: nothing about it is
    /// bound to the drop that eventually follows, which may come from another
    /// pane, after a walk, with the source window closed in between.
    Lift(ItemDrag),
    /// And this puts it down: the `0x08`, `0x13` or `0x0F` the destination
    /// names, and the local projection that draws it there until the shard
    /// answers.
    ///
    /// The destination only — *what* is in the hand is the manager's, so a pane
    /// says where and never what. A drop asked for while the last one is still
    /// in flight is refused there too, which is why the pane does not have to
    /// know the difference between a hand that is holding and one that is
    /// waiting.
    Drop(PendingDrop),
    /// Put the client's own amount picker up over this window's press.
    ///
    /// The manager both raises it and remembers whose it is
    /// ([`Windows::prompt`](crate::windows::Windows::prompt)), which is what
    /// [`Input::Answered`] is later routed by.
    Prompt(SplitPrompt),
    /// Compact the like piles in this container, one ordinary lift-and-drop per
    /// snapshot until nothing more merges.
    ///
    /// A manager machine rather than a burst of [`Effect::Net`], and the
    /// difference is time: the pass sends one merge, asks for the container
    /// again, and plans the next against the *answer* — so it lives across
    /// frames, where a pane only ever sees one input. See `App::stack_pass`.
    StackAll,
    /// This dialog is answered: the `0xB1` goes out **and the window goes off
    /// the list**.
    ///
    /// The one effect that is not decision 5's `Net` beside a `Close`, and the
    /// reason is the first of the three things a `0xB0` does not say: nothing
    /// ever arrives to tell this client the dialog is done. The shard sends one
    /// `0xB0`, waits for one `0xB1` and assumes the window went down with it, so
    /// the answer *is* the close — one act, and taking it apart into two effects
    /// would let the close arm ask the same window for its dismissal answer and
    /// send button zero after the button the player actually pressed.
    Answer(GumpReply),
    /// The keys come to this window until something else asks for them.
    ///
    /// The keyboard is one resource with one owner, for the reason the hand is
    /// (decision 7): there is one of it. Which window holds it is
    /// [`Windows::keyboard`](crate::windows::Windows::keyboard), and *what*
    /// inside that window the keys land in stays with the pane — the same split
    /// as z-order and a pane's own hit test.
    TakeKeyboard,
    /// The keys go back to the world.
    ///
    /// Unconditional and not "if they were mine": this is what a press that
    /// landed on no field means, and it means it about whichever window was
    /// typing. `Dialogs::focus = None` said exactly that, from a field one
    /// client-wide struct kept.
    ReleaseKeyboard,
    /// Make one of the two windows the shard does not know about exist.
    Open(LocalWindow),
    /// Restore a view-owned window the player previously closed. Unlike
    /// [`Open`](Self::Open), its contents are authoritative view state, so the
    /// manager first removes the local close overlay.
    Reopen(WindowSubject),
    /// The client's own modal has been answered, and this is the answer.
    ///
    /// [`Effect::Prompt`]'s other end, and asked for by the *prompt's* window
    /// rather than by the presser: the picker owns the number and knows nothing
    /// about the press it is standing over, so what it says is only what was
    /// chosen. The manager takes the window down and hands the answer to
    /// whichever presser [`Windows::prompt`](crate::windows::Windows::prompt)
    /// names — see `App::answer_prompt`.
    ///
    /// One effect and not a `Net` beside a `Close`, for [`Effect::Answer`]'s
    /// reason one window kind over: answering *is* closing here, and a picker
    /// that could do one without the other would either stand over a press that
    /// has already been divided or leave a press waiting for a number that has
    /// already been sent.
    Answered(Answer),
}

/// The client's own amount picker, as a pane asks for it.
///
/// Its own type rather than a bare number so that the effect says what kind of
/// prompt it is: a second modal — a confirmation, a name to type — would be a
/// second arm of [`Effect::Prompt`] and would be routed back by the same
/// [`Asking`](crate::windows::Asking), which is the whole point of settling
/// who a modal's answer is addressed to once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SplitPrompt {
    /// Which pile is being divided.
    ///
    /// Not read to look anything up — the presser is holding the item, and the
    /// answer is measured against *its* copy when it arrives. It is the
    /// prompt's identity: the window is filed under the item the way the
    /// reference client files its `SplitMenuGump`, so a second prompt cannot
    /// open over the same pile.
    pub item: openshard_protocol::serial::Serial,
    /// The most that may be taken: the pile less the one that stays behind.
    /// A split that took the whole stack would be a lift with extra steps.
    pub most: u16,
}

/// A window that is open because the player asked for it, not because the shard
/// opened it.
///
/// The two kinds whose *existence* is local — a `0x3A` refreshes the skill
/// numbers and a `0x11` the status ones, and neither asks for a window. A
/// container or a paperdoll cannot be here: asking for one of those is
/// [`Effect::Net`], and the window appears when the view grows the entry that
/// `reconcile_own_windows` turns into one (decision 8).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LocalWindow {
    Skills,
    Status,
    #[allow(dead_code)] // Opening affordance arrives with the minimap command/hotkey.
    Minimap,
    /// A zoomable, pannable view of the whole facet.
    WorldMap,
}

impl LocalWindow {
    /// The subject this names in the manager's list.
    ///
    /// Here so that both kinds go through the one door
    /// [`windows::open_local_window`](crate::windows::open_local_window) rather
    /// than through an arm each — the same reason the subject is what a window
    /// *is* now, and not a `bool` on the side. A third local kind adds a line
    /// here and nothing anywhere else.
    pub const fn subject(self) -> WindowSubject {
        match self {
            Self::Skills => WindowSubject::Skills,
            Self::Status => WindowSubject::Status,
            Self::Minimap => WindowSubject::Minimap,
            Self::WorldMap => WindowSubject::WorldMap,
        }
    }
}

/// One line of text on a laid-out window, resolved to a string.
///
/// Owned rather than borrowed for the reason every other window kind's line is
/// (`vendor::Line`, `status::Line`, `skills::Line`): a [`Drawn`] outlives the
/// frame that built it — it is what the *next* frame's pointer is tested
/// against — so it cannot hold a reference into the view.
///
/// Shared by the two kinds whose text is resolved on this side of the crate
/// boundary — a dialog's captions and fields, a paperdoll's name plate and
/// hover label — where the other kinds' lines are resolved in `client/render`,
/// beside the layouts that produce them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    /// Its top-left corner, in absolute gump pixels.
    pub at: GumpPixel,
    /// The face it is written in.
    pub font: Font,
    /// The hue the layout asked for.
    pub hue: Hue,
    /// The box it is cropped to, or `None` for a line that overflows rather
    /// than clipping.
    pub clip: Option<(i32, i32)>,
    /// The line itself.
    pub text: String,
}

impl Line {
    /// Draw it.
    pub fn label(&self) -> GumpLabel<'_> {
        GumpLabel {
            at: self.at,
            text: &self.text,
            font: self.font,
            hue: self.hue,
            clip: self.clip,
        }
    }
}

/// One of the client's own windows, as a component.
///
/// Implemented once per window kind, and reached through [`AnyPane`] rather
/// than through `dyn` — see decision 1 for why the vtable was refused.
///
/// The three methods are three phases of a frame, and decision 6 is that they
/// are called in this order for *every* pane before the next one starts:
/// [`art`](Pane::art) for all, pack once, [`layout`](Pane::layout) for all.
/// That order is why the last two take `&self` and a *shared* atlas — a pane
/// cannot be laid out against an atlas that is still growing.
pub trait Pane {
    /// The art this pane needs packed before it can be laid out.
    ///
    /// Asked every frame and not once at open: what a window needs depends on
    /// what is in it, and the atlas answers a repeat with nothing — see
    /// [`GumpAtlas::add`](openshard_client_render::gump::GumpAtlas::add), which
    /// filters what it already holds.
    fn art(&self, frame: &PaneFrame<'_>) -> Vec<GumpArt>;

    /// Lay this pane out for this frame: the pictures the pass draws and the
    /// pointer is tested against.
    ///
    /// `None` is a window with nothing to draw — a catalogue whose subject has
    /// gone out of the view between the packet and the frame — and not an
    /// error: it simply contributes no entry to
    /// [`Windows::drawn_windows`](crate::windows::Windows::drawn_windows), so
    /// nothing of it is drawn and nothing of it is clickable.
    fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn>;

    /// Offer one input. The answer says whether the pane took it, whether the
    /// frame is stale because of it, and what it is asking the manager to do.
    fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response;
}

/// Every window kind, as one type.
///
/// Decision 1: an `enum` and not `Box<dyn Pane>`, because this is what makes a
/// seventh window kind a *compile error* everywhere the manager still has to
/// know which kind it has — the same reason
/// [`WindowSubject`](crate::windows::WindowSubject) and
/// [`Drawn`](crate::windows::Drawn) are enums — and because a vtable and an
/// allocation in a list this client walks several times a frame buys nothing
/// for a set of kinds that is known at compile time.
///
/// It lives in [`OwnWindow`](crate::windows::OwnWindow), beside the position:
/// **a pane's state is tied to its window's lifetime by construction**, which
/// is the other half of the defect this plan was written for. A shop's scroll
/// position used to be an entry in a map on `Windows` that `close_window` had
/// to remember to remove by hand.
#[derive(Debug)]
pub enum AnyPane {
    Container(container::ContainerPane),
    Vendor(vendor::VendorPane),
    Paperdoll(paperdoll::PaperdollPane),
    Dialog(dialog::DialogPane),
    Skills(skills::SkillsPane),
    Status(status::StatusPane),
    /// The client's own amount picker — the window a Shift-drag asks a question
    /// through. See [`split::SplitPane`].
    Split(split::SplitPane),
    /// The client's own yes/no window — the first kind that is a *question*
    /// rather than a view of something. See [`confirm::ConfirmPane`].
    Confirm(confirm::ConfirmPane),
    /// The party manifest. See [`party::PartyPane`].
    Party(party::PartyPane),
    /// The generated-terrain minimap. Its pixels are recorded by the radar
    /// content pass; the pane supplies lifecycle and hit bounds.
    Minimap(minimap::MinimapPane),
    WorldMap(world_map::WorldMapPane),
    Spellbook(spellbook::SpellbookPane),
}

impl AnyPane {
    /// The pane a window of this subject needs, built when the window opens.
    ///
    /// A kind whose subject carries a key is handed it here and keeps it: a
    /// shop names its vendor in the `0x3B` it sends, and the pane that builds
    /// that packet has to know which mobile it is trading with. That is not the
    /// pane knowing it is a window — the position, the z-order and the list are
    /// still the manager's — it is the pane knowing what it is a pane *of*.
    pub fn of(subject: WindowSubject) -> Self {
        match subject {
            WindowSubject::Container(serial) => Self::Container(container::ContainerPane::new(serial)),
            WindowSubject::Vendor(serial) => Self::Vendor(vendor::VendorPane::new(serial)),
            WindowSubject::Paperdoll(serial) => Self::Paperdoll(paperdoll::PaperdollPane::new(serial)),
            WindowSubject::Dialog(gump_id) => Self::Dialog(dialog::DialogPane::new(gump_id)),
            WindowSubject::Skills => Self::Skills(skills::SkillsPane::default()),
            WindowSubject::Status => Self::Status(status::StatusPane),
            WindowSubject::Confirm(question) => Self::Confirm(confirm::ConfirmPane::new(question)),
            WindowSubject::Party => Self::Party(party::PartyPane::default()),
            WindowSubject::Minimap => Self::Minimap(minimap::MinimapPane::default()),
            WindowSubject::WorldMap => Self::WorldMap(world_map::WorldMapPane::default()),
            WindowSubject::Spellbook(serial) => Self::Spellbook(spellbook::SpellbookPane::new(serial)),
            // The one subject that carries what its pane is built with rather
            // than what the pane looks up. Everything else here names something
            // in the view — a bag, a body, a gump — and the pane reads it every
            // frame; the bound on a split is measured from a pile *at the
            // moment of the press* and must not be re-read afterwards, so it
            // travels with the identity. See `SplitPane::most`.
            WindowSubject::Split { most, .. } => Self::Split(split::SplitPane::new(most)),
        }
    }
}

impl Pane for AnyPane {
    /// The delegating `match` decision 1 pays for the enum with: one line per
    /// kind per trait method, once.
    fn art(&self, frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        match self {
            Self::Container(pane) => pane.art(frame),
            Self::Vendor(pane) => pane.art(frame),
            Self::Paperdoll(pane) => pane.art(frame),
            Self::Dialog(pane) => pane.art(frame),
            Self::Skills(pane) => pane.art(frame),
            Self::Status(pane) => pane.art(frame),
            Self::Split(pane) => pane.art(frame),
            Self::Confirm(pane) => pane.art(frame),
            Self::Party(pane) => pane.art(frame),
            Self::Minimap(pane) => pane.art(frame),
            Self::WorldMap(pane) => pane.art(frame),
            Self::Spellbook(pane) => pane.art(frame),
        }
    }

    fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        match self {
            Self::Container(pane) => pane.layout(frame),
            Self::Vendor(pane) => pane.layout(frame),
            Self::Paperdoll(pane) => pane.layout(frame),
            Self::Dialog(pane) => pane.layout(frame),
            Self::Skills(pane) => pane.layout(frame),
            Self::Status(pane) => pane.layout(frame),
            Self::Split(pane) => pane.layout(frame),
            Self::Confirm(pane) => pane.layout(frame),
            Self::Party(pane) => pane.layout(frame),
            Self::Minimap(pane) => pane.layout(frame),
            Self::WorldMap(pane) => pane.layout(frame),
            Self::Spellbook(pane) => pane.layout(frame),
        }
    }

    fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match self {
            Self::Container(pane) => pane.handle(input, ctx),
            Self::Vendor(pane) => pane.handle(input, ctx),
            Self::Paperdoll(pane) => pane.handle(input, ctx),
            Self::Dialog(pane) => pane.handle(input, ctx),
            Self::Skills(pane) => pane.handle(input, ctx),
            Self::Status(pane) => pane.handle(input, ctx),
            Self::Split(pane) => pane.handle(input, ctx),
            Self::Confirm(pane) => pane.handle(input, ctx),
            Self::Party(pane) => pane.handle(input, ctx),
            Self::Minimap(pane) => pane.handle(input, ctx),
            Self::WorldMap(pane) => pane.handle(input, ctx),
            Self::Spellbook(pane) => pane.handle(input, ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four corners of decision 4 exist as four values, which is the whole
    /// of the fix: a `bool` has two.
    #[test]
    fn taken_and_redraw_are_four_answers_and_not_two() {
        let corners = [
            Response::ignored(),
            Response::consumed(),
            Response::changed(),
            Response::stale(),
        ]
        .map(|response| (response.taken, response.redraw));
        assert_eq!(
            corners,
            [(false, false), (true, false), (true, true), (false, true)],
            "the wheel defect was `consumed` and `ignored` being one value"
        );
    }

    /// Folding is what lets one input have more than one answer without either
    /// of the two questions being lost — a pane's hover beside the manager's
    /// own drag, say.
    #[test]
    fn absorbing_keeps_either_yes() {
        let mut response = Response::consumed();
        response.absorb(Response::stale());
        assert!(response.taken, "the pane took it");
        assert!(response.redraw, "and the other answer says the frame is stale");

        let mut response = Response::ignored();
        response.absorb(Response::ignored());
        assert!(!response.taken);
        assert!(!response.redraw);
    }

    /// Effects come out in the order they were asked for: a pane that raises
    /// itself and then sends a packet means that order.
    #[test]
    fn effects_keep_their_order() {
        let mut response = Response::changed().with(Effect::Raise);
        response.absorb(Response::consumed().with(Effect::Close));
        assert!(matches!(response.out.as_slice(), [Effect::Raise, Effect::Close]));
    }

    /// A subject gets the pane its kind names, which is what makes a seventh
    /// window kind a compile error here rather than a silent nothing.
    #[test]
    fn every_subject_has_a_pane() {
        let serial = openshard_protocol::serial::Serial::new(0x0000_002A).unwrap();
        assert!(matches!(
            AnyPane::of(WindowSubject::Container(serial)),
            AnyPane::Container(_)
        ));
        assert!(matches!(
            AnyPane::of(WindowSubject::Vendor(serial)),
            AnyPane::Vendor(_)
        ));
        assert!(matches!(
            AnyPane::of(WindowSubject::Paperdoll(serial)),
            AnyPane::Paperdoll(_)
        ));
        assert!(matches!(
            AnyPane::of(WindowSubject::Dialog(openshard_protocol::gump::GumpId(7))),
            AnyPane::Dialog(_)
        ));
        assert!(matches!(AnyPane::of(WindowSubject::Skills), AnyPane::Skills(_)));
        assert!(matches!(AnyPane::of(WindowSubject::Status), AnyPane::Status(_)));
        assert!(matches!(AnyPane::of(WindowSubject::Minimap), AnyPane::Minimap(_)));
        assert!(matches!(
            AnyPane::of(WindowSubject::WorldMap),
            AnyPane::WorldMap(_)
        ));
    }
}
