# The map, in three layers

Nine plans live in this folder and its track, each right about its own half, and
none of them says what a map **is** at runtime. That is the gap this document
closes, and it is why the order between them looked arbitrary: the plans were
read in the order they were written rather than in the order the thing they
build depends on.

The want has not changed since [`overview.md`](new_map_representation/overview.md)'s
first line — *a world we can edit*. What changed is the discovery underneath it.
A world we can edit needs a **storage** answer (a base, patches, a snapshot) and
that answer has largely landed. It also needs a **runtime** answer, and nobody
wrote one: the thing a tick, a frame, a step and a route all read. That is
`WorldMap` today, and today it is two of the three layers a world actually has.

**The map is a matryoshka: ground, statics, and the live layer over them.** One
type, held by both ends, so that "the world" is a thing you take rather than a
thing each reader assembles. Everything below is ordered by that sentence.

This document is the entry point for the whole area. It consolidates nine plans,
says which era owns what is left of each, and takes the decisions that were open
between them. The plans themselves stay where they are: read one for **how** its
half was built and why, not to find out what is next.

Track index: [`README.md`](README.md)

## The map, in one type

Three layers, and what orders them is **how fast each one changes**:

| layer | what it holds | changes when | owner today |
|---|---|---|---|
| **the ground** | one `LandCell` per column — a land id and a height | a patch is published | [`LandGrid`](../../crates/common/map/src/grid.rs) inside `WorldMap` ✅ |
| **the statics** | what the importer laid down: walls, trees, floors, roads | a patch is published | `WorldMap.statics`, `Vec<Vec<StaticItem>>` ⚠ |
| **the live layer** | doors, crates, ship decks, house walls **and house floors** | between two ticks | [`Overlay`](../../crates/common/map/src/overlay.rs), the third field of [`World`](../../crates/common/map/src/world.rs) — projected by `FacetState` on the server and rebuilt per view by `clutter::fill` on the client ✅ |

The invariant that makes this a matryoshka rather than three fields in a struct:

> **What may be baked is exactly what is below the live layer.** A navigation
> graph, a span grid, a building flood, a minimap raster — every one of them is
> derived from the ground and the statics, and none of them may contain a door,
> a crate, a moored deck or a house. A reader takes the whole map; a bake takes a
> revision of the two layers under the live one.

That is not a new rule. It is what
[`navigation_graph.md`](navigation_graph.md) means by *"built once from the
static terrain"*, what [`overlay.rs`](../../crates/common/map/src/overlay.rs)
means by *"an overlay is not a terrain"*, and what
[`terrain_seam.md`](terrain_seam.md)'s node E landed as a type. What is new is
that it is stated once, for all three layers at once, by the type that holds
them — instead of being remembered separately by each bake.

### What was assembled at the reader, and is not any more

Both ends used to compose the layers themselves, and neither composed them the
same way:

```text
server   FacetState { map: Option<MapSnapshot>, overlay: Overlay,
                      obstructions, boats, coarse, sectors, regions, … }
client   Resources  { map: MapSnapshot, … }  +  clutter::of(view) -> Overlay
```

The server's overlay is a projection kept in step by `FacetState::refresh`; the
client's is built whole and thrown away whole, per view. That much was always
*right* — they have different lifetimes because the two ends know different
things. What was wrong is that the map and the layer over it were two values a
caller carried in a pair, so every new reader was another place to forget one of
them. E made the *contents* of that layer one type;
[`realtime_map.md`](realtime_map.md)'s **R2** made the **holding** of it one type
as well, and both ends now hold a `World`:

```text
server   FacetState { world: World, obstructions, boats, coarse, sectors, … }
client   Resources  { world: World, … }  +  clutter::fill(world.live_mut(), view)
```

The two remaining differences are the two that are real: who *owns* the indexes
behind the live layer, and what the client does with a base that is optional
only because a shard may have no files at all.

## The three eras, and why this order

```text
R. the map you hold  ──> P. the map you search ──> S. the map you change
   ground, statics,       spans, regions,           live publish, revisioned
   the live layer,        the graph the server      bakes, chunks to the
   one type both ends     finally reads             client, the editor
   hold
```

**R first**, because every other plan takes the map as an argument. A span grid
built over `Vec<Vec<StaticItem>>` and a span grid built over a CSR base are the
same code written twice, and a house floor that does not exist yet is a
pathfinding defect that looks like a pathfinding bug.

