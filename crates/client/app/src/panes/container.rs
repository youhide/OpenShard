//! A bag's window as a component: the last of the six kinds to move in, and
//! the one the hand runs through.
//!
//! Step 6 of `docs/window_components.md`. Every kind before this one could be
//! moved without touching what is on the cursor; this one cannot, because a
//! bag is where an item is lifted from and where it is put down, and both of
//! those are the *shard's* state as much as this client's. Decision 7 is the
//! seam that made it possible:
//!
//! - **The press is this pane's.** [`ItemPress`] has sent nothing. It is one
//!   window's gesture, it may still turn out to be the first half of a
//!   double-click use, and it dies with the window — exactly like a doll's
//!   held button or a dialog's.
//! - **The hand is the manager's.** Once a `0x07` has gone out the item is on
//!   the shard's cursor, one per connection, and no pane may fill or empty
//!   that slot. A pane asks with [`Effect::Lift`] and [`Effect::Drop`] and
//!   reads [`PaneFrame::hand`] to draw what it must.
//!
//! # What a bag is, on the wire
//!
//! A `0x24` names one gump graphic and a `0x3C` lists what is in it, each item
//! with a coordinate measured from the background's top-left corner. There is
//! no layout program, no page and no button — see
//! [`openshard_client_render::container`], which is the whole of the drawing.
//! Everything else this window does is a client-side gesture over that
//! picture: the double click that uses an icon, the drag that lifts one, the
//! two action buttons below the frame, the face one of them takes under the
//! pointer, and the tint an icon takes under it.

use std::cell::RefCell;
use std::time::Instant;

use openshard_client_net::action::Outgoing;
use openshard_client_net::view::WorldView;
use openshard_client_render::container;
use openshard_client_render::gump::{self as gump_art, GumpArt, GumpAtlas, GumpPixel, Picture};
use openshard_protocol::containers::ContainedItem;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::serial::Serial;
use openshard_protocol::speech::Font;
use openshard_protocol::wire::{Graphic, Hue, Layer, RawLayer};

use crate::DOUBLE_CLICK;
use crate::hand::{DragOrigin, Dragged, Hand, ItemPress, PendingDrop};
use crate::panes::{Answer, Button, Effect, Input, Line, Pane, PaneCtx, PaneFrame, Response, SplitPrompt};
use crate::windows::Drawn;

/// The face both of this window's labels are written in — `fonts.mul`'s face
/// 1, the same one every other label in this client uses.
const LABEL_FONT: Font = Font(1);

/// How far down and right of the pointer a hover label sits.
///
/// The paperdoll's is the same step, so the two hovers read as one behaviour
/// rather than two that happen to both be near the cursor.
const HOVER_OFFSET: GumpPixel = GumpPixel::new(14, 18);

/// Where the first swept item is put down in the backpack, and how far apart
/// the rest are laid.
///
/// The backpack stays authoritative about the grid slots and any stack merge;
/// this only keeps the icons from landing in one heap on the way there.
const SWEEP_ORIGIN: GumpPoint = GumpPoint::new(20, 20);
const SWEEP_STEP: i32 = 18;
const SWEEP_COLUMNS: usize = 6;

/// The one graphic this client wields on a double click instead of asking the
/// shard to *use* it.
///
/// A katana's ordinary double click is a wield, and the shard has no packet
/// that means "wear this from where it is": the reference client lifts the
/// item and equips it in one breath, which is the pair below. Every other
/// graphic is a `0x06` and the shard decides what using it means.
const KATANA: Graphic = Graphic(0x13FF);

/// One open container window.
#[derive(Debug)]
pub struct ContainerPane {
    /// Which bag this is a window of.
    ///
    /// A shop's serial and a doll's mobile, one kind over: the subject carries
    /// a key, and the pane is handed it when the window opens because every
    /// packet this window sends names it — see
    /// [`AnyPane::of`](crate::panes::AnyPane::of).
    container: Serial,
    /// The press on one of this bag's icons, until it becomes a lift, a click
    /// or nothing.
    ///
    /// **Decision 7's first half, and the field this whole step was last in
    /// the plan for.** Nothing has been sent while this is alive: the item is
    /// still in the bag, still drawn there, and a release now is a click that
    /// may pair with the next one. It is the pane's for the reason
    /// [`PaperdollPane::held`](crate::panes::paperdoll) and
    /// [`DialogPane::held`](crate::panes::dialog) are — one window, one
    /// gesture, and a window that closes takes its unfinished gestures with
    /// it.
    pressed: Option<ItemPress>,
    /// The icon under the pointer, tinted on the next frame.
    ///
    /// `Windows::hovered_container_item` kept this for every bag at once,
    /// keyed by nothing at all — there was one field and the topmost window
    /// won. Per-pane, two bags can each remember their own, and the one the
    /// pointer is on is the only one that answers, because the tint is written
    /// only while [`PaneCtx::under_pointer`] says so.
    hovered: Option<Serial>,
    /// The last completed click on one of this bag's icons, and when.
    ///
    /// The pair that makes a double click a *use*. It was
    /// `Windows::last_container_click`, one field for every open bag, so two
    /// clicks on two icons in two windows could pair if they happened to be
    /// the same item — which is not a thing that can be arranged, but was not
    /// a thing the rule refused either. Per-pane, the window half is gone by
    /// construction and what is left is the icon and the clock.
    last_click: Option<(Instant, Serial)>,
    /// Whether the pointer is on this window's action button right now, which
    /// is the button's pressed face on the next frame.
    ///
    /// Separate from [`Self::hovered`] on purpose: that field is an *icon* and
    /// answers in a hue, this one is a button and answers in a
    /// [`face`](openshard_client_render::container::action_face) —
    /// the action button is furniture below the frame, so the two can never be true
    /// at once, and conflating them would mean one bit answering for two
    /// different pictures. The same shape [`Self::hovered`] already is: a
    /// memory of the last move, compared against the layout's own predicate
    /// and asked to agree with what is drawn.
    action_hovered: bool,
    /// A one-redraw memo of [`Self::contents`], left by [`art`](Pane::art)
    /// for [`layout`](Pane::layout) to pick up rather than compute again.
    ///
    /// Interior mutability because both methods take `&self` — [`Pane`]'s own
    /// shape — but this is not "whatever the bag last held": it is "whatever
    /// `art` computed a moment ago, for the `layout` call that always follows
    /// it before anything else runs." `render_passes.rs` asks every open
    /// pane's `art`, packs it, and only *then* asks every open pane's
    /// `layout` — two loops, back to back, off the same `view` and the same
    /// `hand`, with no packet fold and no input event between them (D6). That
    /// gap is this field's entire lifetime: [`Self::recall_contents`] empties
    /// it unconditionally, so a `layout` reached without a matching `art`
    /// first — the shop-crate branch, or a test calling one and not the
    /// other — finds nothing and recomputes, and can never read a value some
    /// earlier, unrelated `art` left behind.
    ///
    /// **Deliberately not offered to [`Self::press_action`].** A press runs
    /// off the input event pump, not the redraw that fills this — there is no
    /// guaranteed gap between a redraw and the press that follows it, so the
    /// action button's own call to [`Self::contents`] stays a fresh read every time.
    /// See `docs/window_components.md`'s backlog entry this closes: the
    /// broader question of a once-a-frame scratch for every pane kind stays
    /// open, and this is deliberately narrower than that.
    scratch: RefCell<Option<Vec<ContainedItem>>>,
}

