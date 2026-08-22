# The map, and everyone who reads it

One world, six readers, and until now no owner. This folder holds the map
itself and the systems whose whole job is to answer a question about it.

## The track

[`new_map_representation/`](new_map_representation/) — **a map we can change.**
Today the world is the player's own UO install, nothing in the engine can move
a coastline, and every bake is keyed to file mtimes. The track replaces that
with an imported base, committed patches, and one revisioned snapshot every
reader takes a handle to. Start at its
[`README.md`](new_map_representation/README.md).

## The readers

| | |
|---|---|
| [`terrain_seam.md`](terrain_seam.md) | 🚩 **Six terrains, and one of them is a terrain.** The other five are actions taken over one — a mask of what the live world put in the way, a rectangle, a memo table, the absence of a map — and each was made a kind of terrain because the seam was a trait. The plan to end at `find_path(&MapTerrain, &Overlay, Doors)` with no trait on the search, plus the navigation graph the server loads and never reads and the hot path with no facet-0 measurement. **0 and A–D built:** no facet holds a terrain, `MapTerrain` is two borrows built per question, the shard owns one tile table outright, and the facet-0 oracle has run — it deleted `CachedTerrain` from the plan, reversed the hierarchy's only recorded verdict, and found the one-storey defect the plan beside it exists for. What is left is the `Overlay`, and every edge into it is met. |
| [`navigation_spans.md`](navigation_spans.md) | 🚩 **The first storey.** The layer HPA\* assumes underneath it and this engine never built: a column is a *list* of standable surfaces rather than one height, so a castle plateau is its own span instead of an island. Measured first — 1,462 ns a node expansion, of which A\*'s own machinery is noise, and 92.1% of columns hold no statics at all. Three tiers, under 20 MB, and an oracle per node. **N0 done**; N1 is the structure. |
| [`coarse_pathfinding.md`](coarse_pathfinding.md) | Long routes over static terrain, and what a route is allowed to assume. |
| [`navigation_graph.md`](navigation_graph.md) | The graph itself: regions, components, portals. |
| [`navigation_graph_bake.md`](navigation_graph_bake.md) | The baked artifact, its stamp and its validation. |
| [`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md) | Making the bake and the search affordable. |
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
