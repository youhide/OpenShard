# A window that owns itself: panes, not branches in `App`

Every kind of window this client draws of its own owns its own state and its own
input, and one router offers each event to them in order. A pane **takes readonly
context in and hands mutations out**; it never holds an `&mut App`, never reaches
the shard, and never decides where it sits on the screen.

`WindowSubject` is the list, and it is the authority on how many kinds there are:
a container, a vendor's catalogue, a paperdoll, a `0xB0` dialog, the skill sheet,
the status frame, the amount picker, a confirmation, the party roster, the
minimap, the facet map and a spellbook. The argument below was written against
the first six and the rest joined afterwards without changing it, which is the
property it was written for.

The rest of this document is the argument, kept in the voice it was written in,
because every decision below is still the rule. What each window *is* — its art,
its layout, the index spaces it keeps apart — is
[`design_windows.md`](design_windows.md); how the six kinds actually moved in,
and what that found, is
[`evidence/2026-08-17-the-pane-router.md`](evidence/2026-08-17-the-pane-router.md).

The state it replaced: every gesture was a method on `App` that asked "which
window am I over" again and then took the subject apart on the spot, and every
window's state was a public field of one shared struct that three different
files wrote to.

## Why

Three things are wrong today, and they are one thing.

**Input is a chain of `bool`s that mean two things at once.** The wheel handler
is `scroll_skills() || scroll_vendor() || zoom()`
(`crates/client/app/src/event_loop.rs`), and that one `bool` answers both "this
event is mine" and "ask for a redraw". A shop list scrolled to its last row
answered "nothing moved", the chain fell through, and the wheel became a map
zoom under a pointer that had never left the window — fixed on 2026-08-17 by
making `scroll_vendor` answer the first question, which is a fix by convention
that the next window kind is free to get wrong again.

**No window has private state.** `Windows` (`crates/client/app/src/windows.rs`)
holds `vendor_scrolls`, `vendor_amounts`, `skills`, `held_skill`, `held_doll`,
`last_scroll`, `last_container_click` and `dialogs` as public fields. (**Every one of them is gone**: the first two with
S1, the next two with S2, `dialogs` with S4, `held_doll` and `last_scroll` with
S5, and `last_container_click` with S6. The paragraph is what the plan was
written against, and the Steps below are what has gone.)
`own_windows.rs` writes them from the press path, `render_passes.rs` reads
*and* writes them while laying the frame out (`vendor_amounts.entry(..)` at
`render_passes.rs:190`), and `close_window` has to remember `vendor_scrolls
.remove(&serial)` by hand, because nothing ties a shop's scroll position to the
shop window's lifetime.

**A window talks to the shard from inside its own click handler.**
`press_on_own_window` calls `self.world.shard.link()` and sends `pick_up_item`,
`equip` and `use_object` in the middle of a 260-line function that also
branches on five window kinds. Nothing about a vendor's arithmetic can be tested
without a socket, an atlas and a whole `App` — which needs real client asset
files to construct at all.

## The shape this works toward

```rust
/// One of the client's own windows, as a component.
trait Pane {
    /// The art this pane will need packed before it can be laid out.
    fn art(&self, ctx: &PaneCtx) -> Vec<GumpArt>;

    /// Lay the pane out for this frame: the pictures the pass draws and the
    /// pointer is tested against.
    fn layout(&self, ctx: &PaneCtx) -> Option<Drawn>;

    /// Offer one input. The answer says whether the pane took it, whether the
    /// frame is stale because of it, and what it is asking the client to do.
    fn handle(&mut self, input: Input, ctx: &PaneCtx) -> Response;
}

struct Response {
    /// The event stops here: neither the camera, nor the world, nor a window
    /// under this one ever sees it.
    taken: bool,
    /// The frame is stale. **Separate from `taken` on purpose** — see D4.
    redraw: bool,
    /// What the pane wants done, in order. The manager performs these.
    out: Vec<Effect>,
}
```

Panes are stored in an `enum Pane`, one variant per kind, whose `match`
delegates to the type inside. The manager owns the list, the z-order, each
window's position, and the loop that offers an event to panes from the top down.

## Decisions

**D1. One type per window kind, held in an `enum`, not behind `dyn`.** Each kind
becomes a struct with private fields (`VendorPane { scroll, amounts }`,
`SkillsPane { tree, held }`, …) and an `impl Pane`. They are stored as
`enum Pane { Vendor(VendorPane), Skills(SkillsPane), … }` with one delegating
`match` per trait method.

