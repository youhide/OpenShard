# The lighting model

A specification, not a repair. Everything in
[`../archive/render/lighting_height.md`](../archive/render/lighting_height.md)'s
backlog was a compensation for missing data, and this model replaces the data
instead of the compensations.

The decision it rests on is stated once, at the top, because everything else
follows from it: **the art is albedo, and the light is ours.** UO's sprites are
drawn with light already in them, and every workaround below exists to avoid
arguing with that light. We stop avoiding it. The picture will not be "exactly
like UO", and that is the accepted price.

Stated once more in the form it was decided, because the difference is what makes
this a specification rather than another compensation: **the sprites are treated
as though they were already de-lit — perfectly clean albedo — and this renderer
is the ordinary one every other renderer is.** No invention of ours stands
between a sprite and a light. Where that assumption is false, it is false in the
picture, not in the code.

## The three roots

Not ten workarounds — three decisions, each with a family growing out of it.

**1. The art is pre-shaded, so a real BRDF was forbidden** *(retired at phase 3)*.
A Lambert term would be a second light fighting the painted one, so `light::faces`
is a half-space instead — and a half-space is a step, and a step has to be
softened, so `FACE_EDGE` is a band. That band is measured in *tiles along the
plane's normal* and `z` is divided by `Z_PER_TILE = 11`, which makes one constant
mean ±4 screen pixels across a wall and **±1.1 `z` above a lid** — more than half
a stair step. Measured 2026-08-08: with the flame between two treads, **7059
pixels** of a single flight sit inside that band against `3940` of genuine
penumbra, and the band's price peaks exactly where a flame lies in a surface's own
plane — `0.214` of a channel per pixel, against `0.020` half a step away.

**2. The `place` attachment packed a fragment's height into eight bits and a
four-bit fraction** *(retired at phase 2; the constants it justified went at
phase 4)*,
so a fragment's own position was not exactly known — so a
shadow ray must start away from where it really is. `STAND_OFF = 2/127` of a tile
and `ON_TOP = 1/128` of a `z` unit are **numbers taken from the byte layout**,
not from any statement about surfaces. Their price, measured with the light
oracle: the engine is brighter than the geometry allows on the top band of a
riser by up to **`0.51` of a channel**. And because heights cannot separate
surfaces at that precision, a whole apparatus grew to do it by identity instead
— `exemption`, `on_surface`, `own_run`, `mounted_at`'s height test. Phase 4 took
the bias to zero and dissolved `exemption`; two of that apparatus survived it and
the phase's own account says which, and why each was kept by a measurement.

**3. A static is drawn twice** — as a sprite, and as a mesh over it — so their
silhouettes differ, so the mesh is grown to hide the gap *(the border retired at
phase 6; the second draw goes with the rest of that phase)*.
`WIDTH_OVERLAP = 0.03` of a tile was a **1355-pixel border** around a single
flight at `4:1`, measured by zeroing it; and in a scene with no sprite it bought
nothing at all. It is the case `docs/style.md`'s *No fudge constants* was written
from.

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
| surface normal | G-buffer, `R32Uint`, octahedral (was to be `Rg16Snorm` — see phase 2) | unit vector |
| albedo | G-buffer, `Rgba8UnormSrgb` | linear after decode |
| primitive identity | G-buffer, `R32Uint` | opaque id |
| light accumulation | offscreen, `Rgba16Float` | linear radiance |
| screen | swapchain | tonemapped, sRGB |

**One metric.** `z` is divided by `Z_PER_TILE` **once**, where the map is read,
and never again. Nothing downstream knows that `z` was ever counted differently
from `x`. Half of `docs/archive/render/world_coordinates.md` is this line.

## What goes

Named, so the plan can be checked against the tree:

| goes | what replaces it |
|---|---|
| ~~`light::faces`, `FACE_EDGE`~~ **gone, phase 3** | `light::lit_from`, `max(N · L, 0)` off the G-buffer normal |
| ~~`STAND_OFF`, `ON_TOP`~~ **gone, phase 4** | exact position + self-hit by primitive id, bias `0` |
| ~~`exemption`~~ **gone, phase 4**; ~~`on_surface`, `own_run`~~ **gone, `occluders.md`'s S4** | the same id test, once — inline, in both walks. The other two were `same_run`, a *different* claim (see phase 4) that S4 deleted once every fixture named its own solid |
| ~~`mounted_at`'s height test~~ **gone, phase 4**; `mounted_at` and `MOUNTED_CLEARANCE` **stay, measured** | a sconce burns where it hangs, which the map does not say and the art does — see phase 4 |
| ~~`WIDTH_OVERLAP`~~ **gone, phase 6**; `Prism::mesh`'s `widen_footprint` went with it | one silhouette: a static is drawn once and its geometry met by the view ray — see phase 6 |
| ~~`FLAME_DEPTH`, `pierces`, `crosses`'s softening, `SOFT_CROSSING_*`~~ **gone, phase 5**; `inside` and the `spread` parameter went with them | an area light and eight shadow rays at `FLAME_RADIUS` |
| `(1 − d)²` falloff | windowed inverse square |
| `knee()` | a tonemap on HDR |
| ~~`place`'s `z + 128` · fraction · stance packing~~ **gone, phase 2** | position and normal as data; an id word for what is left |

