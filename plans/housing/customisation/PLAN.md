# The house-design editor

A designed house exists on this shard: its shape is a component on the entity,
both design packets cross the wire, a foundation can be bought and stood in, and
`.hdesign` copies one multi's components onto a house. An owner can open the
editor over their own foundation, move every wall in it, and **commit** — so a
shape a player made five minutes ago is now a shape that stands, blocks, saves
and draws for everybody who walks past it. That was step 3, and it is the whole
of what C7's held-back working copy was for: one commit, one swap.

What is left is the shape's *edges* rather than the mechanism — roofs, a second
working copy to fall back to, and the validation that says a design is a
building.

The model, the packets and the commit rule are
[`docs/housing/design_customisation.md`](../../../docs/housing/design_customisation.md);
what those two phases found is
[`docs/housing/evidence/2026-08-24-the-design-phases.md`](../../../docs/housing/evidence/2026-08-24-the-design-phases.md);
what is built across the whole domain is
[`docs/housing/README.md`](../../../docs/housing/README.md). This page is only
what is not built, and the order to take it in.

## What makes this tractable, and it is already decided

Three decisions are load-bearing here and none of them is open:

- **The working design touches nothing.** While a session is open the world still
  shows and blocks the *committed* design, so there is no incremental obstruction
  churn, no partial design on the wire, and no question about what a stranger
  outside sees. One commit, one swap.
- **The commit tail is six steps** and the fifth is the one that gets forgotten —
  validate, replace and bump, unblock the old shape and block the new, re-run
  `adopt_doors`, **re-hang the sign**, send the revision.
- **The subcommand set is additive.** `EncodedSubcommand::Other(u16)` is a total
  fallthrough, so nothing already routed changes shape when the design
  subcommands arrive, and the dispatch path is four files deep with
  `QuestGumpRequest` as the worked example. Step 1 walked that path for `0x0C`
  and it cost exactly the four files the estimate said.

## The order

- [x] **1. The session brackets.** `DesignSession` on the house entity, entered
      by the owner through `standing_of`, left on end-customisation — and ended
      by logout, death and `collapse_houses`, because a dangling session on a
      despawned house surfaces as a panic rather than as a missing feature. No
      editing verbs yet: this step is what makes "in a session" a state the shard
      can be asked about.

      Built as `housing/src/session.rs`. The way *in* is the house's own window,
      which is where the reference puts it — `HouseGumpAOS`'s "Customize this
      house", drawn for the owner of a house that has a `HouseDesign` and for
      nobody else. The way *out* is `0xD7 0x0C`, the first design subcommand this
      engine routes, and the bracket the editing client sees either way is
      `0xBF 0x20` (type `0x04` / `0x05`) — sent to that one client, because a
      session is a state of its screen and the world goes on showing the
      committed design to everybody else. The ender is one call per event:
      `session::end_for` from the disconnect and from `become_ghost`, and
      `session::end_over` from `decay::demolish`, which is the one call that
      destroys a house and so covers the clock's collapse and the owner's own
      Demolish button together.

      **Three refusals beyond the plan's own, each with a reason.**
      `NotDesignable` — a classic house's shape is a multi id in every client's
      files and there is nothing on this shard to edit. `AlreadyOpen` — two
      working copies of one house are two commits racing to be the shape.
      `ClientTooOld` — a client below `Feature::CustomMulti` has no editor to
      open *and no way to say it closed one*, so a session opened for it could
      only be ended by a logout.

      **What was deliberately left out**, all three because they are about bodies
      rather than about the session being a state:
      ServUO's `BeginCustomize` teleports the editor onto the foundation, hides
      them, and puts everyone else outside; it also refuses a player who is in
      combat, which this engine has no "recently fighting" notion for — the ghost
      refusal is the half it does have. And nothing sends a `0xD8` at the
      brackets, because until step 2 the working design is a copy of the
      committed one and there is nothing new to draw.
