# The reference path tracer

> **This is [`lighting_rebuild.md`](../design_model.md)'s phase 0** — the oracle the rebuild is judged by.
> The **BRDF switch** that file asks for is built: `trace::Brdf` computes either
> physics or the engine's own model, and the gate now runs in the engine's.
> What phase 0 still owes is the *calibration* — one flame, no occluders,
> brightness against brightness rather than shadow against shadow.


A second renderer of the same scene, written to have none of the first one's
ideas. Where `light.rs` and `blit.wesl` decide a shadow by walking a grid of
tiles — carrying a cell, stepping to a boundary, asking an occluder whether it
is the fragment's own — this one has a ray, a box, and where they meet. No
tile appears in it anywhere.

That is the whole claim: **a defect that can only be stated in the walk's own
vocabulary cannot be reproduced here by construction**, rather than by
coverage. It is also a *third party*. When `light::sample` and `blit.wesl`
disagree, both are implementations of one formula and neither can say which is
right; this one is not in that argument.

Written against `crates/client/pathtrace/` (the whole crate) and
`crates/client/render/examples/boxes.rs`'s own `pathtrace_comparison`.

Satellite of [`lighting.md`](../../archive/render/lighting.md), beside
[`lighting_raymarch.md`](../../archive/render/lighting_raymarch.md) — that file is about keeping the
CPU and GPU copies of one walk numerically identical, this one is about having
something that is not either of them.

## Where it lives, and why it is a crate

`crates/client/pathtrace`, **with no dependencies at all** — not on
`openshard-client-render`, not on a shared geometry helper, not on a shared
constant. The arrow only points the other way: the render crate takes it as a
dev-dependency for `examples/boxes.rs`.

That direction is the design. A reference that can reach the thing it checks
will eventually share an answer with it, and the sharing is invisible in the
result — two pictures that agree because they computed the same thing twice
look exactly like two pictures that agree because both are right.

Being a crate rather than another module under `examples/` also buys the one
thing a reference cannot do without: `cargo test --workspace` runs its own
tests. There are 46 of them, and they are what says the reference is not
itself the defect — worked crossings by hand, two proptest laws over the slab
arithmetic, the camera recovery against a projection with `f32` noise in it,
and the estimator on scenes whose answer is known on paper.

## What it takes from the renderer, in full

Two things, and both arrive as **values rather than as formulas**.

**The camera.** `camera::Parallel::measure` takes the world-to-pixel map as a
black box — a closure `boxes.rs` builds out of the renderer's own
`project_exact` and `Camera` — probes it along three axes, recovers the affine
map, and then *checks that assumption* on four probe points it did not measure
from, failing loudly if the map is not affine to within a hundredth of a pixel.
The view direction falls out as the null space of the recovered 2×3 matrix, one
cross product, no linear solve.

Two consequences worth having written down:

- Nothing in the tracer knows a tile is 44 pixels across or that a `z` unit
  lifts a sprite four. A change to the projection reaches the reference camera
  automatically, because the reference camera is *measured* from it every run.
- The recovered kernel of OpenShard's own projection is `(1, 1, 11)`, which is
  `Z_PER_TILE` — arrived at from the picture rather than from the constant.
  The probe measures through an `f32` path (`Camera::to_view_exact` narrows
  there) so the central difference is taken over a 32-tile baseline, which
  divides that noise by 64.

**The metric.** `light::Z_PER_TILE`, read from the engine rather than written
down again, is what converts the world into the isotropic units the tracer's
scene is in. Visibility would survive getting this wrong — it is invariant
under any affine change of coordinates — which is exactly why it has to be
right anyway: a cosine, a solid angle and a penumbra's width are not, so a
scale error would show up in the soft modes as a plausible-looking picture
rather than as a failure.

Everything else — the boxes, the flame, the frame size — the caller states.

## The two models

`trace::Brdf` decides which renderer's light the tracer is computing, and the
switch exists because **an oracle that can compute only one model decides the
model**. Ask a physical tracer whether the engine is right and every pixel of
every surface turned away from a flame comes back as a disagreement — a true
statement about two light models and a useless one about anybody's geometry,
because it says the same thing whatever the shadow walk does.

- **`Lambert`** — `albedo / π`, the receiving surface's own cosine, nothing on
  the side it does not face. Physics, and what this crate did before there was
  a choice.
- **`Flat`** — what the engine computes: the arriving irradiance times the
  albedo, with **no cosine, no `1/π`, and no normal anywhere in it**. UO's art
  is pre-shaded, so the shipped renderer multiplies that picture by a light and
  never asks which way a surface points.

