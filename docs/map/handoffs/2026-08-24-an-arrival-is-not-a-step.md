# 2026-08-24 — an arrival is not a step

The previous session's last table, first row: **the pier report's cause**, with
two suspects left and the cheaper of the two named — *arriving rather than
walking*. It was taken, it is real, it is the largest of the three by an order of
magnitude, and **it is still not the report**.

What it cost is one survey and one rule. The rule is that **the shard had four
spellings of "put a body on the ground here" and none of them was the step rule
— nor did any of them read the live world.**

## Where it stands

### 🚩 The arrival suspect was surveyed, and it is enormous ✅

[`arrival_survey`](../../../crates/common/movement/src/terrain.rs), beside the
two surveys that refuted the `landCheck` mechanism and in the same shape: over
facet 0, for each of the **27,052** pier and bridge decks, where does a body that
*arrives* there end up? A step reaches from the top of the art underfoot. An
arrival has nothing to reach from — so each of the shard's placement rules got
asked the same question, the way its own callers ask it.

| the rule | who arrived through it | on the deck | **under it** | refused |
|---|---|---|---|---|
| `MapTerrain::ground_z` | a fresh character's first tile, `.go`, the spawner's seed | 808 | **25,816** — median 10, worst 67, and 6,266 of them over open water | 0 |
| `MapTerrain::spawn_z` | `npc::spawn`, seeded from the ground | 18,862 | 3,139 (median 23) | 3,450 |
| `MapTerrain::stand_z` | the arrival test a recall or a gate travel is approved by | 21,255 | 0 | 4,593 |
| `housing::doorstep` | a banned player put out of a house | it *named* `at.z` — the house's own floor — and asked nothing | | |
| **`movement::arrival_z`** | **all four now** | 18,868 | **3,132**, only **7** over water | 3,450 |

**`ground_z` is the land tile's own average and reads no static at all.** On a
pier it answers the sea; on a bridge, the ravine. That is the whole of the first
row, and it is the shape of the report written in the code: *falling underground,
specifically on piers and bridges*.

And **none of the four read the `Overlay`**, which is the previous session's
finding one layer up: a body put on a moored ship's deck, or on the first floor
of a house somebody built this morning, lands in the sea or in the ground *by
construction* — the only layer that knows those exist was never asked.
`walk.rs`'s `an_arrival_stands_on_a_deck_the_map_knows_nothing_about` asserts
both halves of that, starting with the map's own refusal.

### 🚩 One rule, and every arrival goes through it ✅

`movement::arrival_z(footing, tile, near_z, height)` — `spawn_z`'s two arms with
the live layer folded into each:

- **The ordinary landing, taken in place.** `can_step` with one tile for both
  ends, rather than a second copy of the rule: "put here" is a step that goes
  nowhere, and it brings decks, stairs, ceilings and shut doors with it. This is
  the arm that keeps a banker put at z = 0 on the bank's ground floor instead of
  climbing to the second.
- **Otherwise every surface either layer has here**, whether or not a step could
  reach it — a shop's raised floor is where the tailor goes and nothing can climb
  to it. Filtered by `can_fit`, nearest to `near_z`, **tie to the lower**, which
  `Overlay::surface_at` and `path::goal_node` already break the same way and
  `spawn_z` left to the map file's static order.

Moved onto it: `WorldState::start_position`, `gm`'s `.go`, `npc::spawn`,
`housing::doorstep`, and the client's own `terrain_overlay` — which was composing
`spawn_z` and `can_stand` by hand and is a fifth spelling. `travel`'s
`can_stand_at` moved to `walk::can_fit` on the same footing, which is ServUO's
`Map.CanSpawnMobile` (`CanFit(x, y, z, 16)`) and what its own comment already
claimed to do and did not.

That last one is the only change with a *gameplay* consequence a player meets,
so it is the one with a shard-level test:
`a_recall_onto_a_moored_deck_is_allowed_and_the_open_sea_is_not` in
`travel_tests.rs` marks a rune three units over open water, recalls to it with no
ship there (refused) and with one moored (allowed, on the deck). It was run
against the retired rule and fails on the second half — a rune marked on your own
ship used to refuse its owner with "Something is blocking the location."

## What was decided

**The seed stays the caller's.** `arrival_z` answers *near a height*, and three
of its callers seed it from the ground on purpose: a spawner names a rectangle
and no storey, so a rat belongs on the floor of the dungeon and not on the
walkway over it. That is why 3,132 of the survey's decks still answer below —
all but seven are bridges over walkable land, where the ground is what the caller
asked about. Making the rule prefer the highest surface would put every spawn on
a roof.

**The first arm is a step and not a copy of one.** The alternative was to
re-derive `check`'s landing over both layers here, which is a second reading of
the same rule in the same crate — the exact defect `Plank::of_art` was fixed for
the day before.

**`.tele` and a moongate crossing keep their verbatim z.** A game master
clicking a spot means that spot, and ServUO's `Moongate.UseGate` is verbatim too.
Written down rather than changed, because the *asymmetry* — a recall checked, a
gate crossing not — is the thing a later reader would otherwise assume was an
oversight.

**The survey keeps all four rules side by side**, `the_reading_that_was`'s
reason: a survey that measured only the rule now in use would print the same
numbers whatever it did, and this one would notice a caller quietly moved back.

## What is clean

`cargo test -p openshard-movement -p openshard-map -p openshard-state -p
openshard-world -p openshard-npc -p openshard-housing`: **1,101 passed, 0
failed, 4 ignored.** `cargo clippy` silent on those and on
`openshard-client-app`. `cargo fmt --check` silent on every touched file. The
workspace as a whole was not run: a parallel session has `world`'s `tick/tests.rs`
and `ai`'s `lib.rs` open, and `client/render`'s `frame.rs` does not compile at
the moment.

## What is next

| | what would close it |
|---|---|
| **The pier report's cause, still unknown — and the last shard-side suspect is a multi-step walk**, where each step is right and the sequence drifts. Both facet surveys measure one step from a known surface | A walk of *several* steps along a pier, comparing where the shard puts the body after n steps against where the client's `predict_step` chain has drawn it. The client's `0x22` carries no position, so the drift is uncorrected until a `0x20` |
| 🚩 **`client.md` has already attributed this report, twice, and `roadmap.md`'s suspect list never mentioned it** | Its two entries call it one bug with two *client* causes: `GroundQuad` builds its four corner heights from the land layer only (still true — `ground.rs`'s `corners` is `WorldMap::land_corners`), and `Walk::step`'s predicted z came from the same place (**fixed** — `ui_command.rs` predicts with `predict_step` on both arms). Whether the surviving half draws as *sinking* is not obvious: the body is drawn **above** the plane, not below it, so the visible failure would have to come from the depth sort putting the plank in front of the body. Read both documents before taking the last suspect |
| **The seven decks over open water that `arrival_z` still answers below** | A handful, and the only ones left where the answer is to nowhere. Print them from the survey and look |
| **3,450 pier decks nothing can be placed on at all** | `can_fit`'s refusal, unchanged by this work and identical to `spawn_z`'s. Parity with ServUO's `CanFit`, so probably correct — but nobody has looked at one |
| **`Boats::deck_at`, `carries` and `blocks_at` still have no caller outside tests** | Inherited from the previous handoff, untouched. Either they read the overlay or they go |

**Where a session starts:** unchanged from the last handoff — era S's live
publish is the only *plan* node open, and it still has a design question in front
of it (who calls `MapSnapshot::publish`, and where in the tick). Everything above
is repair work that needs no plan.
