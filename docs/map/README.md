# The map, and everyone who reads it

One world, six readers, and until now no owner. This folder holds the map
itself and the systems whose whole job is to answer a question about it.

## Start here

[`map_rebuild.md`](map_rebuild.md) — 🚩 **The map, in three layers.** The single
entry point for everything below: the matryoshka a runtime map actually is —
ground, statics, and the live layer over them — and the three eras the nine
plans here are ordered into. **R, the map you hold** (the tile table leaves the
file reader, the live layer joins the type, a house gets floors, the statics stop
being 120,745 vectors); **P, the map you search**; **S, the map you change.**
Read it before opening any of the plans below: it says which era owns what is
left of each, and it takes the decisions that were open between them.

## The plan being executed

[`realtime_map.md`](realtime_map.md) — **era R, in order.** The executable half:
what moves where, in which commit, with a done-when and a risk per node. R1 the
tile table leaves the file reader ✔ · R2 the live layer joins the type ✔ · R3 a
house has floors ✔ · R4 the statics become one immutable run · R5 one install,
one load. A session starts at **R4 or R5** — the two are independent of each
other and neither ever waited on R3.

[`handoffs/`](handoffs/) — where the work stands, one file per session. The
plans hold intent; a handoff holds state.

## The track — era S, half built

[`new_map_representation/`](new_map_representation/) — **a map we can change.**
The world was the player's own UO install, nothing in the engine could move a
coastline, and every bake was keyed to file mtimes. The track replaces that with
an imported base, committed patches, and one revisioned snapshot every reader
takes a handle to. **A0, A and B are built and C's first half with them** — the
shard runs on a base set it owns and a patch survives a restart. What is left —
the live publish, revisioned bakes, chunks to the client, the editor — resumes
after eras R and P. Start at its
[`README.md`](new_map_representation/README.md), and read
[`client_today.md`](new_map_representation/client_today.md) for the measured
backlog era R spends.

## The readers

| | |
|---|---|
| [`terrain_seam.md`](terrain_seam.md) | **Closed, and the record of how.** Six terrains, five of which were not terrains: a mask of what the live world put in the way, a rectangle, a memo table, the absence of a map. It ends at a search that takes explicit types with no trait on it, one `Overlay` both ends build, and a shard that owns its tile table. Its node 0 is the facet-0 oracle every number in the two plans beside it comes from — including the one-storey defect and `CachedTerrain`'s deletion. |
| [`navigation_spans.md`](navigation_spans.md) | 🚩 **The first storey — era P in full.** The layer HPA\* assumes underneath it and this engine never built: a column is a *list* of standable surfaces rather than one height, so a castle plateau is its own span instead of an island. Measured first — 1,462 ns a node expansion, of which A\*'s own machinery is noise, and 92.1% of columns hold no statics at all. Three tiers, under 20 MB, an oracle per node, a measured ×4 against a ×6.4 ceiling. **N0 done; the rest waits for era R.** |
| [`coarse_pathfinding.md`](coarse_pathfinding.md) | Long routes over static terrain, and what a route is allowed to assume. Superseded, by its own first line. |
| [`navigation_graph.md`](navigation_graph.md) | The graph itself: regions, components, portals. |
| [`navigation_graph_bake.md`](navigation_graph_bake.md) | The baked artifact, its stamp and its validation. |
| [`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md) | Making the bake and the search affordable. Phases 1, 2 and 4 built; phase 3 shut until spans exist. |
| [`interiors.md`](interiors.md) | 🚩 The building flood: which cells are inside, which floor a person is on, and a sealed room as a black area. |
| [`cutaway.md`](cutaway.md) | The older, global roof-and-height rule the interior policy is replacing. |
| [`radar.md`](radar.md) | 🚩 **The radar raster, and every window that draws it.** The inventory of the two LOD systems this client has, the numbers for the shipped facet, and the gap that used to be under every radar defect: nothing chose a level. **R0–R6 are built and their loose ends closed**; what is left is section 10, and R7 is a measurement phase that wants asking first. Read it before the two below. |
| [`minimap_lod_plan.md`](minimap_lod_plan.md) | The radar raster as a revisioned, chunked LOD cache — the contract it is built to. |
| [`minimap_lod_handoff.md`](minimap_lod_handoff.md) | Where that work stands, and the four defects it cost. |

They are here together because they are the same question asked five ways, and
because the track above changes the ground under all of them at once: each of
these bakes something off terrain, and each is currently keyed to the *files*
it was baked from rather than to a world revision.

Two neighbours that deliberately did **not** move: [`occluders.md`](../occluders.md)
and [`footprints.md`](../footprints.md) are static geometry for the lighting
rebuild and belong to that document set, even though they read the same
statics.

The third layer's content lives outside this folder too:
[`housing.md`](../housing.md), [`customisation.md`](../customisation.md) and
[`boats.md`](../boats.md) are what gets laid *over* the map, and
[`map_rebuild.md`](map_rebuild.md)'s R3 is where a house stops being walls
without floors.