Three things follow from `Flat`, and they are one variant rather than three
flags because they are one fact — there is no normal. No cosine and no `1/π`;
no back-face test; and **a surface point's own body does not occlude it**. That
third one is not a separate policy smuggled in: without a normal there is no lit
side, so a point on a body's far side would shadow itself against that same body
and the model would have no answer at all. The engine states it as an exemption
in its own walk (a fragment's own occluder does not stop the fragment's ray) and
[`lighting_rebuild.md`](../design_model.md)'s phase 4 restates it as identity.
`Scene::blocked`'s `except` is where it lands, and its own tests hold the
exemption to one body: an exemption that let the whole scene through would pass
every back-face test and quietly produce a reference with no shadows in it.

`Flat` is refused a bounce. Indirect light off a surface with no normal is a
third model, and calling it the second one is how a reference stops being one.

## The two modes

**Degenerate** is the gate. A point emitter, one path per pixel, no bounces:
every random draw is either never made or has one possible outcome, so the
Monte Carlo estimator collapses to a single deterministic visibility test and
the picture is exact. `Image::is_exact()` says so by asking the emitters and
the settings, not by trusting the constructor, and `boxes.rs` asserts on it —
a soft-shadow render disagreeing with a hard-shadow one is not a finding, and a
comparison that cannot tell the two apart would report it as one.

**Full** is a picture, not a check. A spherical emitter, hundreds of paths, a
cosine term, diffuse bounces and a sky. It is *not* compared against anything:
none of what it adds exists in the renderer, so every pixel would "disagree".
It is there to be looked at — 512 samples and 3 bounces over a 512×512 frame is
about 13 seconds, single-threaded.

Both are one body of code. A reference with a separate "fast exact path" is two
implementations again, and the one that gets compared against the renderer
would be the one nobody looks at.

## How a pixel is compared

Only where the two agree about **what surface is there**. The renderer's answer
comes from the `place` attachment (which instance row drew this pixel); the
tracer's is its own nearest hit. Where they differ, the pixel is counted under
`disagree about which surface is there` and no further — an isometric painter's
order and a ray's own nearest hit are two different answers to "what is in
front", and filing that under lighting would name the wrong defect.

Three further splits, each of which exists because folding it in would have
produced a large, confident, wrong number:

- **Out of reach is not shadowed.** A pixel outside a torch's radius is dark
  because of the *radius*. `Visibility::within_reach` carries it separately, as
  the renderer's own debug view already spends a colour doing.
- **Facing away is not shadowed.** A surface whose normal points away from the
  flame is dark because of where it *points*. Under `Brdf::Flat` those pixels
  are ordinary pixels of the comparison — the model being computed has no
  cosine either — while `Visibility::faces_light` still reports the *geometric*
  fact, so one render answers both "does the engine's own model agree here" and
  "which pixels does the choice of model decide". Before the switch existed
  they were held out of the comparison entirely, and it was five to eleven
  thousand pixels a scene smaller for it.
- **An edge is not an interior.** The two renderers answer about *different
  points* — the tracer about the world point under a pixel's centre, the shader
  about the fragment the rasteriser wrote, quantised to a
  hundred-and-twenty-eighth of a tile. Half a pixel decides the answer exactly
  at a boundary and nowhere else, so a disagreement is only reported when
  neither picture has an edge running through the pixel's own eight-
  neighbourhood. **One function over both maps**, because the argument is not
  about what is being compared: it is as true of "which surface is there" along
  a silhouette as of "is this lit" along a shadow's edge, and the silhouette
  split below is what it bought.

And it counts what it checked. `compared` is asserted non-trivial: a detector
that silently compares nothing reads exactly like a detector that found
nothing.

## What it says today

Run over the three scenes `boxes.rs` builds, at their own defaults, **in the
engine's own light model** (`Brdf::Flat`):

| scene | compared | interior | edge | different surface | of those, on a silhouette |
|---|---|---|---|---|---|
| `tree` | 262,085 | **0** | 188 | 59 | 59 |
| `line` | 261,311 | **0** | 105 | 833 | 831 |
| `pair` | 262,144 | **0** | 190 | 0 | 0 |

**Zero interior disagreements on all three.** Every pixel where the two
renderers agree about which surface is there, and where neither has a shadow
edge running through its neighbourhood, they agree about whether the flame
reaches it. Against a renderer that shares no arithmetic and has no notion of a
tile.

And now including the back-facing pixels — five to eleven thousand a scene that
used to be held out, because the tracer could not state the model that lights
them. Those pixels are the ones the shadow walk's own exemption decides, so
they are exactly the pixels a comparison most wants and least had.

That is a much stronger statement than the existing oracles could make. It also
bounds the open residuals in `lighting_raymarch.md` and `lighting_height.md`:
whatever is left of them lives inside one pixel of a shadow's own edge, or in
the categories below, and not in the interior of any lit or shadowed region.