`RAY_CUTOFF` and `MAX_WALK_STEPS` survive: a ray does have a cutoff and a walk
does have a step budget. They stop being *stand-ins* for the things above.

**Half of that is superseded by phase 6e.** `RAY_CUTOFF` survives it too — a ray
still has a cutoff — but `MAX_WALK_STEPS` bounds *cells stepped*, and
[`docs/render/design_occluders.md`](design_occluders.md) deletes the cell. Its replacement is a node
budget over the hierarchy, in the same role and for the same reason: a loop over
data must not become unbounded because somebody widened a radius. The sentence
above was written when the walk was a DDA and is kept as what it then meant.

**`FLAME_SPREAD` was to survive too — "a light does have a size" — and phase 5
found that it was not one.** Its `1.0` of a tile was the numerator of a penumbra
ratio, chosen for the edge it drew; the idea survived and the number did not.
`FLAME_RADIUS` is an eighth of a tile, off the art, and it is the size.

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
  channel — they are the acceptance instrument. The engine's frame and the
  tracer's are side by side as of phase 0: `boxes.rs` writes
  `<base>_lit_vs_traced.png`, three strips — ours, theirs, and the difference
  amplified `8×` so that an agreement is a black rectangle and a disagreement is
  not.

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

**The art is clean albedo by decree, and there is no dial.** The pre-shading in
the sprites is not compensated for, not softened against and not measured — it is
declared to be albedo, and every surface is lit by the same textbook Lambert any
other renderer would use. No stylised wrap, no half-space, no width knob between
the two, and no term anywhere whose job is to argue with the artist.

That knob was in this document until the decision, and it is worth recording what
it was so it is not reinvented: `light::faces` is
`clamp(along / FACE_EDGE + 0.5)`, and that shape — `N·L × k + 0.5` — is *wrapped
diffuse*, the ordinary stylised BRDF, so a width of `2.0` would have made it
half-Lambert and the width a dial between "pre-shaded look" and "Lambert". Gone.

**What survives from that reading is a diagnosis, and it says phase 3 is smaller
than it looks.** What `faces` is passed is not a cosine: `along` is
`dot(normal, toward)` with `toward` left **unnormalised**, so today's argument is
a *distance*. That single missing normalisation is where root 1's two scales come
from — one constant meaning ±4 screen pixels across a wall and ±1.1 `z` above a
lid. Phase 3 normalises the argument and takes the full `N·L`; it does not have a
band to retune, because there is no band.

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

A flame is a sphere of radius `FLAME_RADIUS`. `N` shadow rays per light towards
stratified points on it, `N = 8`. Sample positions rotated by a per-fragment
offset so the error is high-frequency rather than banded. No temporal accumulation
in the first pass; if `8` rays is too noisy or too slow, that is the moment to add
it, and not before.

This deletes the entire `pierces`/`crosses`-softening apparatus, whose band is
`soft × FLAME_DEPTH` with `FLAME_DEPTH = Z_PER_TILE/4` — a penumbra sized for a
wall's top edge three tiles away, applied to an edge a fifth of a tile away.

**Landed at phase 5, with two things this paragraph got wrong.** The radius is
`FLAME_RADIUS` and not `FLAME_SPREAD`, which was never a size — see that phase.
And the blue noise is *world*-space rather than per-pixel, because the CPU twin
has no pixel and a float hash is what two backends disagree about; the
hard-shadow debug view was not built, since the sun still casts one ray through
the same walk and is the hard-shadow case a frame already shows.

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

**Amended 2026-08-10: a pixel whose ray misses is not drawn at all.** The
paragraph above stands for everything except that last sentence, and what
retired it is the fringe it describes. A fragment clamped onto a box's edge takes
whichever face is nearest, and along a silhouette that is a *side* one — so the
overhang reads as a lattice of wall-shaded dots on every floor and roof, one per
tile, which is what a person kept reporting and what this document's floor entry
spent a session chasing. `statics.wesl` now discards a fragment whose ray met no
box, which is the clipping this section rejected.

The trade was measured before it was made rather than argued: at Britain's
`(1501, 1659)` with the roof cut, **4460 pixels of 187,086 static ones change —
2.38%** — and the two pictures are indistinguishable side by side, because a
fringe pixel is by construction one the volume does not describe. The cost is
real where a prism fits its art badly (`best_prism` scores a plain wall at
`0.81`), and it is stated in the shader beside the `discard` so the next person
weighing it has the number. What it buys is that every pixel left is a point *of*
the box it names — the property every plane downstream already assumed.

