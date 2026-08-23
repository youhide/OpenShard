# The first storey

> **Status: live — era P. N0, N1, N2, N3, N3b, N4 and N7 are built.** The gate is
> gone: [`realtime_map.md`](realtime_map.md)'s era R is over, the span layer is
> built and measured against the whole facet, and **the shard now walks on it**
> — a node expansion is 208 ns where it was 1,105 and a search from the castle
> is 0.168 ms where it was 0.793. N3b then spent that on the answer: **a node is
> a place to stand rather than a tile**, so a route may pass over a bridge and
> later under it, and *"from this house's ground floor to its first floor"* is a
> route round the staircase instead of success with an empty route. **N4 has
> retired the defect this plan was written for**: the coarse graph samples spans
> and its edges are directed, so `refused_but_walkable` is **0 in every band
> from all five recorded origins** where the castle plateau alone used to refuse
> 37 of 44. **N7 has put it under a player**: the shard reads the artifact, and
> a creature rounds a town block the exact search cannot see past. **Nothing in
> this plan is open** and every finding with a defect behind it is repaired —
> what is left is N5 and N6, both gated rather than queued. See
> [`map_rebuild.md`](map_rebuild.md) for the order and
> [`handoffs/`](handoffs/) for where the work stands.

Two defects were found in one session and they are the same omission seen from
two sides. The coarse graph
[models one height per tile](terrain_seam.md#-the-coarse-graph-is-a-one-storey-model-of-a-two-storey-world),
so a castle plateau reached by static stairs is an island in a graph whose own
map says otherwise. And a search
[spends all of its time re-deriving the step rule](terrain_seam.md#-a-is-not-what-a-search-spends-its-time-on)
from raw statics — 1,462 ns per node expansion, of which A\*'s own machinery is
within noise of zero.

`NavigationGraph` is HPA\* — Botea, Müller and Schaeffer, cited by
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md)'s
own grounding section — and HPA\* is an abstraction **over a cheap grid**. It
assumes the layer beneath it is fast and already knows what a floor is. That
layer does not exist here. This is the plan for it.

The reference is Recast & Detour (zlib; Unreal's navigation is built on it,
Unity's NavMesh baking descends from it), and two of its stages are what
matters:

- **`rcCompactHeightfield`** gives each column a *list of spans* — open
  vertical intervals — rather than one height. A raised courtyard is its own
  span, not a disagreement with the land under it, which is why a navmesh has
  never had our castle-plateau defect.
- **Off-mesh connections** are explicit links for what geometry does not imply:
  ladders, jumps, doors, teleports. A stair between two spans falls out of the
  spans themselves; anything that does not is *declared* rather than inferred.

Track: [`README.md`](README.md) · The seam it lands under:
[`terrain_seam.md`](terrain_seam.md) · The graph it rebuilds:
[`navigation_graph.md`](navigation_graph.md) ·
[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md)

## What the facet actually holds

Counted, not assumed, because every storage decision below turns on the
distribution rather than the total.
[`span_census`](../../crates/common/movement/examples/span_census.rs) on facet 0
(`7168×4096`, 29,360,128 columns, 458,752 blocks of 8×8), 3.5 s:

| | |
|---|---:|
| standable surfaces, for a walker | **7,986,741** |
| …of which **the land surface** | 7,704,411 (**96.5%**) |
| …of which **come from statics** | **282,330** (3.5%) |
| surfaces only a swimmer stands on | +15,063,491 |
| columns holding **no statics at all** | **27,031,492** (**92.1%**) |
| blocks whose whole 8×8 holds none | **338,008** (**73.7%**) |
| columns with any *standable* static surface | 209,247 (0.71%) |
| blocks with any | 21,684 (4.7%) |
| longest column | **12 spans**, at (1544, 1528) |

And the shape of the distribution, which is the decisive part:

| spans in a column | columns | |
|---:|---:|---:|
| 0 | 21,617,515 | 73.63% |
| 1 | 7,566,737 | 25.77% |
| 2 | 128,067 | 0.44% |
| 3–12 | 47,809 | 0.16% |

**99.4% of columns hold nothing or one thing, and the deepest column on the
facet holds twelve.** A layout that pays per column, or that reserves for the
worst case, is paying for a world that is not there.

## What the census decides

**Three tiers, because the map has three populations.** Not a compression
scheme applied to a uniform structure — three genuinely different cases, each
answered by the cheapest thing that can answer it:

| | population | what answers it |
|---|---:|---|
| **the block** | 73.7% of blocks hold no statics | one flag: every column here is bare ground |
| **the column** | 92.1% of columns hold no statics | the land surface is the only surface, and nothing obstructs above it |
| **the exception table** | 7.9% of columns (2,328,636) | a span list, CSR |

The middle tier is why this is not a trick. A column with no statics has exactly
one standable surface, it is `average_land_z`, and there is nothing above it to
duck under. Storing a span for it would be storing what the land grid already
answers — 96.5% of all surfaces on the facet — while the *expensive* thing
today is not finding the surface but **proving there are no statics to
consider**, which costs a `statics_at` (15.4 ns: two `partition_point`s and a
pointer chase into a per-block `Vec`) on every one of a node's sixteen calls.
One bit replaces that proof.

**A span is four bytes.**

```rust
struct Span {
    /// Where a body's feet rest on this surface.
    stand_z: i8,
    /// The edge a step must reach to climb onto it — `stand_z` for everything
    /// but a climbable platform, whose surface is halved and whose top is not.
    reach_z: i8,
    /// Free height above `stand_z`, saturating at 255. `can_fit` is a compare.
    clearance: u8,
    /// Today: whether only a swimmer stands here.
    flags: SpanFlags,
}
```

`reach_z` and `stand_z` are separate because `platform_surface` already returns
both and they differ on a bridge; folding them would be inventing a rule the
step check does not have. `clearance` is the other half of
[C's leftover](terrain_seam.md#c--the-doubles-become-scenes-): `check_ground`
wants *"is anything in this body"* without *"and is there a surface"*, and a
clearance byte is exactly that question answered once at build time.

**Water is a flag, not a second grid.** A swimmer's surfaces are 15 million
more, and every one of them is a bare ocean column whose height is
`average_land_z` and whose wateriness is one `tiledata` land flag — so they cost
*nothing* under the tiers above and need no storage at all. What the query does
is filter: the structure offers a surface, and the asker's own ability rejects
it. That keeps `swimming` where [D put it](terrain_seam.md#swimming-is-an-argument-now-because-the-thing-it-sits-on-is-the-query)
— on the query, scoped to one asker — instead of forking the artifact in two.

**Estimated resident size: under 20 MB.** 1.8 MB of block index, 7.4 MB of
per-block count tables over the 120,744 blocks that have statics, and ~9.6 MB of
spans. Against the ~150 MB the facet already costs, and against the 117 MB a
faithful `rcCompactCell` (four bytes per column, every column) would take — which
is why Recast tiles its heightfield and never holds a world in one. The census is
what let this be a table rather than a tiling problem. **N1 replaces the estimate
with a measurement.**

**There is no artifact in the first three nodes.** The census computed the whole
distribution — two `stand_surfaces` per column over 29.4 million columns — in
3.5 s, and building the tiers needs strictly less than that: one pass over the
statics to mark non-bare columns, and a span list for the 7.9% that are. The
baking that is actually expensive is the **region graph**, which is already
baked and took 96 s — **11.7 s since [N4](#n4--regions-over-spans)**, which
rebuilt both of its hot passes on the way past. So the span layer is built at
load until something
measures otherwise, and [N6](#n6--an-artifact-if-a-measurement-asks-for-one) is
where that measurement lives. Machinery this plan does not need yet is machinery
it does not mint.

## The shape

```rust
find_path(ground: &Spans, over: &Overlay, doors: Doors, ...)
```

`Spans` answers *movement*: where a body may stand, what it may step onto, and
what it fits under. `MapTerrain` keeps everything that is not a step —
`land_tile`, `statics_at`, `sight_clear`, `ceiling`, and placement's `can_fit`
over arbitrary heights — because those are questions about the world rather than
about a walk, and none of them is on the A\* edge.

This supersedes [`terrain_seam.md`](terrain_seam.md)'s E in exactly one
argument: E ends at `find_path(&MapTerrain, &Overlay, Doors)`, and the
measurement says the search should not take a `MapTerrain` at all. **The split E
draws survives untouched** — a baked static half and a live `Overlay` on top —
and it is the right split for the same reason it always was: the overlay is the
part that changes between ticks and therefore cannot be baked. What changes is
what the static half *is*.

**Nothing here starts until [`terrain_seam.md`](terrain_seam.md) closes** —
which it since has, and the gate moved rather than lifted: it is now
[`map_rebuild.md`](map_rebuild.md)'s R1 and R2, for the same reason spelled out
in [where a session starts](#where-a-session-starts). The reason it is a decision
rather than a dependency graph falling out that way is unchanged. What this plan
substitutes is *one argument* of a call that terrain_seam's E was in the middle
of creating:

```rust
find_path(&MapTerrain, &Overlay, doors, …)   // where the seam ends
find_path(&Spans,      &Overlay, doors, …)   // where this plan ends
```

Written against the tree as it stands, a span search would be written against
`&dyn Terrain` — the thing E deletes — and then rewritten by hand against the
API it should have had from the start. Written after, it is a type substitution
into a signature with exactly one shape, and the `&Overlay` half it composes
with is finished rather than moving.

**The measuring did not wait and should not have.** Every decision this document
takes — the three tiers, the four-byte span, water as a flag, the ×4 expectation
and its ×6.4 ceiling — was settled before E began, by
[terrain_seam's node 0](terrain_seam.md#0--the-oracle-) and by
[the census](#what-the-facet-actually-holds). Deciding early and building late is
the whole shape of this: the cost of waiting is a few weeks of a slow search, and
the cost of not waiting is writing the same file twice.

**What this plan inherits when the seam closes**, by name: `MapTerrain` as two
borrows built per question, `Overlay` and `Doors` as the one live-world type both
ends build, `WorldState` owning the tile table outright, no trait on the search,
and `CachedTerrain` already deleted rather than left to be measured again.

## What this is worth, and the ceiling it runs into

A ratio quoted without its limit is a ratio that ignores its own arithmetic, so
the floor was measured before the estimate was made.
[`step_cost`](../../crates/common/movement/examples/step_cost.rs) runs a search
over open ground walled off from its goal — the budget is spent in full and
`can_step` is one integer compare — leaving nothing on the clock but the binary
heap, the two `FxHashMap`s and the closed set:

| | ns/node |
|---|---:|
| a real search on facet 0 today (601 nodes, `budget/far` p50, three origins) | **1,213 – 1,477** |
| …of which **pure A\***, terrain taken away | **222 – 234** |
| …so terrain is | **~85%** |
| what a span read should cost (8 neighbours, one land read each) | ~100 – 150 |

**So the honest expectation is ×3.5 to ×4 on the search, and the ceiling is
×5.3 to ×6.4.** No amount of work on terrain can pass that, because at the limit
every node still costs 230 ns of A\* machinery. What lands is roughly

```
   now   1,477 = 1,247 terrain +   230 A*
after N3   ~380 =   ~150 terrain +   230 A*
   limit   230 =      0 terrain +   230 A*
```

**N3 landed, and the estimate above was low on the terrain half and right about
the shape.** A whole node expansion is **208 ns** where it was 1,105 on the same
machine on the same day — ×5.3, against the ×5.3–6.4 named as the *ceiling* —
and a real search is 0.168 ms for 601 nodes where it was 0.793, which is ×4.7
and 280 ns a node. The estimate said ~150 ns of terrain and got ~208 including
the overlay's own two checks; it said ×3.5–4 on the search and got ×4.7. The
reason it beat its own estimate is the hoist below, which N3 took *as well as*
the table rather than instead of it: the two compose, and neither alone is what
the row says.

Two things follow, and the second matters more than the first.

**The hoist is not the plan's competitor.** Computing `start_surface` once per
expansion instead of sixteen times is 1,372 → 523 ns on a node, measured — ×2.6
on the terrain half, ×1.9 on a whole search. It is most of what a span grid
gets, for a day's work and no new structure. It was still not taken *instead*,
for the reason [terrain_seam](terrain_seam.md#-a-is-not-what-a-search-spends-its-time-on)
gives: it repairs a query that should be a table lookup, and it does nothing at
all for [N4](#n4--regions-over-spans), which is the node with a defect behind it.
**N3 took it as well** — `steps_out_of` is the hoist, and the table is what it
calls — because once the rule is a lookup, asking for it sixteen times instead
of eight is simply asking twice.

**Speed is not the capability.** A faster node makes a *refusal* cheaper; it does
not turn one into a route. Four destinations in five from a town street are
refused today at budget 600, every one of them by the budget rather than by the
map — and ×4 on the clock leaves that number exactly where it is. What changes it
is the coarse router answering the long routes flat A\* cannot, which is N4, and
which is currently wrong on raised ground. **The user-visible win of this plan is
N4; N1–N3 are what make N4 correct and affordable.**

### Past the ceiling, if it is ever worth it

Both moves are standard and both are out of scope here, named so the ceiling is
not mistaken for a wall:

- **The hash maps are the 230 ns.** `cost`, `came_from` and `closed` take about
  seventeen hash operations per node. A search bounded to 600 nodes has a bounded
  window, so a generation-stamped dense array over its own bounding box replaces
  all three — the textbook move, and worth perhaps 30–50 ns/node, which would put
  the ceiling near ×20 instead of ×6.
- **JPS** (Harabor & Grastien, *Online Graph Pruning for Pathfinding on Grid
  Maps*) attacks the **number** of nodes rather than their cost, which is the
  other axis entirely. It wants a uniform-cost grid with cheap neighbour queries
  — which is precisely what this plan builds, and precisely what does not exist
  today. It is the move *after* N3, never before it.

## The live half, and what it does not carry

The overlay is the half a bake can never hold, so what it holds decides what the
bake is *allowed* to assume. Read off the workspace, not off the design:

| | where it lives | |
|---|---|---|
| **a door** | an entity, `Blocks { door: true }` | ✅ **already right** |
| **a placed crate, a house wall** | an entity, `Blocks` | ✅ already right |
| **a hull** | a plank, `Blocks` | ✅ already right |
| **a moored deck** | a plank, `Stands` | ✅ already right |
| **another mobile** | the client's index only | ⚠️ **the two ends disagree** |
| **a house floor or stair** | **nowhere** | 🚩 **nothing can stand on it** |

**Doors are the case that needs nothing.** A door is an entity and the doorway it
hangs in is *an open gap in the statics by construction* —
[`overlay.rs`](../../crates/common/map/src/overlay.rs)'s header says so and
it is why this works. A door therefore never reaches the bake, cannot be baked
shut, and its two readings stay the `Doors` enum. The span grid is simply blind
to doors, which is the correct relationship.

**Mobiles are an open decision and this plan does not take it.** The server does
not index them at all — two mobiles may stand on one tile — while the client's
`Clutter` inserts every mobile at `PLAYER_HEIGHT` so that a *drawn* route does
not pass through an NPC, and says so in its own comment. E left them out of
`Overlay` deliberately (identity is the server's and the client has none to
offer), so "a body in the way" is currently a client-side courtesy rather than a
rule. Nothing here changes that, and nothing here should: it is a gameplay
decision about whether bodies block, and it belongs wherever that is taken.

### 🚩 Nothing but a ship can be stood upon

`grep -rn "CoverKind::Stands" crates` has **one** producer in the whole
workspace: [`Plank::cover`](../../crates/server/state/src/boat.rs). Every other
live thing — a crate, a house wall, a house *floor* — is either `Blocks` or
absent.

So a placed multi contributes walls and nothing else. Its ground floor is
walkable because the map's own ground is underneath it; **its upper storey has
no surface at all**, because a floor ten units up is neither in the client's
statics (a player house is a runtime entity) nor in the overlay (nothing emits a
standable cover for it).

[`housing.md`](../housing.md) has the question in its backlog and frames it one
step short: *"a two-storey house has two floors over one tile and the step check
has to pick the one the walker is on"*. The step check has nothing to pick
between — and the same document's D-notes state the assumption this contradicts,
that folding only blocking components into the footprint *"keeps a floor and a
roof walkable"*. It keeps them **un-blocked**, which is not the same as
standable, and the difference is invisible at ground level and total above it.

**This is housing's defect, not this plan's** — it is true today, with no spans
anywhere near it. It is named here because a span grid baked from client files
will contain no player house either, so once N3 lands it will *look* like a
pathfinding regression, and because it fixes the shape of what the overlay owes:
a house floor is the general case of what `aboard` does for one ship, and
`CoverKind::Stands` is already the right type for it.

## What a node is, and the z that is already gone

Asked in a session and worth answering in the document, because the answer is
half *already done* and half *a defect nobody had written down*: **does a search
node need a z at all?**

**It does not, and it already has none.** The flat search keys its three hash
tables on [`PathTileKey(u32)`](../../crates/common/movement/src/path.rs#L399),
which is `x << 16 | y` — a planar tile. The resolved landing point, z and all,
travels as a *value* in `came_from`. So z is data on the edge rather than the
identity of the node, and that is the right shape: without it there is no
`MAX_STEP_UP`, no `PLAYER_HEIGHT` of headroom, and no
[`check`](../../crates/common/movement/src/terrain.rs#L294) picking *the highest
surface in reach*, which is how a staircase is climbed at all.

**🚩 What nobody wrote down is what a planar key costs.** A column with two
standing places gets **one** slot in `closed`: whichever surface is reached first
wins, and the other is unreachable for the rest of that search. A bridge and the
ground under it are one node. A moored deck and the water beside it are one node.
Since [`map_rebuild.md`](map_rebuild.md)'s R3 landed, a house's ground floor and
its first floor **are** one node. The coarse graph's one-storey defect — the reason
this plan exists — has a quieter twin in the fine search, and both are the same
sentence: *a tile is assumed to be one place.*

**The census says the repair is nearly free**, which is the whole argument of
[the tiers](#what-the-census-decides) applied to the search instead of to
storage:

| surfaces in a column | columns | |
|---:|---:|---:|
| 0 or 1 | 29,184,252 | **99.40%** |
| 2 | 128,067 | 0.44% |
| 3–12 | 47,809 | 0.16% |

So the node becomes **`(x, y)` plus a span index that is zero for 99.4% of the
facet**, and it still fits the integer fast path the current key exists for:
Felucca is 7,168×4,096 — 13 bits of x, 12 of y — and the deepest column on the
facet holds twelve spans, which is 4 bits. Twenty-nine bits of a `u32`.

**This lands in [N3](#n3--the-search-takes-spans)**, because that is where the
search reads spans and the index becomes a thing it can name. It is recorded here
rather than there because it is a statement about what a node *is*, and because
the pre-span search has the defect today.

### Down is not up, and the graph does not know

The step rule is **already asymmetric**: a climb reaches `start_top + 2`, and a
descent is unbounded — `check` accepts any platform whose top is within reach,
including one far below, and the land branch does the same. Stepping off a
platform you cannot step back onto is therefore ordinary behaviour, not a special
case wanting a mechanism.

The **coarse graph cannot represent it.**
[`navigation_graph.md`](navigation_graph.md) makes a portal only where
`step_allowed` succeeds *in both directions*, so a one-way drop is invisible to
long-distance routing. That is the conservative side of the error — the graph
refuses a route rather than promising one that does not exist — but it is a
refusal, and refusals are what [F's measurement](terrain_seam.md#-and-the-artifact-is-wrong-before-anyone-reads-it)
found the graph already handing out too many of.

**[N4](#n4--regions-over-spans) built directed edges**, then: a portal joins two
places in one direction, and the reverse is a separate edge that may or may not
exist. **5,903 of facet 0's 103,774 portal edges have no reverse** — every one of
them a crossing the old rule deleted. What that leaves
[N5](#n5--off-mesh-links) is what it always should have been — links geometry does
not imply *at all*, a teleporter and whatever the flood says is still unreachable
— rather than the place a drop would have been declared by hand.

## The nodes

```
 terrain_seam.md ✅ ──> map_rebuild.md R1 + R2 ──┐
 (the signature)       (the map, in one type)   │
                                                ▼
 N0. the census ✅ ──> N1. three tiers ✅ ──> N2. the step rule reads them ✅ ─┬─> N3. the search takes Spans ✅
                                                    (the agreement oracle)   │        └─> N3b. the node stops
                                                                             │              being a tile ✅
                                                                             │
                                                                             └─> N4. regions over spans ✅ ──┬─> N5. off-mesh links
                                                                                          │                  │
                                                                                          │                  └─> N7. the server reads the graph
                                                                                          └─> N6. an artifact, if measured           (inherited from F)
```

N0 is done, and it is the one node that ran before the gate — a census reads the
map and writes nothing, so it could not be written against the wrong API.

### N0 — the census ✅

**Done.** [Above](#what-the-facet-actually-holds).
[`span_census`](../../crates/common/movement/examples/span_census.rs) is kept
rather than deleted: a base set or a second facet has its own distribution, and
the tier boundaries are only right for a world that has been counted.

### N1 — three tiers ✅

**Built**, in [`spans.rs`](../../crates/common/movement/src/spans.rs), and the
structure is what this section described: the block tier is the map's own empty
block, the column tier is the land grid read live, and the exception table is
CSR — a per-block base and a `[u8; 64]` of counts, addressed by the land's own
`BlockIndex` so there is no second block indirection to keep in step. The
builder asserts the count fits a byte and that a height fits an `i8` rather
than truncating either. Nothing reads it yet.

**The measurement, against the estimates this document made before it:**

| | estimated | measured |
|---|---:|---:|
| resident | under 20 MB | **16.5 MiB** (17,305,856 B) |
| …block index | 1.8 MB | 1.8 MB (458,752 × `u32`) |
| …count tables | 7.4 MB | 8.2 MB (120,744 × 68 B) |
| …spans | ~9.6 MB | **6.5 MB** (1,635,392 × 4 B) |
| spans stored | ~2.4 M | **1,635,392** |
| build time | "less than the census's 3.5 s" | **0.05 s** |
| equivalence | — | **0 disagreements**, 29,360,128 columns × 2 abilities, 2.0 s |

The span count came in a third under the estimate because the estimate counted
an exception column as at least one span: a column with statics whose land is
water or mountainside stores no ground span, and a column whose statics are all
walls stores only the ground. The build time is 70× under, which is what settles
[N6](#n6--an-artifact-if-a-measurement-asks-for-one) for now — a fiftieth of a
second is not an artifact's worth of work.

**Two types, where this section wrote one.** `SpanIndex` is the bake: owned, no
lifetimes, built at facet load and kept beside the map the way `NavigationGraph`
is. `Spans` is the view a question is asked through — the index, the map, and
the ability of the asker — built where it is asked, which is exactly
`MapTerrain`'s shape. The split is forced by the middle tier rather than
chosen: the bake deliberately does not store the 92% of columns the land grid
can answer, so answering needs the map in hand, and a bake that borrowed the map
could not be stored beside it in `FacetState`. ~~`find_path(&Spans, &Overlay,
…)` is unchanged as the shape N3 lands.~~ **It is not the shape N3 landed**: a
step still needs `MapTerrain` for `obstructed`, `can_fit`, `sight_clear` and
`start_surface`, so the footing stayed the carrier and the bake went *inside*
the terrain, as a third borrow that cannot be omitted. See
[N3](#n3--the-search-takes-spans).

**Done when** — and it is: `Spans::surfaces(x, y)` returns exactly what
`stand_surfaces` returns for every column of facet 0 and both abilities. The
oracle is the [`span_index`](../../crates/common/movement/examples/span_index.rs)
example, which is also where the table above comes from; it is an example rather
than a test for [`span_census`](../../crates/common/movement/examples/span_census.rs)'s
own reason, that 29.4 million columns walked twice by two implementations is
seconds in release and minutes in debug. `cargo test` carries the same oracle
over a box of Britain, which runs in four seconds and takes all three tiers.

### N2 — the step rule reads them ✅

**Built**, as [`Spans::check`](../../crates/common/movement/src/spans.rs), and
the risk this plan was carrying is retired: `check`'s answer does **not** depend
on the source in any way a per-span bake cannot carry. It reaches the source
through exactly the two scalars this section expected, `start_z` and
`start_top`, and the whole of the rule is now a walk of the target column's
stored spans — highest first, first acceptance wins, which is the same choice
`check` expresses as a running maximum over the map file's own order.

**The measurement, on facet 0:**

| | |
|---|---:|
| steps compared (every surface × both abilities × eight directions) | **248,268,125** |
| …of which landed somewhere | 238,291,149 |
| **disagreements** | **0** |
| flood from (1363, 1600, 30), map rule | 3,747,934 tiles in 4.2 s |
| flood from (1363, 1600, 30), span rule | **3,747,934 tiles** in 2.9 s |
| tiles reached by one flood and not the other | **0** |

Both oracles are the [`span_check`](../../crates/common/movement/examples/span_check.rs)
example, which is where this table comes from; the suite carries the per-step
half over a box of Britain, which is 1.9 M steps and runs in a third of a
second. The scene sweep beside it is the one that runs without an install, and
both were checked to *bite* — disabling the `landCheck` clause fails each of
them, which is the property an oracle is worth having.

**A node expansion's landing half**, on facet 0 around (1500, 1900), fastest of
five passes over 10,836 standable tiles — `step_cost`'s own rows, all three with
the same checksum:

| | ns per tile |
|---|---:|
| 8 × `step_allowed` — what a search does today | 1070.2 |
| the same, landings computed once, over the map | 366.1 |
| **the same, landings off the bake** | **169.1** |
| pure A\* with the terrain taken away | ~220 |

The 1,462 ns [`terrain_seam.md`](terrain_seam.md#what-one-search-costs) records
is the same measurement on a different run; the trio above is internally
consistent because it is one run. What it says is that the landing half is now
**under** what A\*'s own machinery costs — which is the point at which N3's
question stops being "is the terrain the cost" and starts being "what is left".
The start half is still the map's, and that is the next paragraph.

**Three clauses became three flags**, and each is a property of the *column*
rather than of the body or of where it came from:

- **The reach test** is `step_top >= item_top`, and `item_top` is `reach_z`.
  Carried by the record N1 already stored, as expected.
- **The obstruction test** is carried by `clearance`, and the byte is exact —
  **with one correction to what N1 wrote.** N1 argued that a saturated 255 could
  only mean "nothing above", because a base and a `stand_z` are both `i8` and so
  a gap can never exceed 255. That is true up to the boundary, and the boundary
  is reachable: a static based at 127 over a surface at −128 is a real gap of
  exactly 255, and it answers differently from "nothing above" for a body that
  needs more than 255 over its feet — which is a body that walked in more than
  239 above where it is landing. `SpanFlags::CEILED` is what separates them, and
  it costs a bit of a byte that had seven spare.
- **The ServUO `landCheck` guard** is two flags rather than one, and the second
  is what removes the land read this section budgeted for. `SpanFlags::LAND_WINS`
  carries three of the guard's four conditions — the static's near edge against
  the land's centre, and that centre against where a body would stand on it.
  `SpanFlags::GROUND` marks the column's own land span, and the fourth condition
  (`test_top > land_z`) is then a comparison against **that span's `reach_z`**,
  which is the tile's lowest corner and is already in the column's own cache
  line. The first condition, `land_is_ground`, needs no storage at all: it is
  whether the ground span survives the asker's ability filter, which is exactly
  what "water is a surface only to a swimmer" already means.

That last one is the shape worth keeping. The residue this section expected to
pay a map read for turned out to be a read of the *column*, because the column
already stores the land it is standing on — which is the reason N1 gave for
storing an exception column's ground where a bare column's is not stored, one
node before anything needed it.

### N3 — the search takes `Spans` ✅

**Built.** Needed N2, and [`terrain_seam.md`](terrain_seam.md)'s E for the other
half of the signature. What the shard walks on is the bake, and what that is
worth is one table:

| facet 0 around (1500, 1900), 10,836 standable tiles, fastest of five | ns |
|---|---:|
| one node expansion, one direction at a time — **what a search did before this** | 1105.5 |
| the same eight answers, landings over the map, work hoisted | 364.1 |
| the same, landings off the bake | 167.1 |
| **`steps_out_of` — what a search does now**, overlay included | **208.1** |
| pure A\* with the terrain taken away | ~183–191 |

**5.3× a node**, and the terrain half of a node is now the same size as A\*'s
own machinery rather than seven times it. One search, from the three origins
[`terrain_seam.md`](terrain_seam.md#what-one-search-costs) recorded, 37,248
destinations each:

| origin | arrived @400 | arrived @600 | p50 @600 was | p50 @600 now |
|---|---:|---:|---:|---:|
| (1363, 1600, 30) the castle plateau | **4,036** | **4,436** | 0.793 ms | **0.168 ms** |
| (1434, 1699, 2) the bank | **6,138** | **7,389** | 0.851 ms | **0.170 ms** |
| (1500, 1900, 0) open country | **17,458** | **18,093** | 0.570 ms | **0.150 ms** |

**Every arrival count is the recorded one, to the unit**, and so is every
per-class node distribution — `goal/region` 111/453, `goal/far` 165/558 from the
bank, unchanged. A faster search that found different routes would be a
different search; this one found the same ones.

**`start_surface` stays on the map, and the measurement is why.** The section
below set out three ways and asked for a number. It is **23.3 ns of a 170.8 ns
node expansion** — one seventh, because it is asked once against the landing
half's eight — so baking it could save at most that, minus what reading a baked
one costs, against a fourth height on every one of 1.6 million spans. And there
is a second reason the plan did not know: **`start_surface` is order-dependent
in a way a span list cannot reproduce.** Its loop keeps a *running maximum* over
the column's statics in the map file's own order and accumulates `z_top` over
everything that passed on the way, so a climbable whose surface is low and whose
art is tall is selected in file order and skipped in descending-surface order —
which is the only order spans are stored in. Baking the start half means storing
the file's order too, and that is not a fourth byte, it is a different table.

**What the search calls is `steps_out_of`, and `step_allowed` is one slot of
it.** A node expansion is now one call that resolves the tile being stepped
*off* once and answers each neighbour once — sixteen landing checks become
eight, because the four cardinals a diagonal asks about as flanks are the four
it was already asking about as destinations. `step_allowed(footing, from, dir)`
is defined as `steps_out_of(footing, from)[dir]`, so there is one rule and no
second copy to drift; a caller that wants one direction pays for the expansion,
which is the price of not having two rules.

**The bake is not optional where the map is.** `MapTerrain::new` takes a
`&SpanIndex` as its third argument, so there is no way to build a terrain that
would silently re-derive every column, and `Footing::of` panicked rather than
accept a map without one. It sat *beside* the world rather than inside it —
`FacetState::spans` on the shard, `Resources::spans` on the client — for a
reason that is worth writing down because it is not a preference:
`openshard_map` is underneath `openshard_movement`, and where a body may stand
is a movement rule, so [`World`](../../crates/common/map/src/world.rs) cannot
hold the projection of its own two layers. `FacetState::set_map` was the one
seam that moved both, and `World::with_tiles` rebakes every facet already
loaded, so the builder's argument order cannot produce a bake over the empty
tile table.

> **Since corrected: the pair is one value, and the panic is gone.**
> [`Ground`](../../crates/common/movement/src/ground.rs) is a `World` and the
> `SpanIndex` over its base, private fields and three functions that write both
> in the same statement — so "a facet with a map and no span bake over it" is a
> state nothing can spell rather than a state `Footing::of` notices. The
> layering argument above is unchanged and is why it *wraps* the world instead
> of the bake moving down: the bake reads `MAX_STEP_UP` and `PLAYER_HEIGHT`, and
> pushing those into `openshard_map` is the move R2 refused when `Cover::meets`
> asked for it. `FacetState` and `Resources` each hold one and neither hands the
> inner world back out. See
> [the handoff](handoffs/2026-08-23-the-ground-and-its-bake-are-one-value.md).

**`MapTerrain::check` is now an oracle and nothing else.** No production caller
reaches it: `can_step`, `land_at`, `surface_at` and `predict_step` all read
`Spans::check`. It stays because it is the only statement of the rule *in terms
of the map files*, and the `span_check` example is 248 million comparisons of
one against the other — an equality nothing would notice losing if both sides
were the same code. Do not delete it as dead.

**What the other rules do is unchanged.** `MapTerrain::obstructed`, `can_fit`
and `sight_clear` still walk the column's statics, and `Footing` still carries a
`MapTerrain`: this node moved the *landing* of a step, which is what a search
asks sixteen times, and not the three rules that ask a different question. So
`find_path(&Spans, &Overlay, …)` — the signature this section was written
around — is not what landed. The footing is still the carrier, because four
rules on it still need the tile table, and the bake travels inside the terrain
rather than beside it.

**N3 had to re-arm N2's own oracle, and that is the shape of every node after
this one.** `span_check`'s coarse half flooded the facet twice — once through
the shipped `step_allowed` and once through a written-out span rule. The moment
`step_allowed` reads the bake, that flood compares the bake against itself and
reports zero differences for the wrong reason. Both sides are now written out in
the example — `map_land` beside `span_land`, identical but for which `check`
answers the landing — and the oracle still holds over the whole facet:
**248,268,125 steps, 0 disagreements; both floods reach 3,747,934 tiles, 0 tiles
differing** (the map flood 4.0 s, the bake flood 2.9 s when this was written;
2.8 s and 1.5 s since the flood hygiene pass folded the corner rule into one
`expansion` over both). A test that calls the shipped rule stops being a test of
the shipped rule the moment the rule moves under it.

**And the composition is asserted, not assumed.**
`the_live_world_adds_takes_away_and_hangs_a_door_over_baked_spans` in
[`walk_scenes.rs`](../../crates/common/movement/tests/walk_scenes.rs) walks onto
a `Stands` cover the bake has never heard of, is refused by a `Blocks` cover in
its own span, and is refused by a shut door under `Doors::AsTheyStand` and
admitted under `AllOpen` — over a `Scene`, which carries a real bake and keeps
it in step with its own map. **All three claims were checked to bite**, by three
mutations of `walk.rs` run one at a time: dropping the overlay's floor, dropping
its veto, and ignoring the door reading each fail exactly the claim they should.


### N3b — the node stops being a tile ✅

**Built.** Needed N3, and it was deliberately not part of it: N3's oracle is
that nothing about the routes changed, and this is the one change that must
alter them. The key is now a **standing place** — the tile *and* the height a
body's feet are at on it — so a column with two floors gets two slots in
`closed`, a route may pass over a bridge and later under it, and a body on a
house's first floor is not the same node as one on the ground beneath.

**The key is `(x, y, z)`, not `(x, y, span)`.** The section below asked for a
span index in the twenty-nine spare bits of the `u32` the search already used,
and that cannot be the key: **the surfaces a search lands on are not all the
map's.** A house's storey, a ship's deck and a placed stair are the
[`Overlay`](../../crates/common/map/src/overlay.rs)'s, `walk::climbed` picks
them, and none of them has a span to be indexed by — a span is a fact about the
map file. What both layers do speak in is the height, because a *landing is* a
height; so the key is `x`, `y` and `z` in forty bits of a `u64`, which also
means no coordinate has to be truncated to fit. Two surfaces of one column at
one height are one place to stand, which is the identity a walk wants anyway.
The `u32` and its span index are not a smaller version of this; they are a
different, narrower answer.

**A destination is a point and a node is a place, and resolving between them is
half the node.** Comparing nodes without it would have swapped one wrong answer
for another: almost no caller has the exact z of the surface it means — the
coarse graph's nodes carry the land's height under the bridge they mean the deck
of, `map_path_probe` sweeps a neighbourhood at the height its *origin* stands
at, a client's click carries whatever the tile it hit was drawn at — and every
one of them arrived before, because arrival threw the height away.
`path::goal_node` resolves the caller's z against what is actually there, the
map's spans and the live world's surfaces together, the way
`Overlay::surface_at` resolves one; ties go to the lower surface so the answer
does not depend on which layer was read first. **The start's own column offers
the start's own height**, because a body standing somewhere is proof that there
is somewhere to stand — without it a search from a place to itself would go
hunting for a surface the world does not list.

**What it changed, enumerated.** The same three origins and 37,248 destinations
each, run before and after in one tree:

| origin | arrived @400 | @600 | destinations that changed |
|---|---:|---:|---:|
| (1363, 1600, 30) the castle plateau | 4,036 → **4,010** | 4,436 → **4,405** | 26 and 31 |
| (1434, 1699, 2) the bank | 6,138 → **6,091** | 7,389 → **7,315** | 47 and 74 |
| (1500, 1900, 0) open country | 17,458 → **17,458** | 18,093 → **18,093** | **none** |

**Open country is bit-identical**, which is the control: nothing there stands on
anything. Of the 178 answers that did move, **176 are columns with more than one
place to stand on them** — the probe prints that attribution itself now, and it
is a count of a set rather than a count. The two that are not are named, because
a plan that says "exactly the multi-span columns" has to account for them:
(1403, 1718) and (1402, 1719) from the bank at budget 400, single-surface both,
whose goal used to be found on the **400th node** and now falls one node outside
the budget — the search spent a node or two on the second height of a column on
the way. Neither is lost at 600, and every one of the 74 that is lost at 600 is
multi-span. **That is the budget being spent differently, not the rule
answering differently**, and it is filed below as what a node budget now counts.

**A refusal replaced a lie, which is what the arithmetic above is.** Every one
of those 176 is a destination the search reached the *column* of and could not
reach the *place* on — a bridge whose deck it got to when the ground was asked
for, or the reverse. Before this node each one was reported as an arrival with a
route that ends somewhere else, and the worst of them was the same column the
body already stood on: `start == goal` compared tiles, so *"from this house's
ground floor to its first floor"* returned **success with an empty route** and
the caller stood still believing it had arrived. Server AI told to walk to a
mobile standing on a bridge over it was told it was already there.

**And the route that could not exist before now does.** Nothing in UO moves up
in place — the eight neighbours are horizontal and the step rule changes height
as a *consequence* of moving — so a route from one floor of a column to another
is a **loop**: out of the column, up whatever tiles rise, and back over the same
`(x, y)`. `a_route_climbs_from_a_villas_ground_floor_to_its_first_floor` in
[`walk_scenes.rs`](../../crates/common/movement/tests/walk_scenes.rs) plans one
over the same two-storey villa the step rule's own test climbs by hand, and
walks it back through `step_allowed` so the search cannot invent a step nobody
may take. **No tile-keyed search could have produced it**: the first visit closed
the column for good, so the return was forbidden by the search's bookkeeping
rather than by the world.

**The sweep over the facet finds none of those loops, and that is a property of
the sweep.** `map_path_probe` counts them — 0 from every origin — because it
runs over the bare map with an empty overlay, and the map's own multi-span
columns are bridges and piers you walk *along* rather than staircases you double
back on. A house is exactly the shape that produces one, and a house is the
overlay's. The count stays in the probe so that a shard-side sweep can be
compared against it, not because zero is the answer.

**The coarse router is untouched, and it was checked rather than assumed.**
`find_path_until` refining a hop now has to arrive at the graph node's own
height, which is `ground_z` — the land alone — so a hop onto a bridge deck could
have started refusing. It does not: `coarse_bench` reproduces every recorded
number to the unit, **37 of 44 from the castle, 5 of 43 from the bank, 0 of 38
from open country**. What was already broken there is [N4](#n4--regions-over-spans)'s
and is unchanged by this.

**The heuristic stays planar, and it goes flat in exactly the case this node
opens.** Chebyshev over `(x, y)` is admissible — every step moves one tile — but
with the goal one storey up in the same column it is zero at the start, and the
search fans out until it meets a stair. Inside a house that is nothing; inside a
castle it can spend the whole budget. The real answer for a long climb is
[N4](#n4--regions-over-spans), where spans are the graph's nodes and a staircase
is a portal.

### N4 — regions over spans ✅

**Done**, and its done-when is met exactly: `coarse_bench`'s
`refused_but_walkable` is **0 in every band from every one of the five origins**.

`NavigationGraph::build` sampled `ground_z` — the land alone — once per tile. It
samples **places** instead: every standing surface the column's spans offer, so a
bridge deck and the road under it are two nodes, and a region's components, its
portals and its intra-region routes are all over places.

**The node is a place and not a span index**, which is
[N3b's correction](#n3b--the-node-stops-being-a-tile) carried here as it warned
it should be. The graph is baked from the bare map, so a span *would* have
served; the map's surfaces are not all the surfaces, and a bake whose identity
was a span could never have the live world placed into it. The key is `(x, y, z)`
at both ends of the plan now.

**And the whole span list is kept, not the reachable part of it.** `check` only
ever answers with a span's own `stand_z`, so the column's spans are a superset of
every landing — which is what the passes need rather than a nicety: a flood that
stepped somewhere the graph had no place for would stop dead and call the ground
unreachable. Keeping a surface nothing can climb onto costs nothing in exchange,
because the component pass is over *directed* steps: it is its own strong
component, with no edge into it.

**The edges are directed**, the second half of the same repair. A shared side
became a portal only where `step_allowed` succeeded *in both directions*, and the
step rule is asymmetric by design — a climb reaches `start_top + 2` while a
descent is unbounded — so every ledge a body may step off but not back onto was
deleted from the graph. A crossing is now one direction and its reverse is its own
entrance over **interned** nodes, so a symmetric border still costs one node a
side and one edge each way.

#### What it measured

The same bench, the same five origins, run **interleaved** over the old artifact
and the new one so the workstation's drift moves both — flat A\*, whose code did
not change, is the control and does not move.

| origin | | refused but walkable |
|---|---|---|
| (1363, 1600, 30) Britain castle | 44 walkable of 45 | **37 → 0** |
| (1434, 1699, 2) Britain bank | 43 of 45 | **5 → 0** |
| (1828, 2745, 0) Trinsic | 36 of 42 | **1 → 0** |
| (600, 2100, 0) Skara Brae | 15 of 35 | 0 → 0 |
| (1500, 1900, 0) open country | 38 of 42 | 0 → 0 |

**Nothing lost a route, and nothing changed one.** At the castle, 37
destinations gained an answer, 0 lost one, and the seven the old graph already
answered come back with **identical route lengths** in both passes — so this
added answers rather than moving them.

**The bake got faster: 96 s → 11.7 s**, and the artifact smaller: 8,527,862 →
7,441,177 bytes, 85,310 → 71,545 nodes, 567,412 → 416,122 edges. Both hot passes
— the component flood and the intra-region routes — asked `step_allowed` once per
direction, which is `steps_out_of` eight times over; they ask `steps_out_of` once
per place now, which is [N3](#n3--the-search-takes-spans)'s primitive arriving in
the bake. The node count fell because a place is one node however many entrances
name it.

**The routing cost roughly doubled**, and the mechanism is measured rather than
guessed: on the seven routes both graphs answer, the coarse query goes from 1.29
ms to 4.39 ms p50 in one pass and 2.02 to 3.85 in the other. `local_costs` joins
an endpoint to the graph with **one exact search per node in its region**, and
the castle's own region went from **18 nodes to 51** while the facet total fell
16%. That is the shape of the cost: it lands where storeys are, which is where
the new answers are. p50 is 2.6–5.6 ms against `MAX_LONG_PATH_TIME`'s 50 ms, so
it was filed rather than fixed here — and **repaired since**: the join is one
flood over the endpoint's region rather than one search per node of it, which
takes the same seven routes to 0.53 ms p50, below what they cost before N4 made
the region denser. See *Out of scope, named*.

**🚩 The done-when cannot see the directed half.** Baking the same places with
the old both-ways requirement puts `refused_but_walkable` at **0 from all five
origins too** — the spans alone do all the work this bench can measure. Directed
edges are real on this facet (5,903 portal edges of 103,774 have no reverse, and
they cost 5,176 nodes and 72,841 edges) but no sampled destination needed one.
They are asserted instead in
`a_ledge_is_a_portal_one_way_and_no_portal_the_other`, over a walkway of statics
— which is the test the terrain-seam work deleted for want of ground that could
carry it, owed back.

### N5 — off-mesh links

Needs N4. **Declared** edges between spans that geometry does not imply.

A stair is not one of them and this is worth stating plainly, because it is the
question that produced this plan: with spans, a staircase is already a chain of
surfaces at rising heights and `MAX_STEP_UP` already climbs it. **Neither is a
drop off a ledge** — the step rule already allows it and N4's directed edges
already carry it, so a one-way link is inferred geometry rather than a declared
one. What needs declaring is what has no walkable geometry between its ends — a
teleporter, and whatever N4's flood shows the spans still cannot connect.

**The content is deliberately empty until N4 says what is missing.** What N5
owns is the format slot and the rule that a link is declared rather than
inferred; inventing links before the flood names them would be guessing at a
world we can measure.

**Done when:** the flood over the graph and the flood over `Spans` reach the
same set, with whatever links that takes enumerated here.

### N6 — an artifact, if a measurement asks for one

Needs N1. **Gated on a number, not on a preference.**

Build `Spans` at load and measure it, on both ends, on facet 0. If it is inside
the startup budget the shard and the client already accept, there is no artifact
and this node closes as *not needed* — which is a real outcome and the expected
one, since the census did strictly more work in 3.5 s.

If it is not, the machinery exists and is not to be reinvented:
[`bake`](../../crates/common/movement/src/bake.rs) already owns stamps,
atomic writes, checksums, typed staleness errors and base-set support, and
`openshard-navigation-bake` already drives it. The spans then go in **the same
file as the graph**, with the version bumped — never a second artifact, because
a graph and the spans it was built from must not be able to arrive at different
revisions of one world.

**Done when:** the load time is recorded here and the node is closed either way.

### N7 — the server reads the graph ✅

**Built**, and with it [`terrain_seam.md`](terrain_seam.md)'s F is answered: the
baked navigation graph was to be wired up rather than stopped being paid for,
and the repair that had to come first was N4's. `FacetState.coarse` now has a
reader on the shard that is not a test.

`openshard_ai::step_toward` asked flat
[`find_path`](../../crates/server/ai/src/lib.rs) at a budget of 400 and, when it
was refused, walked the straight-line direction — so a pet, an escort or a
townsperson could not route across a town while the artifact that would let it
sat loaded and validated in the facet beside it. A refused exact search is asked
of the graph now, past `COARSE_MIN_DISTANCE` tiles, which is **the same
fall-back the client walks a click by**: the two ends no longer disagree about
how far a body can plan.

**Its done-when is met, in
`a_creature_routes_past_its_exact_budget_over_the_coarse_graph`.** Two corridors
that meet a map away from where the walk starts, so the way through is
eighty-odd tiles *away from* a goal thirty-two tiles off: the exact search
refuses at the budget from both origins — the flat one and one on a walkway of
statics five units up — and the walk arrives from both. The flood is the oracle
that the ground is walkable at all, and the control is **the same facet with no
graph**, where a body walks south and stands at the divider for the rest of the
walk. That control is what the shard was.

**One number, and one shared threshold.** `COARSE_MIN_DISTANCE` was a private
constant in the client's `steer.rs`, with the argument for it written there; it
is `openshard_movement`'s now, beside `find_long_path`, because it is a property
of the *router* — joining an endpoint walks its whole region, at both ends —
and not of either caller. (It cost one exact search *per node* of that region
when N7 was written, which is what made the threshold worth drawing; the join is
one flood now, and the threshold is still a real one because the region is
walked either way.) A fall-back the two ends drew at different distances would
be two answers to "how far can a body plan", which is the disagreement this node
closes.

**The bare map is one value now too.** The graph is baked over the bare map, so
the corridor it proposes has to be read over the bare map: each end used to
build that reading itself out of an empty overlay it kept alive somewhere. It is
[`Footing::guide`](../../crates/common/movement/src/footing.rs) — one empty
overlay for the process — with `world::guide` on the client and
`WorldState::guide` on the shard as its two callers.

## Decisions, taken here

**Three tiers, because the map has three populations.** 73.7% of blocks and
92.1% of columns hold no statics at all. The tiers are not a compression of a
uniform structure; they are the structure the census found.

**A span is stored only where the land grid cannot answer.** 96.5% of the
facet's standable surfaces are `average_land_z`. Storing them would be storing a
second copy of the land, and a second copy is a thing that can disagree.

**Water is a flag on a surface, not a second artifact.** Ability is per-query
since D, so the structure offers and the asker filters.

**One artifact or none, never two.** If N6 ever writes spans to disk they go in
the graph's file under a bumped version. Two files stamped separately are two
revisions of one world waiting to happen.

**The block index is the map's own.** `WorldMap` already blocks by 8×8 and
already knows which blocks hold statics. A parallel index would be a second
thing to keep in step, which is the failure this whole document set catalogues.

**The bake and the view are two types, and the middle tier is why.** `SpanIndex`
is what is built and stored; `Spans` is what is asked, and it holds the map
beside the index because 92% of the facet is deliberately not in the index. A
single type would have to either borrow the map — and then it could not be
stored beside it — or store a second copy of the ground, which is the one thing
the middle tier exists to avoid. Taken in N1; see that node.

**The oracle is equivalence, not plausibility.** Nothing here is done because it
looks right. N1 asserts against `stand_surfaces` over the whole facet, N2
against `step_allowed` and against a whole-facet flood, N3 against bit-identical
node counts, N4 against `refused_but_walkable = 0`. Every one of those tools
exists already.

**This plan waits for the map; the measuring did not.** Every number it is built
on was taken before E began, and none of the code was written until E ended —
and the wait carried over to [`map_rebuild.md`](map_rebuild.md)'s R1 and R2 when
E closed, for the same reason under a different name. An optimisation is not
urgent enough to be worth writing twice.

**No hoisting.** The 2.87× available from computing `start_surface(from)` once
per node expansion instead of sixteen times
[is measured](terrain_seam.md#-a-is-not-what-a-search-spends-its-time-on) and
deliberately not taken. It is a local repair to a query that should be a table
lookup, and taking it would make the table look less necessary than it is.

~~**E first, and only N3 waits for it.**~~ **The map first, and N1 waits too.**
E landed, and what replaced it as the gate reaches further into this plan than E
did: `Spans` is built *from* a `&WorldMap` and a `&TileData`, and
[`map_rebuild.md`](map_rebuild.md)'s R1 moves the second of those into its own
crate while R2 folds the live layer into the first. N3 still waits for the
signature; N1 now waits for what it reads. N4–N7 are unaffected, since a region
graph over spans names neither.

## What this supersedes

- [`terrain_seam.md`](terrain_seam.md)'s **E**, in one argument only: the search
  ends at `&Spans` rather than `&MapTerrain`. The `Overlay` half of E is
  untouched and is a precondition here.
- [`terrain_seam.md`](terrain_seam.md)'s **F**, in its precondition: F says the
  artifact is wrong before anyone reads it, and N4 is what makes it right. F's
  own question — whether `step_toward` gains the fall-back or `FacetState.coarse`
  goes — is still F's.
- [`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md)'s
  **Phase 3**, the second hierarchy level, which was already gated on facet-0
  numbers and is now additionally gated on N4: a second level over a one-storey
  model would be a second level of the same mistake.

## Out of scope, named

- **Residency and tiling.** ~~The estimate is under 20 MB~~ — **measured at
  16.5 MiB in N1**, so the whole facet stays resident and Recast's tiling
  problem does not arise. Residency is
  [direction G](new_map_representation/plan.md#g--residency-and-size-deferred-on-purpose)'s
  either way.
- **N1 found: the count tables are bigger than the spans they address.
  ✅ Fixed.** 8.2 MB of `[u8; 64]` against 6.5 MB of spans, because a block with
  any static in it carried sixty-four bytes whether or not sixty-four of its
  columns held anything. N1 estimated the emptiness at 71% from the census; the
  bake counts it exactly, and it is worse: **1,388,743 of the 7,727,616 cells
  those 120,744 tables address own a run, so 82% of every table was a zero.**
  It was filed twice as *not taken* — first because N3 had not yet said whether
  the query is on a hot path, then because the gain was against a number already
  the size of A\*'s own machinery.
  **The occupancy mask is what it proposed and what was built**: a `u64` bit per
  cell beside the base, counts packed one byte per *set* bit in a facet-wide
  run, and the prefix sum taken over the occupied columns before this one rather
  than over all sixty-four cells — reached with a `count_ones` on a word the
  lookup has already loaded. A column with nothing stored — 82% of them —
  returns on the bit test without touching the counts at all.
  Measured on facet 0, release: the addressing is **3.3 MB where it was 8.2**,
  the whole bake **11,713,607 B (11.2 MiB) where it was 16,603,552 (15.8)**, and
  a landing off the bake **158 ns where it was 180** — two runs of each agreeing
  to the tenth, with `steps_out_of` 200 ns against 218. Smaller *and* fewer bytes
  read, which is what the finding predicted.
  **The answers did not move**: 1,635,392 spans as before, and `span_index`'s
  whole-facet oracle agrees with `stand_surfaces` on all 29,360,128 columns for
  both abilities. Nothing is serialised, so `ROUTING_VERSION` is untouched and
  no rebake is owed. The control for the addressing is
  `the_rank_is_over_occupied_columns_and_not_over_cells`, where a rank taken over
  cells sends the block's last cell sixty-one bytes past the end of its run.
  The packed static record stays under its own gate.
- **N1 found: the map and the overlay disagree about a platform of no
  thickness.** `MapTerrain::is_obstructed` gives one a body from `base` to
  `base`, so it is in the way of anything *below* it whose head passes the
  floor; `Cover::of_static` lays no blocking half for the same art at all, and
  its doc says why. So a floor the map shipped and a floor the shard placed
  answer differently for a body underneath — a cellar under a shipped floor, and
  the same cellar under a built one. The span bake reproduces the map's reading,
  because that is what N2's oracle compares against. **N2 did not settle which
  of the two is right**, and could not have: its whole content is that the
  answer did not change, so the one thing it may not do is change this one. It
  stays open, and **N3 did not settle it either**, for the same reason with a
  different shape: N3's oracle is that the routes did not change, and changing
  which of the two readings wins is a change to the routes. It is now visible in
  one place, which is what N3 was expected to buy — `walk::landing` consults the
  map and the overlay in six lines — so what it needs is a decision rather than
  another node. It is a defect of the *step rule*, not of this layer.
- **N2 corrected N1: a `clearance` of 255 was not by itself "nothing above".**
  N1's argument was that a base and a `stand_z` are both `i8`, so a gap can
  never exceed 255 and a saturated byte must therefore mean an absence. The
  bound is right and the conclusion is off by the boundary: a static based at
  127 over a surface at −128 *is* a gap of exactly 255, and it answers
  differently from an absence for a body needing more than 255 over its feet.
  Fixed in N2 with `SpanFlags::CEILED` rather than argued away, because the
  arithmetic that makes it unreachable on Britannia is not arithmetic this
  layer gets to assume about a base set.
- **N2 found: the guard's residue was a column read, not a map read.** N2's
  section budgeted "one flag bit plus, in the residue, a land read the query
  already knows how to make". There is no land read: the exception column
  already stores its own ground, and the guard's fourth condition is a
  comparison against that span's `reach_z`. The general shape is worth
  remembering for N4 and N5 — a clause that seems to need the map often needs
  the *column*, and the column is already in the cache line the query is
  standing in.
- **N3 found: `start_surface` cannot be baked without baking the map file's
  order.** The plan offered three ways to move the start half and asked for a
  measurement; the measurement said one seventh of an expansion, and the *code*
  said something the plan had not seen. `MapTerrain::start_surface` keeps a
  running maximum over the column's statics **in file order** and accumulates
  `z_top` over everything that passed on the way — so a climbable with a low
  surface and tall art is selected when it is met first and skipped when a
  flatter, higher-surfaced static is met first. Spans are stored highest-first,
  which is the other order. A fourth height per span therefore does not
  reproduce the rule; storing the file's order does, and that is a different
  table. Anybody returning to this — N3b, or a future `Stance` that wants to be
  cheaper — should start here rather than from the three options above.
- **N3 found: `WorldState::tiles` is a public field, and writing it does not
  rebake. ✅ Fixed.** `FacetState::set_map` moved the ground and its bake
  together and `World::with_tiles` rebaked every loaded facet, so both seams
  were safe; a direct `state.tiles = table` was not, and since
  [`Ground`](../../crates/common/movement/src/ground.rs) closed the other half
  — the *ground* can no longer move out from under its bake — it was the one
  remaining way to hold a bake that describes neither world in hand.
  **The field is private now**, read through `WorldState::tiles()` and replaced
  through `WorldState::set_tiles`, which is where the rebake loop moved from
  `World::with_tiles`: the write and the rebake are one call, so there is
  nowhere left to do the first without the second.
  **What it cost is worth recording, because the finding did not see it.** A
  struct with one private field cannot be written as a literal outside its own
  module, and `WorldState` was written as one in **five** places — `World::new`
  and a fixture in each of `party`, `guilds`, `boats`, `housing` — each naming
  all twenty-four fields, so a field added here had to be added in five places
  or nowhere. `WorldState::new` is what replaced them: `facets`,
  `default_facet`, `tiles`, `multis`, `start` and a seed, with everything else
  starting empty. The four fixtures shed twenty imports between them, which is
  the measure of how much of each was ceremony.
  **Done when:** `a_late_tile_table_rebakes_every_facet` in `state/src/runtime.rs`
  — **two** facets over an empty table, each with a wall the table cannot see, and
  a `set_tiles` that has to reach both. The control is the loop deleted by hand,
  where it fails at the first facet; one facet would have passed a rebake that
  only ever touched the default one.
- **N3 found: the interiors bake builds two facet-wide span indexes of its
  own.** `PlanarTopology::bake` and `Buildings::bake` in
  `client/render/src/interiors.rs` each take a map and a tile table and now
  build a `SpanIndex` to get a terrain — 0.07 s each, inside a bake that already
  walks the facet, and the client builds a third at startup. Threading one
  through five `bake` signatures would put a movement index in the arguments of
  a wall contour, which is why it was not done; the honest fix is for the
  interiors bake to take the ground it is baking over as one value. **That value
  now exists** — [`Ground`](../../crates/common/movement/src/ground.rs), whose
  `terrain(tiles)` is exactly what both of them build for themselves — so what
  is left of this finding is the signature sweep, across `interiors.rs`'s five
  bakes plus `artscan` and the examples that call them.
- **N3 found: a `Scene` rebakes on every setter.** A fixture that places a
  thousand statics pays a thousand bakes of its own blocks, and each one walks
  `land_kinds`'s 16,384 land ids. Nothing in the suite is slow enough to notice
  — `land_everywhere` was the one sweep that would have been, and it bakes once
  — but a fixture that grows will notice. It is the price of `Scene::terrain`
  taking `&self`: there is nowhere to notice staleness later, and a bake one
  static behind its map is a fixture testing the wrong world.
- **N3b corrected the key: a node cannot be a span index.** This plan wrote the
  key as `(x, y, span)` in twenty-nine bits of a `u32`, twice, and the code says
  otherwise: `walk::climbed` lands a body on the *overlay's* surfaces — a
  house's storey, a deck, a placed stair — and none of those has a span, because
  a span is a fact about the map file. The height is what both layers speak, so
  it is what the key is. The general shape is worth carrying to N4, which is
  about to key a graph by spans: **the map's own surfaces are not all the
  surfaces**, and anything keyed by span alone is a graph the live world cannot
  be placed into. What saves N4 is that its graph is baked from the bare map and
  the live world is applied at refinement time — which is a property to keep
  deliberately rather than to discover.
- **N3b found: a node budget is not a tile budget, and 400 was measured against
  tiles.** `budget` bounds *finalised nodes*, and a column with two floors can be
  finalised twice, so the same 400 buys marginally less ground than it did. On
  Britannia that is 0.6% of columns and the measured cost is two destinations out
  of 37,248 at the bank — (1403, 1718) and (1402, 1719), whose goal used to be
  found on the 400th node — and neither is lost at 600. It is filed rather than
  fixed because the numbers to re-argue are 400 for server AI and 600 for a
  client plan, and the argument for them is a *time* budget: the measurement
  that would move them is the one in
  [`terrain_seam.md`](terrain_seam.md#what-one-search-costs), not this one.
  Whoever revisits them should know the unit changed under the number.
- **N3b found: the probe's `revisits` count is zero over the bare map, and that
  is the sweep rather than the world.** A route that comes back to a column at a
  second height is the thing no tile-keyed search could plan, so
  `map_path_probe` counts them — and finds none from any of the three origins,
  because it runs with an empty overlay and the map's own multi-span columns are
  bridges and piers you walk along. **A house is the shape that produces one.**
  Anybody reading that zero as "no such routes exist" would be reading a sweep
  over a world with no houses in it; the villa test in `walk_scenes.rs` is where
  the shape is asserted, and a shard-side sweep is what would count them.
- **N3b found: `Overlay::surface_at` broke a tie by iteration order. ✅ Fixed.**
  Two surfaces equidistant from the height asked about resolved to whichever the
  overlay happened to yield first, which is a `Vec` order nobody promised — so
  the answer followed the order a house's components were registered in.
  `path::goal_node` never inherited it, because it breaks the same tie by the
  lower surface on purpose; the overlay's own resolver now keys by
  `(distance, surface)` and does the same. The one production caller that can
  see the change is `walk::aboard`, the deck a body steps onto; `can_fit` asks
  for an exact match, and distance zero is a unique minimum.
- **N4 found: `local_costs` is one exact search per node in the endpoint's
  region, and N4 made the regions that matter denser. ✅ Fixed.** Joining an
  endpoint to the graph ran a bounded A\* from it to *every* node of its own
  region, at both ends of the query, and a node that cannot be reached cost the
  whole budget before saying so. The castle's region went from 18 nodes to 51
  while the facet total fell 16%, and the same seven routes went from 1.29 ms to
  4.39 ms p50. **The join is one flood now**, not a fan-out: a uniform-cost
  traversal of the endpoint's own region answers every node at once, expanding
  each place at most once however many nodes stand in it, and a node outside the
  endpoint's reach costs nothing because the flood never arrives there. That
  reach *is* the component label the bake computes and throws away, recovered
  where it is wanted instead of stored — which is the second of the two options
  filed here, arrived at without the artifact growing. The two directions are
  two traversals, because the step rule is asymmetric and a target joined
  forwards would offer corridors whose last hop nothing can walk. On facet 0
  from the castle, release, three runs agreeing to the hundredth: **p50 3.70 →
  0.53 ms at 32 tiles**, 2.74 → 0.66 at 64, 2.44 → 1.00 at 128, 2.44 → 1.13 at
  256, 2.96 → 1.56 at 512, 3.75 → 2.32 at 1024, and the worst reading of any
  band 6.50 → 2.89. Every route came back with the same number of steps. The
  band that N4's regression was measured in is now *below* the 1.29 ms it
  regressed from.
- **N4 found: the bake was paying eight times over for every neighbour, and so
  is every other flood in the tree. ✅ Fixed.** `component_labels` and
  `region_costs` asked `step_allowed` once per direction, and `step_allowed` is
  *defined* as one slot of `steps_out_of` — so each asked for the whole
  expansion eight times and used one answer of it. Repaired in N4 for the bake,
  and it is most of 96 s → 11.7 s; the rest of the tree was left filed, because
  neither remaining copy is on a hot path and both were one line.
  **The repair that landed is not that line.** Three floods had been written
  independently — `coarse_bench`'s `land_component`, `Scene::reachable`, and
  `span_check`'s two-rule comparison — and fixing the expansion in three places
  leaves three places for a fourth copy to be written beside. There is one flood
  now, [`reach::Reach`](../../crates/common/movement/src/reach.rs), and the
  diagnostics and the fixture ask it: `Reach::of` walks the shipped rule and
  `Reach::by` takes an expansion handed in, which is what an oracle comparing
  two rules needs. On facet 0 from the castle, release, A/B on one tree:
  **the whole-facet flood is 5.1 s → 0.9 s** and reaches the same 3,747,934
  tiles. `span_check`'s pair is 4.4 → 2.8 s and 2.6 → 1.5 s, from the same pass
  folding its corner rule — written once per side before — into the shape
  `steps_out_of` gives it, and its oracle still reports 248,268,125 steps, 0
  disagreements, 0 tiles differing.
- **N4 found: in-degree over places is not bounded by the eight directions.**
  Out-degree is — one landing per direction — and the builder's fixed `[_; 8]`
  neighbour arrays assumed the same of the other side. It is false as soon as a
  column has two places: **a stair is exactly the shape that breaks it**, since
  the low place and the high place of one neighbouring column can land on the
  same tread. It panicked on the first stair scene written against it. The
  incoming half is counting-sorted into one run per place now.
- **N4 found: a place is one node, and the old builder did not think so.** Every
  logical entrance minted fresh nodes, so a point named by two entrances — the
  two ways across one border, or a corner where a vertical and a horizontal
  border meet — was two nodes with two identities and two sets of intra-region
  routes to pay for. Interning them is what keeps a directed portal costing what
  a symmetric one did.
- **N4 found: the graph's `walkable` bitmap is still one bit per *tile*, and
  `region_at` still ignores z.** That is deliberate and it is what lets an
  endpoint with a z nobody promised join the graph at all — `path::goal_node`
  resolves the height afterwards. It does mean the graph cannot say *which*
  storey of a tile is walkable without looking at its nodes, so anything that
  reads the bitmap as an answer about a place rather than about a column is
  reading it wrong. [N7](#n7--the-server-reads-the-graph) is the next caller.
- **N4 found: bumping `ROUTING_VERSION` stops the shard from booting, and only
  warns the client.** A stale artifact is `Err` in `boot.rs` and a printed line
  in the client's `lib.rs`, so a shard pulling this change does not start until
  its facets are rebaked. That is the right loudness for a graph that would
  otherwise answer with a one-storey world — it is recorded because it is a
  *deployment* step, not a defect.
- **N7 found: the coarse router refused outright when both endpoints shared a
  region. ✅ Fixed.** `find_long_path` special-cased `from_region == to_region`
  into `region_route`, which is *confined to that 32×32 rectangle* — so two
  points twenty tiles apart whose only connection leaves the region and comes
  back got `LongExit::NoLocalRoute`, and the graph beside them was never
  consulted. Found while sizing N7's fixture: a first shape put both ends in
  region 0 and the router answered `None` at every width tried, while the flood
  said the ground was walkable. **The local route is a first attempt now rather
  than the verdict** — a refusal falls through to the same join, corridor and
  refinement a cross-region query takes, and `NoLocalRoute` is gone with the
  branch that named it. Asserted in `two_points_in_one_region_route_by_leaving_it`:
  a wall the length of region 0 and no further, so the only way from (4, 4) to
  (28, 4) is south into the region below and back north. The corridor answers
  with the same 58 steps the exhaustive exact search walks, the route is checked
  to leave the rectangle — which `region_route` cannot do, so it is the graph's
  own work — and the control is the fall-through disabled by hand, where the
  test fails at exactly that assertion. **What it costs is the finding below.**
- **N7 found: the aggressive chase does not go through `step_toward`.**
  `ai::chase_step` plans its own route with a bare `find_path` at
  `PATH_BUDGET`, caches it as a `ChasePath` and, when it is refused, calls
  `give_up` — guard for ten seconds, then wander. So the fall-back N7 added
  reaches pets, escorts and townspeople going about their business, and not a
  creature chasing a player. Deliberate here, and the argument is that a chase
  is already bounded to twice a creature's sight (`CHASE_RANGE_FACTOR`,
  `CHASE_RANGE_MIN`), so a quarry it may legitimately follow is rarely further
  than the exact search reaches — but the plan named `step_toward` and only
  `step_toward`, and whoever wants a creature to round a town block should know
  the second planner is there.
- **🚩 N7 found: a refused coarse query pays the whole join, and nothing behind
  `step_toward` remembers it. ✅ Fixed.** A goal that is walkable-looking but
  sealed off cost `local_costs` at both ends in full — every node of both
  regions, each to its own budget — plus up to `LIVE_REROUTES` abstract retries:
  **17.4 ms on a 96×64 fixture with twenty nodes, in a debug build**, repeatable
  to the tenth, and the same-region repair widened the class (4.8 ms → 25 ms on
  a 64×64 fixture with sixteen nodes). `chase_step` has `give_up`'s ten-second
  guard behind its refusal; `step_toward` is a pure function of the world and had
  nowhere to put one, so an escort whose goal is unreachable and more than
  `COARSE_MIN_DISTANCE` away paid that on every beat.
  **A body has somewhere to keep it now.** `ai::step_body_toward` is the same
  decision made for an entity rather than for a point: a refusal is written on it
  as a `RouteRefused { goal, until }`, and while that stands the graph is not
  asked about that goal again. Only the coarse half waits — the exact search runs
  every beat as it always did — so what is deferred is the facet-wide answer, for
  `REFUSAL_TICKS` (~2 s, the repath cadence), and a goal that drifts past
  `GOAL_DRIFT` clears it the way it invalidates a `ChasePath`. The three callers
  the plan named — a pet, a townsperson walking home, an escortable — all go
  through it; `step_toward` stays as the pure reading, which is what the shard's
  own walk probe asks. Asserted in
  `a_refused_long_route_is_remembered_until_it_lapses`: one wall, one doorway,
  one shut door, and a route that opens while the memory holds is **not** taken
  until it lapses — the blindness is the only thing about a memory a test can
  see, and with the memory disabled by hand the test fails at exactly that
  assertion.
  What remains true and is not a defect: the graph is what answers when the exact
  search is refused, and on small ground it is not cheaper than that search would
  have been *with a budget the shard does not grant it* — `PATH_BUDGET` is 400
  and the 64×64 fixture's exhaustive search wanted 558 nodes.
- **Found while repairing the join: `can_step` has no corner rule, and the
  shard walked a creature with it. ✅ Fixed.** A diagonal may not clip the
  corner where two blockers meet, and that rule lives in `steps_out_of` — which
  resolves all eight neighbours together precisely so a diagonal can read its
  two flanks, and where it moved in [N3](#n3--the-search-takes-spans).
  `can_step` is one landing: it answers whether a body may *stand* where a step
  ends, and nothing else. Found by two tests in `state/src/obstruct.rs` that had
  been asking `can_step` for the corner rule and had been failing since it
  moved.

  **The player was never one of the sites, and the first writing of this finding
  said otherwise.** A client's `0x02` is approved by `Walker::request`, which
  asks `step_allowed` and has since N3; so does the client's own prediction. The
  two callers that decided a step through `can_step` were both the shard moving
  a body that is *not* a player:
  [`World::step`](../../crates/server/world/src/tick/motion.rs), the decree every
  creature, pet, townsperson and escort is stepped by, and
  [`ai`'s `probe`](../../crates/server/ai/src/lib.rs), which is what a chase asks
  whether the way to its quarry is open. The disagreement was inside one
  creature: `find_path` refused to *plan* a corner cut and the same creature then
  cut one walking straight at its quarry.

  **What the references say**, which is the measurement this was held back for:

  | | the diagonal |
  |---|---|
  | ServUO `MovementImpl.CheckMovement` | a **player** below GM needs **both** flanks (`!left \|\| !right` refuses); **everything else** — NPCs, GMs — needs only **one** (`!left && !right` refuses) |
  | ClassicUO `Pathfinder.CanWalk`, which is both its auto-walk and `PlayerMobile.Walk`'s own prediction | **both** flanks (`dir ± 1`), and a refused diagonal is retried as one of the two cardinals |

  So the reference keeps two rules and gives the lax one to creatures. **This
  shard keeps one, and it is the strict one**: both sites ask `step_allowed`
  now. The argument is that everything else here already speaks it — the baked
  graph, `find_path`, the client, and the player — so a second rule would have
  to be threaded through `steps_out_of`, `find_path` and the bake to buy a
  behaviour nothing has asked for, and the creature's *routes* were strict
  already. The lax reading is a knob that can be added later, and where it would
  go is `steps_out_of`. Recorded here because it is a deliberate divergence from
  ServUO and not an oversight.

  The done-whens are in `world/src/tick/tests.rs`, one per site, and each
  carries its control in the same test rather than by hand:
  `a_server_step_does_not_cut_a_corner` (one crate due east, the south-east
  decree refused, and allowed again the moment the crate is unblocked) and
  `a_chase_does_not_cut_a_corner` (the same corner, walked by a creature: the
  first step is not the cut, and with no crate at all it is). Reverted, both fail
  at the corner assertion and nowhere else.
- **The client's roof cutaway asks `can_step`, so it advances for a diagonal
  the shard refuses. ✅ Fixed.** `advance_cutaway` in
  [`net_command.rs`](../../crates/client/app/src/net_command.rs) moved the
  cutaway source when "this move is locally known to be possible", and it asked
  the one-landing reading — so once every step the shard permits carried the
  corner rule, the two disagreed by exactly a cut: a direction held into a
  building corner moved the roof threshold for a step about to be rubber-banded.
  It was filed rather than fixed because it is presentation and not a step gate;
  what it is *not* is a third reading of *can I go there* inside a client whose
  other two — the walker's own prediction and the held-key detour — both speak
  `step_allowed`.
  **The guard is its own function now**, `cutaway_follows`, which is what makes
  it testable at all: the threshold is otherwise only reachable through a packet
  fold on a live `App`. **And it says what a step is.** `step_allowed` needs a
  direction where `can_step` took two points, so a move that is not one step —
  the body already standing where the threshold is, a z that changed under it, a
  gate, a push — is answered *yes* rather than measured: a threshold stranded
  behind hides the body the cutaway exists to reveal, which is the failure the
  guard was written against in the first place.
  **Done when:** `the_cutaway_does_not_follow_a_corner_cut` — two crates
  flanking an open diagonal, with the same crate moved off the flank as the
  control, and the two not-a-step cases asserted beside them. Reverted to
  `can_step` it fails at the first assertion.
- **`items/mounts.rs` resolves the same stance eight times to put a mount
  down. ✅ Fixed, and the corner rule came with it.** Dismounting looked for
  somewhere beside the rider with eight `can_step` calls, each re-deriving the
  tile being stepped off — the shape N4 found in `component_labels` and
  `region_costs`. It asks `steps_out_of` once now, which is those eight answers
  for the price of one.
  **The rule was the decision, not the swap.** A mount is *placed* beside its
  rider rather than walked there, so a corner rule is not obviously owed. It is
  taken anyway, because the alternative is worse than the cost: every step the
  shard permits has carried the rule since `World::step` went through
  `step_allowed`, so a horse put down through a cut stands where nothing could
  have walked it and where the same rule can refuse to walk it out. "Nowhere
  beside the rider" is an answer this code already had — under the rider — and
  it is the better of the two.
  **What fell out of it is worth knowing before reading that loop:** with the
  corner rule in it, a diagonal is never what the loop picks. A legal diagonal
  needs both flanking cardinals to be steppable, and both come earlier in
  `Direction::to_bits` order — so the choice is the first open cardinal, or the
  rider's own tile.
  **Done when:** `a_dismount_does_not_put_a_horse_through_a_corner` in
  `world/src/tick/tests.rs` — a rider boxed in by seven crates whose one open
  neighbour is a corner cut, and a horse that lands under him. Reverted to
  `can_step` it stands on the diagonal and the test fails there. The control in
  the test is the northern crate taken away, where the same dismount uses it —
  which is what says the refusal was the rule and not a loop that had stopped
  finding anywhere.
- **`a_creature_routes_past_its_exact_budget_over_the_coarse_graph` is
  load-sensitive by construction.** `walk_toward` re-plans from scratch on every
  beat, and every plan reads `MAX_LONG_PATH_TIME` — 50 ms of *wall clock*. A
  debug build on a busy machine can miss it, and one miss anywhere along the
  walk drops the creature onto the straight-line fall-back and ends it far from
  the goal. It went red once during the corner-rule work — `left: Point { x: 37,
  y: 31, z: 0 }` against `right: Point { x: 2, y: 48, z: 0 }` — and did not
  reproduce afterwards, including ten runs in isolation; that run overlapped a
  parallel session's in-flight edit to `spans.rs`, so the cause is **not**
  settled. What is settled is that a wall clock decides this assertion, and a
  deadline the caller names is what would take it out of the assertion.
- **A second `Ground` now exists in `client/app`, and it is the misnamed one.
  ✅ Fixed.** [`steer::Readings`](../../crates/client/app/src/steer.rs) is a
  pair of `Footing`s — the same map read twice, once with the doors shut and
  once open — which is a *reading*, and not ground. Nothing collided at the
  compiler and no file imported both, which is what let one crate spell two
  ideas with one word after the ground and its bake became
  [`Ground`](../../crates/common/movement/src/ground.rs). It is `Readings` now,
  and the type says why on itself. Nothing else moved — the fields, the callers
  and their bindings are what they were — because the name was the whole of it.

- **A dense `average_land_z` array.** 29.4 MB turns the bare-column case from
  four corner reads into one. It waited for N3's measurement, and the
  measurement is that the whole landing half is 167 ns for eight neighbours —
  about 21 ns a tile, four corner reads among them. So it is real and it is
  small: 29.4 MB to shave a fraction of a fifth of a node. Not now.
- **Baked adjacency.** Recast stores neighbour links in the span; this plan does
  not, because the census says a neighbour lookup is already one bit test and a
  land read for 92% of columns. The trigger written here was "if N3 measures a
  node expansion that is still the search's whole cost" — **it does not**: 208 ns
  of terrain against ~190 ns of A\*, so a search is now half heap and hash. An
  8-bit mask per span would attack the smaller half, and the census still proves
  it fits.
- **`sight_clear`'s own height blindness.** The same class of defect — a sight
  line reads the tiles it crosses and not the endpoints' columns, so two mobiles
  on one tile at different z see each other through a floor. It is
  [filed in `terrain_seam.md`](terrain_seam.md#-blindterrain-stood-for-a-rule-that-cannot-exist)
  and it wants the same span list, but it is a change to what a sight line *is*
  and does not belong in a movement plan.
- **The statics layout.** 120,745 allocations and 38.2 MiB where a CSR pair
  would be 2 and ~13.5 MiB —
  [direction B](new_map_representation/plan.md#b--our-own-chunk-format-and-a-uo-importer)'s.
  This plan makes it matter less by taking the statics off the hot path, and
  does not fix it.

## Where a session starts

**Nothing in this plan is open, and nothing forces what is left.** N0–N4, N3b
and N7 are built: the coarse graph refuses nothing the flood says is walkable
from any of the five recorded origins, and since N7 the shard reads it. What
remains is N5 and N6, and both are gated rather than queued — see below.

**What a session that wants work here should read first** is *Out of scope,
named*, which is where six nodes filed what they saw and did not fix. **Every
finding there with a defect behind it has since been repaired** — N7's
same-region refusal, then N4's `local_costs` fan-out and N7's unremembered
refusal, which were one repair wearing two names and were taken together: the
first made the join cheap (a flood over the endpoint's region instead of one
exact search per node of it: p50 3.70 → 0.53 ms at 32 tiles on facet 0, and the
worst reading of any band 6.50 → 2.89), the second made it rare (a refusal is
written on the body and the graph is not asked again for two seconds). What is
left in that section is filed observations with no defect under them — **and one
of those has now been taken as well**: N1's count tables, which the mask cut
from 8.2 MB to 3.3 and the bake from 15.8 MiB to 11.2 without the answers
moving. It was the one the plan named as *the next thing to try if a node
expansion has to get cheaper again*, so what it leaves behind is the same
sentence pointing at the packed static record instead.

**Rebake before running anything.** `ROUTING_VERSION` is 4, so every artifact
baked before N4 is refused — and refused *loudly*: the shard does not boot.
`cargo run --release -p openshard-movement --bin openshard-navigation-bake --
--facet 0` takes 11.7 s.

**Nothing forces N5 or N6.** N5's content is deliberately empty until a flood
says what the spans still cannot connect, and that flood is N5's own first step;
N6 is gated on a number nobody has asked for yet.

**What a session should not do is re-open the landing rule.** `Spans::check` is
what a step asks, `MapTerrain::check` is the map's own statement of the same
rule and has no production caller, and the `span_check` example is the 248
million comparisons between them. That pair is the thing that will notice a bake
which has stopped describing its map — after a patch, after a base set, after
the footprint work in [`footprints.md`](../footprints.md) changes what a static
*is*. Keep both halves.
