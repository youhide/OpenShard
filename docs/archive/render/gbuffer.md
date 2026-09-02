# The G-buffer: the place attachment

> **Consolidated into [`lighting_rebuild.md`](../../render/design_model.md)** — the `place` format, which phase 2 replaces.
> That document is the entry point: it lists what is still live here, which rebuild phase retires or inherits it, and what carries over untouched. This file stays as the record of how it was built and why.


Every world pass (`ground.wesl`, `statics.wesl`, `mesh_face.wesl`) writes a second
colour target beside the picture: for each visible pixel, which tile the thing
drawn there belongs to, its height, what kind of thing it is, and which way its
surface faces. `blit.wesl` reads it to light the frame in world coordinates
instead of screen ones; `select.wesl` reads it a second time, for the ground a
selected thing stands on. `crates/client/render/src/place.rs` is the Rust side
of the format; the shader files are `.wesl` sources compiled to WGSL by
`crates/client/render/build.rs` (see [`lighting_raymarch.md`](lighting_raymarch.md)
for that toolchain).

This document describes what the attachment stores today and how it is
addressed. Shading itself — the raymarch, the occlusion grid, ambient and
sunlight — is [`lighting.md`](lighting.md), [`lighting_world.md`](lighting_world.md)
and [`lighting_raymarch.md`](lighting_raymarch.md). A general (non-axis-aligned)
occluder primitive is [`lighting_geometry.md`](lighting_geometry.md). The full
reasoning behind every choice below — arguments, alternatives considered,
measurements, and the session-by-session work that built it — is in
[`gbuffer_archive.md`](gbuffer_archive.md).

> **It is no longer the only plane.** `docs/lighting_rebuild.md` phase 2 added a
> position plane beside it — `Rgba32Float`, `(x, y, z, 1)`, written by all three
> world passes and read by `blit.wesl` in place of the `tile + fraction` and
> `unpack_place_z` reconstruction described below.
> `crates/client/render/src/gbuffer.rs` owns the set and is where a plane is
> added. What this document describes is still exactly what the `place`
> attachment holds; what has changed is that the *height* and the *sub-tile
> fraction* in it are no longer what the lighting reads.

## The place attachment's format

The attachment is `Rgba16Uint`, four `u16` channels
([`place::FORMAT`](../../../crates/client/render/src/place.rs), `place.rs:392`):

| channel | contents |
|---|---|
| 0 | the id's low 16 bits |
| 1 | the id's high 16 bits |
| 2 | `z + 128` in the low 8 bits, `Stance` in the 4 bits above |
| 3 | `Kind` in the low 2 bits, tile-local `x` in the next 7, tile-local `y` in the top 7 |

