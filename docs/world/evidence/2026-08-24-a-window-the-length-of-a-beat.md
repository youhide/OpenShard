# 2026-08-24 — a window the length of a beat is no window

The previous session's first 🚩: *a townsperson's beat and a route's trust window
are the same number*. It was taken, and what was behind it is bigger than the
entry — **two time windows in `ai` were forty ticks, and so is the beat of the
caller each was written for.** Neither had ever fired for that caller. Both were
found the same way and both are gone.

Three commits, and the second and third are what the first turned over on its
way: the rule about a door in front of a body was written in four places, and
the field that says whether a pet has hands was read by none of them.

## Where it stands

### 🚩 The window is the caller's statement now, not a constant behind it ✅

[`ai::Goal`](../../../crates/server/ai/src/lib.rs) — `Fixed` or `Moving`, passed
by every caller of `step_body_toward`, and the only thing it decides is whether
the route the body keeps carries a *time* window.

**What a window buys is noticing a better way, and nothing else.** Every other
way a kept route can go wrong is caught on its own and without a search: the
body standing somewhere else is `Route::at`, the goal having moved is
`GOAL_DRIFT`, the ground having changed under the next step is `probe`, which
puts every step of every route to the live world before it is taken. So a route
to a post was being re-planned to learn that a shorter way had opened — an
answer nobody asked for, at a full `PATH_BUDGET` search.

**And the caller it would help most never got it.** `REPATH_TICKS` is 40 and
`npc::BEAT_TICKS` is 40, arrived at in two files that do not mention one
another, and `next_beat` never arms a gap *shorter* than its interval. A
townsperson's route was therefore stale on every beat it was ever read on.

| | beat, in ticks | plans over a 24-tile walk |
|---|---|---|
| a chase or a pet, engaged (`Goal::Moving`) | 8 | 5 |
| a townsperson walking home (`Goal::Fixed`) | 40 | **1** |

`a_long_walk_to_a_place_plans_once_altogether` is the second reading of the
counted walk the previous session wrote; the walk itself is one helper now and
the two tests are two `Goal`s through it.
`a_route_to_a_place_is_walked_past_the_window_that_would_have_lapsed` is the
mirror of the blindness test beside it — same ground, same shortcut opening,
same wait — and it fails against the retired rule.

### 🚩 The same collision, one layer down ✅

`REFUSAL_TICKS` **was** `REPATH_TICKS`, so a refused coarse query was remembered
for exactly the 40 ticks a townsperson sleeps between beats. N7 wrote that
memory for "a pet, a townsperson walking home and an escortable"; for the second
of the three it had always lapsed by the time the body woke to use it.

It is 200 now, and the floor under the number is stated where it lives: a
refusal must outlive a beat of everything that reads one. That is what
`GUARD_TICKS` is, and the two are deliberately **not** written as one number —
a value two files arrive at separately is the defect above.

### The door in front of a body is one rule, not four ✅

`cached_step` opened a door on the route it was following. `chase_step` opened
its own, twice. `step_body_toward` handed back a step it had just watched the
live world refuse and left the door to its caller — `npc::walk_home` did it,
re-deriving the door out of the obstruction index for the *first* step only, and
nobody else did.

**So an escortable following its master through a shop door planned the same
route into the same shut door on every beat of its walk.** It had no code of its
own, and the half of `walk_home` that spared a townsperson the same fate was a
second reading of a rule `ai` already applied to every other step of the route.

`way_ahead` is `probe` with the policy applied: `Way::Open(landing)`,
`Way::Opening` (the beat went on the latch and the step is still due), or
`Way::Shut`. Three call sites read it, `walk_home` is the one call it always
should have been, and
`a_door_on_a_freshly_planned_step_is_opened_rather_than_walked_into` pins the
case that had nobody.

### A pet's `opens_doors` was a dead field ✅

`pet_beat` walked every pet on `Doors::AllOpen` whatever body it wore, and since
routes started outliving the beat that planned them, that meant a horse *opening*
doors. The flag is set from the body at the taming and again at the restore, so
it was right all along and unread. It reads the brain now — ServUO's
`BaseAI.CanOpenDoors`, the same read `chase_step` makes of a wild one.

## What was decided

**A bigger number was refused.** The window could have been made 200 like the
refusal, and a townsperson would have got five beats out of it. It would also
have hidden the question behind a different number, and the question is not how
long: it is that one of the two kinds of goal has nothing to gain from a clock.

**A `Goal::Fixed` route keeps the other three checks**, and that is asserted
rather than assumed — `a_body_that_did_not_move_plans_its_route_again` was moved
onto `Fixed` on purpose, so dropping the window cannot quietly drop `Route::at`
with it.

**A pet works a latch only if its body has hands** — the user's call, on
parity. What it costs a player is real and worth writing down: a llama does not
follow you through a shut door until you open it. The alternative was to keep
`AllOpen` and delete the flag from a pet's brain as decoration.

**The brain is read with `?` and never defaulted.** Every pet has one by
construction: taming gives a brainless prop horse one, a restore rebuilds it,
and the tick's loop is over brains.

## What is clean

`cargo test -p openshard-world -p openshard-ai -p openshard-npc -p
openshard-quests -p openshard-state`: **637 (world) + 139 (state) + 18 (npc) +
3 (quests) passed, 0 failed** — `ai` carries no tests of its own, because what it
decides is only decidable against a facet and a world. `cargo clippy` silent on
all of them, `rustfmt` on every touched file.

`cargo check --workspace --all-targets` still cannot be run to silence:
`client/render`'s `frame` test wants a `DirtyRows::start` a parallel session has
mid-change. Every other crate in the workspace compiles.

## What is next

| | what would close it |
|---|---|
| 🚩 **A far chase pays a full 400-node local search before the graph is asked** | Carried, and now with the obstacle named. The proposal is an undirected union-find over the sampled places as a sound refusal oracle, at `u16` a place — but the artifact stores no *addressing* for places, and facet 0 has 25.2M columns against 8.0M places, so a per-place array needs a per-column offset table beside it and the 16 MB is only the labels. **That is a design decision and not a session**: either the bake grows an offset table, or the labels are keyed off `SpanIndex`, which lives in `openshard-movement` and would drag the map crate's own layering into it |
| **Differential heuristics** | Unchanged. Bake K landmark distances beside the navigation graph; admissible, so the oracle is the existing dump — the routes must not move, only the node counts |
| **`spawn_brained` returns the first brained entity, not the one it just spawned** | A test that spawns two creatures gets the same one back twice. `a_pet_works_a_latch_only_if_its_body_has_hands` works round it by spawning the owner first and taking "the other one". A fixture that returns what it spawned would not need the trick |
| **An escortable's walk has no test of its own** | The escort gained a door-opener this session by inheriting one, and what asserts it is a unit-shaped test in `world`. The quest path — a master walking into a shop and the traveller following — is asserted by nothing |
| **`pet_beat` reads a brain and `pets.rs` writes one** | The fallback brain a prop horse is given at taming still spells `opens_doors: false` by hand where every other site derives it from the body. One line, and it is only wrong for a prop *humanoid* nobody has tamed yet |
| **Flow fields, the node budgets, one corner block for a whole expansion** | Unchanged |

**Where a session starts:** unchanged. Era S's live publish is the only *plan*
node open and still has its design question in front of it (who calls
`MapSnapshot::publish`, and where in the tick). The pier report's last shard-side
suspect — a multi-step walk that drifts — is the other handoff's, and is
untouched by any of this.
