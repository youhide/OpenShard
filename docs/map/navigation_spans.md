# The first storey

> **Status: live — era P, started. N0, N1, N2 and N3 are built.** The gate is
> gone: [`realtime_map.md`](realtime_map.md)'s era R is over, the span layer is
> built and measured against the whole facet, and **the shard now walks on it**
> — a node expansion is 208 ns where it was 1,105, a search from the castle is
> 0.168 ms where it was 0.793, and every arrival count is bit-identical to the
> run [`terrain_seam.md`](terrain_seam.md#what-one-search-costs) recorded. The
> risk this plan was carrying is retired and the win is banked. **N3b is next**
> — the node stops being a tile — and it is the one node that *must* change the
> routes. See [`map_rebuild.md`](map_rebuild.md) for the order and
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
baked and takes 96 s. So the span layer is built at load until something
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

**[N4](#n4--regions-over-spans) builds directed edges**, then: a portal joins two
spans in one direction, and the reverse is a separate edge that may or may not
exist. What that leaves [N5](#n5--off-mesh-links) is what it always should have
been — links geometry does not imply *at all*, a teleporter and whatever the
flood says is still unreachable — rather than the place a drop would have been
declared by hand.

## The nodes

```
 terrain_seam.md ✅ ──> map_rebuild.md R1 + R2 ──┐
 (the signature)       (the map, in one type)   │
                                                ▼
 N0. the census ✅ ──> N1. three tiers ✅ ──> N2. the step rule reads them ✅ ─┬─> N3. the search takes Spans ✅
                                                    (the agreement oracle)   │        └─> N3b. the node stops
                                                                             │              being a tile
                                                                             │
                                                                             └─> N4. regions over spans ──┬─> N5. off-mesh links
                                                                                          │               │
                                                                                          │               └─> N7. the server reads the graph
                                                                                          └─> N6. an artifact, if measured        (inherited from F)
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
would silently re-derive every column, and `Footing::of` panics rather than
accept a map without one. It sits *beside* the world rather than inside it —
`FacetState::spans` on the shard, `Resources::spans` on the client — for a
reason that is worth writing down because it is not a preference:
`openshard_map` is underneath `openshard_movement`, and where a body may stand
is a movement rule, so [`World`](../../crates/common/map/src/world.rs) cannot
hold the projection of its own two layers. `FacetState::set_map` is the one seam
that moves both, and `World::with_tiles` rebakes every facet already loaded, so
the builder's argument order cannot produce a bake over the empty tile table.

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
the example — `map_step`/`map_land` beside `span_step`/`span_land`, identical
but for which `check` answers the landing — and the oracle still holds over the
whole facet: **248,268,125 steps, 0 disagreements; both floods reach 3,747,934
tiles, 0 tiles differing** (the map flood 4.0 s, the bake flood 2.9 s). A test
that calls the shipped rule stops being a test of the shipped rule the moment
the rule moves under it.

**And the composition is asserted, not assumed.**
`the_live_world_adds_takes_away_and_hangs_a_door_over_baked_spans` in
[`walk_scenes.rs`](../../crates/common/movement/tests/walk_scenes.rs) walks onto
a `Stands` cover the bake has never heard of, is refused by a `Blocks` cover in
its own span, and is refused by a shut door under `Doors::AsTheyStand` and
admitted under `AllOpen` — over a `Scene`, which carries a real bake and keeps
it in step with its own map. **All three claims were checked to bite**, by three
mutations of `walk.rs` run one at a time: dropping the overlay's floor, dropping
its veto, and ignoring the door reading each fail exactly the claim they should.


### N3b — the node stops being a tile

Needs N3, and it is deliberately **not** part of it: N3's oracle is that nothing
about the routes changed, and this is the one change that must alter them.

The key widens from a planar tile to `(x, y, span)` — twenty-nine bits of the
`u32` it already uses, since the index is zero for 99.4% of the facet. What that
buys is the thing [a node is](#what-a-node-is-and-the-z-that-is-already-gone)
names: a column with two standing places stops collapsing into one slot in
`closed`, so a route may pass over a bridge and later under it, and a body on a
house's first floor is not the same node as one on the ground beneath.

**🚩 And the same-column case is not merely refused today — it is answered with a
lie.** [`search`](../../crates/common/movement/src/path.rs#L210) flattens both
endpoints to tiles and compares them:

```rust
let goal  = Tile::new(to.x, to.y);
let start = Tile::new(from.x, from.y);
if start == goal { return PathSearch { arrived: true, route: Vec::new(), … } }
```

So *"from the ground floor to the first floor of this column"* returns **success
with an empty route**, and the caller walks nowhere believing it has arrived.
Two more places say the same thing more quietly: `if tile == goal` compares
tiles, so reaching the column at any height is an arrival, and the start's own
column is closed by the first `closed.insert`, so it can never be re-entered at
another height. The z of both endpoints reaches only the
[heuristic](../../crates/common/movement/src/path.rs#L408), which ignores it.

It is not a pathfinding curiosity: server AI told to walk to a mobile standing on
a bridge over it is told it is already there.

**There is no vertical edge, and there must not be one.** Nothing in UO moves up
in place — the eight neighbours are horizontal and the step rule changes height
as a *consequence* of moving. A route from one floor to another over one column
is a **loop**: out of the column, up whatever tiles rise, and back over the same
`(x, y)` at a different span. Which is exactly why the key has to be the node:
without it the return is forbidden by the closed set rather than by the world.

Four things this node therefore owns, beyond widening the key:

- **The goal is a node**, `(tile, span)`, with the caller's z resolved to the
  nearest surface the way `Overlay::surface_at` already resolves one.
- **The `start == goal` early return compares nodes**, so the empty-route answer
  above stops being reachable.
- **`cost`, `came_from` and `closed` are keyed by the node**, which is what lets
  the loop come home.
- **The heuristic stays planar, and it goes flat in exactly this case.** Chebyshev
  over `(x, y)` is admissible — every step moves one tile, so it is a lower bound
  — but with the goal one storey up in the same column it is zero at the start,
  and the search fans out until it meets a stair. Inside a house that is nothing;
  inside a castle it can spend the 400- or 600-node budget. The real answer for a
  long climb is [N4](#n4--regions-over-spans), where spans are the graph's nodes
  and a staircase is a portal — which is one more reason the coarse layer is the
  user-visible half of this plan.

**Done when:** a search that must use both heights of one column arrives where it
previously refused; **a request from one floor of a column to another no longer
returns an empty route**; and node counts change on **exactly** the searches that
touch a multi-span column — enumerated, not asserted in bulk, because a count
that moved anywhere else means the key change altered something it had no
business altering.

### N4 — regions over spans

Needs N2. This is the node that retires the one-storey defect, and it is where
the user-visible repair lands.

`NavigationGraph::build` currently samples `ground_z` — the land alone — once
per tile. It samples **spans** instead: a region's nodes are spans, its
components are computed over spans, and a portal on a region border joins two
spans rather than two tiles. A bridge crossing a border is then its own portal,
and the castle plateau stops being an island.

**And the edges become directed**, which is the second half of the same repair.
Today a shared side becomes a portal only where `step_allowed` succeeds *in both
directions* ([`navigation_graph.md`](navigation_graph.md)), and the step rule is
asymmetric by design: a climb reaches `start_top + 2` while a descent is
unbounded. So every ledge a body may step off but not back onto is currently
invisible to long-distance routing — a refusal rather than a lie, which is the
right side of the error and still a refusal. A portal joins two spans one way and
the reverse is its own edge.

**Done when:** `coarse_bench`'s `refused_but_walkable` is **0 in every band from
every one of the five origins** recorded in
[`terrain_seam.md`](terrain_seam.md#-the-coarse-graph-is-a-one-storey-model-of-a-two-storey-world),
including the castle plateau where it is currently 37 of 45. The bake's own
duration and artifact size are recorded beside the 96 s baseline.

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

### N7 — the server reads the graph

Needs N4. **Inherited from [`terrain_seam.md`](terrain_seam.md)'s F**, which
asked whether the baked navigation graph should be wired up or stopped being
paid for, answered *wire it up*, and handed the action here because the repair
that has to come first is N4's.

Server AI plans with flat [`find_path`](../../crates/server/ai/src/lib.rs#L79)
at a budget of 400 explored tiles, so a creature cannot route across a town
while the artifact that would let it sits loaded and validated in
`FacetState.coarse`, read by nothing but a test. The client already falls back
past 8 tiles through `steer::Ground::path`; `step_toward` gains the same
fall-back, and the two ends stop disagreeing about how far a body can plan.

**Done when:** a test walks a creature a distance flat A\* at budget 400 cannot
— over ground the flood says is walkable, from a raised origin as well as a flat
one. The raised origin is the half that would have passed for the wrong reason
before N4.

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
- **N1 found: the count tables are bigger than the spans they address.** 8.2 MB
  of `[u8; 64]` against 6.5 MB of spans, because only 2.3 M of the 7.7 M columns
  in a static-bearing block are exceptions and the other 71% of every table is a
  zero. A per-block `u64` occupancy mask with counts for the occupied columns
  only is about 3.3 MB, and it replaces a 64-byte prefix sum with a
  `count_ones` — smaller *and* fewer bytes read. Not taken: it is a layout
  change with a query change in it, and N3 was the measurement that would say
  whether the query is on a hot path at all. **N3 says it is**: the landing half
  is 167 of a 208 ns expansion and `SpanIndex::stored` — a block lookup and that
  64-byte prefix sum — is inside every one of its eight. Still not taken here,
  because the gain is now against a number that is already the size of A\*'s own
  machinery; it is the next thing to try if a node expansion has to get cheaper
  again. The same gate the packed static record is under.
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
  rebake.** `FacetState::set_map` moves the ground and its bake together, and
  `World::with_tiles` rebakes every loaded facet, so both seams are safe. A
  direct `state.tiles = table` is not: it leaves every facet holding a bake over
  the old table, which is a shard deciding steps by the heights of a world it no
  longer has. Three test fixtures do it today, harmlessly (they assign the same
  table they just baked from). The repair is to make the field private behind a
  setter that rebakes — the same shape `FacetState::obstructions` already has,
  and for the same reason.
- **N3 found: the interiors bake builds two facet-wide span indexes of its
  own.** `PlanarTopology::bake` and `Buildings::bake` in
  `client/render/src/interiors.rs` each take a map and a tile table and now
  build a `SpanIndex` to get a terrain — 0.07 s each, inside a bake that already
  walks the facet, and the client builds a third at startup. Threading one
  through five `bake` signatures would put a movement index in the arguments of
  a wall contour, which is why it was not done; the honest fix is for the
  interiors bake to take the ground it is baking over as one value.
- **N3 found: a `Scene` rebakes on every setter.** A fixture that places a
  thousand statics pays a thousand bakes of its own blocks, and each one walks
  `land_kinds`'s 16,384 land ids. Nothing in the suite is slow enough to notice
  — `land_everywhere` was the one sweep that would have been, and it bakes once
  — but a fixture that grows will notice. It is the price of `Scene::terrain`
  taking `&self`: there is nowhere to notice staleness later, and a bake one
  static behind its map is a fixture testing the wrong world.
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

**N3b — the node stops being a tile.** N3 is built and the win is banked: the
shard's step rule reads the bake, a node expansion is 208 ns where it was 1,105,
and the three recorded searches arrive on exactly the tiles they arrived on
before. Which is precisely why the next node is the one that **must** change
them — a column with two standing places still gets one slot in `closed`, and a
route from a house's ground floor to its first floor is still answered with
success and an empty route. Everything N3b needs is in
[its own section](#n3b--the-node-stops-being-a-tile): the key widens to
`(x, y, span)`, the goal becomes a node, and the four things that follow from
that are enumerated there.

**N4 is the alternative first move, and nothing forces the order.** N3b and N4
both need only what is built; N3b is the finer defect and N4 is the one a player
would notice — a creature that can route out of Britain's castle — with N7 the
node where they actually notice it, because until then nothing on the server
asks. If a session wants the visible repair, take N4 and leave N3b; they do not
collide.

**What a session should not do is re-open the landing rule.** `Spans::check` is
what a step asks, `MapTerrain::check` is the map's own statement of the same
rule and has no production caller, and the `span_check` example is the 248
million comparisons between them. That pair is the thing that will notice a bake
which has stopped describing its map — after a patch, after a base set, after
the footprint work in [`footprints.md`](../footprints.md) changes what a static
*is*. Keep both halves.