### The one real difference: the model itself

**The renderer lights surfaces that face away from the flame.** There is no
cosine term and no back-face test in the shipped model, so a box's south face
with the torch to the north is drawn lit:

| scene | back-facing pixels | of which the frame draws lit | pixels the choice of model decides |
|---|---|---|---|
| `tree` | 5,259 | 4,878 | 4,894 |
| `line` | 10,934 | 6,432 | 6,417 |
| `pair` | 5,352 | 2,700 | 2,700 |

The third column is the switch earning its keep: both models are rendered, and
their two answers subtracted, so the size of the decision is *measured* rather
than inferred from a back-face count. It is asked of every compared pixel and
not only of the back-facing ones — "the model decides exactly the pixels facing
away" is the expectation, and an expectation only ever measured where it already
holds is not one a detector can report on. (The two numbers differ by the
handful of pixels on a shadow's own edge, where the two models' hard edges fall
a pixel apart.)

This is a difference between the two **light models**, not a bug report: UO's
own art has no normals, and a face's brightness in the client comes from the
sprite. Whether the mesh-face path — which *does* have a normal, and states it
in `Stance` — should use it is a design question this file raises and does not
answer; `lighting_rebuild.md`'s phase 3 is where it is answered, and the third
column above is what that phase will move. It is recorded here because the
number is large, because it was invisible to every oracle before this one, and
because `docs/lighting_height.md`'s own recent "the oracle had no half-space
test, and most residuals were that" is the same fact arrived at from the other
side.

### `line`'s 833, explained

The one scene-dependent number nobody had looked at. Split by the same
neighbourhood test the lit comparison uses, **831 of the 833 sit within one
pixel of a silhouette** — the boundary where half a pixel decides which of two
surfaces a ray meets, and where the two renderers are known to answer about
points half a pixel apart. It is the rim of the two boxes, one pixel wide, and
its size is a fact about how much silhouette a scene has: `line`'s two
whole-tile boxes at zoom 4:1 have some seven hundred pixels of it, `tree`'s
small stacked boxes fifty-nine, and `pair`'s two third-tile posts none at all.
The shared silhouette edge the backlog guessed at is in there — 79 pixels of
"the frame draws box 1's lid, the tracer sees box 0" along the seam — but it is
a tenth of the number, not the number.

The remaining **two** are at `(343, 429)` and `(344, 429)`, three pixels above
the bottom corner of box 1, where the box's two silhouette edges converge: the
rim is three pixels wide there, so a one-pixel neighbourhood cannot see out of
it. Not a second phenomenon — the same one, at the one place the detector's own
radius is too small. What would settle it rather than argue it is the backlog's
sub-pixel entry: render the tracer at a higher resolution and downsample, which
replaces the whole neighbourhood heuristic with a bound.

Nothing was hiding in it. The number is now reported split, so a future scene
that grows an *interior* surface disagreement says so on its own line instead of
adding to a total nobody can read.

### And one the tracer found in itself

Worth recording because it is a trap anyone building a reference will meet.
The first version used the textbook area-light estimator — sample the emitter's
surface, weight by its own cosine and `1/d²`, divide by the sampling density.
That estimator is exactly right, and it is exactly right *only with a physical
falloff*: its near-field behaviour is a cancellation between an emitter cosine
going to zero and a `1/d²` growing without bound.

The renderer's falloff is `(1 - d/reach)²`, which does not grow. Put the two
together and only the collapse survives — a wide emitter near the floor drew a
**dark patch directly beneath itself**, exactly where it should be brightest.

The fix is to separate the emitter's two roles: brightness is point-source
photometry from the emitter's centre through whichever curve the caller chose,
and the emitter's *extent* is used for one thing only — where to aim the shadow
ray, which is what a penumbra is made of. `Light::sample`'s own doc carries the
argument. The picture is what found it; no test would have, because no test
knew to ask about a spot on the floor under the torch.

## Running it

```sh
# The gate, as a test. Skips itself where there is no GPU adapter.
cargo test -p openshard-client-render --test traced -- --nocapture
```

```sh
# The same gate as a picture. On by default, every run of the tool.
OPENSHARD_FRAME_DUMP=/tmp/tree OPENSHARD_BOXES_SCENE=tree \
    cargo run --release -p openshard-client-render --example boxes
```

Writes `<path>_pathtrace.ppm` — the frame's own shadow decision and the
tracer's, side by side, grey where a pixel was not compared. `OPENSHARD_BOXES_PATHTRACE=0`
skips it.

```sh
# The picture. Off by default; the sample count is the switch.
OPENSHARD_BOXES_PATHTRACE_SAMPLES=512 OPENSHARD_BOXES_PATHTRACE_BOUNCES=3 \
OPENSHARD_BOXES_PATHTRACE_EMITTER=0.5 OPENSHARD_BOXES_PATHTRACE_EXPOSURE=3.0 \
    cargo run --release -p openshard-client-render --example boxes
```

