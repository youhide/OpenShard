# The map, and everyone who reads it

One world, six readers, and until now no owner. This folder holds the map
itself and the systems whose whole job is to answer a question about it.

## Start here

[`layers.md`](layers.md) — 🚩 **Which layer does this go in?** One page, one
question — *must a bake see it?* — and the table of every answer taken so far:
ground, statics, item, ship, house, customised house. Also the two rules in this
folder that read as a contradiction (*"never an overlay"* against *"a house is a
layer"*) and what actually separates them, and the four different things one art
id can be. Read it before quoting either rule at a question about the other.

[`map_rebuild.md`](map_rebuild.md) — 🚩 **The map, in three layers.** The single
entry point for everything below: the matryoshka a runtime map actually is —
ground, statics, and the live layer over them — and the three eras the nine
plans here are ordered into. **R, the map you hold** (the tile table leaves the
file reader, the live layer joins the type, a house gets floors, the statics stop
being 120,745 vectors — **all four built**); **P, the map you search**; **S, the
map you change.**
Read it before opening any of the plans below: it says which era owns what is
left of each, and it takes the decisions that were open between them.
**Era P has retired its defect: N1 to N4 are built, the shard walks on the span
layer, and the coarse graph is no longer a one-storey model of a two-storey
world.**

## The plan being executed

[`realtime_map.md`](realtime_map.md) — **era R, in order.** The executable half:
what moves where, in which commit, with a done-when and a risk per node. R1 the
tile table leaves the file reader ✔ · R2 the live layer joins the type ✔ · R3 a
house has floors ✔ · R4 the statics become one immutable run ✔ · R5 one install,
one load ✂ struck. A session starts at **era P**, in
[`navigation_spans.md`](navigation_spans.md): era R is over, and N1 to N4 and
N7 are built — the shard walks on the span layer at 208 ns a node instead of
1,105, a node is a *place to stand* rather than a tile, the coarse graph refuses
nothing the flood says is walkable where the castle plateau alone used to refuse
37 of 44, and **the shard reads it**: `ai::step_toward` asks the graph when the
exact search runs out of budget, which is where a player meets any of it.
Nothing in era P is open — N5 and N6 are both gated on a measurement nobody has
asked for. **Rebake first** — `ROUTING_VERSION` is 4 and a shard with an older
artifact does not boot.

[`handoffs/`](handoffs/) — where the work stands, one file per session. The
plans hold intent; a handoff holds state.

## The track — era S, and the map moves now

[`new_map_representation/`](new_map_representation/) — **a map we can change.**
The world was the player's own UO install, nothing in the engine could move a
coastline, and every bake was keyed to file mtimes. The track replaces that with
an imported base, committed patches, and one revisioned snapshot every reader
takes a handle to. **A0, A, B and C are built** — the shard runs on a base set it
owns, a patch survives a restart, and a **running** shard edits its own ground
from four staff verbs, with the log written in the one order that cannot leave a
revision nobody can reach. Chunks now reach connected clients, and the first
Game Master editor cut is described in [`editor.md`](editor.md): catalogue,
terrain/static brushes, local history and revision-checked commit. Revisioned
bakes and editor polish remain. Start at its
[`README.md`](new_map_representation/README.md), and read
[`client_today.md`](new_map_representation/client_today.md) for the measured
backlog era R spends.

**What is left of the representation itself is one document:**
[`what_a_change_costs.md`](new_map_representation/what_a_change_costs.md) — the
map we can change works, and a change to it still costs a facet. Six nodes: one
version 2 of the base set (deflated chunks, a hash per chunk, a minted world id),
products keyed by the chunk they were built from rather than by the facet's
revision, a block replaced where it stands instead of a 115.4 ms rebake at both
ends, a folded log, revert as a verb, and the `tiledata` and multis a shard still
borrows from somebody's UO install.

## The readers

| | |
|---|---|
| [`terrain_seam.md`](terrain_seam.md) | **Closed, and the record of how.** Six terrains, five of which were not terrains: a mask of what the live world put in the way, a rectangle, a memo table, the absence of a map. It ends at a search that takes explicit types with no trait on it, one `Overlay` both ends build, and a shard that owns its tile table. Its node 0 is the facet-0 oracle every number in the two plans beside it comes from — including the one-storey defect and `CachedTerrain`'s deletion. |
| [`navigation_spans.md`](navigation_spans.md) | 🚩 **The first storey — era P in full.** The layer HPA\* assumes underneath it and this engine never built: a column is a *list* of standable surfaces rather than one height, so a castle plateau is its own span instead of an island. Measured first — 1,462 ns a node expansion, of which A\*'s own machinery is noise, and 92.1% of columns hold no statics at all. Three tiers, an oracle per node, a measured ×4 against a ×6.4 ceiling. **N0–N4 are built** — the layer is 16.5 MiB, bakes in 0.07 s, equals `stand_surfaces` on all 29.4 M columns, answers what the map answers on 248,268,125 steps, and **the shard walks on it**: a node expansion is 208 ns where it was 1,105 and a search from Britain's castle 0.168 ms where it was 0.793. N3b spent that on the answer — a node is a place to stand, so 178 destinations that used to report an arrival are now refusals and a route between two floors of one column exists — and **N4 spent it on the coarse graph**: it samples places and its portals are directed, so `refused_but_walkable` is 0 from all five origins where the castle used to refuse 37 of 44, and the bake fell from 96 s to 11.7 s. **N7 spent it where a player is**: `ai::step_toward` asks the graph when its exact search is refused past eight tiles, so a body plans the same distance on both ends of the wire. Nothing in era P is open. |
| [`coarse_pathfinding.md`](coarse_pathfinding.md) | Long routes over static terrain, and what a route is allowed to assume. Superseded, by its own first line. |
| [`navigation_graph.md`](navigation_graph.md) | The graph itself: regions, components, portals — and **G1**, which is how it follows a publish instead of being dropped by one: two rings around the edit, 80 ms against the 28 s a whole bake costs. |
| [`navigation_graph_bake.md`](navigation_graph_bake.md) | The baked artifact, its stamp and its validation. |
| [`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md) | Making the bake and the search affordable. Phases 1, 2 and 4 built; phase 3's span gate is spent with N4 and only its own p95 measurement is left. |
| [`interiors.md`](interiors.md) | 🚩 The building flood: which cells are inside, which floor a person is on, and a sealed room as a black area. |
| [`cutaway.md`](cutaway.md) | The older, global roof-and-height rule the interior policy is replacing. |
| [`radar.md`](radar.md) | 🚩 **The radar raster, and every window that draws it.** The inventory of the two LOD systems this client has, the numbers for the shipped facet, and the gap that used to be under every radar defect: nothing chose a level. **R0–R6 are built and their loose ends closed**; what is left is section 10, and R7 is a measurement phase that wants asking first. Read it before the two below. |
| [`minimap_lod_plan.md`](minimap_lod_plan.md) | The radar raster as a revisioned, chunked LOD cache — the contract it is built to. |
| [`minimap_lod_handoff.md`](minimap_lod_handoff.md) | Where that work stands, and the four defects it cost. |

They are here together because they are the same question asked five ways, and
because the track above changes the ground under all of them at once: each of
these bakes something off terrain, and each is currently keyed to the *files*
it was baked from rather than to a world revision.

Two neighbours that deliberately did **not** move: [`occluders.md`](../render/design_occluders.md)
and [`footprints.md`](../render/design_footprints.md) are static geometry for the lighting
rebuild and belong to that document set, even though they read the same
statics.

The third layer's content lives outside this folder too:
[`housing.md`](../housing.md), [`customisation.md`](../customisation.md) and
[`boats.md`](../boats.md) are what gets laid *over* the map, and
[`map_rebuild.md`](map_rebuild.md)'s R3 is where a house stops being walls
without floors.