- [x] **2. Build, erase and select-floor**, against the working copy only. The
      hex values come out of the reference at implementation time and are cited
      at the constant.

      Built as `housing/src/editing.rs`, behind three more subcommands —
      `0x06` build, `0x05` erase, `0x12` select-storey, each cited to both
      `HouseFoundation.cs`'s registration *and* ClassicUO's own sender, because
      here the two references agree and that is what makes a hex worth citing.
      The payload was the one thing the plan did not price: a `0xD7` value is
      **type-tagged** (`EncodedReader.ReadInt32` reads a byte, then four), so
      `EncodedCommand` grew an `edit` decoded from the subcommand — `Some`
      exactly for those three, and the two fields therefore cannot disagree.

      **The grid is the foundation's own box, one row deeper, and not the
      working copy's.** The reference gets that for free: its
      `MultiComponentList` allocates a fixed grid and `Add` silently drops
      anything outside it. Recomputing the box from the components — the obvious
      port — would shrink the buildable area as pieces come off, so a player who
      erased a corner could never put one back. `buildable_box` derives the same
      row `initial_foundation` lays the stairs on, asked of one function so the
      two cannot drift.

      **Three rules ported that look like details and are not.** A piece
      replaces only what stands at the same height *in the same sense* — one
      wall for another, a floor and a wall side by side — or a client's repeated
      clicks stack a hundred copies of one wall, all of them on the wire. A
      far-south piece is laid at zero, because that row is the stair strip
      rather than a storey. And erasing the last piece off an interior tile lays
      **dirt** at the first storey's height, or the design has a hole the client
      draws the ground through; the erase looks finished without it.

      **What was deliberately left out.** `Designer_Stairs` (`0x0D`), which lays
      a whole stair multi rather than a tile and is a verb of its own.
      ServUO's `ComponentVerification` — a shipped table of which art ids are
      house pieces at all — which this engine has no copy of; the half that is
      free is the roof flag, and a roof is refused because roofs are step 5.
      And the teleport `Designer_Level` does, for step 1's own reason: it is
      about bodies rather than about the session's state.

      **A refused edit says nothing.** The reference answers one by resending
      the design, and that verb is the synch of step 5; until it exists the
      honest answer is to change nothing, so `EditRefusal` goes to the log and
      not to the player.
- [x] **3. Commit and revert**, which is the six-step tail plus throwing the
      working copy away. This is the first step a player can see the result of,
      and the first that can leave a house in a state nobody wants — so the two
      rules the record already paid for apply: nothing comes down until the new
      shape is legal, and the old walls come out as the *old* shape.

      Built as `housing/src/commit.rs`, behind two more subcommands — `0x04`
      commit and `0x1A` revert, each cited to ServUO's registration and to
      ClassicUO's own sender, and each carrying nothing but the terminator byte.
      **The tail cost nothing**, which is C2's dividend arriving: `redesign` has
      been paying for `.hdesign` since the seam landed, so committing an
      editor's design is that call plus ending the session. The two rules above
      are its, and were already tested as its.

      **The session ends last, and that is the one place this differs from the
      reference.** ServUO removes its `DesignContext` and then sends the new
      shape; here the swap is the thing that can be refused, and a player whose
      design was refused still needs the editor to fix it in — so a refused
      commit leaves the house *and* the session exactly as they were. What a
      client sees is the same two packets in the other order: the design detail
      redraws its editor's plan one packet before the `0xBF 0x20` disposes the
      window.

      **A refused commit is spoken, and it is the only design verb that is.** A
      refused edit stays in the log for step 2's reason; a commit is a button
      the player pressed while watching the house, and silence there reads as a
      shard that stopped answering rather than as a no.

      **Revert answers with the design it reverted to** — ServUO's
      `SendDetailedInfoTo`. The client has been drawing its own copy of every
      edit since the session opened, so nothing else on this shard would tell it
      those edits are gone. The storey the editor is on is deliberately left
      alone: which floor is on screen is a fact about the window, and
      `Designer_Revert` does not touch its `Level` either.

      **What was deliberately left out.** ServUO's `ConfirmCommitGump` and the
      gold behind it: the reference charges per component and owes the player
      the number before they pay it, and this shard puts no price on a house at
      all — so there is nothing to confirm and the commit is the commit. And
      `Designer_Clear` (`0x10`), which empties the working design; it belongs to
      no step of this plan, and is recorded below rather than smuggled in here.
- [ ] **4. The cheap half of validation**, enforced at commit: inside the
      foundation's box, under a component ceiling, storeys within the limit.
