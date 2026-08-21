# Automatic navigation graph

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

1. Scan static terrain into walkable cells, preserving the resolved point each
   cell represents. A shared side is a portal only when `step_allowed` succeeds
   in both directions, so a graph edge cannot invent a height transition or
   diagonal corner cut.
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

## Out of scope

- Multiple graph levels. A single automatic graph is enough for this pass.
- Hand-authored waypoints or map-editor metadata.
- Live rebuilding for doors, crates, portals or housing changes.