/// A container, laid out for one frame: the pictures, what they *are*, and
/// what is written over them.
///
/// The middle field is what a bag's [`Drawn`] did not use to carry, and it is
/// the rule `drawn_windows` exists for, stated properly. A click used to be
/// turned into an item by picking an index out of the pictures and then
/// counting that far into a list rebuilt from the view — with the lifted icon
/// filtered out again, by hand, in the same order the layout had filtered it.
/// Two walks, one subtraction each, and nothing but care keeping them in step.
/// Now the list the pictures were built from travels with them, and
/// [`Window::item_at`] is a lookup.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Window {
    /// The background and every icon on it, in painter's order — what the
    /// pointer is tested against.
    pub pictures: Vec<Picture>,
    /// The icons, in the order the pictures after the background were built
    /// from them.
    pub contents: Vec<ContainedItem>,
    /// The action button's caption below the frame, and the name of whatever the
    /// pointer is resting on.
    pub lines: Vec<Line>,
}

impl Window {
    /// The icon at `cursor`, or `None` for the bag itself and for the pixels
    /// its art leaves transparent.
    ///
    /// Picture zero is the background — a press on it is a press on the
    /// *window*, which is the manager's business — so the answer is one place
    /// further along, and index arithmetic that goes the wrong way answers
    /// `None` rather than the last icon.
    pub fn item_at(&self, cursor: GumpPixel, atlas: &GumpAtlas) -> Option<ContainedItem> {
        let index = gump_art::pick(&self.pictures, cursor, atlas)?;
        self.contents.get(index.position().checked_sub(1)?).copied()
    }
}

/// The one client-side action button a bag may have drawn below it.
///
/// Neither is in the shard's art: a `0x24` names one gump and it has no spare
/// room for a control, so both are captions this client writes under the
/// window and tests as boxes. Which of the two a window has is decided in one
/// place — [`ContainerPane::action`] — and read by both the layout that draws
/// the caption and the press that acts on it, where there used to be a
/// predicate in `render_passes.rs` and a second one in `App` that had to agree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    /// Sweep the lot into the backpack. Every bag but the backpack itself:
    /// there is nowhere to sweep your own pack to.
    TakeAll,
    /// Compact the like piles. The backpack's own, because that is where the
    /// loot ends up.
    StackAll,
}

impl Action {
    /// Where it sits and how big it is, or `None` until the window's own art
    /// has been packed — the action button hangs off the bottom edge, so its position
    /// is the background's height.
    fn button(self, atlas: &GumpAtlas, gump: Graphic, at: GumpPixel) -> Option<container::ActionButton> {
        match self {
            Self::TakeAll => container::take_all_button(atlas, gump, at),
            Self::StackAll => container::stack_all_button(atlas, gump, at),
        }
    }

    /// Its caption.
    const fn label(self) -> &'static str {
        match self {
            Self::TakeAll => container::TAKE_ALL_LABEL,
            Self::StackAll => container::STACK_ALL_LABEL,
        }
    }
}

/// Which action button a window has, as a function of the three things that decide it.
///
/// A free function of what it actually reads, so the rule can be pinned
/// without a view — and one rule, where there used to be three that had to
/// agree: two `*_under_pointer` walks that decided which press to answer, and a
/// pair of `if`s in the text pass that decided which caption to draw. A window
/// that drew "Take all" and answered nothing would look broken and be silent
/// about it.
fn action_of(container: Serial, backpack: Option<Serial>, shop: bool) -> Option<Action> {
    if shop {
        // A shop's crate is not loot to sweep and not a pile to compact: its
        // rows are bought with a `0x3B`.
        return None;
    }
    match backpack == Some(container) {
        true => Some(Action::StackAll),
        false => Some(Action::TakeAll),
    }
}

/// Whether the action button wears its pressed face: the pointer is on it,
/// and there is a button at all.
///
/// A free function of the two things that decide it, for the reason
/// [`action_of`] is: `button` is already `None` unless the *caller* has
/// established this window owns the pixel under the cursor
/// (`PaneCtx::under_pointer`), so a covering window's own action button cannot make
/// this one glow through it, and this predicate itself stays one comparison
/// pinned without a [`PaneCtx`].
fn action_is_lit(button: Option<container::ActionButton>, cursor: GumpPixel) -> bool {
    button.is_some_and(|button| button.contains(cursor))
}

/// Whether a click on `item` at `now` is the second of a pair.
///
/// The same shape as the paperdoll's scroll pairing, and for the same reason
/// the window half of it is missing: each bag keeps its own, so two clicks in
/// two windows cannot pair by construction. What is left is the icon and the
/// clock — a click on one bottle and then on the next is two first clicks, and
/// the second does not drink the first.
fn pairs(last: Option<(Instant, Serial)>, now: Instant, item: Serial) -> bool {
    last.is_some_and(|(at, serial)| serial == item && now.duration_since(at) <= DOUBLE_CLICK)
}

/// What one completed pair of clicks on an icon asks for.
///
/// A `0x06` for everything the shard knows how to use, and for [`KATANA`] the
/// lift-and-equip a drag onto the doll would have sent. **Both halves of that
/// pair are ordinary [`Effect::Net`]s and neither is [`Effect::Lift`]**: there
/// is no moment at which the player is holding the sword, so filling the hand
/// would draw an icon under a pointer nobody is dragging and leave it there if
/// the shard refused the equip.
fn double_clicked(item: ContainedItem, worn: RawLayer, player: Serial) -> Response {
    let answer = Response::changed();
    if item.graphic != KATANA {
        return answer.with(Effect::Net(Outgoing::Use(item.serial)));
    }
    answer
        .with(Effect::Net(Outgoing::PickUp {
            item: item.serial,
            amount: item.amount,
        }))
        .with(Effect::Net(Outgoing::Equip {
            item: item.serial,
            layer: worn,
            mobile: player,
        }))
}

