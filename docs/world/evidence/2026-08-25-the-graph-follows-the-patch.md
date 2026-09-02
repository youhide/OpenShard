# The graph follows the patch

S3's third artefact, from [`what_a_change_costs.md`](../../../plans/world/what_a_change_costs/PLAN.md)
and planned as [`navigation_graph.md`'s G1](../design_navigation_graph.md#g1--the-graph-follows-a-patch).
A publish now rebakes the coarse graph around the edit instead of dropping it.

## Where it stands

**Built, at both ends.** Measured by the test that produced the number it
replaces, [`publish_cost`](../../../crates/common/movement/tests/publish_cost.rs),
on Felucca under the profile a `cargo run` builds:

| | before | after |
|---|---:|---:|
| the coarse graph, on the shard's tick | dropped — 28.0 s to rebuild whole | **80 ms** |

`NavigationGraph::rebake_chunks(footing, chunks)` is the whole of it, and the
callers are `FacetState::publish`, `FacetState::undo` and the client's
`ground_moved`. Two prefix sums became tables of `base` and `count` — the third
time this repo has made that change, after the span index's `BlockTable` and
`WorldMap`'s `blocks`.

- **There is one construction.** `NavigationGraph::build` *is* `rebake_regions`
  over every region of the facet. A facet patched into shape and a facet baked
  whole are the same code over the same ground, so they cannot drift; the
  evidence that the restructuring is faithful is that Britannia comes back with
  **71,545 nodes and 416,122 edges**, which is what `navigation_spans.md`'s N4
  recorded.
- **A `NodeId` is an index other regions' edges point at**, so the interning
  table is seeded with the nodes the graph already holds: a place that still has
  a node keeps its number, a place that lost one leaves a dead entry, a new place
  takes a number at the end.
- **The two rebuilt rings are owed different work.** The regions whose ground
  moved are rebuilt whole. The ring around them keeps its *intra-region* edges —
  its places did not move — and floods again only when its node set comes back
  different. That is 111 ms → 80. Their borders are all walked either way,
  because that is what recovers a ring region's node set, and it is why the ring
  beyond them is sampled at all.
- **A run that did not change is not written**, which is what keeps a brush
  stroke from manufacturing garbage out of publishes that moved nothing.
- **Garbage is the span layer's rule and a different answer.** Never compacted
  during a session until the dead outweigh the live — and then a **repack**, one
  walk of what is live, rather than the facet-wide bake `SpanIndex` answers with,
  because a facet-wide bake is the thing this node exists to stop paying.
- **`OSNAV` format 6** carries the tables as they stand, dead entries and all, so
  saving a graph a publish has already moved is the same operation as saving one
  off a bake. A version 5 artifact is refused by name; re-baking is the migration.

**And S2's one caller, which was waiting for the editor tree to settle.** The
client's publish path calls `RadarCache::moved` where it called `set_revision`,
with the map-chunk → radar-chunk coordinate change beside it. That closes
[`what_a_change_costs.md`'s S2](../../../plans/world/what_a_change_costs/PLAN.md)
whole.

## What was decided, and against what

- **Unify the bake rather than write a second path.** The alternative was a local
  rebake beside the whole-facet one, sharing helpers. It would have been a
  smaller diff and two implementations of "what this facet's graph is", with a
  differential oracle as the only thing holding them together. The unified path
  makes the oracle a check rather than a load-bearing rope.
- **The oracle is the graph's *shape*, not its routes.** The unit tests compare
  places and the costs between them, because a `NodeId` is an index and two
  constructions may number the same places differently. This is stronger than
  route parity: two graphs offering the same crossings at the same prices propose
  the same corridors by construction, where route parity passes on a graph that
  quietly lost a portal no sampled query asks about.
- **Eight neighbours, not four.** A corner-diagonal region shares no border with
  the region that moved, so four would do — saying so is a claim about the portal
  pass that the set does not have to make, and the plan named eight.
- **The widening is argued, not caught.** No scene turns "grow the first ring by
  one tile west and north" into an observably wrong answer: over bare land this
  step rule climbs any height at all — a slope is walkable, the two-unit limit is
  a rule about statics — so a land edit does not disconnect ground, and the
  component split that would carry a change two regions out cannot be built out
  of one. What the growth buys is the *invariant*: every region whose places moved
  is inside the rebuilt set, which is what makes a border between a rebuilt region
  and a merely-sampled one unable to mint a node — the claim `intern_node` fails
  loudly on. It is also the rule the span layer already follows one scale down.
- **A dead node stays an entry in `nodes`.** Unreachable rather than absent, and
  counted, so that "what a publish left behind" is a number the repack rule reads
  rather than a shape somebody has to walk to find out.
- **`region_at` and `region_containing` are two questions.** Where a place is, is
  a fact about the grid; whether anything stands there is a fact about the ground.
  Asking the first through the second is how a dying node's edges become
  unfindable half way through a rebake — it cost one panic in a test before it
  was separated.

## What is next

- **80 ms is the price of the *chunk*, not of the edit.** A chunk is 64 tiles and
  a region is 32, so one `.setland` names four regions before the growth, nine
  after it, and twenty-five with the ring. The shard's publish holds the *tiles*
  the patch names — `Patch::touched_chunks` is derived from them — so naming
  those instead would put the first ring at one region and roughly halve the area.
  The client cannot: a chunk off the wire is its unit. That asymmetry is the whole
  of the argument for doing it, and against.
- **S4 — the log is folded**, which is the next node of era S and needs S1's
  minted world id, already built.
- The two remaining bakes keyed to a `MapRevision` are the interiors flood and the
  occluder measurements; neither is a per-chunk artefact and neither is S3's.

## Not ours, seen in passing

`cargo test --workspace` is red in two client tests —
`world::tests::a_diagonal_step_asks_for_the_leaf_on_each_flank_as_well_as_the_landing`
and `dst::a_double_doorway_entered_on_the_diagonal_is_not_a_rubber_band`. Both are
about which door leaves a diagonal step needs, both are untouched by anything
here, and the tree has uncommitted work in `direction.rs`, `walk.rs` and
`world.rs` from the session that is fixing them.
