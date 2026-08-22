//! The player's own windows — containers, paperdolls, the skill window and
//! `0xB0` dialogs — and what the mouse is doing to them: [`Windows`].
//!
//! Every field here is about *this client's own* window layer rather than
//! about the world it draws over: what is open, where each sits, what the
//! last frame laid out for it, and which of the screen's one-of-a-kind
//! devices each window holds. Pulled out of [`crate::App`] for the same reason
//! [`crate::picking::Picking`] and [`crate::input::Input`] were, and unlike
//! those two the fields here *are* read together — `grip` and `hand` are
//! checked side by side on every press, and `own_windows` and `drawn_windows`
//! are asked in the same breath to decide which window a click landed on.
//!
//! `docs/window_components.md` is finished with this module: every step of it
//! took something from here into the window it belonged to, and what is left
//! is what is true of the *layer* rather than of one window — which windows
//! exist, in what order, where each sits, and who holds the three things there
//! is one of: the pointer ([`Windows::grip`]), the keyboard
//! ([`Windows::keyboard`]) and the cursor ([`Windows::hand`], and
//! [`Windows::prompt`] for the press a modal is standing over).
//!
//! [`Windows::world_press`] is the field that is not about a window at all —
//! an item lying on the ground is pressed exactly the way an icon in a bag is
//! — but the *type* it holds moved to [`crate::hand`], because that is where
//! the rest of a press's story lives regardless of which pane, if any, it
//! started on. The field stays here deliberately, beside its two siblings
//! `hand` and `grip`: all three are exclusive devices the manager tracks,
//! and that registry does not change shape just because one of the three
//! types it names is not itself window-shaped.

use std::collections::HashSet;
use std::time::Instant;

use openshard_client_render::gump::{self as gump_art, GumpPixel};
use openshard_client_render::skills;
use openshard_client_render::vendor;
use openshard_protocol::gump::GumpId;
use openshard_protocol::serial::Serial;

use crate::hand::{Hand, ItemPress};

/// Where the first container window opens, and how far each one after it is
/// offset.
///
/// A cascade rather than a pile: the shard sends no position, and two windows
/// at one coordinate look like one window with the wrong contents. The
/// reference client remembers a per-container position across sessions; this
/// does not yet, and the note is in `docs/client.md`.
const CONTAINER_CASCADE: GumpPixel = GumpPixel::new(24, 24);

/// The corner the cascade starts from.
const CONTAINER_ORIGIN: GumpPixel = GumpPixel::new(120, 80);

/// How many windows the cascade steps before it starts over, so that a player
/// who opens a dozen bags does not push the last of them off the screen.
const CONTAINER_CASCADE_LENGTH: i32 = 8;

/// One of this client's own windows: what it is over, where it is, and the
/// state that belongs to it alone.
///
/// Neither packet carries a position: a `0x24` names a container and a gump,
/// a `0x88` names a mobile, and where the window goes is entirely the
/// client's — once the player has dragged one it is the player's. What the
/// window is *over* is looked up in the
/// [`WorldView`](openshard_client_net::view::WorldView) by serial every frame,
/// so a window can never hold a stale copy of what is in the bag or on the
/// body.
///
/// [`pane`](OwnWindow::pane) is the third thing, and it is here rather than in
/// a map on [`Windows`] **so that a window's private state cannot outlive the
/// window**. A shop's scroll position used to be an entry in
/// `Windows::vendor_scrolls` that [`crate::App::close_window`] had to remember
/// to remove by hand; anything that lives here is dropped by the same `retain`
/// that takes the window off the list. See `docs/window_components.md`.
#[derive(Debug)]
pub struct OwnWindow {
    /// What it is a window over.
    pub subject: WindowSubject,
    /// Its top-left corner on the surface.
    pub at: GumpPixel,
    /// The window's own state and its own input handling — see
    /// [`crate::panes`].
    pub pane: crate::panes::AnyPane,
}

impl OwnWindow {
    /// The pointer, in this window's own pixels: the surface's cursor, less
    /// where this window sits, divided by how big it is being drawn.
    ///
    /// **The inverse of `gump::place`, and the only one.** A pane lays itself
    /// out at the origin and at the art's own size
    /// (`docs/window_components.md`'s window-local coordinates), so everything
    /// the manager decides about a window — its placement and its scale — has
    /// to be undone here before a pane is handed a cursor, exactly as it is
    /// applied there before the pane's quads reach the surface. Three callers
    /// ask this — the layout pass, the input router and the hit test — and they
    /// ask *this*, rather than each subtracting `at` for itself, because three
    /// copies of one transform is three chances for the picture and the click
    /// to disagree.
    ///
    /// `floor` and not a truncating cast: a cursor left of or above the window
    /// is a negative local coordinate, and truncation rounds those *up* toward
    /// zero — which would put the column left of a window's edge on column `0`,
    /// inside the picture that starts there, and hand a pane a click that
    /// landed outside it. It is also the rounding `gump::place` draws with,
    /// which is what makes the two agree pixel for pixel at a fractional
    /// scale: a quad covers `x` when `x >= corner`, so the pixel a fractional
    /// edge falls inside belongs to the picture that started before it.
    pub fn local_cursor(&self, cursor: GumpPixel, scale: crate::desk::WindowScale) -> GumpPixel {
        let factor = scale.factor();
        GumpPixel::new(
            ((cursor.x - self.at.x) as f32 / factor).floor() as i32,
            ((cursor.y - self.at.y) as f32 / factor).floor() as i32,
        )
    }
}

