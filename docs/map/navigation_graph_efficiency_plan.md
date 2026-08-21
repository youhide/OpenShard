# Navigation graph efficiency plan

## Goal

Keep long-distance routing complete and deterministic without making graph
size proportional to every tree, rock, or coastline corner.

The graph remains only a static guide. Every actual step is still refined and
authorized by live bounded pathfinding, so doors and other dynamic obstructions
retain their existing behaviour.

## Implementation status

- [x] Phase 1: compact graph representation and validated format v4.
- [x] Phase 2: component-aware logical entrances with deterministic grouping.
- [x] Phase 4: query-local live-transition cache, shared client planning, and
  opt-in path diagnostics.
- [ ] Phase 3: second hierarchy level. The available synthetic open-world
  probe does not justify level 2; the facet-0 route set remains outstanding.
- [ ] Real-install verification: facet-0 bake/load measurements require the
  client data files and the dedicated 2 GiB cgroup environment.

Validation completed so far: on 2026-08-13 formatting passed, all 109 movement
unit tests, four movement integration tests, and five movement doc-tests passed,
and `openshard-client-app` passed its library tests (179 passed, two diagnostic
tests ignored) and `cargo check`. The full workspace test run is
currently blocked by two existing client-render attachment tests on the
available downlevel adapter: `Rgba32Float` cannot be used as a render attachment
there. Workspace clippy with warnings denied remains blocked by the
pre-existing `clippy::precedence` warning in unrelated
`client-render/src/sprite.rs`. The client-app library test target also passed on
this run: 179 tests passed and two diagnostic tests were ignored.
The repeatable `coarse_bench` probe also passed: on a 1024x1024 open world,
25 coarse searches (including endpoint insertion and live refinement) measured
0.974 ms p95 and 0.981 ms worst, with 1052 steps; graph construction took
385.7 ms. The flat comparison returned 1021 steps in 0.803 ms. This is not a
facet-0 benchmark and is insufficient evidence for a second hierarchy level.

## Backlog

- 🚩 **Review finding (2026-08-13): cache invalidation is keyed only by the
  authoritative item set.** `net_command::entered` intentionally retains the
  plan across mobile-only updates, but the assumption that `WorldView.items`
  is the complete input to `Cluttered::can_step` is not covered by an
  integration test. Enumerate every production update that can alter the
  predicate, assert the invalidation boundary, and measure which remaining
  updates can safely retain the cache.

- 🚩 **Real-install facet-0 measurements are still outstanding.** The current
  workspace has no verified run of the post-ML `7168x4096` bake/load procedure
  inside the dedicated `MemoryMax=2G`, `MemorySwapMax=0` cgroup. The baseline
  numbers above remain the comparison point, but artifact size, peak memory,
  cold-load time, readiness behaviour, and the no-build startup property must
  be re-recorded after the compact graph and component grouping landed.
- 🚩 **Phase 3 needs an end-to-end p95 benchmark before a second hierarchy is
  justified.** The implementation deliberately remains single-level. Measure
  the route set listed in Phase 3, including endpoint insertion and live
  refinement, and add level 2 only if the measured p95 improvement is material.
  The available 1024x1024 open-world probe is below 1 ms p95 and does not
  exercise the required forest, shore, mountain, unreachable-water, or
  narrow-entrance cases.
- ✅ **HUD route conversion now reuses the plan replay.** Resolved on
  2026-08-13: the plan stores immutable landing points produced by its separate
  real and doors-open query caches, and the HUD consumes those points. A
  mutation test proves the replay cannot observe a later terrain snapshot.
- 🚩 **Runtime invalidation needs an integration measurement.** The client
  currently invalidates the plan cache when the authoritative item set changes
  and keeps it over mobile-only updates. Verify that every production change
  capable of affecting `Cluttered::can_step` is represented by that item-set
  comparison, then measure whether keeping the cache across each remaining
  update is safe. This overlaps the cache-invalidation review above and should
  be resolved as one task.
- 🚩 **The render attachment tests need a capable adapter or a portable test
  target.** On the current downlevel adapter, the two `attachment` integration
  tests fail while creating the pre-existing `position` `Rgba32Float` render
  target. Keep this separate from navigation validation and make the render
  harness explicit about its required adapter features before calling the
  workspace suite green.

## Baseline

Measured on 2026-08-12 against post-ML Britannia, facet 0 (`7168x4096`), using
a release build isolated in a Linux cgroup with `MemoryMax=2G` and
`MemorySwapMax=0`:

