# 2026-08-23 — N7: the server reads the graph

Era P's last open node, in one commit, and the first one a player is on the
other end of. [`navigation_spans.md`](../navigation_spans.md)'s N7 spends what
N4 built: the baked navigation graph has been loaded, validated and resident on
every shard boot since the terrain-seam work, with **nothing but a test reading
it**. `ai::step_toward` planned with flat `find_path` at a budget of 400 nodes
and, when that was refused, walked the straight-line direction — so a pet, an
escort or a townsperson could not route across a town while the answer sat in
the facet beside it. It asks the graph now, past eight tiles, which is **the
same fall-back the client walks a click by**.

One commit: `01bdd0a2`. It also closes
[`terrain_seam.md`](../terrain_seam.md)'s **F**, which asked *wire it up or stop
paying for it*, answered *wire it up*, and handed the action here.

## Where it stands

**The done-when is a test and the test is
`a_creature_routes_past_its_exact_budget_over_the_coarse_graph`**, in
`world/src/tick/tests.rs`. Two corridors 96×64, divided along y=32 and joined
only at the east end, so the way through is **eighty-odd tiles away from a goal
thirty-two tiles off**:

| | flat origin (2, 20, 0) | raised origin (2, 16, 5) |
|---|---|---|
| the flood says the goal is | walkable | walkable |
| flat A\* at `PATH_BUDGET` | refused, **401 nodes, exit `Budget`** | refused, 401, `Budget` |
| the walk, with the graph | **arrives**, 168 steps | **arrives**, 168 steps |
| the walk, on a facet with no graph | stands at (2, 31, 0) | stands at (2, 31, 0) |

**The raised origin stands on a walkway of statics five units up**, laid on
ground with no room for a body under it — so it is a place `ground_z` does not
report, which is the half that would have passed for the wrong reason before N4.
The test says so in its own terms rather than trusting the fixture: the flood
from the walkway reaches (10, 16) at z=5 and the flood from the plain does not
reach it at all, neither onto it nor under it.

**The control is the same facet with no graph**, and that is what the shard was.
It is also the anti-tautology check, run by hand: with the fall-back disabled the
test fails at the first assertion with the creature at (2, 31, 0) — the divider —
rather than passing for some other reason.

**Both halves of the walk are the shard's own.** `ai::step_toward` says which
way and `step_allowed` says whether a body may, one step per beat, so the test
cannot walk a step the world would refuse.

**`cargo check --workspace --all-targets` and `cargo fmt --all -- --check` are
silent.** `cargo clippy --workspace --all-targets` has one warning and it is not
this: `large_enum_variant` on `openshard-persistence`'s `Store`, a parallel
session's in-flight file. Tests were run on the crates this touches —
`openshard-movement` (137 + 7 + 1 + 5 doctests), `openshard-world` (613),
`openshard-client-app` (381), `openshard-server` (37), `openshard-npc`,
`openshard-quests` — all green, and `openshard-state` ends with the same two
long-standing red tests and no others (R1's finding, still filed under
[*`can_step` does not check the corner*](../../roadmap.md)).

## What the node decided

**The threshold is the router's, not either caller's.** `COARSE_MIN_DISTANCE`
was a private `8` in the client's `steer.rs` with the argument for it written
there. The argument is about `local_costs` — joining an endpoint to the graph is
one exact search per node of its region, at *both* ends — which is a fact about
`find_long_path` and about neither caller, so it moved to
[`navigation.rs`](../../../crates/common/movement/src/navigation.rs) beside it.
A fall-back the two ends drew at different distances would be two answers to
"how far can a body plan", which is the disagreement this node closes; a second
copy of `8` on the shard would have been that disagreement waiting to happen.

**The bare map is one value, and the empty overlay is one for the process.** The
graph is baked over the bare map, so the corridor it proposes has to be read over
the bare map — a door that happens to be shut must not rewrite a route's
*topology*. Each end used to build that reading itself out of an empty `Overlay`
it kept alive somewhere: a `LazyLock` in the client's `world.rs`, and nothing at
all on the shard. It is
[`Footing::guide`](../../../crates/common/movement/src/footing.rs) now, with
`world::guide` and `WorldState::guide` as its two callers and one `NOTHING_PLACED`
behind both. Being *the absence of a live layer*, a second one would only be a
second name for the same emptiness — and an empty overlay a caller keeps beside
its map is a thing that can be written to by mistake.

