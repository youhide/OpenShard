# A window that owns itself: panes, not branches in `App`

The client draws six kinds of its own window — a container, a vendor's
catalogue, a paperdoll, a `0xB0` dialog, the skill sheet and the status frame —
and not one of them owns anything. Every gesture is a method on `App` that asks
"which window am I over" again and then takes the subject apart on the spot, and
every window's state is a public field of one shared struct that three different
files write to.

This plan gives each window kind a type that owns its own state and its own
input, and gives the client one place where an event is offered to windows in
order. A pane **takes readonly context in and hands mutations out**; it never
holds an `&mut App`, never reaches the shard, and never decides where it sits on
the screen.

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

## Steps

Each step compiles and the client runs at the end of it. Panes move in one at a
time, and until a kind has moved, its variant delegates to the existing `App`
method behind the same trait — so the router is real from S1 onward and there is
never a half-routed frame.

- [x] **S0. The trait, the context, the response, the router.** ✅ `panes.rs`
      and `panes/route.rs`: `Pane`, `PaneCtx`, `Input`, `Response`, `Effect`,
      `AnyPane`, and `App::deliver` — top-down, first `taken` wins.
      `event_loop.rs` no longer chains `||` for a window: the five `CursorMoved`
      calls, the three-term release, the press, the right-button close and the
      wheel are one `deliver` each. **Five things landed differently from the
      shape above, and each is written down where it belongs:**
      - `trait Pane` and `enum Pane` cannot both exist. The trait keeps the
        name; the enum is `AnyPane`.
      - The trait has **only `handle`** for now. `art` and `layout` (D6) join it
        with the first pane that has a layout of its own — a pane with no state
        cannot lay anything out, and `render_passes.rs` still lays out all six.
      - `Panes::deliver` is `App::deliver`, because the router still has to
        reach the legacy handlers and those need the whole `App`. The loop over
        the panes itself takes nothing but the list.
      - The panes live in `OwnWindow` (see D2), so the list *is*
        `Windows::own_windows` and there is no second one to keep in step.
      - A shim cannot call `press_on_own_window` from inside `handle`, because
        the context is readonly by construction and that method is not per-kind
        anyway — it hit-tests all six itself. So the router has **three rungs**:
        the manager's own gestures, then the panes, then the legacy chain
        reached only when no pane answered. The third rung is what S7 deletes,
        and while it stands the conflation the wheel defect was made of lives in
        one function instead of five call sites.

      `Effect::Prompt` is not there yet: nothing emits it until S6, and *who a
      modal's answer is addressed to* is a Backlog entry that has to be settled
      first.
- [x] **S1. Vendor.** ✅ `panes/vendor.rs`: `VendorPane { vendor, scroll,
      amounts }`, an `impl Pane` with all three methods, and every question
      about a shop gone from `App` — `scroll_vendor`, `confirm_vendor`, the
      vendor arm of
      `press_on_own_window`, the vendor arm of `render_passes.rs`'s layout loop
      and `Link::buy`/`Link::sell` are all deleted. `close_window`'s two manual
      `remove`s went with the two maps. **Six things landed differently from
      the shape above:**
      - **The context is two structs.** `PaneFrame` is what a pane may read
        while it is packed and laid out (`view`, `resources`, `at`, `cursor`,
        `hand`); `PaneCtx` is that plus what is only true of an *event*. D3
        wrote one, and one does not survive contact with `draw_gump_windows`:
        that function has the view, the files and the pointer, and would have to
        invent a clock, a modifier state and a z-order answer to fill fields no
        layout reads. `PaneCtx { frame, .. }`, so a pane says `ctx.frame.view`.
      - **`PaneCtx::drawn`** — how this window was laid out on the *last* frame.
        Not in D3, and it has to be: **what is clicked is what was drawn**
        (`Windows::drawn_windows`), so a pane that hit-tested a layout it worked
        out at press time would be asking a second question whose answer is free
        to differ from the picture the player is pointing at.
      - **`PaneCtx::under_pointer`** — this window covers the cursor and no
        window above it does. A pane knows where the cursor is *inside* itself
        and cannot know what is drawn over it, and z-order is the manager's by
        D2. It is `App::window_under_pointer`, which every legacy handler opens
        with, handed over instead of asked again.
      - **A located input stops at the window the pointer is on.** The walk in
        `offer_to_panes` breaks after that window for a press and a notch —
        nothing below it may answer either. Without it a moved-in pane two
        windows down takes a click that landed on a bag drawn over it, because
        the kinds that have not moved in decline everything. A release and a
        move stay unbounded: a release finishes a press wherever the pointer has
        got to, and a move is offered to every window.
      - **The hand-full gate moved to the manager**, out of
        `press_on_own_window`'s first lines and into `manager_gestures` — D7 said
        it stays with the manager, and it has to run ahead of the *panes* and not
        only ahead of the legacy chain: a shop that answered a press while the
        hand was full would count up a row instead of doing nothing.
      - **A pane knows what it is a pane of.** `AnyPane::of` hands the vendor's
        serial to `VendorPane::new`: a `0x3B` names the mobile it is addressed
        to, and that is not the subject, the position or the z-order.

      Two things this fixed by construction rather than on purpose. **The
      catalogue is chosen once**: `Stall::of` prefers the buy list, and the
      three old readers disagreed — the frame drew the shop's stock while
      Confirm sold the player's own goods, for a serial in both maps. And the
      order is zipped against the *lines*, so a quantity left over from a
      catalogue that has since shrunk cannot travel.
