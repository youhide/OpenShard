# The occluding primitive: box or mesh

> **Consolidated into [`design_model.md`](../../render/design_model.md)** — box-to-mesh occluders, never started.
> That document is the entry point: it lists what is still live here, which rebuild phase retires or inherits it, and what carries over untouched. This file stays as the record of how it was built and why.


Terrain and statics occlusion moves from a fixed axis-aligned box to a
general mesh, where a box cannot state the shape. The box stays the default
and remains free for everything it already covers. Mobiles and characters
are unaffected and stay billboards. The reasoning behind this direction —
what was argued, what changed, and the full session record — is in
[`lighting_geometry_archive.md`](lighting_geometry_archive.md).

## Scope

**Terrain and statics:** a box is the default primitive and covers a lid, a
body, a tread and a footprint at no extra cost — see
[`lighting.md`](lighting.md)'s "The occluding world". A mesh is the answer
only where a box, or several authored boxes composed together, cannot state
the shape at all: a curved roof, a mountain's slope, custom geometry that
isn't a stack of axis-aligned rectangles at any resolution worth authoring
by hand.

**Mobiles and characters stay billboards.** Their art is 2D sprite frames,
not skeletal geometry, and replacing that is a separate, much larger
question about the art pipeline that this document does not cover.

## What already supports a mesh occluder

- **A solid is already anchored in world coordinates and referenced, not
  owned, by every cell it touches** ([`lighting.md`](lighting.md)'s "The
  occluding world") — a mesh-backed solid needs the same bookkeeping, not
  new bookkeeping.
- **`facing::Blocks` already composes several authored axis-aligned boxes
  per graphic** ([`lighting.md`](lighting.md)'s "The art-measurement
  pipeline"), for a shape one box or one climb profile can't describe (an
  arch: a lintel over a gap two posts don't touch). It is the cheaper answer
  for an irregular but still axis-aligned silhouette, and is tried before a
  mesh for any given shape — a mesh is for what even several boxes can't
  state.
- **The art-measurement pipeline already lets a hand-authored entry win over
  a derived one** ([`lighting.md`](lighting.md)'s "The art-measurement
  pipeline") — a mesh is data nothing can derive from a flat 2D sprite, so it
  needs authoring rather than detection, the same path `Blocks` already
  uses. A mesh is a new payload in that existing authored column, not a new
  mechanism.
- **`crates/client/render/src/mesh.rs`'s own module doc already states the
  general case**: "a sloped roof, or any future custom geometry, builds its
  own `Mesh` the same way, and whatever walks one draws every `Face` alike."
  `MAX_FACE_VERTICES`/`MAX_MESH_FACES` are caps to raise against a real
  shape, not ceilings.

## What changes

- **`occlusion::Solid.space`** (`occlusion.rs:563`, today one box,
  `crate::solid::Solid`) needs a mesh-backed variant. Whether that is an enum
  on `Solid` itself or the box staying the fast path with a mesh addressed
  through a second indexed table is an open design question — see Status.
  The box's own fields (`opacity`, `edges`, the aperture) are unchanged for
  a box; whatever a mesh variant needs is additive.
- **`ray_vs_solid`** (`light.rs:1160`, and the shader copy in `blit.wgsl`)
  needs a mesh sibling, built on both the CPU and the GPU together against
  one shared parity fixture — the same discipline
  [`lighting.md`](lighting.md)'s "Testing and instrumentation" and
  [`lighting_raymarch.md`](lighting_raymarch.md) already run for the box
  test.
- **`solids.wgsl`'s debug view** stays exactly what it is for a box —
  three constant normals, six numbers and a colour, no index buffers — and
  grows a second draw path (a real triangle list) for a mesh occluder, so a
  mesh occluder can be looked at the same way a box one can.
- **The per-face normal question tracked in [`gbuffer.md`](gbuffer.md)'s
  Status and the occlusion-shape question here are one direction, decided
  together from here on** — a general per-face normal for shading a slope is
  smaller than a general shape for a ray to stop at (a box's normal already
  generalises from its own vertices without a mesh, per
  [`lighting.md`](lighting.md)'s "The occluding world"), but a mesh's
  silhouette is not the shadow of its bounding box, and that is what forces
  both questions to be answered together.

## Not affected

- Billboard rendering of mobiles and characters.
- The projection, the client's depth ordering, and drawing a slope as a
  parallelogram ([`lighting.md`](lighting.md)'s "Solids as drawable
  geometry") — these are about how a box is drawn on screen, independent of
  what shape the occluder underneath it is.
- The art-measurement pipeline's own mechanism (the atlas, the
  derive-then-override table) — a mesh is a new payload, not a new column.
- [`lighting_world.md`](lighting_world.md)'s field plane (sky, aperture,
  body) — computed by walking whatever occludes a tile's column, box or mesh
  alike. What is unproven, not assumed safe, is listed in Status.

## Status

**Not yet started**, in order (each depends on the one before it):

- The mesh variant's design for `occlusion::Solid.space` and how
  `Occlusion` bakes, indexes and caches it.
- `ray_vs_mesh`, CPU and GPU, built together against a shared parity
  fixture.
- One real, hand-authored content case (a curved roof or similar), to prove
  the authoring path against real content.
- `solids.wgsl`'s second draw path for a mesh occluder.
- `mesh.rs`'s `MAX_FACE_VERTICES`/`MAX_MESH_FACES` caps, revisited against
  the real content once it exists.

**Inherited, open questions:**

- WebGL2's storage-buffer ceiling ([`gbuffer.md`](gbuffer.md)'s Status) — a
  mesh's vertex data is a worse fit for a fixed-size `Rgba8Uint` grid than a
  box's six numbers. Not resolved here.
- Whether a sloped mesh roof still gives
  [`lighting_world.md`](lighting_world.md)'s sky-column test a clean
  single-bit answer is unproven — that test was built and measured against
  boxes only.