Writes `<path>_pathtrace_full.ppm`. `_EMITTER` is the emitter's radius in tiles
and **`0` is the point emitter** — hard shadows with the cosine, the bounces and
the sky still in, which is the one picture neither mode had: degenerate mode is a
bit a pixel, and full mode was always soft. `_EXPOSURE` is a plain linear
multiplier before the sRGB curve.

## Status

Built and current:

- `crates/client/pathtrace`, zero dependencies, 52 tests under
  `cargo test --workspace`.
- The camera is measured from the renderer's own projection and asserts its own
  affine assumption; nothing restates the projection's formula.
- **The BRDF switch**: `Brdf::Lambert` is physics, `Brdf::Flat` is the engine's
  own model down to its self-occlusion exemption, and the gate runs in the
  second. Both are rendered every run and subtracted, so the size of the model
  decision is a measured number.
- Degenerate mode is a gate, runs by default in `examples/boxes.rs`, and reads
  zero interior disagreements on `tree`, `line` and `pair`.
- **The gate is in `cargo test`**: `crates/client/render/tests/traced.rs` builds
  the `line` scene through the real `GroundRenderer`/`MeshFaceRenderer`/`Blit`
  pipeline offscreen and asserts on it, skipping where there is no GPU adapter.
  It reproduces the tool's numbers exactly and runs in about half a second. The
  judging is shared with the tool rather than copied — one implementation of
  what counts as a disagreement.
- Surface disagreements are split on the silhouette, by the same neighbourhood
  test the lit ones use, and only the interior ones are named.
- Full mode renders soft shadows, a cosine term, indirect light and ambient
  occlusion over the same scene and camera.

## Backlog

- **The gate compares one scene, not three.** `tests/traced.rs` runs `line`;
  `tree` and `pair` are still the tool's alone. Each is a second GPU frame, so
  the cost is real but small — the question is whether the gate should own the
  reference scene's own defaults, which would make editing one of the tool's
  knobs a test failure rather than a silent retirement of a recorded number.
- **The gate and the tool build their scene twice.** The *judging* is one
  implementation (`examples/oracle/pathtrace.rs`, shared by `#[path]`), which is
  the part where a defect could hide. The pipeline around it — occlusion grid,
  camera, mesh rows, ground quads, blit, readback — is written once in
  `examples/boxes.rs`'s own `main` and again in the test, as every GPU fixture
  in this crate does. It cannot make a disagreement disappear, only fail to
  produce one, which is what the test's three non-triviality assertions are for.
  Lifting it into `examples/oracle/` would take the tool's `main` apart, and its
  env knobs and dumps are half of what is in there.
- **Nothing runs it over a real map.** All three scenes are hand-built boxes.
  A static off `tiledata` is a sprite with a `Solid` approximation behind it,
  and the tracer would be checking the approximation rather than what a player
  sees — worth doing anyway, but the limit has to be stated in the same breath
  or a green tracer will be read as a green shard.
- **Phase 0's calibration is not done.** The switch can compute the engine's
  model; nothing yet compares the two as *brightness*. `lighting_rebuild.md`'s
  own "done when" is one flame, no occluders, the tracer's radiance against the
  frame's own pixel to within its quantisation — which is a statement about
  falloff and colour handling alone, and is what everything else rests on. The
  comparison here is still a bit a pixel: lit or not.
- **The cosine question above** wants an answer, not just a count — but it now
  has a measured price (the third column of the model table) rather than a
  guess. A mesh face carries its own normal already; the decision is phase 3's.
- **Single-threaded.** 13 seconds for 512 samples over a 512×512 frame is fine
  for looking at a picture and too slow for a sweep. The pixel loop is
  embarrassingly parallel and each pixel's stream is addressed by its own
  index, so it is already deterministic under any evaluation order — but that
  is a dependency (`rayon`) on a crate whose whole design is having none.
- **The edge exclusion is a neighbourhood test, not a bound.** It correctly
  refuses to report sub-pixel disagreements, and it would also hide a genuine
  one-pixel-wide defect that happened to sit along a shadow edge. It is also
  blind *inside* a rim thicker than one pixel — the two unexplained pixels at
  `line`'s bottom corner are exactly that, and are the cheapest evidence that
  the radius, not the idea, is what is wrong. Rendering the tracer at a higher
  resolution and downsampling would replace the heuristic with a real sub-pixel
  answer.
- **No sun.** `Lighting::sun` is a directional light the renderer supports and
  the tracer has no counterpart for, so a sunlit scene cannot be compared at
  all today.