- [x] **S2. Skills.** ✅ `panes/skills.rs`: `SkillsPane { tree, held }`, an
      `impl Pane` with all three methods, and `own_windows/skills.rs` deleted
      whole — `skill_hit_under_pointer`, `skill_content`, `skill_clicked`,
      `drag_thumb` and `scroll_skills` with it, along with the skills arms of
      `press_on_own_window` and `release_on_own_window`, the skills arm of
      `render_passes.rs`'s layout loop, and `Link::set_skill_lock`/`use_skill`.
      `legacy_window_input`'s wheel arm is **empty**, which is the milestone
      this plan was commissioned for: the `||` chain that made a notch over a
      window a zoom of the map has no terms left.
      **Five things landed differently from the shape above:**
      - **`Windows::skills` did not become a field of the pane; it stopped
        existing.** The `Option<Tree>` was the tree *and* the openness, so this
        step had to answer both. The tree is `SkillsPane::tree`, and the
        openness is the window being in `own_windows` — which means
        `reconcile_own_windows` lost its `skills_open` argument rather than
        having it renamed: its `Skills` arm is now `true`, because a window is
        open by being in the list it is being asked about. Four writers of the
        old field (`close_window`, `sync_own_windows`, the disconnect arm,
        the paperdoll's button) are down to two callers of one door.
      - **That door is `windows::open_local_window`, and it is a free
        function.** `Effect::Open(LocalWindow::Skills)` performs it, and the
        paperdoll's Skills button — legacy until S5 — calls the same one, so
        there is no second way to open the sheet while the migration is half
        done. Its contract is that it is **idempotent**: pressing Skills again
        leaves the window it finds alone, which is what the old
        `get_or_insert_with(Tree::default)` was carefully spelling in two
        places. The status window's cascade in `reconcile_own_windows` is the
        same call now, so S3 is a `bool` to delete rather than a block to
        write.
      - **🚨 A local window has to be closed by name where the view is not the
        authority.** Everything else is dropped by `reconcile_own_windows`
        because the view stopped listing its subject; a skill sheet has no
        subject in the view, so the disconnect arm of `net_command.rs` retains
        it out of the list explicitly. That is the price of "presence is the
        fact" for a kind the view cannot answer for, and it is the honest
        version of the old `windows.skills = None`. S3 inherits the same line.
      - **The wheel's answer changed, and it is the only behaviour this step
        did not merely move.** `scroll_skills` returned one `bool` and returned
        it `true`, so a sheet at the bottom of its list took the notch *and*
        asked for a frame with nothing new on it. `SkillsPane::wheel` answers
        `consumed` there and `changed` otherwise — the harmless half of the
        conflation whose other half was the vendor's visible defect.
      - **The sheet's layout is unconditional, unlike a shop's.** A vendor pane
        answers `None` for a catalogue that has left the view, so
        `render_passes.rs` keeps a reachable arm for it; a skill sheet draws its
        own frame with nothing at all in the view, so its arm is unreachable and
        says so.

      **One ordering this step changed, and why it is safe.** The sheet's
      release used to be the *last* question asked on the way up — the third
      term of `release_container_item() || release_container_press() ||
      release_on_own_window()` — and it is now the first, because panes are
      offered an input ahead of the legacy chain. Nothing can be held by both:
      a press only reaches a pane while the hand is empty (the manager's gate,
      S1), and the press that fills the hand is a press on a container, so
      `SkillsPane::held` and an item transaction cannot be live at the same
      time. Any later kind that keeps a press of its own inherits this
      question, and S6's container is where it stops being trivially true.
- [x] **S3. Status.** ✅ `panes/status.rs`: `StatusPane`, a **unit struct** with
      an `impl Pane` — the kind that proves a pane with no state and no input at
      all is still a pane. The layout moved out of `render_passes.rs`, and
      `Windows::status` is gone: it was the last field in the client that said
      "this window is open" anywhere but in the list of open windows.
      **Four things landed differently from the shape above, and one backlog
      entry closed with the step:**
      - **`reconcile_own_windows` has no openness argument left at all.** S2 took
        `skills_open` and this takes `status_open`, so its signature is the view,
        the list and the overlay — nothing else. Both local kinds answer `true`
        in its `retain` for the same one reason, written once: *the window is
        open because it is here*.
      - **`WindowSubject::is_local()`, which is the Backlog entry this step was
        told to do with itself rather than after itself.** The disconnect arm of
        `net_command.rs` had to name `Skills` to drop it, and S3 would have added
        `Status` beside it — two names to keep in step with
        `open_local_window`'s. It is a predicate now: *nothing in the view holds
        this window open*. A third local kind adds a variant to that `matches!`
        and nothing else.
      - **`Effect::Open` is one arm, through `LocalWindow::subject()`.** It was
        two, and the second of them was `self.windows.status = true` — the field
        rather than the door. Both go through `windows::open_local_window` now,
        and so does the paperdoll's legacy button, whose two `bool`s became one
        `Option<WindowSubject>` for the same reason: one request, one door, one
        difference between them.
      - **The `None` arm of this pane's layout is reachable, unlike the sheet's.**
        The Status button asks for a fresh `0x11` and opens the window in one
        press, so there are frames in which the window is open and this client
        has not a single number to write on the frame. Drawing the empty gump
        then would be a status window belonging to nobody, so the pane answers
        `None` and the window appears with its numbers on the frame the reply
        lands — which is a shop's shape (`render_passes.rs` keeps a reachable
        arm for it), not a sheet's.

      **One ordering changed, the same one S2 changed.** The frame used to be
      appended by the *reconcile* on the frame after the press, because the
      press only set a `bool`; it is appended by the press now. Two windows
      opened on one frame can therefore cascade and stack in a different order
      than before — which was already true of the skill sheet, and is the same
      "position is the manager's" it always was.
