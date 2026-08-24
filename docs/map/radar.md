# The radar raster, and every window that draws it

One pixel per tile is the cheapest picture of a world this engine makes, and
three different windows want it at three different scales. Today one of them —
the minimap — has a chunked, revisioned, LOD-capable cache built for it over
five phases, and the other — the facet map — reaches past that cache's whole
point and asks for the entire world at full resolution, every frame.

This document is the inventory of what exists, the measurement of what it
costs, and the design that makes one raster serve both. It does not replace
[`minimap_lod_plan.md`](minimap_lod_plan.md), which is still the contract the
cache is built to, or [`minimap_lod_handoff.md`](minimap_lod_handoff.md),
which is still the record of what landed. It is the layer those two never
reached: **nothing chooses an LOD.**

> **Status: R0–R8 are built, their loose ends are closed, and the soak R7 asks
> for has been run — by a harness, and it found two things.**
> A window picks its own level from its own pixels, both windows are one
> `RadarView` — one construction, handed to the draw — the queue and the byte
> budget are one implementation under both subsystems, the pyramid is swept
> until it exists, the CPU cache evicts, the facet map wears a plate with a
> close button, and every counter the build wrote is now readable in the
> development HUD. R8 then took the last unbounded thing out of the frame: the
> map is walked at one level, and every coarser product is reduced from it. The
> whole per-frame step is now one function — `radar::advance` — because the
> reading below had to be taken by something that is not a person at a screen,
> and a harness that spelled that order out for itself would have measured a
> radar step nothing plays.
> Sections 1–3 below are therefore **the record of what was
> wrong**, not a description of the code — read them for the reasoning, not for
> the current shape. Section 9 is now the record of the eight things the build
> left open and what closed each; **section 10 is what is open now**, and that
> is where the next session starts. `MAX_LOD` no longer exists (it is
> `max_lod(extent)`), and the line numbers in section 3 are pre-build.

---

## 1. The inventory: there are two LOD systems, and they are not the same one

| | **scene LOD** | **radar LOD** |
|---|---|---|
| lives in | [`render/src/lod.rs`](../../crates/client/render/src/lod.rs), [`composite.rs`](../../crates/client/render/src/composite.rs) | [`render/src/radar.rs`](../../crates/client/render/src/radar.rs), [`radar_pass.rs`](../../crates/client/render/src/radar_pass.rs) |
| what a product **is** | one 8×8 map block, drawn in the isometric projection: RGBA plus its deferred planes | one 64×64-tile square, one texel a tile, categorical `radarcol.mul` colour |
| levels | `BlockLod::{Lod0, Lod1, Lod2}`, where `Lod0` means *no composite at all* | `RadarLod(u8)`, a true quadtree, `0..=MAX_LOD` (4) |
| **who picks the level** | `LodThresholds::next` from the block's projected screen footprint, **with hysteresis**, held in `BlockLodSelector` | **nobody. `RadarLod::BASE` is hard-coded at all four call sites.** |
| identity | `CompositeKey { block, tier, revision }` | `RadarChunkKey { facet, lod, chunk, revision }` |
| producer queue | `CompositeWorkQueue { max_pending, builds_per_frame, BTreeMap<key, order> }` | `RadarWorkQueue { max_queued: 512, builds_per_turn: 8, BTreeSet pending + in_flight }` |
| GPU residency | `CompositeCache`, LRU over a 128 MiB byte budget | `RadarChunkRenderer`, a texture array of 1024 pages × 16 KiB = 16 MiB, LRU |
| CPU residency | — (a composite lives on the GPU) | `RadarCache`, a `BTreeMap`, **unbounded, no eviction, `evicted` is always 0** |
| where a build runs | `composite_producer::produce`, GPU encoder work | `build_base_chunk`, pure CPU, **synchronously inside `App::draw_from`** |

**The products are different in kind and must stay apart.** A composite is a
picture of the world as the camera sees it — heights, art, projection. A radar
texel is a *category*, which is why `reduce_lod_pixel` votes instead of
averaging. Merging them would be the mistake `radar_pass.rs`'s own module doc
already refuses one level down, when it explains why a generated raster is not
a `GumpArt`.

**The machinery around them is the same thing written twice.** Five pieces,
each implemented independently in both columns:

1. a revisioned chunk key whose revision cannot be omitted;
2. a coalescing producer queue with a total bound and a per-turn bound;
3. publish-only-complete, so a partial product is never a cache value;
4. LRU GPU residency against a byte budget, evicting only recreatable data;
5. select-ready-with-fallback, so a miss is never a hole.

That is the extraction this repo would benefit from, and it is *not* what makes
the facet map wrong — so it is section 6, not section 4.

### The third and fourth readers, named in docs and not built

- **`client.md`'s M3b facet map** — the multi-session overview, one marker per
  logged-in body. It is described there as an egui image, which is a third
  placement of the same raster.
- **`radar::bake` and `radar::mark`** — the whole-facet CPU path. No caller but
  their own tests, the same shape the retired `RadarRenderer` had before it was
  removed. See the handoff's "next work" item 5.

---

## 2. The numbers, for the shipped Britannia facet

`map0` post-ML is **7168 × 4096** tiles. `BASE_CHUNK_TILES` is 64, so the level-
zero grid is exactly **112 × 64 = 7168 chunks**. A CPU chunk is
64·64·`Color16` = **8 KiB**; a GPU page is RGBA8 = **16 KiB**.

| level | tiles a texel covers | chunks in the facet | CPU bytes for the whole level |
|---|---|---|---|
| 0 | 1 | 7168 | 57 MiB |
| 1 | 2 | 1792 | 14 MiB |
| 2 | 4 | 448 | 3.6 MiB |
| 3 | 8 | 112 | 0.9 MiB |
| 4 | 16 | 28 | 224 KiB |
| 5 | 32 | 8 | 64 KiB |
| 6 | 64 | 2 | 16 KiB |
| 7 | 128 | 1 | 8 KiB |

Two consequences fall straight out of that table.

- **Levels 2 and coarser, for the entire world, cost 4.8 MiB and 599 chunks.**
  The whole facet, complete, at every scale the map window can usefully show,
  is smaller than the GPU page cache already is.
- **Level 0 for the entire world cannot be held and must never be asked for.**
  57 MiB of CPU products, 112 MiB of GPU pages, against a 16 MiB page cache.

The facet map's zoom range is `1.25^steps`, `steps ∈ -8..=12`, over a fit-to-
window base scale of ~0.089 screen pixels per tile for a 640×458 canvas. So it
spans **0.015 px/tile at full zoom-out** (the facet drawn 107 px wide, wanting
level 6) **to 1.3 px/tile at full zoom-in** (wanting level 0, but only for the
~500×350 tiles actually on screen — 48 base chunks). The minimap's own
`zoom_steps ∈ -6..=12` reaches 0.26, i.e. ~4× the tiles per axis, 16× the
chunks — still, today, all at level 0.

---

## 3. What is wrong, ranked

