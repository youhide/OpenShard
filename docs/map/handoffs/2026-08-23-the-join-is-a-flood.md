# 2026-08-23 — the join is a flood, and a refusal is remembered

The last two findings in
[`navigation_spans.md`](../navigation_spans.md)'s *Out of scope, named* with a
defect behind them, taken together because they are one repair wearing two
names: **the endpoint join was paid in full for an answer nothing keeps.** N4's
`local_costs` fan-out is what made the join expensive; N7's unremembered refusal
is what made it happen every beat.

## Where it stands

**Both fixed.**

### The join is one flood

`local_costs` ran a bounded exact search from the endpoint to **every node of
its own region**, at both ends of a query. A node the endpoint cannot reach cost
the whole budget before saying so, and N4 had just made the regions that matter
three times denser — the castle's went from 18 nodes to 51 — so the same seven
routes went from 1.29 ms to 4.39 ms p50.

It is a uniform-cost flood now. Every place of the region is expanded at most
once, however many nodes stand in it, and a node outside the endpoint's reach
costs nothing at all because the flood never arrives there. **That reach is the
component label the bake computes and throws away** — which was the second of
the two repairs the finding offered, arrived at without the artifact growing by
a byte. The other, bounding the fan-out by distance, would have been a guess
about how far a portal is; this is the answer.

Three things fell out of it that are worth naming:

| | |
|---|---|
| the two directions are two traversals | The step rule is asymmetric, so *out of* an endpoint and *into* it are different sets at different costs. `region_costs` is the forward one and `region_costs_into` the reverse. |
| there is no reverse step rule to ask | So a place's predecessors are found by asking the eight neighbouring columns where *they* land — every predecessor stands in one of them — with each candidate's expansion computed once and kept. |
| the flood needs no deadline | It is bounded by the region where a fan-out was bounded per node and paid per node. The deadline is read after the join, where it already was. |

`RegionPlaces` is what both the bake and a query index a region's places by, and
a query now builds one for itself out of the ground — through the same
`column_places` the facet-wide sampling uses, so what a query sees is exactly
what the bake saw and a node of that region is always findable in it.

### A refusal is written on the body

`ai::step_toward` is a pure function of the world: it could pay the join at both
ends, be refused, and be asked the identical question on the next beat, for as
long as a body kept following something it cannot reach. `chase_step` never did
— a refused chase goes through `give_up` and stands watch for ten seconds — and
a pet, a townsperson walking home and an escortable all did.

`ai::step_body_toward` is the same decision made for an *entity* rather than for
a point. A refusal is written on it as a `RouteRefused { goal, until }`, and
while that stands the graph is not asked about that goal again.

- **Only the coarse half waits.** The exact search runs every beat as it always
  did, so a way that opens within `PATH_BUDGET` is taken at once. What waits is
  the facet-wide answer, for `REFUSAL_TICKS` — the repath cadence, ~2 s.
- **A goal that drifts past `GOAL_DRIFT` clears it**, exactly as it invalidates
  a `ChasePath`: a body following something that has moved on is asking a
  different question.
- **`step_toward` stays**, as the pure reading. The shard's own walk probe in
  `tick/tests.rs` asks through it, and a body that has nowhere to keep an answer
  is a real caller.
- The split is `search_path`'s to `find_path`: one decision, both readings. The
  direction alone cannot say *why* it is what it is, and why is the only thing a
  caller with a memory acts on.

## What it is measured against

**`coarse_bench` on facet 0**, from the castle at (1363, 1600, 30), release,
min-of-three per query, p50 across eight destinations a band. Three runs of each
build agreed to the hundredth of a millisecond.

| band | fan-out p50 | flood p50 | fan-out worst | flood worst |
|---|---|---|---|---|
| 32 | 3.70 | **0.53** | 5.94 | **1.06** |
| 64 | 2.74 | **0.66** | 4.37 | **1.01** |
| 128 | 2.44 | **1.00** | 6.50 | **1.21** |
| 256 | 2.44 | **1.13** | 2.90 | **1.34** |
| 512 | 2.96 | **1.56** | 4.09 | **2.67** |
| 1024 | 3.75 | **2.32** | 5.73 | **2.89** |

**Every route came back with the same number of steps** — all 45 destinations,
diffed. The join got cheaper and the answers did not move. `routed` is unchanged
band for band.

**Band 32 is where N4's regression was measured**, and 0.53 ms is below the
1.29 ms it regressed *from*. The worst reading of any band falls 6.50 → 2.89 ms.

**The bake is byte-identical.** `intra_edges` was given the same early exit —
a flood that has costed every node of its region has nothing left to learn —
and facet 0 baked before and after hashes the same, 7,441,177 bytes,
71,545 nodes, 416,122 edges. So **no rebake is needed for this change**; the
artifact and `ROUTING_VERSION` 4 are untouched.

