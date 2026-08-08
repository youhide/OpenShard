# Lighting, rebuilt — the renderer this should have been

A specification, not a repair. Everything in `docs/lighting_height.md`'s backlog
is a compensation for missing data, and this replaces the data instead of the
compensations.

The decision it rests on is stated once, at the top, because everything else
follows from it: **the art is albedo, and the light is ours.** UO's sprites are
drawn with light already in them, and every workaround below exists to avoid
arguing with that light. We stop avoiding it. The picture will not be "exactly
like UO", and that is the accepted price.

## The three roots

Not ten workarounds — three decisions, each with a family growing out of it.

**1. The art is pre-shaded, so a real BRDF was forbidden.** A Lambert term would
be a second light fighting the painted one, so `light::faces` is a half-space
instead — and a half-space is a step, and a step has to be softened, so
`FACE_EDGE` is a band. That band is measured in *tiles along the plane's normal*
and `z` is divided by `Z_PER_TILE = 11`, which makes one constant mean ±4 screen
pixels across a wall and **±1.1 `z` above a lid** — more than half a stair step.
Measured 2026-08-08: with the flame between two treads, **7059 pixels** of a
single flight sit inside that band against `3940` of genuine penumbra, and the
band's price peaks exactly where a flame lies in a surface's own plane —
`0.214` of a channel per pixel, against `0.020` half a step away.

**2. The `place` attachment packs a fragment's height into eight bits and a
four-bit fraction**, so a fragment's own position is not exactly known — so a
shadow ray must start away from where it really is. `STAND_OFF = 2/127` of a tile
and `ON_TOP = 1/128` of a `z` unit are **numbers taken from the byte layout**,
not from any statement about surfaces. Their price, measured with the light
oracle: the engine is brighter than the geometry allows on the top band of a
riser by up to **`0.51` of a channel**. And because heights cannot separate
surfaces at that precision, a whole apparatus grew to do it by identity instead
— `exemption`, `on_surface`, `own_run`, `mounted_at`'s height test.

**3. A static is drawn twice** — as a sprite, and as a mesh over it — so their
silhouettes differ, so the mesh is grown to hide the gap. `WIDTH_OVERLAP = 0.03`
of a tile is a **1355-pixel border** around a single flight at `4:1`, measured by
zeroing it; and in a scene with no sprite it buys nothing at all.

## The target

Deferred shading, in the ordinary sense:

```
geometry pass ──► G-buffer ──► lighting pass ──► tonemap ──► screen
                  position      per light:        HDR → LDR
                  normal          BRDF × falloff
                  albedo          × shadow rays
                  ids
```

Every quantity lives in one place, in one unit, and is *data* rather than
something reconstructed downstream:

| quantity | where it lives | unit |
|---|---|---|
| fragment position | G-buffer, `Rgba32Float` | tiles, `z` **in tiles** |
| surface normal | G-buffer, `Rg16Snorm`, octahedral | unit vector |
| albedo | G-buffer, `Rgba8UnormSrgb` | linear after decode |
| primitive identity | G-buffer, `R32Uint` | opaque id |
| light accumulation | offscreen, `Rgba16Float` | linear radiance |
| screen | swapchain | tonemapped, sRGB |

**One metric.** `z` is divided by `Z_PER_TILE` **once**, where the map is read,
and never again. Nothing downstream knows that `z` was ever counted differently
from `x`. Half of `docs/world_coordinates.md` is this line.

## What goes

Named, so the plan can be checked against the tree:

| goes | what replaces it |
|---|---|
| `light::faces`, `FACE_EDGE` | `N · L` off the G-buffer normal |
| `STAND_OFF`, `ON_TOP` | exact position + self-hit by primitive id, bias `0` |
| `exemption`, `on_surface`, `own_run` | the same id test, once |
| `mounted_at`, `MOUNTED_CLEARANCE` | a sconce burns where it is; geometry decides the rest |
| `WIDTH_OVERLAP` | one silhouette — see the impostor phase |
| `FLAME_DEPTH`, `pierces`, `crosses`'s softening, `SOFT_CROSSING_*` | an area light and N shadow rays |
| `(1 − d)²` falloff | windowed inverse square |
| `knee()` | a tonemap on HDR |
| `place`'s `z + 128` · fraction · stance packing | position and normal as data |

