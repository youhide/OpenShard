# 2026-08-23 — N4: regions over spans

Era P's fifth node, in one commit, and it is the one the plan was written for.
[`navigation_spans.md`](../design_spans.md)'s N4 retires the **one-storey
defect**: the coarse graph sampled one height per tile — `ground_z`, the land
alone — so Britain's castle plateau was an island in a graph whose own map said
otherwise. **`coarse_bench`'s `refused_but_walkable` is now 0 in every band from
every one of the five recorded origins**, where the castle alone used to refuse
37 of 44. Nothing lost a route, nothing changed one, and the bake got *faster*:
96 s → 11.7 s.

> **⚠ Rebake before running anything.** `ROUTING_VERSION` is 4, so every
> artifact baked before this is refused — and the shard does not boot, it
> errors. `cargo run --release -p openshard-movement --bin
> openshard-navigation-bake -- --facet 0`, 11.7 s.

## Where it stands

The same bench and the same five origins, run **interleaved** over the old
artifact and the new one so the workstation's drift moves both. Flat A\*, whose
code did not change at all, is the control and does not move.

| origin | walkable of sampled | refused but walkable |
|---|---|---|
| (1363, 1600, 30) the castle plateau | 44 of 45 | **37 → 0** |
| (1434, 1699, 2) the bank | 43 of 45 | **5 → 0** |
| (1828, 2745, 0) Trinsic | 36 of 42 | **1 → 0** |
| (600, 2100, 0) Skara Brae | 15 of 35 | 0 → 0 |
| (1500, 1900, 0) open country | 38 of 42 | 0 → 0 |

**Every before-number reproduced to the unit** — the run was made by putting
`ROUTING_VERSION` back to 3 for one build so the old artifact would load, with
the router itself untouched, so the two columns differ only in the file.

**37 destinations gained an answer, 0 lost one, and the seven the old graph
already answered came back with identical route lengths** in both interleaved
passes. This added answers rather than moving them.

**The bake: 96 s → 11.7 s.** Artifact 8,527,862 → 7,441,177 bytes, 85,310 →
71,545 nodes, 567,412 → 416,122 edges.

**`cargo check`, `clippy --all-targets` and `fmt --all` are silent.** `cargo
test --workspace --no-fail-fast` ends with the same two red tests in
`openshard-state` and no others — R1's finding, still filed under [*`can_step`
does not check the corner*](../../roadmap.md).

## What the node decided

**A graph node is a place to stand, and the whole span list is kept.** Not the
reachable part of it: `check` only ever answers with a span's own `stand_z`, so a
column's spans are a **superset of every landing** the step rule can produce over
the bare map — and that is what the passes need rather than a nicety, because a
flood that stepped somewhere the graph had no place for would stop dead and call
the ground unreachable. Keeping a surface nothing can climb onto costs nothing in
exchange: the component pass is over *directed* steps, so such a place is its own
strong component with no edge into it, and no route is planned through one. The
only filter is `can_step` asked of the place itself, which is what drops a column
the live world has walled off.

**The key is the height, at this end too.** N3b corrected the plan's
`(x, y, span)` to `(x, y, z)` and warned N4 to carry it. Carried: a bake keyed by
span alone is a graph the live world could never be placed into. The graph is
baked from the bare map and the overlay is applied when a hop is refined, which
is the property N3b said to keep deliberately.

**In-degree over places is not bounded by the eight directions.** Out-degree is —
one landing per direction — and the builder's fixed `[_; 8]` neighbour arrays
assumed the same of the other side. **A stair is exactly the shape that breaks
it**: the low place and the high place of one neighbouring column land on the
same tread. It panicked on the first stair scene written against it, which is the
best thing a wrong bound can do. The incoming half is counting-sorted into one
run per place now.

**A place is one node, however many entrances name it.** A directed portal means
the two ways across one border are two logical entrances, and a corner where a
vertical and a horizontal border meet is two more. Interning is what keeps a
symmetric border costing exactly what it always did — and the old builder did not
intern, which is part of why 85,310 nodes became 71,545.

