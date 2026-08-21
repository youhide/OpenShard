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
| [`coarse_pathfinding.md`](coarse_pathfinding.md) | Long routes over static terrain, and what a route is allowed to assume. |
| [`navigation_graph.md`](navigation_graph.md) | The graph itself: regions, components, portals. |
| [`navigation_graph_bake.md`](navigation_graph_bake.md) | The baked artifact, its stamp and its validation. |
| [`navigation_graph_efficiency_plan.md`](navigation_graph_efficiency_plan.md) | Making the bake and the search affordable. |
| [`interiors.md`](interiors.md) | 🚩 The building flood: which cells are inside, which floor a person is on, and a sealed room as a black area. |
| [`cutaway.md`](cutaway.md) | The older, global roof-and-height rule the interior policy is replacing. |
| [`minimap_lod_plan.md`](minimap_lod_plan.md) | The radar raster as a revisioned, chunked LOD cache. |
| [`minimap_lod_handoff.md`](minimap_lod_handoff.md) | Where that work stands. |

They are here together because they are the same question asked five ways, and
because the track above changes the ground under all of them at once: each of
these bakes something off terrain, and each is currently keyed to the *files*
it was baked from rather than to a world revision.

Two neighbours that deliberately did **not** move: [`occluders.md`](../occluders.md)
and [`footprints.md`](../footprints.md) are static geometry for the lighting
rebuild and belong to that document set, even though they read the same
statics.
