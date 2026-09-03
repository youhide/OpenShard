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
  `QuestGumpRequest` as the worked example.

## The order

- [ ] **1. The session brackets.** `DesignSession` on the house entity, entered
      by the owner through `standing_of`, left on end-customisation — and ended
      by logout, death and `collapse_houses`, because a dangling session on a
      despawned house surfaces as a panic rather than as a missing feature. No
      editing verbs yet: this step is what makes "in a session" a state the shard
      can be asked about.
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

## Not in this plan

- **An editor in our own client.** A designed house *draws* here; a client that
  can edit one is the other half of steps 1–3 and is its own piece of work,
  against `docs/client/`'s window rules rather than these.
- **House resizing and foundation upgrade.** A placement question wearing a
  design costume — it re-asks the five placement rules on a bigger footprint.
- **A design catalogue**, which is `.hdesign` generalised: content plumbing
  rather than a system.