- [ ] **5. Roofs, backup and restore** — the roof plane and a second working
      copy. Synch (`0x0E`) belongs here too: step 2 promised it this step by
      name, and it is what turns a refused edit from a log line into an answer.
- [ ] **6. The support-and-reachability half of validation**, deferred by name:
      *is this design structurally coherent* is a graph problem, and a floating
      tower is a cosmetic bug rather than a hole in the shard. It is worth doing
      after somebody has built enough houses to want it.

## Found along the way

None is in this plan's scope; all were noticed while the steps above were built
and are recorded here rather than left to be re-found.

- **The house window's bottom button row already runs off its own frame.**
  `sign.rs`'s `FRAME` is 520 wide, the five storage buttons step from x=20 by 100
  apiece, and `Demolish` is then drawn at **x=520** — on the frame's right edge,
  outside the background it is supposed to sit on. It has been that way since the
  storage row landed and no test looks at a coordinate, so nothing says so.
  *Step 1 predicted step 2 would force this and step 2 did not*: the editor's
  toolbar is drawn by the client, out of its own art, and the shard only ever
  sees the `0xD7`s it produces. So the bug is still there, still unforced, and
  now has nothing scheduled to trip over it — which makes it worth fixing on its
  own rather than waiting for a step that will not come.
- **`state/src/lib.rs` re-exports most components and not all of them.**
  `DesignSession` was deliberately not added to that `pub use components::{…}`
  list, because `style.md` says a type is imported from where it is declared —
  which leaves the list half a rule. Either it is finished or it goes; a list
  that holds `HouseDesign` and not the component beside it is the worst of the
  two, since a reader cannot tell whether an absence is a decision.
- **`Designer_Stairs` (`0x0D`) belongs to no step of this plan.** The six steps
  above name build, erase, select-floor, commit, revert, roofs, backup, restore
  and validation, and the reference registers *ten* verbs. The stair one is the
  gap: it lays a whole stair multi's components rather than one tile, expanding
  `MultiData.GetComponents(itemID)` into the working design, and its erase half
  is already there in ServUO's `DeleteStairs` — the branch `Designer_Delete`
  takes before it removes anything. It is closest in kind to step 5's roofs,
  and it should be adopted by that step rather than left as the one editor
  button that does nothing.
- **The synch verb is now one call away, and step 2's reason for silent
  refusals has quietly expired.** `editing.rs` says a refused edit answers
  nothing *because* the shard has no way to resend the design; step 3 needed
  exactly that call for revert, and it turned out to already exist —
  `WorldState::send_design_detail`, written for the client's own `0xD8` query.
  So `Designer_Sync` (`0x0E`) is a subcommand word, a dispatch arm and one line,
  and the day it lands `EditRefusal` should stop going only to the log. Step 5
  owns it; what is worth recording is that the estimate behind "until that
  exists" is now wrong by an order of magnitude.
- **`Designer_Clear` (`0x10`) belongs to no step of this plan either.** The same
  gap `Designer_Stairs` has, found the same way: C6's role table names *clear*
  — "empties the working design" — and none of the six steps does. It is the
  cheapest verb in the family (the working copy becomes the foundation's own
  platform, which `design::initial_foundation` already derives) and the most
  destructive one a client can send, since it throws away an editing session's
  whole work with no confirmation. It should be adopted deliberately or refused
  deliberately, and either way not by omission.
- **Nothing says which art ids are house pieces.** ServUO gates every build
  against a shipped `ComponentVerification` table; this engine has no copy of it,
  so `build` refuses a roof (the tiledata flag is free) and accepts everything
  else — a player's editor can lay a mountain, a corpse or a shopkeeper's sign
  into their own walls. That is not a hole in the shard, since the design only
  ever draws and blocks, but it is the difference between "a house" and "any
  hundred statics", and it is worth deciding deliberately rather than by
  omission. Step 4 is where a decision about it would sit.

## Not in this plan

- **An editor in our own client.** A designed house *draws* here; a client that
  can edit one is the other half of steps 1–3 and is its own piece of work,
  against `docs/client/`'s window rules rather than these.
- **House resizing and foundation upgrade.** A placement question wearing a
  design costume — it re-asks the five placement rules on a bigger footprint.
- **A design catalogue**, which is `.hdesign` generalised: content plumbing
  rather than a system.