/// What the pointer is doing to a window's frame: the whole of a window drag,
/// as the two states it has and the three transitions between them.
///
/// **Down, remember, follow, up.** The button going down on a frame freezes
/// two positions ([`WindowHold`]) and nothing else; every pointer move while
/// it is down puts the window back at *its own* frozen corner plus how far the
/// pointer has travelled since; the button coming up forgets both. While a
/// window is held, [`App::drag_own_window`](crate::App::drag_own_window) is
/// the only writer of [`OwnWindow::at`].
///
/// # Why a delta, and not "where inside it the player grabbed it"
///
/// This was `Option<(WindowSubject, GumpPixel)>` — the window, and the offset
/// from its corner to the cursor — and the mover placed the window at
/// `pointer - offset`. That is the same arithmetic as the delta *only* if the
/// offset is measured in the same pixels as the pointer, and the two places
/// that started a drag disagreed about which those were:
///
/// * `App::press_on_own_window`, the fallback rung, subtracted the window's
///   corner from the absolute pointer — surface gump pixels, the same space
///   `at` is in.
/// * Every pane answered a press it had no use for with `Effect::Grab`
///   carrying its own [`PaneFrame::cursor`](crate::panes::PaneFrame::cursor),
///   which is **window-local**: `at` subtracted *and* divided by
///   [`WindowScale`](crate::desk::WindowScale) — see [`OwnWindow::local_cursor`].
///
/// So at any window scale above the art's own size, a press on a window whose
/// pane answers it — a paperdoll, a bag, a shop, a dialog — jumped the window
/// by `cursor * (factor - 1)` on the first pointer movement after the click,
/// by an amount that depended on where in the frame it had been clicked and
/// with no movement of the mouse to account for it. That is the phantom move:
/// a stale-looking teleport with nothing stale in it, only two frames of
/// reference sharing one field.
///
/// A delta cannot be measured in the wrong space. Both positions in a
/// [`WindowHold`] are absolute — one read from the same pointer that will be
/// read again on the move, one read from the window's own `at` — so the scale
/// never enters the arithmetic and cannot be applied once or twice by mistake.
/// It is also what makes the scale knob safe to turn mid-drag, and what makes
/// the machine hold if a window is re-laid-out under the pointer while it is
/// being carried.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WindowGrip {
    /// No button is down on any window's frame: nothing follows the pointer.
    #[default]
    Idle,
    /// The button went down on a frame and has not come up.
    Held(WindowHold),
}

impl WindowGrip {
    /// The button went down on `subject`'s frame, with the pointer at
    /// `pointer` and that window's corner at `window` — both in absolute
    /// surface gump pixels, and both frozen here until the button comes up.
    ///
    /// A press while something is already held overwrites it rather than being
    /// refused. Two presses with no release between them means the release was
    /// lost — the window closed under the pointer, the client lost focus
    /// mid-drag — and the press happening *now* is the truthful answer to which
    /// frame the pointer has hold of. Refusing it would carry the old window
    /// around under a press meant for another one, which is the shape of the
    /// defect this type was written to end.
    pub fn press(&mut self, subject: WindowSubject, pointer: GumpPixel, window: GumpPixel) {
        *self = Self::Held(WindowHold {
            subject,
            pointer,
            window,
        });
    }

    /// The button came up, or the drag was called off some other way: a modal
    /// standing over the window, the window closing, the world going away.
    ///
    /// Idempotent, and every caller may ask without knowing whether anything
    /// was held — "nothing is being dragged now" is the whole of what it
    /// promises.
    pub fn release(&mut self) {
        *self = Self::Idle;
    }

    /// Which window is being carried and where it belongs with the pointer at
    /// `pointer`, or `None` when nothing is held.
    ///
    /// The delta this type exists for, and the one place it is computed.
    pub fn follow(&self, pointer: GumpPixel) -> Option<(WindowSubject, GumpPixel)> {
        let Self::Held(hold) = self else {
            return None;
        };
        let travelled = GumpPixel::new(pointer.x - hold.pointer.x, pointer.y - hold.pointer.y);
        Some((hold.subject, hold.window.offset(travelled)))
    }
}

/// The two positions one press freezes: where the pointer went down, and where
/// the window it went down on was standing at that moment.
///
/// The window is named by subject rather than by index or by a borrow, because
/// the press that starts a drag also *raises* the window, and raising reorders
/// [`Windows::own_windows`] — an index taken at the press names a different
/// window by the time the pointer moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WindowHold {
    /// The window the button went down on.
    pub subject: WindowSubject,
    /// Where the pointer was at that moment, in absolute surface gump pixels.
    pub pointer: GumpPixel,
    /// Where that window's top-left corner was at that same moment, in the
    /// same pixels — [`OwnWindow::at`] as it stood before the drag.
    pub window: GumpPixel,
}