`Box<dyn Pane>` was the alternative and is rejected. An `enum` is what makes a
seventh window kind a *compile error* everywhere the manager still needs to know
which kind it has — the same reason `WindowSubject` and `Drawn` are enums today
— and the delegating `match` is six lines per method, paid once. `dyn` would
also put an allocation and a vtable in the middle of a list this client walks
several times per frame, for a set of kinds that is known at compile time and
has grown by three in a year.

**D2. Position, z-order and the drag that moves a window belong to the manager.**
A pane reads `at` out of its context and **never writes it**: the cascade,
`raise_window`, `grip` and the close gesture are all the manager's, and a
pane that wants to be moved simply declines the press.

`OwnWindow` keeps `subject` and `at` and **gains the pane beside them** —
`OwnWindow { subject, at, pane }`, built by `reconcile_own_windows` when the
window opens. An earlier draft of this decision said the record stays exactly as
it is and the panes live in a list of their own; that would be a second list
keyed by subject, and a subject in one and not the other is precisely the bug
class this plan is closing. State that lives in the record is dropped by the
`retain` that closes the window, which is what makes `close_window`'s manual
`vendor_scrolls.remove` deletable at all. This is what the user asked for in the phrase this plan was
commissioned with — *"позиция менеджится внешне"* — and it is also why a pane
does not need to know it is a window at all.

Coordinates stayed **absolute gump pixels** through S0–S7, the way `Picture`,
`Drawn` and `gump_art::pick` were then, with the pane subtracting `ctx.at`
where it hit-tested — which is what `vendor::Window::contains` did. Converting
the whole layer to window-local coordinates was a real improvement and a
different plan, filed in the Backlog rather than folded in here because it
touches the render crate and this plan deliberately did not — **closed**,
after S7, by the Backlog entry of that name.

**D3. The context is readonly, and it carries the clock.**

```rust
struct PaneCtx<'a> {
    view: &'a WorldView,        // the authoritative picture
    resources: &'a Resources,   // art, tiledata, skill names, the gump atlas
    at: GumpPixel,              // where the manager has put this window
    cursor: GumpPixel,          // the pointer, in gump pixels
    modifiers: Modifiers,       // shift/ctrl, for the split-drag
    now: Instant,               // for every double-click pair
    hand: Option<ItemDrag>,     // what is on the cursor, if anything — D7
}
```

No `&mut App`, no `Link`, no `&mut WorldView`. `now` is passed rather than read
from `Instant::now()` inside a pane, for the reason the tick's `Rng` is owned
rather than ambient: a pair of clicks is a *timing* rule, and a rule read from
an ambient clock cannot be exercised by a test. `hand` is readonly for the same
reason it is in the context at all: a pane needs it to answer "was something
dropped on me" and to draw a wearable's preview, and no pane may fill or empty
it — see D7.

**D4. `taken` and `redraw` are two fields, because they are two questions.**
This is the whole of the wheel defect, stated as a type instead of a comment.
A pane at the end of its list took the notch (`taken: true`) and has nothing new
to draw (`redraw: false`); a pane whose hover tint changed took nothing
(`taken: false`) and is stale (`redraw: true`). Today's `bool` cannot say either
of those, and the chain in `event_loop.rs` reads it as whichever one the last
author had in mind.

**D5. Effects are the mutation, and `Outgoing` is most of them.**
`client-net`'s `Outgoing` (`crates/client/net/src/action.rs`) is already the
complete vocabulary of "what this client asks the shard for" — `Buy`, `Sell`,
`PickUp`, `Equip`, `Use`, `SkillLock`, `AnswerGump`. A pane returns
`Effect::Net(Outgoing)` and never touches the link; the manager sends it. The
rest of `Effect` is what only the manager can do:

```rust
enum Effect {
    Raise,                       // this pane to the top of the pile
    Close,                       // and off the list; overlay handled by the manager
    Grab,                        // take hold of this window; *where* is the
                                 // manager's to read — the `GumpPixel` this arm
                                 // was sketched with is the Backlog's
                                 // "two frames of reference in one field"
    Net(Outgoing),               // the shard's half — both halves of a transfer
                                 // among them, as two ordinary effects (D7)
    Open(LocalWindow),           // the skill sheet or the status frame
    Prompt(SplitPrompt),         // the client-side amount dialog, whose answer
                                 // has to find its way back — see the Backlog
}
```

Reusing `Outgoing` rather than minting a pane vocabulary is deliberate: a second
enum that means the same things is a second place to add a packet to, and the
translation between them would be a `match` whose arms are all identities.

