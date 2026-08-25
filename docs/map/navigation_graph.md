# Automatic navigation graph

> **Status: built — the decision record for the graph as it stands.** Its regions,
> portals and the both-directions rule are what the shard bakes today.
> [`navigation_spans.md`](navigation_spans.md)'s N4 rebuilds it over spans and
> makes its edges directed. Entry point: [`map_rebuild.md`](map_rebuild.md).

## Decision

Long-distance routing is an automatically derived coarse navigation graph. Its
regions are bounded 32×32 rectangles; actual graph transitions are derived only
from valid terrain crossings on their borders. The bound is intentional: the
first topology-only rectangle implementation produced 1,355,438 nodes on
Britannia because every tree and coastline corner changed an exact row run.
A forest must not be more expensive than a city simply because it has trunks.

The graph is built once from the static `Terrain`, which intentionally contains
neither doors nor placed obstacles. Every actual step is still refined through
the caller's live terrain. This keeps the established client/server door policy:
the graph can suggest a doorway, and only the caller decides whether the body
may open it or must stop at it.

## Graph construction

1. Scan static terrain into the **standing places** each column holds — every
   surface the map's spans offer, so a bridge deck and the road under it are two
   places rather than one cell at one height. A shared side becomes a portal
   where `step_allowed` succeeds, **one direction at a time**: a crossing is
   directed and its reverse is a separate edge that may or may not exist, because
   the step rule is asymmetric by design and a ledge a body may step off but not
   climb back onto is ordinary ground. The step rule is still the only thing
   asked, so a graph edge cannot invent a height transition or a diagonal corner
   cut. See [`navigation_spans.md`](navigation_spans.md)'s N4 for what this
   replaced — one sampled height per tile, and a portal only where both ways
   succeeded — and what it measured.
2. Partition the facet into bounded 32×32 regions. Blocked cells inside a
   region remain blocked and are handled by exact intra-region pathfinding;
   they do not create graph nodes. Thus an internal tree creates zero nodes,
   while a wall or shore affects only the real crossings at a region border.
3. For each maximal contiguous run of valid crossings shared by two regions,
   create one portal. A portal has one midpoint transition when narrow and two
   endpoint transitions when wide. The transitions are vertices of the actual
   indexed graph. An inter-edge crosses the portal; an intra-edge is an exact
   low-level route through one navigation region.

The graph is sparse and its density is bounded by coarse borders, rather than
by the number of corners in static art. It has one level.

## Query and refinement

Endpoints join the transitions in their own navigation region. A* runs over the
indexed transition graph, then existing `find_path` refines each graph segment
against live terrain. Every segment fits inside a bounded coarse region, so it
stays within the normal low-level planning budget.

If live terrain rejects a portal, the query excludes that portal and searches a
bounded number of alternative graph corridors. A block that splits the interior
of a static region remains a caller-side refusal: the static graph is not
rebuilt for live doors, crates or mobiles. The client then falls through to its
existing doors-open attempt, which cuts the resulting route at the real refusal.

## G1 — the graph follows a patch

> **Queued, and it is the last third of
> [`what_a_change_costs.md`](new_map_representation/what_a_change_costs.md)'s
> S3.** The other two thirds are built — the span bake and `WorldMap`'s statics
> both follow a publish locally now — and this is the one artefact that still
> does not: `FacetState::publish` **drops** the router, because a whole-facet
> rebuild is 11.6 s on a tick and a graph of the world as it stood is a router
> planning through a wall somebody just built. Dropping it costs long routes
> until the shard is rebaked and restarted. This is the plan for the third
> answer.

**It is the same fix, for the third time.** The span index and the statics run
each held a facet-wide packed array addressed by a prefix sum, and a prefix sum
*is* an ordering: re-laying one block moved everything after it. Both were
answered by a table of `base` and `count`, a rebuilt piece appended and its entry
repointed. This graph holds two such arrays, and neither is a table:

| | addressed by | what one rebuilt region moves |
|---|---|---|
| `region_nodes` | `region_offsets`, a prefix sum over regions | every later region's nodes |
| `edge_targets` / `edge_costs` | `edge_offsets`, a prefix sum **over nodes** | every later node's edges |

So the first two steps are mechanical and are exactly what the other two
artefacts did.

**The third is not mechanical, and it is this graph's own: a `NodeId` is an
index, and other regions' edges point at it.** `edge_targets` holds node numbers,
so a rebuilt region that renumbered its nodes would silently repoint its
neighbours' edges at somebody else. What makes that tractable is that the bake
already interns a node by *place* — `build_nodes: BTreeMap<(x, y, z), NodeId>`,
kept only while building and then dropped. A local rebake has to keep that
identity instead: **a place that still has a node keeps its number**, a place
that lost one leaves a dead entry, a new place takes a number at the end.

**The area, and it is two rings rather than one.** Sampled from what the span
layer's N8 found one scale down, which is the same mistake in miniature:

1. The **regions covering the touched chunks, grown by one tile** west and north
   — a column's height is the average of the four cells meeting at its north-west
   corner, so an edit is read by the column before it. Their places, components,
   portals and intra-region edges are all rebuilt.
2. Their **eight neighbours**, because a portal is a fact about a *border*: the
   node on the far side of A|B belongs to B, and a border that gained or lost a
   crossing changes B's node set — which changes what B's intra-region routing is
   between.
3. And the ring beyond that, for **edges only**. C's portal edges into B are
   rebuilt because B's were; C's intra-region edges are not, because C's own
   places and node set did not move. This is where the cascade stops, and saying
   so is half of what this node is.

**What must not change**: the router refuses a stale graph rather than answering
from it, so the rebake belongs beside `Ground::publish` in `FacetState` — the
same seam that already writes the ground and its span bake in one statement.

**Done when** `FacetState::publish` rebuilds the coarse graph instead of dropping
it, in time a tick can absorb; and **a facet patched into shape routes exactly as
the same facet baked whole** — the differential oracle
[`publish_locality`](../../crates/common/movement/tests/publish_locality.rs) is
for spans, and this wants its equivalent over `find_long_path`, because a graph
that agrees node-for-node is not the claim and a graph that agrees *route*-for-
route is.

## Out of scope

- Multiple graph levels. A single automatic graph is enough for this pass.
- Hand-authored waypoints or map-editor metadata.
- Live rebuilding for doors, crates, portals or housing changes.