`FLAME_SPREAD`, `RAY_CUTOFF` and `MAX_WALK_STEPS` survive: a light does have a
size, a ray does have a cutoff, and a walk does have a step budget. They stop
being *stand-ins* for the things above.

## How this is judged

**The instrument is a picture beside the path tracer's, looked at by a person.**
Not a number, and not a second implementation of our own arithmetic. Written down
here because it decides what a test in this crate is *for*, and because it retires
most of what the tree called a lighting test.

Twelve went on 2026-08-08 — nine `the_shader_…_agrees_with_light_sample` and the
three flat-face parity rungs, 1,172 lines, `tests/frame.rs` from 5,981 to 4,809.
The reason is one sentence: **their subject is the agreement of two of our own
implementations of the model phases 2–5 delete.** A sweep comparing `blit.wesl`
against `light::sample` cannot go red because the model is wrong — only because
the model is replaced, and both of its sides are being replaced. `assert_parity`,
`assert_parity_of`, `assert_single_face_parity`, `assert_two_face_edge_parity`,
`ring_of_lights` and `single_face_bounds` went with them.

Two of the twelve carried the `#[ignore]`d corner-tie tie-break, so the CPU/GPU
tie-break gap `lighting_raymarch.md` records is now recorded *only* in prose. It
was never going to be closed by a test that outlives the walk it is about.

What survives, and the rule that decides it — **does the test's subject survive
the rebuild?**

- **The brute-force oracles stay.** `tests/lighting.rs`'s `brute_force_blocked`
  and `frame.rs`'s `ground_truth_blocked` are dumb fixed-step point samplers
  against `solids_at`'s own boxes: no DDA, no `floor()`/`fract()` reconstruction,
  no shared arithmetic with either walk. Their subject is the occlusion grid and
  its boundary derivation, which phase 4 keeps. This is the one non-circular
  coverage in the tree and retiring it would be retiring a role, not an
  instrument.
- **World claims stay, as claims.** "A shut room keeps its light inside", "a hole
  in a floor lets the light through", "a torch does not light the storey above
  it" are statements about the world and survive the rebuild verbatim. Their
  *margins* do not: `> 0.2` and `< 1e-6` were calibrated against a pipeline that
  has already changed once under them — see `brighter_by`'s own account of what
  phase 1 did to every one of them. Expect to re-take them per phase, and expect
  that re-taking to be a judgement rather than a fix.
- **Pipeline mechanics stay untouched.** Blit texel-for-texel, the hue ramp,
  sprite silhouettes, depth order, the camera. None of it is about light.
- **Pictures are promoted.** `tests/pictures.rs`, `tests/traced.rs`,
  `dump_the_lighting_views`, `examples/synthetic_stair.rs` are no longer a side
  channel — they are the acceptance instrument, and the work they still need is
  to put the engine's frame and the tracer's *side by side* for one look.

## What arrives, in detail

### The G-buffer

Four attachments and a depth buffer. Position as `Rgba32Float` rather than
reconstructed from depth: the isometric projection is invertible and the
reconstruction is exact in principle, but it is also the single thing every
defect on the height track came from, and this plan does not re-earn that. (An
optimisation later, gated on a test that reconstruction equals the stored
position to `1e-5` of a tile, is welcome.)

`ids` carries what `place`'s `kind` and `id` carry today, because selection,
outlines and tooltips read them — plus a per-**primitive** index, which is the
thing shadow rays compare.

### The BRDF

`albedo × N·L × light colour × intensity × attenuation × shadow`, summed over
lights, plus `albedo × sky colour × sky visibility` for ambient.

**The formula is already there and its argument is wrong**, which is worth
knowing before phase 3 rewrites it. `light::faces` is
`clamp(along / FACE_EDGE + 0.5)`, and that shape — `N·L × k + 0.5` — is
*wrapped diffuse*, the ordinary stylised BRDF. What is passed to it is not a
cosine: `along` is `dot(normal, toward)` with `toward` left unnormalised, so it
is a **distance**. That single missing normalisation is where the two scales come
from, and it means phase 3 is smaller than it looks: normalise the argument, and
the band becomes angular and identical for every surface; set the width to `2.0`
and it *is* half-Lambert. The full `N·L` is then one more step, and the width is a
knob between "hard half-space, as today" and "Lambert", which is exactly the
dial between keeping the pre-shaded look and replacing it.