On the debug fixture the previous handoff measured — 64×64, sixteen nodes,
fastest of ten — the same-region refusal is **28.8 → 16.4 ms** and the corridor
**30.1 → 17.8 ms**. The synthetic fixture understates the repair by design: it
has eight nodes to a region where the castle has fifty-one, and the flood's cost
is the region's while the fan-out's was the region's *times its nodes*.

## The done-whens, and their controls

**`a_one_way_drop_joins_an_endpoint_one_way`** (`navigation.rs`). A walkway of
statics five units up, stopping well short of the region border so no node
stands on it: a body up there can leave and nothing can arrive.

| | |
|---|---|
| the ground, first | `step_allowed` off the walkway is `Some`, back onto it is `None` |
| `Join::OutOf` | reaches **every** node of the region |
| `Join::Into` | reaches **none** of them |
| and the router says it | the drop is a route, and nothing climbs back |

*The control* is the reverse flood replaced by the forward one — one line. The
test fails at *and no portal of it reaches the walkway* and nowhere earlier.

**`a_refused_long_route_is_remembered_until_it_lapses`**
(`world/src/tick/tests.rs`). One wall across a 96×64 facet with a single
doorway at the far end, and a shut door on it. The corridor exists on the bare
map the graph is baked over, and the live layer refuses every hop of it.

1. Shut: the step is the straight line at the wall, and a `RouteRefused` naming
   the goal is on the body.
2. The door opens. The step is **still** the straight line — the graph was not
   asked.
3. `REFUSAL_TICKS` pass. The step is the corridor's, aimed at the doorway, and
   the memory is cleared.

*The control* is the memory disabled by hand. The test fails at step 2 — *the
graph is not asked again while the refusal stands* — with `SouthEast` against
`South`, which is the corridor answering when it should not have been consulted.
**The blindness is the only thing about a memory a test can see**, and it is
deliberate: two seconds of not re-asking is what the memory buys.

## What is clean

`cargo fmt`, `cargo clippy --workspace --all-targets`, and the suites of every
crate this touches: `openshard-movement` (139 + 1 + 7 + 5 doctests),
`openshard-world` (614), `openshard-client-app` (381), `openshard-ai`,
`openshard-npc`, `openshard-quests`.

**Two tests were red on `main` before this work and are green now.** Found by
running the affected suites, verified to fail with this change reverted, and
repaired because a red pair makes every other suite's silence unreadable. Both
are in `crates/server/state/src/obstruct.rs`, and each had had the thing it
asked about moved out from under it:

- `a_diagonal_is_refused_when_either_flank_is_blocked` (and its passing-for-the
  -wrong-reason sibling `a_diagonal_passes_an_open_corner`) asked `can_step`
  whether a diagonal cuts a corner. **`can_step` is one landing and has no
  corner rule** — it answers whether a body may stand where a step ends. The
  rule is `steps_out_of`'s, which resolves all eight neighbours together
  precisely so a diagonal can read its two flanks, and it moved there in N3.
  Both now ask `step_allowed`, which is the reading every caller wants. *This
  is worth knowing beyond the test:* anything deciding a **step** through
  `can_step` is skipping the corner rule.
- `a_live_terrain_with_no_map_reports_no_water` built a `Footing` with no map
  and then called `.map.unwrap()`, so it panicked unconditionally. Water is the
  map's word and a footing with no map has no word — the type says so now — so
  what is left worth asserting is the consequence, and it asserts that.

Not repaired and not this work's: `cargo clippy --workspace --all-targets` is
not silent on `main` either — one `needless_borrow` in
`crates/common/uofiles/src/map.rs`, plus three in
`crates/client/render/tests/traced.rs`, which a parallel session has open.

## What is next

**Nothing in [`navigation_spans.md`](../navigation_spans.md) is open**, and
nothing there is a defect any more. What is left in *Out of scope, named* is
filed observation: the count tables that are bigger than the spans they address,
the map and the overlay disagreeing about a platform of no thickness, the
`WorldState::tiles` field that can be written past its bake, the interiors bake
building span indexes of its own, `steer::Ground` being the misnamed one.

**The two nodes that remain are gated rather than queued.** N5's content is
deliberately empty until a flood says what the spans still cannot connect, and
N6 is gated on a number nobody has asked for.

**🚩 What the corner-rule repair turned up is the one open defect**, and it is
bigger than the tests that led to it. `can_step` has no diagonal rule in it, and
**two production callers decide a step through it**: the shard's own move
validator in
[`tick/motion.rs`](../../../crates/server/world/src/tick/motion.rs) and `ai`'s
`probe`, which is what a chase asks whether a direction is open. So `find_path`
refuses to *plan* a corner cut and the shard then *permits* one walked by hand
— and a client sending steps itself is not a client that plans them. The two
tests are repaired to ask `step_allowed`; the two production sites are left
alone deliberately, because changing what the shard permits is a gameplay rule
and wants its own measurement against both references. Filed in
[`navigation_spans.md`](../navigation_spans.md)'s *Out of scope, named*, and it
belongs to the step rule rather than to this plan.

**What would block it:** nothing.