`Open` names a `LocalWindow` — the skill sheet or the status frame — and not a
`WindowSubject`. Those two are the only kinds whose *existence* is this client's
own: a container or a paperdoll is open because the shard opened it, so asking
for one is `Net(Use)` or `Net(Paperdoll)` and the window appears when the view
grows the entry `reconcile_own_windows` turns into one. An `Open(WindowSubject)`
would have two unanswerable arms and would read as though a pane could conjure a
bag.

**D6. Art packing is a phase of its own, before layout.** `layout` takes `&self`
and a *shared* atlas, which is only possible because packing is a separate call:
`vendor::art_of()`, `container::art_of(..)`, `paperdoll::art_of(..)` and
`gump::art_of(..)` already exist as free functions and are already called ahead
of layout in `render_passes.rs`. The trait states that order instead of leaving
it to the caller: `art` for every pane, pack once, then `layout` for every pane.

**D7. The hand is a slot, not a gesture: a transfer is two transactions.** An
earlier draft of this decision said that lifting a sword out of a bag and
dropping it on a paperdoll is "one gesture over two windows", by analogy with an
egui drag payload, and kept the transaction with the manager for that reason.
The conclusion stands; the reason was wrong, and the reason is what decides the
shape.

Lifting and dropping are two independent requests with **server state between
them**. `0x07` puts the item into a slot on the connection, `0x08` or `0x13`
takes it out, and `0x27` bounces it back to where it came from. This shard's own
server is already built exactly that way: `Connection::held: Option<HeldItem>`
(`crates/server/state/src/connection.rs:107`) holds an item that is "in limbo
until a `0x08` lands it", one per connection because "a cursor holds one thing",
and a second lift is answered by bouncing the first
(`DragCancelReason::AlreadyHolding`, `crates/server/items/src/drag.rs:31`).

So the hand stays full for as long as the player likes. They can walk, open a
third bag, close the window the item came out of, or drop the connection — which
the server handles by name, because an item in the hand is off every sector and
out of everyone's `seen`. The pane that sourced the item need not exist when the
drop happens, and there is no gesture spanning anything.

That splits today's `ItemDragTransaction` along the seam `owns_cursor` already
admits — "after `Held` the shard may already have detached the item from its
source":

- `Pressed` — a press that has not moved yet and may still become a double-click
  "use". One window, one gesture, nothing sent. **Private to the pane**, like
  every other press-and-release pair the window layer already keeps
  (`held_doll`, `held_skill`, `gump::Dialogs::holding`).
- `Held` and `Dropped` — the hand, and a drop whose answer is in flight. A
  mirror of the server's slot, so **the manager's**, and readonly in `PaneCtx`.

There is one hand because the *server* has one, not because this plan picked a
number: the invariant is `AlreadyHolding`, read off the other end of the wire.
ClassicUO agrees about the owner — `ItemHold` is a field of `GameCursor`
(`GameCursor.cs:144`), not of any gump, and the eight gumps that mention it only
read it.

What this buys is that there is no `Lift`/`Drop` effect *pair*. A pane emits
`Net(Outgoing::PickUp)`; later, and possibly from another pane, some pane emits
`Net(Outgoing::DropInto | Equip | DropOnGround)`. Two ordinary effects with
nothing paired about them. What stays with the manager is the part that was never
a pane's: the precondition that a press does nothing at all while the hand is
full — today the first question `press_on_own_window` asks
(`own_windows.rs:724`) — the local projection that subtracts the item from its
source until the shard answers (`reproject_item_drag`), and drawing the item on
the cursor, which is the cursor's job and not a window's.

**D8. What is not a pane.** `own_windows`, `locally_closed`,
`reconcile_own_windows` and the cascade stay where they are — they are about
*which* windows exist, which is the manager's question and is already settled by
`docs/client_window_state.md`. `Drawn` stays the layout type; panes produce it.

**D9. An exclusive input device is the manager's, and what it lands on is the
pane's.** Added by S4, and it is D2 and D7 stated once for all three: z-order,
the hand and the keyboard are all *one of a kind on the screen*, so the manager
owns which window has each — `grip`, `item_drag`, `keyboard`, every one of
them a subject — and the pane owns what that means inside itself. A pane reads
the manager's half out of its context (`under_pointer`, `hand`,
`has_keyboard`) and never writes it, and asks for a change with an effect.

The failure this prevents is the one the plan keeps meeting: two panes each
believing they hold the same thing. `Dialogs::focus` avoided it by being one
field for every dialog at once, which is the map-keyed-by-window shape this
plan exists to undo; `DialogPane::focus` avoids it by being read only when the
manager says the keys are coming here.

