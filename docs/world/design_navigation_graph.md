# Automatic navigation graph

> **Status: built — the decision record for the graph as it stands.** Its regions,
> portals and the both-directions rule are what the shard bakes today.
> [`navigation_spans.md`](design_spans.md)'s N4 rebuilds it over spans and
> makes its edges directed. Entry point: [`map_rebuild.md`](../archive/world/map_rebuild.md).

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
   cut. See [`navigation_spans.md`](design_spans.md)'s N4 for what this
   replaced — one sampled height per tile, and a portal only where both ways
   succeeded — and what it measured.
2. Partition the facet into bounded 32×32 regions. Blocked cells inside a
   region remain blocked and are handled by exact intra-region pathfinding;
   they do not create graph nodes. Thus an internal tree creates zero nodes,
   while a wall or shore affects only the real crossings at a region border.
3. For each maximal contiguous run of valid crossings shared by two regions,
   create one portal. A portal has one midpoint transition when narrow, and when
   wide it has its two ends **plus one every `PORTAL_SPACING` crossings between
   them**. The transitions are vertices of the actual indexed graph. An
   inter-edge crosses the portal; an intra-edge is an exact low-level route
   through one navigation region.

   **The spacing is a bound on a detour and it was measured.** Until
   `ROUTING_VERSION` 5 a wide run had only its two ends, so a 32-tile border of
   open ground was crossable at its corners and nowhere else and a body in the
   middle of a region paid up to sixteen tiles to reach one. On open country
   that costs almost nothing — a long route amortises it — and on a click at a
   building it cost 32% at the p95, because a roof fails the bounded search for
   a reason that is not distance and so reaches the graph however near it is.
   See `docs/world/README.md`'s findings 25 and 29, and
   [`plans/world/pathfinding/PLAN.md`](../../plans/world/pathfinding/PLAN.md)'s
   P1 for the four numbers the spacing was chosen on.

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

