# 2026-08-23 — a region is not a component

The first of the three things [N7](2026-08-23-n7-the-server-reads-the-graph.md)
found and did not fix, and the one with a user-visible defect behind it:
`find_long_path` **refused outright when both endpoints shared a region**, over
ground the exact search walks.

A region is a 32×32 rectangle of the facet, cut by arithmetic on coordinates —
`region_at` is a division, not a flood. A component is what is actually joined.
The router treated the first as the second: `from_region == to_region` went to
`region_route`, which is *confined to the rectangle*, and that search's refusal
was returned as the query's answer. So two points twenty tiles apart whose only
connection leaves the region and comes back got `LongExit::NoLocalRoute` and the
graph beside them was never consulted — a creature and its goal inside one
rectangle, joined only by the way round outside it, got no route from either
search.

## Where it stands

**Fixed.** The local route is a first attempt now rather than the verdict: when
it fails, the query falls through to the same join, corridor and refinement a
cross-region query already took. Nothing else about the router moved — the same
`local_costs` at both ends, the same `abstract_path`, the same
`LIVE_REROUTES` retries, and the same live footing approving every step.

`LongExit::NoLocalRoute` is gone with the branch that named it; there are four
refusal names where there were five, and
[`terrain_seam.md`](../research/terrain_seam.md) says so beside the tally that predates
it. `refine`'s `expect` said *different regions always need graph transitions*,
which was the endpoints' property and is now the abstract route's own: it says
*an abstract route always names at least one node*.

**The done-when is `two_points_in_one_region_route_by_leaving_it`**, in
`navigation.rs`. A 64×64 grid, a wall the length of region 0 and no further, so
the only way from (4, 4) to (28, 4) is south into the region below, across, and
back north:

| | |
|---|---|
| both endpoints' region | the same one, asserted rather than assumed |
| the exhaustive exact search (budget 4,096) | **58 steps**, 558 nodes explored, 6.7 ms |
| `find_long_path` before | `None` |
| `find_long_path` now | **58 steps**, and the walk ends on the goal |
| where the route goes | outside the rectangle — checked step by step |

**That last row is what says the graph answered.** `region_route` cannot leave
the region, so a step with `y >= 32` in the returned route is the corridor's own
work and not the local search's.

**The control is the fall-through disabled by hand.** With the same-region
branch made to return `None` again, the test fails at exactly the assertion that
matters — *a corridor answers* — and not before it: the oracle above it, the
exhaustive exact search, still passes. The refusal was the router's and not the
ground's.

**`cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets` are
silent** — including the `large_enum_variant` on `openshard-persistence` that
N7 recorded as a parallel session's in-flight file; it is gone. Tests were run
on the crates this touches: `openshard-movement` (138 + 1 + 7 + 5 doctests, one
more than N7 left), `openshard-world` (613), `openshard-client-app` (381),
`openshard-ai`, `openshard-npc`, `openshard-quests` — all green.

## What it decided

**The local search is tried first, and not skipped.** The alternative was to
send every same-region query to the graph and let the corridor's own refinement
find the short local route. It is refused for two reasons: a route inside one
region is what the exact search is *for*, and it is the cheaper of the two by
five times on the fixture measured below — and the graph is baked over the bare
map, so a corridor is a proposal about topology that the live layer then has to
approve, where `region_route` walks the live footing directly.

**A refusal is a stage, not an exit.** `NoLocalRoute` was a name for *we did not
ask*, which is not a reason a query failed; the four that remain — `OffGraph`,
`NoJoin`, `NoCorridor`, `PortalsExhausted`, and `Deadline` beside them — each
name a thing that was tried and did not work. Keeping the variant to mean "the
local route failed on the way to the graph" would have put a stage in an enum of
outcomes.

**The order is local, then graph, and the deadline is read once.** A local
refusal caused by the deadline rather than by the ground falls through too, and
the deadline check that already stood after `local_costs` catches it as
`Deadline` — so a query that ran out of time is not reported as a query that
found no join.

## What was found

Filed in [`navigation_spans.md`](../design_spans.md)'s *Out of scope,
named*, on the finding this repair widens.

- **🚩 The cost of the join reaches more queries now.** A same-region refusal
  used to be one confined A\*; it pays the whole endpoint join twice over one
  region instead. On the sealed version of this fixture — 64×64, sixteen nodes,
  debug, repeatable to the tenth — it is **4.8 ms → 25 ms**. The successful
  query is the same shape at **37 ms**, for the 58 steps the exhaustive exact
  search found in 6.7 ms at a budget of 4,096 nodes. So on this ground the
  coarse router is not cheaper than the exact search would have been *with a
  budget the shard does not grant it*: `PATH_BUDGET` is 400, and the exact
  search wanted 558. That is the fall-back working as designed, and it is also
  the argument for the two open findings being one repair — the join is paid in
  full for an answer nothing keeps.

## What is next

**Nothing in [`navigation_spans.md`](../design_spans.md) is open**, and the
two findings that remain with a defect behind them are one repair wearing two
names:

- N4's **`local_costs` fan-out** — joining an endpoint runs a bounded A\* from it
  to *every* node of its region, at both ends, and a node it cannot reach costs
  the whole budget before saying so. The repair is a design question: bound the
  fan-out by distance, or cut it to the endpoint's own *component*, which is a
  label the bake computes and then throws away.
- N7's **unremembered refusal** — `step_toward` is a pure function of the world
  and has nowhere to put a "this goal was sealed ten seconds ago" guard, which
  is what `chase_step` has in `give_up`.

Whoever takes either should take both: the first makes the join cheap, the
second makes it rare, and the numbers above are what either would be measured
against.

**Rebake before running anything.** `ROUTING_VERSION` is 4 since N4, so every
artifact baked before it is refused and the shard does not boot.
`cargo run --release -p openshard-movement --bin openshard-navigation-bake --
--facet 0`, 11.7 s.

**What would block it:** nothing.