/// What a window is over: a bag's contents, a body, or a dialog the shard
/// drew.
///
/// One list holds all three, because dragging, raising, hit-testing and
/// closing are the same gesture over any of them — decision 5 in
/// `docs/client.md`, and the reason the container's window machinery was
/// written in this client's own gump pixels rather than as an egui window.
/// They differ in exactly two places, and each is a `match` three arms long:
/// what is laid out for it (see [`Windows::drawn_windows`], which is also
/// what the pointer is tested against), and what closing one means.
///
/// The dialog is the newest of the three and the one that had to *leave*
/// somewhere to get here: a `0xB0` was an egui window with the shard's art
/// drawn underneath it, which is two windows' worth of frame and two
/// opinions about where every button is. See [`crate::panes::dialog`], which
/// owns everything about one that no packet carries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum WindowSubject {
    /// A container the shard has opened, by its serial.
    Container(Serial),
    /// A vendor catalogue, whose controls are client-side rather than ordinary
    /// container dragging. It exists for both buy (`0x74`) and sell (`0x9E`).
    Vendor(Serial),
    /// A mobile whose paperdoll the shard has opened, by its serial. The same
    /// serial may name a container *and* a paperdoll — a player is both —
    /// which is why this is the identity and not the serial alone.
    Paperdoll(Serial),
    /// A `0xB0` dialog, by the id the shard filed it under.
    Dialog(GumpId),
    /// This character's skills. No key at all: a `0x3A` carries no serial, so
    /// there is one skill window and it is always about the body at this end
    /// of the connection — see `view::Player::skills`.
    ///
    /// One of the two window kinds whose *existence* is not in the view. A
    /// container window is open because the shard opened it and a dialog
    /// because the shard drew it, so `sync_own_windows` can read both off the
    /// view; this one is open because the player pressed Skills, and **being
    /// in [`Windows::own_windows`] is the whole of that fact** — see
    /// [`open_local_window`], which is the only thing that puts it there.
    ///
    /// It used to be `Windows::skills` being `Some`, which was the tree and
    /// the openness in one field: closing the window and forgetting which
    /// headings were shut were one write, and four files did it. The tree is
    /// a field of `panes::skills::SkillsPane` now.
    Skills,
    /// This character's status window. No key, for the skill window's reason:
    /// a `0x11` is about the one player this connection is.
    ///
    /// The other of the two kinds whose *existence* is not in the view, and it
    /// was the last field in the client that said "this window is open"
    /// anywhere but in [`Windows::own_windows`]. `Windows::status` was that
    /// `bool`, written by five places and read by a sixth; being in the list is
    /// the whole of the fact now, the same as every other kind's.
    Status,
    /// The client's own amount picker, standing over a Shift-drag.
    ///
    /// The third kind whose existence is local, and the first one that is open
    /// because *this client* asked rather than because the player or the shard
    /// did: a press turned into a question, and this is the question on the
    /// screen. It used to be an `egui::Window` in the shell with a
    /// `DragValue` on it — a second window system drawn over the gump layer,
    /// with its own idea of where a window is and no way to be clicked by the
    /// same walk everything else is.
    ///
    /// **The only subject that carries something its pane cannot look up.** A
    /// bag, a body and a gump are all in the view, so their panes re-read them
    /// every frame and can never hold a stale copy; `most` is measured from the
    /// pile at the moment of the press and is deliberately *not* re-read, so
    /// that the bar cannot slide under the player's finger when the pile
    /// changes. `item` is the identity — one prompt per pile, the way the
    /// reference client files its `SplitMenuGump` — and nothing looks it up
    /// either: the press the answer belongs to is holding its own copy, and
    /// [`ItemPress::split`](crate::hand::ItemPress::split) is what measures the
    /// answer against the pile as it stands when it arrives.
    Split { item: Serial, most: u16 },
    /// A question this client is asking on its own behalf — see
    /// [`Question`](crate::panes::confirm::Question), which is both the key and
    /// the whole of what the window means.
    ///
    /// Keyed by the question rather than by nothing, so that two questions are
    /// two windows: one plate showing whichever was asked last would answer the
    /// wrong packet for the other. Its *existence* is the view's, not this
    /// client's — a party invitation stands because a `0x78` said so — which is
    /// why it is reconciled like a dialog rather than opened like a skill sheet,
    /// and why it is not [`WindowSubject::is_local`].
    Confirm(crate::panes::confirm::Question),
    /// The party manifest. No key, for the skill sheet's reason: a client is in
    /// one party at a time, and the roster is about the body at this end of the
    /// connection.
    ///
    /// Its existence is the view's, like a question's and unlike a sheet's: the
    /// window appears when a roster arrives and goes when the last member
    /// leaves, so it is reconciled rather than opened by a button. That is also
    /// the whole of what it inherits from the `egui::Window` it replaced, which
    /// was drawn from `!members.is_empty()` in exactly the same way.
    Party,
    /// Generated terrain around the player. Its existence is local, like the
    /// skill sheet; terrain products themselves remain in `App::radar_cache`.
    Minimap,
    /// The facet-wide map.  Its terrain is the same generated radar product
    /// as the minimap, only shown through a rectangular, pannable viewport.
    WorldMap,
    /// A spellbook the shard has opened, keyed by its item serial.  Its
    /// membership is a `0xBF 0x1B` record, separate from the pack containing
    /// the book.
    Spellbook(Serial),
}

impl WindowSubject {
    /// Whether this window exists because the player asked for it rather than
    /// because the shard opened it.
    ///
    /// The three kinds [`reconcile_own_windows`] cannot answer for — the two
    /// [`open_local_window`] puts in the list, and the amount picker
    /// [`open_split_window`] does: there is no container, no mobile and no gump
    /// in the view to hold any of them open, so the view going away does not
    /// take them with it. (The picker is not the *player's* window the way the
    /// other two are — this client put it up to ask a question — but the fact
    /// this predicate states is about the view and not about who asked.)
    /// Everything that has to drop
    /// *every* window when the world ends — the disconnect — asks this instead
    /// of naming the kinds, which is a list that would otherwise have to be
    /// kept in step by hand.
    pub const fn is_local(self) -> bool {
        matches!(
            self,
            Self::Skills | Self::Status | Self::Minimap | Self::WorldMap | Self::Split { .. }
        )
    }
}

