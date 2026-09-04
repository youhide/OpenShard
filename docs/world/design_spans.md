# The span layer

The layer HPA\* assumes underneath it and this engine never had: **a column is a
*list* of standable surfaces rather than one height**, so a raised courtyard is
its own span instead of a disagreement with the land under it, and a search asks
a table where it used to re-derive the step rule from raw statics.

This is the model as built. How it got there, what each node measured, and the
findings it left behind are [`evidence/2026-08-25-the-span-layer.md`](evidence/2026-08-25-the-span-layer.md);
what is still open for the domain is [`README.md`](README.md).

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

Two types carry it: `SpanIndex` is the bake — owned, no lifetimes, built at
facet load and kept beside the map the way `NavigationGraph` is — and `Spans` is
the view a question is asked through, the index, the map and the ability of the
asker together. The middle tier below is what forces the split.

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
[C's leftover](research/terrain_seam.md#c--the-doubles-become-scenes-): `check_ground`
wants *"is anything in this body"* without *"and is there a surface"*, and a
clearance byte is exactly that question answered once at build time.

Three of the flags are the step rule's own clauses turned into properties of the
column: `CEILED` separates a real 255-unit gap from "nothing above", `LAND_WINS`
carries three of ServUO's four `landCheck` conditions, and `GROUND` marks the
column's own land span so the fourth is a comparison against a `reach_z` that is
already in the cache line.

**Water is a flag, not a second grid.** A swimmer's surfaces are 15 million
more, and every one of them is a bare ocean column whose height is
`average_land_z` and whose wateriness is one `tiledata` land flag — so they cost
*nothing* under the tiers above and need no storage at all. What the query does
is filter: the structure offers a surface, and the asker's own ability rejects
it. That keeps `swimming` where [D put it](research/terrain_seam.md#swimming-is-an-argument-now-because-the-thing-it-sits-on-is-the-query)
— on the query, scoped to one asker — instead of forking the artifact in two.

**Resident size: 11.2 MiB** — 3.3 MB of addressing over the 120,744 blocks that
have statics and 6.5 MB of spans (1,635,392 of them), built in 0.05 s. Against
the ~150 MB the facet already costs, and against the 117 MB a faithful
`rcCompactCell` (four bytes per column, every column) would take — which is why
Recast tiles its heightfield and never holds a world in one. The census is what
let this be a table rather than a tiling problem.

**There is no artifact.** The span layer is built at load, because building it
costs a twentieth of a second where the census that sized it took 3.5 s. The
baking that is actually expensive is the **region graph**, which is a separate
artefact at 19.8 s. Machinery this layer does not need is machinery it does not
mint; if a measurement ever asks for one, the rule is that the spans go in the
graph's own file under a bumped version, never in a second file that could
arrive at a different revision of one world.

## The shape

```rust
find_path(footing: &Footing, ...)   // and the footing carries the bake
```

`Spans` answers *movement*: where a body may stand, what it may step onto, and
what it fits under. `MapTerrain` keeps everything that is not a step —
`land_tile`, `statics_at`, `sight_clear`, `ceiling`, and placement's `can_fit`
over arbitrary heights — because those are questions about the world rather than
about a walk, and none of them is on the A\* edge.

**The bake is not optional where the map is.** `MapTerrain::new` takes a
`&SpanIndex` as its third argument, so there is no way to build a terrain that
would silently re-derive every column. `openshard_map` is underneath
`openshard_movement` and where a body may stand is a movement rule, so
[`World`](../../crates/common/map/src/world.rs) cannot hold the projection of
its own two layers: [`Ground`](../../crates/common/movement/src/ground.rs) is
the `World` and the `SpanIndex` over its base, with private fields and three
functions that write both in the same statement. "A facet with a map and no span
bake over it" is a state nothing can spell.

**A node expansion is one call.** `steps_out_of` resolves the tile being stepped
*off* once and answers each of the eight neighbours once, so sixteen landing
checks become eight — the four cardinals a diagonal asks about as flanks are the
four it was already asking about as destinations. `step_allowed(footing, from,
dir)` is *defined* as `steps_out_of(footing, from)[dir]`, so there is one rule
and no second copy to drift, and **the corner rule lives there**: a diagonal may
not clip the corner where two blockers meet, which is why `can_step` — one
landing, and nothing else — is not the reading a step is decided by.

**`start_surface` stays on the map.** It is one seventh of an expansion, and it
is order-dependent in a way a span list cannot reproduce: it keeps a running
maximum over the column's statics in the *map file's* order, where spans are
stored highest-first. Baking it means baking the file's order, which is a
different table rather than a fourth byte.

**`MapTerrain::check` is an oracle and nothing else.** No production caller
reaches it — `can_step`, `land_at`, `surface_at` and `predict_step` all read
`Spans::check`. It stays because it is the only statement of the rule *in terms
of the map files*, and `span_check` is 248 million comparisons of one against
the other. Do not delete it as dead.

**What it is worth, measured:** a node expansion is **208 ns** where it was
1,105, and a search from Britain's castle 0.168 ms where it was 0.793. A node is
now ~200 ns of terrain against ~190 ns of A\*'s own heap and hash, which is the
ceiling this could reach; the lever that is left is *fewer* nodes, which is the
coarse graph. The numbers, the four attempts that did not move them, and the
profile that closed the question are in
[the record](evidence/2026-08-25-the-span-layer.md#the-closing-measurements).

## The live half, and what it does not carry

The overlay is the half a bake can never hold, so what it holds decides what the
bake is *allowed* to assume. Read off the workspace, not off the design:

| | where it lives | |
|---|---|---|
| **a door** | an entity, `Blocks { door: true }` | ✅ **already right** |
| **a placed crate, a house wall** | an entity, `Blocks` | ✅ already right |
| **a hull** | a plank, `Blocks` | ✅ already right |
| **a moored deck** | a plank, `Stands` | ✅ already right |
| **another mobile** | `Footing`'s fourth field, off the sector grid | ✅ settled since |
| **a house floor or stair** | the overlay's `Stands`, laid by `Cover::of_static` | ✅ era R's R3 |

**Doors are the case that needs nothing.** A door is an entity and the doorway it
hangs in is *an open gap in the statics by construction* —
[`overlay.rs`](../../crates/common/map/src/overlay.rs)'s header says so and
it is why this works. A door therefore never reaches the bake, cannot be baked
shut, and its two readings stay the `Doors` enum. The span grid is simply blind
to doors, which is the correct relationship.

**Bodies block, and the span grid was right to stay out of it.** `Footing` grew
a fourth field, `Bodies`, built out of the sector grid at the question and read
by `walk::landing`. No span, no `Cover`, no overlay entry: the bake is still over
ground that has nobody on it, which is what keeps a *corridor* a statement about
topology. See *a mobile is not an obstacle* in
[the shove record](evidence/2026-08-24-mobiles-and-the-shove-rule.md).

**A house floor is a surface, and that was the one hole.** `grep -rn
"CoverKind::Stands" crates` had a single producer in the workspace — a ship's
deck — so a placed multi contributed walls and nothing else: its ground floor
was walkable because the map's own ground is underneath it, and **its upper
storey stood on nothing**. `Cover::of_static` grows the `Stands` arm for a
platform tile now, with the same climbable halving `stand_surfaces` applies to a
map static, so the two agree by construction rather than by resemblance. The
reasoning is [era R's R3](evidence/2026-08-23-era-r-the-map-you-hold.md#r3--a-house-has-floors).

**The bake rule is stated by the type.** `Spans` is built below the live layer,
so a door, a crate and a house floor are invisible to it *by construction*
rather than by each builder remembering. A route onto a second storey therefore
comes from the overlay at query time — `walk::climbed`, the highest live surface
in reach and above what the map answered — and the span grid never claims one.

## What a node is, and the z that is already gone

**A search node is a place to stand, not a tile.** The key is `x`, `y` and `z`
in forty bits of a `u64` — the tile *and* the height a body's feet are at on it
— so a column with two floors gets two slots in the closed set, a route may pass
over a bridge and later under it, and a body on a house's first floor is not the
same node as one on the ground beneath.

**It is not a span index, and the reason generalises.** A span is a fact about
the *map file*, and the surfaces a search lands on are not all the map's: a
house's storey, a ship's deck and a placed stair are the
[`Overlay`](../../crates/common/map/src/overlay.rs)'s. The height is what both
layers speak, because a *landing is* a height. Anything keyed by span alone is a
structure the live world cannot be placed into — which is why the coarse graph,
which *is* keyed off the map's own places, is baked from the bare map and has
the live world applied at refinement time.

**A destination is a point and a node is a place, and resolving between them is
half of it.** Almost no caller has the exact z of the surface it means — the
coarse graph's nodes carry the land's height under the bridge they mean the deck
of, a client's click carries whatever the tile it hit was drawn at.
`path::goal_node` resolves the caller's z against what is actually there, the
map's spans and the live world's surfaces together; ties go to the lower surface
so the answer does not depend on which layer was read first. The start's own
column offers the start's own height, because a body standing somewhere is proof
that there is somewhere to stand.

**A node budget is not a tile budget.** `budget` bounds *finalised nodes*, and a
column with two floors can be finalised twice, so the same 400 buys marginally
less ground than it did before the key carried z. On Britannia that is 0.6% of
columns; whoever re-argues 400 for server AI or 600 for a client plan should
know the unit changed under the number.

### Down is not up, and the graph knows

The step rule is **asymmetric**: a climb reaches `start_top + 2`, and a descent
is unbounded — `check` accepts any platform whose top is within reach, including
one far below. Stepping off a platform you cannot step back onto is ordinary
behaviour, not a special case wanting a mechanism.

The coarse graph used to be unable to represent it: a portal existed only where
`step_allowed` succeeded *in both directions*, so every one-way drop was deleted
from the graph. **Portal edges are directed now** — a portal joins two places in
one direction and the reverse is a separate edge that may or may not exist — and
**5,903 of facet 0's 103,774 portal edges have no reverse**. What that leaves
off-mesh links is what they always should have been: connections geometry does
not imply *at all*, a teleporter and whatever a flood says is still unreachable,
rather than the place a drop would have been declared by hand.

## Decisions, taken here

**Three tiers, because the map has three populations.** 73.7% of blocks and
92.1% of columns hold no statics at all. The tiers are not a compression of a
uniform structure; they are the structure the census found.

**A span is stored only where the land grid cannot answer.** 96.5% of the
facet's standable surfaces are `average_land_z`. Storing them would be storing a
second copy of the land, and a second copy is a thing that can disagree.

**Water is a flag on a surface, not a second artifact.** Ability is per-query,
so the structure offers and the asker filters.

**One artifact or none, never two.** If the spans are ever written to disk they
go in the graph's file under a bumped version. Two files stamped separately are
two revisions of one world waiting to happen.

**The block index is the map's own.** `WorldMap` already blocks by 8×8 and
already knows which blocks hold statics. A parallel index would be a second
thing to keep in step, which is the failure this whole document set catalogues.

**The bake and the view are two types, and the middle tier is why.** `SpanIndex`
is what is built and stored; `Spans` is what is asked, and it holds the map
beside the index because 92% of the facet is deliberately not in the index. A
single type would have to either borrow the map — and then it could not be
stored beside it — or store a second copy of the ground, which is the one thing
the middle tier exists to avoid.

**`Spans` is movement's own map, and it stays in `openshard-movement`.** It is a
projection of the two lower layers for one purpose — where a body may stand and
what it fits under — and it is not the world. The map crate holds the world;
movement holds what movement derived from it.

**The oracle is equivalence, not plausibility.** Nothing here is done because it
looks right. The layer asserts against `stand_surfaces` over the whole facet,
the step rule against `step_allowed` and against a whole-facet flood, the search
against bit-identical node counts, the graph against
`refused_but_walkable = 0`. Every one of those tools exists in the tree.

**The strict diagonal, one rule for everybody.** ServUO keeps two — a player
needs both flanks and everything else needs one — and this shard keeps the
strict one for players, creatures, the baked graph and the client alike. A
second rule would have to be threaded through `steps_out_of`, `find_path` and
the bake to buy a behaviour nothing has asked for. It is a deliberate divergence,
and where a lax reading would go is `steps_out_of`.

## What this supersedes

- [`terrain_seam.md`](research/terrain_seam.md)'s **E**, in one argument only:
  the landing half of a step ends at the bake rather than at the map. The
  `Overlay` half of E is untouched and is a precondition here.
- [`terrain_seam.md`](research/terrain_seam.md)'s **F**, in its precondition: F
  says the artifact is wrong before anyone reads it, and directed portals over
  places are what make it right.
- [the graph efficiency record](evidence/2026-08-23-navigation-graph-efficiency.md)'s
  **Phase 3**, the second hierarchy level, which was gated on facet-0 numbers
  and then on regions over spans: a second level over a one-storey model would
  have been a second level of the same mistake. What it still wants is its own
  end-to-end p95 measurement.