| Measurement | Exact-row regions | Current 32x32 regions |
|---|---:|---:|
| Bake duration | 603.7 s | 96.3 s |
| Regions | 311,296 | 28,672 |
| Nodes | 1,355,438 | 140,456 |
| Directed edges | 19,221,671 | 2,104,020 |
| Artifact | 513,896,076 bytes | 265,082,856 bytes |
| Bake peak memory | not isolated | about 1 GiB |

The exact-row partition was not sparse topology in practice. Its median region
contained seven cells, 92.3% of regions were one tile wide or tall, and 56.7%
contained no more than eight cells. It effectively emitted topology around
trees and irregular shores.

The current bounded partition fixes the main node explosion: an obstacle inside
a 32x32 region creates no graph node. A tree exactly on a region border can
still split one raw entrance into several entrances, which is addressed below.

The current artifact also carries a much larger problem unrelated to portal
density: its `Vec<Option<RegionId>>` tile lookup costs approximately
`29,360,128 * 8 = 234,881,024` bytes by itself. A regular partition does not
need to store those region ids.

## Grounding in HPA*

The implementation follows the main abstraction from Botea, Müller, and
Schaeffer's *Near Optimal Hierarchical Path-Finding*:

- divide the grid into bounded rectangular clusters;
- identify maximal obstacle-free entrances on adjacent cluster borders;
- put one transition in a narrow entrance and two at the ends of a wide one;
- precompute optimal intra-cluster distances between transitions;
- optionally group lower-level clusters into larger higher-level clusters.

The paper deliberately uses no semantic map labels and states that the same
algorithm handles forests, irregular obstacles, and buildings. A forest is not
made into solid terrain: its trees remain low-level obstacles inside a cluster.
Higher abstraction levels make the forest a cheap macro-area without erasing
its real connectivity.

The paper used 10x10 base clusters for its tested maps, a width threshold of 6
for choosing one versus two transitions, and 2x2 grouping at higher levels.
Those are measured parameters, not constants to copy onto a `7168x4096` UO
facet. OpenShard keeps equal cost for cardinal and diagonal steps because the
movement protocol charges both the same time; the paper's `1`/`1.42` costs do
not match this game.

Primary source:
<https://webdocs.cs.ualberta.ca/~mmueller/ps/2004/hpastar.pdf>

## Invariants

Every phase must preserve these properties:

1. Static blocked terrain is never made walkable.
2. An abstract inter-edge represents a real mutually valid border crossing.
3. An intra-edge exists only when its target is reachable under static movement
   rules, including height and diagonal corner rules.
4. Live terrain authorizes every refined step.
5. Construction, serialization, loading, and route selection are deterministic.
6. A corrupt, stale, wrong-facet, wrong-dimension, or wrong-version artifact is
   rejected rather than partially used.
7. Normal server, client, and playground startup never build a graph.

## Phase 1: compact graph format v4

This phase changes representation only. It must not change abstract nodes,
edges, costs, or route answers.

### Region lookup

Remove the serialized and in-memory `Vec<Option<RegionId>>`.

For a point `(x, y)`, compute its region arithmetically:

```text
regions_across = ceil(map_width / 32)
region_x = x / 32
region_y = y / 32
region_id = region_y * regions_across + region_x
```

Preserve the distinction between a walkable and blocked tile with a packed
walkability bitset. Facet 0 needs about 3.5 MiB instead of roughly 224 MiB.

### Compact nodes

Store only data that cannot be derived:

```text
x: u16
y: u16
z: i8
```

Derive the node's region from `(x, y)`. Validate coordinates and walkability
when loading.

### CSR adjacency

Replace `Vec<Vec<Edge>>` with compressed sparse rows:

```text
edge_offsets: [u32; node_count + 1]
edge_targets: [u32; edge_count]
edge_costs:   [u16; edge_count]
```

A path wholly inside a 32x32 region cannot need more than 1,023 simple steps,
so `u16` is sufficient for an intra-edge cost. The loader must nevertheless
validate costs and collection bounds before allocation.

### Region node membership

Either reorder nodes by region and store one range per region, or use a second
CSR table with `u32` node ids. Prefer reordering if it does not complicate
deterministic node numbering or route tie-breaking.

### Format requirements

- bump the format version;
- retain the routing-algorithm version independently;
- retain atomic write, checksum, input stamp, facet, and dimensions;
- validate all offsets as monotonic and in range;
- validate exact payload consumption and reject trailing bytes;
- never expand attacker-controlled lengths before checking them against the
  remaining payload.