/// What the last frame drew for one window, and what it answers to.
///
/// Three shapes because the three window kinds answer different questions
/// about a click: a dialog's picture may be a button or a switch, a
/// paperdoll's may be one of the frame's own buttons, and a container's is an
/// item or the bag. What they have in common is the list of pictures, which
/// is what the pointer is tested against — see
/// [`crate::App::window_under_pointer`].
pub enum Drawn {
    /// A dialog: the pictures, hits and boxes, and the text resolved over them
    /// — see [`crate::panes::dialog::Window`], which is why this is not the
    /// render crate's `gump::Window` alone.
    Dialog(crate::panes::dialog::Window),
    /// A container: the background, every icon in it, and what the icons were
    /// — see [`crate::panes::container::Window`], which carries the list the
    /// pictures were built from so that a click maps to an item without a
    /// second walk that is free to disagree.
    Container(crate::panes::container::Window),
    Vendor(vendor::Window),
    /// A paperdoll: the frame, its furniture and the doll, and the text
    /// resolved over them — see [`crate::panes::paperdoll::Window`], which is
    /// why this is not the render crate's `paperdoll::Doll` alone.
    Paperdoll(crate::panes::paperdoll::Window),
    /// The skill window: the scroll, the rows inside its viewport, and the
    /// bar.
    Skills(skills::Sheet),
    /// The status frame and the numbers written over it.
    Status(openshard_client_render::status::Window),
    /// The amount picker: its frame, its knob, its button, and the number.
    Split(openshard_client_render::split::Window),
    /// A yes/no question: the plate, its two buttons, and the wording.
    Confirm(openshard_client_render::confirm::Window),
    /// The party manifest: the stretched plate, its ten name rows, and its
    /// controls.
    Party(openshard_client_render::party::Window),
    /// The radar content bounds; it has no gump pictures to pick.
    Minimap(crate::panes::minimap::Window),
    /// The rectangular facet-map bounds; it intentionally has no gump art.
    WorldMap(crate::panes::world_map::Window),
    /// The spell list on an opened book.
    Spellbook(openshard_client_render::spellbook::Window),
}

/// Whose press the client's own modal is standing over.
///
/// A prompt suspends exactly one [`ItemPress`](crate::hand::ItemPress), and
/// the answer has to find its way back to whoever is holding it — by
/// *identity*, never by "whichever window is on top", because the player can
/// raise a bag over the prompt while it is up. That is decision 9 for a third
/// exclusive device: the manager says where the answer goes, the holder says
/// what it means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Asking {
    /// The press on a world item, which [`Windows::world_press`] holds.
    World,
    /// A press inside one of this client's own windows, which that window's
    /// pane holds. The answer reaches it as
    /// [`Input::Answered`](crate::panes::Input::Answered), routed the way a
    /// keystroke is.
    Window(WindowSubject),
}

/// A client-side multi-pass compaction of one open container.
#[derive(Clone, Debug)]
pub struct StackPass {
    pub container: Serial,
    /// Source serial of the last merge, until a full container refresh removes it.
    pub awaiting: Option<(Serial, Instant)>,
}

impl Drawn {
    /// What was drawn, in painter's order — the one question every window
    /// kind answers the same way.
    pub fn pictures(&self) -> &[gump_art::Picture] {
        match self {
            Self::Dialog(window) => &window.art.pictures,
            Self::Container(window) => &window.pictures,
            Self::Vendor(window) => &window.pictures,
            Self::Paperdoll(window) => &window.doll.pictures,
            Self::Skills(sheet) => &sheet.pictures,
            Self::Status(status) => &status.pictures,
            Self::Split(split) => &split.pictures,
            Self::Confirm(question) => &question.pictures,
            Self::Party(manifest) => &manifest.pictures,
            Self::Minimap(minimap) => std::slice::from_ref(&minimap.frame),
            Self::WorldMap(_) => &[],
            Self::Spellbook(book) => &book.pictures,
        }
    }
}