/// The sweep behind "Take all": one ordinary lift and drop per item, in the
/// order the bag listed them.
///
/// Every pair stays on the server-authoritative drag path, so reach,
/// ownership and weight are all checked the way they would be for a hand
/// drag. The grid is only to keep the icons apart on the way in — the backpack
/// decides where they actually land, and whether two of them merge.
///
/// Nothing touches the hand, for [`double_clicked`]'s reason: the player never
/// holds any of this.
fn take_all(contents: &[ContainedItem], backpack: Serial) -> Vec<Effect> {
    contents
        .iter()
        .enumerate()
        .flat_map(|(index, item)| {
            let column = (index % SWEEP_COLUMNS) as i32;
            let row = (index / SWEEP_COLUMNS) as i32;
            [
                Effect::Net(Outgoing::PickUp {
                    item: item.serial,
                    amount: item.amount,
                }),
                Effect::Net(Outgoing::DropInto {
                    item: item.serial,
                    container: backpack,
                    at: GumpPoint::new(
                        SWEEP_ORIGIN.x + column * SWEEP_STEP,
                        SWEEP_ORIGIN.y + row * SWEEP_STEP,
                    ),
                }),
            ]
        })
        .collect()
}

/// The backpack this character is wearing, if any.
fn backpack_of(view: &WorldView) -> Option<Serial> {
    view.player
        .equipment
        .iter()
        .find(|item| item.layer == Layer::BACKPACK)
        .map(|item| item.serial)
}

/// What the hover label says: the item's name, and how many of it.
fn hover_text(item: &ContainedItem, name: &str) -> String {
    format!("{name} ({})", item.amount.0)
}

impl ContainerPane {
    /// The pane for the bag `container` names.
    pub const fn new(container: Serial) -> Self {
        Self {
            container,
            pressed: None,
            hovered: None,
            last_click: None,
            action_hovered: false,
            scratch: RefCell::new(None),
        }
    }

    /// The icon this window is tinting, if the pointer is on one.
    ///
    /// The one thing outside the window layer that asks a bag a question: the
    /// tooltip pick order puts an icon in an open bag in front of anything in
    /// the world behind it, and that order is `picking_query`'s rather than
    /// this pane's. `App::hovered_container_item` is the walk that asks.
    pub const fn hovered(&self) -> Option<Serial> {
        self.hovered
    }

    /// Whether this window is an ordinary bag rather than a shop's crate.
    ///
    /// A shop is a [`WindowSubject::Vendor`](crate::windows::WindowSubject)
    /// and never a `Container`: `reconcile_own_windows` refuses to open one for
    /// a serial the view holds a buy catalogue for, and it runs at the top of
    /// every frame — so a *layout* can take this for granted, and does.
    ///
    /// A press cannot, and that is the only reason this exists. Input arrives
    /// between frames, so a `0x74` folded since the last reconcile leaves this
    /// window standing as a bag for one gap, and a press landing in it would
    /// put a `0x07` on the wire for a shop's stock — which is bought with a
    /// `0x3B` or not at all.
    fn ordinary(&self, frame: &PaneFrame<'_>) -> bool {
        !frame.view.vendor_buys.contains_key(&self.container)
    }

    /// Which client-side action button this window has below it, if any.
    fn action(&self, frame: &PaneFrame<'_>) -> Option<Action> {
        action_of(self.container, backpack_of(frame.view), !self.ordinary(frame))
    }

    /// Which action button this window has, and where it sits — resolved once so the
    /// layout that draws it, the caption written over it, the press that
    /// acts on it and the hover state that tints it all agree by
    /// construction. `None` until the container's own art and the action button's
    /// are both packed, the same as [`Action::button`] on its own.
    fn action_button(&self, frame: &PaneFrame<'_>) -> Option<(Action, container::ActionButton)> {
        let action = self.action(frame)?;
        let gump = *frame.view.containers.get(&self.container)?;
        // Window-local — see `PaneFrame::cursor`'s doc — so `button.at` comes
        // out measured from this window's own top-left, exactly like every
        // icon in it.
        let button = action.button(frame.files.gump_atlas, gump, GumpPixel::new(0, 0))?;
        Some((action, button))
    }

    /// The icons this window draws, which is the view's list with the hand's
    /// two corrections applied.
    ///
    /// **A successful lift is not echoed back to the client that made it** —
    /// that client has already taken the icon out of its own gump — so the
    /// held item is subtracted here for as long as the hand holds it. And a
    /// drop this window is waiting on is added at the place it was dropped, so
    /// there is no gap between letting go and the shard's answer.
    ///
    /// Takes `view` and `hand` rather than a whole [`PaneFrame`] on purpose:
    /// neither this nor [`Self::recall_contents`] touches `frame.resources`,
    /// and a test that wants to pin the caching in [`Self::scratch`] would
    /// otherwise need a full [`Resources`](crate::resources::Resources) — the
    /// same wall step 8 of `docs/window_components.md` is still standing
    /// behind.
    fn contents(&self, view: &WorldView, hand: Option<Hand>) -> Vec<ContainedItem> {
        let held = hand.map(|hand| hand.drag().item.serial);
        let mut contents: Vec<ContainedItem> = view
            .contents
            .get(&self.container)
            .into_iter()
            .flatten()
            .filter(|item| Some(item.serial) != held)
            .copied()
            .collect();
        if let Some(Hand::Dropped {
            drag,
            destination: PendingDrop::Container { container, at },
        }) = hand
        {
            if container == self.container {
                contents.push(ContainedItem { at, ..drag.item });
            }
        }
        contents
    }

    /// What [`layout`](Pane::layout) reads: whatever [`art`](Pane::art) left
    /// in [`Self::scratch`] a moment ago, or a fresh [`Self::contents`] if
    /// there is nothing there.
    ///
    /// The `take()` is unconditional — it empties the cell whether or not it
    /// found anything — so a value can be read here at most once, and a call
    /// with no matching `art` immediately before it recomputes rather than
    /// answering with somebody else's frame.
    fn recall_contents(&self, view: &WorldView, hand: Option<Hand>) -> Vec<ContainedItem> {
        self.scratch
            .borrow_mut()
            .take()
            .unwrap_or_else(|| self.contents(view, hand))
    }

    /// The two things written over this window: the action button's caption, and the
    /// name of the icon the pointer is resting on.
    ///
    /// Resolved here rather than in `client/render` because both need what
    /// that crate has never heard of — the atlas the action button's position comes
    /// out of, and the `tiledata` an item's name does. The pass draws what the
    /// layout produced, the way it draws a dialog's captions and a doll's name
    /// action button.
    fn lines(&self, frame: &PaneFrame<'_>, contents: &[ContainedItem]) -> Vec<Line> {
        let mut lines = Vec::new();
        if let Some((action, button)) = self.action_button(frame) {
            lines.push(Line {
                at: button.label_at(),
                font: LABEL_FONT,
                hue: Hue::LABEL,
                clip: None,
                text: action.label().to_owned(),
            });
        }
        if let Some(item) = self
            .hovered
            .and_then(|serial| contents.iter().find(|item| item.serial == serial))
        {
            lines.push(Line {
                at: frame.cursor.offset(HOVER_OFFSET),
                font: LABEL_FONT,
                hue: Hue::LABEL,
                clip: None,
                text: hover_text(item, &frame.files.tiledata.static_tile(item.graphic.0).name),
            });
        }
        lines
    }