A fragment a sprite discarded writes nothing, so the attachment states what is
*visible*, which is the question lighting asks. `Kind::Nothing = 0` (the cleared
background, or a pass drawing something outside the world, such as a name over
a speaker's head) is not lit and not dimmed — `blit.wesl` passes it through
unchanged.

**Why an id and a depth, and not `(x, y, z)`.** The projection
([`camera.rs`](../../../crates/client/render/src/camera.rs)) is affine and fixed —
`screen_x = (x - y) * HALF_WIDTH`, `screen_y = (x + y - 1) * HALF_HEIGHT -
z * Z_STEP` — two equations in three unknowns. A screen pixel names a *line*
through world space, not a point: one degree of freedom is missing regardless
of how simple the projection is, because that is what an orthographic
projection is. The depth test every world pass already runs
(`depth_state()`, `LessEqual`) already picks the one point along that line
that is visible; what the attachment still has to answer is which *object*
the surviving fragment belongs to. Hence an id, addressing a row that carries
the rest.

The depth texture itself (`Depth24Plus`,
[`renderer.rs:46`](../../../crates/client/render/src/renderer.rs)) is not that
value and is never read back — every world pass writes
[`depth::Order::to_depth`](../../../crates/client/render/src/depth.rs) (`Order` at
`depth.rs:55`, `to_depth` at `depth.rs:74`), which folds `(tile - base) *
DEPTH_PER_TILE + priority_z` into one ordering key. `priority_z` is `z` bent
by object-kind-specific adjustments — ground averages its four corners and
subtracts 2 ([`land_priority_z`](../../../crates/client/render/src/depth.rs),
`depth.rs:104`), a static shifts ±1 by two tiledata flags
([`static_priority_z`](../../../crates/client/render/src/depth.rs), `depth.rs:129`),
a mobile adds 1 ([`mobile_priority_z`](../../../crates/client/render/src/depth.rs),
`depth.rs:149`) — so two different world heights can fold to the same key
(`depth::tests::priority_z_can_collide_for_two_different_world_heights`,
`depth.rs:236`, pins a flat static at `z=5` and a wall at `z=4` producing the
identical value). It is an ordering key, not a linear depth a position could
be reconstructed from.

## Kind and Stance

[`Kind`](../../../crates/client/render/src/place.rs) (`place.rs:69-85`) is two bits:
`Nothing = 0`, `Land = 1`, `Static = 2` (also a server-dropped ground item),
`Mobile = 3` (also a worn layer). It selects which per-kind storage buffer the
id addresses (see below) and, for `KIND_LAND`, tells `blit.wesl` to zero the
shading normal — land carries no facing.

[`Stance`](../../../crates/client/render/src/place.rs) (`place.rs:134-179`) is four
bits, ten values: `Upright = 0` (nothing known about which way it faces —
a tree, a body, a wall the art did not name an edge for), `Flat = 1` (a
floor, a rug, a road, and — since the fix below — land), `FaceNorth`/
`FaceEast`/`FaceSouth`/`FaceWest = 2..5`, four corner values
`CornerNorthSouth`/`CornerNorthWest`/`CornerEastSouth`/`CornerEastWest =
6..9`, and `MeshFace = 10`, a routing sentinel (see "Mesh faces" below) —
never a real stance, never written into a `SpriteQuad`. `STANCE_CORNER =
6` (`place.rs:184`) is the first of the four corner values; a corner's two
faces come out of its number by arithmetic, not a table:
`right = FaceNorth + (offset >> 1)`, `left = FaceSouth + (offset & 1)`, where
`offset = stance - STANCE_CORNER` (pinned by
`place::tests::a_corner_s_number_holds_both_of_its_faces`).

`STANCE_SHIFT = 8` (`place.rs:389`) is where `Stance` rides in the third
channel, above `z + 128`.

Land also carries `STANCE_FLAT` in the attachment (`ground.wesl` stamps it
alongside the height) so that `blit.wesl`'s occlusion self-exemption logic —
built for a wall-mounted fixture standing on the face it is bolted to — can
tell a flat surface from `Upright`'s "nothing known" case; without it, land
read as `Upright` and wrongly earned an exemption meant only for a fixture
on its own surface (`docs/lighting_raymarch_archive.md`, session 23). `kind`, not
`stance`, is what tells land apart from an ordinary standing surface for the
one consumer (a wall's flat cap) that also wants the half-space light gate
`STANCE_FLAT` otherwise implies — `fs_main` zeroes the normal for
`KIND_LAND` after computing it, rather than branching the gate on `stance`
alone.

## `pack_place`

[`place_format.wesl`](../../../crates/client/render/src/shaders/place_format.wesl)
carries the shift/mask constants and one packing function, shared by
`ground.wesl`, `statics.wesl` and `mesh_face.wesl` — the alternative, one
hand-built `vec4<u32>` literal per file, is what let one producer
(`ground.wesl`) go a full session without stamping `stance` at all (see
`gbuffer_archive.md` and `docs/lighting_raymarch.md` for that bug). WGSL has
no `#include`; each producer imports the same constants and calls:

```wgsl
fn pack_place(id: u32, raw_z: f32, stance: u32, kind: u32, sub: vec2<f32>) -> vec4<u32>
```

(`place_format.wesl:75-80`). `stance` is a required parameter, so a producer
cannot build a `place` value without deciding on one — that closes the
*omission* half of the bug class. It does not close the *commission* half: a
producer can still pass the wrong stance constant, and nothing catches that
but a per-producer, per-stance pixel-decode test (`tests/frame.rs` has one
for each of `ground.wesl`'s `Flat`, `mesh_face.wesl`'s `MeshFace` sentinel,
and `statics.wesl`'s `Flat`/`FaceEast`/`FaceSouth`).

`Place::packed()` (`place.rs:366-371`) is the Rust-side twin, used to build
the two words a `SpriteQuad` or `GroundQuad` carries and pinned by
`place::tests::a_place_packs_into_two_words`.

## The id and its storage buffers

The id addresses one of four per-kind storage buffers, selected by `kind`
(read from the attachment's own fourth channel, not from any bits of the id
itself — the id is the full, unconstrained 32-bit value
`place.x | (place.y << 16)`):

| `Kind` | buffer | row type | backing |
|---|---|---|---|
| `Land` | `ground_instances` | `GroundInstance` | the ground pass's own `GroundQuad` buffer, bound a second time |
| `Static` (ordinary) | `face_instances` | `FaceInstance` | the statics pass's own `SpriteQuad` buffer, bound a second time |
| `Static` + `Stance::MeshFace` sentinel | `mesh_instances` | `MeshFaceInstance` / `MeshFaceRow` | its own small storage-only buffer |
| `Mobile` | `mobile_instances` | `FaceInstance` | the mobiles pass's own `SpriteQuad` buffer, bound a second time |

No second upload for the first two and the fourth: `SpriteQuad`'s and
`GroundQuad`'s existing vertex buffers gain the `STORAGE` usage flag
alongside `VERTEX` (`renderer.rs:1541`, `renderer.rs:1654`) and are bound a
second time to `blit.wesl`'s and `select.wesl`'s fragment stage
(`blit.wesl` bindings 9-12, `select.wesl` bindings 3-4). `mesh_instances`
is a genuinely separate, storage-only buffer
(`new_mesh_row_buffer`, `renderer.rs:1508-1513`), because a mesh face has
no picture and shares no field with `SpriteQuad`. It also has its own,
much smaller initial capacity, `INITIAL_MESH_FACES = 64`
(`renderer.rs:1497`) — climbable statics are a small, bounded class next to
ordinary ones — growing by the same power-of-two-on-demand rule as
`INITIAL_QUADS`.

**Row shapes**, mirrored byte-for-byte between the Rust writer and the WGSL
reader:

- `FaceInstance` / `SpriteQuad` (`sprite.rs:25-61`): `rect: vec4<f32>`,
  `region: vec4<f32>`, `depth: f32`, `hue: u32`, `place: vec2<u32>`,
  `twin: u32` — 52 real bytes, but `SpriteQuad::STRIDE = 64`
  (`sprite.rs:82`, `16 * 4`): WGSL rounds a storage struct's size up to its
  own alignment (16 bytes, from the two `vec4<f32>` fields), so
  `SpriteQuad::write` pads with 12 trailing zero bytes to keep the two sides
  the same width. Only `place` is read by `blit.wesl`/`select.wesl` today;
  the rest of the struct exists so the stride matches.
- `GroundInstance` / `GroundQuad` (`ground.rs:24-74`): declared as sixteen
  plain scalars in WGSL rather than grouped into `vec4`s the way
  `FaceInstance` is — `GroundQuad::write`'s first field is a bare `(x, y)`,
  eight bytes, and a `vec4` field placed right after it would force WGSL to
  align it to sixteen bytes, opening a gap the real bytes do not have and
  reading every field after it four bytes short. `GroundQuad::STRIDE = 68`
  (`ground.rs:74`, `17 * 4`). Only `place0` (the packed tile) is read.
- `MeshFaceRow` / `MeshFaceInstance` (`mesh_face.rs:22-44`): `tile: u32`
  (packed `x | y << 16`), `stance: u32` — `STRIDE = 8`, no padding, because
  this buffer is never bound as a vertex attribute.

**Capacity.** All three vertex/storage-dual buffers start at
`INITIAL_QUADS = 4096` (`renderer.rs:38`) and grow by doubling
(`next_power_of_two()`) whenever a frame's quad count exceeds the current
capacity (`renderer.rs:506-510` for ground, `renderer.rs:1073-1075` for
statics/mobiles) — never a hard cap, never a dropped instance. The initial
allocation is not sized to measured load: the widest real frame measured
(Britain, widest zoom, `Cutaway::OPEN`, via `tests/cost.rs:300`) draws
27,889 ground quads and 6,560 static quads (431 of them corner-stance,
6,991 faces once corners are split), 6.8x and 1.6x `INITIAL_QUADS`
respectively — so the first frame drawn at that location reallocates before
a single pixel is on screen, on every run. Mobile and mesh-face counts are
not in this measurement: mobiles are server population, not map data, and no
map-density argument bounds them from a static snapshot; mesh faces are a
small, bounded class (climbable statics/items only) next to 6,560 ordinary
statics.

## The face-instance row: what moved, what stayed

Only a static's or a mobile's own **tile** — the integer `(x, y)` its
instance stands on — moved off the attachment and onto its row. Two things
that look like they belong in an instance row do not, because they are
genuinely per-fragment, not per-instance:

- **`z`.** A standing face's height is recomputed per fragment from screen
  position in `statics.wesl` (`z = base + ((sub.x + sub.y - 1) *
  HALF_TILE_HEIGHT - down) / Z_STEP`) — that is what gives a wall a lighting
  gradient down its face instead of one flat brightness.
- **The sub-tile fraction.** Computed per fragment for a static's face the
  same way it is for the ground, so that "the next tile along a wall's run
  starts its fraction at 0 where this one ended at 1" — a row of wall tiles
  reads as one continuous surface, not a row of separately lit sprites. Both
  a wall's and a floor's fraction spread across the whole tile the same way
  ground's does.

Both stay exactly where and how they were computed, packed into the
attachment's own channels 2 and 3. Ground needs no equivalent move at all:
`ground.wesl`'s vertex shader already evaluates the bilinear height formula
once, exactly, at each of the tile's four real corners, and the rasteriser's
own linear interpolation of that per-vertex height and fraction across the
two triangles was always the "free" part — ground was never reconstructing
anything from a closed form, so only its tile (`ground_instances[id].place0`)
needed to move.

## Corner faces

A corner static's picture is two faces at once — the north-or-east half and
the south-or-west half of its column, `statics.wesl` already resolves this
per fragment (see the `Stance` arithmetic above). What the attachment could
not do before was address the two halves separately: both wrote one shared
id.

[`SpriteQuad::twin`](../../../crates/client/render/src/sprite.rs) (`sprite.rs:60`)
is the fix: for a row with a corner stance,
[`split_corners`](../../../crates/client/render/src/sprite.rs) (`sprite.rs:161`)
appends a second, undrawn row past the frame's real instances, sharing the
same tile, and sets the drawn row's `twin` to point at it. `statics.wesl`'s
existing `across > 0.0` test — the same test that already resolves which
half's `Stance` a pixel gets — picks which of the two ids a pixel's half
writes to the attachment. No second rasterised triangle, no second pipeline.
A wall's sprite has one relevant face and one id, unchanged.

`split_corners` runs on the **merged** list of map statics and
server-dropped items, at the call site in
[`crates/client/app/src/lib.rs:4464`](../../../crates/client/app/src/lib.rs), after
both are collected and appended — an item can carry a corner `Stance` the
same way a map static can (both go through the same placement arithmetic),
so running it only over map statics would leave a corner-shaped item's two
halves sharing one id.

## Mesh faces

A tread is a box with up to two faces a camera ever sees: its top, and the
riser between it and the tread before it. Instead of one flat billboard
approximated by a blended normal, each face is drawn and shaded as its own
honest, axis-aligned surface — a top's normal is `[0, 0, 1]`, a riser's is
the climb direction's own outward normal, neither blended nor derived from a
neighbour.

**Geometry.** [`crate::mesh`](../../../crates/client/render/src/mesh.rs) is a
small, producer-agnostic abstraction: `Face` (`mesh.rs:39`) is up to
`MAX_FACE_VERTICES` `= 4` (`mesh.rs:21`) corners in ring order plus a unit
normal; `Mesh` (`mesh.rs:97`) is a fixed-capacity list of up to
`MAX_MESH_FACES` `= 2 * MAX_TREADS = 8` (`mesh.rs:29`, `MAX_TREADS = 4` at
`facing.rs:1377`) faces. `Face::fan` (`mesh.rs:83`) triangulates a
four-corner face as `0,1,2,0,2,3`.

[`Prism::mesh`](../../../crates/client/render/src/facing.rs) (`facing.rs:1205`)
builds one `Mesh` per climbable, prism-fit static: a flat top per tread at
its own real height, and a riser between it and the tread (or the static's
own base) before it, facing `[-ox, -oy, 0]` for the climb direction's
outward `[ox, oy]`. A riser stops at exactly the two treads' own heights, and
the tie at that edge is watertight by construction: both sides are built from
the same `footprint` expression and the same `top_z`, so the shared corners are
bit-identical in world space, and `statics::push_mesh` projects a corner with a
pure function of that corner.

> **This paragraph used to say the opposite, and the reading in it was wrong.**
> Risers were grown by a `SEAM_OVERLAP` of `0.15` `z` at both ends "because two
> exactly-touching quads left a hairline of the enclosing sprite's own flat
> shading surviving at the projected pixels the rasteriser assigned to neither
> triangle". The hairline was real; the edge named for it was not.
> `examples/synthetic_stair`'s face map — one colour per plane, straight off the
> `place` attachment — finds **zero** pixels inside a flight's silhouette
> belonging to no face without the overlap, over four climb directions × four
> zoom notches × five tread profiles, and the tread count is what moves that
> edge's own sub-pixel phase. `WIDTH_OVERLAP`'s own doc had already measured
> where the leak actually was — "not at a tread/riser tie at all", but the outer
> silhouette, where the fitted prism meets the art's true one and no second face
> borders the edge at all — and the constant aimed at the wrong edge was left
> standing beside it. What it cost while it stood: 1120 pixels of a single
> flight drawn outside their own plane, a one-pixel dark hairline across every
> lit tread, and every step's corner displaced `2.4` px at `4:1`.
> `facing.rs`'s `a_tread_and_its_riser_share_an_edge_bit_for_bit` is the gate on
> the property that replaces it.

`Prism::tread_normal` (a blended `Surface::Flat`/`Surface::Face` normal
standing in for a tread's top) and `light::Surface::Sloped` /
`Spot::sloped` no longer exist: measuring `light::sample` against
`examples/isolated_scene.rs`'s own stair reproduction at each tread's real,
constant height (rather than the fake continuous ramp the blend was fit to)
showed the "hard cliff" that motivated the blend was a property of that
sampling, not of the real geometry — a flat top reads a flat `cone` value,
and the treads above it are correctly, fully occluded. `light::Surface` is
`Upright` / `Flat` / `Face(Face)` only (`light.rs:1367`).

**Rendering.** `renderer::MeshFaceRenderer` and `mesh_face.wesl` are a
second, invisible pipeline — not a variant of `SpriteRenderer`'s — because a
mesh face's true screen shape is an arbitrary projected quadrilateral, not
an axis-aligned rectangle. It draws raw, CPU-triangulated vertices
(`MeshFaceVertex`, `mesh_face.rs:55-93`, `STRIDE = 36`) and writes only
`place` — no colour target, the same shape `SpriteRenderer::render_mask`
uses for a pass that ignores the visible picture. Depth is the enclosing
static's own `SpriteQuad::depth`, reused rather than recomputed.

Each vertex carries both its projected screen position and its true world
position (`MeshFaceVertex::world`); because the projection is affine and
every face is planar, the rasteriser's own linear interpolation gives every
fragment an exact world position for free — `sub = fract(world.xy)`,
`z = world.z`, neither approximated nor re-derived the way a standing
sprite's per-fragment `z`/fraction are. `MeshFaceVertex::tile`
(`mesh_face.rs:80-92`) carries the face's own known tile alongside `world`,
because subtracting it rather than flooring `world.xy` is what keeps a
fragment on a face whose own edge sits on a whole-number boundary (a
tread's outer corner) from being assigned to the wrong side of that
boundary.

**The id scheme does not grow `Kind`.** A mesh face stays `Kind::Static`;
`Stance::MeshFace = 10` in the attachment's stance bits is a routing
sentinel, not a real stance — `blit.wesl` reads it before resolving a tile
and, on seeing it, reads `mesh_instances[id]` (a `MeshFaceRow`: tile +
the face's *real* stance) instead of `face_instances[id]`. The real stance
is always one of `Flat`/`FaceNorth`/`FaceEast`/`FaceSouth`/`FaceWest`,
because `Prism::mesh` only ever produces those five exact normals today;
[`Stance::of_normal`](../../../crates/client/render/src/place.rs) (`place.rs:249`)
recovers the stance from a `[f32; 3]` normal for exactly that closed set,
pinned by `place::tests::of_normal_recovers_every_stance_prism_mesh_can_produce`.
`blit.wesl`'s existing `outward(stance)` then gives the normal unchanged —
no packed general-vector encoding exists or is needed for these five.

**Collection.** Both map statics (`statics.rs`'s `push_mesh`/`collect`) and
server-dropped ground items
([`items.rs:104-153`](../../../crates/client/render/src/items.rs)) build mesh
vertices and rows for any placement carrying `Placed::prism` — a climbable
item gets the same honest mesh a climbable map static does. Both lists are
merged before `MeshFaceRenderer::render` runs once over the combined set
(`crates/client/app/src/lib.rs:4457-4461`).

## Occlusion-side tread geometry

The render-side decomposition above has an occlusion-grid twin:
`occlusion::Builder::add`'s climbable branch
(`occlusion.rs:1524`, the decomposed case at `occlusion.rs:1569-1610`)
pushes two `Solid`s per tread instead of one whole-tile body — a
zero-height lid at the tread's own top
([`Solid::tread_top_box_of`](../../../crates/client/render/src/occlusion.rs),
`occlusion.rs:791`) and a panel spanning the rise from the tread before it
([`Solid::tread_riser_box_of`](../../../crates/client/render/src/occlusion.rs),
`occlusion.rs:832`), its edge named `opposite(edge_of(up))` the same way an
ordinary named-edge wall panel's is. Both box constructors share
`Solid::strip_footprint` (`occlusion.rs:892`) for the climb-axis footprint
math; `Solid::tread_box_of` no longer exists.

**Fallback.** A climbable static whose art the prism-fit search cannot
decompose into treads still gets one whole-tile, `EDGE_ANY` body
(`occlusion.rs:1624-1635`) — the same answer every climbable static got
before per-tread fitting existed. This is a known, standing gap, not a bug:
[`lighting.md`](lighting.md)'s own measurement of the fit's coverage is the
source for how much of the install this fallback still covers.

Full DDA/raymarch mechanics — the shadow walk itself, corner-tie parity,
the ground-stance bug this same `Solid` decomposition surfaced — are
[`lighting_raymarch.md`](lighting_raymarch.md).

## Selection

`select.wesl` asks two different questions per fragment, of two different
records:

1. **Is this pixel the selected object?** Answered by a mask: a separate,
   tiny silhouette draw
   ([`SpriteRenderer::render_mask`](../../../crates/client/render/src/renderer.rs),
   `renderer.rs:1158`) with its own instance numbering starting at zero, not
   a second use of the world pass's ids. A pixel's `place` id can never be
   compared against "the picked object's own id" — there is no shared id
   space between the two draws.
2. **Is this pixel the ground the selected thing stands on?** Answered by
   the `place` attachment, resolved through the same `face_instances` /
   `ground_instances` buffers `blit.wesl` reads (`select.wesl` bindings 3
   and 4, mirroring the same `FaceInstance`/`GroundInstance` struct
   layouts). The ground wash tests `kind == KIND_LAND || (kind ==
   KIND_STATIC && stance == STANCE_FLAT)` (`select.wesl:146`) — `stance`
   here is the attachment's own bits, read directly, never resolved through
   `mesh_instances`.

**Known gap.** A mesh face's attachment stance is always the
`Stance::MeshFace` routing sentinel, never its real stance (which lives in
`mesh_instances`, not the attachment) — so a tread's or a lid's own flat
top, drawn through the mesh pass, can never satisfy the ground-wash test's
`stance == STANCE_FLAT` check even though it genuinely is
`Stance::Flat`. Standing since the mesh pass landed, not introduced by
anything since.

## Picking is unrelated

`statics::pick`, `items::pick`, `mobiles::pick`
(`crates/client/render/src/{statics,items,mobiles}.rs`) answer "what is
under the cursor" entirely on the CPU, by replaying each candidate's own
placement and testing the atlas for an opaque texel — no readback of
`place`, no GPU round-trip. The four near-duplicate walkers this makes
(`statics`/`items`/`mobiles`/`gump`) are a separate, smaller cleanup tracked
in `docs/client.md`'s M5 backlog, unrelated to this attachment.

## Geometry-agnostic by design

Nothing about the id-and-depth scheme assumes the surface is a flat
billboard. A depth and an id are exactly what a rasterised triangle of real
geometry produces too — reconstruction never depended on the shape of the
thing drawn, only on what its own row says. `docs/client.md`'s later
milestones bring more real geometry into this client; nothing here needs to
change shape when they do. The one alternative this design does *not* take
— deriving a billboard's exact world position from its screen row and a
closed-form offset, true today and false the day a sprite becomes a mesh —
was considered and dropped for exactly that reason.

## Status

**Built and running**, all verified by `cargo test --workspace`/clippy/fmt
and the frame-parity suite:

- The `(id, z+stance, kind+fraction)` attachment format, `pack_place`, and
  the four per-kind storage buffers (`ground_instances`, `face_instances`,
  `mobile_instances`, `mesh_instances`), read by both `blit.wesl` and
  `select.wesl`.
- Corner faces addressed as two ids via `SpriteQuad::twin` /
  `split_corners`.
- Honest per-face mesh geometry for treads (top + riser), replacing the
  blended `Prism::tread_normal`/`Surface::Sloped`, for both map statics and
  server-dropped climbable items.
- The occlusion grid's matching per-tread lid+panel decomposition.
- Direct pixel-decode test coverage (not just shadow-parity coverage, which
  can pass for the wrong reason) for every `pack_place` caller's stance:
  `ground.wesl`'s `Flat`, `mesh_face.wesl`'s `MeshFace` sentinel,
  `statics.wesl`'s `Flat`/`FaceEast`/`FaceSouth`.

**Known, standing gaps** (not bugs in progress — stable, current facts):

- Buffer capacity is one flat `INITIAL_QUADS = 4096` shared by all three
  vertex/storage buffers, not sized to the measured per-kind load; the
  widest real frame measured reallocates both the ground and the static
  buffer on its first frame, every run.
- `select.wesl`'s ground-wash test cannot recognize a mesh face's own real
  `Flat` stance (a tread's or lid's top drawn through the mesh pass), only
  the attachment's routing sentinel.
- A climbable static the prism-fit search cannot decompose into treads
  still occludes as one whole-tile body, not per-face geometry.
- `statics.wesl`'s `FaceNorth`/`FaceWest` stances have no direct
  pixel-decode test (only `Flat`/`FaceEast`/`FaceSouth` do) — rare in
  practice (five graphics out of 1197), not pointed at by any known bug.

**Open question.** The per-face normal for anything that is not one of
`Prism::mesh`'s five axis-aligned cases — an inclined roof, a curved
surface, arbitrary future custom geometry — has no format yet. A general
packed normal (an 8-bit octahedral encoding or similar was suggested) is
the likely shape, but nothing has been measured against a real render-side
consumer, and none exists yet. This is now linked to the occlusion grid's
own equivalent gap — a mesh occluder's *silhouette*, not just its shading
normal, is wanted for the same curved-roof case — both pointing toward
[`lighting_geometry.md`](lighting_geometry.md) for the geometry-primitive
direction; this document's own scope stays the render-side normal encoding
itself. See `gbuffer_archive.md` for what has been tried and ruled out.