- [x] **S4. Dialog.** ✅ `panes/dialog.rs`: `DialogPane { gump_id, page,
      switches, entries, held, focus }`, an `impl Pane` with all three methods,
      and `crate/gump.rs` deleted whole — `Dialogs`, its `by_dialog` map, its
      `sync`, its `layout`, its `lines`, its `press`/`release` and its four
      keyboard methods with it. The dialog arms went out of
      `press_on_own_window`, out of `release_on_own_window` (which is now the
      paperdoll's alone), out of `render_passes.rs`'s layout loop, and out of the
      per-view art loop one rung above the windows. **Six things landed
      differently from the shape above:**
      - **🚨 The keyboard is a manager's slot, exactly as the hand is.**
        `Dialogs::focus` was `(GumpId, TextEntryId)` — the window *and* the box,
        in one field on a struct that held every dialog at once. Split by the
        same rule D7 splits a transfer by: *which window* the keys go to is
        `Windows::keyboard`, because a keyboard is exclusive across windows and
        no pane can see what another has taken; *which box inside it* is
        `DialogPane::focus`. A pane reads `PaneFrame::has_keyboard` — the
        manager's answer, like `under_pointer` — before it draws a caret, so a
        field left focused in a window the player clicked away from is inert
        rather than a second opinion.
      - **`Input::Key`, routed by identity and not by the pointer.** The router's
        walk is otherwise top-down and stops at the window under the cursor; a
        keystroke is offered to the window `Windows::keyboard` names and to no
        other, because a player typing into a field can raise a bag over it and
        the letters must not follow the click. That is this plan's own Backlog
        entry — *who a modal's answer is addressed to* — answered for the
        keyboard, and it is one `addressed` line inside the existing walk rather
        than a second router. `Key` is three arms (`Typed(char)`, `Backspace`,
        `Done`) and not a key code: which physical key means which stays in
        `event_loop.rs`, and a character rather than the keyboard's `&str`
        keeps `Input` free of a lifetime.
      - **`Effect::Answer`, which is the one close that travels.** D5 lists
        `AnswerGump` among the `Outgoing` a pane emits as `Effect::Net`, and that
        is half of what answering a dialog is: nothing on the wire ever says the
        window is gone, so the `0xB1` *is* the close. As `Net` beside `Close` it
        would be two effects with a coupling — and the `Close` arm asks a dialog
        for its **dismissal** answer, so button zero would go out behind the
        button the player actually pressed.
      - **Nothing is seeded, and that is what deleted `sync`.** `Dialogs::sync`
        copied every layout's `initial` flags into a map on the first frame and
        was careful never to do it again. Absence means the layout now, so there
        is no first time to get right — and the *answer* is built from the
        layout's own list of switches and fields rather than from what the map
        happens to hold, which is what the seeding was quietly arranging in a
        place where forgetting it would have dropped a field from the packet.
        The rest of `sync` was a `retain` keyed by gump id, and that is
        `reconcile_own_windows`'s, which drops the window and the pane together.
      - **A dialog's text is resolved in the layout, so `Drawn::Dialog` carries
        it.** A caption is a *key* — into the wire's text table or the client's
        cliloc — and resolving one needs the `OpenGump`, the cliloc and what the
        player has typed. The text pass used to reach into `Dialogs` for all
        three; `Drawn::Dialog(dialog::Window { art, lines })` means both passes
        draw a dialog's text the way they draw a shop's, out of what the layout
        produced. **And it is what found the Backlog entry below to be wrong**:
        the second walk had been iterating `std::iter::empty` since window text
        moved next to its own art, so its arms were unreachable rather than
        disagreeing. It is deleted.
      - **`{ nomove }` is the one press that is taken and does not grab**, and
        it could not have stayed in the tail of `press_on_own_window`: that tail
        ends every press it reaches with a grab. It is one arm of
        `DialogPane::press` now, beside the arm that does grab.

      **One ordering changed, the same one S1 and S2 changed.** A press on a
      dialog is answered before `press_on_own_window`'s container furniture —
      the take-all and stack-all buttons — where it used to be answered after
      them. Both of those already bail when a window covers them, and a dialog
      only takes a press it is the topmost window for, so the difference is a
      button drawn outside its own bag's picture and under a dialog: the visible
      window wins now.
- [x] **S5. Paperdoll.** ✅ `panes/paperdoll.rs`: `PaperdollPane { mobile, held,
      last_scroll, hovered, hand_over }`, an `impl Pane` with all three
      methods, and `own_windows/paperdoll.rs` deleted whole —
      `doll_button_under_pointer`, `doll_clicked` and `scroll_paired` with it,
      along with `App::hover_paperdoll_item`, `App::release_on_own_window`
      (whose last tenant it was), the paperdoll layout and text arms of
      `render_passes.rs`, `lib.rs`'s `scroll_pairs`, and five `Link` methods
      (`status`, `skills`, `quest_log`, `guild_menu`, `virtue` — plus
      `log_out`, whose one remaining caller was a channel-bounds test that now
      exercises `stop_attacking`). The Skills and Status buttons are the
      `Effect::Open` that had been waiting under `#[expect(dead_code)]` since
      S2, so both expects are gone, and so are the ones on `PaneFrame::hand`
      and `PaneCtx::now` — the pane reads both. **Six things landed
      differently from the shape above:**
      - **🚨 One press is declined on purpose, and it is the migration's
        arrow reversed.** A press on a worn item of the player's own body
        starts an item transfer, and the transfer machinery — `Pressed`, the
        slop, the split prompt — is the hand's (D7) and moves at S6 with the
        container, "the one the hand runs through". The pane answers
        `ignored` for exactly that press, so the walk falls through to the
        legacy chain, whose paperdoll arm keeps its worn-item half and lost
        its button half. Everywhere else the legacy chain answers because a
        pane has not moved in; here it answers because the pane says so.
      - **`Drawn::Paperdoll` carries its text, and `Line` moved up a level.**
        The name on the plate comes out of the view and the hover label out of
        `tiledata`, so S4's shape applies: `panes::paperdoll::Window { doll,
        lines }`, resolved in the layout. The owned line type is
        `panes::Line` now — hoisted out of `panes/dialog.rs` and given a
        `font` field, because the plate's face is named by the render crate
        and a `Line` that hardcoded `CAPTION_FONT` would have been right by
        coincidence. Both app-side kinds share it; the render-side kinds keep
        their own lines beside their layouts.
      - **The hover is pane state, and the preview is half of one.**
        `hovered: Option<Layer>` and `hand_over: bool` are written by
        `Input::Move` and answered with `Response::stale` exactly when either
        changed — the Backlog's "a picture that depends on the pointer owes a
        frame", paid by remembering what the picture was. The preview *item*
        is deliberately not remembered: layout reads it out of `frame.hand`,
        so a hand that has emptied takes the preview with it on the next
        frame whether or not the pointer moves again. `preview_equipment`
        kept a copy, and the copy could outlive the drag it described.
      - **`last_scroll` lost its subject key.** The old `scroll_pairs`
        compared "same window" as a field; per-pane state makes two clicks on
        two dolls unpairable by construction, so the rule that is left is the
        picture and the clock. Its tests moved with it, minus the
        cross-window case, which is now a sentence in a comment rather than
        an assertion.
      - **`button_effects` is a free function of what a click actually
        reads.** `doll_clicked` read the whole `App`; the mapping from a
        button to its packets and windows takes `(own, war, backpack,
        paired)` and can be pinned without a view — which is what the pane's
        five tests do, checked by two mutations (drop the button comparison
        from the pairing rule, drop the `own` guard from Status/Skills; one
        named test reddens for each).
      - **The vendor's tint went with this step, as the Backlog entry said it
        should.** `VendorPane::tint` remembers which action plate the pointer
        was on, `Input::Move` answers `stale` on change — and it is handled
        *ahead of* the `under_pointer` gate, because the layout's own tint
        predicate (`action_at` on the raw cursor) is not z-gated and a
        memory that disagreed with the layout would ask for no frame while
        the picture changed.

      **One ordering changed, the same one every step changes.** A press on a
      doll's buttons and a release over them are answered by the pane, ahead
      of the container furniture and of `release_container_item`. Nothing can
      be held by both sides: a press only reaches a pane while the hand is
      empty (the manager's gate, S1), and the pane declines the one press
      that would fill it — so `held` and an item transaction cannot be live
      at the same time, which is S2's argument with one new case.
- [x] **S6. Container.** ✅ `panes/container.rs`: `ContainerPane { container,
      pressed, hovered, last_click }`, an `impl Pane` with all three methods,
      and the last window kind out of `App` — `press_on_own_window`'s container
      arm, `container_item_under_pointer`, `hover_container_item`,
      `paperdoll_item_under_pointer`, `stack_all_button_under_pointer`,
      `take_all_button_under_pointer`, `take_all_from_container`,
      `release_container_item`, `release_container_press`,
      `drag_container_item`, `finish_stack_split` and `split_amount` all gone,
      along with the container's layout arm, its art loop and its label arm in
      `render_passes.rs`, and `Link::drop_into`/`Link::equip`. The paperdoll's
      declined press came in with it, and so did the drop onto a doll.
      **Seven things landed differently from the shape above:**
      - **🚨 `ItemDragTransaction` is `Hand`, and the third state left for the
        panes.** D7 said the press is the pane's and the hand is the manager's;
        the *type* said all three were one machine, and `owns_cursor` existed
        to tell them apart. `Hand` is `Held | Dropped` — everything in it owns
        the cursor by being there, so the manager's gate is the field being
        `Some` — and the press is `ContainerPane::pressed`,
        `PaperdollPane::pressed`, or **`Windows::world_press`** for an item
        lying on the ground, which has no window to keep it in. Three holders,
        one rule for what a press becomes (`ItemPress::dragged` →
        `Still | Ask | Lift`), because a policy restated per holder is the
        second policy this plan keeps meeting.
      - **`Effect::Lift` and `Effect::Drop`, and they are not the pair D7
        refused.** What D7 refused is a transfer *transaction* spanning two
        windows, and there is none: a lift and a drop are unrelated effects,
        possibly from two panes, possibly with a walk between them. What each
        one *is* is a wire half and a mirror half **fused**, for
        `Effect::Answer`'s reason — a hand filled with no packet behind it is
        an item this end has taken out of a bag nobody else knows about, and a
        packet with no hand behind it draws the item back in the bag it has
        left. A pane names only the destination, so `PendingDrop` names its own
        packet and `Link` lost `drop_into` and `equip`.
      - **A bare `Net(PickUp)` still exists, and the difference is the
        cursor.** The wield a double click on a katana is, and the "Take all"
        sweep, are lift-and-drop pairs the player never *holds* — there is no
        moment at which anything is on the cursor — so both are ordinary
        `Effect::Net`s and neither touches the hand. That is exactly how they
        behaved before; the distinction is now written down where the two
        effects are declared.
      - **Who a modal's answer is addressed to — the Backlog entry, settled.**
        `Windows::prompt: Option<Asking>` says whose press the amount picker is
        standing over (`Asking::World` or `Asking::Window(subject)`), and
        `Input::Answered(Answer)` is routed by it, exactly as S4 routes
        `Input::Key` by `Windows::keyboard`. The pane's half is
        `PaneFrame::has_prompt`, beside `has_keyboard`: it reads it to leave
        its own press alone — a move must not lift the pile out from under the
        number being chosen, and the release that *opened* the prompt must not
        put the press down. `split_pending` was a `bool`, which is what
        "whoever is on top" looks like when there has only ever been one
        presser.
      - **`Drawn::Container` carries the list its pictures were built from.**
        A click used to be turned into an item by picking an index out of the
        pictures and counting that far into a list rebuilt from the view — with
        the lifted icon filtered out *again*, by hand, in the order the layout
        had filtered it. Two walks with one subtraction each, and nothing but
        care keeping them in step. `container::Window { pictures, contents,
        lines }` makes the hit test a lookup, which is `drawn_windows`' own rule
        stated properly rather than obeyed twice.
      - **The two plates are one predicate now** — the Backlog entry about
        `stack_all_button_under_pointer` and `take_all_button_under_pointer`,
        answered. Both walked every window and asked `window_under_pointer()`
        *inside* the loop; what that was reaching for is the router's own rule,
        that a press stops at the window it landed on, so a pane hit-tests
        itself and the walk is gone rather than restated. Which plate a window
        has is `plate_of(container, backpack, shop)`, read by the layout that
        draws the caption *and* by the press that acts on it — where a text pass
        and two `App` walks used to have to agree, and a window that drew "Take
        all" and answered nothing would have looked broken while saying nothing.
      - **A plate press raises its window, and it did not before.** Every other
        press on a window's furniture raises; the two plates returned early,
        ahead of `raise_window`. One line, and it is the only thing on this list
        a player can see.

      **Two orderings changed.** A press on an icon and a release over a bag are
      answered by the pane, ahead of the container furniture — which is now the
      pane's too — and a drop onto a doll is answered by `PaperdollPane`, where
      `release_container_item`'s `match` on the window kind used to answer for
      every kind at once. What is left of that function is the ground: a release
      that no window claimed. Releasing over a shop or a skill sheet still drops
      on the ground behind it, because neither pane answers a drop and neither
      is a place to put anything.
- [x] **S7. Delete the branches.** ✅ *Most of this step had already happened as
      its kinds moved.* `hover_container_item` and every `WindowSubject` match
      inside the old handlers were gone with S6, along with
      `scroll_vendor`/`scroll_skills` (S1, S2),
      `release_on_own_window`/`hover_paperdoll_item` (S5) and
      `release_container_item`'s match on the window kind (S6). `App` did not
      know what any of the six was.

      **What was actually left was a rung, not a branch, and it has one now.**
      `legacy_window_input` is `App::fallback_gestures` — the router's third
      rung, named for what it is: the press that picks a window up when no pane
      wanted it (`press_on_own_window`, the raise-and-grab tail), and the
      world's own press and drop (`press_world_item`/`drag_world_item`,
      `drop_hand_on_ground`), which no pane can answer for because the ground is
      not a window. Neither moved *where* it runs — both still run **behind**
      the panes, because a shop's Confirm and a sheet's thumb are asked first —
      only what it is called changed, and the word "legacy" is gone from the
      client.

      **The other half of the step, folded in: `window_under_pointer` asked
      once.** The Backlog entry "worked out once per move and twice per press"
      is closed. `App::deliver` asks `window_under_pointer` a single time per
      input and hands the answer down as `owner` to `manager_gestures`,
      `offer_to_panes` and `fallback_gestures` — none of which asks again.
      `press_on_own_window` and `close_window_under_pointer` take `owner` as a
      parameter instead of calling the walk themselves; the texel pick against
      every window's last frame that a press used to pay for up to three times
      now runs once.
- [x] **S8. The test the wheel defect would have failed.** ✅ `PaneFrame` no
      longer carries `&Resources`. It carries **`panes::PaneFiles`** — the nine
      borrows a window actually reads (`gump_atlas`, `font_atlas`, `art`,
      `tiledata`, `gumps`, `cliloc`, `equip_conv`, `skill_names`,
      `skill_groups`) — and every one of them can be built from nothing, so a
      `PaneCtx` no longer needs a client install. `panes/fixture.rs` is that
      context on a stack: `Install::shipping` packs the atlas from solid blocks
      the caller names, `FontAtlas::pack([])`, `TileData::empty`, the new
      `Art::empty`, `Default` for the three tables, and `None` for the two an
      install may legitimately not ship.
      **The assertion the step is named for now runs through `handle`**, in
      `panes/vendor.rs`: a nine-row catalogue scrolled to its end, one more
      notch, `taken` and `!redraw` — with the layout the notch is answered
      against produced by the pane's *own* `layout`, so the picture the test
      hit-tests is the picture the player would be pointing at.
      **Four things landed differently from the shape above:**
      - **The blocker was one field, not the trait.** "`App` needs asset files"
        narrowed to "`Resources` needs asset files" at S6 and to
        "`PaneCtx::frame::resources` needs asset files" here. What made it a
        field rather than a design is that a pane never wanted `Resources`: it
        wanted nine things out of it, and the other twenty — the facet, the
        navigation graph, the 195MB `anim.mul` — were in reach only because
        nobody had said they were not. `PaneFiles` is that sentence as a type,
        and it is now a decision to widen rather than a field already there.
      - **`Art::empty` and `Uop::empty` are new, and they are `TileData::empty`'s
        idiom rather than a stub.** A `Uop` with no entries answers `Ok(None)`
        for every name, which is exactly what a real container answers for an
        index it does not ship — so nothing here guesses at what a file says.
        The doc on each says so, and points at the real-install test that is
        where an assertion about real art belongs.
      - **`PaneFiles::of` is called twice in `render_passes.rs`, once per
        builder, and that is decision 6 showing through the borrow checker.**
        It borrows `resources`, and the packing sweep between the `art` loop and
        the `layout` loop grows the atlas — which is the *reason* the two loops
        are separate. One `PaneFiles` spanning both would not compile, and the
        thing it would not compile *for* is the invariant.
      - **The second assertion is the one no call to `wheel` can make.** A notch
        offered to a window the pointer is not on is not that window's: deleting
        the `under_pointer` gate from `VendorPane::handle` reddens exactly that
        one test and leaves the other twelve green. The rule's own tests could
        not see it, which is what "through `handle`" buys. The conflation
        mutation — `wheel`'s `consumed` back to `ignored` — reddens the
        through-`handle` test beside the three rule tests, so the new test
        covers the old defect as well as the plumbing.

      The wheel is asserted through `handle` for **two** kinds, the two the
      defect was about. The other four have a context they can be tested with
      and no through-`handle` test yet — a Backlog entry, not a step.

## Backlog

- ~~**Window-local coordinates.** D2 keeps absolute gump pixels so this plan does
  not reach into the render crate. Every pane hit-test then begins by
  subtracting `ctx.at`, which is a line that can be forgotten — and a pane that
  forgets it hit-tests against the top-left of the screen, which looks like
  "the window is dead" from the outside. Worth doing after S7, when there are
  six panes to convert at once and one place that measures a cursor.~~
  **Closed, all six kinds at once, as this entry asked.** The render crate's
  window-layout functions (`vendor::buy`/`sell`, `container::window*`,
  `skills::window`, `status::window`, `paperdoll::window`/`name`, and the
  dialog layout, `gump_art::window`) **keep their `at: GumpPixel` parameter**
  rather than losing it — the smaller of the two mechanical shapes this entry
  left open, chosen because dropping the parameter everywhere would have
  meant rewriting every internal `.offset(at)` call in six files *and* the
  expected values in every one of their existing unit tests, for a shape those
  tests already exercise correctly at a nonzero `at`. Every pane now calls
  them with `GumpPixel::new(0, 0)` always (`PaneCtx`/`PaneFrame` in
  `panes.rs`), so every `Picture`, `Line` and `Scissor` a pane's `layout`
  produces is window-local by construction, and `PaneFrame::at` — which had
  no reader left once every call site was zeroed — is gone rather than kept
  as a field nothing reads.

  **The "one place that measures a cursor" is two, one per direction**, and
  each is named where it lives: `render_passes.rs`'s `draw_gump_windows`
  moves the *art* into screen space, once per window per frame, and
  `own_windows.rs`'s `App::window_under_pointer` moves the *cursor* into each
  candidate window's own space instead, before testing it against that
  window's (already window-local) last-drawn pictures. `panes/route.rs`'s
  `offer_to_panes` and `render_passes.rs`'s two `PaneFrame` builders convert
  the same way before a pane ever sees `ctx.frame.cursor` — that is what
  makes a pane's own hit test (`window.hit(ctx.frame.cursor, ...)`, `Effect::
  Grab(ctx.frame.cursor)`, and so on) need no arithmetic of its own at all,
  which was the whole complaint this entry opened with. A test in `gump.rs`
  (`a_magnified_window_picks_what_it_draws`) pins the identity both
  directions rely on: the pixel the draw pass puts a picture's texel on must
  be the pixel whose local cursor picks that picture.

  **Since: the placement gained a scale, and that made both directions one
  function each.** `desk::WindowScale` is how big every window draws — a
  fractional upscale on the art's own pixels, saved as `window_scale` in
  `client_ui.toml` and turned by the dev window's Windows tab — because the
  reference client has no display scaling at all and its windows are postage
  stamps on a modern screen. A window is therefore *magnified and moved*
  rather than only moved, and the three per-`Picture` movers this entry
  introduced (`Picture::offset`, `Scissor::offset`, `GumpLabel::offset`) are
  **gone**, replaced by one `gump::place(&mut quads, at, magnify)` that the
  draw pass calls on a window's art and on its text alike: art, labels and a
  scissored row all end as `SpriteQuad`s in the same window-local pixels, and
  three ways to place them would be three things to keep in step with the
  pointer. The pointer's own half is `windows::OwnWindow::local_cursor`,
  which subtracts the placement and divides by the same factor, **flooring**:
  a cursor left of a window is a negative local coordinate and a truncating
  cast would round it onto column `0`, inside the picture that starts there,
  and floor is also the rounding a quad's own edge lands on, which is what
  makes the two agree pixel for pixel at a fraction. Anything cropped is cut
  *before* `place`, in the window's own pixels, where the cut is exact. Two
  things deliberately keep the art's own size: the shard's hover tooltip (it
  is drawn over the world as well, and belongs to no window) and the HUD chat
  box (`desk::ChatScale` is its own knob).

  **And it cost a round trip to learn what was already written down**: the
  scale was read from `App::desk`, which is the file as it was *loaded* —
  the copy the slider moves is `Shell::desk`, and the two meet only at exit.
  The knob therefore did nothing at all, which reads from the outside as "the
  window layer does not scale" rather than as "the number never arrived".
  `Shell::tuning`'s own doc had said so about the lighting, in those words.
  It is `App::window_scale` now: the shell's copy while there is a shell, the
  app's only for a run that has none.
- ~~**The vendor window and the skill window disagree about what a wheel over a
  window means.** Skills claims its whole frame; the vendor claims only its
  catalogue viewport (`catalogue_contains`), so a notch over the shop's buttons
  still reaches the camera. Both are panes now (S1, S2), so this is a per-pane
  decision that is *visible* as a decision — two `handle` arms, four lines
  apart in shape — but that does not settle it: somebody has to say which is
  right. ClassicUO claims the whole window.~~ **Closed: the whole frame is
  every window's, matching ClassicUO.** `VendorPane::handle` (`vendor.rs`) no
  longer gates `taken` on `catalogue_contains` — that predicate now only
  decides whether the catalogue itself has a row to move
  (`VendorPane::wheel_over_window`), the same split `SkillsPane` already made.
  A notch over a button or a plate is `Response::consumed` and never falls
  through to the camera.
- ~~**`if let Some(window) = self.window.as_ref() { window.window.request_redraw() }`,
  twenty times.**~~ **Closed by `App::ask_redraw()`** (`window.rs`, beside
  `create_window`): `if let Some(window) = self.window.as_ref() {
  window.window.request_redraw(); }` and its one `as_mut()` cousin in the
  `Resized` arm both collapse to `self.ask_redraw();`. Twenty call sites in
  `event_loop.rs`, one method.
- ~~**`stack_all_button_under_pointer` and `take_all_button_under_pointer` walk
  the window list and then ask `window_under_pointer()` inside the loop.**~~
  **Closed by S6.** What that predicate was reaching for is the router's own
  rule — a located input stops at the window it landed on — so both walks are
  gone rather than restated, and a pane hit-tests itself. The plate a window has
  is one predicate (`plate_of`) read by the layout that draws its caption and by
  the press that acts on it; there were three readers and they had to agree.
  *One shape worth keeping from how it landed:* the plates hang **below** the
  window's own art, so a pane is offered that press with `under_pointer` false —
  which is the same fact the old walks were computing, arrived at by the router
  rather than by asking a second time.
- ~~**Who a modal's answer is addressed to.**~~ **Closed by S6, the way the
  entry itself proposed.** `Windows::prompt: Option<Asking>` records whose press
  the amount picker went up over — `Asking::World` for an item on the ground,
  `Asking::Window(subject)` for a bag — and `Input::Answered(Answer)` is routed
  by it, by identity and never by z-order. It is S4's keyboard split one device
  over: the manager owns *which* presser the answer belongs to, the presser owns
  what it means, and the pane reads its half as `PaneFrame::has_prompt` beside
  `has_keyboard`. A second client-side modal adds an `Answer` arm and nothing
  else.

  *What this cost, and it is the shape to expect next time:* the answer arrives
  from the shell rather than from the event loop, so `App::apply` delivers it —
  an input is an input, and this one is addressed — and the record is cleared
  **after** the walk, because the walk is what reads it to find the addressee.
- ~~**The window under the pointer is worked out once per move and twice per
  press.**~~ **Closed by S7.** `App::deliver` asks `window_under_pointer` once
  per input and hands the answer down as `owner` — to `manager_gestures`'s
  keyboard release (S4), to `offer_to_panes`'s walk (which used to ask again
  itself), and to `fallback_gestures`'s `press_on_own_window` and
  `close_window_under_pointer`, both of which now take `owner` as a parameter
  instead of calling the walk. One texel pick against the window list per
  event, not up to three.
- ~~**`close_window`'s dialog arm answers the same `None` to two questions.**~~
  **Closed by splitting the `and_then` chain into two steps.** The `find` and
  the `dismiss` call are no longer one expression: a missing window now returns
  `false` of its own accord, with a comment saying why — there is nothing here
  for the press to have taken, so it did not — and only a *found* dialog whose
  pane answers `None` still returns `true`, unchanged, because that is `{
  noclose }` and the press really was the window's. The no-such-window arm is
  unreachable today (both callers pass a subject that came from a real window),
  and the split does not change that; it changes the function to say so on its
  own instead of depending on it. `AnyPane::of` still makes a `Dialog` subject
  holding some other pane impossible, so that sub-case stays dead code inside
  the `match`, documented in place rather than folded away.
- ~~**A vendor's ACCEPT and CLEAR tint on hover, and nothing asks for a frame
  when it changes.**~~ **Closed by S5, in the pairing the entry itself asked
  for.** `VendorPane::tint` remembers which plate the pointer was on and
  `Input::Move` answers `Response::stale` when that changes; the paperdoll's
  hover moved into its pane the same way (`hovered`, `hand_over`), and
  `App::hover_paperdoll_item` is gone. One shape worth keeping from how it
  landed: the pane's memory uses the *layout's own* predicate
  (`vendor::Window::action_at` on the raw cursor, no z-gate), because a
  memory computed by a stricter rule than the picture's would ask for no
  frame while the picture changed.
- ~~**The press that picks a window up is the manager's, and it lives in the
  legacy chain's tail.**~~ **Closed by S7.** `App::fallback_gestures` is the
  router's fourth rung, exactly as this entry proposed: the manager's own
  gestures that are a *fallback* rather than a precondition, reached only after
  every pane has declined. `press_on_own_window` (the raise-and-grab tail) and
  the world's own press and drop (`press_world_item`/`drag_world_item`,
  `drop_hand_on_ground`/`release_world_press`) all live there now, unmoved from
  where they ran — the fix was the name, not the order.

  One wart the entry named is still there and was not this step's to fix: that
  tail reads the grab offset off `own_windows.last()` rather than off the
  window it is picking up, right only because `raise_window` has just moved
  that window to the end.
- ~~**A press on a bag over a window over another bag is offered a plate it
  cannot see.** The plates hang below a window's own art, so a pane is offered
  their press with `under_pointer` false — and the walk only stops *after* the
  window the pointer is on, so a plate belonging to a window **above** that one
  still answers even when a window is drawn over the plate itself. That is
  exactly what the two old walks did (they tested the plate before bailing at
  the pointed-at window), so S6 preserved it deliberately; it is worth deciding
  whether it is right rather than inherited. The honest fix is for a plate to be
  part of the window's picture — pixels rather than a box — which is also what
  would let it tint on hover like every other control this client draws.~~
  **Closed by giving both actions pixels — the honest fix this entry itself
  named.** `docs/findings.md` had already ruled out a synthetic quad tight to
  the old 72×18/80×18 box. The first attempt reused `skills.rs`'s
  `TOTAL_PLATE` (`Graphic(0x0836)`, 210×19) as a generously-sized background,
  on the strength of that constant's name and of the finding's description of
  its size — and **that art is a picture of a sentence**, ClassicUO's
  `_bottomComment`, so every bag drew "Left-click the button before a skill to
  use the skill." under itself in purple. See `findings.md`, which now records
  both the art's real identity and the resolution: there is no plate, there is
  a **button** — `container::ACTION_UP`/`ACTION_DOWN` (`0x0FA5`/`0x0FA7`,
  30×22, the generic `4005`/`4007` pair), with the caption beside it rather
  than over it (`ActionButton::label_at`). `ActionButton`'s size comes from
  the atlas the same way the window's own background size does, rather than
  from the two hardcoded numbers this entry described.

  The button is drawn as a real `Picture` in `Window::pictures`
  (`container::window_with_action`), pushed by `ContainerPane::layout` at the
  same position `press_action`'s box test uses — one `action_button` resolves
  both, and the layout, the press and the hover face all read it rather than
  recomputing it three ways. Once the button has pixels, it is exactly as
  pickable and exactly as occludable as the background and every icon beside
  it: `window_under_pointer`'s existing per-pixel walk resolves the covering
  case by construction, and `ContainerPane::handle`'s press arm no longer
  branches on `!under_pointer` to reach the plate — reaching the pane at all
  now means this window owns the pixel, plate included, and `press` decides
  which part of its own art the press landed on. `press_plate`'s own box test
  stays, exactly as this entry allowed: it now only ever runs for a window
  the pixel-pick has already confirmed owns that pixel.

  The button also answers the pointer now: `ContainerPane::action_hovered`, a
  bit beside `hovered` rather than folded into it (an icon and the button can
  never be true at once, and are two different pictures), resolved through
  `container::action_face` to the pressed art — **not** through
  `HIGHLIGHT_HUE`, which goes back to meaning only "the icon under the
  cursor". Same `Response::stale`-on-change shape `ContainerPane::hover` was
  already written in. Both faces are packed by `ContainerPane::art` whenever
  the window has an action at all, so arriving on the button is never the
  frame the atlas first hears of the pressed picture.
- ~~**`Windows::world_press` is in the window layer because the hand is, and it
  is not about a window.** An item lying on the ground is pressed exactly the
  way an icon in a bag is — same type, same rule for what the press becomes —
  and the manager holds it because the world has no pane. That is honest, and
  the field is still the one thing in `Windows` that is not about a window. It
  belongs beside the picking state, or in a `Hand`-shaped module of its own with
  the two effects and `ItemPress::dragged`; worth deciding when S7 gives the
  world's gestures their rung, since the two questions are the same one.~~
  **Closed by a `Hand`-shaped module of its own, `hand.rs`.** `ItemPress`,
  `DRAG_SLOP`, `centre_of`, `Dragged`, `DragOrigin`, `PendingDrop`, `ItemDrag`
  and `Hand` — the whole rule for what a press becomes, wherever it landed —
  moved out of `windows.rs` into `crate::hand`, along with the tests that
  pinned them. `Windows::world_press` **stays a field of `Windows`**, and that
  is deliberate rather than left behind: it is one of three exclusive devices
  the manager tracks (beside `hand` and `grip`), and moving the *registry*
  would have been the bigger, unrequested change this entry never asked for —
  only the *type* `world_press` holds was window-shaped by accident, and that
  is what moved.
- ~~**A bag rebuilds its icon list two or three times a frame.** `contents()`
  filters the view's list and projects a pending drop into it, and it is asked
  by `art`, by `layout` and by a press on the sweep plate. Each is a `Vec` of
  what is in the bag — small, and the same allocation the old layout made — but
  it is a list computed from the same three inputs three times. Whether a pane
  wants a once-a-frame scratch is a question for every kind and not just this
  one, so it is not a container fix.~~ **Closed for `art`/`layout`, locally, by
  `ContainerPane::scratch`.** The two calls that are provably paired — every
  redraw asks a pane's `art` and then its `layout`, back to back in
  `render_passes.rs`, off the same `view` and the same `hand`, with no packet
  fold or input event between them — now share one computation: `art` leaves
  what it computed in a `RefCell` and `layout` takes it with
  `ContainerPane::recall_contents`, unconditionally emptying the cell so a
  `layout` that ever runs without a matching `art` first recomputes rather
  than reading a stranger's leftovers. `contents()` itself moved to take
  `(view, hand)` instead of a whole `PaneFrame`, which is what let the cache be
  pinned by a test without the `&Resources` that blocks S8.

  The sweep plate's own call is **deliberately not folded in**: a press runs
  off the input event pump, not the redraw clock, so there is no gap between a
  redraw and the press that follows it for a cache to sit in safely — reading
  one there would be trusting a value that may already be older than the last
  packet. It still asks `contents()` fresh, as it always has.

  The broader question this entry's last sentence raised — whether a pane
  wants a once-a-frame scratch as a thing `trait Pane` itself offers, for
  every kind and not just this one — **stays open**. This closes only the
  container's own case, by a mechanism private to `ContainerPane`.
- ~~**A scroll that could not move still asks for a frame.** `SkillsPane::wheel`
  answers `consumed` at either end, and the arrows and the track beside it
  answer `changed` unconditionally: pressing Up at the top of the list is a
  redraw of a picture that has not changed. The wheel is the one that mattered
  — it is the one whose answer decides whether the camera hears the event — and
  the buttons are the same conflation with nothing riding on it. One line each,
  and the shape is already there to copy.~~ **Closed by copying the wheel's own
  shape.** `SkillsPane::clicked`'s `Hit::Up`, `Hit::Down` and `Hit::Track` arms
  now read the offset before and after `scroll_by`/`scroll_to` and answer
  `Response::consumed()` when it did not move, `Response::changed()` when it
  did — the same `before`/`after` comparison `wheel` already made. Nothing
  rode on it: these presses never reached the camera, so the only effect was
  the extra frame.
- ~~**`Drawn` is produced by a pane but consumed by passes that know all six
  kinds — and there are two of them.**~~ **Closed by S4, and the entry was
  wrong about how.** It said the two walks "already disagree", reading the
  vendor's empty arm in `presentation.rs` against its filled arm in
  `render_passes.rs`. The truth was worse and cheaper: `presentation.rs`'s walk
  iterated `std::iter::empty::<(&WindowSubject, &Drawn)>()`, and had since
  window text moved next to its own art — because one global text pass let a
  lower catalogue's lines cover a later paperdoll. So every arm in it was
  unreachable, including a dialog arm that reached into `Dialogs` for a text
  table. S4 deleted the walk. There is one place per-kind text is turned into
  labels, and a seventh window kind costs one branch.

  *What the entry was right about stands and is worth keeping in mind:* a frame
  assembled in more than one place makes agreement a coincidence
  (`docs/parity.md`), and **dead code in the shape of a live branch is the same
  defect wearing a disguise** — it reads as a second assembler, it type-checks
  like one, and the next author keeps it in step for nothing.

- ~~**Four kinds have a context and no test through it.**~~ **Closed.** Four
  tests now go through `Pane::handle`/`Pane::layout` against
  `panes::fixture::Install`, the vendor's own shape: a container's icon becomes
  a lift on the move past the drag slop
  (`a_press_on_an_icon_becomes_a_lift_through_handle`, `container.rs`); a
  paperdoll's own worn item does the same
  (`a_press_on_our_own_worn_item_becomes_a_lift_through_handle`, `paperdoll.rs`
  — S5's "declined press" is superseded reading: S6 folded that machinery in,
  so a press on your own worn item is taken and lifts directly, exactly like a
  bag); a dialog's caret is drawn only while `PaneFrame::has_keyboard` is true
  (`the_caret_is_drawn_only_while_this_window_holds_the_keyboard`, `dialog.rs`
  — `Input::Key` itself is routed a level up, in `route.rs`'s
  `offer_to_panes`, addressed by `Windows::keyboard` before a pane is ever
  asked; the one place `DialogPane` reads `has_keyboard` at all is deciding
  whether the focused field draws a caret); and the status frame's `None`
  layout, plus a press with no layout behind it staying `ignored`
  (`a_frame_with_no_status_reply_yet_lays_out_nothing`,
  `a_press_with_no_layout_behind_it_is_still_ignored`, `status.rs`).
- **Whether `trait Pane` should offer a once-a-frame scratch.** Carried over
  from the container's `contents()` entry, whose last sentence raised it and
  which closed only the container's own case with a private `RefCell`. Every
  kind recomputes something between `art` and `layout`; whether that pairing is
  a thing the trait states — the way decision 6 states the *order* — or a thing
  each pane arranges for itself is undecided, and the container is the only kind
  that has needed it so far.

### Found while making the amount picker a gump (2026-08-17)

The picker is the seventh kind — `panes::split`, `render/src/split.rs`, see
`docs/client.md`'s section of that name. Four things it surfaced and did not
settle:

- **A subject carries a value now.** `WindowSubject::Split { item, most }` is the
  first identity with something in it that is not a key: `most` is measured from
  the pile at the moment of the press and is deliberately never re-read, so it
  travels with the subject to reach `AnyPane::of`. Every other kind's pane looks
  its subject up in the view instead. If a second window ever wants that shape,
  the honest fix is probably for `AnyPane::of` to take the subject *and* what the
  window was opened with, rather than for identities to keep growing fields.
- **The picker is placed and every other window is cascaded.** It opens at
  `pointer - (80, 40)` and is clamped only against negative corners: nothing at
  that layer knows how wide the surface is, so a prompt raised near the right or
  bottom edge hangs off it. The surface size would have to reach the manager for
  that to be fixable, and the same fact would fix the cascade running windows off
  the screen — one backlog entry, two symptoms.
- **The wheel entry above is now half answered.** "Which windows claim a wheel
  they have no use for" was written when none had a use; the picker does — a
  notch steps the number, as `HSliderBar` does — so the question is only about
  the kinds that claim one and ignore it.
- **`shell::Request` is shrinking.** The split field is gone from it, and what is
  left is party and diagnostics. When the last window-shaped member goes, `apply`
  is a frame-late door with nothing window-shaped left to carry.
- **What `WindowScale` does *not* reach, and each is a decision somebody should
  make rather than a gap.** The cascade constants (`CONTAINER_ORIGIN`,
  `CONTAINER_CASCADE`, `windows.rs`) are screen placement and are left
  unmagnified, so at three times the art two cascaded bags overlap by far more
  of each other than the 24-pixel step was chosen to leave — and the eighth
  window runs off a small screen sooner, which is the entry above's second
  symptom arriving earlier. `SPLIT_OFFSET` *is* magnified, because it is half
  the picker's own art rather than a screen distance, and the two being
  different kinds of constant in the same file is the thing to notice. The
  shard's hover tooltip and the HUD chat box keep the art's own size on
  purpose (the first is drawn over the world too; the second has
  `desk::ChatScale`), which at three times a window is a legible bag with an
  illegible tooltip beside it.
- **The diagnostic tools do not know the scale.** `tests/gumpshot.rs` and
  everything else in `docs/parity.md`'s list assemble a frame by hand and place
  windows themselves, so a tool's picture is the client's only at
  `WindowScale::MIN`. That is one more caller of the placement that is not
  `gump::place` — the shape `parity.md` exists to complain about.
- ~~**Two frames of reference in one field: the window that teleports on a
  click.**~~ **Closed (2026-08-22), and it was the scale entry above growing
  teeth.** `Windows::dragging` was `Option<(WindowSubject, GumpPixel)>` — the
  window and *where inside it* the player grabbed it — and `drag_own_window`
  placed the window at `pointer - offset`. That arithmetic is right only if the
  offset is in the same pixels as the pointer, and the two rungs that started a
  drag disagreed about which those were: `press_on_own_window` measured
  `pointer - window.at` (absolute surface pixels), while **every pane** answered
  a press it had no use for with `Effect::Grab(ctx.frame.cursor)` — window-local,
  which is `at` subtracted *and* divided by `WindowScale::factor`. Above the
  art's own size, therefore, a press anywhere on a paperdoll, a bag, a shop, a
  sheet or a dialog moved the window by `cursor * (factor - 1)` on the first
  pointer movement after the click, by an amount that depended on where in the
  frame it was clicked and with no mouse movement to account for it. The client
  ships with `window_scale` on a knob, so this was every player above 1.0.

  It is `Windows::grip: WindowGrip` now — `Idle | Held(WindowHold)`, with
  `press` / `follow` / `release` and no other writer — and **a drag is a delta**:
  the press freezes the pointer *and* the window's corner, both absolute, and
  every move places the window at its frozen corner plus how far the pointer has
  travelled. There is no space left to measure the offset in, so there is no
  space to measure it in wrongly; the scale never enters the arithmetic, which
  also makes the knob safe to turn mid-drag. `Effect::Grab` carries **nothing**:
  a pane cannot say where a press landed in the manager's pixels, and it does
  not have to — the manager reads the same pointer field on the press that it
  will read on the move. `App::grab_window` is the one door both rungs go
  through.

  **Two more ways the hold outlived the gesture, found while writing the
  machine and closed with it.** A left release that egui claims never reaches
  `panes::route` (`Shell::on_window_event` returns before the `match`), so a
  window let go over a panel stayed held and resumed following the pointer with
  the button up; `window_event`'s consumed branch releases the grip now, beside
  the keys it already let go of for the same reason. And `Focused(false)` did
  the same for an alt-tab mid-drag — the release happens in another
  application — so it releases the grip too. Five cases in
  `windows.rs`'s `grip_tests`, one of which asserts its own fixture is a scale
  the old convention got wrong.
- **`DRAG_SLOP` is measured in whichever pixels its holder counts in, and the
  three holders do not count in the same ones.** Found while writing the grip
  above, and *not* folded into it: it is the item press, not the window drag.
  `ItemPress::at` is stored in `PaneFrame::cursor` by a bag's pane and a doll's
  pane — window-local, so divided by `WindowScale::factor` — and in absolute
  surface pixels by the manager for `world_press`. `ItemPress::dragged` compares
  each against a cursor from the same space, so neither is *wrong*; what differs
  is how far the player's hand has to travel before a press becomes a lift. At
  three times the art an icon in a bag lifts after `DRAG_SLOP * 3` real pixels
  and one on the ground after `DRAG_SLOP`. Whether the slop is a distance on the
  screen or a distance in the art is a decision nobody has made — the same
  question `SPLIT_OFFSET` and the cascade constants answer in opposite
  directions two entries up.

## Status

**S0 through S8 built** (2026-08-17) — **the plan is complete.**

**S0 through S7 built** (2026-08-17). The router is real, every input the
window layer sees goes through it, and **all six kinds have moved in**: a shop
owns its scroll position, its chosen quantities and its tinted plate; the
skill sheet owns its tree and the control the mouse is holding; the status
frame owns its layout, which is all it has; a `0xB0` dialog owns its page, its
switches, what has been typed into it, the button the finger is on and the box
the keys are going into; a paperdoll owns the button the finger is on, its
scroll pairs, the worn layer under the pointer, the preview of what the hand
would put there and the press that lifts a worn item off it; and a container
owns its icons, the press on one, the pair that makes two clicks a use, the
tint under the pointer and the plate below the frame. Each owns its art, its
layout and its input, and `App` knows what none of them is — except to close
one, and closing a dialog is asking its own pane what to answer with.

`render_passes.rs` lays out **no window kind at all** any more. It packs what
the panes ask for, draws what they laid out, and writes the lines they
resolved.

What this changed for a player, in one line each. **The wheel** (S0): `taken`
rather than "did anything move" decides whether the camera hears a notch — the
defect this plan grew out of, stated as a type instead of as a convention.
**A shop that is in both catalogues** (S1): what is drawn and what Confirm sends
are now the same list; they were not. **Nothing at all** (S2): the skill sheet
behaves as it did, minus a frame it used to ask for at the end of its list.
**Nothing at all** (S3): the status frame opens on the press rather than on the
frame after it, which is a cascade order and not a picture.
**Nothing at all** (S4), with one thing that is now true by construction: a
dialog's answer names every field the shard declared, where it used to name
every field a sweep had copied into a map.
**The shop's two plates tint on the move that crosses them** (S5): the hover
used to be right only when something else — the animation clock, another
window — happened to draw a frame; a move that changes a tint asks for one
now, on the shop and on the doll alike.
**A bag's plate raises its window** (S6): pressing "Take all" or "Stack all"
puts that bag on top, which every other press on a window's furniture already
did.
**Nothing at all** (S7): a rename and a single `window_under_pointer` ask per
event, where up to three used to run.

The `||` chain the plan was written against is **gone**, and so is the last
window kind behind it. `App::fallback_gestures` — `legacy_window_input`,
renamed — answers no window at all now: its wheel and key arms are empty, its
press arm is the raise-and-grab tail, and its release and move arms are the
world's — an item on the ground has no pane to keep its press in. The word
"legacy" is gone from the client, and `App::window_under_pointer` is asked once
per input in `App::deliver` and handed down as `owner`, rather than asked again
by each of the three rungs that need it.

**No window's openness is kept outside the list of open windows.**
`Windows::skills` went with S2 and `Windows::status` with S3, and
`reconcile_own_windows` takes the view, the list and the overlay. The two kinds
the view cannot answer for say so with `WindowSubject::is_local()`.

**And no window's state is kept on `Windows` at all.** `Windows::dialogs` was
the last of the maps (S4), `held_doll` and `last_scroll` the last of the
keyed-by-subject fields (S5), and S6 took the container's four —
`hovered_container_item`, `last_container_click`, the `Pressed` half of
`item_drag`, and `split_pending`. What is left is what is true of the *layer*:
which windows exist, in what order, where each sits, what the last frame drew,
and who holds each of the three things there is one of on a screen — the
pointer (`grip`), the keyboard (`keyboard`) and the cursor (`hand`, with
`prompt` for the press a modal is standing over). Plus one that is not about a
window at all and says so: `world_press`, the press on an item lying on the
ground.

The `#[expect(dead_code)]` checklist is **empty**. `PaneCtx::modifiers` is read
by the bag's Shift-split, which was the last entry on it.

**S8 closed the last of it, and what it deleted was a field.** A `PaneCtx` used
to carry the whole of `Resources` — the facet, the navigation graph, the 195MB
animation file — so a pane could only be asked a question on a machine with a
client install on it. It carries `PaneFiles` now: nine borrows, every one of
them buildable from nothing, which is what makes `panes/fixture.rs` a stack
value and `Pane::handle` reachable from a test. The wheel defect is asserted
through the front door for the two kinds it was about, and the gate in front of
the rule — a notch on a window the pointer is not on — is asserted for the first
time at all, because no call to `wheel` could ever have seen it.

**The plan is done.** What is left is in the Backlog and is a list of tests that
can now be written, plus one open question about whether the trait should say
anything about a per-frame scratch.