    /// Whether this was the second click on the same icon, and the bookkeeping
    /// either way: a completed pair arms nothing and a first click arms the
    /// next, so three quick clicks are a pair and a single.
    fn paired(&mut self, item: Serial, now: Instant) -> bool {
        let paired = pairs(self.last_click, now, item);
        self.last_click = (!paired).then_some((now, item));
        paired
    }

    /// A left press inside the window's own art, action button included.
    ///
    /// Four answers, and every one of them raises: a press on a window is
    /// what puts it on top. The action button is tried first — it is furniture below
    /// the frame and not an icon, so [`Window::item_at`] below never sees it
    /// and would otherwise read a action button press as a grab of the bag itself.
    /// Short of that, an icon either completes a pair — a use, and nothing is
    /// held — or starts a press this pane keeps; anything else is the bag
    /// itself, which is the grip the window is moved by.
    fn press(&mut self, window: &Window, ctx: &PaneCtx<'_>) -> Response {
        let raised = Response::changed().with(Effect::Raise);
        // `ctx.frame.cursor` is already local to this window — see
        // `PaneFrame::cursor`'s doc — so it is the grab offset outright.
        let grab = || Effect::Grab(ctx.frame.cursor);
        if !self.ordinary(&ctx.frame) {
            return raised.with(grab());
        }
        // `press_action`'s own box test is safe to run unconditionally here:
        // `ctx.under_pointer`, already true or `press` would not have been
        // called, means this window owns the pixel the press landed on —
        // action button included, now that it is drawn as a real picture — so the
        // only question left is which part of the window's own art it was.
        let action_answer = self.press_action(ctx);
        if action_answer.taken {
            return action_answer;
        }
        let Some(item) = window.item_at(ctx.frame.cursor, ctx.frame.files.gump_atlas) else {
            return raised.with(grab());
        };
        if self.paired(item.serial, ctx.now) {
            let worn = RawLayer(ctx.frame.files.tiledata.static_tile(item.graphic.0).layer);
            let mut answer = raised;
            answer.absorb(double_clicked(item, worn, ctx.frame.view.player.serial));
            return answer;
        }
        // Where inside the icon the player took hold of it, so that the item
        // keeps its place under the pointer once it is on the cursor — and so
        // that a drop lands where the icon looks like it is, rather than where
        // the pointer happens to be inside it. `item.at` is already this
        // window's own local coordinate (the wire's own, measured from the
        // background's top-left) and so is `ctx.frame.cursor`, so no window
        // placement has to be added in to compare the two.
        let icon = GumpPixel::new(item.at.x, item.at.y);
        self.pressed = Some(ItemPress {
            item,
            origin: DragOrigin::Container(self.container),
            at: ctx.frame.cursor,
            grab: GumpPixel::new(ctx.frame.cursor.x - icon.x, ctx.frame.cursor.y - icon.y),
        });
        raised
    }

    /// A left press on the action button below the window.
    ///
    /// **Only called from [`Self::press`], and only once that has already
    /// established this window owns the pixel the press landed on.** The
    /// action button used to be offered no pixels at all, so every window's
    /// `press_action` had to run its own box test against the raw cursor —
    /// which is what let a action button belonging to a window the pointer could not
    /// see still answer a press (`docs/window_components.md`'s backlog entry
    /// this closes). Now the action button is a picture in [`Window::pictures`], so
    /// [`PaneCtx::under_pointer`] already means "the pointer is on this
    /// window's own art, action button included" before this is ever reached, and
    /// this box test only decides *which part* of that art it landed on.
    fn press_action(&self, ctx: &PaneCtx<'_>) -> Response {
        let Some((action, button)) = self.action_button(&ctx.frame) else {
            return Response::ignored();
        };
        if !button.contains(ctx.frame.cursor) {
            return Response::ignored();
        }
        let raised = Response::changed().with(Effect::Raise);
        match action {
            Action::StackAll => raised.with(Effect::StackAll),
            Action::TakeAll => match backpack_of(ctx.frame.view) {
                Some(backpack) => take_all(&self.contents(ctx.frame.view, ctx.frame.hand), backpack)
                    .into_iter()
                    .fold(raised, Response::with),
                // Nothing to sweep into. The action button is drawn — a body can lose
                // its pack — and pressing it asks for nothing.
                None => raised,
            },
        }
    }

    /// The release that ends whatever this window was in the middle of.
    ///
    /// Two things can be, and never both at once: the hand can be full over
    /// this bag, or this pane can be holding a press. The manager's own gate
    /// is what makes the pair impossible — a press never reaches a pane while
    /// the hand is full — which is step 2's argument with the case it was
    /// waiting for.
    fn release(&mut self, ctx: &PaneCtx<'_>) -> Response {
        if let (Some(hand), true) = (ctx.frame.hand, ctx.under_pointer) {
            // The gesture targets the *bag*, never an icon drawn in it: a
            // floor item released over a nested pack would otherwise enter
            // that pack, with its coordinates measured in the wrong window.
            // Merging two stacks is a destination gesture of its own and is
            // not this one.
            let grab = hand.drag().grab;
            // `ctx.frame.cursor` is already this window's own local pixels —
            // see `PaneFrame::cursor`'s doc — so only the grab offset inside
            // the icon is left to subtract.
            let at = GumpPoint::new(ctx.frame.cursor.x - grab.x, ctx.frame.cursor.y - grab.y);
            return Response::changed().with(Effect::Drop(PendingDrop::Container {
                container: self.container,
                at,
            }));
        }
        // The press is suspended under the amount prompt: letting the button
        // go is what *opened* it, and putting the press down here would leave
        // the answer with nothing to divide.
        if ctx.frame.has_prompt {
            return Response::consumed();
        }
        match self.pressed.take() {
            // A click that never became a drag. The double-click decision was
            // made on the press, so there is nothing left to do but let go —
            // and nothing new to draw, which the answer says.
            Some(_) => Response::consumed(),
            None => Response::ignored(),
        }
    }

