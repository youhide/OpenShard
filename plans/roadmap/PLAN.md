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
| People and creatures | ranked, [`docs/npc/README.md`](../../docs/npc/README.md) § what is open, with [`plans/npc/`](../npc/creature_casting/PLAN.md) for what is not built |

## Next

**The migration is done, and this page is now only an order.** Every area in the
table above keeps its own ranked "what is open", and `docs/roadmap/` holds no
phase record at all: `npc` was the last batch, on 2026-09-03, and it took
`docs/roadmap/06-gameplay/` with it — the five files left there were all its own,
and [`docs/roadmap/README.md`](../../docs/roadmap/README.md) is now an index of
where every phase record went.

What comes next is a choice between areas rather than another migration. The
first of them was **a creature that casts**, and its first phase landed on
2026-09-03: a creature carries mana and a repertoire from its spawn data, throws
before it closes, and the cast goes through the sequence a player's does. Magic
is no longer one-directional.

What is still open there is the half that plan calls the decision — a creature
throws the strongest thing it can afford and nothing chooses by *category*, so
nothing heals, curses or escapes. That is C2, with C3's cadence behind it:
[`plans/npc/creature_casting/PLAN.md`](../npc/creature_casting/PLAN.md), and the
record is
[`docs/npc/evidence/2026-09-03-a-creature-that-casts.md`](../../docs/npc/evidence/2026-09-03-a-creature-that-casts.md).

## Gameplay

Every subject of the old gameplay phase has a domain now. Combat, skills and
magic are [`docs/combat/README.md`](../../docs/combat/README.md), with
[`plans/combat/actions/PLAN.md`](../combat/actions/PLAN.md) and
[`plans/combat/spells/PLAN.md`](../combat/spells/PLAN.md). Housing and boats are
[`docs/housing/README.md`](../../docs/housing/README.md), with
[`plans/housing/boats/PLAN.md`](../housing/boats/PLAN.md),
[`plans/housing/customisation/PLAN.md`](../housing/customisation/PLAN.md) and
[`plans/housing/house_region/PLAN.md`](../housing/house_region/PLAN.md). AI,
chat, regions' guards, guilds, parties and quests are
[`docs/npc/README.md`](../../docs/npc/README.md), with
[`plans/npc/creature_casting/PLAN.md`](../npc/creature_casting/PLAN.md),
[`plans/npc/pets/PLAN.md`](../npc/pets/PLAN.md) and
[`plans/npc/quests/PLAN.md`](../npc/quests/PLAN.md).

What stayed behind is `backlog/gameplay.md`, one record whose rows belong to four
domains at once: each batch lifted its own rows into its README's "what is open"
without cutting the record itself.

- [ ] Resolve what is left of the data-table and Felucca-converter findings in
      [`backlog/gameplay.md`](../../docs/roadmap/backlog/gameplay.md). The rows
      about creatures, vendors and notoriety are ranked in
      [`docs/npc/README.md`](../../docs/npc/README.md) now — including
      adjacent-tile quarry pathing, which used to be this list's own line — and
      what remains there belongs to `world`, `items` and `server`.

## Operations

Moved out with the `server` migration: the plugin lifecycle, the administration
API, the operator's stop and the licence gate are
[`plans/server/operations/PLAN.md`](../server/operations/PLAN.md), in the domain
whose crates they are. Metrics and tracing were on that list and are not any
more — a shard publishes `GET /metrics` and `GET /health` and the record is
[`docs/server/evidence/2026-09-03-metrics-and-health.md`](../../docs/server/evidence/2026-09-03-metrics-and-health.md).
The map editor keeps its own plan,
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