/// The player's own window layer, and what the mouse is doing to it — see the
/// module docs.
pub struct Windows {
    /// The windows this client has open of its own — containers and
    /// paperdolls alike — bottom to top.
    ///
    /// Painter's order *is* z-order here, the same as the pictures inside
    /// one: the pass has no depth, so the last window in the list is the one
    /// drawn over the others and the first one picking finds. One list and
    /// not two, because a bag dragged over a paperdoll has to stay over it.
    pub own_windows: Vec<OwnWindow>,
    /// A window this end has closed, ahead of the shard thread's own
    /// [`view::WorldView`](openshard_client_net::view::WorldView) agreeing.
    ///
    /// [`crate::link::Body::predicted`]'s counterpart for a window's
    /// openness rather than a body's tile: `close_window`/`answer_gump`
    /// insert here, and [`reconcile_own_windows`] treats a subject in this
    /// set as closed regardless of what the view still says, dropping the
    /// entry once the view itself agrees the subject is gone — the same
    /// reconciliation `Folded::corrected` runs for a mispredicted step, one
    /// layer down. See `docs/client_window_state.md`'s D2.
    ///
    /// # There is no packet and no command behind this
    ///
    /// This used to say the two callers "send the `link::Command::CloseWindow`"
    /// as well. There is no such variant: it was S0's patch in that plan and
    /// S2 retired it in favour of this overlay. Nothing tells the link thread's
    /// own `WorldView` that a window closed, and nothing needs to — that copy
    /// is cloned across exactly once, in the `Update::World` published at world
    /// entry, and every packet after it is folded into *this* thread's copy
    /// instead. The comment outlived the mechanism by four months and named a
    /// symbol a reader could not find.
    pub locally_closed: HashSet<WindowSubject>,
    /// Every open window as the last frame laid it out: its subject, and the
    /// pictures that were drawn for it in painter's order.
    ///
    /// **What is clicked is what was drawn**, which is why this is
    /// remembered rather than recomputed at the press. A paperdoll's layout
    /// is not a function of the window alone — it reads the view, the
    /// tiledata and the client's own `gumpart` to decide which picture a
    /// worn item is — and a second walk asking those questions again is a
    /// second answer waiting to disagree with the one on the screen. It is
    /// the same rule [`crate::items::place`] follows in the world, one layer
    /// up.
    ///
    /// A frame behind, therefore: a window that has just opened is not
    /// pickable until it has been drawn once, which is also the frame its
    /// art is packed on and so the frame it first has any pixels to be
    /// picked by.
    pub drawn_windows: Vec<(WindowSubject, Drawn)>,
    /// What the pointer is doing to a window's frame — see [`WindowGrip`],
    /// which is the whole of a window drag and the reason this is a machine
    /// with named states rather than a pair of numbers.
    pub grip: WindowGrip,
    /// What is on the cursor, or `None` for an empty hand.
    ///
    /// **One resource with one owner**, decision 7 — the mirror of the shard's
    /// own one-item slot. A pane reads it out of
    /// [`PaneFrame::hand`](crate::panes::PaneFrame::hand) to answer "was
    /// something dropped on me" and to draw a preview, and no pane fills or
    /// empties it: both halves of a transfer are asked for as
    /// [`Effect::Lift`](crate::panes::Effect::Lift) and
    /// [`Effect::Drop`](crate::panes::Effect::Drop), which this end performs.
    ///
    /// It used to be `item_drag`, an `ItemDragTransaction` whose first state
    /// was a press that had sent nothing. That state is a pane's now — see
    /// [`ItemPress`](crate::hand::ItemPress) — which is why the field is
    /// called what it is: what is left here is the hand.
    pub hand: Option<Hand>,
    /// The press on an item lying in the *world*, which no pane holds because
    /// the ground is not a window.
    ///
    /// The manager's copy of what a bag's pane and a doll's pane each keep for
    /// their own icons — one press, three possible holders, one rule for what
    /// it becomes ([`ItemPress::dragged`](crate::hand::ItemPress::dragged)).
    /// It is here rather than beside the picking state because the hand it
    /// turns into is here.
    pub world_press: Option<ItemPress>,
    /// Who the client's own amount prompt is addressed to, or `None` while no
    /// prompt is up.
    ///
    /// The keyboard's shape one modal over (see [`Windows::keyboard`]): the
    /// manager owns *which* press a modal's answer belongs to, because a player
    /// can raise another window while the prompt stands, and the holder of that
    /// press owns what the answer means. Without it the answer would go to
    /// whoever happened to be on top — the plan's Backlog entry about who a
    /// modal's answer is addressed to, settled the same way
    /// [`Input::Key`](crate::panes::Input::Key) was.
    ///
    /// It replaces `split_pending`, which was a `bool` on a client that could
    /// only ever have one presser.
    pub prompt: Option<Asking>,
    /// An automatic sequence of ordinary lift/drop requests, one per fresh snapshot.
    pub stack_pass: Option<StackPass>,
    /// Which window the keys are going to, or `None` for the world.
    ///
    /// **One resource with one owner**, the shape decision 7 gives the hand and
    /// decision 2 gives z-order: there is one keyboard, so no pane can be
    /// trusted with the question and no pane can see another's answer. What a
    /// window does with the keys once they arrive is its own —
    /// [`DialogPane`](crate::panes::dialog) is the only kind that has anywhere
    /// to put them, and which of *its* boxes is a field of the pane.
    ///
    /// It replaces `Dialogs::focus`, which was the window *and* the field in one
    /// tuple on a struct that held every dialog's state at once. Read through
    /// [`App::keyboard_window`](crate::app::App::keyboard_window) rather than
    /// directly, because a window can leave the list between the answer that
    /// took it down and the frame that tidies up.
    pub keyboard: Option<WindowSubject>,
}

/// Open one of the windows the shard does not know about, if it is not open
/// already.
///
/// The skill sheet and the status frame: nothing in the view asks for either,
/// so nothing in [`reconcile_own_windows`] can put them there — the player
/// pressed a button on their paperdoll, and this is that press arriving. See
/// [`crate::panes::LocalWindow`], which is the effect a pane asks for.
///
/// **Idempotent, and that is the contract**: pressing Skills a second time
/// while the sheet is up must leave the window it finds alone, scroll position,
/// shut headings and all. A window is its pane, so re-opening one would be
/// throwing that away — which is exactly what the old
/// `skills.get_or_insert_with(Tree::default)` was careful not to do, in two
/// places that each had to remember.
///
/// Cascaded like a bag, for want of anywhere better: the reference client
/// remembers where each window was left, which is the backlog entry every kind
/// here shares.
pub fn open_local_window(own_windows: &mut Vec<OwnWindow>, subject: WindowSubject) {
    if own_windows.iter().any(|window| window.subject == subject) {
        return;
    }
    let step = own_windows.len() as i32 % CONTAINER_CASCADE_LENGTH;
    own_windows.push(OwnWindow {
        subject,
        at: GumpPixel::new(
            CONTAINER_ORIGIN.x + CONTAINER_CASCADE.x * step,
            CONTAINER_ORIGIN.y + CONTAINER_CASCADE.y * step,
        ),
        pane: crate::panes::AnyPane::of(subject),
    });
}