    /// The move that turns a press into a lift, and keeps the tint honest.
    ///
    /// Both, in that order and not one or the other: the pointer that lifts an
    /// icon has also left it, so the tint it was wearing has to go with it.
    /// Neither is `taken` — a move is not an exclusive event — and the answer
    /// is [`Response::stale`] exactly when something on the screen changed.
    fn moved(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let mut answer = self.hover(ctx);
        answer.absorb(self.hover_action(ctx));
        if ctx.frame.has_prompt {
            return answer;
        }
        let Some(press) = self.pressed else {
            return answer;
        };
        match press.dragged(ctx.frame.cursor, ctx.modifiers.shift) {
            Dragged::Still => answer,
            // The prompt goes up once and the press waits under it. `Still` is
            // what every following move answers, because `has_prompt` above is
            // true from here until the number arrives.
            Dragged::Ask(most) => {
                answer.absorb(Response::stale().with(Effect::Prompt(SplitPrompt {
                    item: press.item.serial,
                    most,
                })));
                answer
            }
            Dragged::Lift(drag) => {
                self.pressed = None;
                self.hovered = None;
                answer.absorb(Response::stale().with(Effect::Lift(drag)));
                answer
            }
        }
    }

    /// Follow the pointer with the icon the next frame will tint.
    ///
    /// Gated on [`PaneCtx::under_pointer`] so that a bag covered by another
    /// window does not light up through it — unlike the vendor's plate tint,
    /// which is not z-gated because its *layout* is not either. The rule is
    /// the same one stated both times: what the pane remembers and what the
    /// layout draws have to be computed by the same predicate.
    fn hover(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let hovered = match (ctx.under_pointer, ctx.drawn) {
            (true, Some(Drawn::Container(window))) => window
                .item_at(ctx.frame.cursor, ctx.frame.files.gump_atlas)
                .map(|item| item.serial),
            _ => None,
        };
        if self.hovered == hovered {
            return Response::ignored();
        }
        self.hovered = hovered;
        Response::stale()
    }

    /// Follow the pointer with whether it is on this window's action button.
    ///
    /// [`Self::hover`]'s counterpart for the action button rather than an icon, and
    /// gated the same way and for the same reason: `under_pointer` already
    /// means the action button's own pixels — not just the button's box — are what
    /// the pointer is on, now that the action button is drawn rather than left as
    /// text over nothing. A move that leaves it up is the fix for the
    /// backlog entry this step closes read the other way round: a pressed face
    /// that used to be wearable through a window drawn on top cannot be any more,
    /// because [`Self::action_button`]'s box test is only ever asked while
    /// `under_pointer` says this window owns the pixel.
    fn hover_action(&mut self, ctx: &PaneCtx<'_>) -> Response {
        let button = ctx
            .under_pointer
            .then(|| self.action_button(&ctx.frame))
            .flatten();
        let on = action_is_lit(button.map(|(_, button)| button), ctx.frame.cursor);
        if self.action_hovered == on {
            return Response::ignored();
        }
        self.action_hovered = on;
        Response::stale()
    }

    /// The amount prompt has been answered, and the press it suspended is
    /// this pane's.
    ///
    /// The split is finished here rather than left on the cursor, and that is
    /// deliberate: the lift path suppresses the `Remove` for the client that
    /// made it, so a part left hanging on the pointer after a modal can hide
    /// the old serial indefinitely. Dropping it back beside its remainder
    /// produces an `Add` for that serial, and asking for the container again
    /// replaces the whole list — including the remainder the shard has just
    /// made.
    fn answered(&mut self, answer: Answer) -> Response {
        let Some(press) = self.pressed.take() else {
            return Response::ignored();
        };
        let Answer::Split(amount) = answer else {
            // Dismissed, and the press goes with it: the player said no to the
            // only question this gesture was asking.
            return Response::changed();
        };
        let Some(drag) = press.split(amount) else {
            return Response::changed();
        };
        // Beside the pile it came out of, one icon down and along, so the two
        // halves do not sit on top of each other while the shard decides.
        let at = GumpPoint::new(press.item.at.x + SWEEP_STEP, press.item.at.y + SWEEP_STEP);
        Response::changed()
            .with(Effect::Lift(drag))
            .with(Effect::Drop(PendingDrop::Container {
                container: self.container,
                at,
            }))
            .with(Effect::Net(Outgoing::Use(self.container)))
    }
}

impl Pane for ContainerPane {
    /// The background, every icon in it, and the action button's own art when this
    /// window has one.
    ///
    /// Asked of the view every frame rather than remembered *across* frames,
    /// because what is in a bag changes without the window hearing about it —
    /// and the atlas answers a repeat with nothing, so asking for the same
    /// list twice costs a comparison. The action button's graphic is the same one
    /// graphic for every window that has one, so this never grows the atlas
    /// past its first ask — it is here at all only because [`Self::action`] is
    /// a question about the view (a shop's crate has no action button) that this
    /// method must not answer a second, disagreeing way.
    ///
    /// What this call to [`Self::contents`] computes *is* remembered for the
    /// rest of this one redraw — see [`Self::scratch`] — so [`layout`],
    /// called next for this same window, does not ask a third time.
    fn art(&self, frame: &PaneFrame<'_>) -> Vec<GumpArt> {
        match frame.view.containers.get(&self.container) {
            Some(gump) => {
                let contents = self.contents(frame.view, frame.hand);
                let mut wanted = container::art_of(*gump, &contents);
                if self.action(frame).is_some() {
                    // Both faces, not the one this frame happens to want: the
                    // pointer arriving on the button must not be the moment
                    // the atlas first hears of the pressed art, or the press
                    // would draw nothing for the one frame the pack takes.
                    wanted.push(GumpArt::Gump(container::ACTION_UP));
                    wanted.push(GumpArt::Gump(container::ACTION_DOWN));
                }
                // Left for `layout` — called next, for this same window,
                // before anything else runs (see `Self::scratch`) — to pick
                // up instead of asking `contents` a second time.
                *self.scratch.borrow_mut() = Some(contents);
                wanted
            }
            // The window is open and the view has no `0x24` for it — one frame
            // between the packet that took the bag away and the reconcile that
            // will take the window with it. Nothing computed, so nothing to
            // leave in `Self::scratch` either — `layout` takes the same path
            // and finds it empty (or a stranger's leftover, which it discards).
            None => Vec::new(),
        }
    }

    fn layout(&self, frame: &PaneFrame<'_>) -> Option<Drawn> {
        let gump = *frame.view.containers.get(&self.container)?;
        let contents = self.recall_contents(frame.view, frame.hand);
        let action = self
            .action_button(frame)
            .map(|(_, button)| (button, container::action_face(self.action_hovered)));
        // Window-local — see `PaneFrame::cursor`'s doc.
        let pictures =
            container::window_with_action(gump, &contents, GumpPixel::new(0, 0), self.hovered, action);
        let lines = self.lines(frame, &contents);
        Some(Drawn::Container(Window {
            pictures,
            contents,
            lines,
        }))
    }