### Phase 1 acceptance

- built and compact-loaded graphs produce identical routes;
- all existing rejection tests pass;
- facet 0 artifact is at most 40 MiB;
- graph cold load is at most 1 second on the benchmark machine;
- bake stays below 1.5 GiB peak under the 2 GiB hard limit;
- the whole debug shard stays below 1 GiB through readiness.

## Phase 2: component-aware logical entrances

This phase removes portal multiplication caused by isolated obstacles located
exactly on a 32x32 boundary.

### Why raw entrance runs are insufficient

The paper defines an entrance as one maximal contiguous obstacle-free border
segment. Consequently, one tree on a border splits an otherwise open boundary
into two entrances. This is correct but unnecessarily dense when both segments
connect the same navigable spaces on both sides.

### Component labelling

For each base region, compute strongly connected components under static
movement rules. Strong rather than undirected components are required because
height transitions may be traversable in only one direction.

The labels are bake-time scratch data and do not need to enter the artifact.
A compact per-tile temporary label is acceptable as long as the complete bake
remains below the memory limit.

### Logical entrance grouping

For every valid border crossing, record:

```text
(component_on_first_side, component_on_second_side, crossing)
```

Group crossings by the component pair rather than by raw contiguity. All
crossings in one group connect the same two mutually navigable spaces, even if
single trees divide them along the border.

Choose representatives deterministically:

- one representative nearest the median for a compact group;
- two maximally separated representatives for a wide group;
- stable coordinate ordering for every tie.

Never merge crossings belonging to different component pairs. A real wall that
separates two rooms must retain separate entrances even if both openings lie on
the same cluster border.

### Phase 2 tests

1. Nine hundred trees strictly inside regions add no portal nodes.
2. Moving those trees onto region borders does not make node count grow
   linearly with tree count when both sides remain one component.
3. A wall dividing a region into two components retains independent entrances.
4. Two disconnected gates on a border retain both when their component pairs
   differ.
5. One-way height transitions are not merged as if they were mutually
   reachable.
6. Randomized maps retain static reachability parity with exhaustive A*.
7. Every returned route replays successfully through `step_allowed`.

### Phase 2 acceptance

- node count on facet 0 does not exceed the current 140,456;
- border-heavy synthetic forests demonstrate bounded portal growth;
- no randomized reachability regression;
- route length regression is measured and reported, not assumed;
- bake and runtime remain within Phase 1 memory limits.

## Phase 3: measure a second HPA* level

Do not add hierarchy merely because the paper describes it. First benchmark
current graph searches over representative long routes:

- Britain to Yew through forest;
- Britain to Minoc around mountains and shore;
- long open-area route;
- unreachable destination across water;
- routes crossing many narrow entrances;
- randomized reachable endpoint pairs across facet 0.

Record median, p95, and worst abstract-search time plus nodes expanded. If the
current single-level search is already negligible beside live refinement, stop
here.

If it is not negligible, prototype level 2:

- compare grouping base regions by 2x2 and 4x4;
- retain only nodes on the higher-level cluster boundary at level 2;
- compute higher-level intra-edges as shortest paths through the level-1 graph;
- insert query endpoints through their containing clusters;
- search at the highest useful level and refine through level 1, then live
  terrain.

Adding a higher level must not introduce new route-quality loss: every
higher-level edge represents an existing shortest lower-level graph path. The
base entrance representatives remain the only source of abstraction error.

### Phase 3 acceptance

Add level 2 only if it produces a material p95 improvement after including
endpoint insertion and refinement, while keeping:

- artifact at most 50 MiB;
- bake below 2 minutes on the benchmark machine;
- bake peak below 1.5 GiB;
- identical route results between built and loaded graphs.

## Phase 4: runtime transition cache for live refinement

### Goal

Keep the static graph as a long-distance guide, but stop repeating the same
runtime neighbour queries while endpoint costs, portal alternatives, and route
refinement inspect a live 32x32 region. No individual path query may exceed the
50 ms interactive-walk limit.

### Current cost

`MapTerrain::can_step` reads the in-memory map and tiledata on every attempted
step. It recomputes land heights, scans statics, checks tile flags, and then
`Cluttered` checks dynamic items. A diagonal `step_allowed` also checks both
cardinal flanks. This is not disk I/O, but it is repeated work.