/// How far up and to the left of the pointer the amount picker opens.
///
/// The reference client's own `Mouse.Position - (80, 40)`, which is very nearly
/// the middle of the 164×74 frame: the window arrives under the hand that asked
/// for it, so the bar and the button are both a small movement away rather than
/// wherever the last window happened to cascade to.
const SPLIT_OFFSET: GumpPixel = GumpPixel::new(80, 40);

/// Put the amount picker up over a Shift-drag, unless one is already up over
/// this pile.
///
/// The third door into [`Windows::own_windows`], beside
/// [`reconcile_own_windows`] for the windows the view asks for and
/// [`open_local_window`] for the two the player does. It is its own door
/// because this kind is placed rather than cascaded — a modal that opened in the
/// corner of the screen while the pointer was in the middle of it would be a
/// question asked somewhere else — and because the subject carries the bound its
/// pane is built with.
///
/// `at` is the pointer, in absolute gump pixels. The window is nudged back onto
/// the screen only in so far as it is never placed at a negative corner: a
/// prompt hanging off the right-hand edge is the reference's behaviour too, and
/// this client has no idea how wide the surface is down here.
///
/// `scale` is how big the picker will be *drawn* ([`crate::desk::WindowScale`]),
/// and it is here rather than only in the draw pass because [`SPLIT_OFFSET`] is
/// half the frame's own art: a constant in art pixels, subtracted from a cursor
/// in screen pixels. Left unmagnified, the window that is supposed to arrive
/// under the hand would arrive up and to the left of it by half its size at
/// twice the scale, and by a whole frame at three times.
pub fn open_split_window(
    own_windows: &mut Vec<OwnWindow>,
    prompt: crate::panes::SplitPrompt,
    at: GumpPixel,
    scale: crate::desk::WindowScale,
) {
    let subject = WindowSubject::Split {
        item: prompt.item,
        most: prompt.most,
    };
    // Filed under the pile, so a second question about the same one cannot be
    // asked — the reference's `GetGump<SplitMenuGump>(item)` guard, which is
    // what that keying is for.
    if own_windows.iter().any(|window| match window.subject {
        WindowSubject::Split { item, .. } => item == prompt.item,
        _ => false,
    }) {
        return;
    }
    let magnify = scale.factor();
    own_windows.push(OwnWindow {
        subject,
        at: GumpPixel::new(
            (at.x - (SPLIT_OFFSET.x as f32 * magnify).round() as i32).max(0),
            (at.y - (SPLIT_OFFSET.y as f32 * magnify).round() as i32).max(0),
        ),
        pane: crate::panes::AnyPane::of(subject),
    });
}

