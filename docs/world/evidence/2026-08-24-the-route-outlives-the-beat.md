# 2026-08-24 — the route outlives the beat

One commit, against the top line of the previous handoff's backlog: *the exact
search runs every beat and its route is thrown away*. It is not thrown away any
more, and the two things found on the way there are worth more than the saving:
a route was being **advanced before the world had applied it**, and the constant
that decides how long a route is trusted is the same number as a townsperson's
whole beat.

**Twenty-four tiles of open ground used to cost twenty-four searches. It costs
five.**

## Where it stands

`step_body_toward` keeps what it plans. Every body that walks somewhere without
being a chase — a pet closing on its owner, a townsperson heading back to its
post, an escortable trailing its master — asked for a whole route on every beat,
took its first step and dropped the rest. The route is written on the body now
and followed a step per beat, which is the cadence `chase_step` has always walked
its own by and the references' own pattern.

- **`ChasePath` is `Route`**, and the rename is the point rather than tidying: it
  is no longer a chase's alone. It is shared **on purpose**, and what makes that
  safe is that it is keyed by two things — `goal` and `at` — so a route about
  another journey or from another place is stale by construction. The worst two
  callers can do to one another is force a re-plan, which is what every one of
  them did on every beat before this.
- **`cached_step` is the one mechanism**, and `chase_step`'s inline copy of it is
  gone. Four ways a route stops being one: the body is not standing where the
  next step starts, the goal moved past `GOAL_DRIFT`, the steps ran out, or
  `REPATH_TICKS` passed. A route that survives all four is still only a plan —
  the step it offers goes through `probe` before it is taken, so a crate dropped
  on the way, a door swung shut or a body standing in it costs a re-plan and
  never a step the shard would refuse.
- **`probe` hands back the landing instead of a yes**, because that is what a
  route has to be written down against.

## What was decided

### `Route::at` — a route says nothing from anywhere else ✅

**This is a defect fix and not bookkeeping.** A route was advanced on the beat
its direction was handed *out*, which is before the world has applied it — and
the world may refuse it: a mobile stepped into the way, the body is frozen, a
decree moved it. The route then ran from a place the body was not standing in,
and **every step of it stayed legal one at a time**, which is exactly why nothing
ever failed loudly. What it cost was not a wall walked through but a body quietly
walking a plan nobody made.

`at` is the whole place and not the tile — a landing is a tile *and* a height,
which is the identity `find_path` itself searches over.

The chase inherited the check along with the mechanism, so a chase whose step the
world refuses now re-plans around whatever stopped it instead of walking on from
where it thought it would be.

### The door policy is read from `Doors` and not from the body ✅

A cached step that a door refuses is not the world changing under the route: a
body that planned on `Doors::AllOpen` planned through that door. So `cached_step`
opens it, and a body that planned on `Doors::AsTheyStand` planned round shut
doors in the first place — one in the way is news, and the route is dropped.

That is the enum's own documented contract (*"a route is planned on it by a body
that intends to open its way along it"*) rather than a new rule, and it is read
from the argument the caller has already answered the question with. **It changes
one body's behaviour**: `pet_beat` passes `AllOpen`, so a pet now opens a door
standing on its route. What it did before was plan through the same door every
beat and butt into it for ever. The pet's own brain still says
`opens_doors: false` — see the backlog.

### The saving is counted, and the factor is not the route's length ✅

The previous handoff estimated this at *roughly the length of a route*. It is
not, and the arithmetic says why: a route is trusted for `REPATH_TICKS` and a
body acts every beat, so **one plan covers the whole of the first against the
second** and the route's length never enters it.

| | beat, in ticks | beats one plan covers | searches it saves |
|---|---|---|---|
| a chase or a pet, engaged | 8 (400 ms) | 5 | 4 in 5 |
| a pet following, idle | 16 | 2 | 1 in 2 |
| an escortable (`ESCORT_BEAT_TICKS`) | 6 | 6 | 5 in 6 |
| a townsperson (`npc::BEAT_TICKS`) | 40 | **1** | **none** |