**A client with no facet open now gets a footing with no map, where the old
`guide` panicked.** `Footing::guide` reads the ground's own `terrain(tiles)`,
which is an `Option`; the client's version went through a `terrain()` that
`expect`s a facet. Nothing asks before it has one — the change is that saying
"there is no map" is what a value the caller cannot mis-pair can say, and a panic
cannot.

**`PATH_BUDGET` is public.** It is the subject of an assertion and not only a
knob: the test has to say which budget the exact search was refused at, and a
copy of `400` in the test would be a second place to change it.

**The corridor is the only thing the bare map decides.** `find_long_path` takes
the guide and the live footing side by side, and only the second approves a step
— so a crate dropped in a doorway still refuses the step it is standing in, and
a door-opener still plans through doors with `Doors::AllOpen` exactly as it did.

## What was found

Filed in [`navigation_spans.md`](../navigation_spans.md)'s *Out of scope, named*.

- **🚩 The coarse router refuses outright when both endpoints share a region.**
  `find_long_path` special-cases `from_region == to_region` into `region_route`,
  which is confined to that 32×32 rectangle — so two points twenty tiles apart
  whose only connection leaves the region and comes back get
  `LongExit::NoLocalRoute`, and the graph is never consulted. Found while sizing
  the fixture: a first shape put both ends in region 0 and the router answered
  `None` at every width tried, over ground the flood called walkable. Not the
  shard's worst case — a body and its goal in one region are what the exact
  search is for — but it is a refusal the graph could answer, and the repair is
  to let the corridor leave the region when the local route fails rather than
  *instead of* trying it.
- **The aggressive chase does not go through `step_toward`.** `ai::chase_step`
  plans its own route with a bare `find_path` at `PATH_BUDGET`, caches it as a
  `ChasePath`, and on a refusal calls `give_up` — guard ten seconds, then
  wander. So this node reaches pets, escorts and townspeople and not a creature
  chasing a player. Deliberate: the plan named `step_toward` and only
  `step_toward`, and a chase is already bounded to twice a creature's sight, so
  a quarry it may legitimately follow is rarely further than the exact search
  reaches. Whoever wants a creature to round a town block should know the second
  planner is there.
- **🚩 A refused coarse query pays the whole join, and nothing behind
  `step_toward` remembers it.** A goal that looks walkable and is sealed off
  costs `local_costs` at both ends in full, plus up to `LIVE_REROUTES` abstract
  retries: **17.4 ms on a 96×64 fixture with twenty nodes, in a debug build**,
  repeatable to the tenth. `chase_step` has `give_up`'s ten-second guard behind
  its refusal; `step_toward` is a pure function of the world and has nowhere to
  put one, so an escort whose goal is unreachable and more than eight tiles away
  pays that on every beat. The 50 ms deadline bounds it and nothing here is on a
  tick's critical path, but the cost is new and it is per beat per body.

## What is next

**Nothing in [`navigation_spans.md`](../navigation_spans.md) is open.** N0–N4,
N3b and N7 are built. N5 (off-mesh links) is deliberately empty until a flood
says what the spans still cannot connect, and that flood is N5's own first step;
N6 (an artifact for the spans) is gated on a number nobody has asked for, and the
expected outcome is *not needed*.

**Rebake before running anything.** `ROUTING_VERSION` is 4 since N4, so every
artifact baked before it is refused and the shard does not boot.
`cargo run --release -p openshard-movement --bin openshard-navigation-bake --
--facet 0`, 11.7 s.

**If someone picks up one of the three findings**, the same-region refusal is the
one with a user-visible defect behind it: a creature and its goal in one 32×32
rectangle, joined only by a way round outside it, get no route from either search.
The other two are a cost and a scope note.

**What would block it:** nothing.
