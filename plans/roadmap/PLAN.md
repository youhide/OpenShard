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
| Combat, skills and magic | ranked, [`docs/combat/README.md`](../../docs/combat/README.md) § what is open, with [`plans/combat/`](../combat/actions/PLAN.md) for what is not built |
| Housing and boats | ranked, [`docs/housing/README.md`](../../docs/housing/README.md) § what is open, with [`plans/housing/`](../housing/customisation/PLAN.md) for what is not built |
| npc | the section below, until it is migrated |

## Next

**Migrate the last domain.** One area still has its phase records under
`docs/roadmap/`: `npc`. It is one batch — decisions into `design_*`, phase
records and "amendments forced by" into `evidence/`, what is open into a domain
README — and it is the batch that takes
[`docs/roadmap/06-gameplay/`](../../docs/roadmap/06-gameplay/README.md) with it,
because the five files left there are all its own: AI, chat, regions, guilds,
and parties-and-quests.

`combat` went on 2026-09-03 and took Combat, Skills and Magic; `housing` went the
same day and took Housing and Boats-and-customisation. What stays behind either
way is `backlog/gameplay.md`, one record whose rows belong to four domains at
once: each batch lifts its own rows into its README's "what is open" without
cutting the record itself.

## Gameplay

Combat, skills and magic left this section on 2026-09-03: the animation,
skill-presentation and deferred-spell rows are ranked in
[`docs/combat/README.md`](../../docs/combat/README.md) now, and the ordered work
is [`plans/combat/actions/PLAN.md`](../combat/actions/PLAN.md) and
[`plans/combat/spells/PLAN.md`](../combat/spells/PLAN.md).

Housing and boats left it the same day. Boats B3–B4 are
[`plans/housing/boats/PLAN.md`](../housing/boats/PLAN.md), house customisation
C3–C4 is [`plans/housing/customisation/PLAN.md`](../housing/customisation/PLAN.md),
and the house-as-a-region decision that blocked five phases ago is
[`plans/housing/house_region/PLAN.md`](../housing/house_region/PLAN.md).

- [ ] Adjacent-tile quarry pathing (`npc`).
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