> **Built.** The last third of
> [`what_a_change_costs.md`](../../plans/world/what_a_change_costs/PLAN.md)'s S3,
> and the artefact that used to be **dropped** on every publish: a whole-facet
> rebuild is half a minute on a tick, and a graph of the world as it stood is a
> router planning through a wall somebody just built, so `FacetState::publish`
> took the router away and told the operator to rebake offline.
>
> `NavigationGraph::rebake_chunks(footing, chunks)` is the answer, and both ends
> call it — the shard's `FacetState::publish` and `undo`, and the client's
> `ground_moved`. Measured on Felucca, one chunk, in the profile a `cargo run`
> builds: **28.0 s → 80 ms**, with the graph coming back node for node and edge
> for edge the same as a whole bake (71,545 and 416,122, the counts N4 recorded;
> P1's spacing has since made them 95,672 and 740,339).
> What follows is that plan, with what the doing of it added marked as such.

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

### What the doing of it added

- **There is one construction, not two.** `NavigationGraph::build` *is*
  `rebake_regions` over every region of the facet, so "the facet patched into
  shape" and "the facet baked whole" are the same code over the same ground
  rather than two implementations somebody has to keep agreeing. What that cost
  is the facet-wide sampling pass: a bake holds one `RegionPlaces` per region it
  is working over instead of one array over the whole facet, which is the same
  memory and the same work, and it is what lets the small case exist at all.
- **The oracle is the graph's shape, not its routes.** The unit tests compare
  *places and the costs between them* — a `NodeId` is an index and two
  constructions may number the same places differently — which is strictly
  stronger than route parity: two graphs offering the same crossings at the same
  prices propose the same corridors by construction. Route parity would pass on
  a graph that had quietly lost a portal nothing sampled asks about.
- **The rings are owed different work.** The regions whose ground moved are
  rebuilt whole. The ring around them keeps its **intra-region** edges — its
  places did not move, so its routes stand — and only rebuilds them when its
  node set comes back different, which is what took the measured cost from
  111 ms to 80. Its borders are all walked either way, because that is what
  recovers its node set, and that is why the ring beyond is sampled.
- **A run that did not change is not written.** Most of the second ring comes
  back exactly as it was, and rewriting it would manufacture garbage out of a
  publish that moved nothing there — a brush is a stream of publishes, so this
  is the difference between a session that repacks and one that does not.
- **The garbage rule is the span layer's, and its answer is not.** Dead entries
  are never compacted during a session until they outweigh the live; where
  `SpanIndex` then bakes the facet whole, this **repacks** — one walk of what is
  live, renumbering nodes, because a facet-wide bake is what the node exists to
  stop paying.
- **The file carries the tables.** `OSNAV` format 6: the two prefix-sum offset
  arrays became `base`/`count` runs, written as they stand — garbage and all —
  so saving a graph a publish has already moved is the same operation as saving
  one straight off a bake. A version 5 artifact is refused by name and rebaked.
- **The widening is argued, not caught.** The first ring is taken over the
  chunks' tiles grown one west and north, and no scene turns that growth into a
  wrong answer: over bare land this step rule climbs any height at all, so a land
  edit does not disconnect ground. What the growth buys is that every region
  whose *places* moved is inside the rebuilt set, which is what makes a border
  between a rebuilt region and a merely-sampled one unable to mint a node — the
  claim `intern_node` fails loudly on.

### What is left, measured

**80 ms is the chunk's price, not the edit's.** A chunk is 64 tiles and a region
is 32, so one `.setland` names four regions before the growth and nine after it,
and the ring makes twenty-five. The shard's own publish holds the **tiles** the
patch names — `Patch::touched_chunks` is derived from them — so naming those
instead would put the first ring at one region and roughly halve the area. The
client cannot: a chunk off the wire is its unit, and it is told nothing finer.
That asymmetry is the whole of the argument for doing it, and against.

**The outer ring is sampled to recover a node set, not to answer anything.** A
border between a rebuilt region and a merely-sampled one cannot have changed — it
is argued above, and the sampled side's places and labels are read only so that
the border can be walked again and the rebuilt side's *complete* node list come
out of it. What would remove that cost is knowing which border made each node, so
that a region could keep the nodes of its unaffected borders and rebuild only the
rest. It is not free to know: a place at a region's corner is named by two
borders, and a node dropped because one of them stopped naming it while the other
still does is a node the graph loses silently. Measure before believing it is
worth it — the ring is sampling and component labelling, not floods.

## G2 — the artifact follows the graph

**Built.** G1 made the graph follow a patch; nothing made the *file* follow the
graph, and the two together are what a shard is. The coarse graph is rebaked on
the tick that commits an edit and lives in memory from then on, but the artifact
beside the base set is only ever as new as the last bake — so a shard that was
edited and then restarted met its own artifact one or more revisions behind the
world its log had just rebuilt, and refused to boot:

```
navigation artifact ./felucca-navigation-0.bin is stale: built from map
revision 7, expected 9
```

That is not a rare state. It is the state every edit leaves behind, so the shard
was one `.setland` away from a start that needed a half-minute whole-facet bake
before it would come up again.

**The log is what closes it.** A world of ours *is* the base set plus its log, so
the patches between the artifact's revision and the world's are on disk beside
it — and G1 already knows how to spend them. Boot reads the artifact as far
behind as the log can carry it, unions the chunks the missed patches touched,
runs the same `rebake_chunks` a publish runs, and writes the artifact back.

- **`bake::load_behind`, beside `bake::load`.** Two callers, not two levels of
  strictness: a tool that wants *the* graph of a world, and a shard that holds
  the ground and the log and can rebake the difference. The patch log is the one
  input the two stamps may honestly disagree about — an artifact baked at
  revision 7 was stamped over a shorter log, or over no log at all — so that
  entry is dropped from **both** sides rather than compared leniently. A base set
  that was re-imported or a tile table that moved is refused exactly as before:
  nothing replays those.
- **A file can only say it is *below*.** Whether the log actually holds the
  patches between the two revisions is a question for the log, and the loader
  does not pretend to answer it; `boot`'s `missed_chunks` is where a gap the log
  cannot cover is found, and it ends in the same rebake command as before.
- **Ahead is refused.** An artifact newer than the world it names is a log that
  lost records under a graph, and there is no direction to replay that in.
- **One rebake over the union, not one per patch.** The rebuilt set derived from
  a union contains every set derived from a member of it, and the ground is at
  its final revision either way — the world was loaded by applying the whole log
  before any of this ran.
- **The catch-up happens after the facet is loaded**, through
  `World::catch_up` → `WorldState::catch_up` → `FacetState::catch_up`, which is
  `publish`'s second half without the publish. That is where the facet's span
  index already exists: carrying the graph forward outside the world would mean
  baking a second span index over the same facet to do it.
- **A write that fails is not fatal.** What is in memory is the world as it
  stands; the next start catches up again rather than getting it wrong.

What is *not* built is a shard that writes the artifact as it runs. It does not
need to be: the catch-up costs one rebake per restart and a crash is covered by
the same path, where a save-on-shutdown would leave a killed shard exactly where
this started.

## Out of scope

- Multiple graph levels. A single automatic graph is enough for this pass.
- Hand-authored waypoints or map-editor metadata.
- Live rebuilding for doors, crates, portals or housing changes.
