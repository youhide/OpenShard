# The first storey

> **Status: live — era P, not started.** Its N0 is done and it waits for
> [`realtime_map.md`](realtime_map.md)'s R1 and R2, which move the two types N1
> is built from. See [`map_rebuild.md`](map_rebuild.md) for the order.

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

Two things follow, and the second matters more than the first.

**The hoist is not the plan's competitor.** Computing `start_surface` once per
expansion instead of sixteen times is 1,372 → 523 ns on a node, measured — ×2.6
on the terrain half, ×1.9 on a whole search. It is most of what a span grid
gets, for a day's work and no new structure. It is still not taken, for the
reason [terrain_seam](terrain_seam.md#-a-is-not-what-a-search-spends-its-time-on)
gives: it repairs a query that should be a table lookup, and it does nothing at
all for [N4](#n4--regions-over-spans), which is the node with a defect behind it.

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
After [`map_rebuild.md`](map_rebuild.md)'s R3 a house's ground floor and its
first floor will be one node. The coarse graph's one-storey defect — the reason
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
 N0. the census ✅ ──> N1. three tiers ──> N2. the step rule reads them ──┬─> N3. the search takes Spans
                                                    (the agreement oracle) │        └─> N3b. the node stops
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

### N1 — three tiers

Build the structure and nothing else. No search reads it yet; the only consumer
is N2's oracle.

- `Spans` in `openshard-movement`, built from a `&WorldMap` and a `&TileData`.
- The block tier reuses the map's **own** block indirection rather than minting
  a second one. `WorldMap` already indexes 8×8 blocks and already knows which
  hold statics; a parallel index is a second thing to keep in step, and this
  document set is a catalogue of what that costs.
- The exception table is CSR: a per-block base into a flat span array, and a
  per-block `[u8; 64]` of counts. The census caps a column at 12, so a count is
  a byte and there is no overflow case to design — **but the builder asserts
  it** rather than truncating, because a base set is a world nobody has counted.
- The land surface of a bare column is *not* stored.

**Done when:** `Spans::surfaces(x, y)` returns exactly what
`openshard_movement::surfaces::stand_surfaces` returns, for every column of
facet 0, for both abilities — asserted by a whole-facet test, not a sample —
and the built size and build time are recorded in this document.

### N2 — the step rule reads them

The rule moves from *deriving surfaces* to *choosing among stored ones*, and the
whole of this node is proving it did not change an answer.

`check(x, y, start_z, start_top)` becomes a walk of the target column's spans
rather than of its statics: pick the highest whose `reach_z` is within
`start_top + MAX_STEP_UP`, require `clearance` for the body, and apply the flags
filter. Everything the current `check` computes from `tiledata` — the platform
test, the climbable halving, the ServUO `landCheck` guard that lets low ground
poke through a static — is resolved once, in N1, when the span list is built.

**The risk this node exists to retire** is that `check`'s answer depends on the
source in some way a per-span bake cannot carry. It reaches the source through
exactly two scalars, `start_z` and `start_top`, which is why this is expected to
work — and expected is not measured. Where it turns out to be false, that is a
finding about `check` and it is filed here rather than worked around.

Two oracles, both already built this session:

- **Per-step agreement.** For every span on a sampled block and all eight
  directions, the new answer must equal `step_allowed`'s exactly — the landing
  point, not merely whether one exists.
- **Whole-facet flood equivalence.** The breadth-first flood
  [`coarse_bench`](../../crates/common/movement/examples/coarse_bench.rs) uses
  as ground truth, run over both, must reach the identical set of tiles. This is
  the test that would have caught the one-storey defect, and it is the one that
  makes the rest of this plan safe to build on.

**Done when:** both pass on facet 0, and `step_cost`'s node-expansion row is
re-run and recorded here.

### N3 — the search takes `Spans`

Needs N2, and needs [`terrain_seam.md`](terrain_seam.md)'s E for the other half
of the signature.

`find_path`, `find_path_toward`, `search`, `step_allowed`, `corner_open`,
`Around::read` take `&Spans` and `&Overlay`. The overlay is consulted after the
static answer, exactly as it is designed to be: it subtracts a blocked tile and
adds a moored deck, which is the one thing
[`docs/boats.md`](../boats.md)'s B3 argued a mask alone cannot do.

**Done when:** `map_path_probe` is re-run from the same three origins and the
node-expansion cost is in this document beside 1,462 ns. Arrivals and node
counts must be **bit-identical** to the run recorded in
[`terrain_seam.md`](terrain_seam.md#what-one-search-costs) — a faster search
that finds different routes is a different search.

**And the composition is asserted, not assumed.** A test that puts a `Stands`
cover over bare ground and walks onto it, a `Blocks` cover in a body's span and
is refused, and a shut door that both refuses under `Doors::AsTheyStand` and
admits under `AllOpen` — over baked spans rather than over a scene built for the
occasion. The overlay is consulted *after* the static answer and can overrule it
in one direction only, which is the property this pins.

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

- **Residency and tiling.** The estimate is under 20 MB, so the whole facet
  stays resident and Recast's tiling problem does not arise. If N1's measurement
  says otherwise, that is
  [direction G](new_map_representation/plan.md#g--residency-and-size-deferred-on-purpose)'s.
- **A dense `average_land_z` array.** 29.4 MB turns the bare-column case from
  four corner reads into one. An obvious follow-up and exactly the kind of thing
  that should wait for N3's measurement to say whether it is needed.
- **Baked adjacency.** Recast stores neighbour links in the span; this plan does
  not, because the census says a neighbour lookup is already one bit test and a
  land read for 92% of columns. If N3 measures a node expansion that is still
  the search's whole cost, an 8-bit mask per span is the next move and the
  census already proves it fits.
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

**Not here yet — [`map_rebuild.md`](map_rebuild.md)'s era R first.**
[`terrain_seam.md`](terrain_seam.md) has closed and its E landed the signature
this plan substitutes into, so the gate this section was written against is
gone — and a second one replaced it. The map is about to become one type holding
three layers (R2), the tile table is about to leave the file reader (R1), and
`Spans` is built from exactly the two layers underneath the live one. Written
before that, N1 is written against a `&WorldMap` and a `&TileData` it will be
handed differently a week later, which is the same mistake this plan already
waited once to avoid.

When era R closes — R1 and R2 are what N1 actually needs, not R3–R5:

**N1, and it needs no client install to write** — only to test, which this
machine can do. The census is done and the tier boundaries are decided; N1 is the
structure and one whole-facet equivalence test.

**N2 is where the risk is**, and it is worth reading its own section before
starting N1, because what N1 must store is decided by what N2 has to prove. If
`check` turns out to reach the source through more than `start_z` and
`start_top`, the span record grows a field, and it is cheaper to find that out
before the builder is written.

**N4 is the one a player would notice**, and N7 is where they notice it.
Everything before N4 is a structure and an oracle; N4 is the node where a
creature can route out of Britain's castle, and N7 is the node where it actually
does, because until then nothing on the server asks.
