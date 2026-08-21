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
`Drawn::Minimap` are a first-class window: one authoritative rectangle, sized
from the packed `SMALL_FRAME`/`LARGE_FRAME` gump art (`SMALL_EXTENT`/
`LARGE_EXTENT` fall back for an install that lacks it), dragged, raised and
hit-tested by `Windows` like any other. `M` opens it; a double left-click
toggles small/large, and ctrl+wheel over the window steps `zoom_steps`
(`Window::zoom`, `1.25^steps`). The terrain and its overlays are clipped to
the round frame (`Placement::circle`) and drawn at the classic 45°
(`Placement::rotation`), with the rim art layered on top so the keyed centre
of `SMALL_FRAME`/`LARGE_FRAME` shows generated terrain instead of blacking it
out.

Retired on the way: the whole-bitmap `RadarRenderer` and `radar.wgsl`. It was
phase 1's "replace the ambiguous whole-bitmap model", left standing as a stepping
stone; once the chunk path ran end to end it had no caller but its own tests, and
a second mutable radar texture nothing draws is a trap rather than a fallback.

## Verification

`cargo test -p openshard-client-render` (615 lib + 4 GPU radar-pass tests),
`cargo test -p openshard-client-app --lib` (364), `cargo clippy` on both crates,
`cargo fmt`. The GPU tests need a device and skip without one; they cover chunk
seams, the coarse stand-in under a built chunk, the marker's shape and place, and
the backdrop's edge.

## One defect the phases left behind

The window drew a black square and never anything else, on a shard whose
`radarcol.mul` and map both loaded. Two demands feed `RadarWorkQueue` — a
mutation's dirty keys, and a window asking for ground it is about to draw — and
reconciliation only knew about the first: `pending.retain(|key| cache.is_dirty(key))`
dropped every request the minimap had just made, because demand for never-built
ground is not an invalidation. The producer was handed an empty batch every
frame, no chunk was ever published, `select_ready` had nothing to answer with,
and the pass fell through to its `UNKNOWN` backdrop for as long as the window
stayed open.

`refresh_dirty` is now `reconcile`, and it keeps a pending key while the key is
still worth building: the facet's current revision, and no complete product
published for it yet. The second half matters as much as the first — a window
re-asks for all of its visible chunks every frame, so keeping every current key
would rebuild ready terrain forever.

## A second defect: the enqueue side raced the same wedge the dequeue side had already fixed

Zoomed out, the minimap filled from its north-west corner and stopped partway
down: everything north of some latitude had terrain, everything south of it
stayed `UNKNOWN` forever, however long the window stayed open. Not a transient
fill-in-progress state — a permanent line, because nothing ever revisited the
south half once it was skipped.

`take_for_producer_near` already exists to keep a *different* raster-order bias
off the *dequeue* side — its own doc names "a visibly displaced wedge after
rotation" as the reason coordinate order is wrong for a bounded batch. But the
*enqueue* loop, in `App::draw_from`'s radar-preparation block, still walked
`region_base_chunks` in its native north-to-south, west-to-east order and
called `RadarWorkQueue::request` unconditionally.
`request` silently refuses once `pending.len() + in_flight.len() ==
max_queued`; a zoomed-out, HiDPI, desk-scaled region can need more level-zero
chunks than the 512-key bound, and raster order means north rows always claim
the bound first — south rows past the cutoff were never even inserted into
`pending`, so `reconcile` never saw them, `take_for_producer_near` never built
them, and no fallback ancestor could exist either, since an ancestor only comes
from four *published* children.

The fix is symmetric with the dequeue side: `region_base_chunks_near(region,
player_chunk)` enumerates nearest-first, so when the bound is actually
exceeded, the *farthest* ring from the player goes unbuilt — a radius that
grows in, not a hemisphere that never resumes. The loop also skips a key
`radar_cache.get` already answers for, which used to cost a `pending` slot a
still-unbuilt chunk could have used, even though `reconcile` pruned it a moment
later.

## A third defect: the same wedge again, one layer further down, while standing still

Fixing the request queue did not fix the symptom on its own. Reported live,
against the fix above already applied, standing still (so recentring was
never in play): the round frame filled evenly at first, then the top kept
gaining detail while the bottom went back to plain backdrop, for a few
seconds, before settling on its own. Not the second defect's hemisphere —
this one *reverses itself* and stops, which the queue fix cannot produce on
its own (a starved chunk never becomes ready and then unready again).