Lambert with no `1/π`, and the intensities calibrated to match: the constant is a
convention, and putting it in would mean re-tuning every flame in the tree for a
factor everyone then divides back out. Stated here so nobody re-derives it as a
bug.

### Attenuation

Windowed inverse square — the standard form, physical near the source and
finite at the rim:

```
let d2     = dot(L, L);              // squared distance, tiles
let window = saturate(1 - (d2 / (radius * radius))²);
let falloff = window * window / (d2 + 1);
```

The `+1` keeps a fragment at the flame's own position finite. `(1 − d)²` gave a
pool with a straight-edged gradient and no physical meaning; this gives the same
"soft pool with a hard end" the reference isometrics draw, and agrees with a path
tracer, which the old one cannot.

### Shadows

A ray per light per fragment, against a uniform grid of primitives — the
`occlusion` grid as it stands, with primitive ids added.

**Self-intersection is solved by identity, not by epsilon.** `if hit.primitive ==
origin.primitive { continue }` — the textbook answer, exact, with no tolerance
anywhere. A ray leaving a tread *does* cross its own flight's riser when the
geometry puts one in the way, and it *should*: that is a real occluder standing
in a real place. Every "my own static does not shadow me" rule goes.

Bias is `0`. If a grazing case ever needs one, it is `normal * k * pixel_scale`
where `pixel_scale` is `length(fwidth(world))` — a nudge in units of *how big
this pixel is*, which is the only honest unit for one.

### Soft shadows

A flame is a sphere of radius `FLAME_SPREAD`. `N` shadow rays per light towards
stratified points on it, `N = 8` by default and `1` for a hard-shadow debug view.
Sample positions from a per-pixel blue-noise offset so the error is high-frequency
rather than banded. No temporal accumulation in the first pass; if `8` rays is
too noisy or too slow, that is the moment to add it, and not before.

This deletes the entire `pierces`/`crosses`-softening apparatus, whose band is
`soft × FLAME_DEPTH` with `FLAME_DEPTH = Z_PER_TILE/4` — a penumbra sized for a
wall's top edge three tiles away, applied to an edge a fifth of a tile away.

### One silhouette: the sprite is the shape, the prism is the geometry

