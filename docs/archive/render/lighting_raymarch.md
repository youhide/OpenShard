# The shadow raymarch: boundary precision and CPU/GPU parity

> **Consolidated into [`design_model.md`](../../render/design_model.md)** — the walk itself, which survives.
> That document is the entry point: it lists what is still live here, which rebuild phase retires or inherits it, and what carries over untouched. This file stays as the record of how it was built and why.


Current state of the boundary-precision machinery inside the shadow ray
walk: how a cell index is derived without landing on the wrong side of a
tile edge, what is proven identical between the CPU and GPU implementations
of the walk, the one known place they still disagree, and the WESL
toolchain that builds the shaders both sides read their shared constants
from. The reasoning behind each of these — bugs found, approaches tried and
rejected, the full session-by-session work that built it — lives in
[`lighting_raymarch_archive.md`](lighting_raymarch_archive.md), organized
under headings that mirror this file's.

This file is a satellite of [`lighting.md`](lighting.md): that doc states
the walk's *rules* (what a panel does, what a body does, the self-shadow
exemptions); this one states how the walk stays numerically exact against
those rules once a coordinate lands exactly on a boundary.
[`gbuffer.md`](gbuffer.md) covers the `place` attachment's own format and
`pack_place()`, including the stance-omission bug that motivated the WESL
migration described below — this file covers the *build toolchain* that
compiles the shaders, not the packing format itself.
[`lighting_geometry.md`](lighting_geometry.md) is where the occluding
primitive's box assumption is being lifted for shapes a box cannot state.

Written against `crates/client/render/src/light.rs`, `mesh_face.rs`,
`statics.rs`, `debug.rs`, `solid.rs`, `occlusion.rs`, `tests/lighting.rs`,
`tests/frame.rs`, `examples/synthetic_stair.rs`, `examples/isolated_scene.rs`,
`examples/boxes.rs`, `build.rs`, and the shader sources under
`src/shaders/` (`ground.wesl`, `statics.wesl`, `mesh_face.wesl`,
`select.wesl`, `blit.wesl`, `place_format.wesl`).

Everything below assumes the occluding primitive is an axis-aligned box —
`ray_vs_solid`, `walk_cells_streaming`, the whole DDA. `lighting_geometry.md`
is where that assumption is being lifted: a mesh occluder is wanted for a
shape a box cannot state, and will need a `ray_vs_mesh` sibling to
everything this file describes for the box case. None of it is stale
because of that — the box stays the default, and everything below keeps
mattering for it — but a mesh ray-test needs this file's parity discipline
(below) read first: keeping two independent implementations of one formula
from silently drifting apart is the harder half of what this file
describes, not the box arithmetic itself.

## The tile-boundary hazard

A shadow ray's start and end each belong to one integer tile, and the DDA
walk needs to know which tile to begin stepping from. A raw world
coordinate near an exact tile edge is not a reliable way to recover that
tile: `floor()` of a coordinate sitting exactly on a boundary can round to
either neighbour, and both ends of a ray are nudged off the surface they
are drawn on (`stand_clear`) before the walk ever sees them — so "the tile
a coordinate floors into" and "the tile the point is conceptually standing
on" can disagree exactly at a boundary, which is exactly where a wall's own
far edge, a tread's own edge, or a fragment sitting on its own tile's exit
edge lives.