/// [`crate::App::sync_own_windows`]'s membership logic, pulled out to a free
/// function so it can be exercised without an `App` — which needs real
/// client asset files to construct at all, the same reason `dst.rs` mirrors
/// `App`'s walk loop rather than driving the real thing in a test.
///
/// Opens a window for everything `view` has that `own_windows` does not, and
/// drops every window whose subject `view` no longer has — except a subject in
/// `locally_closed`, which stays dropped and stays un-reopened regardless of
/// what `view` says, until `view` itself agrees the subject is gone. That is
/// the reconciliation: an overlay entry survives only until the view it is
/// ahead of catches up, the same moment `Folded::corrected` would clear a
/// mispredicted step in `link.rs`, one layer down. A subject the view never
/// lists in the first place — the two [`WindowSubject::is_local`] kinds — has
/// nothing to reconcile against and is not put in the overlay at all.
///
/// **Neither local kind is passed in any more, in either direction.** Both are
/// opened by [`open_local_window`] and closed by the `retain` in
/// `App::close_window`, so being in `own_windows` *is* the fact and there is no
/// second copy of it here to disagree with. This function took a `status_open`
/// argument until step 3 of `docs/window_components.md`, and a `skills_open`
/// beside it until step 2; what they were is a window's openness kept somewhere
/// other than the list of open windows.
pub fn reconcile_own_windows(
    view: &openshard_client_net::view::WorldView,
    own_windows: &mut Vec<OwnWindow>,
    locally_closed: &mut HashSet<WindowSubject>,
) {
    locally_closed.retain(|subject| match *subject {
        WindowSubject::Container(serial) => view.containers.contains_key(&serial),
        WindowSubject::Vendor(serial) => {
            view.vendor_buys.contains_key(&serial) || view.vendor_sells.contains_key(&serial)
        }
        WindowSubject::Paperdoll(serial) => view.paperdolls.contains_key(&serial),
        WindowSubject::Dialog(gump_id) => view.gumps.iter().any(|gump| gump.gump_id == gump_id),
        WindowSubject::Skills => false,
        WindowSubject::Status => false,
        WindowSubject::Minimap => false,
        WindowSubject::WorldMap => false,
        WindowSubject::Spellbook(serial) => view.spellbooks.contains_key(&serial),
        // Nothing in the view holds the picker open, so there is nothing for an
        // overlay entry to be ahead *of* — the same as the two kinds above.
        WindowSubject::Split { .. } => false,
        // A question dismissed without an answer stays dismissed for exactly as
        // long as the fact behind it stands — a dialog's rule, one kind over.
        // Nothing tells the shard a plate was closed, so only the view settling
        // the question can clear the overlay and let the window open again.
        WindowSubject::Confirm(question) => question.stands(view),
        // And the manifest, for the question's reason: the roster is what holds
        // it open, so a window closed by hand stays closed only until the party
        // itself is gone.
        WindowSubject::Party => crate::panes::party::in_a_party(view),
    });
    own_windows.retain(|window| {
        if locally_closed.contains(&window.subject) {
            return false;
        }
        match window.subject {
            WindowSubject::Container(serial) => {
                view.containers.contains_key(&serial)
                    && !view.vendor_buys.contains_key(&serial)
                    && !view.spellbooks.contains_key(&serial)
            }
            WindowSubject::Vendor(serial) => {
                view.vendor_buys.contains_key(&serial) || view.vendor_sells.contains_key(&serial)
            }
            WindowSubject::Paperdoll(serial) => view.paperdolls.contains_key(&serial),
            WindowSubject::Spellbook(serial) => view.spellbooks.contains_key(&serial),
            WindowSubject::Dialog(gump_id) => view.gumps.iter().any(|gump| gump.gump_id == gump_id),
            // Nothing to reconcile against, and nothing to ask: the window is
            // open because it is here. `close_window`'s own `retain` is what
            // takes it away, and anything here would be a second opinion about
            // that — the two fields this replaced could each say the window was
            // shut while the window was still in this list.
            WindowSubject::Skills
            | WindowSubject::Status
            | WindowSubject::Minimap
            | WindowSubject::WorldMap
            | WindowSubject::Split { .. } => true,
            // And a question stands for as long as what it is about does. This
            // is the arm that takes an invitation off the screen when the shard
            // withdraws it, without anybody having pressed either button.
            WindowSubject::Confirm(question) => question.stands(view),
            // The last member leaving takes the window with it, without anybody
            // having pressed anything.
            WindowSubject::Party => crate::panes::party::in_a_party(view),
        }
    });
    // Containers first and paperdolls after, and both in the view's own
    // iteration order — which is a `HashMap`'s and therefore not stable. That
    // decides only where two windows opened on the *same frame* cascade to,
    // and nothing else: a window's position is its own from the moment it is
    // placed.
    let wanted = view
        .containers
        .keys()
        .filter(|serial| !view.vendor_buys.contains_key(serial) && !view.spellbooks.contains_key(serial))
        .map(|serial| WindowSubject::Container(*serial))
        .chain(
            view.paperdolls
                .keys()
                .map(|serial| WindowSubject::Paperdoll(*serial)),
        )
        .chain(
            view.spellbooks
                .keys()
                .map(|serial| WindowSubject::Spellbook(*serial)),
        );
    let wanted = wanted.chain(
        view.vendor_buys
            .keys()
            .chain(view.vendor_sells.keys())
            .map(|serial| WindowSubject::Vendor(*serial)),
    );
    for subject in wanted.collect::<Vec<_>>() {
        if own_windows.iter().any(|window| window.subject == subject) {
            continue;
        }
        // Still overlaid: the view has not caught up with the close yet, and
        // re-opening it here is exactly the reopen this overlay exists to
        // stop.
        if locally_closed.contains(&subject) {
            continue;
        }
        let step = own_windows.len() as i32 % CONTAINER_CASCADE_LENGTH;
        own_windows.push(OwnWindow {
            subject,
            at: GumpPixel::new(
                CONTAINER_ORIGIN.x + CONTAINER_CASCADE.x * step,
                CONTAINER_ORIGIN.y + CONTAINER_CASCADE.y * step,
            ),
            pane: crate::panes::AnyPane::of(subject),
        });
    }
    // No arm for either local kind here, and that is step 3's whole shape: the
    // values on a status frame are authoritative, but the decision to look at
    // them is the player's, and a `0x11` arrives at every login — so a window
    // opened from the data would be a window nobody asked for. What opens one
    // is `open_local_window`, called from the button that was pressed.
    //
    // A dialog is placed where the shard asked for it, and it is the only
    // window kind that is: a `0xB0` carries a coordinate and a `0x24` does
    // not. So no cascade — two dialogs the shard put in one place are two
    // dialogs the shard put in one place, and moving them would be this
    // client second-guessing a layout it was handed.
    let dialogs: Vec<(GumpId, GumpPixel)> = view
        .gumps
        .iter()
        .map(|gump| (gump.gump_id, GumpPixel::new(gump.at.x, gump.at.y)))
        .collect();
    for (gump_id, at) in dialogs {
        let subject = WindowSubject::Dialog(gump_id);
        if own_windows.iter().any(|window| window.subject == subject) {
            continue;
        }
        // Overlaid the same as a container or paperdoll: `answer_gump` sets
        // this before the view has forgotten the dialog, and the view is
        // what is stale here — see `App::answer_gump`.
        if locally_closed.contains(&subject) {
            continue;
        }
        own_windows.push(OwnWindow {
            subject,
            at,
            pane: crate::panes::AnyPane::of(subject),
        });
    }
    // And this client's own questions, which are reconciled like a dialog and
    // not opened like a skill sheet: a party invitation is on the screen because
    // a `0x78` said so, so the same walk that opens a window for it is the walk
    // that takes it away when the shard settles it.
    //
    // Over the *questions* rather than over anything in the view, because that
    // is where the set lives — see [`Question::ALL`](crate::panes::confirm::Question).
    // Cascaded like a bag for want of anywhere better: the reference centres its
    // `QuestionGump` on the window, which is a size this function has never been
    // told and deliberately is not — where a window goes is the manager's, and
    // the manager's own placement rule is the cascade.
    for question in crate::panes::confirm::Question::ALL {
        let subject = WindowSubject::Confirm(question);
        if !question.stands(view) {
            continue;
        }
        if own_windows.iter().any(|window| window.subject == subject) {
            continue;
        }
        // Dismissed by the player, and the question still standing: the overlay
        // above is what keeps it dismissed, and this is where it would otherwise
        // be re-opened on the very next frame.
        if locally_closed.contains(&subject) {
            continue;
        }
        let step = own_windows.len() as i32 % CONTAINER_CASCADE_LENGTH;
        own_windows.push(OwnWindow {
            subject,
            at: GumpPixel::new(
                CONTAINER_ORIGIN.x + CONTAINER_CASCADE.x * step,
                CONTAINER_ORIGIN.y + CONTAINER_CASCADE.y * step,
            ),
            pane: crate::panes::AnyPane::of(subject),
        });
    }
    // And the manifest, which is the same three questions in a row: is there a
    // party, is the window already up, and has the player put it away since.
    let subject = WindowSubject::Party;
    if crate::panes::party::in_a_party(view)
        && !own_windows.iter().any(|window| window.subject == subject)
        && !locally_closed.contains(&subject)
    {
        let step = own_windows.len() as i32 % CONTAINER_CASCADE_LENGTH;
        own_windows.push(OwnWindow {
            subject,
            at: GumpPixel::new(
                CONTAINER_ORIGIN.x + CONTAINER_CASCADE.x * step,
                CONTAINER_ORIGIN.y + CONTAINER_CASCADE.y * step,
            ),
            pane: crate::panes::AnyPane::of(subject),
        });
    }
}