**And the census beside it says the cost is not spread evenly, which the frame
number cannot.** `client/render/examples/discard_census.rs`
(`docs/render/design_footprints.md`'s S4) walks every opaque pixel of every static in a window
rather than the ones a camera happens to show, so it is the upper bound of the
same phenomenon and it can say *whose* pixels they are. At Britain's `121×121`
around `(1501, 1659)`: **a lid loses 1.48% of its art, a fitted prism 0.35%,
panels 11.09% — and the whole-tile class 32.44%**, with the roofs inside it at
44–53% (`0x05A2` "slate roof" is 48×76 pixels of picture standing on a box three
`z` units tall). The discard is therefore mostly a measurement of *the height
nobody takes*, not of the impostor: a picture five times taller than the box
under it hangs over its own lid, and phase 6i's roofs are where that is worst.

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

## What is measured and what is invented, counted

`examples/geometry_census.rs`, over Britain — `121x121` tiles around
`(1501, 1659)`, 11,184 statics, 480 distinct graphics. The question it answers is
"what of this geometry is a claim about the art, and what is a stand-in":

| | |
|---:|---|
| **3.2%** | a fitted prism, one body a tread — the only box whose *shape* came out of the picture |
| **39.6%** | a lid: measured, but a plane with `min.z == max.z` |
| **25.4%** | panels on the edges the silhouette named, each inset by `PANEL_THICKNESS` |
| **0.2%** | a whole tile, a climbable the prism search could not fit |
| **31.6%** | **a whole tile, because the art would not say** |

So **68.2% is measured off the art in some way and 31.8% is a whole tile standing
in for a shape nobody has** — and the biggest single class in the world is a
*plane*, which is the degeneracy the floor entry in the backlog is about.

Crossed with the other axis: **32.7% of statics are a point of no primitive at
all** — `occlusion::opacity` reads them `CLEAR`, `Builder::add` pushes nothing,
`Occlusion::id_of` has no name — and **15.1% of the world is a `CLEAR` piece that
is nevertheless handed a box with real height** by `statics::push_volumes`. That
combination is the one this document's cornice and floor entries both end at: a
side face a fragment can be answered with, and no identity to excuse it by.

These are the numbers to re-run after anything on the backlog lands, and the
reason they are in the document rather than in a session note: every "how much of
this is a crutch" argument on this track has so far been made from memory.

## Accepted costs

- **The picture changes, and nothing in the renderer compensates.** Pre-shaded art
  multiplied by our light is double-contrast: a face already darkened by the
  artist and turned away from a flame goes darker than UO ever showed it. Exposure
  and ambient are ordinary exposure and ordinary ambient and they are all there
  is — neither is tuned *against* the art. If a scene still reads wrong, the
  answers are content (a shard ships better art) or de-lighting as a project of
  its own, and never a term in the BRDF.
- **Statics without a good prism** get a rougher volume, and their impostor normal
  is an approximation of an approximation. Visible on the odd tree and fence.
- **Cost.** Eight shadow rays a light a pixel is more work than one, and the
  lighting pass is already the expensive one. The phase that adds them measures
  it; if it does not fit, the answer is fewer rays plus temporal accumulation, not
  a return to an analytic fudge.

## Questions the model answered

Written down rather than guessed at, and closed:

- **Do statics need per-face albedo?** No — closed by the decree. A prism's four
  sides sample the same sprite through one projection, so a wall's two visible
  faces carry the art's own two shadings and we multiply both. Flattening them
  per face would be de-lighting through the back door, and the answer is the
  same as to de-lighting itself: not in this renderer. Whatever the sprite says
  is albedo.
- **Does the ground want normals at all?** Yes, and it was as close to free as
  the question hoped. UO's terrain is a height field with per-corner heights, so
  it has real normals, and `ground.wesl` writes the bilinear patch's own. The
  one-torch-on-open-ground pool is barely changed from the half-space's, because
  on level land the normal is `(0, 0, 1)` and a flame above it is nearly
  overhead. What the normal buys is the *slope*, which had no lighting at all
  before and now catches a flame the way the hill it is faces it.

The one question still open — how much a global exposure has to give back
against double contrast — is not a question about the model, and it is carried
in [`plans/render/lighting/PLAN.md`](../../plans/render/lighting/PLAN.md).

## Where the rest of this document went

This file used to carry the rebuild's phase journal and its backlog as well as
the model. They are records, not design, and they moved:

- how phases 0–8 were built, what each measured and what each got wrong —
  [`evidence/2026-08-11-lighting-rebuild-phases.md`](evidence/2026-08-11-lighting-rebuild-phases.md);
- every finding the rebuild turned up, with its number —
  [`evidence/2026-08-11-lighting-backlog-findings.md`](evidence/2026-08-11-lighting-backlog-findings.md);
- what the rebuild did to each of the seven documents it consolidated —
  [`../archive/render/README.md`](../archive/render/README.md);
- what is not built — [`plans/render/lighting/PLAN.md`](../../plans/render/lighting/PLAN.md).

The live status, the ranked queue and the open defects are
[`README.md`](README.md).
