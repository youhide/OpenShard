# LOD1 rolled out, LOD2 held back

Where the map-block LOD work stood on 2026-08-13: what is safe, which code
carries it, and the order the remaining investigation was to be done in. The
model is [`../design_lod.md`](../design_lod.md); what is still owed is
[`plans/client/lod/PLAN.md`](../../../plans/client/lod/PLAN.md).

## Handoff: LOD1 rollout, LOD2 held back

**Current safety state (2026-08-13).** The independent producer, cache, queue,
LOD selection and telemetry code are live. `App::draw_from` enables LOD1 and
caps selected LOD2 at LOD1. The injected `--scenario lod-sweep` diagnostic
reaches `1/2x`, pans through block boundaries without desktop input, and can
be run with `OPENSHARD_COMPOSITE_AUDIT=1` to read each completed cache ID plane
before restore. LOD1 is spatially lossless: it preserves one cache texel per
producer texel, because a deferred G-buffer's colour, ID, position, normal and
depth must remain one atomic fact. LOD2 is disabled.

The prior failure was an invalid ownership contract: the renderer removed
static rows when the fixed-height source image could not contain a tall roof.
The real-frame oracle now exercises the producer/capture/restore path against
the full LOD0 scene. A cache miss, unprepared job or cutaway keeps ground at
LOD0 for that frame; map statics stay live in every case.

### Relevant code

- `crates/client/app/src/presentation.rs`: LOD1-only rollout cap, producer
  invocation and its fixed local camera.
- `crates/client/app/src/window.rs`: `prepare_composite_job`, which prepares
  only the ground/texmap inputs its producer owns.
- `crates/client/render/src/composite.rs`: fixed producer contract,
  conservative capture, deferred restore and synthetic GPU oracle.
- `crates/client/app/src/render_passes.rs`: the point at which a ready block
  excludes cacheable LOD0 ground, draws its deferred composite, then draws all
  live map statics.

### Required investigation and fix order

1. **Done: real-map producer oracle.** The gated
   `frame::real_map_block_producer_keeps_every_owned_map_tile_after_restore`
   test uses real map/land/static atlases and the actual producer camera. It
   reads source and restored colour, IDs and positions and asserts the cached
   ground pixels survive LOD1 capture/restore.
   It passed with the development client. Re-run it with
   `OPENSHARD_CLIENT=/path/to/client cargo test -p openshard-client-render --test frame real_map_block_producer_keeps_every_owned_map_tile_after_restore -- --exact`
   after changing the producer contract or its capture shaders.
2. **Done: prepare every producer source input.** `prepare_composite_job`
   grows only the owned 8×8 block's land/texmap graphics. Static atlas state is
   not an input to this cache, so packet-driven static-atlas growth cannot
   mutate or invalidate a completed ground composite.
3. **Done: share and test the transform.**
   `CompositeProducerJob::rect_in(camera)` replaces the separate
   `render_passes::block_rect()` calculation. The renderer test proves that
   producer/source and visible rectangles agree, and that east/south adjacent
   blocks retain their projected offsets through zoom and minification.
4. **Done: conservative replacement.** Only a ready deferred composite
   excludes LOD0 geometry. Any cache miss, input-preparation failure, cutaway,
   mutation or animated source keeps the detailed path; no cache allocation by
   itself can suppress it.

For the delayed, no-motion report use `--scenario zoom-soak`: it waits for the
real window viewport to settle, renders for three seconds at the opening zoom,
injects up to three zoom-out notches (subject to the GPU texture limit), and
then performs no pan. It logs an error if the camera changes after that point.
Run it with `OPENSHARD_ATLAS_AUDIT=1`,
`OPENSHARD_COMPOSITE_AUDIT=1`, `OPENSHARD_LOD_SCREEN_AUDIT=1` and
`OPENSHARD_LOD_FRAME_ORACLE=1`; those four independent checks cover atlas
bytes, cached producer bytes, visible ground coverage and the final displayed
pixels respectively.
On a live shard, its post-zoom report also lists every authoritative update by
packet kind. `--scenario zoom-soak-freeze-server` runs the same transition but
counts and deliberately does not fold those server updates after zoom; it is a
diagnostic A/B mode only, for separating packet-driven scene changes from a
cache or texture mutation.

When an intermittent roof artifact is visible in an ordinary session, press
the status-strip **capture GPU dump** button (or `F12`). It arms the next
ordinary world frame and writes its rendered world/G-buffer planes, instance
buffers, `inputs.txt` and `lod-oracle.txt` into
`OPENSHARD_FRAME_DUMP_DIR/frame-N` (default:
the system temporary directory's `openshard-frame/frame-N`). The log reports
the exact directory; archive that complete directory rather than taking a
screen shot. The same one-shot action also performs atlas CPU/GPU readbacks,
visible-tile coverage and a complete displayed-pixel comparison with fresh
LOD0; its log names any mismatched pixels and source blocks.
5. **In progress: LOD1 field run; LOD2 held back.** The direct `lod-sweep`
   now verifies the texture state rather than inferring it from a screenshot:
   cache entries retain their owner IDs while the next producer job redraws all
   source planes. LOD1 is enabled and lossless; only LOD2 may minify source
   texels. Continue the scenario over ordinary terrain,
   map edges, dense/tall statics and animated statics; the Frames HUD must show
   bounded queue/cache values, no atlas rebuild every frame, and no black,
   holes or shifted map pixels. `OPENSHARD_LOD_FRAME_ORACLE=1` additionally
   compares every displayed cached pixel to a fresh full-LOD0 render (including
   the lighting branch). The slow max-zoom pan now matches after slopes remain
   live. Enable LOD2 only after that run is clean.

Existing tests cover synthetic ownership boundaries, depth interaction,
producer sizing and bounded queue work; the gated real-map oracle covers
end-to-end LOD1 map-ground ownership. It is deliberately not a substitute for
the sustained interactive field run required before LOD2.