The mesh is *narrower* than the art (`best_prism`'s score is never exactly `1.0`),
which is why it was grown. Neither growing it nor clipping the sprite to it is
right: the art is the artist's statement of the shape and the prism is our
approximation of its volume.

So: **draw the sprite, and get position and normal by intersecting the view ray
with the prism analytically in the fragment shader.** Impostor rendering, the
ordinary kind. The silhouette is the sprite's, exactly as today; the depth and
the normal are the prism's; there is no second draw and therefore no seam and no
overlap constant. A pixel of the sprite whose ray misses the prism takes the
nearest point on it — the art overhangs its own volume by a pixel or two and that
is what it means.

### Billboards

A mobile is a sprite with no volume, and `N·L` needs a normal. Two candidates,
both cheap, and the phase picks by looking:

- **facing the camera** — a flat billboard lit as a plane turned towards the
  viewer. Never wrong, never interesting;
- **inflated from the silhouette** — the signed distance field of the sprite's
  alpha gives a gradient, and `normalize(vec3(∇sdf, k))` rounds the figure off.
  This is what 2D games with dynamic lighting do, it is computed once per art
  frame at load, and it makes a person standing beside a torch look like a person
  beside a torch.

Mobiles remain non-occluders. A billboard is no volume, so it casts nothing.

### Colour

sRGB in, linear throughout, tonemap out — the thing the current pipeline does not
do at all, and the reason it cannot agree with any reference renderer even in
principle. Art atlases and hue ramps are `…UnormSrgb` so the hardware decodes
them; accumulation is `Rgba16Float`; the final pass applies exposure and an ACES
fit.

Hue tinting is untouched: it indexes a 32-entry ramp by the art's own red channel
(`statics.wesl`'s `round(r * 31)`), and this plan never rewrites the art. What
changes is that the ramp's colour is decoded to linear before the light
multiplies it — which is what makes a tinted cloak in torchlight the same colour
as a tinted cloak in daylight, only dimmer.

## Phases

Each is landable alone and leaves the tree working.

**Phase 0 — the reference, and it must judge the same model.**
`crates/client/pathtrace` (in flight in a parallel session) becomes the oracle,
with a **BRDF switch**: it has to be able to compute what the engine computes, or
the choice of model is made by the choice of instrument rather than by us.
`synthetic_stair`'s light oracle (`write_light_reference`,
`write_light_difference`) is the comparison harness and already reports by class.
*Done when:* the path tracer and the engine agree on a scene with one flame and
no occluders, to within the frame's own quantisation — which is a statement about
falloff and colour handling alone, and is the calibration everything else rests
on.

**Phase 1 — linear and HDR.** *(Landed.)* sRGB decode, the multiplication in
linear radiance, exposure and an ACES curve, encoded once.
`shaders/tonemap.wesl` and `src/tonemap.rs` are the pair, and nothing else in the
crate may spell those curves again.

What it cost, which was not the shader: **every authored light value silently
changed meaning.** `NIGHT.sky = 0.20` was a fraction of a *displayed* value, and
`0.20` of radiance is an overcast afternoon — the first frame after the change
had no night in it at all. So every one of them is now `srgb_to_linear` of the
number a person chose, with the chosen number kept in
`the_authored_light_values_are_their_own_srgb_intent`: the artistic intent stays
written down beside its conversion, and a constant nudged by hand to make a
picture look right turns that test red instead of quietly redefining what "night"
was. `GROUND_AMBIENT`, `NIGHT`, `SKYLIGHT`, `TORCH`, `CAMPFIRE` and `midday`'s sun
all moved; the campfire's `1.25` is past sRGB's domain and carries the exponent
alone.

Three tests changed rather than broke, and each got stronger for it. The blit's
"copy, byte for byte" is now `tonemap::shade_u8` of the world texel — it catches a
blit that shifts by a texel *and* a colour pipeline that has drifted from its own
twin. The CPU/GPU parity sweep predicts through the same pipeline. And the pool
test's ratios are taken in **linear** light, because "twice as bright" was being
asserted about sRGB bytes, where it means nothing.

*Done when:* two equal flames are twice one flame in linear light
(`two_equal_lights_are_twice_one_in_linear_light`), and the picture baselines are
re-taken deliberately. **Both done.**

What phase 1 deliberately did *not* do: `Rgba16Float` accumulation. The whole
composition happens in one shader pass, in `f32` registers, so there is nothing to
store at intermediate precision yet — the moment a second pass appears (bloom, or
the glow layer), that is when the target format matters.

And what the pictures say, three ways, on `one-torch-on-open-ground` and
`a-shut-room-with-a-torch-in-it`: the old pipeline and the restated one put the
**night at the same level** — which is the whole claim of the restatement — while
the pool between them is wider, warmer and no longer burnt to a white core, since
the light now sums physically and the shoulder holds the top instead of a clamp
flattening it. The middle picture, linear light with the old numbers, is what
"the constants silently changed meaning" looks like: no night at all.

**Phase 2 — the G-buffer.** Position, normal, ids, albedo. `place`'s packing goes;
its readers (select, outline, tooltips, every oracle in `examples/`) move to `ids`.
*Done when:* a `View::Normal` shows the geometry's own normals, and a test asserts
the stored position equals the world position the mesh pass computed, exactly.

**Phase 3 — the BRDF.** `N·L` replaces `faces`. `FACE_EDGE` is deleted.
*Done when:* the light oracle's "inside FACE_EDGE" class no longer exists, and its
residual against the path tracer is quantisation only.

**Phase 4 — shadows by identity.** Primitive ids in the grid, self-hit by id,
bias `0`. `STAND_OFF`, `ON_TOP`, `exemption`, `on_surface`, `own_run` and
`mounted_at`'s height test are deleted.
*Done when:* the light oracle reports zero brighter-than-geometry pixels on the
whole flame-height sweep — the class that today reads 175 at `z 0`.

**Phase 5 — area lights.** N rays to a sphere. `FLAME_DEPTH`, `pierces` and
`crosses`'s softening are deleted.
*Done when:* the penumbra matches the path tracer's within sampling noise, and the
noise is measured rather than asserted away.

**Phase 6 — the impostor.** Sprite silhouette, analytic prism for depth and
normal, one draw. `WIDTH_OVERLAP` is deleted.
*Done when:* the difference frame's "drawn by one side only" classes are zero
except for rasteriser fill-rule dashes, against today's 1370.

**Phase 7 — billboards.** Normals for mobiles, chosen by looking at both.
*Done when:* a person standing beside a torch reads as lit from the torch's side,
in a frame a human being has looked at.

**Phase 8 — the sun.** A direction, the same BRDF, the same rays, sky visibility
as ambient occlusion.

## Accepted costs

- **The picture changes.** Pre-shaded art multiplied by our light is
  double-contrast: a face already darkened by the artist and turned away from a
  flame goes darker than UO ever showed it. This is the decision at the top of
  the document, and the exposure and ambient are the knobs that make it liveable.
- **Statics without a good prism** get a rougher volume, and their impostor normal
  is an approximation of an approximation. Visible on the odd tree and fence.
- **Cost.** Eight shadow rays a light a pixel is more work than one, and the
  lighting pass is already the expensive one. The phase that adds them measures
  it; if it does not fit, the answer is fewer rays plus temporal accumulation, not
  a return to an analytic fudge.

## Open questions

Written down rather than guessed at:

1. **How much does exposure have to give back?** Double contrast is a global
   effect and a global exposure may absorb most of it. Nobody has looked at a
   real frame with `N·L` on it yet, and that is a one-evening experiment inside
   phase 3.
2. **Do statics need per-face albedo?** A prism's four sides sample the same
   sprite through one projection, so a wall's two visible faces get the art's own
   two shadings — which are pre-shaded, and which we have just decided to
   multiply. It may look right anyway. It may need the art's shading flattened
   per face, which is de-lighting through the back door and would need its own
   decision.
3. **Does the ground want normals at all?** UO's terrain is a height field with
   per-corner heights, so it has real normals — and its art is nearly unshaded, so
   `N·L` on it is pure gain. Probably free, worth confirming early.

## The plans this consolidates

Seven documents describe how the current lighting was built, and a session that
starts by reading them starts by reading five thousand lines to find out which
paragraphs are still true. **This is the entry point now.** Each of them stays as
the record of how something was built and why — nothing is deleted, and the
reasoning in them is worth more than the code it justified — but the *live work*
is here, in one list.

| document | what it is | what happens to it |
|---|---|---|
| [`lighting.md`](lighting.md) | the current system, end to end: place attachment, occlusion grid, ray walk, sun, beams, doors, art measurement | **the thing being replaced.** Its mechanisms are retired phase by phase; its *content* work (below) survives untouched |
| [`lighting_world.md`](lighting_world.md) | ambient, the sky field, the day curve, tonal response | **mostly survives.** The sky field is ambient occlusion by another name and phase 8 adopts it; the day curve and the tonal response become phase 1's and phase 8's business |
| [`lighting_raymarch.md`](lighting_raymarch.md) | the DDA walk, CPU/GPU parity, the tile-boundary hazard | **survives as the walk.** Phase 4 changes what a hit *means* (identity, no bias), not how cells are stepped. Its corner-tie parity gap outlives the rebuild |
| [`lighting_geometry.md`](lighting_geometry.md) | box → mesh occluders, never started | **cheaper after phase 4**, which makes primitives addressable by id. The choice of primitive shape stays its own question |
| [`lighting_height.md`](lighting_height.md) | the height track: four landed phases and a long backlog | **the backlog is mostly deleted rather than fixed** — see the mapping below |
| [`lighting_reference.md`](lighting_reference.md) | the path tracer, a third opinion with no shared arithmetic | **becomes phase 0**, the oracle everything else is judged by |
| [`gbuffer.md`](gbuffer.md) | the `place` attachment's format, ids, per-face mesh geometry | **phase 2 replaces the format** and inherits every one of its readers. Its open question — how to encode a normal for a non-axis-aligned face — is answered there: octahedral, in the buffer |
| [`world_coordinates.md`](world_coordinates.md) | a position should carry its own cell; one metric | **half of it is phase 2** (positions as data, `z` in tiles once). The CPU-side type stays its own track |

### What each phase deletes from `lighting_height.md`'s backlog

So that backlog can be read as "work" rather than as a list of things that may or
may not still matter:

| backlog entry | fate |
|---|---|
| `FACE_EDGE`'s two scales; the flame at a surface's own height | **phase 3** — there is no band |
| `STAND_OFF`/`ON_TOP` at a grazing corner; the `ON_TOP` twin | **phase 4** — there is no nudge |
| risers excused as a group; `own_run`; `flame_end`'s height test; a mobile shadowed by its own wall | **phase 4** — identity answers all four |
| the `ground < 1e-6` shortcut ignoring a lid's footprint | **fixed** — it was worth fixing alone, and was |
| `WIDTH_OVERLAP`'s border | **phase 6** |
| the riser penumbra graded over a third of a face | **phase 5** |
| the wire's span rounding to nearest; the exact-tangent definition | **phase 4** — a primitive is not a byte range any more |
| `boxes.rs` reading `Unreached` as shadowed; `two_cubes.rs`'s old idiom; the projection idiom stated five times; `mesh::Face`/`facing::Face` colliding | **survive** — instrument work, still worth doing |
| `Occlusion::owner_at`'s linear scan; `selected`/`outlined` stamping `OwnerId::NONE` | **survive**, reshaped by phase 4's ids |
| `tests/cost.rs` measuring three planes of five; `plan::Wall::top` as an `i32`; hand-copies of the third channel | **survive**, and phase 2 is when they get corrected |

### Carried over: work no phase here deletes

Gathered from every document above, because these are the things that would
otherwise be lost between plans. None of it blocks the rebuild; all of it is
still wanted.

**Content and features**
- The day curve — until it lands, a default frame carries no ambient split at
  all and a house reads as bright as the street (`lighting_world.md`).
- Light carried by mobiles other than the local player; a serial-derived flicker
  phase (`lighting_world.md`).
- The screen-space glow for a flame's own halo, and the sunbeam shaft through a
  window (`lighting.md`).
- Doors, the ported open/shut occlusion table — built, and untouched by any of
  this.
- Land as an occluder: a hill casts no shadow today (`lighting.md`).
- Leaded/lattice window apertures, refused rather than measured; the aperture
  channel of the field is reserved and always zero (`lighting.md`,
  `lighting_world.md`).
- `Builder::add` consuming an authored `Blocks` list — the table format supports
  arches and lintels, nothing wires one into the live grid (`lighting.md`).
- Night Sight's interaction with a real day curve is undecided
  (`lighting_world.md`).
- A mobile as a soft sub-tile occluder; a body's diagonal footprint the
  axis-aligned `Solid` cannot state (`lighting.md`, `lighting_world.md`).

**Known gaps that outlive the rebuild**
- The corner-tie CPU/GPU parity gap, with two `#[ignore]`d tests
  (`lighting_raymarch.md`). Phase 4 does not touch stepping, so it stays.
- Nothing runs the tracer over a real map — all three scenes are hand-built
  boxes (`lighting_reference.md`). **Phase 0's own work**, along with the
  brightness calibration that phase's "done when" actually asks for.
- The tracer is single-threaded, 13 s a frame — too slow for a sweep, and a
  sweep is how the last three defects were found (`lighting_reference.md`).
- Buffer capacity is one flat `INITIAL_QUADS = 4096` for all kinds, and the
  widest real frame reallocates on its first frame, every run (`gbuffer.md`).
- A climbable the prism-fit cannot decompose still occludes as a whole-tile
  body (`gbuffer.md`).
- A courtyard overhang can make the sky-column test misread a tile; 28 of 2,560
  outdoor tiles in Britain read dark (`lighting_world.md`).

## Backlog

Things noticed while writing this, not blocking any phase:

- `docs/lighting_height.md`'s backlog does not disappear — most of its entries
  are *deleted* by a phase here rather than fixed, and each should be marked with
  which phase kills it rather than left reading as work.
- ~~The `ground < 1e-6` shortcut (both walks and the shader) is a real defect
  today and becomes moot at phase 4; if phase 4 slips, it is worth fixing
  alone.~~ **Fixed.** All three copies gate on the lid's own footprint now, by
  the horizontal half of `ray_vs_solid`'s parallel-axis rule — `light::
  over_footprint` and `blit.wesl`'s twin. Only the horizontal half, because a
  vertical ray's height answer is `crosses`'s soft one and `ray_vs_solid` would
  answer it hard, erasing the penumbra.
- **There is no lit-against-lit picture, and three separate things stop one being
  drawn.** The tool writes the engine's shaded frame (`<base>_lit.ppm`) and the
  tracer's (`<base>_pathtrace_full.ppm`) as two files, and the only thing it puts
  *side by side* is a pair of shadow masks. Laying the two shaded pictures beside
  each other today would show a difference for three reasons that are not about
  light: the tracer's albedos are written down in `oracle::pathtrace::Mirror::of`
  (`[0.72, 0.70, 0.66]` for a body, `[0.42, 0.44, 0.40]` for the ground) and are
  not the engine's art; its flame is `intensity: 6.0` against the engine's `1.0`;
  and it has no ambient where the engine has `NIGHT`. A fourth, underneath them:
  `mesh_face.wesl` writes only the `place` attachment, so a box's *face* has no
  albedo in the world texture at all — there is nothing on the engine's side to
  compare a body's colour against yet.

  This is phase 0's "done when" restated as a picture, and it is the thing to fix
  before anybody judges a phase by looking. The smallest honest first scene is the
  one that phase already names: **one flame, flat ground, no occluders**, where
  the albedo is the same ground art on both sides and what is left to differ is
  falloff, intensity and colour handling alone. The two encodes must also pass
  through **one** curve — `tonemap::tonemap` then `linear_to_srgb` — and
  `pathtrace_comparison`'s hand-rolled sRGB is a second spelling of the second
  half of that, which phase 1's own rule forbids.
- `examples/two_cubes.rs` still projects world points without asking whose pixel
  it got. Phase 2 moves every other reader to `ids`; this one should go with them.
- **The parity harness could not see a sub-tile lid, and still barely can.** The
  shader's copy of the shortcut above was fixed and forty-seven frame tests
  stayed green with the fix deleted again: no parity scene had a solid narrower
  than its tile, so the branch was never run. It has one now, and `Fixture` can
  state an *owner* — without which a fragment on a tread is shadowed by the step
  it stands on and every finer question about a flight is unreachable. What is
  still true is that this is one scene and one pixel of it: the vertical case
  needs the flame exactly over a swept fragment, so one flame buys one comparison.
  A sweep that varied the flame across the tile would buy the whole strip.
- ~~**Parity is circular for any defect both walks share.**~~ **Acted on.** It
  compared the shader against `light::sample`, so a rule wrong in the same way on
  both sides reported agreement — and the whole family is now deleted, see *How
  this is judged*. What is left of that test is its *direct* half:
  `the_shader_does_not_stop_a_vertical_ray_with_a_lid_it_is_not_under` reads two
  of the frame's own pixels and no longer calls a sweep at all. It is the direct
  claim that fires when the shader's gate is removed, and it was always the only
  half that could.
- **The parity apparatus was built on `place`'s packing, which is why it could not
  have survived phase 2 anyway.** `parity_frame` writes the attachment by hand,
  texel by texel, in the exact `z + 128 | stance` layout — so it is a second
  author of the format the G-buffer replaces. `parity_place`, `parity_frame` and
  `Fixture` are kept for now because three surviving tests draw a frame through
  them; phase 2 rewrites all three against the G-buffer or deletes them.
- The three-tread flight is rebuilt by hand in five tests in `light.rs` and now a
  sixth in `frame.rs`, each restating the same `Prism::new(Face::North, &[1, 3,
  5])` and the same tile bounds. It is the scene every stair defect is found on
  and it should be one constructor.
- `renderer.rs`'s `depth_state()` has lost its doc comment: `PLACE_TARGET` was
  inserted between the comment and the function, so the whole explanation of the
  depth test and its `LessEqual` tie-break now reads as documentation of the place
  attachment, and the function itself has none.