### 3.1 Nothing selects an LOD. This is the root cause of everything below it.

`RadarLod::BASE` is hard-coded in all four places that name a level: the
requester in [`presentation.rs:2113`](../../crates/client/app/src/presentation.rs#L2113),
the minimap draw in [`render_passes.rs:475`](../../crates/client/app/src/render_passes.rs#L475),
the facet-map draw in [`render_passes.rs:641`](../../crates/client/app/src/render_passes.rs#L641),
and `region_base_chunks`, whose own doc says level zero "is the only chunk grid
a rectangle of world tiles maps onto directly".

Coarse levels exist, are correct, are tested, and are *only ever produced as a
side effect*: `build_ready_ancestors` reduces a parent when the fourth child of
its family lands. Nothing ever schedules one. So a level-4 picture of the facet
requires all 7168 level-zero chunks to have been built first — which is the one
thing that cannot happen.

**The three defects section 3 of the handoff documents — the hemisphere wedge,
the page-cache truncation, the corner ring — are all the same defect seen from
three floors of the building.** A window zooms out; its tile demand grows with
the square of the zoom; the level stays at 0; a bound is exceeded; something
gets dropped, and the shape of the dropping is the symptom. Every one of those
fixes made the *shortfall* radial instead of directional. None of them removed
the shortfall, because the shortfall is asking for a level the window cannot
display anyway.

**The invariant this design exists to establish:** *a window's chunk demand is
a function of its own pixel area, never of the world's size or its own zoom.*
At the correct level a 640×480 canvas needs ~80 chunks and a 200×200 minimap
~16, at every zoom either of them has.

### 3.2 The facet map asks for the whole world at level zero, twice per frame

[`presentation.rs:2080`](../../crates/client/app/src/presentation.rs#L2080)
builds a `RadarRegion` spanning the entire facet whenever the world map is
open, and then walks `region_base_chunks_near` over it — 7168 coordinates
collected into a `Vec` and sorted, **every frame**. Then
[`render_passes.rs:641`](../../crates/client/app/src/render_passes.rs#L641)
walks `region_base_chunks` over the same 7168 and calls `select_ready` on each.

At 8 builds a frame it fills the 512-key queue's worth in ~15 s and then keeps
going, publishing into an unbounded CPU cache, while the 1024-page GPU cache
can only ever show 14% of the level-zero facet. What a person sees is a disc of
terrain around the window's centre and black everywhere else, permanently.

### 3.3 The facet map replaces the minimap's region instead of adding its own

The `if world_map_open` at
[`presentation.rs:2080`](../../crates/client/app/src/presentation.rs#L2080) is
an `if/else`: while the facet map is open, **the minimap requests nothing of its
own**. It draws whatever the facet map's demand happened to build. Two open
windows are two demands; the producer must serve a list, not a branch.

### 3.4 `select_ready`'s ancestor search is a linear scan of the whole cache

[`radar.rs:650`](../../crates/client/render/src/radar.rs#L650) filters
`self.ready.iter()` to find a coarser ancestor, and the stale-exact fallback
below it scans again. `ready` is a `BTreeMap` keyed
`(facet, lod, chunk, revision)` — in that order — so an ancestor is at most
`MAX_LOD` direct `get`s and the stale fallback is one bounded range query.

Cost today with the facet map open: 7168 calls a frame, each scanning every
retained product. This is a straight defect fix, independent of the design.

### 3.5 The GPU page cache is shared by both windows and thrashes between them

`Screen::radar_chunks` is one `RadarChunkRenderer`. Two windows whose combined
working sets exceed 1024 pages evict each other's pages *within one frame*, and
then again next frame, forever. `render_region` also `eprintln!`s on every
over-capacity frame ([`radar_pass.rs:911`](../../crates/client/render/src/radar_pass.rs#L911)),
so an open facet map spams stderr at frame rate. It also allocates a fresh
instance buffer per window per frame
([`radar_pass.rs:965`](../../crates/client/render/src/radar_pass.rs#L965)).

With correct LOD selection the combined working set is ~100 pages and the
thrash disappears; the shared-cache hazard should still be named and measured
rather than assumed gone.

### 3.6 The CPU cache never evicts anything

`RadarCache::evicted` is a counter nothing increments. `ready` grows for the
life of the process — walking a character across a facet publishes chunks that
are never reclaimed. There is no byte budget, no LRU, no pinning rule.

### 3.7 `MAX_LOD` is a constant where it should be a property of the facet

`MAX_LOD = 4` covers 1024 tiles across, which its own doc justifies against the
*minimap's* needs. The facet map at full zoom-out needs level 6, and the level
at which the whole facet is a single chunk is 7. A larger facet needs a deeper
ladder, which the constant cannot express.

### 3.8 The facet map's window is not a window

- **No art at all.** `WorldMapPane::art` returns `Vec::new()` and
  [`windows.rs:333`](../../crates/client/app/src/windows.rs#L333) answers
  `Drawn::WorldMap(_) => &[]`. There is no frame, no title bar, no close
  button; `TITLE_HEIGHT: 22` reserves an invisible drag strip.
- **The canvas drag is broken.** `Input::Move` is deliberately *unbounded* —
  [`route.rs:180`](../../crates/client/app/src/panes/route.rs#L180) marks only
  `Press` and `Wheel` as located, so every pane sees every move. The pane's
  `Move` arm at
  [`world_map.rs:77`](../../crates/client/app/src/panes/world_map.rs#L77)
  does `self.drag_from.replace(ctx.frame.cursor)` **before** testing whether a
  drag was in progress. After the first mouse movement anywhere on screen,
  `drag_from` is `Some` forever and the map pans with every pointer motion,
  with no button held and the pointer nowhere near the window.
- **Pan is unclamped and in the wrong unit.** `pan: (i32, i32)` accumulates
  gump pixels with `saturating_add`, so the map can be dragged entirely out of
  its own frame and never comes back. Storing a *world centre* instead makes
  the same drag mean the same ground at every zoom, and makes clamping a
  statement about the facet rather than about a pixel offset.

### 3.9 What is *not* wrong, contrary to the premise

The minimap does **not** rewrite a texture per step: it uploads immutable
chunk pages once and a step changes only uniforms and one overlay instance. It
**is** drawn in a gump — `SMALL_FRAME` (5010) / `LARGE_FRAME` (5011), circular
clip, 45° rotation, rim art layered over the generated terrain. It **is**
dragged, raised, hit-tested and closed by `Windows` like any other window. Its
one real defect is 3.1, which it shares with everything else.

---

## 4. The design

### 4.1 One view, two windows

Introduce the type both windows are already an instance of:

```
RadarView {
    facet, 
    centre:   RadarTile,     // what the window is looking at
    tiles_per_pixel: f32,    // the whole of "zoom", in one honest unit
    placement: Placement,    // where it lands, its rotation, its clip shape
}
```

with two derived answers — `region()` (the world rectangle it needs) and
`lod()` (the level that rectangle should be fetched at). The minimap
constructs one with `centre = player`, `rotation = π/4`, `circle = true`; the
facet map with `centre = its own panned centre`, `rotation = 0`,
`circle = false`. `radar_native_extent`'s hard-won arithmetic — no `sqrt(2)`,
`ceil` not `round`, a tangent margin that scales with the window and with zoom
but not with HiDPI — becomes `region()`'s body, unchanged and still the one
copy.

This deletes the two near-duplicate ~100-line arms in `render_passes.rs` and
the `if world_map_open` fork in `presentation.rs`. **Demand becomes a list of
open views**, walked in turn, which is what fixes 3.3.

### 4.2 The level is chosen from tiles-per-pixel, with hysteresis

`lod = clamp(floor(log2(tiles_per_pixel)), 0, max_lod(facet))`, and the
threshold gets the same hysteresis `lod.rs` already implements for the scene —
a view sitting on a boundary must not rebuild its whole working set every
frame the wheel jitters. `lod.rs`'s `LodThresholds`/`BlockLodSelector` shape is
the model; whether it is literally reused is section 6's question.

`max_lod` becomes a function of the facet's own extent: the level at which the
facet is one chunk, `ceil(log2(max(w, h) / BASE_CHUNK_TILES))` — 7 for
Britannia.

### 4.3 A chunk can be built at any level, directly from the map

`build_base_chunk` becomes `build_chunk(map, colors, key)` for any LOD: raster
the chunk's full tile span into a scratch buffer with the existing block-major
`fill`, then reduce it `lod` times with the existing `reduce_lod_pixel`. The
reduction rule is unchanged, so a chunk built directly is **bit-identical** to
one reduced from four children — which is a test, not a hope.

`build_lod_parent` stays as the cheap path for when a family happens to be
complete. `build_ready_ancestors` stays as the opportunistic climb.

Cost is `4^lod · 4096` tile colours. A level-2 chunk is 256×256 tiles ≈ 1024
blocks ≈ a few milliseconds; a level-4 chunk is 16× that again.

> **R8 keeps this as the product rule and refuses it as a scheduling rule.**
> `build_chunk` still builds any level, and that is what makes the two paths one
> product. What no longer happens is a producer being *handed* such a key: the
> "16× again" above is 232 ms and 192 MiB by level seven, in one frame. The
> climb was never the opportunistic half — it is the only half.

### 4.4 The producer's budget is time, not a count

`builds_per_turn: 8` is a count, and it was right while every build cost the
same. Once a build's cost is `4^lod`, a count is a budget that varies by 256×
between levels. Replace it with a **cost budget in base-chunk units**, spent
per frame: a level-2 chunk costs 16 units, a level-4 chunk 256, and the frame
spends, say, 32.

### 4.5 The coarse pyramid is swept once and then simply exists

The whole facet at levels 2 and coarser is 599 chunks and 4.8 MiB (section 2).
Its total build cost is one walk of the facet — 29.4 M tile colours,
~1 s of CPU — and under 4.4's budget it is a **few seconds of wall clock,
spread over frames, once per session**, with no thread and no disk artifact.

> **That cost was an arithmetic claim, and the build made it false.** Asking for
> every level separately is one walk *per level*: 1.01 s measured, of which
> 232 ms is a single key. R8 makes the sentence above true by construction — the
> walk is level two alone, 126 ms, and the 151 products above it are reduced from
> it for another 113 ms of arithmetic. Its "coarsest level first" micro-decision
> below is retired with it: a parent cannot precede its children.

So: when a facet map opens (or, cheaper for the player, eagerly at idle), the
requester enqueues the facet's level-2-and-coarser chunks at low priority.
Once they land, *every* zoom of the facet map has a complete picture, and the
minimap gains a real fallback ladder for ground it has never visited — which
the handoff's "known limits" names as the thing today's design cannot give it.

Levels 0 and 1 stay strictly demand-driven around each open view, nearest-
first, exactly as today. They are the only levels a byte budget has to bound.

**Persisting the swept pyramid to disk, keyed by the source revision, is
deliberately out of scope here.** It is an optimisation of a cost the player
pays once per session, and it belongs with the other bakes in
[`new_map_representation/plan.md`](new_map_representation/plan.md)'s section D
when derived data gets its revision key. Naming it now so it is not
re-invented later.

### 4.6 Budgets, and what may be evicted

- **CPU (`RadarCache`)**: a byte budget with LRU eviction, which
  `RadarCacheCounters::evicted` finally reports. Two pinning rules: a chunk at
  or above the sweep level is never evicted (it is 4.8 MiB and it is the
  fallback floor), and a chunk in any open view's current draw list is never
  evicted.
- **GPU (`RadarChunkRenderer`)**: unchanged 16 MiB / 1024 pages, but now
  reached only by a pathological view. The over-capacity `eprintln!` becomes a
  counter in the frame report; `cap_draws_by_distance` stays as the backstop it
  was written to be.
- The per-frame instance-buffer allocation becomes a reused, grown-on-demand
  buffer.

### 4.7 `select_ready` becomes O(levels)

Direct `get` per ancestor level, bounded range query for the stale fallback.
Independent of everything else and worth doing first.

### 4.8 The facet map becomes an actual window

- A frame, a title strip that is visible, and a close affordance — the same
  furniture every other local window has.
- The drag defect: guard the `Move` arm on `drag_from.is_some()`, and start
  the drag only on a press that is `under_pointer`. Add the interaction test
  the pane does not have.
- State becomes `centre: RadarTile` + `tiles_per_pixel`, clamped so the facet
  cannot be dragged out of its own frame.
- Its markers are the player today and, per M3b, one per logged-in body later —
  the same overlay, already built.

---

## 5. What the design does *not* change

- The colour rule (`tile_color`, highest static, `>=` against the land,
  `UNKNOWN` for absent) — untouched.
- `reduce_lod_pixel`'s categorical vote — untouched, and now load-bearing in
  one more place.
- The key's identity and the publish-only-complete contract — untouched; this
  is `minimap_lod_plan.md`'s first non-negotiable and it holds.
- Markers as overlays that never touch cached terrain pixels — untouched.
- `nearest` sampling, no blending, `UNKNOWN` backdrop under everything —
  untouched.

---

## 6. Decisions taken

Three questions this document opened with, answered before the plan below was
written, so nothing in it is provisional.

1. **The shared machinery is extracted now, under both subsystems.** Section
   1's five pieces are duplicated between the radar and the scene composites,
   and the extraction happens as its own phase (R1) rather than as a later
   track. What is generalised and what is deliberately *not* is pinned in R1
   itself — the boundary is the whole point of the phase.
2. **The coarse pyramid is swept lazily, on the first facet-map open.** A
   player who never opens the map pays nothing. The first open fills in over a
   few seconds, coarsest level first, over the `UNKNOWN` backdrop that already
   exists. Persisting it to disk stays out of scope, for
   [`new_map_representation/plan.md`](new_map_representation/plan.md)'s section
   D to pick up when derived data gets its revision key.
3. **The facet map wears the stretched plate.** `gump::resize` already draws a
   nine-slice `resizepic` from `0x0A28`, and the party manifest already uses it
   at 450×480 with the `0x00F3`/`0x00F2` close button. No new window-manager
   feature; in particular **no resizable windows** — that is a manager
   capability nothing else here has, and inventing it for one window is the
   half-measure this repo keeps refusing.

---

## 7. The plan

Seven phases. R0 is independent of every other and can land first; R1 through
R6 are ordered by what each one's successor needs. Each names the micro-
decisions it settles, so a later session does not get to re-open them.

### R0 — the four fixes with no design in them ✅

Independent of the rest, safe to land alone, and each one repairs something
measurable today.

1. **`select_ready` becomes O(levels).** `ready` is a `BTreeMap` keyed
   `(facet, lod, chunk, revision)` in that order, so an ancestor is at most
   `max_lod` direct `get`s and the stale-exact fallback is one bounded range
   query over `(facet, lod, chunk, 0..=current)`.
2. **The facet map's canvas drag.** `Input::Move` is unbounded *by design*
   ([`route.rs:180`](../../crates/client/app/src/panes/route.rs#L180)) — the
   pane is what must be guarded, not the router. Test the `drag_from.is_some()`
   guard before touching it, and start a drag only on a press that is
   `under_pointer`.
3. **The over-capacity `eprintln!`** at
   [`radar_pass.rs:911`](../../crates/client/render/src/radar_pass.rs#L911)
   fires every frame an open facet map is over budget. It becomes a counter on
   the renderer, read by R7.
4. **The per-frame instance buffer** at
   [`radar_pass.rs:965`](../../crates/client/render/src/radar_pass.rs#L965)
   becomes a reused, grown-on-demand buffer, in both `RadarChunkRenderer` and
   `RadarOverlayRenderer`.

**Micro-decisions.** The old exhaustive-scan `select_ready` is kept as a
`#[cfg(test)]` reference implementation, and the new one is tested against it
over a generated cache — that is the oracle, rather than asserting the three
`RadarReadyKind`s by hand a fourth time.

**Done when** a generated cache of a thousand mixed-revision, mixed-level
products gives byte-identical answers from the fast path and the reference for
every key in a covering sample; a pointer moved across the screen with no
button held leaves `WorldMapPane::pan` untouched, as an
`openshard-client-app` test; and no radar draw allocates a buffer per frame.

### R1 — the shared chunk machinery ✅

A new module in `client/render` — `chunk_cache` — holding exactly the two
pieces that are genuinely the same code twice, and nothing else.

**`WorkQueue<K: Copy + Ord>`** — bounded, coalescing producer scheduling.
`request`, `reconcile(is_still_wanted)`, `take_for_producer(order)`, `finish`,
`abandon`, `pending_len`, `in_flight_len`, counters. `RadarWorkQueue` and
`CompositeWorkQueue` become thin newtypes over it, each keeping its own key
type and its own `reconcile` predicate.

**`LruBudget<K: Copy + Ord>`** — a use clock, byte accounting, a protected set
and `evict_to_budget` returning a report. It owns the *decision*, never the
storage: `CompositeCache`, `RadarChunkRenderer` and (in R5) `RadarCache` each
keep their own entries and ask it what to drop.

**Deliberately not generalised, and this is the phase's real content:**

- **The product.** A `CompositeTexture` is GPU-side with eight planes; a
  `RadarChunk` is a CPU `Vec<Color16>`. One trait over both would exist only to
  be downcast.
- **The fallback ladder.** The radar's is *coarser ancestor, then stale-exact*;
  the composite's is *more detailed, then draw LOD 0 geometry instead*. These
  are opposite directions with opposite meanings.
- **The quarantine.** `CompositeQuarantine` is the scene renderer's answer to a
  known-bad composite. The radar has no such failure mode.
- **The builders.** `build_chunk` walks a `WorldMap`;
  `composite_producer::produce` records an encoder. Nothing shared.

**Micro-decisions.** No `Box<dyn>` and no trait objects on any path a frame
touches — `WorkQueue` and `LruBudget` are generic over the key only, and the
callers keep monomorphic wrappers. The counters keep their existing field
names, so the existing tests of both subsystems do not change their
expectations.

**Done when** both queues and all byte-budget eviction is one implementation;
`cargo test -p openshard-client-render` and `-p openshard-client-app` pass with
no test's *expectations* edited, only its imports.

### R2 — the ladder becomes something you can ask for ✅

1. **`max_lod` is a property of the facet, not a constant.** `MAX_LOD = 4`
   becomes `max_lod(extent) = ceil(log2(max(w, h) / BASE_CHUNK_TILES))` — 7 for
   Britannia's 7168×4096, the level at which the facet is a single chunk.
2. **`build_chunk(map, colors, key)` builds any level directly.** Raster the
   chunk's full tile span with the existing block-major `fill` into a scratch
   buffer, then reduce `lod` times with the existing `reduce_lod_pixel`.
   `build_base_chunk` becomes its `lod == 0` case.
3. **`region_chunks(region, lod)` and `region_chunks_near(region, lod, centre)`**
   join `region_base_chunks`, which stays for the invalidation path and its
   tests.
4. **The producer's budget becomes a cost, not a count.** A level-`n` chunk
   costs `4^n` base-chunk units; the frame spends a fixed number of units.
   `builds_per_turn: 8` becomes `units_per_turn: 8`, which is the same thing at
   level zero and stops being a 256× lie at level four.

**Micro-decisions.** The scratch buffer is allocated once per producer turn and
reused across the turn's chunks, sized for the largest level the turn will
build. A level-`n` build over `4^n · 4096` tiles is a single `fill` call over a
`2^n · 64`-square region, so the block-major walk is unchanged and
`WorldMap::statics_in_block`'s one-fetch-per-block property still holds.

**Done when** a test asserts `build_chunk(key at lod n)` is **bit-identical**
to `build_lod_parent` climbed from its `4^n` level-zero children, for `n` up to
3 on a fixture map — this is what makes the two production paths one product
rather than two pictures that resemble each other. And `max_lod` for
7168×4096 is 7, with a test.

### R3 — one view, two windows ✅ (with one seam left, see 9.1)

The type both windows already are an instance of:

```
RadarView { facet, centre: RadarTile, tiles_per_pixel: f32, placement: Placement }
```

with `region()` — `radar_native_extent`'s arithmetic moved in whole, no
`sqrt(2)`, `ceil` not `round`, the tangent margin unchanged — and `lod()`:

```
lod = clamp(floor(log2(tiles_per_pixel)), 0, max_lod(facet))
```

with a **10% dead band** on each boundary. Ten percent because one wheel notch
is 25%: a dead band under a notch means a notch always moves the level, and
jitter from a window resize or a device-scale change never does.

1. The minimap constructs one with `centre = player`, `rotation = π/4`,
   `circle = true`; the facet map with its own centre, `rotation = 0`,
   `circle = false`.
2. **`presentation.rs`'s `if world_map_open` becomes a list.** The requester
   walks every open view in turn, nearest-first within each — which is what
   ends the facet map starving the minimap (defect 3.3).
3. **`render_passes.rs`'s two ~100-line arms become one**, taking a
   `RadarView`.

**Micro-decisions.** `tiles_per_pixel` is the single honest unit and replaces
every separate `magnify` / `device_scale` / `zoom` factor at the seam — the
three keep their own meanings on the way in and are multiplied exactly once,
in the constructor. The tangent margin keeps its existing rule of scaling with
the window and with zoom but *not* with HiDPI or desk scale; the handoff
records three measured reports for why, and none of them is re-opened here.

**Done when** an offscreen test shows the minimap at `zoom = 1` drawing
pixel-for-pixel what it draws today; a test shows an open facet map changing
not one of the minimap's requested keys; and a test shows a view over a region
a hundred times larger requesting **the same number of chunks** — which is the
invariant this whole document exists for.

### R4 — the sweep ✅ (with one hazard left, see 9.3)

On the **first** open of a facet map, enqueue that facet's chunks at
`SWEEP_LOD` and coarser, at a priority below every open view's own demand.

**`SWEEP_LOD = 2`.** Levels 2 and coarser are 599 chunks and 4.8 MiB — smaller
than the GPU page cache already is, and covering the facet map from fit-zoom
down to about four tiles a pixel. Level 1 for the whole facet is 14 MiB and no
window ever wants the whole facet at that scale; levels 0 and 1 stay strictly
demand-driven around each open view.

**Micro-decisions.** Coarsest level first, so the map has a complete (blocky)
picture within a fraction of a second and sharpens; the fallback ladder already
paints a coarse stand-in under a finer product, so this needs no new drawing
rule. Enqueued once per facet per session, guarded by a flag on the cache, not
by "is the queue empty" — an emptied queue is not evidence the sweep ran.

**Done when** opening the facet map and waiting leaves every level ≥ 2 complete
for the facet, at 4.8 MiB; closing and reopening builds nothing; and the
minimap, walked onto ground it has never visited, now falls back to a coarse
ancestor instead of the backdrop — which is the "known limit" the handoff
records as unfixable under today's design.

### R5 — the CPU cache gets a budget ✅

`RadarCache` takes an `LruBudget` from R1 and finally increments
`RadarCacheCounters::evicted`. Two pinning rules:

- a chunk at `SWEEP_LOD` or coarser is **never** evicted — it is 4.8 MiB and it
  is the fallback floor;
- a chunk in any open view's current draw list is never evicted.

**Micro-decisions.** The budget is bytes, not entries, and it counts only the
unpinned tail — the same shape `CompositeCacheLimits` already states for the
scene, so the two read alike. Superseded-revision products are the first
candidates, ahead of any current-revision one, whatever the use clock says.

**Done when** a scripted walk across a facet leaves the unpinned tail at or
under its budget with `evicted` non-zero, and every level ≥ 2 still resident.

### R6 — the facet map becomes a window ✅ (tests short of its own bar, see 9.7)

1. **The plate.** `gump::resize(atlas, Graphic(0x0A28), at, width, height)`,
   the same nine-slice the party manifest uses, with the `0x00F3`/`0x00F2`
   close button and a visible title. The content rectangle is inset by the
   plate's own measured corner sizes, falling back to a constant when the art
   is absent — the discipline `SMALL_EXTENT`/`LARGE_EXTENT` already set for the
   minimap.
2. **The state becomes world-space.** `pan: (i32, i32)` in gump pixels becomes
   `centre: RadarTile` plus `tiles_per_pixel`, clamped to the facet. The same
   drag then means the same ground at every zoom, and the clamp is a statement
   about the world rather than about a pixel offset.
3. **Zoom is about the pointer**, not about the window's centre — the standard
   map gesture, and the one that makes a clamped centre feel right.
4. **Markers.** The player today; per `client.md`'s M3b, one per logged-in body
   later. Same overlay, already built.

**Done when** the facet map drags, raises, closes and z-orders like every other
local window under `Windows`, with interaction tests for each; the canvas
cannot be dragged out of its own frame at any zoom; and `Drawn::WorldMap` no
longer answers `pictures()` with an empty slice.

### R7 — measure, and then soak ✅ (the soak is 10.1, and it found 10.5)

`RadarCacheCounters` and `RadarWorkCounters` existed and nobody read them; R0
added a third on the page cache. Putting them in the frame report **was a UI
addition and was asked for first** — it was asked, and the answer was the frame
report, which is `shell::radar_report` under the perf panel's map-composite
grid.

Everything this phase asked to be measurable now is. Chosen level per view,
with the `tiles_per_pixel` it was chosen from beside it, because the selection
has a 10% dead band and a level without its input cannot be told from a
selector that has stopped responding. Chunks requested and how each was
answered — `radar::resolve_demand` returns the `RadarReadyKind` tally along
with the keys the draw will use, one walk for both, because the three fallback
modes and the no-answer case all look like missing terrain on screen and mean
four different things. CPU raster milliseconds, timed around the producer loop
itself. GPU pages resident, evicted, and truncated — the three page numbers,
with the third said in words and in yellow when it is not zero. And the three
bounds are each reported beside what they bound: the CPU cache's retained bytes
against its tail budget, the queue's outstanding work against `max_queued`, the
resident pages against the array's capacity.

Two decisions inside that are worth keeping. There is deliberately **no
refusal counter** on the work queue: `request_sweep` being refused is ordinary
— `drain_sweep` offers every owed key again next frame precisely because it is
— so a refusal total would climb through a healthy session and read as an
alarm; the headroom is the honest form of that question. And the per-frame
sample is **a frame behind on purpose**: the HUD is assembled near the top of
`draw_from`, before the views are built and long before the producer runs, so
`App::radar_frame` carries the last frame's levels, tally and cost rather than
reporting a frame's worth of nothing. The three counter sets beside it are
live.

**What was still owed was the reading, and it is taken.** Not by a person in
front of the worst case — by `examples/radar_soak.rs`, which drives the frame's
own step with no device and can therefore be *at* every scale rather than at the
one the development machine happens to be. "Walking costs no raster work" is
nineteen nanoseconds a step; 4.6's "reached only by a pathological view" is
false one scale above this machine's. Both readings, and what they cost, are
10.1 and 10.5.

### R8 — the map is walked at one level ✅

R2 gave `build_chunk` the ability to raster **any** level directly out of the
map, and R4 gave the sweep **every** level from `SWEEP_LOD` to `max_lod` to ask
for. Each was right on its own. Together they meant the producer walked the
whole facet once per level, and that the coarsest of those walks was a *single
key* — which `take_for_producer_by_cost` cannot refuse, because a turn always
takes at least one job whatever it costs. §4.4's cost budget was a rate, never a
bound on one build.

`examples/radar_floor_cost.rs`, against the shipped Felucca install
(`7168×4096`, `samples=3`, release):

| level | chunks | tiles a chunk covers | scratch | one chunk | the level |
|---|---|---|---|---|---|
| 2 | 448 | 65,536 | 192 KiB | 282 µs | 126 ms |
| 3 | 112 | 262,144 | 768 KiB | 1.27 ms | 142 ms |
| 4 | 28 | 1,048,576 | 3 MiB | 5.03 ms | 141 ms |
| 5 | 8 | 4,194,304 | 12 MiB | 22.5 ms | 180 ms |
| 6 | 2 | 16,777,216 | 48 MiB | 94.7 ms | 189 ms |
| 7 | 1 | 67,108,864 | 192 MiB | **232 ms** | 232 ms |
| | | | | | **1.01 s** |

So the first open of a facet map put a **232 ms frame** and a 192 MiB transient
allocation in a player's way — one key, inside `App::draw_from` — and the floor
§4.5 costed at "one walk of the facet, ~1 s of CPU" was in fact seven and a half
walks. The same floor **reduced** from level two instead: **113 ms**, no chunk
above 282 µs, and every one of the 151 coarse products **bit-identical** to the
direct build it replaces.

1. **`SWEEP_LOD` is the ceiling as well as the floor.** One constant, two roles,
   because they are one statement: the level the map is walked at. Nothing
   coarser is ever built from terrain; it is reduced.
2. **The three doors that could name a coarser key each clamp.**
   `request_views` builds at `min(lod, SWEEP_LOD)`, `begin_sweep` owes one level
   (448 chunks, not 599), `invalidate_tile` marks dirty no higher than the
   ceiling.
3. **A parent may be reduced with an absent child, but only where the facet
   ends.** A level's chunk count is `ceil(extent / side)`, and an odd count means
   the level above asks for a child past the edge. Britannia goes odd at level
   **four** — seven chunks across — so a family-complete climb stops there:
   measured 6 of 8 chunks at level five, 1 of 2 at six, 0 of 1 at seven. An
   absent child off the facet is a quadrant of `UNKNOWN`, which is exactly what
   `build_chunk` rasters for those tiles; an absent child *inside* the facet is
   still a hole and still refused.
4. **`build_ready_ancestors` takes the facet's extent, not a level.** Both
   answers it needs come from it — how high the ladder goes, and which absent
   children are ground the facet does not have — and handing in a level beside
   an extent would be two spellings of one fact, free to disagree.
5. **The turn budget is sixty-four units, and only now is it a bound.** A
   coarse product lands *after* its children instead of before them, so §4.5's
   "coarsest level first, a blocky picture within a fraction of a second" is
   retired: the floor fills nearest each view's own centre and each completed
   family lights one tile of the level above. At eight units — one floor chunk
   a turn — that is 448 frames, seven seconds of a facet map with nothing in
   it, for 126 ms of actual work. Sixty-four is four floor chunks, 1.1 ms of
   map walk in the worst turn there is, and under two seconds to fill. It is a
   bound rather than a rate because no key costs more than sixteen units any
   more; the reading that moves it again is R7's `raster`.

**What this costs, said plainly.** R3's invariant — *a window's chunk demand is
a function of its own pixel area* — now holds for the keys a view **draws**. The
keys it asks to have **built** are its region's floor, which for a fully
zoomed-out facet map is that facet's whole floor. It is the same total map work
either way; what changes is that it is spent 282 µs at a time instead of 232 ms,
and that every level above the ceiling comes free with it.

**Done when** the sweep, driven through a queue eight slots wide, leaves every
level up to Britannia's seventh complete while no key above the ceiling ever
reaches a producer — one test, which is also the edge rule's oracle, since it
fails at level five without it. And `radar_floor_cost` reports every coarse
chunk of the shipped facet identical between the two paths, which is what makes
them one product rather than two pictures that resemble each other.

---

## 8. What this retires

When R3 lands, [`minimap_lod_handoff.md`](minimap_lod_handoff.md)'s three wedge
defects stop being live entries and become history: the hemisphere wedge, the
page-cache truncation and the corner ring are one defect — a window asking for
a level it cannot display — and their three fixes (`region_base_chunks_near`,
`cap_draws_by_distance`, the `sqrt(2)` removal) all stay, as the backstops they
were written to be. None of them is undone here. They simply stop being
reachable in ordinary play.

---

## 9. What the build left open, and what closed it

R0–R6 landed together, and everything in this section was found by reading that
work against this plan. Nothing here was a redesign — the design of sections 4
and 6 held — and all eight are now closed. Each entry keeps what was wrong,
because the reasoning is the part worth reading twice, and ends with what the
fix was.

### 9.1 The view was built twice, and only the level crossed between them ✅

This was the one item that mattered, because it was the defect this whole
document was written against, one layer up.

`App::draw_from` built a `RadarView` per open window to compute *demand* and to
drive that window's `RadarLodSelector`; `draw_gump_windows` then built **a
second `RadarView`** per window, from the same `Drawn` bounds, to compute the
*draw*. What crossed the seam was a `&[(WindowSubject, RadarLod)]` slice — the
level only.

The two constructions agreed by arithmetic coincidence, not by construction:
one read `window_scale.factor()` and `App::gump_scale()`, the other `magnify`
and `frame.scale`, and `gump_scale()` and `frame.scale` are separately-written
spellings of `shell.pixels_per_point()`. Let either drift and the requested
region stops being the drawn region — a chunk built and never shown, or shown
and never built, which is exactly [`parity.md`](../parity.md)'s hazard.

**Closed by widening the slice.** `draw_gump_windows` takes
`&[(WindowSubject, RadarView, RadarLod)]` and looks its window up; the second
construction is gone. The one that remains is in `App::draw_from`, where the
window's live position is read from `own_windows` — the half the draw arm had
that the demand arm did not.

Two properties of the surviving construction are worth stating, because both
are now load-bearing rather than incidental:

- It reads `windows.drawn_windows`, which is **a frame behind** by that list's
  own design (a window is not pickable until it has been drawn once). The
  radar's placement is therefore a frame behind too, and it is the *same* frame
  behind for the request and the draw, which is the property that matters.
- A window opened this frame has no view yet, so its first frame draws its rim
  with no terrain in it — the same one-frame rule, in the same list.

### 9.2 `radar_native_extent` and `radar_region_for` were `#[cfg(test)]` ✅

R3 said `radar_native_extent`'s hard-won arithmetic "becomes `region()`'s body,
unchanged and still the one copy". It was instead *copied* into
`RadarView::region` and `with_tangent_margin_fraction`, and the original pair in
[`minimap.rs`](../../crates/client/app/src/panes/minimap.rs) was marked
`#[cfg(test)]` so its tests would keep compiling. So there were two copies,
production read one, and the five tests recording the three measured margin
reports — flat count, no-`zoom` fraction, no HiDPI scaling — asserted about the
copy no frame calls.

**Closed by deleting the pair.** The five tests build the same `RadarView`
`App::draw_from` builds and assert on `region()`. The reasoning moved with
them: `TANGENT_MARGIN_FRACTION` carries why the margin exists and what it may
not scale with, `RadarView::region` carries the `sqrt(2)` refusal and the
corner-ring starvation it caused. `with_tangent_margin` — a second spelling of
the same 21%, with no callers at all — went with them.

### 9.3 The sweep could drop chunks and still call itself done ✅

`RadarCache::begin_sweep` flipped a per-facet flag the first time a facet map
opened, and the enqueue loop then called `request_sweep` once per chunk.
`request_sweep` returns `false` when the queue is at `max_queued`, and a refused
key was never offered again — the flag already said the sweep had run. It worked
by arithmetic rather than by construction (599 sweep chunks plus both windows'
demand under 1024), and its failure was silent: a hole in the fallback floor
that shows up as a patch of backdrop at some zoom, weeks later.

**Closed by making the cache owe the keys.** `begin_sweep(facet, extent)`
enumerates every level from `SWEEP_LOD` to the facet's own `max_lod` into an
owed set; `drain_sweep` offers what is left every frame the facet map is open
and strikes a key off when its product lands — or when a mutation has moved the
facet's revision past it, from which moment the chunk belongs to the dirty set
and not to the sweep. A refused request is simply one tried again next frame.
The test drives the whole floor through a queue of eight.

### 9.4 The demand loop could not be tested ✅

R3's "done when" asked for a test that an open facet map changes not one of the
minimap's requested keys — defect 3.3, the one that starved the minimap. It did
not exist, because the requester lived inside `App::draw_from`, which needs a
window, a device and a shell.

**Closed by lifting it out.** `radar::request_views` is the whole per-frame
requester as a function of `(views, &RadarCache, &mut RadarWorkQueue)`, and the
missing test is three lines: the facet map's demand is offered first, which is
the order that starves the minimap if anything at all is shared, and every key
the minimap asks for alone survives.

### 9.5 `RadarLodSelector` remembered a level across a facet change ✅

`update` clamped upward only while `selected < max`, and its downward loop
stopped at zero. A selector carrying a level chosen for Britannia kept it when
the view moved to a smaller facet, and could then return a level above that
facet's own `max_lod` — naming a grid that does not exist.

**Closed by clamping `selected` to `max` on entry**, with a test that moves one
selector from Britannia to a two-chunk facet.

### 9.6 Two small hardening items in the same files ✅

- **`take_for_producer`/`take_for_producer_near` indexed `priorities[key]`
  inside the sort comparator.** Every path that makes a key pending also
  inserts its priority, so it was an invariant rather than a bug — but a panic
  sited inside an `Ord` comparator, in a frame, is a bad place to spend one.
  Both now read `.get().copied().unwrap_or(View)`.
- **`RadarCache::evict_to_budget` rebuilt its pinned set from every ready key,
  every frame**, and `LruBudget::retained_bytes` summed every entry each call.
  Nothing pinned can *lower* the ceiling, so a cache inside its tail budget
  cannot evict: that question is asked first and answered in `O(1)`, because
  `LruBudget` now carries its retained total instead of summing to find it.

### 9.7 What was built but not tested to its own bar ✅

- **R6's window furniture.** There are now tests for the drag actually panning
  (a press takes the drag and raises; the move walks the centre by the
  pointer's own distance in that zoom's tiles) and for "the canvas cannot be
  dragged out of its own frame at any zoom" — the phase's own headline claim,
  at every one of the twenty-one zoom steps, pulled into both corners, asserted
  against the visible rectangle. Removing `clamp_centre` fails it.
- **R0's per-frame allocation.** `RadarChunkRenderer::instance_capacity` is
  public, and a GPU test draws the same two chunks three times: the capacity
  moves once and then not at all.
- **`RadarPageCounters::over_capacity_draws`.** Still written and never read —
  see 10.1, which is where it regains a voice.

### 9.8 One behaviour change nothing recorded ✅

`RadarView::region()` clamps the fetched rectangle's origin against **both**
facet edges; `radar_region_for` saturated only at zero. Standing at the map's
east or south edge, the minimap therefore shifts its region back inside the
facet — terrain where there used to be a band of `UNKNOWN`, and a player marker
that walks off the centre of the circle instead. It is the better behaviour and
it matches the west/north edge, which always did this.

**Closed by asserting it**, in the same test as the west/north case, with the
reason it is deliberate written beside it.

---

## 10. What is open now

### 10.1 The soak R7 asks for — taken, by a harness ✅

R7 built the instrument and nothing read it, and the reason it stayed unread is
worth keeping: the HUD shows its numbers on **one** machine at **one** scale,
and the worst case R7 names — HiDPI, desk-scaled, fully zoomed out — is a scale
most machines are not. But nothing in that worst case needs a window. The level
a view picks, the rectangle it fetches, the chunks it draws and what the
producer spends walking the map are all functions of the map, the colour table
and four numbers a person could have typed into a settings panel.

So the reading is [`examples/radar_soak.rs`](../../crates/client/render/examples/radar_soak.rs),
against the shipped Felucca install, release. It drives `radar::advance` — the
same call `App::draw_from` makes, and deliberately the same one: a harness with
its own copy of *ask, sweep, build, resolve, evict* would be measuring a radar
step nothing plays, which is 9.1's mistake arriving through the diagnostic
instead of through the picture. What it cannot answer is GPU residency and
eviction, which are a real device's; what it measures instead is
`over_capacity_draws`' own **predicate** — how many chunks a view hands
`render_region`, against how many pages exist — which is the half that needs no
device.

**The floor fills in.** Both windows open at their widest zoom, an empty cache,
two physical pixels to a gump pixel (an ordinary HiDPI screen at no desk scale):
the facet map picks level 2 and draws 448 chunks, the minimap level 1 and 144.
The floor is complete in **122 frames and 592 chunks**, and no view is missing
terrain from frame 111 on. The fallback ladder is visible doing its job on the
way: `coarser` climbs to 144 as the minimap's level-1 chunks are stood in for by
their level-2 ancestors, and falls back to zero as the exact products land. The
CPU cache settles at **5.8 MiB of its 32 MiB tail** and evicts nothing; the
queue peaks at 588 of 1024.

**Walking costs no raster work — twenty nanoseconds a step.** A minimap alone at
zoom 0, a player stepping one tile a frame for 256 tiles: **4 frames of the 256
did any raster work at all**, one per chunk edge crossed. The other 252 cost
about twenty nanoseconds each, which is the producer being handed nothing. The
argument this document has carried since R7 is now a number.

**What in the above is a wall clock, and what is not.** Every count here — 122
frames, 592 chunks, 448 and 144 pages, 5.8 MiB, 4 building frames of 256 — is
exact and reproduces run to run, because they are properties of the map and the
bounds. The milliseconds are not: the floor's raster total moved between 154 ms
and 194 ms across two runs on the same machine, and its worst frame between
1.65 ms and 3.35 ms, with parallel work on the other cores. Read them as *a
frame is a millisecond or two, not a hundred* — which is the claim R8 made and
the one worth checking — and take a fresh reading before quoting a number to
three digits. `coarse_bench`'s discipline of printing how a reading was taken is
the shape this should grow towards.

**The two bounds a run without a device can read** are both fine at 1×: the
worst zoom of either window asks for 344 pages of 1024, and the CPU tail never
comes near its budget at any scale — 18% at the reading above. The third is
10.5.

**What the harness restates, and the drift that buys.** The scenario is the two
panes' own numbers — the plate's fallback inset, the minimap's 15% rim, both
zoom ranges, the 21% margin — copied into the example, because
`openshard-client-render` cannot depend on `openshard-client-app` and must not:
the dependency runs the other way. The *mechanism* is shared and the *scenario*
is not, which is the right side of that trade — but a pane that changes its
extent or its zoom range leaves this reading describing a client nobody runs,
silently. Every constant names where it came from, and that is the whole of the
defence.

### 10.2 The coarse floor is swept once, and a terrain edit does not re-sweep

`drain_sweep` strikes a key off when the facet's revision moves past it, on the
grounds that the chunk is then the dirty set's to rebuild. That is correct for a
chunk an `invalidate_tile` actually named, and it is *not* the same claim as
"the floor is current": a revision bump that reaches the whole facet would leave
coarse products that only `select_ready`'s stale-fallback path can use. Nothing
in the shard moves a facet revision that way today. Naming it because the next
map-editing feature is what would.

**R8 sharpened this rather than causing it, and it is the one thing R8 gives up.**
An edit already left every coarse product stale, because a facet-wide revision
moves all of them at once and a parent needs four children at the *new* revision
while an edit rebuilds one. What `invalidate_tile` used to do about that was
rebuild the edited tile's own ancestors directly from the map — a column of
current products in an otherwise stale ladder, bought with the frame R8 exists to
give back (232 ms of it, at the top). It now marks dirty no higher than the
ceiling. So the honest statement of the gap is: **after a terrain edit the coarse
ladder is stale until something re-sweeps the floor, and nothing does.** The shard
that gives this path a production writer owes the ladder a revision model — a
chunk whose *content* did not change should keep its identity across a facet's
revision — and not a bigger frame budget. Era S's live publish is where that
lands.

### 10.3 One page eviction per insert was an invariant nothing states ✅

`RadarChunkRenderer::resident_layer` inserted one page, evicted to budget, and
took `eviction.keys.first()` as the layer to reuse. Every other evicted key
would be dropped from `residency` while staying in `self.pages` — a page the
budget believes is free and the map still hands out, which is the corruption
`cap_draws_by_distance` exists to prevent, arriving by the other door.

It could not happen: one insert of one fixed-size page puts the budget at most
one page over, so `keys` was never longer than one. That is an arithmetic
argument about `RADAR_CHUNK_PAGE_BYTES` being constant, made in a different
file from the loop that depended on it, and written down nowhere. Found while
adding the eviction counter, which is what made the plural in `keys.len()`
visible at all.

**Closed by draining the list**, which needed the other half stating too: a
fresh layer was `pages.len()`, and that is only the next unused layer while the
allocated ones are dense — evicting two and reusing one would leak the other and
then hand it out again. `free_layers` is the missing half — *every layer ever
allocated is either held by a page or waiting in it* — so `pages.len()` is
reached only when nothing is free. The test is a picture rather than a counter:
two pages churned through nine chunks and then asked to hold two at once, where
one layer holding both draws the same colour twice. Its control is the free list
removed by hand, and it fails on the west half being green.

### 10.4 The margin fraction is measured, and its measurement is a person looking

`TANGENT_MARGIN_FRACTION` is 21% because three reports said 20.7% left a visible
seam. The tests pin the *arithmetic* — that it scales with the window and with
`zoom` and with nothing else — and no test says the seam is gone, because
nothing here can see. It is a `silhouettes.md`-shaped question: first attribute
it with a debug view, then decide.

### 10.5 The shared page array is crossed at two physical pixels to a gump pixel

4.6 said the GPU page cache is now "reached only by a pathological view", and
the soak says that is true of a 1× surface and false of the next one up.
`radar_soak`'s third reading, in pages of the 1024 the array has:

| physical px per gump px | facet map, worst zoom | minimap, worst zoom | together |
|---|---|---|---|
| 1 | 280 (27%) | 64 (6%) | 344 (34%) |
| 2 | **988 (96%)** | 144 (14%) | **1132 (111%)** |
| 4 | 3744 (366%) | 400 (39%) | 4144 (405%) |

Two physical pixels to a gump pixel is a HiDPI laptop with the desk scale left
alone. Four is that laptop at a 2× desk scale — a 2368-pixel-wide facet map,
which is large but not absurd; eight is a window wider than any screen and is in
the table only to show the shape.

**The two failures are different and both are 3.5's.** One view above 1024 —
the facet map's 3744 — trips `over_capacity_draws` and `cap_draws_by_distance`
drops the surplus, which is terrain that is simply not drawn. Two views summing
above it — 1132 at the ordinary HiDPI scale — is the other half of the same
defect: one `RadarChunkRenderer` serves both windows, so each frame the minimap
evicts 144 of the facet map's pages and the facet map uploads them again, 2.3
MiB of page copies a frame, for as long as both windows are open.

**The worst zoom is not the widest, which is the part no one had reason to
expect.** Fitted, the facet map asks for 448 pages; five notches *in*, it asks
for 988. A zoom step shrinks the region by 1.25 and the level only steps by
powers of two, so the demand climbs through a level's whole band and then falls
off a cliff when the level changes — the peak sits just inside each boundary,
and the global peak is at the finest boundary the region is still large at.
Everything in section 3 reasons about *zooming out* as the direction that hurts;
the measurement says the middle of the range is.

### 10.6 The level rule always errs finer, and that is where the factor of four is

4.2 chose `lod = floor(log2(tiles_per_pixel))` and gave no reason for the
rounding. `floor` never magnifies: a texel is at most one tile per pixel and at
worst half of one, so the picture is sharp-to-aliased rather than
sharp-to-blocky. What it costs is exactly the 10.5 table. At two physical pixels
to a gump pixel and zoom step 5 the view wants 1.98 tiles a pixel: `floor` says
level 0 and 988 pages, `round` would say level 1 and 247 — the same window, a
quarter of the array, and one tile drawn across 1.01 pixels instead of 0.5.

This is a decision to take rather than a defect to fix, and it is the cheapest
of the three ways out of 10.5: `round` costs a fraction of a texel of sharpness,
a second `RadarChunkRenderer` per window costs 16 MiB and leaves the truncation
case untouched, and a larger array costs GPU memory the client has other uses
for. It is left open because it changes what a person sees, and this document
has spent nine sections learning not to decide those by arithmetic — see
`lighting_pitfalls.md` on the same discipline. First attribute it with both
levels drawn side by side, then decide.
