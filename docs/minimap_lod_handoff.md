# Minimap LOD cache — handoff

Where `docs/minimap_lod_plan.md` stands. The plan is the model; this is the
score.

## Built

**Phase 1 — immutable raster products.** `RadarChunkKey { facet, lod, chunk,
revision }` is the complete identity of a terrain raster, constructible only by
`RadarCache`. `BASE_CHUNK_TILES` is 64 (eight `Map::BLOCK_SIZE` blocks); base
chunks are always complete, with cells beyond the facet `UNKNOWN`, which is what
lets a parent be reduced from four children with no map-edge case.
`build_base_chunk` uses the block-major `fill` walk; `reduce_lod_pixel` votes on
categorical colours and never averages RGB.

**Phase 2 — ownership, invalidation and bounded production.** `App::radar_cache`
and `App::radar_queue` live with world content, not with a window, so closing the
minimap discards nothing. `RadarWorkQueue` coalesces requests, hands out
`builds_per_turn` keys and publishes only complete products; `abandon` returns a
slot the producer could not build, which is what stops a lost dispatch from
silently filling the bound.

**Phase 2.4 — the fallback is live.** The draw path asks `select_ready`, which
answers with the exact current product, else the nearest ready coarser ancestor,
else that chunk's own newest complete picture. `build_ready_ancestors` is what
makes an ancestor exist: the fourth child of a family reduces it into its parent
and climbs to `radar::MAX_LOD`. Nothing schedules a derived level, and none is
ever built from a family with a hole in it.

**Phase 3 — GPU residency and the content pass.** `RadarChunkRenderer` keeps
chunks in bounded texture-array pages, LRU-evicted, uploaded through an encoder
copy so a page cannot be overwritten before the draw that reads it.
`select_region_chunks` places each chunk by **its own** LOD, orders coarsest
first so a built chunk paints over the stand-in covering it, and gives one
product one draw however many requests fell back to it. Per-chunk data is an
instance buffer, never the uniform block — see the commit that fixed it, and
`docs/client.md`.

**Phase 3.4 — overlays.** `RadarOverlayRenderer` draws the solid rectangles: the
`UNKNOWN` backdrop under the terrain, so an unbuilt window is not a hole, and the
player's cross over it. `radar::MARKER_ARMS` is the one shape the bitmap stamp
and the overlay share.

**Phase 4, in part.** `WindowSubject::Minimap`, `MinimapPane` and
`Drawn::Minimap` are a first-class window: one authoritative rectangle
(`panes::minimap::EXTENT`), dragged, raised and hit-tested by `Windows` like any
other. `M` opens it.

Retired on the way: the whole-bitmap `RadarRenderer` and `radar.wgsl`. It was
phase 1's "replace the ambiguous whole-bitmap model", left standing as a stepping
stone; once the chunk path ran end to end it had no caller but its own tests, and
a second mutable radar texture nothing draws is a trap rather than a fallback.

## Verification

`cargo test -p openshard-client-render` (604 lib + 4 GPU radar-pass tests),
`cargo test -p openshard-client-app --lib` (350), `cargo clippy` on both crates,
`cargo fmt`. The GPU tests need a device and skip without one; they cover chunk
seams, the coarse stand-in under a built chunk, the marker's shape and place, and
the backdrop's edge.

## Next work

1. **Phase 2.2 has no caller.** `RadarCache::invalidate_tile` is written and
   tested, and nothing in the client mutates map or statics yet, so no terrain
   edit ever reaches it. It becomes real work when something can change the
   ground — housing, or a shard-sent static.
2. **Phase 2.5, counters.** `RadarCacheCounters` and `RadarWorkCounters` exist
   and nobody reads them. They belong beside `CompositeTelemetry` in the HUD,
   which is a UI addition and wants asking first.
3. **Phase 4's decoration.** A gump frame and a close affordance; `M` is
   provisional and named so in `event_loop.rs`.
4. **Phase 5, measure and soak.** No radar numbers are in the frame report, so
   "walking costs no raster work" is currently an argument rather than a
   measurement.
5. **`bake` and `mark` have no caller** but their own tests, the same shape the
   retired `RadarRenderer` had. They are the CPU whole-facet path — worth keeping
   only if something is going to want a whole-map image; otherwise they follow it
   out.
6. **Markers are only the player.** Party, waypoint and corpse are the same
   overlay and the same cross; what is missing is the decision about which of
   them belongs on a minimap, not the drawing.

## Known limits

Base chunks are the only level ever requested; derived levels appear only where a
family completes, which walking over new ground does not do. The fallback
therefore earns its keep on revisited ground and after an edit, not on a first
visit — that is by design, since building a coarse picture of ground nobody has
rastered would mean rastering it.