**The bake was paying eight times over for every neighbour.** `component_labels`
and `region_costs` asked `step_allowed` once per direction, and `step_allowed`
is *defined* as one slot of `steps_out_of` — so each asked for the whole
expansion eight times and used one answer of it. That is most of 96 s → 11.7 s,
and it is N3's own primitive arriving in the bake.

## What was found

Filed in [`navigation_spans.md`](../design_spans.md)'s *Out of scope,
named*.

- **🚩 The done-when cannot see the directed half.** Baking the same places with
  the *old* both-ways requirement puts `refused_but_walkable` at **0 from all
  five origins too** — the spans alone do everything this bench can measure.
  Directed edges are real on the facet (**5,903 of 103,774 portal edges have no
  reverse**, and they cost 5,176 nodes and 72,841 edges) but no sampled
  destination needed one. They are asserted instead in
  `a_ledge_is_a_portal_one_way_and_no_portal_the_other`, over a walkway of
  statics — the test the terrain-seam work deleted for want of ground that could
  carry it, owed back and now paid.
- **🚩 The routing cost roughly doubled, and the mechanism is measured.** On the
  seven routes both graphs answer: 1.29 → 4.39 ms p50 in one pass, 2.02 → 3.85 in
  the other. `local_costs` joins an endpoint to the graph with **one exact search
  per node in its region**, at both ends of every query, and a node it cannot
  reach costs the whole budget before saying so. **The castle's own region went
  from 18 nodes to 51** while the facet total fell 16% — the cost lands where the
  storeys are, which is where the new answers are. Filed rather than fixed:
  2.6–5.6 ms against a 50 ms deadline, and the repair is a design question
  (bound the fan-out by distance, or cut it to the endpoint's own *component* —
  a label the bake computes and then throws away).
- **The same eight-times-over flood is still in two diagnostics.**
  `coarse_bench`'s own `land_component` (6 s a flood, 12.8% of the facet) and
  `Scene::reachable`, which is every scene fixture's oracle. Neither is on a hot
  path; both are one line.
- **The graph's `walkable` bitmap is still one bit per tile**, and `region_at`
  still ignores z. Deliberate — it is what lets an endpoint with a z nobody
  promised join the graph at all, and `path::goal_node` resolves the height
  afterwards — but it means the bitmap cannot say *which* storey is walkable.
  Anything reading it as an answer about a place rather than about a column is
  reading it wrong. N7 is the next caller.
- **Bumping `ROUTING_VERSION` stops the shard from booting and only warns the
  client.** `Err` in `boot.rs`, a printed line in the client's `lib.rs`. That is
  the right loudness for a graph that would otherwise answer with a one-storey
  world; it is recorded because it is a deployment step, not a defect.

## What is next

**[N7 — the server reads the graph](2026-08-25-the-span-layer.md#n7--the-server-reads-the-graph)**,
inherited from [`terrain_seam.md`](../research/terrain_seam.md)'s F, whose precondition
N4 has just spent. Server AI plans with flat `find_path` at a budget of 400, so
a creature still cannot route across a town while a **correct** artifact sits
loaded and validated in `FacetState.coarse` with a test for its only reader. The
client already falls back past 8 tiles through `steer::Ground::path`;
`step_toward` gains the same fall-back. Its done-when is a test that walks a
creature a distance flat A\* at budget 400 cannot, from a raised origin as well
as a flat one — and the raised origin is the half that would have passed for the
wrong reason before this node.

**What N7 should know.** The join cost above is per node in the endpoint's
region, and a creature re-planning every beat pays it every beat. If N7's
measurement is unhappy, the `local_costs` entry in *Out of scope, named* is
where to start rather than the budget.

**Nothing forces N5 or N6.** N5's content is deliberately empty until a flood
says what the spans still cannot connect, and that flood is N5's own first step.
N6 is gated on a number nobody has asked for.

**What would block it:** nothing.