**P second**, because it is *derived*: `Spans` is a projection of the two lower
layers, and the region graph is a projection of `Spans`. Both substitute one
argument of a call whose other argument R is still shaping — which is exactly
the argument [`navigation_spans.md`](navigation_spans.md) already makes for
waiting on `terrain_seam.md`, one plan further along.

**S last**, because the half of it that mattered has already landed and the half
that is left publishes *revisions of a structure whose shape is still moving*. A
live publish that rebuilds touched chunks has to know what a chunk of the new
layout is; an editor previews through the runtime's own apply path. Both are
cheap after R and both are written twice if they go before it.

**What does not wait for any of this**: a defect a player can see in a reader —
the radar, the interiors flood, cutaway — is repaired when it is found. The eras
order *plans*, not repairs.

## Where the area stands

Read off the workspace and the plans, so a session does not re-derive it:

| | |
|---|---|
| the block order is one type's | ✅ `LandGrid`, [`snapshot.md`](new_map_representation/snapshot.md) phase 1 |
| one revisioned snapshot per facet, every reader holds a handle | ✅ phase 2 |
| the world is a crate; UO's files are an importer | ✅ phase 3 |
| our own chunk format, a base set, a shard that runs on it | ✅ [direction B](new_map_representation/plan.md#b--our-own-chunk-format-and-a-uo-importer) — 7,168 chunks, 102.6 MiB, byte-identical round trip |
| a world with a history: `Patch`, `publish`, the `.ospatch` log, one CLI | ✅ [direction C](new_map_representation/plan.md#c--patches-and-the-resolved-snapshot), first half |
| no trait on the search; `MapTerrain` is two borrows; one `Overlay` both ends build | ✅ [`terrain_seam.md`](terrain_seam.md), nodes 0 and A–E |
| the coarse graph is worth wiring up, and is wrong on raised ground | ✅ measured — [F, answered](terrain_seam.md#f--the-graph-nobody-reads) |
| the span census: 92.1% of columns hold no statics, deepest column is 12 | ✅ [N0](navigation_spans.md#n0--the-census-) |
| the tile table lives in the file reader | ✅ R1 — `openshard-tiles`, and `uofiles` is readers, formats and errors |
| the live layer is not in the map | ✅ R2 — one `World` on both ends |
| a house has walls and no floors | ✅ R3 — a platform lays two covers, and `can_step` climbs onto them |
| the statics are 120,745 vectors | ✅ R4 — one run, two allocations, 38.2 → 29.5 MiB |
| both ends load the same install separately | ✂ R5 struck — a shard and a client *are* two processes; only the test harness is one |
| a column is a *list* of standable surfaces | ✅ [N1](navigation_spans.md#n1--three-tiers-) — 16.5 MiB, baked in 0.05 s, equal to `stand_surfaces` on all 29.4 M columns |
| the step rule reads them | ✅ [N2](navigation_spans.md#n2--the-step-rule-reads-them-) — 248,268,125 steps and two whole-facet floods, 0 disagreements |
| the search stops asking the map | ✅ [N3](navigation_spans.md#n3--the-search-takes-spans) — 208 ns a node against 1,105, arrivals bit-identical |
| a column with two floors is two nodes | ✅ [N3b](navigation_spans.md#n3b--the-node-stops-being-a-tile) — the node is a place to stand, and 178 arrivals round Britain were lies |
| regions over spans | ✅ [N4](navigation_spans.md#n4--regions-over-spans) — the coarse graph refuses nothing the flood says is walkable, from any of five origins |
| the server reading the graph | ✅ [N7](navigation_spans.md#n7--the-server-reads-the-graph) — a creature walks a route flat A\* at budget 400 refuses, from a raised origin as well as a flat one |
| live publish, revisioned bakes, chunks to the client, the editor | ⬜ era S |

## Era R — the map you hold

**The executable plan is [`realtime_map.md`](realtime_map.md)** — the commit
order, the code sites, the tests and the risk per node. What follows here is the
era's shape and what each node owes; the two are the same split
[`plan.md`](new_map_representation/plan.md) and
[`snapshot.md`](new_map_representation/snapshot.md) already use, and progress
belongs in neither: it goes in [`handoffs/`](handoffs/).

### R1 — the table leaves the file reader ✔ built

**Goal.** `openshard-uofiles` reads files and does nothing else.

[`snapshot.md`](new_map_representation/snapshot.md)'s phase 3 made this argument
and applied it to one type. The world moved out of the `.mul` reader because
*"a shard that never had a client install cannot have a world while the world's
type is declared inside the reader"*. **The same sentence is true of the tile
table and was not acted on**, so today twelve crates depend on
`openshard-uofiles` and most of them want one thing from it: what a graphic *is*.

`TileData` is not a file. It is the table that says a graphic blocks, is a
platform, is climbable, is water, is a door, is this tall, weighs this much. A
base set that never met an install still needs one; a generated world needs one;
the live layer cannot build a `Cover` without one.

- A new crate — `crates/common/tiles`, `openshard-tiles` — holds `TileData`, its
  two entry types, `TileFlags`, and the ids that index it. `LandTile` moves
  there from `openshard-map`: an id belongs beside the table it indexes.
  `Graphic` stays in `openshard-protocol`, because it is on the wire.
- `openshard-uofiles` keeps the **reader**: `tiledata.mul`'s two layouts, the
  High Seas widening and the arithmetic that detects it, the group headers. It
  fills a `TileData` and hands it back, exactly as `uofiles::map` fills a
  `WorldMap`.
- [`surfaces.rs`](../../crates/common/movement/src/surfaces.rs) leaves too, and
  not to the same place. It is a **rule**, not a table and not a reader:
  `stand_surfaces` walks a column and says where a body could stand, which is
  movement's question and is the seed [N1](navigation_spans.md#n1--three-tiers)
  builds `Spans` out of. It goes to `openshard-movement`.
  `client/render` already depends on that crate, so the interior index that
  shares it loses nothing.
- **The dependency rule this strikes** is written in five places —
  [snapshot.md:308](new_map_representation/snapshot.md#L308),
  [:363](new_map_representation/snapshot.md#L363),
  [:509](new_map_representation/snapshot.md#L509),
  [plan.md:114](new_map_representation/plan.md#L114),
  [README.md:33](new_map_representation/README.md#L33) — as *"`openshard-map`
  depends on `openshard-protocol` and nothing else"*. It was a proxy for the
  property that actually matters, and it is replaced by that property:
  **`openshard-map` opens no files.** Depending on the tile table is not opening
  a file; it is naming what the world is made of.

**Done when:** `openshard-uofiles` exports readers only; `openshard-map`,
`openshard-movement` and `openshard-state` reach the table through
`openshard-tiles` and no gameplay crate depends on `openshard-uofiles` to ask
what a graphic is; `cargo check --workspace --all-targets`, `cargo test
--workspace`, `cargo clippy --workspace --all-targets` and `cargo fmt --all` are
silent.

### R2 — the third layer joins the type

**Goal.** One value is the map. A reader takes it and cannot be handed half.

- `Overlay`, `Cover`, `CoverKind` and `Doors` move from `openshard-movement` to
  `openshard-map`, beside the ground and the statics. They are *storage* — a
  span and a kind per tile — and storage is what the map crate is for. What
  moves with them is `Tile`; what does **not** move is the rule that reads them
  (`blocker_at`, `surface_at` stay behaviour on the type, `step_allowed` and the
  search stay in movement).
- The builders stay where the knowledge is. `Cover::of_static` takes a
  `StaticTile` from R1's crate; `FacetState`'s projection from `Obstructions` and
  `Boats` is the server's; `clutter::of` is the client's. **Neither end's
  builder moves** — E put the invariant behind four mutators on `FacetState` and
  that stays exactly as it is.
- What changes is who holds the result: `FacetState.map` and `FacetState.overlay`
  become one field, and `Resources.map` plus a per-view `Overlay` become one
  value the frame pins. `MapTerrain` keeps being a borrow-pair view over it, so
  no query signature changes shape twice.
- **The live layer is not part of a revision.** `MapSnapshot` revisions the two
  lower layers; the live layer is what the shard has put there since. A publish
  does not invalidate it and a bake never reads it. Say it in the type: the
  revision sits under the layer, not over it.

**Done when:** no production caller carries a map and an overlay as two values;
`grep -rn "Overlay" crates/common/movement` finds the rules that read one and
not the type; a bake cannot reach the live layer from the value it is given.

### R3 — a house is a layer, and it has floors ✅

**Goal.** You can stand on the second storey. **Built** — see
[`realtime_map.md`](realtime_map.md#r3--a-house-has-floors) for the node and
[`handoffs/`](handoffs/) for the session.

The open row in
[`mechanics.md`](new_map_representation/mechanics.md#open-with-what-would-close-it)
— *whether a house is a patch to the world or stays an entity overlay* — closes
in favour of **the live layer**, on the evidence already measured: a castle is
3,667 components over 31×32 tiles, so a house as a patch is a bulk insert into an
immutable base, and *"the flat layout refuses to let a house be anything but an
overlay"* ([`client_today.md`](new_map_representation/client_today.md)). It also
closes the way housing already built it: components are resolved at placement and
stored, which is [`housing.md`](../housing.md)'s D2.

What that leaves is the half nobody built. `grep -rn "CoverKind::Stands" crates`
has **one** producer in the workspace — `Plank::cover`, a ship's deck. Every
house component is either `Blocks` or absent, because
[`Cover::of_static`](../../crates/common/map/src/overlay.rs) is
`tile.flags.is_blocking().then(…)`. A floor is a *platform* and usually not
blocking, so it produces nothing at all: a house's ground floor is walkable
because the map's own ground is under it, and **its upper storey stands on
nothing**. `navigation_spans.md` flags this and correctly calls it housing's
defect rather than pathfinding's; here is where it is owned.

- `Cover::of_static` grows the other arm: a platform tile yields
  `CoverKind::Stands` at `z + height`, with the climbable halving — the same
  arithmetic `stand_surfaces` applies to a *map* static, which is what makes the
  two agree by construction rather than by resemblance.
- It is not double counting. `of_static` is called for an **entity's** component
  at placement; the base statics are read by the layer below and never enter the
  overlay.
- A tile may then carry several `Stands` at different heights — the map's ground,
  a house's first floor, a deck. `Overlay::surface_at` already picks the nearest
  to where the body is, which is the rule a two-storey house needs; what it has
  never had is a second entry to choose between. That is a test, not a design.

  **It was a design.** `surface_at` is asked by `can_step` only where the *map*
  refuses the tile outright — a deck over open water — and a house stands on
  ground the map answers for, so no number of `Stands` entries would ever have
  been read. R3 built the missing half as `climbed`: the highest live surface
  **in reach** and **above** what the map answered, which is Sphere's
  `GetFixPoint` rather than nearest-z. Nearest-z is still `aboard`'s rule for
  the deck, and reconciling the two is filed in
  [`roadmap.md`](../roadmap.md)'s *Backlog from R3*.
- **The picture and the footprint stop expanding a multi twice.** `net_command`'s
  multi expansion is the third path over the map and is named as out of scope by
  [`terrain_seam.md`](terrain_seam.md#out-of-scope-named); with the components
  producing covers at placement, one expansion feeds both.

**Done when:** a mobile walks up a house's stairs and stands on its second floor,
refused nowhere the ground floor is not; a test asserts a floor over open ground
is `Stands` and a wall over it is `Blocks`; the multi is expanded once. **Met**,
all three.

**What it must not do:** bake a house. A live house must be removable in one
operation, which is why it is a layer. Committing one into the base stays what
[direction F](new_map_representation/plan.md#f--the-editor) says it is — an
editor operation, one-way, after which the entity ceases to exist.

### R4 — the statics stop being 120,745 vectors ✅

**Goal.** The base layer is one immutable run, and the thing that changes is a
layer above it.

**Built.** The measured end of it is in
[`realtime_map.md`](realtime_map.md#what-it-measures) — 40,078,758 bytes in
120,745 allocations became 30,903,722 in two — and the one thing the node
decided against this plan is below.

Measured on the shipped facet: 2,906,871 statics across 120,744 non-empty
blocks, **120,745 allocations, 38.2 MiB**, median 18 per block. The CSR pair that
replaces it is **already written** — [`Chunk`](../../crates/common/map/src/chunk.rs#L154)
holds one flat run and a prefix sum — and nothing carries it into `WorldMap`
because `from_parts`, the four accessors and `place_static` are shaped around
per-block vectors.

Take the layout, and take the property under it:

- `statics` becomes a flat `Vec<StaticItem>` and a `Vec<u32>` of per-block
  offsets: 2 allocations, ~29.6 MiB, and every accessor still hands back
  `&[StaticItem]` — the block sort by `(y, x)` is unchanged, so
  `statics_at`'s two binary searches and `statics_in_row`'s contiguous row
  survive untouched.
- `place_static` as an in-place tail shift **does not exist**. A builder assembles
  the base; a patch that adds or removes a static rebuilds the blocks it touched,
  which is what [direction C](new_map_representation/plan.md#c--patches-and-the-resolved-snapshot)
  already says a publish does. `patch::apply_op` moves onto that path.
  **Built the other way round, and the reasoning is
  [in the node](realtime_map.md#what-the-node-decided):** the pair stayed and
  what changed is what it costs. `Vec::insert` on the run *is* "rebuild the block
  this touches", spelled as one move of the tail; the builder's job — a facet
  assembled without ever inserting an item — is `from_parts`', which takes the
  run and a count per block, and both importers go through it.
- **The packed record is a separate step, gated on a measurement.** Four bytes an
  item — `x`/`y` relative to the block, `hue` in a side table for the 0.95% that
  use one — is 38.2 → ~13.5 MiB and 16 items to a cache line instead of 6.4. It
  also changes accessors from handing back a reference to handing back a value,
  which is visible outside the type. It goes after N3's node-expansion
  measurement says whether the statics are still on anybody's hot path — **and
  after a second measurement that is the harder gate: whether the shift and the
  unaligned load cost less than the cache lines they save.** Size alone does not
  buy a packing. The land's own padding byte is 29.4 MB under exactly the same
  gate, and is filed in [`../roadmap.md`](../roadmap.md).

**Done when:** `WorldMap` holds two allocations of statics; the base is built
rather than inserted into; the resident size is recorded here; every statics test
and the base-set round trip pass unchanged. **Met**, with the second clause read
as it is written above: a facet is assembled by `from_parts` and never by
insertion, and the pair that inserts is the patch path alone.

### R5 — one install, one load ✂ struck

**Goal.** A process holds one facet, once.

**Struck, and the era ends at R4.** A shard and a client are two processes —
that is how the game is played, the client opening the install on the player's
machine and the shard opening its own world. The node's whole memory argument is
about `openshard-playground`, which is a *test harness* running both ends in one
process; optimising it means giving the two ends a shared handle they must not
have anywhere else. The correctness half — the two ends matching because they
opened the same install — is answered by the shard *telling* the client what the
world is, which is [direction E](new_map_representation/plan.md#e--to-the-client)
of era S and not a node here. The paragraphs below are kept as the reasoning
that was struck, not as work.

[`boot.rs`](../../crates/server/server/src/boot.rs) and
[`client/app/src/lib.rs`](../../crates/client/app/src/lib.rs) each load a facet
for themselves. Under `openshard-playground` that is one process holding two
~150 MiB copies of one facet, and — the half that is not about memory —
[`overview.md`](new_map_representation/overview.md) opens by saying the two ends
*"match because they opened the same install, not because either was told what
the world is"*. With the map one type, one loader can hand both ends the same
handle.

**Done when:** the playground loads a facet once and both ends read that value;
the shard's own loader is the only production `load_facet` call.

## Era P — the map you search

Inherited whole from [`navigation_spans.md`](navigation_spans.md), whose nodes,
measurements and DoDs stand as written. **N1 and N2 are built** — the span layer exists,
is 16.5 MiB, says what `stand_surfaces` says for every column of facet 0, and
the step rule over it answers what the step rule over the map answers — and N3
is where a session starts. Three things this consolidation adds:

- **`Spans` is movement's own map, and it stays in `openshard-movement`.** It is
  a projection of the two lower layers for one purpose — where a body may stand
  and what it fits under — and it is not the world. The map crate holds the
  world; movement holds what movement derived from it.
- **The bake rule is now stated by the type.** `Spans` is built below the live
  layer, so a door, a crate and a house floor are invisible to it *by
  construction* rather than by each builder remembering. R3's house floors are
  therefore the overlay's answer at query time, not the span grid's — a route
  onto a second storey comes from `surface_at`, and the graph never claims one.
- **N4 is the node a player notices**, and N7 is where they notice it. Both were
  gated on N2's oracles; those ran and passed, and **both are built** — the
  graph samples places and its portals are directed, so `coarse_bench` refuses
  nothing the flood reaches from any of the five recorded origins, and
  `ai::step_toward` asks that graph when its own exact search is refused past
  eight tiles. The shard's planner and the client's now fall back at the same
  distance, off the same constant.
- **The search's node has no z, and that turned out to be half a defect.** Asked
  in the session that wrote this document, answered in
  [what a node is](navigation_spans.md#what-a-node-is-and-the-z-that-is-already-gone):
  the flat A\* already keys on a planar tile and carries z as data, which is
  right — and it means a column with two standing places has *one* slot in the
  closed set, so a bridge and the ground under it are one node. The one-storey
  defect has a twin in the fine search. It is N3b, and the census makes it nearly
  free: the node becomes `(x, y, span)` with the index zero for 99.4% of the
  facet, twenty-nine bits of the `u32` the key already is. **The sharpest form of
  it is a wrong answer rather than a refusal**: asked to path from one floor of a
  column to another, the search compares flattened tiles, finds them equal, and
  returns *arrived* with an empty route — so an NPC told to walk to a mobile on
  the bridge above it believes it is already there.

[`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md)'s
phases 1, 2 and 4 are built; its phase 3 — a second hierarchy level — was shut
until N4, because a second level over a one-storey model is a second level of the
same mistake. **N4 is built, so that gate is spent**; what phase 3 still wants is
its own end-to-end p95 measurement.

## Era S — the map you change

The storage answer, resumed. What is already true — a base set, a patch log,
`publish`, one resolver both the shard and the bake go through — stays; what
waits is the rest of
[`plan.md`](new_map_representation/plan.md)'s directions:

| | |
|---|---|
| **C, second half** | a live publish: an edit taking effect in a running shard between two ticks, and reaching a connected client. The precondition landed with `terrain_seam.md`'s D — `FacetState.map` is a `MapSnapshot` the shard can take `&mut` of. What is left is **who** calls it and **where in the tick** |
| **D** | derived data keyed by the source revision instead of by file mtimes — the navigation bake and the building flood **already carry a `MapRevision` and refuse themselves on a mismatch**; the occluder measurements and the radar do not, and the radar's revision dimension has no production writer at all |
| **E** | whole chunks to our client, over a pipe chosen there and not before |
| **F** | the editor, and committing a house into the base as its one-way operation |
| **G** | residency and compression, still a constraint rather than a step |

**This reverses `plan.md`'s stated order**, which reads *"A0, then A, then B,
then C, with D following C closely"*. D following C closely was right when C was
the next thing; with R and P in front of it, a bake keyed to a revision would be
keyed to revisions of a layout R4 is still changing. The order is now R, P, S,
and `plan.md`'s own Order section carries a pointer here.

### A tile of ground moved — rebake, and never an overlay

The question this era is actually asked, and the one the plans answered in
pieces: an operator raises a tile of land and publishes. What happens?

**The map resolves it. There is no patch overlay, and there must not be one.**
[`MapSnapshot::publish`](../../crates/common/map/src/snapshot.rs#L165) applies
the ops to the world in place and moves the revision, and its own comment already
draws the line this section is about: *"it changes only the tiles the ops name —
no chunk is re-cut, nothing is re-hashed and no facet is rebuilt… what is
invalidated by it is every bake over `Patch::touched_chunks`, and that is
direction D's to notice."*

An overlay of *ground* would be a fourth layer, and it fails on every count the
three real ones pass:

- a reader would be applying patches itself, which
  [`mechanics.md`](new_map_representation/mechanics.md) forbids by name —
  nobody may see a half-applied world;
- two sources of truth for a column's height, on the hottest path there is: a
  node expansion reads sixteen of them;
- **it would break this document's own invariant.** A ground overlay has to be
  *in* the bake for the bake to be right — so it is not a live layer at all, it
  is the base with a slower spelling. The live layer is entities, and the ground
  is not an entity.

**So: rebake, and the whole question is how local.** A facet's navigation graph
takes 96 s to build, which is not a publish — it is an outage. Everything needed
to make it local already exists and nothing has been wired to it:

- [`Patch::touched_chunks`](../../crates/common/map/src/patch.rs#L236) hands back
  the chunks the ops fell in, deduplicated;
- the chunk is 64×64 and every artefact derived from terrain is already keyed to
  that grid, which is [`mechanics.md`](new_map_representation/mechanics.md#chunks)'s
  stated reason for the size — D's invalidation is one-to-one;
- the graph's regions are 32×32, so what a raised tile can change is bounded: the
  walkability of the column, the eight transitions into it, its region's internal
  components, and the portals of the region borders it touches. The neighbours
  whose answer *crossed* into the changed chunk are the second half, and they are
  the half a naive implementation forgets.

**What is genuinely undecided is the window.** Between the publish and the
finished rebake, a stale bake exists. Today's rule is that a stale artifact
refuses itself rather than answering from the old world — which means routing in
those chunks degrades to flat A\* until the rebake lands, and is the default this
plan inherits. The alternative is to rebuild the touched regions *inside* the
publish, so a revision is never visible without the derived data that matches it.
That is a real choice with a latency cost, it is C's and D's to take together,
and it should be taken with a measurement of a single-region rebuild rather than
by preference.

**To the client, whole chunks — never a stream of operations.** A client that
lost the connection mid-stream would hold a world that never existed. That is
[direction E](new_map_representation/plan.md#e--to-the-client), and the classic
2D client is out of it by design: it reads its own files and does not see our
changes.

## The readers, and what they owe

[`interiors.md`](interiors.md), [`cutaway.md`](cutaway.md),
[`radar.md`](radar.md) and [`minimap_lod_plan.md`](minimap_lod_plan.md) are not
in the eras: they consume the map rather than shaping it. Two things are true of
all of them and worth stating once.

- **Each bakes something off terrain, and they no longer agree about what makes
  one stale.** The navigation bake and the building flood carry a `MapRevision`
  and refuse themselves when the world has moved; the occluder measurements are
  keyed to files, and the radar cache has a revision dimension whose only writers
  are tests. Finishing that is D's, in era S — and a reader that cannot say which
  revision it was built from cannot be invalidated by a publish at all.
- **Each is a separate walk of the same statics.** `client_today.md`'s finding
  10 — *"the highest static on a tile is re-derived by linear scan in four
  places"* — is the shape of that, and R4 makes it cheaper without making it
  fewer. Whoever unifies the walk should read that finding first: file order is
  draw order, so the sort key cannot simply become z.

## Decisions, taken here

**The map may know what a tile is.** The rule *"`openshard-map` depends on
`openshard-protocol` and nothing else"* is struck where it is written, and
replaced by the property it was standing in for: **the map crate opens no
files.** Layering was never the argument — the readers were. A world made of
tiles that may not name the tile table is a world that cannot describe itself.

**A table is not a file, and a rule is not a file either.** `TileData` and
`stand_surfaces` leave `openshard-uofiles`, which keeps the parsing and nothing
else. This is phase 3's own argument, applied to what phase 3 left behind.

**Movement keeps its own map.** `Spans` is a derived structure with one purpose
and it lives with the rules that read it. The map crate is not where a search's
index goes, however much it looks like a third layer.

**A house is a layer, and its floors are surfaces.** The open row in
`mechanics.md` closes: an entity overlay, never a patch, and `Cover::of_static`
grows the `Stands` arm so that the storey a house draws is the storey a body may
stand on. A house committed into the base stays an editor operation and only
that.

**The base is immutable; everything that changes is a layer over it.** That is
what makes the CSR layout free rather than costly, what makes a publish a
rebuild of touched blocks rather than a memmove, and what makes the live layer's
existence a design rather than a workaround.

**The eras are R, P, S**, and every plan named here inherits that order over the
one written in its own text.

**Statics: CSR now, packing later.** Two allocations and an immutable base are
architecture; four bytes a record is an optimisation with an API cost, and it
waits for a measurement that says the statics are still on a hot path.

## What each document becomes

| | |
|---|---|
| [`overview.md`](new_map_representation/overview.md) | **live** — the want, unchanged. Still the only one you must read to argue about the idea |
| [`mechanics.md`](new_map_representation/mechanics.md) | **live**, minus one row: the house question is answered above |
| [`plan.md`](new_map_representation/plan.md) | **live** for C–G; its Order section is superseded by the eras here |
| [`snapshot.md`](new_map_representation/snapshot.md) | **the record** of A0 and A, plus the crate rule this document strikes |
| [`client_today.md`](new_map_representation/client_today.md) | **live** — the measured backlog era R spent. Finding 6 is R4 and is spent, finding 7 was R5 and is withdrawn, finding 10 is the readers' |
| [`terrain_seam.md`](terrain_seam.md) | **closed.** The record of how the seam went, and where the facet-0 oracle's numbers live |
| [`navigation_spans.md`](navigation_spans.md) | **live** — era P in full, N0–N4 and N7 done; N5 and N6 gated |
| [`navigation_graph*.md`](navigation_graph.md) | **live** — the graph, its artifact, and its efficiency phases 1/2/4 built; 3 was gated on N4 and is now gated only on its own measurement |
| [`coarse_pathfinding.md`](coarse_pathfinding.md) | **superseded**, by its own first line |
| [`interiors.md`](interiors.md) · [`cutaway.md`](cutaway.md) · [`radar.md`](radar.md) · [`minimap_lod_*.md`](minimap_lod_plan.md) | **live**, and not in the eras: readers, repaired when they break |
| [`housing.md`](../housing.md) · [`customisation.md`](../customisation.md) · [`boats.md`](../boats.md) | **live**, and the third layer's content: what gets laid over the map |

## Open, with what would close it

| question | what settles it |
|---|---|
| ~~**Do bodies block?**~~ **Taken: they do.** The shard refuses a step onto a living body and plans around one — `Footing`'s fourth field, built out of the sector grid at the question. The layer carried it unchanged, as this row said it would: no `Cover`, no overlay entry, no identity in `openshard-map`. See [`roadmap.md`](../roadmap.md)'s *a mobile is not an obstacle* | Settled. What is left is the client's end, which still plans through a crowd, and the *shove* — in UO a player at full stamina pushes past for 10 stamina rather than being stopped, and this engine hard-blocks on purpose |
| **Which components of a multi are floors** | R3 reads the platform flag, which is what a static floor already is. If a shipped house has a floor that flag does not mark, that is a finding about the table and belongs in [`findings.md`](../findings.md) |
| **Two floors over one tile, for the picture** | The step check chooses the highest surface in reach (`walk::climbed`, not `surface_at` as this table used to say); the *renderer* choosing a storey is [`interiors.md`](interiors.md)'s own subject, and R3 has given it the second surface it lacked |
| **The packed static record** | N3's node-expansion measurement: whether the statics are still on a hot path once spans exist |
| **The publish window** — a revision is visible before the rebake over its touched chunks finishes. Does routing there degrade to flat A\* until it lands, or does the publish carry the rebuild and pay the latency? | A measurement of one region's rebuild against the 96 s whole-facet bake. See [a tile of ground moved](#a-tile-of-ground-moved--rebake-and-never-an-overlay) |
| **Residency** | [direction G](new_map_representation/plan.md#g--residency-and-size-deferred-on-purpose), unchanged: the working set a real session touches against the cost of a miss |

## Where a session starts

**Era P — era R is over.** R1 to R4 are built: the tile table is
`openshard-tiles`, a crate with no dependencies at all; one `World` is the map on
both ends of the wire; a house has floors a body can stand on, which was the
first thing here a player would notice; and a facet's statics are one run. R5 is
struck, for the reason in its node, so there is no fifth.

What era R leaves behind is measurement rather than structure, and it is in
[`../roadmap.md`](../roadmap.md): the land's fourth byte is 29.4 MB of padding,
which is more than R4 saved. It is gated on the read staying as fast, and so is
the packed static record — a packing that costs a shift per read on the ground
walk is not an improvement, whatever it weighs.

**Where the work stands is [`handoffs/`](handoffs/)**, newest last — not here,
and not in the plan.

**Era P is no longer gated, and it has started.** The gate was R2 for the
*type*, and after that the shape of the layers `Spans` is a projection of: what
a house contributes to a surface (R3) and how the statics are held (R4). Both
landed, R5 — the only node that was left — is struck, and
[N1](navigation_spans.md#n1--three-tiers-) and
[N2](navigation_spans.md#n2--the-step-rule-reads-them-) are built: the span
layer is a real 16.5 MiB structure, and the step rule over it is proved to be
the step rule over the map. **The only risk in that plan is retired**, and
**N3 is the next node** — where the search stops asking the map at all.

**Era S is resumed, not restarted.** Its first half is built and running; the
handoffs in [`handoffs/`](new_map_representation/handoffs/) are where its state
lives, and the plan holds the intent.
