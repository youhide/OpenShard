# Minimap LOD cache plan

## Outcome

The minimap is a regular client window. Its placement, z-order, dragging,
close behaviour and input travel through `Windows` and `Pane`, as they do for
a container or skill sheet. Its terrain is a revisioned, chunked LOD cache: a
player step changes the sampled region and the player marker, never causes a
terrain raster walk or texture upload.

This is not a plan to turn radar pixels into `GumpArt`. `gumpart` is immutable
UI decoration; radar terrain is generated data. The commonality is the window
layer and its painter ordering. The radar producer is a content renderer called
while that window's layer is recorded.

## Non-negotiable contracts

- One owner defines a radar cache key: facet, chunk coordinates, LOD and source
  revision. There must be no second cache keyed only by player position.
- Terrain and static changes invalidate their intersecting base chunks and all
  derived parents. The next frame may show a safe coarser/older ready level, but
  never uninitialised pixels or a hole.
- Pixel generation and GPU uploads are bounded work outside `App::draw_from`'s
  hot presentation path. Recording an already-ready window layer is allowed.
- The player, waypoint and transient entities are overlays. They do not alter
  cached terrain pixels or invalidate terrain chunks.
- LOD levels are semantic raster products, not linearly filtered mipmaps:
  colours are categorical map colours, so a coarser tile needs an explicit
  reduction rule and nearest sampling.
- Every minimap surface is clipped to its window bounds and recorded at the
  point implied by `own_windows` painter order.

## Where this stands

**Read [the cache's own record](evidence/2026-08-22-minimap-lod-cache.md)
first.** It is the score to this plan's
model: which phases are built, what was retired on the way, and what the next
session picks up. This document is the shape the work has to keep, and it does
not change as the work lands.

## Phase 1 — define immutable raster products

1. Replace the ambiguous whole-bitmap radar model with explicit
   `RadarChunkKey { facet, lod, chunk, revision }`, `RadarChunk` and a
   `RadarRegion` draw request.
2. Establish one fixed base chunk size, aligned to `WorldMap::BLOCK_SIZE`;
   document the border rule and conversion from world tile to chunk/local tile.
3. Move the existing colour walk into a chunk builder. `radar::bake` remains a
   convenience/reference builder and the chunk builder must produce the same
   pixels for the equivalent rectangle.
4. Define and test the LOD reduction rule before writing GPU code. It must
   preserve a deterministic representative colour (including `UNKNOWN`) and
   must not average RGB values.

Acceptance:

- Tests cover a base chunk at map edges, static-over-land precedence and an LOD
  parent derived from four children.
- A cache key cannot be constructed without a revision and LOD.
- No renderer API accepts player position as a reason to upload terrain.

## Phase 2 — cache ownership, invalidation and bounded production

1. Add one cache owner beside the map/content revisions, not beside the UI
   window or `Screen`; it survives closing the minimap and is invalidated by
   world/map changes rather than window events.
2. Track dirty chunks by source revision. An edit marks the base chunk dirty and
   recursively marks its LOD parents dirty.
3. Add a bounded producer queue. It builds chunks off the presentation hot path
   and publishes only complete CPU products; duplicate requests coalesce.
4. Set an explicit fallback policy: a missing requested LOD uses the nearest
   ready coarser ancestor or last matching revision, never a blank texture.
5. Add counters for requested, ready, stale, queued, rebuilt and evicted chunks.

Acceptance:

- A one-tile mutation rebuilds only its base chunk and ancestors.
- Walking over unchanged terrain produces zero cache builds and zero terrain
  texture uploads.
- Queue growth, memory budget and fallback state are visible in frame diagnostics.

## Phase 3 — GPU residency and radar content pass

1. Give ready chunks GPU residency through a bounded atlas/page cache (or a
   texture-array equivalent chosen with target-size limits in hand).
2. Upload a chunk exactly when its CPU revision becomes GPU-resident; do not
   rewrite a monolithic radar texture each step.
3. Make the draw request select chunks and UVs for the requested LOD, with
   nearest sampling and window scissoring.
4. Draw player/waypoint overlays after terrain in the same window layer. Their
   per-frame data is small and independent of chunk uploads.
5. Add offscreen tests for chunk seams, map edges, LOD transition, scissor and
   marker ordering.

Acceptance:

- A steady walking trace changes uniforms/overlay geometry only.
- The radar has no seam or blended coastline across chunk/LOD boundaries.
- GPU residency respects its byte budget and evicts only recreatable products.

## Phase 4 — make it a first-class window

1. Add `WindowSubject::Minimap`, a `MinimapPane`, and a corresponding drawn
   shape with one authoritative hit rectangle.
2. Route open/close, raise and drag through `Windows`; choose and document its
   open affordance separately (paperdoll button, hotkey or command).
3. Extend `draw_gump_windows` into the common window-layer recorder: it asks a
   pane for gump art/text as today and records specialised content at that
   window's painter position. A minimap must not be a detached HUD pass after
   all windows.
4. Add interaction tests for drag, raise, hit bounds and z-order.

Acceptance:

- The minimap behaves like every local window under drag, close and z-order.
- Closing it does not evict its terrain cache; reopening it does not rebuild
  ready chunks.

## Phase 5 — measure and soak

1. Add a deterministic scenario: open minimap, walk across chunk boundaries,
   zoom across LOD thresholds, mutate terrain/static data, then reopen it.
2. Record CPU raster time, queue depth, uploads/bytes, cache hit rate, chosen
   LOD and GPU draw time in the frame report.
3. Soak a connected client while walking and panning for several minutes on a
   discrete GPU and an integrated GPU.

Acceptance:

- Normal movement has no radar-driven CPU raster work or terrain upload.
- Invalidations are bounded and observable; no stale pixels survive their source
  revision after the cache reports ready.
- No visual gap, mixed-LOD seam or input/z-order regression appears in soak.

## Deliberately deferred decisions

- Exact base chunk size and cache byte budget: choose from adapter limits and a
  measured chunk-build benchmark in phase 1, not by habit.
- LOD reduction rule: colour voting, centre-sample and priority classes have
  different readability trade-offs and need fixture comparison.
- Dynamic world items: decide whether they belong to a separate overlay cache
  or should invalidate terrain only after the static-map path is proven.
- Opening affordance and gump frame artwork: UI product decisions, independent
  of cache correctness.
