# Minimap LOD cache — handoff

## Implemented groundwork

Phase 1 is implemented in `client/render/src/radar.rs`, with the first cache
ownership step from Phase 2.

- `RadarChunkKey { facet, lod, chunk, revision }` is the complete identity of
  an immutable terrain raster. Its constructor is crate-visible: only
  `RadarCache` creates keys, so a window or player position cannot create a
  competing terrain cache.
- `RadarChunk` holds a complete fixed-size CPU product, and `RadarRegion` is a
  world-space draw request with no player marker or upload reason.
- `BASE_CHUNK_TILES` is 64 (eight `Map::BLOCK_SIZE` blocks). Base chunks are
  always 64 by 64; east/south cells beyond the facet are `UNKNOWN`. The shared
  `world_tile_to_base_chunk` conversion uses floor division/remainder, so tile
  `(64, 0)` is chunk `(1, 0)`, local `(0, 0)`.
- `build_base_chunk` uses the authoritative block-major `fill` walk. `bake`
  remains the whole-facet convenience/reference builder.
- LOD products are categorical reductions. `build_lod_parent` requires four
  direct children from the same facet/revision and uses majority colour voting.
  Ties select north-west, north-east, south-west, south-east order; `UNKNOWN`
  is a normal candidate and colours are never RGB-averaged.
- `RadarCache` owns current per-facet revisions and completed CPU chunks. A
  revision change makes prior products unreachable through normal lookup, and a
  worker result carrying an old revision is rejected at publication.
- `App::radar_cache` owns this cache outside `Windows`/`Screen`; closing a
  future minimap window cannot evict terrain products.

## Verification

The final local checks passed:

- `cargo fmt --all`
- `cargo test -p openshard-client-render radar --lib` — 18 passed
- `cargo test -p openshard-client-app --lib` — 316 passed, 3 ignored
- `git diff --check`

## Next work

1. Phase 2.2: map/static mutations must mark their base chunk dirty and
   recursively mark all LOD parents dirty.
2. Phase 2.3: add a bounded, coalescing producer queue that publishes only
   complete CPU chunks off the presentation path.
3. Phase 2.4–2.5: implement ready fallback selection and cache/queue counters.
4. Phase 3: introduce bounded GPU residency, chunk/UV selection with nearest
   sampling and scissoring, then draw overlays after terrain.

## Current limits

There is intentionally no producer queue, mutation hook, GPU atlas, minimap
window or renderer integration yet. `RadarCache` is the revision-safe CPU
owner those steps build on; it does not itself schedule work or render.