    /// A press on an icon, on the action button below, or on the bag itself, the
    /// release that finishes whichever gesture that was, the move that
    /// lifts, and the prompt's answer.
    ///
    /// The press is *located*, and reaching this pane at all means
    /// [`PaneCtx::under_pointer`] is already true — the router only offers a
    /// located input to the window whose own art, action button included, is the
    /// topmost opaque pixel under the cursor. `press` decides which of the
    /// three that was. The release and the move are offered to every window
    /// — a press finishes wherever the pointer has got to, and a tint left
    /// behind on the bag the pointer just left has to be cleared.
    ///
    /// **No wheel.** Nothing in a bag scrolls: the shard sends coordinates and
    /// the window is exactly as big as its art, so a notch over one goes past
    /// it to the camera, as it always has.
    fn handle(&mut self, input: Input, ctx: &PaneCtx<'_>) -> Response {
        match input {
            Input::Press(Button::Left) => {
                // Nothing of this window — background, icon or action button — is
                // the topmost pixel here, so there is nothing of ours for the
                // press to have landed on. A window drawn over this one's
                // action button now correctly wins that pixel instead of this pane
                // being asked anyway (`docs/window_components.md`'s closed
                // backlog entry).
                if !ctx.under_pointer {
                    return Response::ignored();
                }
                // Never drawn, so there is nothing to hit-test against and no
                // pixels the player can have meant. The window is still raised
                // and moved by the manager's own path.
                let Some(Drawn::Container(window)) = ctx.drawn else {
                    return Response::ignored();
                };
                self.press(window, ctx)
            }
            Input::Release(Button::Left) => self.release(ctx),
            Input::Move => self.moved(ctx),
            Input::Answered(answer) => self.answered(answer),
            Input::Wheel(_) | Input::Press(Button::Right) | Input::Release(Button::Right) | Input::Key(_) => {
                Response::ignored()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::containers::GridSlot;
    use openshard_protocol::items::ItemAmount;

    use crate::panes::fixture;

    use super::*;

    fn serial(raw: u32) -> Serial {
        Serial::new(raw).expect("an item serial")
    }

    fn item(raw: u32, graphic: Graphic, amount: u16) -> ContainedItem {
        ContainedItem {
            serial: serial(raw),
            graphic,
            amount: ItemAmount(amount),
            at: GumpPoint::new(0, 0),
            grid: GridSlot(0),
            hue: Hue::NONE,
        }
    }

    /// A bare view with our own body entered and nothing else — enough to
    /// carry one bag's contents for the caching tests below. `WorldView` has
    /// no `Default` on purpose (see its own doc comment); `entered` is its
    /// cheapest real constructor and touches no client file.
    fn bare_view() -> WorldView {
        use openshard_protocol::direction::{Direction, Facing};
        use openshard_protocol::world::{MapSize, PlayerStart, Point};
        WorldView::entered(PlayerStart {
            serial: serial(0x0000_002A),
            body: Graphic(0x0190),
            position: Point::new(1475, 1770, 20),
            facing: Facing::walking(Direction::South),
            map: MapSize::BRITANNIA,
        })
    }

    /// The pairing rule, minus the half the shape now answers: two clicks in
    /// two bags cannot pair, because each window keeps its own.
    #[test]
    fn two_clicks_on_one_icon_are_a_pair_and_two_icons_are_not() {
        let first = Instant::now();
        let soon = first + DOUBLE_CLICK / 2;
        let bottle = serial(0x4000_0001);

        assert!(
            !pairs(None, first, bottle),
            "the first click of all pairs with nothing"
        );
        assert!(pairs(Some((first, bottle)), soon, bottle));
        assert!(
            !pairs(Some((first, bottle)), soon, serial(0x4000_0002)),
            "the bottle beside it is not the same bottle"
        );
        assert!(
            !pairs(
                Some((first, bottle)),
                first + DOUBLE_CLICK + std::time::Duration::from_millis(1),
                bottle
            ),
            "and a click a breath too late is a first click"
        );
    }

    /// A completed pair disarms the state, so three quick clicks are a pair
    /// and a single rather than a pair and a half.
    #[test]
    fn a_completed_pair_starts_over() {
        let mut pane = ContainerPane::new(serial(0x4000_0000));
        let first = Instant::now();
        let step = DOUBLE_CLICK / 4;
        let bottle = serial(0x4000_0001);

        assert!(!pane.paired(bottle, first), "armed");
        assert!(pane.paired(bottle, first + step), "paired");
        assert!(
            !pane.paired(bottle, first + step * 2),
            "the third click starts a new pair rather than riding the old one"
        );
    }

    /// The ordinary double click asks the shard to use the item, and the one
    /// graphic this client wields itself asks for the two packets a drag onto
    /// the doll would have sent — as `Net`, so nothing is ever on the cursor.
    #[test]
    fn a_double_click_uses_the_icon_and_a_katana_is_worn() {
        let me = serial(0x0000_002A);
        let bottle = item(0x4000_0001, Graphic(0x0F0E), 1);
        let answer = double_clicked(bottle, RawLayer(0), me);
        assert!(matches!(
            answer.out.as_slice(),
            [Effect::Net(Outgoing::Use(used))] if *used == bottle.serial
        ));

        let katana = item(0x4000_0002, KATANA, 1);
        let answer = double_clicked(katana, RawLayer(1), me);
        assert!(
            matches!(
                answer.out.as_slice(),
                [
                    Effect::Net(Outgoing::PickUp { item, .. }),
                    Effect::Net(Outgoing::Equip { item: worn, layer, mobile })
                ] if *item == katana.serial && *worn == katana.serial && layer.0 == 1 && *mobile == me
            ),
            "the wield is a lift and an equip, and neither is the hand"
        );
    }

    /// A sweep is one lift and one drop per item, in the bag's own order and
    /// laid out six to a row so the icons do not land in one heap.
    #[test]
    fn a_sweep_names_every_item_twice_and_the_backpack_once() {
        let pack = serial(0x4000_0100);
        let contents: Vec<ContainedItem> = (0..7)
            .map(|index| item(0x4000_0001 + index, Graphic(0x0EED), 1))
            .collect();
        let effects = take_all(&contents, pack);
        assert_eq!(effects.len(), 14, "a lift and a drop each");

        let Effect::Net(Outgoing::DropInto { container, at, .. }) = &effects[1] else {
            panic!("the second effect of a pair is the drop");
        };
        assert_eq!(*container, pack);
        assert_eq!(*at, GumpPoint::new(20, 20), "the first lands at the origin");

        let Effect::Net(Outgoing::DropInto { at, .. }) = &effects[13] else {
            panic!("the seventh pair's drop");
        };
        assert_eq!(
            *at,
            GumpPoint::new(20, 38),
            "the seventh item starts the second row"
        );
    }

    /// An empty bag sweeps nothing, rather than asking for a lift of nothing.
    #[test]
    fn a_sweep_of_an_empty_bag_asks_for_nothing() {
        assert!(take_all(&[], serial(0x4000_0100)).is_empty());
    }

    /// The hover label names what is under the pointer and how many of it —
    /// the shard's own tooltip is a separate thing and arrives later.
    #[test]
    fn a_hover_names_the_item_and_its_amount() {
        let gold = item(0x4000_0001, Graphic(0x0EED), 137);
        assert_eq!(hover_text(&gold, "gold coins"), "gold coins (137)");
    }

    /// Which action button a window has, in the one place that decides it: the
    /// backpack compacts, every other bag is swept into it, and a shop's crate
    /// has neither.
    #[test]
    fn the_backpack_compacts_and_every_other_bag_is_swept() {
        let pack = serial(0x4000_0100);
        let chest = serial(0x4000_0200);

        assert_eq!(action_of(pack, Some(pack), false), Some(Action::StackAll));
        assert_eq!(action_of(chest, Some(pack), false), Some(Action::TakeAll));
        assert_eq!(
            action_of(chest, None, false),
            Some(Action::TakeAll),
            "a body with no pack is still offered the action button, and pressing it asks for nothing"
        );
        assert_eq!(
            action_of(chest, Some(pack), true),
            None,
            "a shop's rows are bought, not swept"
        );
    }

    /// The predicate the hover tint narrows to, and the same shape
    /// [`action_of`]'s test uses: a free function pinned without a
    /// [`PaneCtx`]. `button` being `None` stands in for
    /// [`hover_action`](ContainerPane::hover_action)'s own gate — a window that
    /// does not own the pixel under the cursor is never handed a button to
    /// test at all — so a covering window cannot light this one's action button
    /// through it.
    #[test]
    fn the_action_lights_up_only_on_its_own_box() {
        let button = container::ActionButton {
            at: GumpPixel::new(100, 100),
            size: (210, 19),
        };
        assert!(action_is_lit(Some(button), GumpPixel::new(150, 110)));
        assert!(
            !action_is_lit(Some(button), GumpPixel::new(400, 400)),
            "well outside the action button"
        );
        assert!(
            !action_is_lit(None, GumpPixel::new(150, 110)),
            "no button at all — this window does not own the pixel"
        );
    }

    /// The action button's own picture sits exactly where the button says, and a
    /// click on it is not a click on any icon — `item_at`'s contract for the
    /// background applies to the action button the same way, now that it too is
    /// drawn. A shop's crate, which has no action button, draws no extra picture at
    /// all.
    #[test]
    fn an_action_picture_sits_at_the_buttons_position_and_is_not_an_icon() {
        use openshard_client_render::gump::{GumpArt, GumpAtlas};
        use openshard_uofiles::color::Color16;
        use openshard_uofiles::image::Image;

        const BAG: Graphic = Graphic(0x003C);
        let block = |width: u16, height: u16| {
            Image::new(
                width,
                height,
                vec![Color16(0x7FFF); usize::from(width) * usize::from(height)],
            )
        };
        let atlas = GumpAtlas::pack([
            (GumpArt::Gump(BAG), block(140, 100)),
            (GumpArt::Gump(container::ACTION_UP), block(30, 22)),
            (GumpArt::Gump(container::ACTION_DOWN), block(30, 22)),
        ])
        .expect("three small blocks fit an atlas 2048 on a side");

        let at = GumpPixel::new(100, 50);
        let button = container::take_all_button(&atlas, BAG, at).expect("both are packed");
        let window = Window {
            pictures: container::window_with_action(
                BAG,
                &[],
                at,
                None,
                Some((button, container::action_face(true))),
            ),
            contents: Vec::new(),
            lines: Vec::new(),
        };
        assert_eq!(
            window
                .pictures
                .last()
                .map(|picture| (picture.graphic, picture.at)),
            Some((GumpArt::Gump(container::ACTION_DOWN), button.at)),
            "the action button is the last picture drawn, at the button's own position, wearing the face it was handed"
        );
        assert_eq!(
            window.item_at(button.at, &atlas),
            None,
            "the action button is furniture below the frame, not an icon"
        );

        let no_action = Window {
            pictures: container::window_with_action(BAG, &[], at, None, None),
            contents: Vec::new(),
            lines: Vec::new(),
        };
        assert_eq!(
            no_action.pictures.len(),
            1,
            "a shop's crate has no action button, so nothing beyond the background is drawn"
        );
    }

    /// **What is clicked is what was drawn.** The list the pictures were built
    /// from travels with them, so an index into the pictures is a lookup and
    /// not a second walk — and picture zero is the bag itself, which is the
    /// window rather than anything in it.
    #[test]
    fn a_click_lands_on_the_icon_that_was_drawn_there() {
        use openshard_client_render::gump::{GumpArt, GumpAtlas};
        use openshard_uofiles::color::Color16;
        use openshard_uofiles::image::Image;

        const BAG: Graphic = Graphic(0x003C);
        const CANDLE: Graphic = Graphic(0x0A28);
        let block = |width: u16, height: u16| {
            Image::new(
                width,
                height,
                vec![Color16(0x7FFF); usize::from(width) * usize::from(height)],
            )
        };
        let atlas = GumpAtlas::pack([
            (GumpArt::Gump(BAG), block(140, 100)),
            (GumpArt::Item(CANDLE), block(10, 20)),
        ])
        .expect("two small blocks fit an atlas 2048 on a side");

        let contents = vec![
            ContainedItem {
                at: GumpPoint::new(20, 20),
                ..item(0x4000_0001, CANDLE, 1)
            },
            ContainedItem {
                at: GumpPoint::new(60, 20),
                ..item(0x4000_0002, CANDLE, 1)
            },
        ];
        let at = GumpPixel::new(100, 50);
        let window = Window {
            pictures: container::window_highlighted(BAG, &contents, at, None),
            contents: contents.clone(),
            lines: Vec::new(),
        };

        assert_eq!(
            window.item_at(GumpPixel::new(165, 75), &atlas),
            Some(contents[1]),
            "the second candle, at its own place in the bag"
        );
        assert_eq!(window.item_at(GumpPixel::new(125, 75), &atlas), Some(contents[0]));
        assert_eq!(
            window.item_at(GumpPixel::new(200, 130), &atlas),
            None,
            "the bag itself is the window, not something in it"
        );
        assert_eq!(
            window.item_at(GumpPixel::new(10, 10), &atlas),
            None,
            "and nothing at all outside the window"
        );
    }

    /// `recall_contents` reuses exactly what was left for it — the case
    /// `art` and `layout` hit every ordinary redraw — rather than answering
    /// with a fresh (and here, deliberately *different*) computation. Proves
    /// reuse rather than coincidence: the stashed value and the view/hand
    /// passed alongside it disagree, so a pass-through to `contents` would
    /// be caught.
    #[test]
    fn recall_contents_reuses_what_art_left_behind() {
        let pane = ContainerPane::new(serial(0x4000_0000));
        let stashed = vec![item(0x4000_0009, Graphic(0x0EED), 1)];
        *pane.scratch.borrow_mut() = Some(stashed.clone());

        // A view that would compute something else entirely, so a bug that
        // silently ignored the cache and recomputed would be visible.
        let mut view = bare_view();
        view.contents
            .insert(pane.container, vec![item(0x4000_0001, Graphic(0x0A28), 1)]);

        assert_eq!(
            pane.recall_contents(&view, None),
            stashed,
            "the cached list wins over a fresh computation"
        );
    }

    /// The other half of the same contract: nothing was stashed — either
    /// because `art` never ran (a test calling `recall_contents` on its
    /// own) or because it hit the no-gump branch and left the cell empty —
    /// so the answer is an ordinary, fresh `contents()`.
    #[test]
    fn recall_contents_falls_back_to_a_fresh_read_when_nothing_was_cached() {
        let pane = ContainerPane::new(serial(0x4000_0000));
        let mut view = bare_view();
        let candle = item(0x4000_0001, Graphic(0x0A28), 1);
        view.contents.insert(pane.container, vec![candle]);

        assert_eq!(
            pane.recall_contents(&view, None),
            vec![candle],
            "no cache to reuse, so this is `contents()` itself"
        );
    }

    /// The staleness this cache exists to rule out: a value left over from
    /// an earlier bag state must never survive to answer a *later* one.
    /// `take()` is what guarantees it — reading the cache once empties it —
    /// so a second read in a row, standing in for "the next redraw", cannot
    /// see the first redraw's answer even though nothing here calls `art`
    /// again to overwrite it.
    #[test]
    fn a_read_cache_does_not_survive_to_answer_the_next_read() {
        let pane = ContainerPane::new(serial(0x4000_0000));
        let mut view = bare_view();
        let first_item = item(0x4000_0001, Graphic(0x0A28), 1);
        view.contents.insert(pane.container, vec![first_item]);

        // Simulates one redraw: `art` stashes what it computed for the bag
        // as it stood then, `layout` reads it back through the same call
        // this test makes directly.
        *pane.scratch.borrow_mut() = Some(pane.contents(&view, None));
        let first_read = pane.recall_contents(&view, None);
        assert_eq!(first_read, vec![first_item]);

        // The bag changes — a packet lands, standing in for the gap between
        // one redraw and the next — and nothing refills the cache before
        // this second read, exactly as would happen if `layout` somehow ran
        // twice without an intervening `art`.
        let second_item = item(0x4000_0002, Graphic(0x0EED), 1);
        view.contents.insert(pane.container, vec![second_item]);
        let second_read = pane.recall_contents(&view, None);

        assert_eq!(
            second_read,
            vec![second_item],
            "the second read reflects the bag's current contents, not the first read's stale list"
        );
    }

    /// **Step 8, through the front door.** `ItemPress::dragged` already pins
    /// the slop-then-lift rule (`hand.rs`'s own tests); this goes in through
    /// [`Pane::handle`] instead — the located gate, the `drawn` lookup and
    /// `Window::item_at` ahead of it — which is what `docs/window_components.md`'s
    /// backlog entry asked for: a press on an icon becomes a lift without a
    /// client install on disk, now that [`fixture::Install`] answers every
    /// borrow a bag's own `handle` reads.
    #[test]
    fn a_press_on_an_icon_becomes_a_lift_through_handle() {
        const BAG: Graphic = Graphic(0x003C);
        const CANDLE: Graphic = Graphic(0x0A28);
        let files = fixture::Install::shipping([
            (GumpArt::Gump(BAG), (140, 100)),
            (GumpArt::Item(CANDLE), (10, 20)),
        ]);
        let bag = serial(0x4000_0100);
        let mut view = bare_view();
        view.containers.insert(bag, BAG);
        view.contents.insert(
            bag,
            vec![ContainedItem {
                at: GumpPoint::new(20, 20),
                ..item(0x4000_0001, CANDLE, 1)
            }],
        );

        let mut pane = ContainerPane::new(bag);
        // Inside the candle's own icon, at (20, 20) sized 10×20.
        let icon = GumpPixel::new(25, 30);
        let laid_out = pane
            .layout(&files.ctx(&view, None, icon, true).frame)
            .expect("a bag with a gump in the view lays itself out");
        let ctx = files.ctx(&view, Some(&laid_out), icon, true);
        let press = pane.handle(Input::Press(Button::Left), &ctx);
        assert!(press.taken, "a press on our own window is always ours");
        assert!(
            matches!(press.out.as_slice(), [Effect::Raise]),
            "nothing has been sent yet — the press is only a press, held by the pane"
        );
        assert!(pane.pressed.is_some());

        // Past the slop, so the held press has become a drag.
        let dragged = icon.offset(GumpPixel::new(10, 0));
        let laid_out = pane
            .layout(&files.ctx(&view, None, dragged, true).frame)
            .expect("the bag still lays out the same way");
        let ctx = files.ctx(&view, Some(&laid_out), dragged, true);
        let answer = pane.handle(Input::Move, &ctx);
        assert!(
            matches!(answer.out.as_slice(), [Effect::Lift(_)]),
            "the move past the slop is what turns a press into a lift"
        );
        assert!(
            pane.pressed.is_none(),
            "the press is spent once it becomes a lift"
        );
    }

    /// The end-to-end shape `art`/`layout` actually run: two frames, two
    /// different bags of items, and each frame's `layout` seeing exactly
    /// that frame's own state — never a lingering answer from the frame
    /// before it. This is the regression the backlog entry
    /// (`docs/window_components.md`) was closed against: three redundant
    /// recomputations became one memo per redraw, and it had to be provably
    /// impossible for that memo to leak across a change in what the bag
    /// holds.
    #[test]
    fn the_cache_tracks_a_bag_that_changes_from_one_frame_to_the_next() {
        let pane = ContainerPane::new(serial(0x4000_0000));
        let mut view = bare_view();

        // Frame one: a single candle in the bag, nothing on the cursor.
        let candle = item(0x4000_0001, Graphic(0x0A28), 1);
        view.contents.insert(pane.container, vec![candle]);
        *pane.scratch.borrow_mut() = Some(pane.contents(&view, None)); // art
        assert_eq!(pane.recall_contents(&view, None), vec![candle]); // layout

        // Frame two: the candle is gone (swept, used, whatever) and a bottle
        // has taken its place. A stale cache would still say "candle".
        let bottle = item(0x4000_0002, Graphic(0x0F0E), 1);
        view.contents.insert(pane.container, vec![bottle]);
        *pane.scratch.borrow_mut() = Some(pane.contents(&view, None)); // art
        assert_eq!(
            pane.recall_contents(&view, None), // layout
            vec![bottle],
            "the second frame's layout sees the second frame's bag"
        );
    }
}