`a_long_walk_plans_once_a_repath_window` is the count and not a sample: the tick
is deterministic and its dice are seeded. Twenty-four tiles of open ground, the
shipped 400 ms beat — 24 beats, **5 plans**. Open ground is deliberately the easy
case for the *old* code, since there is nothing to route around, and it is still
five searches against twenty-four.

## What is clean

`cargo test -p openshard-world -p openshard-ai -p openshard-npc -p
openshard-quests -p openshard-state -p openshard-movement`: 631 (world) + 157
(movement) + 139 (state) + 18 (npc) + 3 (quests) passed, 0 failed — `ai` carries
no tests of its own, because what it decides is only decidable against a facet
and a world. `cargo clippy` silent on all of them, `rustfmt` on every touched
file.

Three tests are new and each pins a different half:

- `a_planned_route_is_walked_rather_than_planned_again` — **the blindness**, the
  same thing the refusal memory's test asserts and for the same reason: it is the
  only thing about a kept answer a test can see. A better way opens while a route
  stands and is not taken until the route lapses. The oracle is
  `ai::step_toward`, which is the same decision with nowhere to keep it, so it
  says what a body with no route would do on the same ground in the same instant.
- `a_body_that_did_not_move_plans_its_route_again` — `Route::at`.
- `a_long_walk_plans_once_a_repath_window` — the count above.

**Not ours and still there:** `client/render`'s `frame` test wants a
`DirtyRows::start` that a parallel session has mid-change, so
`cargo check --workspace --all-targets` cannot be run to silence today. Every
other crate in the workspace compiles.

One thing to own, and it is the mirror of the previous handoff's: a parallel
session's bare `git commit` swept most of this session's working tree into
`e81c9879 upd` before it was finished. Nothing was lost and the tree is right;
what is in `2c6ea476` is the remainder. `git commit -- <paths>` guards the
direction this session was on the wrong end of.

## What is next

| | what would close it |
|---|---|
| 🚩 **A townsperson's beat and a route's trust window are the same number** | `npc::BEAT_TICKS` is 40 and `ai::REPATH_TICKS` is 40, chosen independently, and `next_beat` never returns a gap *shorter* than its interval — so a townsperson's route is always stale by the time it beats again and the cache cannot help the caller it would help most. Nothing in either file says the two are related. The decision to make is whether a route to a **static** goal deserves a time window at all: every step is already put to the live ground by `probe` and the goal drift is checked separately, so what the window buys is noticing a *better* way, and what it costs here is a full `PATH_BUDGET` search every two seconds for the whole of a sixty-second walk |
| 🚩 **A far chase pays a full 400-node local search before the graph is asked** | Carried over untouched. `plan_step` asks `find_path` first and falls through to `find_long_path` only on refusal. For a destination past `COARSE_MIN_DISTANCE` that the local search will refuse, that is the whole budget spent to learn what the region components already know — and they are not asked, because the bake does not keep them: `component_labels` is computed, read by the portal pass and dropped. An undirected union-find over the sampled places would be a **sound** refusal oracle (never a false refusal, since a directed route implies an undirected one) at `u16` a place — 16 MB over facet 0's 8.0M — and a new field in the bake |
| **Differential heuristics** | Unchanged. §3 of the previous handoff. Bake K landmark distances beside the navigation graph; admissible, so the oracle is the existing dump — the routes must not move, only the node counts |
| **`pet_beat` plans through doors it says it cannot open** | It passes `Doors::AllOpen` while `pets.rs` gives every pet `opens_doors: false`. Since this session the `AllOpen` half wins on a cached route, so a pet opens doors; the flag says it does not. One of the two is wrong and it is a gameplay question, not a code one — ServUO's pets do not work latches |
| **A door on a *freshly planned* first step is the caller's, on a cached one it is not** | `cached_step` opens doors on the route it is following; `step_body_toward` does not open one standing on the step it has just planned, and `npc::walk_home` opens it for itself afterwards. They do not double-fire and a pet only loses a beat to the difference, but "a townsperson opens the door in front of it" is written in two places |
| **Flow fields, the node budgets, one corner block for a whole expansion** | Unchanged from the previous handoff |
| ~~**The exact search runs every beat and its route is thrown away**~~ | **Done, above.** |