#[cfg(test)]
mod grip_tests {
    use super::*;

    /// Any window will do: the grip is about two positions and a subject, and
    /// nothing it does depends on what the window is over.
    const SUBJECT: WindowSubject = WindowSubject::Skills;

    fn window_at(at: GumpPixel) -> OwnWindow {
        OwnWindow {
            subject: SUBJECT,
            at,
            pane: crate::panes::AnyPane::of(SUBJECT),
        }
    }

    /// **The phantom move.** A click that does not move the mouse must not move
    /// the window, and the scale the window is drawn at must not enter into it.
    ///
    /// The first assertion is about the *fixture* rather than about the code:
    /// it says this pointer and this scale are a case the old convention —
    /// `at = pointer - PaneFrame::cursor`, a window-local offset subtracted
    /// from an absolute pointer — placed the window somewhere other than where
    /// it already stood. Without it the test would pass just as happily on a
    /// scale of 1, where the two conventions agree and there was never a
    /// defect to catch.
    #[test]
    fn a_press_that_does_not_move_the_pointer_does_not_move_the_window() {
        let scale = crate::desk::WindowScale::new(1.7);
        let at = GumpPixel::new(120, 80);
        let pointer = GumpPixel::new(200, 160);
        let window = window_at(at);

        let local = window.local_cursor(pointer, scale);
        assert_ne!(
            GumpPixel::new(pointer.x - local.x, pointer.y - local.y),
            at,
            "this fixture has to be one the window-local offset got wrong, \
             or the case below proves nothing",
        );

        let mut grip = WindowGrip::default();
        grip.press(SUBJECT, pointer, window.at);
        assert_eq!(grip.follow(pointer), Some((SUBJECT, at)));
    }

    /// The window travels exactly as far as the pointer, whichever way it goes
    /// and however many moves it takes — every one of them measured from the
    /// press and not from the move before it, so nothing accumulates.
    #[test]
    fn the_window_travels_as_far_as_the_pointer_and_no_further() {
        let at = GumpPixel::new(120, 80);
        let pointer = GumpPixel::new(200, 160);
        let mut grip = WindowGrip::default();
        grip.press(SUBJECT, pointer, at);

        assert_eq!(
            grip.follow(GumpPixel::new(210, 155)),
            Some((SUBJECT, GumpPixel::new(130, 75))),
        );
        assert_eq!(
            grip.follow(GumpPixel::new(190, 200)),
            Some((SUBJECT, GumpPixel::new(110, 120))),
        );
        assert_eq!(
            grip.follow(pointer),
            Some((SUBJECT, at)),
            "a pointer back where it went down puts the window back where it was",
        );
    }

    /// Up is the end of it: the moves after a release are nobody's.
    #[test]
    fn a_release_lets_the_window_go() {
        let mut grip = WindowGrip::default();
        grip.press(SUBJECT, GumpPixel::new(200, 160), GumpPixel::new(120, 80));
        grip.release();
        assert_eq!(grip, WindowGrip::Idle);
        assert_eq!(grip.follow(GumpPixel::new(400, 400)), None);
        grip.release();
        assert_eq!(grip, WindowGrip::Idle, "letting go twice is letting go");
    }

    /// A press with no release before it re-anchors on the press that is
    /// happening now — the lost-release case, and the one that used to carry
    /// the *previous* gesture's numbers into this one.
    #[test]
    fn a_second_press_forgets_the_first() {
        let mut grip = WindowGrip::default();
        grip.press(SUBJECT, GumpPixel::new(200, 160), GumpPixel::new(120, 80));
        grip.press(SUBJECT, GumpPixel::new(300, 300), GumpPixel::new(40, 40));
        assert_eq!(
            grip.follow(GumpPixel::new(310, 290)),
            Some((SUBJECT, GumpPixel::new(50, 30))),
        );
    }

    /// An idle grip answers no move at all — which is what stops a pointer
    /// crossing a window from dragging it.
    #[test]
    fn an_idle_grip_carries_nothing() {
        assert_eq!(WindowGrip::default(), WindowGrip::Idle);
        assert_eq!(WindowGrip::Idle.follow(GumpPixel::new(10, 10)), None);
    }
}