The static `NavigationGraph` does not cache these live transitions: its portal
and intra-region data describe the bare map, while doors and placed objects
must remain authoritative at runtime. Endpoint-to-portal A* therefore repeats
many of the same `can_step` calls for each portal candidate.

### Query-local cache

Introduce a terrain wrapper owned by one route query/frame. Cache the complete
answer for:

```text
(from.x, from.y, from.z, direction) -> Some(landing) | None
```

Use separate cache instances for:

- real terrain, with doors and placed objects as they stand;
- doors-open terrain, used only to identify a route that reaches a closed door.

The wrapper delegates all non-movement `Terrain` methods. `step_allowed` and
the diagonal flank checks go through the cached `can_step` answer. The cache is
read-only after an entry is created and is discarded after the query/frame;
terrain snapshots are never mixed.

### Consumers

Route all of these through the same cache instance:

1. ordinary bounded A*;
2. local endpoint-to-portal costs;
3. abstract-route refinement;
4. route replay/append validation for the same plan;
5. HUD route conversion when it belongs to the same frame plan.

Do not cache across a terrain update until runtime invalidation is explicitly
designed. A changed door, item, or mobile must not reuse an old `Some` result.

### Instrumentation

Record per plan and per terrain half:

- `can_step` calls before the wrapper;
- cache hits and misses;
- unique transition entries;
- A* nodes explored;
- total query time and the hard-deadline exit reason.

Keep logging opt-in through `OPENSHARD_PATH_DEBUG`; never log each transition
in normal operation.

### Tests and acceptance

1. Cached and uncached routes have identical directions and reachability.
2. Closed-door, opened-door, diagonal-corner, slope, and multi-floor cases
   retain their existing answers.
3. Real and doors-open caches cannot return each other's answers.
4. Repeated portal searches demonstrate cache hits for the same transitions.
5. The problematic three-door-house query stays below 50 ms, with no single
   A* or long-path query exceeding the hard deadline.
6. Movement and client tests, randomized static-map parity, formatting, and
   release checks all pass.

## Adaptive or semantic regions

Do not initially introduce a separate semantic `Forest` region type.

The movement rules currently assign no extra traversal cost to forest ground;
only individual blocked tiles matter. Marking a whole forest solid would reject
valid routes, while assigning a special cost would invent gameplay policy.

An adaptive quadtree or topology-derived navmesh remains a possible later
experiment, but it has additional risks:

- irregular cluster boundaries complicate deterministic entrance generation;
- very large regions can exceed bounded live-refinement budgets;
- narrow corridors and one-way height transitions need explicit preservation;
- dynamic endpoint insertion becomes less predictable;
- it is harder to place a strict upper bound on preprocessing work.

The component-aware entrance scheme already gives the desired semantic effect:
a large connected forest behaves like one navigable macro-area, its trees stay
local obstacles, and only connectivity-changing approaches become graph
choices.

Revisit adaptive regions only if the compact component-aware graph still misses
the artifact, bake-time, or query-time targets.

## Real-install verification

Every completed phase ends with the same release procedure:

1. Run all movement tests and route-parity properties.
2. Run formatter, targeted clippy with warnings denied, and workspace compile
   checks.
3. Bake facet 0 in a dedicated Linux cgroup with:

   ```text
   MemoryMax=2G
   MemorySwapMax=0
   ```

4. Record wall time, CPU time, `MemoryPeak`, regions, nodes, edges, and bytes.
5. Cold-load the artifact into a shard under the same hard memory limit.
6. Confirm readiness logs contain graph loading and no navigation build phases.
7. Start the playground and confirm the client loads the same artifact.
8. Preserve the previous valid artifact until the new file passes validation;
   atomic rename remains mandatory.

## Implementation order

1. Compact computed region lookup and walkability bitset.
2. Compact nodes, region membership, and CSR adjacency.
3. Land format v4 tests and run the real-install benchmark.
4. Add bake-time component labels.
5. Group logical entrances by component pairs.
6. Run forest, wall, height, randomized, and real-install verification.
7. Benchmark query latency.
8. Add a second hierarchy level only if the benchmark justifies it.

## Terminal outcome

The work is complete when:

- graph density is controlled by meaningful connectivity, not isolated art
  corners;
- facet 0 artifact is compact enough to load cheaply;
- bake and runtime stay safely below 2 GiB without swap;
- valid long routes remain reachable and live terrain remains authoritative;
- server, client, and playground load one shared artifact and never rebuild it
  during normal startup.