The third bound is the GPU page cache: `RadarChunkRenderer`'s texture array.
While a zoomed-out region is still filling in, no single coarse ancestor
covers it yet (an ancestor needs every child of its family, and nearest-first
production reaches the family's farthest child last), so the draw list for
one frame is a *pile of individual fine products*, one per base chunk built
so far — nothing bounds that pile at the LOD grid, only the page cache does.

**The shipped 16 MiB cache has 1024 pages.**
`RADAR_CHUNK_CACHE_BUDGET`, `RADAR_CHUNK_PAGE_BYTES` and
`RADAR_CHUNK_CACHE_LAYERS` live together in `radar_pass.rs`; the window asks
wgpu for that many array layers, capped by the adapter's own limit. This pair
must stay together: previously the renderer was given a 16 MiB budget but the
device request inherited WebGPU's 256-layer default, so a 729-chunk minimap
silently drew only its nearest 256 pages. A unit test binds the byte budget to
the requested 1024 layers. An adapter that genuinely exposes fewer layers
still opens and takes the fallback below; its log names the constrained count.

Before that correction, once the pile passed 256, `render_region` used to
`draws.truncate(capacity)` straight after `select_region_chunks`' own sort —
coarsest first, then
**north row before south row within a LOD**. That order is right for
*painting*: a stand-in has to go down before whatever paints over it. It is
wrong for *choosing what to drop*, and dropping by it reproduces the second
defect's exact shape one layer further down the pipeline: as the fine-product
pile grew past 256, frame over frame, more of it fell off the truncated tail,
and the tail is disproportionately south (higher `chunk.y` sorts later). The
picture reads as gaining detail at the top and losing it at the bottom, and
it stops the moment enough families complete that a coarse ancestor collapses
many small entries into one and the pile drops back under 256.

`cap_draws_by_distance` (`radar_pass.rs`) replaces the blind truncate: when
over budget, sort by distance from the *window's own centre* — using each
draw's already-computed screen placement, not its LOD or its chunk
coordinate — keep the nearest `capacity` of them, then re-sort that survivor
set back into paint order before drawing. The shortfall at the same budget is
unchanged; only its shape is. It now comes off the edge of a disc centred on
the player, not off one compass direction of the window.

## A fourth defect: the corner ring the rotation never needed

Reported live again, on top of all three fixes above: the round frame filled
in, but the terrain diamond sat visibly smaller than the circle — flat black
wedges between its edges and the frame — and tiles that did load read as
"the same small size as before," not adapting to zoom. Two symptoms, one
cause, and it was a "one source of truth" gap exactly as guessed.

`native_extent` — how many world tiles the minimap fetches — was computed
twice: once in `render_passes.rs`'s draw block, once in `App::draw_from`'s
radar-preparation block. Both carried a `sqrt(2)` factor, added (with a
comment to match) on the belief that a 45°-rotated square needs to be bigger
than its own circle to avoid leaving the circle's corners uncovered. That
belief doesn't hold: a square whose half-side already equals the circle's
radius fully contains that circle *at any rotation* — its edges are tangent
to the circle, never short of it, wherever the rotation puts them (the
`sqrt(2)` intuition conflates a square's *diagonal* against the circle's
*diameter*, the wrong pair; the relevant pair is the square's *half-side*
against the circle's *radius*, and those already match by construction). The
factor bought no coverage. What it cost was real, in two ways at once:

- **Scale.** The extra tiles got mapped onto the *same* on-screen square
  (`select_region_chunks`' `scale_x = at.extent / region.extent` divides them
  away), so each tile ended up roughly 29% smaller on screen than the "one
  tile, one physical pixel" contract the surrounding comment itself states.
  Tiles read as undersized at every zoom level, which is what "the same small
  tiles as before" was — the zoom knob was moving the world shown, not the
  size anything rendered at.
- **Priority.** The extra ring was pure margin *beyond* what the circle
  needed, and it was also the single farthest ground from the player within
  the region. Under this session's nearest-first production order (the fix
  for the second defect, above), that ring is always the last thing built —
  and it can never earn an LOD stand-in either, since an ancestor needs every
  descendant built at least once, which unvisited edge ground never gets. So
  it stayed backdrop indefinitely, which a person sees as "the map is smaller
  than the window."

`radar_native_extent` (`panes/minimap.rs`) is now the one function both
`render_passes.rs` and `App::draw_from` call for this arithmetic, with no
`sqrt(2)` in it. One consequence worth naming: the region fetched for a
rotated, circular window is now the *same size* as it would be for an
unrotated rectangular one — rotation costs nothing extra, because a square
already covers its own circle.

**Reported live once more, right after this fix landed:** the black wedges
were gone, but a thin ring of backdrop tiles remained right at the frame's
own edge, and it read as under-rounding. It was — "tangent" is an exact
answer with no slack in it, and two ordinary sources of slop each eat into an
exact answer: `.round()` can round the true value *down*, and a nearest-
sampled chunk texture is a handful of axis-aligned quads standing in for a
true circle, which does not paint flush to a zero-width mathematical line.
`radar_native_extent` now rounds up (`.ceil()`, never `.round()`) and adds a
small margin for exactly this slack.

**That margin has gone through three shapes, each wrong for a different
measured reason.**

1. **A flat two tiles a side.** Covered the small classic frame; reported
   live on the large frame (`M`'s own double-click, `LARGE_FRAME`/
   `LARGE_EXTENT`), it measured visibly short. Whatever this slack pays for
   scales with the window's own *size*, not with a constant.
2. **A fixed 3% of `content_extent` alone, no `zoom` in it.** Covered both
   frames at their default zoom; reported live at the large frame's *maximum
   zoom-out*, it thinned to an almost-invisible stripe. One world tile is
   `zoom` physical pixels (the fetch's own contract, from the second defect's
   fix), so a margin counted in tiles and held constant is a margin that
   shrinks in actual screen pixels as the window zooms out — backwards from
   what a fixed *visual* seam needs.
3. **`margin_fraction * content_extent / zoom`** —
   [`TANGENT_MARGIN_FRACTION`](../../crates/client/app/src/panes/minimap.rs), now
   the margin's *physical* size, converted to a tile count only at the end,
   the same way the main fetch already was. This is the shape that survived:
   it scales with the window (fixes report 1) and with `zoom` (fixes report
   2), and it deliberately does **not** scale with `magnify` or
   `device_scale` — the physical seam this pays for does not depend on HiDPI
   or desk scale, and multiplying it by them too, on a zoomed-out HiDPI
   window, is exactly how this margin would balloon back into the
   corner-ring starvation the `sqrt(2)` factor caused. `TANGENT_MARGIN_FRACTION`
   itself stays a few percent, not the `sqrt(2)` term's ~41%, because it is
   additive slack, not a second multiplicative factor on the whole fetch.

Three different bugs, three different reasons a rotated circular minimap's
fetch size kept drifting off from what it actually needed — all three now
live in the one function, with the reasoning for each pinned beside it
rather than in a commit message.

## Next work

1. **Phase 2.2 has no caller.** `RadarCache::invalidate_tile` is written and
   tested, and nothing in the client mutates map or statics yet, so no terrain
   edit ever reaches it. It becomes real work when something can change the
   ground — housing, or a shard-sent static.
2. **Phase 2.5, counters.** `RadarCacheCounters` and `RadarWorkCounters` exist
   and nobody reads them. They belong beside `CompositeTelemetry` in the HUD,
   which is a UI addition and wants asking first.
3. **Phase 4's decoration.** A close affordance; `M` is provisional and named
   so in `event_loop.rs`. The gump frame itself (`SMALL_FRAME`/`LARGE_FRAME`,
   round clip, 45° rotation, zoom) is built — see above.
4. **Phase 5, measure and soak.** No radar numbers are in the frame report, so
   "walking costs no raster work" is currently an argument rather than a
   measurement. Two independent bounds — `RadarWorkQueue`'s 512-key request
   queue and `RadarChunkRenderer`'s adapter-capped 1024-layer GPU page cache — are both
   untested against a real HiDPI + desk-scale + fully-zoomed-out worst case;
   both fixes above make exceeding their bound degrade radially instead of
   directionally, but neither proves the bound is never exceeded in practice,
   especially on an adapter that cannot expose all 1024 layers.
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
