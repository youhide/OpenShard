# The house-design editor

A designed house exists on this shard: its shape is a component on the entity,
both design packets cross the wire, a foundation can be bought and stood in, and
`.hdesign` copies one multi's components onto a house. **What no player can do is
change one** — every shape so far is either a shipped multi or a staff copy of
one.

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
- [ ] **2. Build, erase and select-floor**, against the working copy only. The
      hex values come out of the reference at implementation time and are cited
      at the constant.
- [ ] **3. Commit and revert**, which is the six-step tail plus throwing the
      working copy away. This is the first step a player can see the result of,
      and the first that can leave a house in a state nobody wants — so the two
      rules the record already paid for apply: nothing comes down until the new
      shape is legal, and the old walls come out as the *old* shape.
- [ ] **4. The cheap half of validation**, enforced at commit: inside the
      foundation's box, under a component ceiling, storeys within the limit.
- [ ] **5. Roofs, backup and restore** — the roof plane and a second working
      copy, which are the remaining `0xD7` roles.
- [ ] **6. The support-and-reachability half of validation**, deferred by name:
      *is this design structurally coherent* is a graph problem, and a floating
      tower is a cosmetic bug rather than a hole in the shard. It is worth doing
      after somebody has built enough houses to want it.

## Found along the way

Neither is in this plan's scope; both were noticed while step 1 was built and are
recorded here rather than left to be re-found.

- **The house window's bottom button row already runs off its own frame.**
  `sign.rs`'s `FRAME` is 520 wide, the five storage buttons step from x=20 by 100
  apiece, and `Demolish` is then drawn at **x=520** — on the frame's right edge,
  outside the background it is supposed to sit on. It has been that way since the
  storage row landed and no test looks at a coordinate, so nothing says so. Step
  2 adds a *toolbar's* worth of buttons to this window, which is the moment the
  row has to be laid out rather than stepped.
- **`state/src/lib.rs` re-exports most components and not all of them.**
  `DesignSession` was deliberately not added to that `pub use components::{…}`
  list, because `style.md` says a type is imported from where it is declared —
  which leaves the list half a rule. Either it is finished or it goes; a list
  that holds `HouseDesign` and not the component beside it is the worst of the
  two, since a reader cannot tell whether an absence is a decision.

## Not in this plan

- **An editor in our own client.** A designed house *draws* here; a client that
  can edit one is the other half of steps 1–3 and is its own piece of work,
  against `docs/client/`'s window rules rather than these.
- **House resizing and foundation upgrade.** A placement question wearing a
  design costume — it re-asks the five placement rules on a bigger footprint.
- **A design catalogue**, which is `.hdesign` generalised: content plumbing
  rather than a system.