`light::Spot` (`light.rs`) and `MeshFaceVertex` (`mesh_face.rs`,
`mesh_face.wesl`) both carry their own tile explicitly instead of having
the walk re-derive one. `Spot::at`/`::flat`/`::face` take
`tile: (i32, i32)` from the caller, who already knows it — a static's own
placement, a ground tile being iterated, a test fixture's own coordinates —
and `walk_cells_streaming`'s first cell and its per-axis `boundary` seed
both read the carried tile rather than flooring the (already-nudged)
`from` position. Once a tile is carried and never re-derived, a fraction
that legitimately sits on an exact boundary is harmless: it still names the
right starting cell, and the walk's own `INSIDE` fraction clamp
(`lighting.md`'s "The G-buffer bridge") keeps a sub-tile fraction from ever
reading as the *next* tile's `0.0` in the first place. This is the fix
every other boundary-precision guarantee in this file depends on.

## `ray_vs_solid` and the cell walk

An occluder is a `Solid` — an axis-aligned box (`solid.rs`,
`lighting.md`'s "The occluding world"). `ray_vs_solid` is the exact slab
method: where a straight segment enters and leaves the box, as a fraction
of the whole segment, continuous throughout and with no notion of "which
tile" anywhere in it. It is mirrored, not shared, on the two sides that
need it — CPU: `light::ray_vs_solid` (`light.rs:1160`, used directly, and a
second copy inside `walk_cells_streaming`); GPU: `ray_vs_solid` in
`shaders/blit.wesl:763` — because CPU Rust and WGSL cannot share a function
body. A tangent touch (the segment grazing exactly one edge or corner of
the box without crossing into it) comes back as a real, zero-length
crossing rather than `None`; whether a caller treats a zero-length touch as
"blocked" is left to the caller, not decided inside the slab test.

`walk_cells_streaming` (`light.rs:2304`, CPU) and `walk`
(`shaders/blit.wesl:975`, GPU) are the one shared algorithm, ported line for
line: a single-axis DDA (no diagonal jump) that steps toward whichever of
`boundary.x`/`boundary.y` is nearer, testing every solid on each candidate
cell against `ray_vs_solid`'s own exact interval rather than reconstructing
one from the DDA step itself. The per-cell occlusion decision is an inline
closure in `walk_cells_streaming` on the CPU side and the free function
`cell_stopped` (`shaders/blit.wesl:838`) on the GPU side — WGSL has no
closures, so the two are the same logic in a different shape, not a
different decision. `candidate_tiles`/`dda_walk` (`light.rs:1946`, `1843`)
are a third, independent enumeration — a DDA that never skips a cell — kept
as an oracle for `walk_cells_exact` (`light.rs:2025`), not part of the
shipped render path.

**One asymmetric, already-landed tolerance.** `shaders/blit.wesl` widens
`cell_stopped`'s own rejection by `RAY_TANGENT_TOLERANCE = 1.0e-2`, applied
only at the walk's own trajectory cell (`0.0` at the unconditional diagonal
probe `candidate_tiles`'s own shape already covers) — a genuine near-tangent
crossing that GPU `f32` rounding was missing on one specific corner shape.
The CPU side (`light::ray_vs_solid`) carries no equivalent tolerance and
does not need one: CPU's own rounding already lands on the generous side of
the same tangent, and widening it anyway was tried and reverted — it
collapsed a real, if small, interior crossing on a cell only
`walk_cells_exact`'s wider candidate set reaches, turning one fixed
disagreement into several new ones (caught by proptest fuzzing
`walk_cells_streaming` against `walk_cells_exact`, not by inspection).

**What used to exist and does not anymore.** `walk_cells`, `corner_tie`,
`panel_stop` and `DdaTransition::Corner` are gone, both sides — a corner is
handled today by `candidate_tiles`'s unconditional diagonal-neighbour probe
plus `ray_vs_solid`'s own exactness, not by a special-cased jump or a
hand-tuned tie-break. The per-cell *rules* themselves (panel pierce, a
body's length-vs-pierce test, a lid's strict crossing, a corner's
supercover) are `lighting.md`'s own subject ("The shadow ray walk"); this
file is about keeping two implementations of those rules numerically
identical, not what the rules are.

## CPU/GPU parity

`light::sample` (`light.rs`) is the shader's ray walk re-implemented on the
CPU — the same functions in Rust that `shaders/blit.wesl` has in WGSL. It
exists because "why is this pixel lit" needs a list of reasons a rendered
picture cannot produce, but a second implementation of one formula only
earns its keep if the two are actually held together: `tests/frame.rs`'s
`assert_parity` uploads a synthetic `place` attachment, runs the real blit
shader over it, and asserts every sampled pixel agrees with `light::sample`
fed the same input, across dozens of scenes (`room`,
`wall_with_a_torch_beside_it`, `house_corner`, `wall_with_a_hole_in_it`,
`lantern_in_a_room`, and more) chosen to exercise a different branch of the
walk each — a plain whole-tile room, a named panel, a house corner where
both cell shapes touch, a panel with a hole in it. A parity failure means
the CPU debugging oracle has silently diverged from what the shader
actually draws, which is a tooling-trust question, not by itself a claim
about whether the picture is correct.

## The one known parity gap

At an exact tile-corner tie, `light::sample` and `shaders/blit.wesl`'s copy
of the identical formula can disagree about which axis is nearer:
`per_tile[axis] = 1.0 / delta[axis].abs()` is computed independently on
each side, and the two `f32` divisions can round differently under CPU
Rust arithmetic than under the GPU shader compiler (naga/wgpu), sending the
walk's very first step into a different cell on the two backends for a ray
whose geometry is a genuine, exact tie. This is a real configuration a
regular grid against a fixed light will eventually land on — not a
contrived probe — but every caller of `light::sample` in the workspace is
debug or test tooling (`tests/frame.rs`, `tests/lighting.rs`,
`examples/isolated_scene.rs`, `examples/boxes.rs`,
`artscan/examples/probe.rs`, `debug.rs`'s tile-brightness map);
`shaders/blit.wesl`'s own `walk` is the only thing that ever draws a frame a
player sees, and it is internally self-consistent regardless of which side
of a tie its own comparison lands on. So the gap is "does the CPU debug oracle
still match the shader," not a visible shadow defect.

Two tests in `tests/frame.rs` are `#[ignore]`d for exactly this, each with
the reasoning in its own doc comment:
`the_shader_lights_a_frame_as_light_sample_does` and
`the_shader_and_light_sample_agree_about_a_carried_beam`. An epsilon bias
on the stepping comparison (`boundary.x < boundary.y - EPS`) was tried
first and made things measurably worse — it fixed the tied case but sent
`walk_cells_streaming`'s own stepping down a cell a bare comparison would
not have, failing ordinary-geometry parity tests nowhere near a tie — so no
epsilon is applied on either side today. Two repair shapes remain untried:
a cross-multiplied comparison (`ahead.x * abs(delta.y)` against
`ahead.y * abs(delta.x)`, avoiding the reciprocal division whose rounding
differs between backends — cheap, worth trying first) and a walk that
tests both branches of every exact tie rather than committing to one,
making the outcome order-independent by construction (real structural
work, its own session).

## The reference path tracer

Everything in this file is about holding two implementations of *one* walk to
each other. [`path_tracer.md`](../../render/reference/path_tracer.md) is the other half:
a third renderer that shares no arithmetic and has no notion of a tile, so it
can arbitrate where the two copies here disagree. Its degenerate mode reads
**zero interior disagreements** against the rendered frame on all three of
`boxes.rs`'s scenes, which bounds what is left of the two open residuals below
— whatever they are, they live inside one pixel of a shadow's own edge.

## The ground-occlusion oracle

`examples/boxes.rs` runs two independent visibility oracles against the
rendered picture, neither sharing arithmetic with `light::sample` or
`shaders/blit.wesl` — both are a fresh, textbook point-vs-AABB slab test
(`segment_clear_of_box`/`oracle_visible`), because reusing the engine's own
`ray_vs_solid` to check the engine's own walk would prove nothing.

**The box-top oracle** sweeps a grid over each box's own top face and
compares against the rendered `View::Shadow` pixel there
(`OPENSHARD_BOXES_ORACLE=0` to skip, on by default). Both the `tree` and
`line` scenes currently read `0/9216` disagreements on both box tops — this
half is closed.

**The ground oracle** (`OPENSHARD_BOXES_GROUND_ORACLE`, on by default)
sweeps a dense `240×240` grid of *ground* points instead, projects each
through the scene's real camera to the pixel it lands on, skips any point
inside a box's own footprint (that pixel is the box's mesh, not ground),
and compares. It still finds disagreements, split into two shapes:

- In the `tree` scene, `368` points read "too dark" — the oracle asks about
  a world point whose projected screen pixel is actually covered by a
  taller neighbouring box's own mesh (isometric depth: a taller object
  drawn over the ground behind it), not a point the renderer ever claimed
  was visible ground. This is a known limitation of the oracle's own
  methodology, confirmed by the pixel colour matching the fully-blocked-
  on-mesh constant exactly — not an engine defect, and not counted against
  the walk itself.
- In the `tree` scene, `159` points read "too light": `light::sample`
  predicts occlusion at a box's own silhouette *corner* that the rendered
  picture misses. In the `line` scene (whole-tile boxes, so the ground
  attachment's sub-tile quantisation cannot be a confound) `692` points
  show the same "too light" shape; this count predates the WESL migration
  below and has not been independently re-measured since, though the
  migration itself is confirmed not to change any rendered output.

Given that hard shadows (no corner softening at all) are what this pass
draws today, the `159`/`692` "too light" residuals plausibly are the same
near-tangent CPU/GPU divergence "The one known parity gap" above already
describes, landing in a different scene — but this is **not confirmed**:
nobody has checked whether the `tree` and `line` residuals are actually the
same shape, only that both sit at a box corner and both are consistent
with the same family of near-tangent rounding disagreement. The next check
is a direct one: rerun the ground oracle scoped tight to a single box's own
corner in each scene and compare the two residual shapes directly.

Reproduce:

```sh
OPENSHARD_BOXES_SCENE=tree cargo run --release -p openshard-client-render \
    --example boxes
```

(or `OPENSHARD_BOXES_SCENE=line`) — the ground oracle runs by default and
prints both scenes' mismatch counts, split by direction, plus a handful of
example points, on stderr.

## The WESL build

Shader sources are `.wesl` files under `crates/client/render/src/shaders/`
— `ground.wesl`, `statics.wesl`, `mesh_face.wesl`, `select.wesl`,
`blit.wesl` — compiled to plain WGSL at build time by `build.rs` (the
crate's first build-dependency, `wesl = "0.4"`) via
`wesl::Wesl::new("src/shaders")` and `build_artifact`, one call per file;
each of `renderer.rs`/`blit.rs`/`select.rs` loads its compiled output through
`include_str!(concat!(env!("OUT_DIR"), "/<name>.wgsl"))` in place of the
old `include_str!("<name>.wgsl")`. This exists so the `place` attachment's
shift/mask constants and its one packing function,
`pack_place(id, raw_z, stance, kind, sub) -> vec4<u32>`, live once in
`place_format.wesl` and are imported by the three producers rather than
hand-copied per file — WGSL alone has no `#include`, so before this
migration each of `ground.wgsl`/`statics.wgsl`/`mesh_face.wgsl` carried its
own copy of `KIND_*`, `SUB_TILE`/`SUB_TILE_MASK`,
`PLACE_STANCE_SHIFT`/`PLACE_STANCE_MASK`/`PLACE_Z_MASK` and
`STANCE_FLAT`/`STANCE_FACE_*`/`STANCE_CORNER`/`STANCE_MESH_FACE`, free to
drift silently. See [`gbuffer.md`](gbuffer.md)'s "`pack_place`" section for
what the shared function actually closes (the *omission* half of a
producer never stamping a value) and what it does not (a producer stamping
the *wrong* value, still only caught by a per-producer pixel-decode test).
`statics.wesl` still declares its own `STANCE_SHIFT`/`STANCE_MASK` locally
on purpose — a different word, the *instance* input's stance bits at
shift 16, not the attachment's own shift 8 — noted in its own comment
rather than folded into the shared file.

`wesl-rs` (the crate doing the compiling) is stricter than naga was about
one corner of WGSL's own grammar: mixing `<<` and `|` in one expression
without parentheses, which the grammar actually requires and naga had been
accepting anyway. `ground.wesl`'s own `sub` line needed parentheses added
for this; the other four files already parenthesized every mixed
expression and needed no change. Compiling all five costs no nightly
toolchain — it builds clean under this workspace's own stated MSRV
(`rust-version = "1.88"`, root `Cargo.toml`).

## Status

Built and current:

- A shadow ray's start and end always carry their own integer tile
  explicitly (`Spot::tile`, `MeshFaceVertex::tile`); no boundary-adjacent
  coordinate is re-derived by flooring a raw float.
- `ray_vs_solid` (exact slab test) and `walk_cells_streaming`/`walk`
  (single-axis DDA, no corner-jump) are the one algorithm, mirrored on CPU
  (`light.rs`) and GPU (`shaders/blit.wesl`). `walk_cells`, `corner_tie`,
  `panel_stop` and `DdaTransition::Corner` no longer exist.
- CPU/GPU parity is enforced by `tests/frame.rs`'s `assert_parity` across
  dozens of scenes, and by two independent, no-shared-arithmetic oracles in
  `examples/boxes.rs` (a box-top oracle and a ground oracle).
- The box-top oracle reads `0/9216` disagreements on both scenes it checks.
- The WESL build: five `.wesl` shader sources compiled to WGSL at build
  time, importing the `place` attachment's packing constants and
  `pack_place()` from one shared `place_format.wesl` file instead of five
  hand-copied blocks.

Open:

- **The corner-tie parity gap.** At an exact tile-corner tie,
  `light::sample` and `shaders/blit.wesl`'s copy of `1.0 / delta[axis].abs()`
  can round differently between CPU and GPU arithmetic, stepping the walk into
  different first cells. Accepted rather than chased: `light::sample` is
  never in the real render path, so this is a debug-oracle/renderer
  disagreement, not a visible defect. Two tests are `#[ignore]`d for it
  (`the_shader_lights_a_frame_as_light_sample_does`,
  `the_shader_and_light_sample_agree_about_a_carried_beam`, both in
  `tests/frame.rs`); a cross-multiplied stepping comparison and a
  tie-order-independent walk are both untried repairs.
- **The ground-oracle "too light" residual.** `159` points in the `tree`
  scene and `692` in the `line` scene still disagree with `light::sample`
  at a box's own silhouette corner. Plausibly the same phenomenon as the
  corner-tie gap above, landing in a different scene — not yet confirmed;
  the two residual shapes have not been directly compared.
- **A dark hole inside a lit vertical face** — `examples/boxes.rs`'s `tree`
  scene, a closed dark patch inside the lower box's own south face, below
  the joint and below the top edge, lit above it and lit below it. The
  cause is that height is an integer everywhere a shadow is decided:
  `pack_place` writes `round(raw_z)`, so a face's continuously varying
  height becomes one-unit treads, and where a tread lands on a neighbouring
  solid's own base `on_surface`/`exemption` reads that solid as the
  fragment's own and drops it from the walk. Confirmed by construction:
  `OPENSHARD_TREE_H1=3.5` moves the joint off an integer and the face comes
  back clean. **Not** a missing footprint — `footprint_bytes` and `box_of`
  already carry a solid's horizontal extent exactly; the asymmetry is that
  horizontal geometry is exact and vertical geometry is rounded. Its own
  track now: [`lighting_height.md`](lighting_height.md), which also owns the
  exemption-by-identity work this needs to close for good.
- **`examples/boxes.rs`'s own module doc still describes `walk_cells`**,
  which no longer exists — including the `light.rs:2269` line number and
  the `walk_cells_exact` comparison the `OPENSHARD_BOXES_ORACLE_EXACT`
  knob ran. The knob and the prose need re-reading against
  `walk_cells_streaming`.
- **A true fixed-point world coordinate** (tile + N bits of sub-tile
  resolution, no `f32`) would remove float-epsilon boundary bugs at the
  source rather than by carrying a tile explicitly around each one. Buys
  nothing more for the bug classes above than carrying the tile already
  closes; would buy something broader (no float epsilon anywhere a world
  position is stored or compared), but that is a question about
  `geometry::Vec2`, the camera, movement and the protocol, not lighting —
  its own track if ever picked up, not scoped here.
