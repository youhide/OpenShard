# The order of work

The one page that says **which area is worked next**, across the whole engine.
Order, not dates.

It is not a status page and not a list of findings. What is built is described
in [`../../docs/`](../../docs/README.md); what is open inside an area is ranked
by that area's own README, and a measurement or a phase report is evidence.
This page only says what comes next and what has to be true before it can.

## Where an area's open work lives

A domain that has been migrated to the documentation canon keeps its own ranked
"what is open", and this page does no more than point at it. A domain that has
not been migrated keeps its queue here, because it has no README to keep it in —
so migrating a domain is also what empties its section below.

| Area | Its open work |
|---|---|
| World and map | ranked, [`docs/world/README.md`](../../docs/world/README.md) § what is open |
| The client | ranked, [`docs/client/README.md`](../../docs/client/README.md) § what is open |
| Rendering and lighting | [`docs/render/README.md`](../../docs/render/README.md), with [`plans/render/`](../render/lighting/PLAN.md) for what is not built |
| The protocol | ranked, [`docs/protocol/README.md`](../../docs/protocol/README.md) § what is open |
| The server | ranked, [`docs/server/README.md`](../../docs/server/README.md) § what is open, with [`plans/server/operations/PLAN.md`](../server/operations/PLAN.md) for what is not built |
| Items and crafting | ranked, [`docs/items/README.md`](../../docs/items/README.md) § what is open, with [`plans/items/item_identity/PLAN.md`](../items/item_identity/PLAN.md) for what is not built |
| Combat · housing · npc | the sections below, until each is migrated |

## Next

**Migrate the remaining domains.** Three areas still have their documents flat in
`docs/` and their phase records under `docs/roadmap/`: `combat`, `housing`,
`npc`. Each is one batch — decisions into `design_*`, phase records and
"amendments forced by" into `evidence/`, what is open into a domain README — and
none of them blocks another. The batch that claims a domain also takes its phase
file out of `docs/roadmap/` and its section out of this page.

The order to take them in is cheapest-first: `combat`, then `housing`, `npc`.
All three are served by one file,
[`docs/roadmap/06-gameplay/`](../../docs/roadmap/06-gameplay/README.md), and the
`items` batch answered what happens to it the way `server` did: take the two
subject files that belong to you, leave the other ten and the shared
`backlog/gameplay.md` where they are, and let the last domain out take the
directory with it. `backlog/gameplay.md` in particular is one record whose rows
belong to four domains at once; each batch lifts its own rows into its README's
"what is open" without cutting the record itself.

## Gameplay

- [ ] Complete classic-client per-weapon and per-body animations.
- [ ] Finish the remaining usable skills and exact skill/spell presentation.
- [ ] Add summons, the deferred spell subsystems, and adjacent-tile quarry
      pathing.
- [ ] Complete boats B3–B4.
- [ ] Complete house customisation C3–C4.
- [ ] Resolve the data-table and Felucca-converter findings in
      [`backlog/gameplay.md`](../../docs/roadmap/backlog/gameplay.md).

## Operations

Moved out with the `server` migration: metrics and tracing, the plugin
lifecycle, the administration API, the operator's stop and the licence gate are
[`plans/server/operations/PLAN.md`](../server/operations/PLAN.md), in the domain
whose crates they are. The map editor keeps its own plan,
[`plans/world/map_editor/PLAN.md`](../world/map_editor/PLAN.md).

## Later

- [ ] LLM NPCs, quest generation, GM assistant, and Discord integration — only
      after the engine stands on its own.
      [`backlog/later.md`](../../docs/roadmap/backlog/later.md).

## The formatting gate

`cargo fmt --all -- --check` is not silent, and the file it names is not the one
this entry named when it was written. On 2026-09-02 the whole drift is
`crates/server/state/src/item_definition.rs`; the three files this entry used to
list — `server/housing/src/lib.rs`, `server/housing/src/tests.rs`,
`server/world/src/tick/houses.rs` — are clean. Name the command, not the files:
the files change and the number in a queue goes stale without anybody seeing it.
