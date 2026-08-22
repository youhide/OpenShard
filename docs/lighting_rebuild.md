# Lighting, rebuilt — the renderer this should have been

A specification, not a repair. Everything in `docs/lighting_height.md`'s backlog
is a compensation for missing data, and this replaces the data instead of the
compensations.

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
from `x`. Half of `docs/world_coordinates.md` is this line.

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
[`docs/occluders.md`](occluders.md) deletes the cell. Its replacement is a node
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
(`docs/footprints.md`'s S4) walks every opaque pixel of every static in a window
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

## Phases

Each is landable alone and leaves the tree working.

*Where the rebuild stands, as of 2026-08-09.* The table is a **pointer, not the
record** — each phase's own paragraphs below carry what it did, what it measured
and what it got wrong, and a claim here that disagrees with one of them is this
table being stale.

| | Phase | State | What is left in it |
|---|---|---|---|
| 0 | the reference | ✅ done | — the tracer over a **real map**, which is a carried item rather than this phase |
| 1 | linear and HDR | ✅ landed | — |
| 2 | the G-buffer | ✅ position, normal, ids, albedo | — |
| 3 | the BRDF | ✅ landed | — |
| 4 | shadows by identity | ✅ landed | — |
| 5 | area lights | ✅ landed | — |
| 5b | a flame has no centre | ✅ landed | — |
| 6 | the impostor | 🚧 6a, 6c, 6d, 6f and 6g landed | a corner's two panels' **ids** still told apart by the screen half — the *stance* is the met box's since 6g, and only the row number is left; and the phase's own second number — how far a real static's art overhangs its prism, which is the **fringe** along a sprite's silhouette — still untaken |
| 6e | the grid stops being a rule | ✅ landed [`occluders.md`](occluders.md) | **All six steps are green.** The grid is out of the walk on both backends, and S3b's merge folds a run of wall into one primitive — 73 pieces to 9 on the crate's own two-storey house, with no pixel moved. That document is a **record** now, and the four findings that outlive it — the aperture still measured in a tile, the instruments that could not see a merge (closed since — one of them was drawing it), `PANEL_THICKNESS`'s fattening the merge turned out **not** to answer, and `footprint`'s `i32` ranges — are in this document's backlog |
| 7 | billboards | 🚧 position and the camera-facing normal landed | a mobile pass in a picture harness, the inflated-silhouette candidate, and the choice between them — its *done when* is a person looking at a lit frame |
| 8 | the sun | ⬜ not started | all of it |

**Where a session starts, as of 6i's gates landing on 2026-08-10:** phase 6e is
closed and there is no live sub-plan under this document any more —
`occluders.md` is a record like the seven above it. **6d is closed, and 6f, 6g
and 6h are the bill it ran up** — three defects in a row, each reported by a
person looking at a lit frame and none caught by anything under `cargo test`: the
sprite path naming the wrong tread of a flight, then carrying a corner panel's
stance across a tread, then being met against a face buried inside a merged
solid. [Phase 6i](#) is the account of why nothing caught them and the gates that
would have; three of its four items are in, and **the one left is its item 1 — a
fixture that drives `statics::collect` over a fitted climbable**, which is the
only way any instrument in this tree sees the path a staircase actually takes.
Its entry point is written under the item. Read 6f's account beside it: it is a
worked example of removing a pass by what it *computed* rather than by what it
*delivered*. **Phase 7 is half-open,
its own account is above, and what it is waiting on is now named rather than
generic**: `examples/isolated_scene.rs` needs a mobile pass before there is a
picture of a figure beside a torch to look at, the inflated-silhouette
candidate has not been started, and the choice between the two is what the
phase's own *done when* is. Beside it, **phase 8** is untouched, and three
defects a person has seen and nobody has fixed — a flame's own sprite reads
black, a sprite's top edge is serrated where a missed ray takes a nearest face,
and a whole-tile body writing a camera-facing normal is what darkened statics at
6c. All three are one question about what a *body* should write for a normal,
they are in the backlog with their measurements, and they are the ones that
decide whether a lit frame reads right. ⚠ **Unrelated to any of this:** the
working tree also carries a large, uncommitted, in-flight change to the gump,
paperdoll and text-shaping code (`crates/client/app/src/{gump,lib,shell}.rs`,
`crates/client/render/src/{gump,paperdoll,text}.rs` and their tests) from a
parallel session — it currently leaves `openshard-client-app` (and therefore
`openshard-playground`) failing to build. It is not this document's concern and
this session did not touch it, but it is why a real client could not be used to
look at phase 7's picture and had to be named as a blocker instead.

**Phase 0 — the reference, and it must judge the same model.**
`crates/client/pathtrace` (in flight in a parallel session) becomes the oracle,
with a **BRDF switch**: it has to be able to compute what the engine computes, or
the choice of model is made by the choice of instrument rather than by us.
`synthetic_stair`'s light oracle (`write_light_reference`,
`write_light_difference`) is the comparison harness and already reports by class.
*Done when:* the path tracer and the engine agree on a scene with one flame and
no occluders, to within the frame's own quantisation — which is a statement about
falloff and colour handling alone, and is the calibration everything else rests
on. **Done.** The scene is `boxes.rs`'s `flat`, the gate is
`the_frame_and_the_path_tracer_agree_about_brightness_on_open_ground`, and the
measurement is **262,144 pixels compared, worst channel one step of 255** —
257,972 of them identical and the remaining 4,172 exactly one step apart. The
tolerance is `2` and the residual is `1`, so it is a quantisation rather than a
margin sized to fit; at `0` the gate goes red, which is how that was checked
rather than argued.

*What had to become true first*, and each was a difference that is not about
light:

- **the albedo is the same on both sides.** `oracle::ground_albedo` reads it off
  the world texture the ground pass drew and decodes it, so it is a measurement
  and not two authors writing the same constant down. `Mirror::of`'s
  `[0.42, 0.44, 0.40]` is now `Albedos::INVENTED` — still the value where a
  comparison does not read colour, but a call site has to *say* so.
- **the flame is the same flame.** `Light`'s own colour and intensity travel to
  the reference through `Mirrored`; the tracer's own `intensity: 6.0` was picked
  to make its own picture readable and made every shaded comparison meaningless.
- **one curve.** `tonemap::encode` is the radiance-only half of `shade` —
  `linear_to_srgb(tonemap(x))` — and both pictures go through it.
  `pathtrace_comparison`'s hand-rolled sRGB, with a `clamp` where the shoulder
  is, was a second spelling of it and phase 1's own rule forbids one.
- **the ambient is nothing, deliberately.** A degenerate path trace is direct
  light and has no ambient term, so `NIGHT` would be a constant on one side of
  the comparison only — and not one that could be subtracted back out, since the
  sum passes through a tonemap. Giving the tracer an ambient instead would put
  this renderer's own model inside the thing that checks it.

The scene has no boxes for the fourth reason the backlog named: `mesh_face.wesl`
writes no colour, so a box's face has nothing on the engine's side to compare a
body albedo against. That is phase 6's, and `Albedos::body` stays invented until
then.

*And it found a defect in the instrument on its first run.* Both pixel oracles in
`boxes.rs` read `Shade::lit()`, which answers `false` for a fragment **outside
every flame's radius** as well as for a shadowed one — and compared it against
`oracle_visible`, which is pure geometry and knows nothing of a torch's range.
`Shade` exists to make exactly that distinction available and its own doc says a
caller that must not count it has to match on the variant. Every scene until now
had its flame reaching the whole canvas, so the conflation never fired; `flat` at
1:1 reported **67,728 of 262,144 ground pixels "rendered too dark"**, every one
of them simply out of reach. Both oracles now skip `Shade::Unreached` and report
how many they skipped.

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
**Both done** — position and normal below, and the id plane after them retired
the `place` attachment outright. What is left of the phase is albedo, which is
phase 6's: a mesh face has none.

*Position landed.* `crates/client/render/src/gbuffer.rs` is the set — a `Gbuffer`
owning the planes and a `Views` lending them, so the two still to come are one
edit each and not thirty. The plane is `Rgba32Float`, written by all three world
passes and read by `blit.wesl` as `at`; `unpack_place_z`, the seven-bit fraction
decode and the whole `tile + sub` reconstruction are gone from that shader.
`a_mesh_face_pixel_carries_its_exact_world_position` is the phase's own "done
when", half of it: the mesh pass is the producer whose vertices carry true world
positions, and the test picks a point at `15.1` above a tile at `0.3, 0.7` —
a height no sixteenth and a fraction no hundred-and-twenty-seventh can hold — so
that it fails if anything on the path quantises. It asserts the packed height
beside it, to compare the two rather than merely have both.

Three things it deliberately did not do. **`z` stays in `z` units:** the
occlusion grid, every solid's span and the whole walk are stated in them, and a
G-buffer that alone counted in tiles would be a second metric rather than one.
**The tile stays a row lookup:** it is what the walk starts cell stepping from,
and `floor`ing a position back into it is the class of bug `walk`'s own comment
records. **The position is clamped into its tile** exactly where `pack_place`
clamps the fraction, so this step changed precision and nothing else; the clamp
went at phase 4 — not because the cell stopped being a separate fact, which it
has not, but because nothing floors that position and eight thousandths of a tile
of error in a ray's origin is the largest thing left once the bias is zero.

*Normal landed.* The plane, `View::Normal`, and — the thing worth saying first —
**a normal is written by the pass that knows it now, not derived by the pass that
reads it.** `blit.wesl`'s `outward(stance)` is gone from the lighting entirely:
`statics.wesl` writes `outward` of the stance it has *just* resolved a corner
into, `mesh_face.wesl` carries `mesh::Face::normal` on its own vertices —
measured geometry, the one producer whose normal was never a stance — and
`ground.wesl` writes a zero outright. That last one closes a `select` on the kind
that had been sitting in the reader: land and a wall's flat cap are one stance
and only one of them wants the half-space gate, and the pass that knows which it
is drawing is the one that says so now. `Stance::normal` is the Rust twin,
`Stance::of_normal`'s inverse, and the two round-trip in a test.
`two_mesh_faces_carry_their_own_two_normals` is the phase's other half of "done
when": a tread's top and its riser, one draw, two normals — and the place
attachment asserted beside them holding `MeshFace` for **both**, which is the
measure of it. The attachment cannot tell those two surfaces apart. The plane
can.

Two things it did not do the way this document said. **The format is
`Rgba32Float` and not `Rg16Snorm`, octahedral.** Every 16-bit norm format is
behind `wgpu::Features::TEXTURE_FORMAT_16BIT_NORM`, which is native-only and not
in WebGPU's core set — so the row in the table above was never available. The
nearest compact renderable format, `Rgba16Float`, is not taken either: the
hand-written producers (`plan.rs`'s diagnostic pictures, `tests/`' fixtures)
*write* this plane from the CPU and there is no `f16` on that side, so it would
mean a hand-rolled encoder — a second spelling of a float format with no compiler
comparing the two. Octahedral has a second problem of its own besides: it has no
zero, and **the zero vector is a value here** — a billboard has no side, and
phases 6 and 7 are the work of leaving less of that in a frame, not of pretending
it is absent today.

And **the client asked an adapter for more than WebGPU's guaranteed minimum for
the length of this phase.** A world pass writes the picture and every plane in
one draw, and `maxColorAttachmentBytesPerSample` bounds the total: the floor is
32 and the set was already at exactly 32 before this — picture 8, `place` 8,
position 16 — so *no* fourth plane fitted, in any format.
`gbuffer::required_limits` is the one place that asks,
`attachment_bytes_per_sample` sums the real per-format table rather than the
widths a person reads off the names, and `a_g_buffer_costs_what_it_says` pins
the total. The cost was stated plainly rather than absorbed while it stood: a
device reporting only the minimum could not run this client. Both later steps of
this phase gave it back — the id plane four (48 → 44) and the packed normal
plane twelve (44 → 32) — so the assertion now reads the other way, and the
target layout phase 6 has to hit is that same 32 with no separate picture beside
the albedo.

*The id plane landed, and it is where the attachment ended.* `place`'s eight
bytes a fragment were an id, a height in whole units and sixteenths, a stance
and seven bits of tile-local `x` and `y`. The position plane had already taken
the height and the fraction and the normal plane the facing the stance stood in
for, so what was left was **six bits and an id** — `gbuffer::pack_ids`, one
`R32Uint`, kind in the low two bits, stance in four above it, the row in the
twenty-six above that. `crate::place` keeps `Kind`, `Stance` and `Place` — the
vocabulary and the *instance row*'s own two words — and carries no attachment
format at all; `packed_height`, `unpacked_height`, `Z_FRAC_*`, `SUB_TILE`,
`STANCE_SHIFT`, `FORMAT`, `texture` and `CLEAR` are gone, along with
`place_format.wesl`'s `pack_place` and `unpack_place_z`.

**The kind is at the bottom of the word on purpose.** The clear value is zero
and `Kind::Nothing` is zero, so a pixel nothing drew and a pixel a pass stamped
as nothing are the same number — which is the invariant every reader's first
branch rests on, and the one thing a layout can quietly break.
`nothing_drawn_and_nothing_cleared_are_one_kind` and
`an_id_word_holds_three_things_and_gives_all_three_back` are the two halves of
it.

**It bought a third of the budget back, which is why it went next.**
`ATTACHMENT_BYTES_PER_SAMPLE` was 44 against 48, and the twelve still over
WebGPU's floor of 32 were the normal plane's — packed next, below, which is what
brought the total to 32 exactly. And the stance survived the move, so the phase
did not retire it: `blit.wesl` still reads it to route a mesh face's id to its
own instance buffer and to ask the shadow walk's own-run test which edge a
fragment stands on. **Phase 4 is what retires the second**; the first goes when
a mesh face stops being a pass of its own.

Two things it changed that are not the format. `parity_place`'s sub-tile
fraction is an `f32` rather than sixteen-of-a-hundred-and-twenty-seven, kept at
the same grain so that no parity margin moved for a reason that is not under
test. And `View::Place`'s checkerboard is drawn from the **tile** now: it was
taken from the two halves of the *id*, so a frame's squares counted instance
rows rather than tiles, and it went unnoticed because a diagnostic is read for
whether a gradient is there and both versions have one.

*And the normal plane was packed, which is what put the whole set under
WebGPU's floor.* The plane was `Rgba32Float` — sixteen bytes a fragment for a
unit vector and a coverage bit — and those twelve extra bytes were, after the id
plane, the entire remainder of what this client asked an adapter for above the
guaranteed 32. `ATTACHMENT_BYTES_PER_SAMPLE` is **32 exactly** now, so
`a_g_buffer_costs_what_it_says` asserts `<= floor` where it used to assert
`> floor`, and the sentence "a device reporting only the minimum cannot run this
client" is retired rather than softened.

**It went before phase 6 because it is a term of phase 6's own sum.** That
phase's target layout is position `16` + normal + albedo `8` + ids `4` with no
separate picture beside them — which comes to 32 only if the normal is 4. It was
never a tidiness item.

Four decisions, and the second is the one that had to be made rather than
looked up:

- **`R32Uint`, octahedral, integers on both sides.** `Rg16Snorm` is behind
  `TEXTURE_FORMAT_16BIT_NORM`, native-only; `Rgba16Float` is renderable and was
  refused for the reason phase 2 refused it — this plane is *written from the
  CPU* by `plan.rs` and by two fixtures, there is no `f16` there, and a
  hand-rolled encoder is a second spelling of a format with no compiler
  comparing the two. An integer word has neither problem and turns the encoding
  into the thing this crate already keeps honest.
- **The two non-vectors stayed in the plane, in two bits of their own.** A
  fragment nothing drew and a fragment with no facing are different answers, and
  the four-float plane separated them with its fourth channel. Fifteen bits an
  axis leaves two over, so `NORMAL_DRAWN` and `NORMAL_FACING` carry that split at
  no cost — rather than being inferred from the id word beside it (`KIND_NOTHING`
  and `STANCE_UPRIGHT` do name the same two states today). The plane still means
  something read alone, which is how `View::Normal` and every test that copies it
  back read it.
- **The span is even.** Each axis quantises to `32766` steps and not to the
  `32767` its bits allow, so that zero lands on a code instead of half a step off
  one. Nearly every normal this renderer writes is cardinal — every wall face,
  every lid, every level tile — and an odd span moves all of them by a
  ten-thousandth for nothing. With an even one, all six round-trip bit-for-bit,
  and the sweep's worst over the whole sphere is **`0.0068°`**, against a `0.01°`
  bound taken from what a channel can show rather than from the mapping.
- **The gate is an integer against an integer, not a tolerance.**
  `two_mesh_faces_carry_their_own_two_normals` renders a face and asserts the
  word the GPU wrote equals `gbuffer::pack_normal`'s. `normal_format.wesl` and
  its Rust twin are two spellings no compiler compares, and this is the only
  thing that does — fault-injected by moving the span on one side alone, which
  turns it red. The test grew a **third** face, a slope off every axis, for
  exactly that: the two cardinal ones go through the packing's exact cases and
  would survive a fold spelled differently.

*And one thing the first version of that sweep got wrong, which is worth
keeping.* It measured the angle as `acos` of a dot product and reported the
packing losing `0.028°` — four times the truth. Near zero, `acos`'s derivative
is infinite, so a dot carrying `f32`'s own `1e-7` comes back as `sqrt(2e-7)`,
which is `0.026°`: the number being read was the *instrument's* noise floor and
nothing of the packing was visible under it. The chord is well conditioned —
subtracting two nearby `f32`s is exact — and `2·asin(|a − b| / 2)` is the same
angle.

Left: albedo for a mesh face, which has none — phase 6.

**Phase 3 — the BRDF.** `N·L` replaces `faces`. `FACE_EDGE` is deleted.
*Done when:* the light oracle's "inside FACE_EDGE" class no longer exists, and its
residual against the path tracer is quantisation only. **Both done.**
`light::lit_from` and `blit.wesl`'s twin are `max(N · L, 0)` — `clamp`, one
`normalize`, no constant of any kind between them — and the class the difference
picture spent a colour on is gone from the code rather than reading zero.

*The change was one line and the argument to it.* `dot(normal, toward)` divided
by a width became `dot(normal, normalize(toward))`, and every consequence in this
phase follows from that division: the term stops being a distance in tiles, so it
stops needing a width to be measured against, so `FACE_EDGE` has nothing left to
be. `MOUNTED_CLEARANCE` was `0.5 + FACE_EDGE` and is a plain `0.7` — the same
number on purpose, so that phase 3 moved the picture through the shading term and
through nothing else. **Phase 4 did not delete it** — see that phase for the
measurement that kept it.

*The reference had to be asked a different question, and that is what says the
term is right.* `Brdf::Flat` is a description of the engine **before** this phase
— no cosine, no `1/π`, no notion of a normal — so a brightness gate against it
would have judged us against the renderer we had just replaced.
`the_frame_and_the_path_tracer_agree_about_brightness_on_open_ground` renders
`Brdf::Lambert` now, and the two conventions meet in one place: the reference's
flame carries `oracle::pathtrace::LAMBERT_PI`, because our Lambert has no `1/π`
and physics does. **262,144 pixels compared, 23,564 bright and 238,580 dim, worst
channel one step of 255, nothing past the two-step quantisation.** The engine's
cosine and a path tracer's are the same cosine, measured rather than argued.

The *visibility* comparison beside it stays in `Brdf::Flat`, and the split is
worth stating because it looks like an inconsistency and is not: that variant's
three clauses are one fact — there is no normal — and the third of them, "a
surface point's own body does not occlude it", is still exactly what the shipped
walk does. Phase 4 is what turns that into identity and is where the visibility
gate moves too.

*The scene had to move as well, twice, and each time because a cosine made a
degenerate configuration visible.* A flame at `z: 0.0` is **in** the ground's own
plane, where the cosine is zero everywhere and no pool exists at all;
`light::gather` never builds one there — it adds `FLAME_LIFT` to every light —
so two frame tests were writing "on the ground" and meaning "where a fire on the
ground burns". `FLAME_LIFT` is `pub` now and they say the second. And the
brightness gate's flame went from three `z` to a whole tile up, because a source
a quarter of a tile over flat ground grazes it: the frame had 812 bright pixels
against the ten thousand the gate needs before it is measuring a curve rather
than its tail.

Three world claims were re-taken, and the rule from *How this is judged* held —
each was a judgement about the scene rather than a margin nudged to fit:

- **the pool test and the wall test** got the lift above. The wall test's radius
  went from four tiles to six besides: the far tile was still *inside* the pool
  and no longer said anything there a byte could hold, so the walled and open
  frames read alike and the test would have passed by measuring nothing.
- **the wall-run seam test** asserted a floor of `0.2` on the face beside the
  lamp. A lamp standing *along* a wall grazes it, so the whole face went dimmer
  without the claim under test changing at all. It is a *range* now — the east
  end at least twice the west — which is what "lit from one end" says and what a
  level never did.

*And the ground has normals*, which answers open question 3. `ground.wesl` writes
the bilinear patch's own — the cross product of the two tangents of the surface
its vertices are already lifted to, with the corner heights divided into tiles in
the vertex stage so the fragment stage needs no `viewport` binding for it. A flat
tile's four heights are equal, both derivatives are zero and the answer is exactly
`(0, 0, 1)`, arrived at rather than special-cased. The deliberate zero it replaced
was a defect of the half-space and not of the normal: a floor is the one surface a
flame is routinely almost in the plane of, and gating it blacked out every ground
pixel a fixture was not comfortably above. A cosine is *small* at a grazing flame
rather than absent, which is what a floor lit by a torch standing on it looks
like.

What it costs, and it is the phase's own finding rather than a surprise: **a
surface a flame grazes goes markedly darker, and walls are what a lamp grazes.**
On `a-wall-run-with-a-lamp-along-it`'s elevation the face is plainly dimmer than
the half-space drew it and the gradient is tighter, while `one-torch-on-open-
ground`'s pool is barely changed — which is the shape open question 3 predicted
for land and open question 1 is still about. Nothing here compensates for it and
nothing here should: exposure and ambient are ordinary exposure and ordinary
ambient, and neither has been touched yet.

**Phase 4 — shadows by identity.** Primitive ids in the grid, self-hit by id,
bias `0`. *Done when:* the light oracle reports zero brighter-than-geometry
pixels on the whole flame-height sweep. **Done.** The sweep read
`31 / 15 / 13 / 0 / 0 / 0 / 0` at flame heights `0..6` when the phase started and
read **zero at every one of them**, worst channel `0.000`. (The `175 at z 0`
this line used to quote was measured before phase 3; the cosine had already taken
most of it.)

*Re-read after phase 6 reshaped a tread*, which is the first thing the sweep is
for: `0 / 2 / 0 / 0 / 0 / 0 / 0`, worst channel `0.022` — **two pixels at `z 1`
alone**, one brighter and one darker, and both at a place the reshaping created.
They are in the backlog with their addresses. The face oracle is `0` of `10,824`
at every height, so it is not a disagreement about visibility.

*The rule is one comparison, and every arm of the apparatus it replaced was a
proxy for a name a fragment did not have.*

```
if hit.primitive == origin.primitive { continue }
```

Three readings in order, because each failure says what the next had to be. **A
height inside a span** — two things stacked on one tile meet at a single plane, so
no precision separates them, and two side by side span the same heights outright,
so each was excused from the other while standing squarely in front of it
(`examples/boxes.rs`'s `pair`, three oracles fully red). **An `OwnerId`** — the
*static*, `lighting_height.md`'s own phase 3, right for a wall and one level too
coarse for a flight: one `Builder::add` pushes a lid and a panel per tread, all
wearing one owner, so a tread was excused from the riser that genuinely stands
between it and the flame, and the height came back as `drawn_on` to patch it.
**A `SolidId`** — the primitive itself. A flight's treads shadow each other
because they are different solids, which is what different solids do.

*What the fragment carries it in, and the split is the part worth keeping.* A
mesh face is one primitive by construction, so `MeshFaceRow` carries its
`SolidId` outright; the join is `occlusion::Part`, the `n`th solid one
`Builder::add` pushed, and the `n`th face of `Prism::mesh` is that solid because
both walk the same treads from `treads()` and `up()`.
`a_flight_draws_its_own_solids_in_the_grid_s_own_order` holds that against the
geometry for all four climb directions rather than leaving it as two loops that
agree. A **sprite** instance is not one primitive — a corner is two panels and one
picture, and only a fragment's own stance says which — so `blit.wesl`'s
`own_solid` narrows the instance's owner by that stance, once per fragment. It is
exact for everything but a fitted climbable, whose pixels the mesh pass draws.
(**That last clause is what 6d falsified and 6f repaired** — a sprite fragment
carries its solid outright now, off the box the impostor met. See 6f.)

*The bias is zero, and the two constants had already lost both of their reasons.*
`STAND_OFF` was `2/127` of a tile and `ON_TOP` `1/128` of a `z` — numbers off the
retired attachment's byte layout. One thing they bought was a ray not starting
inside the surface it was drawn on, which is identity's job. The other was a face
pixel walked from *in front of* its plane, because the attachment placed it
behind one and because a crossing could be found on the wrong cell; phase 2's
exact position and `lighting_raymarch.md`'s per-solid `ray_vs_solid` removed both.
`mesh_face.wesl`'s `INSIDE` clamp on the position it writes went with them — eight
thousandths of a tile of error in the ray's origin on exactly a flight's outer
corner, which is where every stair defect is found.

*Three of the plan's deletions did not happen, and each was settled by injecting
the fault rather than by reading the code.*

- **`own_run` stays**, as `same_run` with its height gate folded in. ⚠ **This
  reading did not survive**: `docs/occluders.md`'s S4 deleted `same_run` outright,
  and what was wrong with the measurement below is that three fixtures asked the
  walk about a fragment that named no solid, so `on_the_lit_surface` — which reads
  the fragment's own box — could never fire and the cell arithmetic was the only
  thing left standing. What follows is what was measured then, kept whole.
  Identity cannot answer it: a run of wall is *N different statics* cut on tile
  boundaries, so the panel next along the run is a different solid however
  exactly a fragment names its own. Neutralised, `light_runs_along_a_wall_and_
  stops_across_it` and `the_two_faces_of_a_corner_are_lit_from_the_side_each_
  looks_at` go red. Restricting it to *neighbouring* cells — leaving the
  fragment's own cell to identity, which reads like the tidier rule — turns the
  same two red. What retires it is the grid merging a run of coplanar panels into
  one solid: `lighting_geometry.md`'s question, not this phase's.
- **`on_surface` stays** as that gate's own test, and is exact now: its `ON_TOP`
  tolerance was the nudge handed back, and both went together.
- **`mounted_at` and `MOUNTED_CLEARANCE` stay.** "A sconce burns where it is"
  means, in practice, a flame at its tile's *centre* — behind the plane of the
  face it is bolted to, where the cosine is zero along the whole face, so every
  wall carrying one would come out black top to bottom. It is not a compensation
  for a missing rule but the client's reading of where a wall-mounted static
  hangs, which the map does not say. Neutralised, `a_sconce_lights_the_street_
  and_not_the_room_behind_it` and the wall-run test go red. What would retire it
  honestly is the *art*: the sprite shows the sconce standing out from the wall,
  and nothing measures that.

*What the phase's own text meant by "`mounted_at`'s height test" is `flame_end`*,
and that **is** deleted: `skip_last && cell == last && on_surface(to_z, …)`
excused a panel on the cell a flame *ends* in. `mounted_at` moving the flame onto
the next tile is what made it unnecessary — neutralised, the suite stayed green
and the oracle stayed at zero on every flame height. `skip_last`, both walks'
`last`, `ExemptionContext` and `Exemption` went with it. What it covered and
nothing now does: a flame standing inside a whole-tile body, a lantern in a
tree's box — which is a wrong box rather than a rule the walk owes it.

*And the identity compare itself was fault-injected*, because nothing else would
have said whether it is load-bearing. Forced to `false`, three tests go red: the
flight fixture, `the_face_of_a_wall_is_lit_from_inside_the_room` and
`a_carried_light_lights_the_way_it_is_pointed`. The last two are also the only
place the `None` half of it is measured — a flat fragment's own solid is a lid,
and `crosses`'s strictness already answers a ray leaving a plane exactly; a face
fragment's own solid is a panel, and `same_run` masked its own cell's side
whatever the fragment carried — that rule is gone with S4, and `on_the_lit_surface`
in its place answers nothing for a fragment with no box either.

*Three world claims were re-taken, and the rule from *How this is judged* held —
each was a judgement about the scene.* Two were the same graze: **a flame exactly
level with a tread**, whose riser stops at exactly the tread's height, so the ray
runs along the riser's top edge and a flame of real depth is half cut by it —
`0.5`, exactly, where the nudge had made it `1.0`. Both flames are `FLAME_LIFT`
above the tread now, which is where a torch burns. The third is **the floor
line**: a wall pixel at exactly a storey's floor height, which now names the wall
it is a point of instead of leaning on two constants to be lifted above the
boards. Above the line it is dark a sixteenth of a `z` up; *at* the line it is a
graze, recorded as a range rather than dropped — one mathematical plane, not the
four screen pixels the original defect was.

*What it cost, measured:* 88 pixels of a tread's outer corner read shadowed where
the face oracle's point-source geometry says lit — the same coplanar-edge graze,
at the line where a tread's lid meets its riser's plane. Both walks agree about
them; it is the engine's area light against a point source, and phase 5 is where
those become comparable. Against 473 "rendered too light" before the phase.

**That last sentence was wrong, and phase 5 is what measured it.** Making the
oracle an area light left all 88 exactly where they were. What they are is a ray
touching the riser's box at `t = 0` — the fragment stands *in* that box's own top
plane, so no interval separates them and identity cannot excuse a different
primitive. Phase 5's own account has the rule that closed them; the number is `0`
of 11,469 now.

**Phase 5 — area lights.** N rays to a sphere. `FLAME_DEPTH`, `pierces` and
`crosses`'s softening are deleted.
*Done when:* the penumbra matches the path tracer's within sampling noise, and the
noise is measured rather than asserted away. **Done.** The gate is
`the_frame_and_the_path_tracer_agree_about_every_interior_pixel`'s second half:
**11,896 pixels partly lit on both sides, the frame's penumbra `+0.0070` of a
flame from the reference's on average against the `0.025` a model difference would
have to clear, and `0.0676` of mean absolute difference against the `0.0995` half
a ray of eight plus the reference's own measured noise allows.** The noise is the
reference rendered twice under two seeds: worst `0.3125`, mean `0.0547`.

*The four constants were one apparatus, and the size in them was not a size.*
`FLAME_SPREAD` was `1.0` of a tile, `SOFT_CROSSING_MIN`/`MAX` bounded the
`t / (1 - t)` ratio it multiplied, and `FLAME_DEPTH` converted the width that
produced into a height because every edge softened vertically is horizontal. That
is the textbook penumbra formula with the source's own size in it — and the size
was a tile because a tile drew an edge somebody liked, which made the flame a
pancake: a tile across and a quarter of a tile tall. `FLAME_RADIUS` is **an eighth
of a tile**, and it is the one measurement in the pile that was ever taken from
the art — `FLAME_DEPTH`'s own, a torch's drawn flame at eight or ten screen pixels
and four pixels to a `z`, which is exactly twice this as a diameter. `pierces`,
`inside`, `crosses`'s band and the `spread` parameter every walk threaded went
with them; `hole` is a rectangle and `pierced` is what is left of a panel after
one.

*What the pictures cost, and it is the whole visible change:* **shadows are about
eight times crisper.** On `torch_before_a_wall`, the band up the wall's top edge
went from about eight `z` to one, measured by the same sweep that asserts it —
`a_ray_grazing_the_top_of_a_wall_is_dimmed_rather_than_switched` steps an eighth
of a `z` now to have anything to look at. That is not a regression: an eighth of a
tile is what the flame is, and the old width was the number that made the picture
rather than the number in it.

*The sampling, and the one place two backends had to be made to agree.* Eight rays
at a Vogel spiral on the disc the sphere presents to the fragment — the
silhouette, which is what `pathtrace::Emitter::Sphere` samples too, `sqrt` of the
index for equal area, laid out in tile space and multiplied back into `z`. Only
visibility is sampled; the falloff and the cosine stay at the flame's centre,
where at an eighth of a tile they move by well under a byte. The pattern is
rotated per fragment, or eight rays are eight visible bands — and the rotation is
**an integer hash of the world position quantised to a hundred-and-twenty-eighth
of a tile**, because the obvious thing (a hash of the pixel) cannot be spelled on
the CPU side at all and the usual `fract(sin(dot(…)))` is the arithmetic two
backends are least likely to agree about. Being stable in the *world* is worth
having on its own: a panning camera does not make a penumbra crawl.

What it costs, and the rotation is what buys it: **a sweep is monotone only to
within one ray.** Two neighbouring points of a sweep are two different rotations of
the same eight directions, so `a_ray_grazing_the_top_of_a_wall_…` allows
`1 / SHADOW_RAYS` of slack and says so as the ray count rather than as a number.

*The cost, measured on Britain at 4:1 by holding the frame still and moving the
ray count alone* — `tests/cost.rs`, seven flames, two million pixels:

| | one ray | eight rays |
|---|---|---|
| `night` (the seven flames) | 1.276 ms | 2.818 ms |
| above the `dark` floor | 0.255 ms | 1.819 ms |
| `sun` (still one ray, and unmoved) | 1.344 ms | 1.347 ms |

Seven times the work for eight times the rays, and 2.8 ms of a 16.7 ms frame. The
sun's row is the control: it casts one ray either way and does not move, which is
what says the measurement is about the rays and not about the day's weather.

*Three oracles were asked about a point and had to be asked about the sphere.*
None of them is the walk, and each was reporting the difference between a point
source and a body as the renderer's:

- **the brute-force fuzz** (`tests/lighting.rs`) shrank to a case where the centre
  ray clips a wall tile's far corner and most of the eight miss it. It asks
  `light::flame_points` where the rays end now — the *scene* shared, not the
  answer; its own dumb 0.001-tile stepper is untouched. It then found a corner
  clip under a thousandth of a tile deep on its first run, which is the second
  time that step has been defeated by a fixture, so it is five times tighter
  again and the file still runs in a second.
- **`boxes.rs`'s box-top and face oracles**, and **`synthetic_stair.rs`'s**: a
  share of the flame rather than a bit. The stair's *light* reference multiplies
  by that share instead of gating on it, which is what the engine does.

*And the 88 pixels phase 4 left on the table are gone — for a reason that phase
named wrongly.* Phase 4 recorded 88 pixels of a tread's outer corner drawn
shadowed where the face oracle said lit, and wrote that they were "the engine's
area light against a point source, and phase 5 is where those become comparable."
They are not. Making the oracle an area light left all 88 exactly where they were,
which is what says the cause is not the light's body — and the report named it
outright: a fragment at `(100.01, 101.00, z 1.0)` stopped by *a panel spanning
`z 0.00..1.00` on its own cell*, which is the riser directly under the tread's
lip. A riser is a plane on the climb axis and a tread's lid stops exactly at it,
so a fragment on that lip stands in the riser's own plane at exactly the riser's
top, and every ray it sends touches that box at `t = 0` and nowhere else. Identity
cannot excuse it: the riser is genuinely a different primitive.

What closes it is one line in each walk and in the shader, and it is `crosses`'s
own strictness said about a box instead of a plane:

```
if entered == 0.0 && leaves == 0.0 { continue; }
```

**A ray that only touches a solid at the point it starts from has not gone through
it.** No epsilon — both ends are exact numbers off the slab test, and the rule is
narrow by construction: a ray that starts *inside* a box leaves it at some `t > 0`
and a lid the ray genuinely crosses is found at the `t` of its own plane, so
neither is touched. The face oracle reads `0 of 11,469` after it, against 88
before, and the light oracle stays at zero on every flame height.

**Phase 5b — a flame has no centre.** *(Landed.)* Every term of a flame's
contribution is a function of the *sample point*: visibility, the cosine, the
falloff and the beam. `light.at` stops appearing in the shading loop at all.

*Why it is a phase and not a backlog line.* Phase 5 gave the flame a body for
one term and left it a point for the others, and the backlog entry below has the
pictures: a lamp lower than `FLAME_RADIUS` above a floor puts half its sphere
below that floor's own plane, those rays are traced, and near a join they leave
the fragment's own primitive and come back "blocked" — a **wedge** of shadow on a
surface that is flush and continuous. The cure is the physical form and it is
exact rather than a mitigation, because the set of rays a join can block and the
set of rays below the horizon are *the same set*.

So one number replaces two:

```
Λ = (1 / N) · Σ_p  V(p) · max(N · L_p, 0) · fall(p)² · cone(p)
```

and **the outer `facing` multiply is deleted rather than kept** — a cosine
applied twice is the same defect wearing the fix's clothes.

*The decisions, pinned so the step has none left in it.*

- **The construction is "the sample point is the only place a flame has a
  position", not "the cosine moves inside the loop".** Fixing the cosine alone
  removes this defect; removing the centre removes the *class*, and the class is
  the shape this repo has a name for — one state in two shapes. `fall` has no
  kink and would not have shown, and `cone` has a hard rim and would have shown
  eventually. Both are one line each here and a second incident later.
- **The one thing that stays at the centre is the cull, and it is therefore
  conservative.** `d >= 1.0` skips a light before any ray is walked; that is a
  **broad phase** and it is forbidden to change the answer, so it culls on the
  near side of the sphere: `distance - FLAME_RADIUS >= light.radius`. A fragment
  the centre says is out of reach can be reached by the near edge of a body that
  has one.
- **A sample with `cos <= 0` is not traced.** Its contribution is exactly zero
  whatever stands in its way, so this is an exact skip and not a tolerance — and
  it is up to *half the rays* in exactly the grazing configurations that cost the
  most today. The step is expected to be a cost win, and `tests/cost.rs` says so
  or it does not.
- **`View::Shadow` stays visibility.** It is the ordinary meaning of a shadow
  term, and it is the one instrument that separates "the walk is wrong" from "the
  cosine is wrong" — this defect and the black emitter below were both diagnosed
  by reading it. So the loop carries two accumulators, one ray each, and the
  debug view walks every sample including the skipped ones. That is not two
  answers to one question: the skip is separately gated as a proven no-op, so the
  rays it drops contribute zero to the number the lit path returns. **No new
  view** is added.
- **The name goes with the meaning.** `shadow()` no longer returns a share of a
  flame, so it is not called `shadow` and does not return `through`. It returns
  the share the flame *delivers* and, beside it, the share it is *visible* over,
  and every diagnostic that wanted the second one asks for it by name.

*Done when:* the wedge is gone at a measured count, and the frame has moved
towards the reference rather than away from it. **Both done**, and the gate is
new: `the_frame_and_the_path_tracer_agree_about_every_interior_pixel` could not
have carried it. That gate reads `View::Shadow`, and visibility is the one term
this phase does not touch — a flame was already a body for it — so both shadow
gates are invariant to phase 5b in *either* direction and neither could have
caught the defect or can now witness the fix. What can is a picture with light in
it: `a_flame_just_over_a_landing_does_not_wedge_it_with_its_own_below_horizon_
rays` renders the stair scene in `View::Flames` with a flame **half a `z` above
the top landing** — against a `FLAME_RADIUS` of `1.375` `z`, so half the sphere
is inside the boxes — and lays it against the tracer with every albedo set to
one, which is what makes a scene of boxes judgeable for brightness at all while
`mesh_face.wesl` still writes no colour. `oracle::pathtrace::shading` is the
comparison.

**The number is the signed mean, and it fell twentyfold: `-0.0044` of full scale
to `-0.0002`, over 256,711 pixels.** The reference disagrees with *itself* by
`0.0067` a pixel over those same pixels, so the standard error of that mean is
`1.3e-5` — the residual is fifteen of those and the defect was three hundred.
`WEDGE_BIAS` is `0.002`, between them and near neither.

*And a person looked at it*, which is what *How this is judged* asks for. Before,
the top landing is **three flat blocks with a hard step at each join** — the
middle one holding the pool, the two either side plainly darker, the step running
the landing's whole width. After, it is one gradient across all three and the
steps are gone. 163,492 of 262,144 pixels move, worst channel `122` of 255.

**162,921 of them are brighter and 571 darker, which is the opposite of what the
prototype recorded**, and the correction is worth more than the tidiness of
deleting the claim. The backlog entry this phase came from says "21,177 pixels
move on the stair fixture, 20,308 of them darker, which is the overestimate the
centre cosine was paying out". There is no overestimate to pay out: `max(·, 0)`
is convex, so `mean_p max(N·L_p, 0) ≥ max(N·L_centre, 0)` for every configuration
there is — an average over the body is never dimmer than the centre's own cosine.
What the centre cosine cost was *darkness*: rays below the horizon, counted as
shadow at every join. Whatever the prototype measured, it was not this.

*Gates, each fault-injected to red — or to zero — in the same session that trusts
it*, the habit `docs/occluders.md`'s S3 paid for. Two of the three came back with
an answer the phase had not predicted:

- **Injection: the centre cosine, restored.** The gate is red at `-0.0044` and
  the three blocks are back in the picture. Both numbers above are that run.
- **Injection: the skip, removed** — `every_sample` forced true, so a
  below-horizon ray is walked in the lit path too. The frame is **byte for byte
  identical**: `cmp` over the two `512×512` dumps reports `0` of 1,048,576 bytes
  apart. That is the claim stated as a claim rather than as four decimal places of
  an aggregate, and `OPENSHARD_WEDGE_DUMP` is the hook that made it available.
- **Injection: the cull, tightened** to `distance >= light.radius`. Also byte for
  byte identical, and **the phase's own prediction here was wrong.** "Pixels at
  the rim of every pool change" assumes some sample of a flame can be nearer than
  its centre, and none can: `flame_points` samples the disc the sphere *presents*
  to the fragment — the silhouette — and every point of it is `sqrt(d² + r²)`
  away. So the tight cull is already exact for the sampler we have, and the
  conservative form is a **guard rather than a behaviour**. It is kept, with the
  lemma that makes it free pinned as its own test:
  `the_cull_is_conservative_and_no_sample_is_nearer_than_the_flames_centre` sweeps
  five directions at three distances, and the day a sampler reaches for the volume
  instead of the silhouette it goes red and the guard starts earning its keep.

*What it cost, measured on Britain by holding the frame still and moving the
model alone* — `tests/cost.rs`, seven flames, at the widest zoom:

| | centre cosine, eight rays | per-sample, below-horizon rays skipped |
|---|---|---|
| `night` (the seven flames) | 1.94 ms | 1.51 ms |
| above the `dark` floor of 0.71 ms | 1.23 ms | 0.80 ms |
| `sun` (the control) | 0.90 ms | 1.00 ms |

A third off the flame work, which is the cost win the skip was expected to be.
**The control moved, and it is reported rather than absorbed**: the sun's path is
untouched by this phase and its row still rose by a tenth of a millisecond, so
something outside the flame loop — register pressure across one shader, most
likely — is in the reading too. Taking the control's drift out of the flame row
by hand leaves a quarter rather than a third, and a quarter is the number to
believe.

*What it settled about the two rules below it, and one of the two went the other
way.*

- **S3's exemption is unreachable, exactly as predicted.**
  `a_landing_cut_into_three_primitives_is_not_shadowed_by_its_own_pieces` passes
  with `on_the_lit_surface` neutralised — `0` of 720 fragments blamed — where the
  same neutralisation with the centre cosine restored reports **480 of 720**. So
  it is this phase and not the fixture. The whole of `tests/lighting.rs` passes
  with that rule neutralised besides. **The price is that the gate is now vacuous
  with respect to D2**: its flame lies exactly in the landing's own plane, which
  is a ray whose cosine is exactly zero, so nothing is traced and nothing is
  blamed. Deleting the rule or keeping it as a proven no-op is the decision the
  plan deferred to *after* this measurement; what has to come first is a fixture
  that can still reach it, and there may not be one.
- **`same_run` is not retired, and S4 does not get its licence back.**
  Neutralised, `light_runs_along_a_wall_and_stops_across_it` and
  `the_two_faces_of_a_corner_are_lit_from_the_side_each_looks_at` go red exactly
  as phase 4 measured them. **But the reading is about the fixture as much as the
  rule**: both build their spots with `Spot::face` and no `part_of`, so
  `spot.solid` is `None`, `own_box` is `None`, and D2 — the thing that would
  answer for a coplanar neighbour — cannot be asked at all. The question is open
  and its next step is in the backlog. ✅ **Settled since, and it was the fixture:
  `same_run` is deleted.** Three places named no solid — those two spots and
  `plan::elevation`'s own rows — and with all three naming one the rule has no
  case left anywhere in the crate. The backlog entry below has the numbers and
  `docs/occluders.md`'s S4 has the deletion.

*The side-lit case was checked and the fixture does not show it.* On
`a-wall-run-with-a-lamp-along-it`'s elevation — the one fixture in the tree built
for it — 7,572 of 29,696 pixels move, 3,080 brighter and 4,492 darker, **worst
channel 3 of 255**, and there is no wedge and no seam in either picture. So this
phase neither confirms nor refutes the reporter's hypothesis that the artefact
goes the same way; what it confirms is `ebfe83c`'s own sentence, that the side-lit
seam is present whether or not a fixture shows it.

*A new accepted cost, and it is the estimator's.* The eight rays are now in the
**brightness** and not only in the visibility, because the cosine joined the sum.
Where a fragment stands inside the flame's own sphere — within about a tenth of a
tile — how many of its eight samples clear its plane is a coin flip, and the
result is grain in a bright region rather than in a penumbra. On the wedge scene
the worst pixel is `0.4896` of full scale from the reference and 126 of 256,711
are past an eighth, every one of them in that hot spot. The old model had zero
variance in the cosine because it evaluated it once. Temporal accumulation is the
answer if it ever matters, which is the same answer phase 5 parked.

*What it does not fix, named so nothing claims it.* The black emitter below: a
flame an eighth of a tile across, standing inside its own lamp post's box, is
still inside it at every sample. And the sun has no cosine at all today — it is
added straight, with no `N·L` anywhere — which is phase 8's "the same BRDF" and
not this step's.

*And a correction this step owes phase 5's own paragraph*, in both copies of it —
`light::shadow`'s doc comment and `blit.wesl`'s: "moving the sample point moves
either term by well under a byte" is true of the falloff and true of the cosine's
*magnitude*, and false at the horizon, where `max(N · L, 0)` has a kink. The
error there is not a share of the radius, it is the whole clamp. Both copies are
gone with the function: `light::shadow` is `light::arrival` and returns
`Arrival { delivered, visible, stopped_by }`, `blit.wesl`'s is `arrival` returning
the first two, and `Reach::cone` — "how much of the beam falls here and how
squarely the surface looks at it", both at the centre — is `Reach::delivered`,
which is the sum. `Reach::added` is `delivered` times the colour and the
intensity, for the sun's own `Reach` as well, which is the one invariant left
where two numbers used to be multiplied at the call site.

*One thing the phase changed that is not the model.* `Reach::stopped_by` now names
only what a ray **with light to lose** was stopped by. A below-horizon ray
delivers zero whatever stands in its way, so blaming an occluder for it is
crediting the walk with a darkness the cosine had already decided — which is the
wedge stated as a diagnostic rather than as a picture, and it is why the S3 gate
above reads zero.

**Phase 6 — the impostor.** Sprite silhouette, analytic prism for depth and
normal, one draw. `WIDTH_OVERLAP` is deleted.
*Done when:* the difference frame's "drawn by one side only" classes are zero
except for rasteriser fill-rule dashes, against today's 1370.

*Three things this paragraph does not settle, written down before the phase
starts rather than discovered inside it:*

- **Where the prism comes from.** A fragment needs its static's own boxes, which
  means a range into a storage buffer per instance. `statics.wesl`'s header said
  it kept "inside WebGL2's ceiling" and that was a stale objection — the crate's
  ceiling is WebGPU (`lib.rs`, `docs/lighting.md` decision 30.5) and `blit.wesl`
  beside it reads eleven storage buffers. The comment is corrected; the design
  is allowed.
- **The view ray is a constant and the phase should say so.** From
  `camera::project_exact`, holding a screen point fixed gives `dx = dy` and
  `22·(dx + dy) = 4·dz`, so `dz = 11·dx` — and at `Z_PER_TILE = 11` the
  direction in the isotropic metric is exactly **`(1, 1, 1)`**. So there is no
  per-fragment unprojection to write: it is a slab test against a constant
  direction. Writing an inverse projection here would be the sixth spelling of
  one, which is already a backlog entry of its own.
- **The miss case needs its own measurement.** "A pixel whose ray misses the
  prism takes the nearest point on it" lives on a pixel or two of silhouette and
  no picture gate will ever fail on it. The phase's "done when" should carry a
  second number: how many fragments took the nearest-point path, and how far off
  the prism they were.

*Phase 6a landed: the arithmetic, on its own.* `crate::impostor` is `ray_from`,
`meets` and `nearest` with thirteen tests and no pipeline change. The three
points above are settled by it in order — the ray is `VIEW`, one constant
direction; the miss is `Meeting::outside`, how far in **tiles** a fragment fell
outside its own volume, a number rather than a branch; and the boxes come from
the instance, below. Two decisions the geometry does not force are written at
their own definitions: ties between two exit faces go to `z`, then `y`, then
`x`, so a lid reads as a lid to its own rim; and `TANGENT` is `1e-4` of a tile
against a *measured* `3.5e-6` of `f32` rounding at a corner.

*And the scope widened, deliberately: the impostor is for **every** static, not
only a fitted climbable.* A wall, a floor and a body are boxes too, so the same
meeting answers them — which retires `statics.wesl`'s whole inverse projection
(the stance switch, `INSIDE`, the corner branch, the height recovered from
`pixel_y`) rather than leaving it beside the new path for the majority of
statics. Three backlog entries go with it: the `INSIDE` clamp that still sits a
hundred-and-twenty-seventh of a tile behind every east and south face,
`own_solid`'s ambiguity on a fitted climbable, and `parity_frame`'s fixture
naming an owner where the shader compares a solid.

**A fragment meets only its own static's boxes, and that is what makes the seam
disappear by construction rather than by a border.** Today a mesh face is a
polygon in a shared buffer, rasterised among everyone else's, and which object a
pixel belongs to is settled afterwards by the depth test — so where two
silhouettes disagree there is a pixel belonging to neither, and nothing but
growing a shape can cover it. Under the impostor a pixel is *already* one
instance's, because that instance's own quad drew it, and the boxes it is met
against are that instance's alone. A neighbour's geometry is not reachable. So
"the silhouettes disagree" stops being an event between two objects and becomes
a property of one: `Meeting::outside`, which is a number to fix the geometry by.
See `docs/style.md`'s *No fudge constants* — this phase is where that rule was
written down, and `WIDTH_OVERLAP` is the case it was written from.

*What the mesh pass is, which this phase settles rather than deletes.* It writes
no colour and takes its depth from the sprite beneath it: it is a **layer over**
a static, not a pass that draws one. With the impostor no real static needs that
layer. What still does is the four hand-built scenes — `examples/boxes.rs`,
`two_cubes.rs`, `tests/traced.rs`, `examples/synthetic_stair.rs` — which have no
sprites at all and exist to watch rays travel through geometry. So the pass
stays as **the hand-built-geometry layer**, off every real static, and gains a
colour target: without one a box in `boxes.rs` has no albedo and the comparison
against the path tracer there still runs on the invented `Albedos::body`. That
is phase 2's leftover closed at the diagnostic layer, where it actually lives —
not, as this document first had it, as a side effect of deleting a pass.

*And the grid's own climbable geometry was wrong for this, so it was fixed rather
than worked around.* A fitted climbable's occluders were **surfaces, not a
volume**: a lid per tread (degenerate in `z`) and a riser per tread (degenerate
on the climb axis), whose union encloses nothing. For a flight climbing *away*
from the camera that happens to cover the visible side; for one climbing
*towards* it every riser faces away and is hidden behind its own tread, so the
grid held no vertical surface at all where the art draws the whole front of the
staircase.

The impostor spent one commit meeting a volume rebuilt from the `Prism` and
joining back to the grid by `occlusion::Part` — which is a second statement of
one shape, and therefore the thing this phase's own rule forbids. **`Builder::
add` pushes one body a tread now**, its strip from the static's base to its own
height, and `push_volumes` is the grid's boxes copied. The split's reason is on
the record in `gbuffer_archive.md` step 4b — "the representation the render pass
(step 4c) needs to walk" — and both halves of it are retired: phase 2 gave the
normal a plane of its own, and this phase takes the mesh pass off every real
static.

Three things came with it. `WIDTH_OVERLAP` and `widen_footprint` are **deleted**,
since there is no second silhouette left for a border to reach across. The
**vertical-ray shortcut** in both walks and in `blit.wesl` now looks at bodies as
well as lids — it skipped everything with an `edges`, which was already a gap for
every body in the world (a ray straight up out of a tree's box left it
unstopped), and treads-as-bodies is what exposed it. And one world claim was
**retired by the geometry rather than re-taken**: a fragment on a tread's top
used to be shadowed by "the riser that tread stands against", which is a surface
of the tread's own body — a surface cannot shadow a point of itself, and that
assertion was measuring the split. What replaced it is a fragment on the flight's
front shadowed by the tread above, and a fragment on the bottom tread shadowed by
the two climbing away from it.

*And the staircase's own oracle followed, which is what a gate on a scene's
geometry is built to make happen.* `examples/synthetic_stair.rs` panicked on its
first run after the reshaping — "the grid holds 3 solids and this oracle derived
6 planes" — because its whole check was a plane-for-solid pairing, and the two
lists stopped having the same length. What it states now is **both** shapes and
the join between them: a `Body` a tread, derived from the profile and held
against the grid's own solid corner for corner; a `Slab` a drawn face, held
against the mesh as before; and each face held against the body it is a face of,
so the surface list and the volume list cannot say different staircases. Two
things the panic pulled up with it. The example rebuilds `push_mesh`'s loop by
hand and so still asked for `Part::nth(part)` when the real pipeline had already
divided — six faces against three bodies, a second failure waiting one line
below the first. And the fragment's exemption changed *kind*: what the oracle
drops is the primitive the engine drops, `lit.solid`, which is now the tread's
whole body rather than the one plane the pixel sits on. Dropping nothing at all
was tried first — a body has an inside, so "the ray leaves at `t == 0`" excuses
a fragment's own surface with no name needed — and the sweep priced it at nine
pixels across the seven flame heights, which is the engine's own rule showing up
as a measurement. The rule was never an epsilon; phase 4 wrote it as one
sentence because for a *plane* the name and the tangency are one sentence.

*Phase 6c landed: the pass meets its own boxes, and the inverse projection is
gone.* `statics.wesl` reads a storage buffer of `impostor::Volume`s, takes the
run of it its own instance names, and answers a fragment's **position and
normal** with the face of the box its view ray leaves. What that replaced is
five branches: a floor's two fractions from the pixel's offset from the tile's
centre, a wall's one fraction pinned to the edge its stance names and the other
run along it, and a height recovered from `pixel_y` at four screen pixels a `z`.
`INSIDE` went with them — the hundred-and-twenty-seventh of a tile every one of
those was clamped by — and `place_format.wesl`'s `outward` went with the
*normal*, which is the whole of what that table was for.

**The wrong table it was, and the impostor cannot spell it.** `outward` answered
`(0, −1, 0)` for a north face, the side turned *away* from the viewer, where
what a camera sees of a north wall is its `+y` surface — this document's own
backlog, five graphics out of 1197, which is why nothing had caught it. A ray
from the camera leaves a box through `+x`, `+y` or `+z` and through nothing
else, so there is no row left to be wrong. `crate::place::Stance::normal` keeps
the table with its defect written down beside it: its readers are the hand-built
G-buffers of `plan.rs` and the fixtures, which state a scene by naming a stance
and have no boxes to meet.

*The gate is `a_sprite_pixel_meets_the_same_box_on_both_sides`, and it is the
only thing comparing the two spellings.* `impostor.wesl` and `impostor.rs` are
one arithmetic written twice with no compiler between them —
`normal_format.wesl`'s situation one plane down — so the test renders a sprite
over three boxes and asserts, for **every one of 4,620 fragments**, that the
normal word the GPU wrote equals `gbuffer::pack_normal` of what this side
answers and that the point agrees to `1e-4` of a tile. It reports what it
reached rather than leaving that to a reader: 638 east faces, 913 south faces,
3,069 lids, and 2,684 fragments answered by the nearest-point fallback. Two
fault injections turn it red — the `+y` normal flipped, and `far` shifted by two
thousandths — and one deliberately does *not*: the tie between two exit faces is
a **line** across a box rather than an area, so a sweep of whole pixels reaches
it only by luck, and `impostor.rs`'s own lid test is where that case is
constructed instead.

**And it found that the grid was the wrong place to ask what shape a static is.**
`push_volumes` read the occlusion grid, and `Builder::add` answers two questions
at once: what shape is this, and does it stop light. It refuses outright
everything the tiledata does not mark `NO_SHOOT` or `WINDOW` — so on one real
place at radius 6, **nineteen of thirty-nine drawn pictures stood as no box at
all, twelve of them south-facing walls**. Read through the grid, every one of
those became a billboard: the middle of its tile, no facing, lit from every
side. That is a *worse* answer than the stance it replaced, and it would have
undone `docs/lighting.md`'s decision 27 — a lamp beside a wall lighting its cap
as fully as one standing over it — for every wall cap in the world.

So the two questions came apart. `occlusion::boxes_of` is what shape a static
standing at a place *is*, one function with two readers: `Builder::add` is now
the opacity gate, the owner, the roof bit and the hole's placement wrapped
around it, and `push_volumes` is the same shapes joined to the grid by `Part`
for their `SolidId` — or `NOBODY` where the grid refused them, which is the
honest name for a shape no shadow ray will meet. **A pane of glass has a shape
whether or not it casts a shadow.** The census reads `0 of 39` now, and
`examples/isolated_scene.rs` prints it every run.

Two things it changed that are not the pass. `blit.wesl`'s ambient takes the sky
share from **the tile the instance carries** rather than `floor(position)`: a
south or east face's fragment now lies exactly on its tile's boundary, where
flooring reads the neighbour, so a street wall would have taken the room's
ambient along one of its two sides and not the other. The walk beside it already
took the carried tile, and says why at its own `first`. And
`StaticGeometry::absorb` is one place for joining the map's furniture to the
server's dropped items, because **three of the four lists are addressed by
index** — which turned up a live defect the phase did not go looking for: the
mesh rows were concatenated without shifting the vertices that name them, so a
climbable *item* drew its faces against whichever of the map's rows its own
numbering landed on. It needs a climbable item to show, which is why nothing had.

Two world claims were re-taken, and both were the same claim: **a face lies on
its own edge, exactly.** They asserted `120/127` of a tile — one step of the
retired clamp short of it — and the reason was that `blit.wgsl` found a cell by
flooring a position. Neither half survives: the walk takes the cell from the
tile the instance carries, and what a fragment is exempt from has been primitive
identity since phase 4. So the number is the plane the geometry states, asserted
to the float.

*What is left of phase 6, and it is not small.* ~~The mesh pass still runs over
every fitted climbable~~ **done, 6d** — see that phase's own account below. The
corner's two panels are still told apart
by the **screen half** rather than by the box the ray met — the impostor picks
between them for the normal, but the id has to follow `split_corners`' twin row
and a box carries no row number. ~~`own_solid` still scans a cell to name a
sprite's solid, where the box the fragment met already carries one.~~ **Done,
6f**, and it was not a cost item after all — it was the defect 6d uncovered. And the
phase's own second number — how far a *real* static's art overhangs its own
fitted prism — is still unmeasured: the gate's fixture is a plain rectangle
nobody fitted to anything, so its overhang is a property of the fixture.

*A person has now looked at a lit frame of it*, which is what *How this is
judged* says the instrument is. `examples/isolated_scene.rs` at Britain's
`(1497, 1626, 10)`, radius 6, one lamp post added by hand so the scene has a
flame in it at all, read in `View::Lit`, `View::Light`, `View::Normal` and
`View::Shadow`. Three things it says. **The census holds on a scene 6c never
ran** — `0 of 340` drawn pictures stand as no box, against the `19 of 39` the
grid answered with before the split. **Nothing reads as a seam**: no border, no
pixel of a silhouette belonging to neither side, which is the phase's own claim
by construction and is now a claim somebody has checked with their eyes.
`View::Normal` over the same place is what it was — every fragment carries a
facing, a wall reads as a green face, a red end cap and a blue top. And it
found **two** defects, each its own backlog entry below: a flame's own sprite is
black, and a shadowed floor leaked a line of light along every tile boundary —
that second one is **fixed**, and it was 6c's own arrival, since the position
that contradicts its instance's tile is what the impostor started writing.

**Phase 6d — the mesh pass off real statics, and its colour target.** *(Landed
2026-08-09.)* Two changes, named together because the second only matters once
the first is true.

*Off real statics.* `statics::collect` and `items::collect` each had one call
to `push_mesh`, gated on `Placed::prism` — the second draw over a climbable
static's own billboard sprite that 6c's impostor made redundant for position and
normal but nobody had yet removed. Both went, along with `Placed::prism` itself
and `push_mesh`/`MeshSink`, which had no caller left once they did: `push_mesh`
was `pub(crate)` for exactly those two call sites, because a third, external one
(`examples/*.rs`, `tests/*.rs`) cannot see a `pub(crate)` item at all, so the
four hand-built diagnostic scenes that still draw mesh geometry were never
routed through it and did not need to change. What is left calling
`MeshFaceRenderer::render` is exactly those four —
`examples/boxes.rs`, `examples/two_cubes.rs`, `tests/traced.rs`,
`examples/synthetic_stair.rs` — plus the crate's own direct tests of the pass
(`tests/frame.rs`'s `render_places` helper), which draw geometry with no sprite
under it and have no impostor of their own to fall back on.

*And a colour target.* `mesh_face.wesl`'s `FragmentOut` gained a `color`
attachment at location 0 — `crate::blit::WORLD_FORMAT`, sRGB-encoded from a new
`MeshFaceVertex::colour` (linear, flat across a face, `tonemap::linear_to_srgb`
in the fragment stage the same way every other producer of that plane writes
it) — and `MeshFaceRenderer`'s pipeline and render pass grew a fourth target and
a fourth colour attachment (`target.view`, loaded rather than cleared, ahead of
the three G-buffer planes, matching `GroundRenderer`'s own target order). This
is what phase 2's own table meant by "a mesh face has none": the G-buffer's
albedo plane *is* the world/picture texture (`gbuffer.rs`'s own doc says so
directly), and until this phase the one producer that drew into it without a
sprite underneath wrote nothing there at all.

*The oracle side follows it.* `oracle::body_albedo` reads the colour back off
the frame the same way `oracle::ground_albedo` already does for land — a box's
own faces, filtered by `Stance::MeshFace`'s routing sentinel rather than by
`Kind::Land`, asserted flat, decoded `srgb_to_linear`. `examples/boxes.rs` and
the shared fixture in `tests/traced.rs` now write every box's face in
`oracle::pathtrace::Albedos::INVENTED.body` — the same authored linear value on
both sides of the vertex/oracle call, so "the same albedo on both sides" is a
measurement of the frame again rather than two authors typing the same three
floats. `Albedos::INVENTED.body` stays the fallback for `scene_flat`, which has
no boxes and therefore nothing to read.

*Done when:* a box's own colour is on the engine's side of a shaded comparison
at all. **Done, and measured rather than assumed clean.** `OPENSHARD_BOXES_SCENE=pair`:
the visibility and face oracles — unaffected by any of this, and run first as
the sanity check that nothing about *where* a fragment is moved — read
`0` disagreement everywhere, on both boxes' east and south faces and on the
ground behind them, exactly as before. `oracle::body_albedo` reads a single flat
colour off the two boxes' six drawn faces with no panic, which is the measured
half of "the same albedo on both sides": the bytes the mesh pass wrote are the
bytes the oracle got back.

**What it does not close, named so nothing claims it.** The full shaded
comparison (`View::Lit` against the tracer's own `Brdf::Lambert` render) on a
scene *with* boxes is not tight, and was never expected to be: `boxes.rs`'s own
code lights every scene but `scene_flat` with `NIGHT` ambient on the engine's
side and gives the reference none at all, deliberately, because "giving the
tracer an ambient instead would be this renderer's own ambient model, restated
inside the thing that checks this" — the same reasoning phase 0 gave for why
only a boxless, ambient-free scene is the calibration gate. On `pair`, mean
channel difference `42.58` of `255`, worst `71` — a number this phase makes
*measurable for the first time*, not one it introduces; before it, the same
comparison had nothing on the engine's side to disagree about, because there
was no colour to compare. Closing it wants either an ambient-free box scene
(`scene_flat`'s own trick, extended) or a reference that models sky/ground
ambient honestly, and it is not this phase's own "done when".

**And there is no automated gate on any of this yet.** `oracle::body_albedo` is
exercised by `examples/boxes.rs`, a tool a person runs, and by nothing under
`cargo test`: no scene in `tests/traced.rs` currently asks for a shaded
comparison on a box, because every one that has boxes reads `View::Shadow`
(visibility, which never cared about albedo — the comment at each of those call
sites already says so) or pins both sides' albedo to `1.0`
(`a_flame_just_over_a_landing_does_not_wedge_it_with_its_own_below_horizon_
rays`, deliberately, to isolate the below-horizon wedge from a second measured
quantity). A regression that made a mesh face's own colour wrong would be caught
by nothing but a person looking at `boxes_lit_vs_traced.png`. Worth a scene, the
day this crate wants one: ambient-free, one box, `body_albedo` on both sides,
the same shape `the_frame_and_the_path_tracer_agree_about_brightness_on_open_
ground` already is for the ground plane.

**Phase 6i — the gates 6f, 6g and 6h cost, and why three in a row got through.**
🟡 *Items 2, 3 and the fourth (`synthetic_stair`) landed 2026-08-10, and the
floor's corner leak with them. **Item 1 is the whole of what is left**, and the
one open defect beside it is the fringe. Item 2 landed by being read rather than
done as written — the filter it named is not what excludes a sprite fragment
from that test — and the floor's leak closed a real hole without explaining the
picture that found it; both accounts say so where they stand.*

Three defects, one after another, all of them found by **a person looking at a
lit frame** and none by anything under `cargo test`. They are one failure and it
is worth naming before the gates are listed: 6d removed a pass by checking what
it **computed** — a position and a normal, both of which the impostor genuinely
answers better — and not what it **delivered**. A `MeshFaceRow` was also carrying
the *name of the primitive* (6f), the *stance of the surface* (6g), and a
*silhouette wide enough to cover the sprite* (the fringe, still open). Three
facts came off with the pass and nothing said so.

*Why nothing caught them.* Three structural reasons, each with its own fix, and
none of them is "somebody forgot a test".

1. **Every stair instrument in this tree drives the mesh pass.**
   `examples/synthetic_stair.rs`, `examples/boxes.rs`, `examples/two_cubes.rs`
   and `tests/traced.rs` each build a `MeshFaceVertex` list by hand — that is
   how they came to be `pub` rather than routed through the retired
   `push_mesh`. The client draws a staircase through the **sprite and the
   impostor**. So the four instruments built for stairs are blind to the path a
   stair actually takes, by construction, and were green through all three
   defects. *Done when:* one fixture drives `statics::collect` over a fitted
   climbable and compares against the tracer. `tests/frame.rs` already has two
   `statics::collect` call sites to build on, and `tests/cost.rs` a third.

   **The one item of the four still open, and its entry point is narrower than
   it looks.** Both `frame.rs` call sites hand `collect` a real `WorldMap` off
   `client_dir()` and an `Occlusion::EMPTY`, so neither is a fixture — they skip
   where the client files are absent, and they ask nothing about volumes. A
   *synthetic* map is available and was not when item 3 chose to restate
   `push_volumes`'s eight lines instead:
   `openshard_map::map::WorldMap::from_blocks` builds the land and
   `WorldMap::place_static` puts a static on it. What it still needs, and what
   to cost before planning the rest: a `TileData` the fixture states itself, and
   — the real constraint — **a picture a `Prism` fits**, since the fit reads the
   art's silhouette and a rectangle is not a staircase. `tests/prism.rs` is
   where such a picture would have to come from.

2. **The one gate that states the invariant filters it out.** ✅ *Landed
   2026-08-10, and not where this item said the filter was.*
   `traced.rs`'s `a_face_fragments_own_plane_is_the_primitives_own_number` —
   the test whose whole subject is "a fragment's plane is its primitive's own
   number, bit for bit" — opens with `if texel.stance != Stance::MeshFace {
   continue }`. It cannot see a sprite fragment, and 6f, 6g and 6h are each a
   fragment whose plane is not its primitive's own number. *Done when:* the same
   sweep runs over sprite fragments, which is the same loop with the filter
   inverted and `mine` read off the position plane's fourth channel.

   **Inverting that filter yields nothing, and the reason retires the item as
   written**: `traced.rs`'s scene draws *no sprite at all*. It builds a
   `MeshFaceVertex` list by hand and runs the ground pass beside it — the same
   fact as item 1, that every stair instrument in this tree drives the mesh
   pass. The filter is not what excludes a sprite fragment there; the fixture is.
   So the sweep over sprite fragments is item 3's, in `tests/frame.rs`, and what
   was genuinely still missing was the *strength* of the claim: that sweep held
   the plane to `1e-3` where the mesh one holds it bit for bit.

   It is an equality now, on both paths, and by construction rather than by
   measurement. `impostor::meets` reached the met plane through
   `from + ((hi − from) / VIEW) * VIEW` — a divide and a multiply, and `VIEW.z`
   is `Z_PER_TILE`, eleven and no power of two, so the `z` round trip had
   nothing exact about it and a driver contracting the pair into an `fma` need
   not have agreed with one that did not. Measured before the change: 0 of
   78,400 fragments off, which is a fact about this fixture's numbers and not a
   reason. `meets` now takes the exit axis's coordinate from the bound that
   chose it — the plane the `t` was solved for — in both twins, and the sweep
   asserts `at[axis] == hi[axis]`.

   What does *not* depend on this is D2's exemption, and that is worth knowing
   before the next person tightens something for its sake: since 6f
   `on_the_lit_surface` reads the plane off `solid_at(mine)`, so both sides of
   its equality come out of one buffer and neither is the fragment's position.

3. **Nothing compares a fragment's four facts against each other.** ✅ *Landed
   2026-08-10, `a_sprite_fragment_is_a_point_of_the_primitive_it_names`,
   `tests/frame.rs`.* Position, normal, solid and stance are each checked
   against the *producer's* own arithmetic — `a_sprite_pixel_meets_the_same_
   box_on_both_sides` against `impostor::nearest`, `a_direction_survives_the_
   normal_packing` against `pack_normal` — and never against one another. They
   are not four independent measurements: three of them are properties of one
   box. *Done when:* one sweep, three lines, over a scene carrying a merged
   run, a fitted climbable, a corner, a wall and a floor:
   - the position lies on the boundary of `primitives[mine]` — **6f fails this**
     (a fragment on the third tread naming the first);
   - the normal names a camera-facing face *of that primitive*,
     `at[axis] == primitives[mine].hi[axis]` — **6h fails this** (the buried
     face is interior to the merged box);
   - the stance is `stance_of(normal)` — **6g fails this**.
   Each defect fails exactly one line, and the sweep reads three planes that are
   already read back. This is the cheapest of the three items and the one that
   generalises: it is the statement that a fragment is a point *of* something.

   Built rather than driven through `statics::collect`: that function needs a
   real `WorldMap`/`TileData`/`Cutaway` pipeline, and `push_volumes` — the thing
   under test — is `pub(crate)`, unreachable from a `tests/` binary at all. The
   fixture restates its eight lines instead, off the same two `pub` primitives
   `push_volumes` itself is built from — `occlusion::boxes_of` for the shape,
   `Occlusion::id_of`/`Occlusion::solid` for the grid's own name of it — so the
   restatement cannot silently diverge from a formula, only from geometry the
   grid disagrees with. A merged run (three tiles, one owner, `occlusion::
   merge`'s fold), a fitted climbable (`facing::Prism`, three treads), a
   corner (`Facing::Corner`), a lone wall panel and a floor lid, each its own
   `SpriteQuad` against a shared `Occlusion`. Confirmed to have teeth by fault
   injection: swapping `stance_of`'s `FACE_EAST`/`FACE_SOUTH` arms in
   `statics.wesl` turns the third line red at the first mismatched pixel, then
   reverted.

*And a fourth item, which is a tool that stopped working and nobody noticed.*
✅ *Landed 2026-08-10.* `examples/synthetic_stair.rs` panicked outright for
`OPENSHARD_STAIR_RUN>1` — `gate_against_grid` derived one body per flight per
tread and asserted it against the grid, and S3b had merged the run into one
primitive spanning every flight (`this oracle says 101, the grid's own solid says
103`). The **one knob in the tree that poses the two-abutting-statics question** —
the question 6h turned out to be about — had been unusable since the merge
landed.

It learns about merging. What made the derivation wrong was a premise that file
stated out loud and got backwards: *"each flight of a run gets its own `Owner`,
which is the whole point of building the run"*. An `Owner` is a `(z, graphic)`
and carries no tile, so the flights of a run are **one** owner and, with one
`Part` a tread, one primitive a tread. `Body::primitive` names the fold, `merged`
takes it and checks it is a union of point sets rather than a bounding box (the
pieces agree exactly off the run's axis and tile that axis with no gap), and the
gate holds the folded boxes against the grid's own solids **and** asserts every
flight names one `SolidId` for a tread — which is what makes it a statement about
the grid rather than about the fixture. `oracle_visible` drops a primitive and no
longer a piece, matching the walk after the merge. Green where it was red:
`0/32472` face pixels disagree at `RUN=3`, `0/2304` at `RUN=2 UP=east`, `1/29265`
at `RUN=4` with four treads.

Two findings came out of running it, and both are worth more than the fix:

- **The flame's position was stated three times** — the `Light` the renderer
  gets, the crosshair, and the oracle's own tuple — and the three agreed only
  because they were the same expression, `at + (ldx, ldy)`. Moving the anchor to
  the run's last tile (so the default flame stands *beside* a wide run instead of
  inside its third flight) moved one of them, and the face oracle immediately
  reported **1,375 pixels of a three-flight run as the renderer's fault**. One
  expression now. An oracle lighting the scene from somewhere the renderer did
  not is the most expensive shape of instrument defect there is: it reads exactly
  like the thing it is built to find.
- **That fixture cannot pose the exemption question it now answers correctly.**
  Dropping the whole merged tread and dropping only the fragment's own flight's
  piece give **identical** counts on every scene it builds, because a riser sits
  on its own body's face and a lid on its own body's top — a ray leaving either
  never re-enters the tread it belongs to. So the granularity is right by
  construction and gated by nothing. What would pose it is a fragment whose own
  primitive stands *between* it and the flame, which for a merged run means
  looking along the run rather than across it.

**Phase 6h — the impostor meets the *merged* primitive.** *(Landed 2026-08-10.
`docs/occluders.md`'s D6, which that plan decided and did not do.)* With 6f and
6g in, the person who reported the wedges reported what was left: **bright,
one-pixel vertical strokes at every seam between two abutting statics**, once a
tile, on an otherwise shadowed staircase — garbage on the vertical joins.

Measured rather than guessed at, and the G-buffer said it in one row: at the
stroke's column the normal plane reads `(+1, 0, 0)` where every neighbouring
pixel reads `(0, +1, 0)`. An **east** face, one pixel wide, at the tile boundary.
`statics::push_volumes` was still handing the impostor `boxes_of`'s per-*tile*
shapes, and S3b had folded the run into one primitive: so two adjacent statics of
one staircase stood as two boxes with a face **buried between them**, a face the
merged solid does not have. And because a merged primitive is one id, the buried
fragment was excused from shadow by the solid it was buried in — fully lit, at
full flame colour, against a dark tread.

`push_volumes` now takes the grid's own box wherever `Occlusion::id_of` names one
and keeps `boxes_of`'s where it does not. That fallback is not a hedge: it is 6c's
own finding, that `Builder::add` refuses about half the drawn pictures of a
Britain street outright, so reading *everything* through the grid would turn every
one of them back into a billboard. Measured on the same frame: 42 stray bright
pixels before, **0** after, with the normal at the seam column now south like its
neighbours. Whole crate green.

**Phase 6g — and the stance the box's face is, not the one the art was read
as.** *(Landed 2026-08-10, straight after 6f and for the same report.)* 6f gave
a sprite fragment the *identity* of the box its view ray met and left the
*stance* alone. The stance is the second thing the mesh pass had been carrying
for a climbable, and `blit.wesl` reads it for `lit_plane` — the plane D2's graze
exemption is stated against. A plane the fragment is not in is the wrong one to
excuse a candidate against, and for a flight of steps the plane it named was not
even close: `facing_of` reads a staircase's silhouette as a **corner of a
house** (`occlusion::boxes_of` says why, at length), so every pixel of a tread
was carrying the face of a corner panel, picked by *which half of the sprite it
was drawn on* — `across > 0.0`. That draws a wedge whose straight edge is the
sprite's own middle column, which on screen is a **vertical** line, which is what
a person looking at a lit staircase reported.

`statics.wesl`'s `stance_of` takes it off the met face instead: `+z` is
`STANCE_FLAT`, `+x` is `STANCE_FACE_EAST`, `+y` is `STANCE_FACE_SOUTH`, and
there is no fourth case because `meets` only ever names a camera-facing face.
`FACE_NORTH` and `FACE_WEST` become unreachable for a static that met a box, and
that is not a gap: a panel standing on its tile's north edge is *drawn* on the
box's `hi.y`, which is what `FACE_SOUTH` names, and `lit_plane` agrees with the
impostor by construction now rather than by a table. The corner branch stays for
the two things the box cannot answer — the `id` (a corner's halves address two
instance rows and a box carries no row number, which is this phase's own last
join) and the stance of a fragment with no box at all.

Every gate green, including `tests/traced.rs`'s wall scenes, which is what says
the wall case — whose plane moved by `PANEL_THICKNESS`, from the panel's far
side to the side the camera sees — moved the right way. On the crate's own
Britain staircase the dashed hairline 6f left along the tread/riser joins is
continuous now instead of alternating: the alternation *was* the screen half.

**Phase 6f — a fragment carries the name of the box it met.** *(Landed
2026-08-10, and it is 6d's own bill.)* A person playing the shard reported that
staircases had started "artefacting with polygons" — and they had, from the hour
6d landed. `View::Shadow` on a real flight in Britain draws it outright: a
checkerboard of triangular wedges down every staircase, dark red against white,
where every other surface in the frame is clean.

*What it was.* `blit.wesl` asked `own_solid` which solid of the grid a **sprite**
fragment is a point of, by scanning the fragment's own cell for a solid with the
drawn static's owner and a shape its stance agreed with. That is exact for
everything `Builder::add` stands **one** shape per owner for — a wall's panel, a
floor's lid, a body's tile, never two of a kind — and ambiguous for the one thing
that is not: a fitted climbable stands one box per *tread*, every one of them
`Edges::ANY` under one `Owner`, so the scan named a set and the loop returned
whichever tread the cell's reference list held first. Every pixel of a flight
claimed to be a point of its bottom step, and the steps above it self-shadowed.

**And this was written down.** `own_solid`'s own doc named the fitted climbable
as "the one case this cannot answer", and excused it in the next clause: *"and it
is the case that does not ask: every pixel of it is drawn by the mesh pass over
the sprite, which carries its id."* 6d deleted that pass. The backlog entry for
the same function is filed under **cost** — thirteen scans of one cell for a
four-tread flight — and says the exactness point in its last sentence, where
nothing reads it as a hazard. A premise stated as an aside in the code that
depends on it, and a defect filed as a performance item, are the two halves of
why 6d shipped this: the phase checked what it *removed* (position, normal) and
not what the thing it removed had also been **carrying**.

*The fix.* `impostor::Volume` has carried its box's `SolidId` since 6b, in a word
the vector's own alignment paid for. `statics.wesl` now keeps which box its ray
landed on and writes that name into the **position plane's fourth channel** —
which every producer had been filling with a constant `1.0`. An id is three bytes
and an `f32` holds every integer to `2^24` exactly, so the round trip is lossless
by construction; `SolidId::NOBODY` does not fit and does not need to, since a
negative channel is the whole of "a point of no solid". `solid_format.wesl` is
the format, `gbuffer::pack_solid`/`unpack_solid` its Rust twins,
`gbuffer::Fragment::solid` the field a fixture states it in, and `own_solid` and
`OWNER_NONE` are gone from the pass along with the last thing that compared an
*owner* at all.

Two things it is better at besides. A **corner**'s two panels were told apart
here by the resolved stance and are told apart now by the box the ray met. And a
cell scan per fragment left the pass entirely.

*Gates.* `a_sprite_pixel_meets_the_same_box_on_both_sides` — the sweep that
already compares the GPU's meeting against `impostor::nearest` over the same
boxes — now gives its three boxes three **distinct** names and asserts the
channel equals the met box's own, as an equality and not a tolerance.
Fault-injected in the same session: writing `volumes[in.volumes.x].solid` (always
the first box, which is precisely the shape of the shipped defect) turns it red
at the first fragment. `the_shader_does_not_stop_a_vertical_ray_with_a_lid_it_is_
not_under` states the bottom tread's `SolidId` outright, where it used to lean on
the grid's reference order; `plan::elevation` states the panel it drew, through a
new `Occlusion::id_facing` — the CPU home of the by-side rule that used to live
in the shader. Whole crate green: 430 lib tests and every integration suite,
`tests/traced.rs` and `tests/lighting.rs` among them.

*What it does not close, measured rather than assumed.* The wedges are gone; a
**hairline** remains, one dashed pixel along every tread/riser join. That is the
other half of what the mesh pass had been carrying — the **stance**. `blit.wesl`
reads it for `lit_plane`, the graze exemption's plane, and for a stair fragment
the stance is still the sprite's corner-derived face rather than the surface the
fragment is actually on. The measured normal is the honest answer and it is
already in the G-buffer; what stops a one-line swap is that for a *wall* the two
disagree by design — `lit_plane(FaceNorth)` names the panel box's `lo.y` and the
impostor's normal names its `hi.y`, `PANEL_THICKNESS` apart — so moving it is a
change to every wall in the world and wants its own measurement. In the backlog.

**Phase 6e — the grid stops being a rule.** 🚩 **[`docs/occluders.md`](occluders.md)
is the plan and the live document; this paragraph is its summary and does not
carry the decisions.** What it fixes is the ragged boundary between solids on
neighbouring tiles, its "done when" is that there are no holes, no fringe and no
stair-stepping there, and the broad phase it lands on is a bounding volume
hierarchy rather than the tile grid.

The tile is the *map's* unit and it
has no business in the answer. Light is `∫ visibility × BRDF × falloff`,
visibility is "does this segment meet any primitive", and a primitive is a box
in the world — none of those three sentences contains a tile. The grid exists so
that a ray need not be tested against a city, which is a **broad phase**: it is
allowed to decide *which primitives to ask about* and forbidden to change what
the answer is.

*Most of the pass is already there*, which is what makes this a phase rather
than a rewrite. Positions are world floats (phase 2), normals are world vectors
(phase 2), the cosine and the windowed inverse square are pure functions of two
points, the flame is a sphere and its eight samples are points on it (phase 5),
and `ray_vs_solid` is an exact slab test in world coordinates with no tile
anywhere in it. What is left is five places where a cell is load-bearing, and
each is nameable:

- **A primitive's own coordinates are stored relative to its tile.** ✅ **Done —
  `docs/occluders.md`'s S1.** `occlusion::Solid::box_from_footprint` reconstructed
  a box as `tile + byte/255` on each of four sides, so a primitive **could not
  express a shape wider than one tile**, and its corners were quantised to a
  two-hundred-and-fifty-fifth of one. That was the deepest of the five: it is why
  a wall run is N boxes and a storey's floor is one box a tile, and therefore why
  the silhouette of either is a staircase at tile granularity in any view that
  reads the geometry. A primitive now carries its own six `f32` in a storage
  buffer (`Occlusion::primitive_bytes`, `blit.wesl`'s `Primitive`), and
  `Solid::wire_box` is the whole of what the wire costs. **The four rules below
  are still standing** — the ceiling is lifted, the merge that needs it is S3.
- ~~**`starting_cell`**~~ — bookkeeping about which cell a ray begins in, and
  this document's own backlog already said it is a repair rather than a
  construction. **Gone already, and not by the hierarchy**: S4 deleted it once a
  census showed the case it was written for — a fragment standing strictly
  outside its own carried tile — happens **zero times in any generated or
  rendered scene**, and that the case it still decided 11,544 times, an exact-edge
  tie, has one answer whichever of the two cells a walk starts in. The carried
  tile went with it, off `LitEnd` and off three functions of `blit.wesl` that
  threaded it down to one reader. This step inherits one rule fewer.
- ~~**`same_run`**~~ — a rule stated in cells outright (`cell.x == first.x`).
  **Gone already, and not by the merge**: S4 deleted it once every fixture named
  the solid its fragment is a point of, which left `on_the_lit_surface` — a
  theorem about a box and a plane, needing no cell — answering every case the
  cell arithmetic had. This step inherits one rule fewer.
- ~~**The vertical shortcut**~~ — `solids_at(first)` and nothing else, an
  optimisation that has twice had to grow a footprint gate to stop being a
  *different* answer. **Gone already, and not by the hierarchy**: S4 deleted it
  once a census showed the whole crate enters it **zero times** — a flame is a
  sphere and none of its samples is its centre, so no ray is vertical — and that
  the one thing it did differently was skip every panel, which is a wall a
  fragment inside it was lit straight through. This step inherits one rule fewer.
- **The per-cell `max`** — `stopped = max(stopped, by_surface)` once per cell,
  so that "two panels of one corner are two faces of one wall, crossed once".
  That is a statement about *overlapping boxes for one physical surface*, and it
  is spelled as a statement about a cell.

*The order matters, and the first step is not in this list.* Merging coplanar
neighbours into one primitive is the **prerequisite**, not a tidy-up: it is what
makes the per-cell `max` unnecessary rather than deleted and hoped for — a run of
wall that is one solid has no second face to double-count and no sibling to be
excused from. So: widen a primitive's coordinates off the tile, merge, then delete
the rules that are left, in that order.

*And `same_run` did not wait for it.* Phase 4 measured that identity alone could
not retire it, and that measurement was of a **fixture**, not of the rule: three
places asked the walk about a fragment that named no solid, so the rule that reads
a fragment's own box could not fire and the cell arithmetic was all that was left.
With all three naming one, S4 deleted `same_run` outright — no merge involved.

*Done when:* a walk's answer is a function of the primitives and the segment
alone — gated by equality against brute force over **every** primitive in the
scene, which is the one non-circular oracle shape this tree already has — and
`first` and the per-cell `max` are gone from both walks and from `blit.wesl` —
`same_run`, the vertical shortcut and `starting_cell` already are. `first` is now
a bare `from.floor()`, a cell used as an index and not as a rule, so it goes with
the grid itself at `docs/occluders.md`'s S5 rather than before it.

*What this is not.* It is not about seams between sprites: the grid never had
anything to do with the picture, and phase 6c already made a fragment's shape a
property of its own instance. And it is not a promise about cost — the broad
phase's shape (the same tile index, kept as a candidate list that no rule reads,
against a real bounding-volume hierarchy) is the one decision here that is a
trade rather than a derivation, and `tests/cost.rs` cannot price either today,
since it builds its frame against `Occlusion::EMPTY`.

**Phase 7 — billboards.** Normals for mobiles, chosen by looking at both.
*Done when:* a person standing beside a torch reads as lit from the torch's side,
in a frame a human being has looked at.

*The position half is landed, and it was not in this paragraph.* The phase was
written as a question about the **normal**, and a person looking at a figure
standing next to a lamp reported two things instead: it is lit flat across, and
it carries **horizontal bands**. Both are one cause, and it is the other field —
a mobile has no volume, so the impostor had nothing to meet and the pass fell
back to a *point*, the middle of the tile with the height running down the
picture. That point is the same for **every pixel of a screen row**: nothing
about the light can vary along a row, which is the flatness; and `blit.wesl`'s
`dither` turns the sample pattern by an angle belonging to the position, so one
row gets one turn of the spiral and the next another — an eight-ray estimate,
banded.

So a billboard is a **plane** and no longer a point: vertical, through its tile's
centre, turned towards the camera, and a fragment of it is where its own view ray
meets that plane. `impostor::billboard_at` is the derivation and the shader's
copy is one formula with it; the height it answers with is what the pass already
drew, to the bit, since `Z_PER_TILE / TILE_WIDTH` *is* `1 / Z_STEP`. No choice
was made here — the plane the sprite is drawn on is not a candidate among
several, which is why this half could land without the looking the rest of the
phase needs.

**A static with no box keeps the tile's centre**, and the pass now tells the two
apart by kind. They were one branch and they are not one state: a mobile has no
volume by construction (*"a billboard is no volume, so it casts nothing"*, above)
while a static without one is a **measurement that is missing** — the grid
refused it, or it is a text glyph. `a_floor_spreads_across_its_tile_and_a_wall_
stands_up_it` states why the second must not get a plane and would go red if it
did: what a wall's picture runs along is the world axis the wall is built on, a
screen *diagonal*, and the tiledata does not say which of the two; a billboard's
plane runs along `x - y`, the one direction no wall runs. The same fixture is the
gate for both halves now, and its mobile stanza fails with the branch neutralised.

What is left of the phase is the normal, unchanged: the camera-facing plane
against the silhouette's own inflated field, chosen by looking. The bands are
gone either way; what the normal buys is the torch-side reading the *done when*
asks for.

~~🚩 **And the plane stands at the wrong place while a mobile walks — reported
2026-08-10 as "vertical stripes, in motion".**~~ **Fixed 2026-08-10,
`f41dd86`.** Two expressions for one position, and only one of them moved.
`mobiles::place` puts the sprite's rect at `cell_centre`, the *eased* body
position between the tile it left and the tile it is walking to, snapped to
the eye's own lattice (`docs/camera.md` D11). The `place` word beside it is
`Place::of_mobile(mobile.at)` — the **destination tile, an integer**.
`billboard_at` took the tile and added the fragment's own offset from the
sprite's middle, so it answered *tile centre + the offset from where the
figure is drawn*: standing still the two anchors were the same point and
nothing was wrong, and mid-step they were up to a whole tile apart. The
figure's light was computed for somewhere it was not, and the error slid
smoothly and then snapped when `at` changed — which was the motion in the
report.
<br>
Why it read as *vertical* stripes rather than as a wobble is the plane itself:
a screen **column** of a billboard is one `(x, y)` — only `z` runs down it — so
every shadow boundary crossing a figure is a vertical edge by construction, and
sliding the anchor swept those edges across the sprite. `dither`'s quantum
sharpened them: it hashes the position to a hundred-and-twenty-eighth of a tile,
and one screen pixel across a billboard is `1/44` of a tile, so neighbouring
columns drew unrelated turns of the eight-ray spiral.
<br>
**The fix is one anchor rather than two** — `mobiles::billboard_offset` reads
how far `Mobile::drawn` sits past `Mobile::at`'s tile (a new exact inverse,
`camera::unproject_ground`) and packs it as two fixed-point `i16`s into the
word that was free. **Not `SpriteQuad::owner`, though an earlier draft of this
entry named that one** — `owner` is compared against `OwnerId::NONE` by the
shadow walk's own-run test for every row, mobile included, so it is live.
`twin` is the one a mobile never reads: its stance is always `Upright`, so it
never draws a corner, which is `twin`'s only other job. `impostor.wesl`'s own
`billboard_offset` unpacks the word back and `billboard_at` is handed
`tile + billboard_offset(in.twin)` instead of the bare tile.
<br>
**Gate:** `tests/frame.rs`'s
`a_walking_billboard_is_lit_where_it_is_drawn_not_where_it_is_going`
(`957b8f0`) walks a billboard through five points of one step and checks the
position G-buffer's own `(x, y)` against `camera::unproject_ground(Mobile::
drawn)` computed independently — the same "two spellings compared" shape
`a_billboards_normal_is_the_plane_it_is_drawn_on` already holds the normal to.
Fault-injected back to the bare tile, it fails at up to a whole tile off
(`301.5` vs `300.5` at `left = 1.0`, one step in); with the fix, agreement to
`1e-4` of a tile.

*The camera-facing half is landed, 2026-08-09, and the inflated-silhouette half
is not started.* Before this a mobile fell into the same branch as a static
missing a measurement and read as the zero vector — `blit.wesl`'s own comment
for that value is "lit from every side", and `cosine = 1.0` unconditionally is
what a person's "lit flat across" report was. `impostor::billboard_normal` is
`(1, 1, 0)` normalised, `VIEW`'s horizontal part and the plane's own normal
stated rather than guessed — the same fact `billboard_at`'s own doc comment
already named and nothing had wired into shading. `statics.wesl` now tells a
mobile apart from a static-with-no-box for the *normal* the same way it already
does for the *position*: the first gets the plane's normal, the second keeps the
zero vector, because the second is a genuinely missing measurement and the first
is not. `a_billboards_normal_is_the_plane_it_is_drawn_on` is the packing gate,
the same shape `two_mesh_faces_carry_their_own_two_normals` and
`a_sprite_pixel_meets_the_same_box_on_both_sides` already hold their own
producers to — fault-injected back to the zero vector, red at `60°` off; with
the fix, `0.01°`, `a_direction_survives_the_normal_packing`'s own bound and not
a margin sized to fit. The two sides do not agree bit-for-bit, unlike a cardinal
face: `(1, 1, 0)` sits on the octahedral map's own fold line, where the GPU's and
the CPU's `normalize` land a quantisation step apart on `z` alone (`8.6e-5`,
both reading `0.0` to every digit a person would type) — the angle bound is the
honest comparison for a direction this format does not promise a bit-exact round
trip for, and the cardinal promise is untouched.

**And this is not the phase's own "done when".** The plane's normal is one
vector, the same at every pixel of a mobile's sprite, so a torch to a figure's
left does not read any brighter on the figure's left than a torch to its right
would — only the ordinary falloff-by-distance every fragment already gets
varies at all. That is real progress over "lit from every side" and it is
"never wrong, never interesting" exactly as this document named it above: the
flat, one-cosine-for-the-whole-figure reading is gone, and the *directional*
reading a person beside a torch would actually notice is not bought by this half
alone. Weighing it against the inflated-silhouette candidate — the thing the
*done when* asks for — wants a picture of a real figure beside a real light, and
two things stand between here and one: `examples/isolated_scene.rs`, built for
exactly this kind of check at phase 6, has no mobile pass yet ("a dummy stands
in for it"), and no fixture in the tree runs the full ground-plus-statics-
plus-mobiles-plus-lighting pipeline in one frame the way the real client does.
Both are this phase's own next step, ahead of building the second candidate —
there is no picture to choose between two candidates with yet.

**Phase 8 — the sun.** A direction, the same BRDF, the same rays, sky visibility
as ambient occlusion.

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

## Open questions

Written down rather than guessed at:

1. **How much does exposure have to give back?** Double contrast is a global
   effect and a global exposure may absorb most of it. **Still open, and it now
   has a picture under it rather than a guess.** Phase 3's frames say the loss is
   not global at all: open ground barely moves and a *grazed vertical face* moves
   a great deal, which is the case a global exposure is worst at absorbing. The
   experiment is still one evening; it is no longer inside phase 3, because
   nothing in phase 3 is what a knob would be turned against.
2. ~~**Do statics need per-face albedo?**~~ **Closed by the decree.** A prism's
   four sides sample the same sprite through one projection, so a wall's two
   visible faces carry the art's own two shadings and we multiply both. Flattening
   them per face would be de-lighting through the back door, and the answer is the
   same as to de-lighting itself: not in this renderer. Whatever the sprite says
   is albedo.
3. ~~**Does the ground want normals at all?**~~ **Answered by having them, phase
   3.** UO's terrain is a height field with per-corner heights, so it has real
   normals, and `ground.wesl` writes the bilinear patch's own. It was as close to
   free as the question hoped: the one-torch-on-open-ground pool is barely changed
   from the half-space's, because on level land the normal is `(0, 0, 1)` and a
   flame above it is nearly overhead. What the normal buys is the *slope*, which
   had no lighting at all before and now catches a flame the way the hill it is
   faces it.

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
| [`lighting_raymarch.md`](lighting_raymarch.md) | the DDA walk, CPU/GPU parity, the tile-boundary hazard | **survived phases 4–6 as the walk, and phase 6e retires it.** Phase 4 changed what a hit *means* (identity, no bias) and not how cells are stepped; [`occluders.md`](occluders.md) deletes the stepping itself, and with it the tile-boundary hazard and the corner tie — a hierarchy has no cell to be on the boundary of. What carries over unchanged is `ray_vs_solid`, which was never about cells: it is an exact slab test in world coordinates and it is what the new traversal ends in |
| [`lighting_geometry.md`](lighting_geometry.md) | box → mesh occluders, never started | **cheaper after phase 4**, which makes primitives addressable by id, and **started at phase 6**: a tread is one body rather than two degenerate surfaces, which is the first time the grid's own shape was chosen for what a *view* ray needs as well as a shadow ray. `facing::Blocks` — an authored list of up to four boxes, written and wired to nothing — is where the generic form continues |
| [`lighting_height.md`](lighting_height.md) | the height track: four landed phases and a long backlog | **the backlog is mostly deleted rather than fixed** — see the mapping below |
| [`lighting_reference.md`](lighting_reference.md) | the path tracer, a third opinion with no shared arithmetic | **becomes phase 0**, the oracle everything else is judged by |
| [`gbuffer.md`](gbuffer.md) | the `place` attachment's format, ids, per-face mesh geometry | **phase 2 replaced the format** and inherited every one of its readers. Its open question — how to encode a normal for a non-axis-aligned face — is answered there: an octahedral pair packed as integers into an `R32Uint`, with two bits over for the two answers that are not directions. (`Rg16Snorm`, which this document first named, is not a format wgpu will render to under WebGPU's core set; the plane spent one phase as three floats before it was packed) |
| [`world_coordinates.md`](world_coordinates.md) | a position should carry its own cell; one metric | **half of it is phase 2** (positions as data, `z` in tiles once). The CPU-side type stays its own track |

### What each phase deletes from `lighting_height.md`'s backlog

So that backlog can be read as "work" rather than as a list of things that may or
may not still matter:

| backlog entry | fate |
|---|---|
| ~~`FACE_EDGE`'s two scales; the flame at a surface's own height~~ | **done, phase 3** — there is no band, and a flame in a surface's own plane is a cosine of zero rather than a half |
| `STAND_OFF`/`ON_TOP` at a grazing corner; the `ON_TOP` twin | **done, phase 4** — there is no nudge |
| risers excused as a group; `flame_end`'s height test; a mobile shadowed by its own wall | **done, phase 4** — identity answers all three |
| `own_run` | **survives phase 4, measured** — a run of wall is N statics, which no identity merges. **Retired at phase 6e**, which is where a run *does* become one solid: [`occluders.md`](occluders.md) S3 merges it and S4 deletes the rule, each behind its own measurement |
| the `ground < 1e-6` shortcut ignoring a lid's footprint | **fixed** — it was worth fixing alone, and was |
| `WIDTH_OVERLAP`'s border | **done, phase 6** — there is no second silhouette for a border to reach across |
| the riser penumbra graded over a third of a face | **done, phase 5** — there is no band; a penumbra is eight rays disagreeing |
| the wire's span rounding to nearest; the exact-tangent definition | **phase 4** — a primitive is not a byte range any more |
| `boxes.rs` reading `Unreached` as shadowed; `two_cubes.rs`'s old idiom; the projection idiom stated five times; `mesh::Face`/`facing::Face` colliding | **survive** — instrument work, still worth doing. One of the five spellings of the projection went at phase 6c: `statics.wesl`'s inverse of it is `impostor::ray_from` now, which is a forward ray rather than an unprojection |
| `Occlusion::owner_at`'s linear scan; `selected`/`outlined` stamping `OwnerId::NONE` | **survive**, reshaped by phase 4's ids |
| `tests/cost.rs` measuring three planes of five; `plan::Wall::top` as an `i32`; hand-copies of the third channel | **survive** — the third channel's copies went with the channel, and the other two are still work |

### Wanted after the model works, and deliberately not before

Asked for while this document was being settled, and parked on purpose: each of
them is a *second* answer to "what does a lit frame look like", and a second
answer is only readable once the first one produces a picture worth comparing
against. None of them is a reason to soften a phase above.

- **UO's own light, as a mode you can pick.** The reference client draws light by
  blending sprites from `light.mul`, keyed by `lightidx.mul` and by a light id in
  the tiledata entry — a source's *shape* is a picture, not a radius, which is
  where a window's light patch on the floor comes from. Neither file is read by
  this client at all; `light::flame` is a stand-in of one warm default and a
  wider campfire, and it is the only invention left in the pass. Reading them is
  worth doing on its own — it replaces that function and nothing above it — and
  on top of that, a *native* mode that blends the sprites the way the client does
  instead of shading with ours belongs beside the deferred pipeline as a switch,
  not as a fork. See `lighting_archive.md`'s account of the reference client's
  arrangement, and `docs/client.md`'s own backlog line.
  - Scoped 2026-08-10, not started: `crates/common/uofiles/src/tiledata.rs`
    already parses `TileFlags::LIGHT_SOURCE` (`is_light_source()`, line 211-212),
    but `StaticTiles` (fields at lines 276-319) carries no light-id field — that
    parse is still missing. No reader for `light.mul`/`lightidx.mul` exists
    anywhere in the workspace (confirmed by grep); ClassicUO's
    `IO/Resources`-equivalent (`ClassicUO.Assets/LightsLoader.cs`) is the
    reference: each entry is a small bitmap of 5-bit intensities (values above
    `0x1F` bit-inverted), turned into a greyscale RGB blended additively at a
    fixed *screen* position — no 3D, no occlusion test beyond one binary check
    of the tile diagonally in front of the source
    (`GameScene.AddLight`, `ClassicUO.Client/Game/Scenes/GameScene.cs:402-546`).
    The natural composite point on our side is `crates/client/render/src/blit.rs`,
    where lighting is already applied once on the way to the surface
    (`docs/lighting.md:745-761`); a toggle would follow the existing `App`
    boolean-plus-F-key pattern (`crates/client/app/src/lib.rs:2073-2144` — F10
    night, F8 sunlit, F6 sky field, F7 lantern; F5 is the solids debug overlay,
    next free key is open). Open question not yet decided: whether the mode
    fully replaces the deferred pipeline's shading or composites on top of it.
- **The stylised end, revisited as an experiment.** The dial between a half-space
  and Lambert is deleted from the plan, and the alternatives it came from are
  recorded in `lighting_archive.md`. Once phases 3–6 give frames a person is
  happy with, trying a stylised BRDF against them is a comparison with a baseline,
  which is the only form in which it is worth anything. Not a knob shipped
  half-tuned in the meantime.
- **The circle of transparency** — a radius around the body inside which walls go
  translucent. It is not a lighting feature at all: it is the fifth item of the
  blended pass `docs/client.md`'s "What is still M3" describes, recorded here only
  because it was asked for in the same breath and belongs written down somewhere.

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
- ~~The corner-tie CPU/GPU parity gap, with two `#[ignore]`d tests
  (`lighting_raymarch.md`). Phase 4 does not touch stepping, so it stays.~~
  **It does not outlive the rebuild after all — phase 6e ends it by deleting the
  thing it is about.** A corner tie is two backends disagreeing about which
  *cell* a ray crosses first, and [`occluders.md`](occluders.md) leaves no cells
  to tie. What replaces the claim is stronger and not weaker: the traversal's two
  spellings are gated against one another on a rendered frame, and both against
  brute force over every primitive. The entry stays listed here, struck, because
  "this outlives everything" was said in this document with confidence and the
  correction is worth more than the tidiness of deleting it.
- Nothing runs the tracer over a real map — all four scenes are hand-built boxes,
  and the fifth is hand-built flat ground (`lighting_reference.md`). The
  brightness calibration beside this entry **is done** (phase 0); a real map is
  not, and is now the whole of what is left of it.
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

- ~~🚩 **Nothing gates that an instrument writes what a world pass writes, and the
  field it got wrong is the one no picture shows.**~~ **Built: `tests/attachment.rs`,
  three tests, each fault-injected to red.** The claim it states is the one nobody
  did — *a row that draws a static this frame's grid holds names the occluder the
  grid holds it as* — and what made it statable is that a `plan::Picture` now
  carries the rows it was drawn from (`plan::Named`). A picture could not be asked
  what attachment it came from, which is exactly how `plan::elevation` stamped
  `OwnerId::NONE` into every row for three phases with two green tests over it.

  **The world-pass half is a round trip and not an equality.** `items::collect`
  calls `owner_at` itself, so comparing its row against `owner_at` would be the
  code agreeing with itself; the gate goes the other way instead — take the number
  the row carries, look it up in the grid's own list for that tile, and require the
  solid it lands on to be the very static that was drawn. That is only *reachable*
  on a tile holding two occluders, since on a one-solid tile every number in range
  resolves to the same static. So the fixture is two scenes: the wall run, and
  `storey_over_a_torch`, whose ring tiles carry a wall at `z 0` and another at
  `z WALL_HEIGHT`. The test counts both — rows examined, and rows on an ambiguous
  tile — because a census that examined nothing passes.

  **The two questions, answered.** *`Kind` is enough* to say which writers must
  ask, among the passes that write a G-buffer: `Kind::Static` is exactly that set.
  The two passes that write `Kind::Static` beside a bare `NONE` — `statics::selected`
  and `items::outlined` — are out of the claim rather than exceptions to it, because
  the silhouette pipeline's vertex layout declares no `place` attribute at all
  (`renderer.rs`: *"a silhouette has no hue and no place"*), and a row that reaches
  no attachment cannot be wrong about one. And the ground's `GroundQuad` is the
  **honest exception**: `occlusion::place` is only ever handed statics and ground
  items, so no land tile is ever a solid, and an owner field there could only ever
  hold `NONE` — which is a field a later writer gets wrong for free.

  🔴 **And a third instrument was carrying the defect's shape, unread.**
  `plan::draw`'s `owner_of` was a constant `OwnerId::NONE`, correct only because
  `drawn` asks for an owner where it builds a *face* row and a plan view builds
  none. That is the same sentence as the bug that shipped: a constant that is right
  until something reads it, in a field no pixel shows. It is `unreachable!` now, so
  a plan view that grows a static stops instead of quietly drawing a fragment that
  is a point of nothing. Found by the injection that failed to go red — the first
  version of the plan-view test asserted `owner == NONE` and was a tautology
  against a hardcoded constant.

  **Still not gated, and named so it is not assumed:** `MeshFaceRow::solid`, the
  mesh half of the same join. No built scene has a climbable — `scene.rs` mentions
  neither prism nor climbable — so there is no synthetic route through
  `items::collect` that produces a mesh row at all. It wants a scene before it can
  want a test. `mobiles.rs`, `text.rs` and `gump.rs` are unasserted on purpose:
  their `NONE` is honest by kind, and the claim above does not reach them.
- ~~🚩 **The impostor's *normal* is the whole of what 6c did to a sprite's
  shading, and it was measured by injection rather than argued.**~~ **Done
  2026-08-11**, by the candidate this entry itself named. The record, then the
  answer and what it cost.

  A person
  reported a static reading darker and striped where it used to be even, and
  three frames of one place (Britain `(1497, 1627)`, `View::Light`, a lamp post
  added by hand) say which half of 6c it is: the commit before 6c draws the post
  fully lit and the flight beside it in broad even bands; HEAD draws the post
  black and the flight cut into dark stripes; and HEAD **with `best.normal`
  forced to the zero vector and the position left alone** is the pre-6c picture
  again. So the position half is innocent and the cosine against a box's
  camera-facing face is the whole difference. What that face *is* is worth
  stating plainly: `Meeting::normal` is always one of `+x`, `+y`, `+z`, so every
  sprite fragment claims to look towards the camera, and a flame standing behind
  that plane — including a lamp's own, inside its own box — reads `N · L ≤ 0`.
  `View::Normal` over the same place shows it directly: a lamp post's pole comes
  out split down the middle into a green half and a red one, which is a whole
  tile's box answering for a picture of a thin pole. **The black emitter entry
  above and this are one finding**, and the candidate this suggests and nobody
  has measured is that a *body* — `edges == EDGE_MASK`, the box for a graphic
  whose art would not name a side — should write **no facing** rather than a
  face, keeping the measured normal for the panels and lids where the art really
  does say which way a surface looks. It is a different answer from the three at
  the black-emitter entry and should be judged beside them, not instead.

  **That is what landed.** `impostor::Volume` carries the box's own `Edges` now
  — free, in the alignment word after `lo` that this side had to write anyway,
  beside the `solid` that already rode in the one after `hi` — and `statics.wesl`
  writes `pack_normal(vec3(0.0))` where the mask is `EDGES_ANY`. The **stance**
  is still taken from the met face, deliberately: it names which face of which
  box the ray left by, which `blit.wesl`'s `on_the_lit_surface` and `own_solid`
  read as geometry, and that is a fact whether or not the art drew a plane
  there. The facing is the only thing this refuses to claim.

  *Why it is not an exemption.* It is a measurement that was never taken, said
  so — `normal_format.wesl`'s middle state, the one `blit.wesl` lights from
  every side. The same sentence is already written one module over:
  `light::mounted_at` refuses to move a flame off an `Edges::ANY` cell because
  "there is no direction in it to move along and a guess would be a wrong one".
  The impostor was making exactly that guess, one pass along.

  *Measured, on the frame the defect was reported from* — `isolated_scene` at
  Britain `(1497, 1626, 10)`, radius 6, a lamp post by hand, `640×480`,
  `View::Lit`, against the same frame with `EDGES_ANY` set to a mask no box can
  carry (which is the injection, and it is also the positive control below):
  **31,375 of 307,200 pixels change, 10.21% of the frame**, worst channel step
  `168` of `255`. On the one-item scene the entry above reproduces at, over the
  8,064 pixels the lamp's own picture covers: the mean brightest channel goes
  `4.3 → 17.5`, and the share at or under `8` of `255` — black, to a person
  looking — goes **82.7% → 39.1%**. The pictures either side are the acceptance
  instrument and they say it plainly: a black silhouette with a green wick
  becomes a lit lamp, and the flight of steps beside it stops being striped.

  *What holds it.* Three gates, each red under the injection above.
  `grids.rs`'s `the_statics_pass_knows_which_mask_means_the_art_named_no_side`
  pins `EDGES_ANY` from the shader's own source (`docs/pixels.md` rule 6).
  `frame.rs`'s `a_sprite_pixel_meets_the_same_box_on_both_sides` now sweeps a
  fixture whose masks are the ones the grid would hand those shapes — two treads
  and a lid — and asks both halves of the rule of every texel of the quad, with
  the *face* still compared everywhere through the stance so the CPU-against-GPU
  claim about `meets`'s arithmetic is untouched. And
  `a_sprite_fragment_is_a_point_of_the_primitive_it_names` reads its axis out of
  the stance rather than the normal, and counts both populations, so a scene
  that drifted to all bodies or to none could not pass it by never reaching it.

  *What it does not fix, named so nothing claims it.* A body is lit from every
  side, so a crate has no shading across its own faces — that is the pre-6c
  picture for exactly the set 6c had no measurement for, and what would improve
  on it is a *measured* facing, not a better guess. **A climbable's tread is
  swept up with them and should not be**: `boxes_of` hands a tread `Edges::ANY`
  to pick an occlusion test, and a tread's lid and its riser are planes the art
  did draw — one field with two domains, filed and measured in its own entry
  above ("A climbable's tread is marked a body"). And the emitter's own
  remaining darkness is `FLAME_LIFT`: half a tile, whatever the sprite's height,
  so a lamp post's pool is centred at its **foot** and its head — nineteen `z`
  up — takes the far end of an inverse square. See the entry below.
- 🟡 **A flame burns half a tile up whatever it is standing in, so a tall
  emitter's own head is the dimmest part of it.** `FLAME_LIFT` is `Z_PER_TILE /
  2`, and its doc argues the number honestly — a brazier's flame is about there,
  and "the sprite's real height is not available here, and asking the atlas for
  it would tie the lights to whether this frame's art happened to be packed".
  What the fix above made visible is the cost on a *tall* one: a lamp post's
  picture is seventy-six pixels of sprite, nineteen `z` units, and its lantern is
  at the top of it while the flame burns at `5.5`. On the one-item scene the pool
  is plainly centred on the post's **foot** — the base is the brightest thing in
  the frame and the lantern head takes the far end of an inverse square. It is
  not a defect in the model and it is not the black emitter: it is a light placed
  from the map's `z` alone, on a sprite whose height nothing in `light::gather`
  can see. The two candidates are the ones the entry above rejected *for that
  defect* and which are still live for this one: read the flame's own height off
  the art (which wants the atlas in the light collector, and is the same
  measurement `MOUNTED_CLEARANCE`'s doc asks for), or off `calc_height`, which is
  in reach today and is the item's own height rather than its flame's.
- **"Vertical steps along the tiles" is reported and not reproduced.** Named by a
  person at Britain `(1459, 1693)` beside the ragged silhouette above, in the
  live client. `examples/isolated_scene` at that place, with a lamp post added by
  hand and every knob at its default, does not draw them: the wall's lit face is
  smooth across its tile boundaries and the ground has no tile-shaped step in it
  at all. What differs between the two pictures is what the tool cannot yet
  build: the client's carried lantern is a **beam**, its ambient may have the
  **sky field** on — which is per tile and interpolated nowhere, the first
  candidate for anything tile-shaped — and the knobs may be off their defaults.
  Pinning it wants the client's own `View::Normal` of the same frame, which
  separates a geometry answer from a walk answer, plus the tab's numbers.
- 🟡 **Light comes through a floor at the *corner points* between its tiles, and
  it is one line of `primitive_stopped`.** **The hole is closed and the report is
  not explained** — the two halves came apart when the fix was measured, and
  what follows the original entry says which is which. Seen from under a ceiling
  at
  Britain's `(1492, 1642)`, `z 28` under statics at `z 40` and `z 23`: a regular
  lattice of bright dots, one per tile **corner**, and nothing along the joins
  between them. The lattice is the tell — a leak along a join is an interval
  problem and a leak at a join's *point* is a degenerate one.
  <br>
  The rule is read rather than guessed. `primitive_stopped`'s lid arm asks
  `ray_vs_solid` for the run the ray spends inside the lid's **horizontal
  footprint** (an infinite-`z` box) and hands `crosses` the `z` at the two ends
  of that run. A ray threading the exact corner of the footprint enters and
  leaves it at one `t`, so those two `z`s are **the same number** — and
  `crosses`, which asks whether the ray went from one side of the lid's plane to
  the other, correctly answers "it did not", because over an interval of zero
  length nothing travels anywhere. Every one of the four lids sharing that
  corner answers the same way, so the point is a hole through a continuous
  floor. Along a join *line* the ray still crosses the interior of one footprint
  on the other axis, `entered < leaves`, and it is caught — which is exactly the
  shape a person sees.
  <br>
  **The fix is to state the lid rule directly rather than as an interval.** A lid
  is a plane: pierce it once — `t = (lid_z − from.z) / delta.z` — and ask whether
  `(x(t), y(t))` is inside the footprint, inclusively. At a corner that test says
  *yes*, which is the honest answer for a point interior to the floor as a whole;
  a ray running along the lid's own plane has no `t` in `(0, 1)` and stays
  unblocked, which is the strictness `crosses`'s doc argues for (a candle
  standing on the floor it lights). It is also **smaller** than what it replaces:
  one intersection instead of a slab test plus a crossing test, and the
  `-1.0e6`/`1.0e6` sentinel box goes with it. `light::crosses` is the CPU twin
  and moves with it; the gates are `tests/lighting.rs`'s floor scenes and
  `scene::storey_over_a_torch`, which is the fixture the *opposite* defect (a
  floor stopping nothing at all) was found on.
  <br>
  **Done 2026-08-10, and smaller still than the entry proposed.** The pierce
  does not have to be computed: `primitive_stopped` has already run
  `ray_vs_solid` against the lid's own box — footprint *and* `z` span — and
  returned early when it missed, so *did the ray meet this lid* is answered
  before the arm begins. What was left was *did it get from one side to the
  other*, and that is `crosses` over the **segment's own two ends**. One line in
  each twin, the sentinel box gone from both. The gate is
  `light::tests::a_ray_through_the_point_four_floor_tiles_share_is_stopped_by_
  them` — four floor tiles of four graphics (a merged floor is one primitive
  with no interior corner to leak at), a fragment over one and a flame under the
  one diagonally opposite, so the segment's midpoint *is* the shared corner.
  Confirmed to have teeth by fault injection: the old arm makes it read
  `streaming 1, exact 1` where it now reads `0`.
  <br>
  🚩 **What is not confirmed is that this is what the person saw**, and the
  measurements say so plainly. A sweep of 40,000 fragments across four tiles of
  a floor standing in the shadow of a storey ten `z` above it, at
  `FLAME_RADIUS`, leaks **nothing** — under the old arm as much as the new one,
  which is why that sweep is not in the tree: a gate that cannot fail is worse
  than none. And `examples/isolated_scene` at `1492,1642,28`, with a flame added
  above the `z 40` lid, renders **byte-identical** in `View::Light` under the two
  arms. Both readings say the same thing: the corner case is a set of measure
  zero for an ordinary ray, so it cannot on its own paint one dot per corner over
  a floor. Either the lattice has a second cause — the impostor naming one of
  four coplanar lids at a fragment that sits exactly on their shared corner is
  the nearest candidate, and it is a 6f-shaped question — or the arrangement that
  produces it is not the one reconstructed here. **What the next attempt needs
  is the frame**: the camera and the light the person actually had, since the
  coordinates alone did not rebuild it.
  <br>
  **And that is what it was — the impostor, not the walk.** The person reported
  it a second time with the tile they were standing on (`1492, 1643`, stand
  `z 20`), `isolated_scene` there reproduced the lattice at once, and reading the
  G-buffer settled it in one row: at a bright pixel `View::Shadow` and
  `View::Height` read **exactly what its neighbours read** and `View::Normal`
  does not. The dots are not lit more; they are *facing* differently. Fourteen
  of them, on a lattice of exactly `TILE_WIDTH` — one per tile corner — each
  carrying a side face's normal on a surface whose every other pixel carries
  `+z`, at `z ≈ 40`: a lid. A wall's cosine in the middle of a roof. See
  `impostor::meets`.
- 🚩 **A corner still lights up where a lid meets a side face, and it is the
  *shadow* term there rather than the face.** The same person, same roof, two
  tiles over: `1510, 1636` and `1490, 1636`, stand `z 20`, ceiling `z 40`, the
  roof `0x051C`. Reproduced in `isolated_scene` at both — two or three clusters
  of three to six pixels, not the one-per-corner lattice the entry above was.
  <br>
  A nine-by-nine window of the G-buffer round the brightest one reads like this,
  and it is the whole diagnosis: the lower half is `+z` and lit and *dark*
  (a lid facing up, with the cosine giving it nothing), the upper half is `+x`
  or `+y` and **shadowed**, and along the seam between the two there is a notch
  of five pixels that are `+x`/`+y` and **lit** — a side face's cosine, at full
  flame, against neighbours of the same normal and the same `z` that the walk
  calls shadowed. So it is not which face was met: the face is the same as its
  neighbours', and the walk answers differently for it.
  <br>
  What that leaves is the exemption. `on_the_lit_surface` releases a candidate
  whose extent along the fragment's own normal axis **ends** on the fragment's
  plane, and a seam is exactly where a lid's edge and a wall's face share a
  coordinate — so a fragment there is released from the very primitive that
  shadows its neighbours. Measured at the other spot: the bright pixel names a
  different solid from the pixels below it (`0` against `15` and `26`), which is
  the same sentence from the other end.
  <br>
  **What it needs is the instrument this class keeps asking for and the tree
  does not have: which primitive a pixel names, as a picture.** Four defects on
  this track have now been "the fragment names the wrong box" (6f, 6h, the lid
  face, this), and each was diagnosed by hand-decoding the position plane's
  fourth channel through a throwaway shader edit. A `View::Solid` beside
  `View::Normal` — the id hashed to a colour — plus a per-pixel *who stopped the
  ray* probe would have made each of them minutes' work.
  <br>
  ✅ **`View::Solid` landed, and it answered on its first frame.** The whole
  `+x`/`+y` region above the seam is a point of **no primitive at all**, while
  the `+z` surface below it names one. The chain from there is short and every
  link is in the tree already:
  <br>
  `occlusion::opacity` reads a graphic's own flags — `NO_SHOOT` is opaque,
  `WINDOW` is a pane, **everything else is `CLEAR`** — and `Builder::add`
  returns without pushing anything at all for a `CLEAR` one. So this roof piece
  stands nothing in the grid: it is not geometry, it stops no light, and
  `Occlusion::id_of` has no name to give it. `statics::push_volumes` still hands
  the impostor a `boxes_of` box for it — that is 6c's deliberate fallback, since
  the grid refuses about half of Britain's drawn pictures and the alternative is
  a billboard — so the fragment gets a measured position and a measured *face*
  while being a point of nothing. And `boxes_of` reads a picture with no `FLOOR`
  flag through `edges_of`, as a **wall**: side faces, at the tile boundary, at
  the very height the roof lies at.
  <br>
  So the glow is three facts meeting. The pixels are the picture's overhang past
  that box — a *miss*, taking the nearest face, which along a silhouette is a
  side one. A side face has a real cosine towards a flame the roof's own lid has
  none of. And being a point of nothing they are exempt from nothing, so which
  of them the neighbouring lid shadows is decided by where the clamp put them on
  its edge — five pixels clear it and blaze.
  <br>
  **And the client's own files name the pieces**, which settles what is a defect
  here and what is data. `tile_probe` on the tiles round the glow:
  <br>
  - `0x051C` "stone pavers", `z 40`, `FLOOR|NO_SHOOT|PLATFORM` — the surface the
    person calls the roof. A lid, opaque, **in the grid**: that is the `+z`
    region, naming its solid.
  - `0x00C8`/`0x00C9` "stone wall", `z 20`, height 20, `WALL|NO_SHOOT|BLOCK` —
    the wall under it, in the grid.
  - `0x00DD`/`0x00DE` "stone wall", `z 40` and `z 43`, height 3, `WALL|BLOCK`
    and **no `NO_SHOOT`** — the wall's top course, standing at exactly the
    pavers' own height. `occlusion::opacity` reads that as `CLEAR` and
    `Builder::add` returns without pushing anything, so it is the piece that is
    a point of nothing.
  <br>
  So it is a **cornice, not a roof**, and it is `WALL`-flagged: reading it as a
  wall is right, and the side faces are its own. Asking `is_roof()` in
  `boxes_of` was tried against this frame and moved no pixel — the note stands
  in that function, since the header does claim a roof is a lid and the next
  person will think of it too.
  <br>
  What is left is the **fringe**, and this is the case that decides its open
  item above. The lit pixels are the picture overhanging its own box: they take
  the nearest face — which along a silhouette is whichever, and here is a side
  one with a real cosine — and they are clamped onto the box's *edge*, which is
  exactly where the neighbouring lid's shadow boundary runs, so a few of them
  land clear of it and blaze. Being a point of nothing they cannot be excused by
  identity either.
  <br>
  ✅ **And the person who reported it named the shape of the answer: give a
  floor real bounds** — done 2026-08-10, `docs/parity.md`'s **P4.1**, which
  carries what landed and the one thing this paragraph got wrong (the
  thickness: a `z` unit puts the top of every interior wall under a storey into
  shadow, measured, so `occlusion::LID_THICKNESS` is `1/64` and argued from the
  wire's resolution and the screen's instead). A lid was the one primitive in
  this grid that was a
  *plane* — `min.z == max.z` — and every defect on this list that involves a
  floor is a consequence of that degeneracy rather than of any one rule: the
  corner leak (an interval of no length), the strictness `crosses` needs (a
  candle on the floor it lights), the fragment sitting exactly *in* the plane
  and so on neither side of it, and `meets` having to be told that a lid's side
  faces are lines. A floor a `z` unit thick is a body like every other, and each
  of those dissolves rather than being ruled about: a ray from its top going up
  never enters it, a ray from its top going down does, and its faces have area.
  <br>
  What it touches, and what has to be measured before it lands: `Solid::box_of`
  gives a lid `bottom..top` today and would give it `bottom - 1 .. top`, so
  every floor in the world moves; the walk's whole `Edges::NONE` arm — and
  `crosses` with it — becomes a body's `opacity` outright; `occlusion::merge`
  starts folding floors as bodies; and `impostor::meets`'s "a lid has no side
  face" guard becomes dead. The gates that decide it are `tests/lighting.rs`'s
  floor scenes, `scene::storey_over_a_torch`, and the traced suite — a storey's
  floor is the fixture that catches both directions, and the *thickness* is the
  one number to justify rather than pick: a `z` unit is what `Z_STEP` calls one
  step of height, and a floor thinner than the quantum its own height is stated
  in is a floor the wire cannot describe.
  <br>
  Three ways out of the fringe, and the zero vector is not one of them: `blit.wesl` shades a
  fragment with no facing as *lit from every side*, so a fringe given none comes
  out brighter still. What is left is to give a **miss** the face the sprite's
  own volume presents rather than the nearest one (uniform along a silhouette,
  and for a panel it is the panel's own side), or to stop drawing geometry the
  grid holds nothing for — which 6c already refused, and rightly: the fallback
  is what keeps half of Britain's pictures off billboards.
- 🚩 **A wall run built of several graphics shows the same "lid at the seam"
  glow as the cornice above, and it is not that case — these statics *are* in
  the grid.** Reported live in `openshard-playground`, not yet a checked-in
  fixture: Britain, `(1507, 1656)`–`(1507, 1662)`, an upper-storey brick wall
  standing on the tile's `East` edge, three (and more) consecutive tiles each a
  different graphic — `0x0038`, `0x0035`, a window at `0x003C` — all
  `WALL|NO_SHOOT|BLOCK`, so `Builder::add` pushes a real panel for every one of
  them. None of this run is the "point of nothing" the entry above is about.
  `View::Normal` at the seam between two of these tiles reads a `+z` (lid) band,
  roughly 20–40 screen pixels wide at `zoom` 4–6, cut into the *middle* of an
  otherwise uniform `+x` face — not a silhouette edge, since the `+x` colour
  returns on both sides of it. Reproduces in `isolated_scene`
  (`OPENSHARD_SCENE_AT=1507,1660,27`) too, measured pixel by pixel, so it is not
  particular to the live client's own scene assembly.
  <br>
  **Ruled out this session, each checked rather than assumed.** The two panels'
  own end faces (`Solid::box_of`'s outer plane) are bit-for-bit equal — in `f64` and
  after the `f32` wire round-trip, verified numerically for the real tile
  coordinates: no rounding anywhere in the box's own extent, on either axis.
  The defect reproduces with **no active flame** anywhere in the scene
  (`View::Shadow`/`View::Light`/`View::Sun` all flat there), so it is not
  `on_the_lit_surface` or the `lit.solid == Some(id)` exemption — there is no
  shadow ray for either to get wrong. `sample_count` is `1` everywhere in
  `renderer.rs`/`gbuffer.rs`, no MSAA anywhere to blend a normal across an edge.
  `depth::base_for` is `x + y`, symmetric in the two axes. The saved world's
  `decorations`/`items` tables hold nothing near this tile.
  <br>
  **Reported direction-specific, and still unexplained.** Seams along a run's
  `y` — a tile's `East`/`West` panel, thin in `x` — show it; seams along a
  run's `x` — `North`/`South`, thin in `y` — do not, on the same building.
  Nothing read this session in `Solid::box_of`, `lit_plane` or `depth::base_for`
  treats the two axes differently, so the asymmetry itself is still open.
  **Not yet checked:** the corner case's own guarantee does not obviously reach
  this one — `Facing::Corner`'s designed `PANEL_THICKNESS`-square overlap is
  real, but it is for *one* static naming two edges; `boxes_of`'s plain per-tile
  push has no stated equivalent for *two different* statics meeting across a
  tile boundary, and whether that gap is real was not settled either way. Nor
  is the selection `statics.wesl` runs between *different* static instances
  competing for one screen pixel — this session never opened that shader.
  <br>
  **It does not reproduce in `isolated_scene`, and the reason is that the tool
  and the client do not have the same primitives at the same place.** Measured
  rather than argued, at `1507,1660` and again at `1505,1653`: over a bare pair
  of abutting panels, over the four-panel run, and over the whole building with
  everything the tiles really hold, the `+x` face is continuous across every `y`
  seam and every `+y` run is `4` screen pixels — `PANEL_THICKNESS`'s own `0.2`
  of a tile, an honest end face at a free end. The one-pixel runs a census does
  find are all wedge *tips*: a lid emerging from behind a wall, one pixel on the
  first row and three on the next. Nothing a person would call a stripe.
  <br>
  What is not shared with the client is the **partition**. `merge::merged` runs
  in `Builder::finish` over the *frame's rectangle*, so which pieces of a run
  fold into one primitive is a fact about what else got into the picture. Same
  camera, same place, radius `4` against radius `16`: **132 of 83,830 adjacent
  pixel pairs change their answer to "one primitive or two"** — seams appear and
  vanish with the rectangle. And `statics::push_volumes` takes the *grid's*
  merged box wherever the grid names the piece, so this reaches the normal plane
  by construction. A defect that lives on a seam therefore cannot be reproduced
  by pointing the tool at the coordinates: the seam is not there to hit.
  <br>
  **Ruled out this session.** The client's F10 is, as far as `View::Normal` is
  concerned, exactly "meet the sprites against the grid or against nothing" —
  with the lights off `App::draw` never calls `light::collect`, so
  `statics::collect` gets an empty grid and every fragment takes
  `statics.wesl`'s billboard fallback. That switch is now
  `OPENSHARD_SCENE_IMPOSTOR=0` in `isolated_scene`, and at both places above the
  two `View::Normal` dumps are **equal to the pixel** (0 of 691,200) while
  `View::Solid` goes from 87 primitives to none: the merged box and the per-tile
  box agree everywhere a fragment lands *there*. The **bake** is ruled out too —
  the client passes `Some(&mut self.occlusion_bake)` and the tool passes `None`,
  and the only oracles for it held one rectangle still, which is the one state a
  cache is never asked about. `a_baked_grid_is_the_one_the_walk_builds_after_the_camera_moves`
  now walks the rectangle across the town a tile a frame and the baked grid is
  the walked one at every step.
  <br>
  **The rectangle is exhausted, measured.** Growing what the tool is given of
  the map settles the partition: radius `16` against `24` moves 36 of 205,148
  adjacent pairs, and radius `24` against `40` moves **none of 207,415**. The
  stripe census is identical at all three. So handing the tool the whole map
  changes nothing at this place, and "the client has more in frame" is not the
  difference.
  <br>
  **The anchor is a real one-pixel difference, and it is now expressible.**
  `OPENSHARD_SCENE_ANCHOR_REAL=1` builds the scene where the map has it instead
  of translating it next to the synthetic origin. Same data, same camera — the
  anchor delta is a whole number of tiles, so the projection moves an exact whole
  number of screen pixels and the framing is bit-identical — and **760 pixels
  come out different, 746 of them one-pixel runs**: 514 that were a wall's own
  face at `(100,100)` are a lid's `+z` at `(1507,1660)`. That is the arithmetic
  and nothing else, and it is exactly the width the reported defect has. What it
  does *not* do at this place is put a new stripe in the middle of a uniform
  face: the wedged-run census barely moves (25 → 24), so what the anchor buys
  here is edges landing one pixel over, not the reported band.
  <br>
  **And nothing loses its facing.** Four places — `1505,1653`, `1507,1660`,
  `1490,1636`, `1497,1626` — crossed with both the anchor and the grid: **zero**
  pixels of 691,200 come out with no facing in any of the sixteen frames. So the
  "faces disappear when the light goes on" a person sees is not a fragment left
  without an answer, at least nowhere this tool has been pointed. The grid moves
  the count by nothing at three of the four and by thirteen wedged runs at
  `1497,1626`, which is the one place worth going back to with a picture.
  <br>
  **And then the person showed a picture, and the thing they were reporting was
  never a stripe.** At Britain's `(1501, 1659)` — a counter (`0x0B40`,
  `BLOCK|PLATFORM`, height 6) and boards on the tile, shingles overhead — what a
  `View::Normal` crop holds is isolated **specks**: single pixels carrying a side
  face's normal with lid on all four sides of each, spaced **exactly
  `TILE_WIDTH`** apart. It reproduces in `isolated_scene` at once — seven of
  them, two naming no primitive at all and every one naming a primitive its
  surroundings do not — and it reproduces **identically at both anchors**, so it
  was in every frame this session had already drawn.
  <br>
  **The measurement was wrong, not the renders.** Every census run above counted
  *runs*: a foreign face wedged between two of the same, run-length one. A speck
  has no run — its neighbours along the row are lid on both sides only because
  the neighbours in the column are too — so a run-length detector reports zero on
  a frame full of them. `docs/style.md`'s own moral, from the other end: a
  detector must be able to say what it counted, and this one was blind to the
  shape it was hunting.
  <br>
  **Where it is not.** `examples/speck_probe.rs` sweeps a body's whole top face
  through `impostor::meets` at the sub-pixel step the projection actually puts
  fragments on — corners inclusive, since the tie rule is written for them — and
  **0 of 7921** samples come back a side face, at Britain's magnitude and near
  the origin alike. So the face choice for one box is not it, and neither is the
  `hi.z > lo.z` guard, which covers a lid and was never about a body. What is
  left is which *box* the fragment is given: each speck names a different
  primitive from everything around it, which is `impostor::nearest` picking
  another of the static's own volumes — or `push_volumes` handing it a list the
  sprite is not a picture of. That is the next thing to instrument, and it wants
  the real `boxes_of` for `0x0B40`/`0x0B01` rather than a synthetic pair.
  <br>
  **Cut the roof and the count goes from 7 to 66**, which is what makes this a
  reproduction rather than a sighting. `OPENSHARD_SCENE_NO_ROOFS=1` is now the
  tool's own cutaway — the third difference with the client, and the state a
  player standing indoors is actually in. Under the roof the specks stop being
  scattered and line up: **dashes of four, running along the diagonal a tile
  boundary projects to**, and each pixel of a dash sits on one floor slab while
  naming the **neighbouring** one (`here (69,55,46)`, `around (131,88,61)`, and
  the pair steps to the next tile with the next dash). Thirty-two of the
  sixty-six are a point of no primitive at all.
  <br>
  So the surface is a floor of abutting lids, the line is the seam between two of
  them, and it is one pixel wide because the seam is a shared plane and the
  fragment on it is answered by whichever slab wins the tie. That is the whole
  report, and it is the same sentence as the reporter's first picture: a short
  red stroke on the join between a green face and the blue above it.
  <br>
  ✅ **And the cause, measured end to end by `examples/seam_probe.rs`** — which
  prints, for the real graphics on the real tiles, the box each static stands and
  whether it has height. At `(1501, 1659)`:
  <br>
  - `0x04AC` "wooden boards", `FLOOR|NO_SHOOT|PLATFORM`, box `z 27.0..27.0` — a
    **lid**. `meets`'s `hi.z > lo.z` guard means a fragment of it *cannot* come
    back `+x` or `+y`. So the dashed line is not the floor's own pixels, and the
    whole "which slab wins the tie at the shared plane" reading above is wrong.
  - `0x0B01` `BLOCK|PLATFORM` at `z 27.0..30.0`, `0x0AFE` at `z 30.0..35.0`,
    `0x0E29` at `z 30.0..31.0` — the furniture standing *on* that floor. Every
    one of them has height, and every one is **`opacity 0`, `CLEAR`**: the same
    "point of nothing" the cornice entry above is about. `Builder::add` pushes
    nothing, `id_of` has no name — which is exactly the 32 of 66 specks that name
    no primitive at all — and `push_volumes` hands the impostor a `boxes_of` box
    regardless.
  <br>
  So the speck is a pixel of a *piece of furniture's* sprite overhanging its own
  box: a **miss**, clamped to the nearest point on the box, and along a
  silhouette the nearest face is a side one. It lands on the floor because that
  is what the sprite overhangs onto, and it repeats on a lattice of one tile
  because the boxes are per tile and the furniture tiles across the room.
  <br>
  ❌ **And that is wrong for 59 of the 66, measured by doing it.** A throwaway
  `if !hit(best) { discard; }` in `statics.wesl` — the whole of "just do not draw
  the fringe" — takes the count from **66 to 59**. So only seven of them are
  overhang misses. The rest are *hits*: the ray genuinely enters the box (or
  grazes it inside `TANGENT`) and leaves through a side face, which is a correct
  answer about the box and a wrong one about the picture.
  <br>
  What the other planes say about them, at the same pixels: the height plane
  differs from the four neighbours' only in the low bits of one channel — the
  same surface, not a different body — and only **2 of 66** have all four
  neighbours naming *one* primitive, so a speck sits on a boundary between
  primitives rather than marooned inside one. They come in dashes of four along
  the direction a tile's own `y` edge projects to.
  <br>
  The reading that fits all of it, and the part that is still inference: a
  sprite is 44 pixels wide and **overhangs its neighbours' tiles**, so the pixel
  drawn over the boundary belongs to a static whose box is a tile away; its ray
  enters that box near the edge and exits through the side. `nearest` only ever
  sees one static's own volumes, so no choice between neighbours is involved —
  which means the fix is not in the selection but in what a box is. It is the
  accepted cost this document already states — *"statics without a good prism get
  a rougher volume"* — read back as a lattice, and clipping the sprite does not
  touch it.
  <br>
  **Which makes seven of them the fringe, not a new defect** — the same open item the
  cornice case ends on, and **not with the same three ways out**, which this
  paragraph got wrong when it was written: the cornice entry names its own
  candidates "the zero vector is not one of them", ruling out a no-facing miss
  by reasoning alone — a fringe with no facing is lit from every side, which
  makes a *blaze* brighter, not fainter. What is actually open there is two,
  not three: keep the clamp, or give a miss the face the sprite's own volume
  presents. No-facing is a live, unmeasured candidate for a *different*
  backlog item (the serrated-edge entry below), not for this one — and
  `statics.wesl`'s own history ("One silhouette", read at
  `docs/parity.md`'s P4 step 2) is a third, prior data point again: giving
  every miss no facing, tried globally rather than scoped to either of these
  cases, measured a worse artefact (a lattice of lit dots across every floor
  and roof seam) and was reverted. Three mentions of the same shape of fix
  reaching three different verdicts is itself worth having written down.
  What this session adds beyond that correction is that the fringe is not a
  rare corner: it is a dashed line across every floor a person stands on
  indoors, and the pieces making it are `CLEAR` ones the grid holds nothing
  for, so no identity can excuse them either.
  <br>
  **What is left of the difference, ranked, and none of it measured yet:** the
  frame's rectangle and the live `Cutaway` (both feed the partition above); the
  **anchor** — the tool translates the place onto `SYN_ANCHOR (100,100)` while
  the client works at `(1507,1660)`, and `Solid::space` is absolute, so an `f32`
  ulp there is some sixteen times the one here and any tie on a shared plane is
  decided at a different precision in the two; the **atlas**, which has grown all
  session in the client and holds a screen's worth here, and which is where
  `boxes_of`'s `Shape` comes from; the **clocks** (`0.0` and
  `StaticAnimations::default()` against advancing ones, so an animated static is
  a different picture with a different box); the server's **ground items**; and
  the camera, which follows a walking player at smoothed sub-tile positions
  rather than sitting on a tile anchor.
  <br>
  🚩 **And the dash has moved off the floor onto the furniture's own top**
  — reported 2026-08-10 by a person looking at a client F12 dump (eye tile
  `(1496, 1659)`, `1919x2077`, night, a torch in hand), read back off the planes
  rather than argued: along an alchemist's counter, one stepped dashed line per
  tile boundary, and the line is on the counter's *lid* rather than on the floor
  beside it. Per pixel of a dash, against its own neighbours two rows away:
  `kind` static in all three, `height` the same `z 33` surface on both sides of
  the line, `shadow` white and `reach` unchanged — and `normal` flips from the
  lid `(0,0,1)` to a **side** `(1,0,0)` while `flames` goes from `(19,9,2)` to
  `(255,167,37)`. The light did not change; the facing did, and a vertical face
  turned at the torch takes a full cosine where the lid took a grazing one.
  Frame-wide, the signature — a static fragment whose normal is a side face with
  a lid two rows above *and* below — is **464 pixels, 442 of them with the
  sub-tile position pegged exactly to a tile edge**, 87 of them with the flame
  term blown out, which is the part a person sees.
  <br>
  **Why the floor's cure does not reach it.** `shows_a_side` refuses a face
  thinner than the grid that reads it, and that ends this for a floor because a
  floor is a lid — `LID_THICKNESS`, a sixteenth of a pixel of side. A counter is
  a *body*: its side face is several `z` tall, passes the same test honestly, and
  is what the graze at the top edge is handed. So the two halves of that repair
  (`FRAGMENT` for the seam, `shows_a_side` for the face) covered the population
  they were measured on and left the abutting-body case standing.
  <br>
  ✅ **Hit, not miss — settled by looking, once F2 could be believed.** The
  switch had to be repaired first (`docs/lighting_state.md`'s fringe entry: the
  silhouette pass was overwriting it inside the frame), and with all three states
  reaching the screen the reporter's answer was *the picture changes and the
  seams do not*. So the fringe is not this, on a lid, the way it was not this for
  59 of the 66 specks on a floor.
  <br>
  ✅ **And it is the box's own top edge, decided by a rounding the grid cannot
  show. `impostor::RIM`, landed 2026-08-11.** `meets` picks the face whose exit
  comes first; along the line where a body's lid ends and its side begins, the
  side's exit comes first by *less than the distance to the next sample*. Those
  fragments are one row wide, and one row along a projected diagonal is a stepped
  dashed line — which is what the reporter drew a finger along. The rule is the
  [`FRAGMENT`] argument a third time, after the hit tolerance and after
  `shows_a_side`: **a side wins only by more than the picture can show.**
  <br>
  It was nearly refused on a misread of its own probe, and the misread is worth
  keeping because it is a shape of mistake rather than a slip. Over a body of the
  shape `seam_probe` prints for this furniture (one tile, five `z` units), across
  its own sprite's samples: 1,010 fragments answered with a side face, the gap to
  the lid's own exit running 0.000 to 0.827 tiles against a `FRAGMENT` of 0.032,
  and 46 of them under it. Read as a *share* — 4.5%, a fringe, not the subject —
  that is a refusal. **Drawn instead of divided**, the same 46 are a band exactly
  one fragment wide running the whole length of both top edges, and there is
  nothing else on the box that is a line. A ratio and a picture of one population,
  and only the picture answers "is this the thing a person is pointing at".
  <br>
  **Priced on the real frame, `examples/discard_census.rs` over Britain's 121×121,
  the same run with the rule and without it:**
  <br>

  | | without | with |
  |---|---:|---:|
  | fragments given an east face | 57,687 | **55,971** |
  | fragments given a south face | 49,504 | **48,304** |
  | fragments given the lid | 1,514,304 | **1,517,220** |
  | the comb's control — two neighbouring **hits** disagreeing | 313,755 · 1.35% | **311,433 · 1.34%** |
  | comb inside an overhang | 6,393 · 0.22% | 6,343 · 0.22% |
  | comb where the overhang joins the art | 732 · 0.30% | 730 · 0.30% |

  <br>
  **2,916 fragments of 1.6 million move, 0.18%, and 2,322 disagreeing
  neighbouring pairs go with them.** The last row is the one that makes it a
  repair rather than a preference: the population this rule does not touch is
  unchanged to the pixel, and the one it does touch is where two neighbours
  stopped contradicting each other. Nothing anywhere got worse.
  <br>
  **And it is a rule about one box**, which is the property that was asked for
  and the reason a second candidate is not being built. That candidate: these
  pieces are `CLEAR`, `opacity 0`, so `Builder::add` pushes nothing, `id_of` has
  no name, and `push_volumes` keeps the **per-tile** box rather than
  `occlusion.solid(id).space` — `SOLID_NOBODY` in 270 of 270 of the seam's own
  pixels. Since `merge::merged` folds a named run into one `Solid` whose space is
  the union, naming these pieces would dissolve the join outright: a run of
  counters would be one box with no interior face to meet. It would work, and it
  is the wrong lever. A body's top edge is a line on an isolated table too, with
  no neighbour to merge with, and a rule that only comes out right when something
  folded is a rule that owes its correctness to an optimisation. The naming
  question stays open on `docs/parity.md`'s P4 step 2 for its own reasons —
  identity, shadows, and a grid 15.1% larger — and no longer has this defect
  riding on it.
- ~~🚩 **A sprite's own top edge is serrated**~~ **Measured 2026-08-10, and the
  clamp keeps the fringe.** The candidate this entry and the cornice entry both
  ended on — *give a miss the face the sprite's own volume presents* — was
  written on both sides, run, and refused on the numbers. It is
  `impostor::presented_face`, kept in the tree because
  `examples/discard_census.rs` calls it to price it, and nothing in the pipeline
  does. The instrument is that census's new **`Comb` pass**: it counts
  *disagreeing neighbouring pixels* rather than shares of a population, which is
  the only shape of number that can tell a serration from a two-toned overhang
  — the same face counts describe both.
  <br>
  Britain's `121×121` around `(1501, 1659)`, per neighbouring pair of drawn
  pixels, the clamp against the candidate:
  <br>

  | population | clamp | candidate |
  |---|---:|---:|
  | comb *inside* an overhang, 2,882,656 pairs | 0.22% | **0.02%** |
  | the join to the art it hangs off, 243,275 pairs | **0.30%** | 32.59% |
  | — panels alone | 0.85% | **97.68%** |
  | the control: two *hits*, 23,156,254 pairs | 1.35% | 1.35% |

  <br>
  **The number the candidate's argument never had: 91.79% of the art bordering
  an overhang is on the box's own lid.** An overhang hangs *above* its box, so
  the pixel beside it is where the view ray grazes over the top face — a `z`
  face even on a wall panel whose every other pixel is a side one. The clamp
  agrees with that neighbour by construction, being the same clamp one fragment
  along; a rule reading the *volume* contradicts it by construction, because a
  panel presents its side. That is a hard line drawn along the top of every wall
  in the world, traded for a comb that the control says was never the larger
  number: **two neighbouring pixels that both hit disagree six times as often as
  two misses do.** The overhang is smoother than the picture it hangs off.
  <br>
  What is left of this entry is a sentence rather than a plan: the clamp lies
  about *position* — up to 133 fragments, four tiles — and that lie is bounded
  by the overhang, which is bounded by how badly a box fits its art. So the
  fringe is downstream of the height nobody measures and of nothing else, and
  the population it is measured over is roofs (`0x05A2` loses 35.2% of its art
  to a box three `z` tall). It closes here and reopens only if that changes.
  The report that opened it, kept because it is what a person saw: seen at
  Britain's `(1459, 1693)`
  in `View::Light` and again in `View::Normal`: along a wall's top boundary the
  normal alternates pixel by pixel between the wall's own camera-facing face and
  the neighbour above it, and the light alternates with it. The rule it comes
  from is this document's own — *"a pixel of the sprite whose ray misses the
  prism takes the nearest point on it — the art overhangs its own volume by a
  pixel or two and that is what it means"* — and the accepted cost beside it
  (*"statics without a good prism get a rougher volume"*). What nobody wrote down
  is that the *nearest face* of a miss flips between two answers along a
  silhouette, so a smooth overhang reads as a comb. Three candidates and none of
  them measured: keep it (it is a fringe of one pixel, and phase 6d moves these
  fragments anyway); give a **missed** ray no facing at all, which is what the
  normal plane's third state means and is honest about a volume that does not
  describe the pixel — but it puts a fringe lit from every side against
  neighbours that are not, so it has to be looked at rather than argued; or take
  the *instance's* own single facing for a miss, which is the pre-6c answer for
  the whole sprite applied to its overhang alone. **Measure the flipping pixels
  first** — how many, how far out (`Meeting::outside`), and whether they are the
  same set as the ones a person can see. That instruction is what was carried
  out above; the third candidate it lists, "the instance's own single facing",
  *is* the one the census priced.
- **A billboard's brightness is a per-row estimate no longer, and what is left is
  ordinary sampling noise.** Phase 7's position half took away the correlation
  that turned eight rays into bands; it did not take away the eight rays. A
  mobile standing next to a flame is now dithered per pixel like everything else,
  which is the *same* grain the entry below names and is what the ray-count knob
  exists to trade against. Worth a look on a real figure before deciding
  anything: at `FLAME_RADIUS` the grain is small, and it was only ever a person's
  complaint at a flame size eight times that.

- ~~🚩 **Two world claims are asked about a fragment that is a point of no solid,
  and that is why `same_run` still reads as load-bearing.**~~ **Done, and it was
  three places rather than two.** `light_runs_along_a_wall_and_stops_across_it`
  and `the_two_faces_of_a_corner_are_lit_from_the_side_each_looks_at` built their
  spots with `Spot::face` and never called `Spot::part_of`, so `spot.solid` was
  `None` and `on_the_lit_surface` — the rule that would excuse a coplanar
  neighbour along a run — was never even consulted. Both now name their own solid
  out of the grid the same frame built: the run's panel by
  `Occlusion::id_of`, the corner's by its **edge mask** rather than by a `Part`
  number, since which panel is pushed first is `boxes_of`'s business and not a
  fixture's.

  **The third was an instrument, and it is the one that mattered.**
  `plan::elevation` — what `pictures.rs`'s two wall tests are pictures *of* —
  wrote `OwnerId::NONE` into every row it built, under a comment saying a
  diagnostic picture is never walked for shadows. `View::Flames` **is** a walk, so
  every pixel of an elevation was a point of nothing: exempt from nothing,
  shadowed by its own panel. A `Wall` now carries `of`, the static the run is made
  of, and `drawn` asks the caller for the owner where it builds the row — the
  same shape `statics::quad_of` has in a real frame. Stated by the caller and not
  searched for: a tile holds several occluders, and picking the wall-shaped one
  would be the instrument deciding what it is a picture of.

  **The measurement S4 was waiting for, on the whole crate, both sides
  neutralised:** with `same_run` returning zero in `light.rs` *and* in
  `blit.wesl`, all 510 tests of `openshard-client-render` pass except
  `same_run`'s own unit test — the brute-force oracles, the GPU parity sweep and
  both wall pictures included. Before the three fixes the same injection turned
  four tests red. The controls both ways: with `on_the_lit_surface` neutralised
  instead and `same_run` live, the crate is **also** green; with both neutralised,
  `a_room_lights_its_own_wall_and_not_the_storey_over_it` and
  `a_wall_lit_from_one_end_has_no_dark_stroke_at_its_seam` go red. So the pair is
  genuinely load-bearing and the two rules are **mutually redundant on every
  fixture in the tree** — which is a licence for S4's deletion and not a proof
  that the tree can choose between them. What chooses is the argument, and it was
  already written: D2 is a theorem about a box and a plane, `same_run` is cell
  arithmetic that excuses more than the theorem allows — a tile's *north* panel on
  the same row is excused by `same_run` and correctly is not by D2.

  **And it is deleted**, `docs/occluders.md`'s S4, first of that step's four: out
  of both walks in `light.rs`, out of `blit.wesl`, taking `on_surface` — the
  height half of the mask, whose only reader it was — and the two unit tests whose
  whole subject was either. The panel arm of all three walks is `pierced` and
  nothing else now. S4's gate in full: suite green with the rule neutralised
  before the cut, suite green after it, and the identity injection turning
  **exactly the same six tests** red before and after, so the self-shadow rule is
  demonstrably untouched.
- ~~🚩 **S3's surface exemption is now unreachable, and its gate is vacuous rather
  than green.**~~ **Refuted, and the entry above is why.** Phase 5b's numbers — `0`
  of 720 blamed with the rule neutralised, the whole of `tests/lighting.rs` passing
  without it — were taken while `same_run` was standing beside it answering the
  same cases, and while three fixtures named no solid so that the rule could not
  fire at all. With `same_run` deleted and every fixture naming its own solid, the
  same injection is anything but vacuous: `on_the_lit_surface` forced to `false` on
  both sides turns `a_room_lights_its_own_wall_and_not_the_storey_over_it` and
  `a_wall_lit_from_one_end_has_no_dark_stroke_at_its_seam` red. It is the rule the
  crate now stands on, and the general lesson is worth more than the entry: **a
  no-op measured beside a second rule that covers the same ground is a statement
  about the pair, not about the rule.** Neutralise one at a time *and* both.
- 🚩 **A test whose subject moved out from under it goes on passing, and this
  track has now found two of them in one place.** Both vertical-ray tests were
  written when a flame was a point; phase 5 made it a sphere whose samples are
  never its centre, and from that day neither test sent a vertical ray — the
  branch each is named for was entered zero times by the whole crate. Nothing
  said so, because a test that stops reaching its own case *passes*. The repair
  is a **positive control** in each (`flame_points` must return the fragment's own
  `x` and `y`), and the question it raises is general: **which other fixtures name
  a case a later phase took away?** The candidates are every test written against
  a point flame — anything whose scene puts the flame exactly on a plane, exactly
  on an axis or exactly at a corner, since an eighth of a tile of sphere is enough
  to move all three. Worth one sweep of `tests/` with that question rather than a
  guess.
- **`brute_force_blocked`'s step count comes from the *horizontal* run alone, so a
  steep ray gets almost no samples.** `steps = ceil(ground / BRUTE_STEP)`, and
  `ground` is `sqrt(dx² + dy²)` — a ray climbing twenty `z` over a hundredth of a
  tile is sampled once, and a vertical one not at all (`1..1` is an empty range,
  so it returns "open" without looking). Its own-column exemption covers the whole
  segment of a vertical ray besides, so it has *no opinion* about the case rather
  than a wrong one. Nothing has been convicted by it yet — the fuzzers aim
  horizontally — but this is the one non-circular oracle in the tree and its
  resolution should come from the segment it is measuring: `sqrt(dx² + dy² +
  (dz / Z_PER_TILE)²)`, the same isotropic metric everything else uses. Cheap, and
  it wants its own run before it is trusted, because a finer march can only turn
  "open" into "blocked" — which is a finding either way.
- **`walk_sun` answers an overhead sun by hand, and its reason is the one the
  vertical shortcut just lost.** `horizontal < 1e-6` returns `(1.0, None)` under
  the comment *"the only thing that could shadow the spot is on its own tile —
  which is exempt"*. Since phase 4 a fragment is exempt from **its own primitive**
  and not from its tile, so a floor under a roof, both on one tile, is a sun ray
  through a roof. Measure zero in practice — a sun exactly overhead is one instant
  of one day curve — which is why it is a backlog line and not a defect report;
  what it is *not* is a rule that still has an argument behind it. Deleting it is
  the same one-line change the vertical shortcut was, and the same census applies:
  find out whether any fixture reaches it first.
- **A flame of radius zero costs eight identical rays.** `flame_radius` is a knob
  now — `Lighting`'s field, and `examples/boxes.rs`'s `OPENSHARD_FLAME_RADIUS`
  since a person wanted to see how hard a shadow can be — and at zero every one of
  the eight sample points *is* the centre, so both walks and the shader walk one
  segment eight times and average it with itself. Nothing shipped asks for zero, so
  this is a diagnostic paying eight times over rather than a frame doing it; the
  fix is one branch, and the reason to write it down rather than add it is that a
  branch on a radius is a second code path through the hot loop and wants a
  measurement before it earns one. `pathtrace::Body::Point` already makes exactly
  this distinction on the reference's side, and `Image::is_exact` is how it says so.
- **A flame's eight rays are now in the brightness, and near the flame that is
  visible grain.** Phase 5b's accepted cost: where a fragment stands inside the
  sphere, how many of its samples clear its own plane is a coin flip, and 126 of
  256,711 pixels of the wedge scene are more than an eighth of full scale from the
  reference — worst `0.4896`, all within about a tenth of a tile of the flame.
  Nothing in the tree gates it, and the two things that would are the ones phase 5
  parked: more rays, or temporal accumulation. Worth a picture at a real lamp
  before either is built.

- ~~🚩 **The flame has extent for the shadow term and no extent for the cosine,
  and that is the wedge of shadow at every join.**~~ **Done — phase 5b, and its
  own account is up there.** Two of the claims below did not survive the landing:
  the prototype's "20,308 of them darker" is the wrong sign (163,492 pixels move
  on the gate's own fixture and 162,921 of them are *brighter* — an average of
  `max(N·L, 0)` over a body is never dimmer than the centre's cosine), and
  `same_run` was **not** retired by it. What follows is the measurement that
  produced the phase, kept whole, because the reasoning is what made the phase
  right even where its numbers were not.

  `shadow()` averages visibility over `SHADOW_RAYS` stratified points of a sphere
  of `FLAME_RADIUS`; `lit_from()` then multiplies by **one** cosine taken from the
  flame's *centre*. So the light is an area source for occlusion and a point source
  for shading — one state in two shapes, and the difference lands on the screen.

  What it draws: a lamp lower than `FLAME_RADIUS` above a floor puts half its
  sphere **below** that floor's own plane. Rays to that half are traced, and near a
  join they leave the fragment's own primitive and enter the neighbouring one — so
  they come back "blocked" and darken a surface that is flush and continuous.
  Further from the join the same dipping ray stays inside the fragment's own box and
  identity excuses it. Hence a **wedge**, widest at the join and tapering away, on
  three flights of stairs that are geometrically one landing. Same shape for two
  wall panels, two floor planks, any two abutting statics.

  The cure is the physical form, and it is exact rather than a mitigation: a sample
  point's contribution is `V(p) · max(N·L_p, 0)`, so a point below the fragment's
  horizon contributes **zero whatever stands in its way**. The set of rays that a
  join blocks and the set of rays that are below the horizon are *the same set* —
  which is why moving the cosine inside the loop removes the wedge entirely instead
  of dimming it.

  **Prototyped and rendered.** Per-sample cosine, outer multiply removed:
  the wedges vanish, and so does the eight-ray speckle on grazing surfaces — a
  below-horizon sample becomes a deterministic zero instead of a coin flip between
  blocked and open. 21,177 pixels move on the stair fixture, 20,308 of them darker,
  which is the overestimate the centre cosine was paying out.

  **The side-lit case is real and is not to be discarded for want of a picture.**
  It was reported from the client and reproduced in an earlier session; this
  session failed to render it, which is a fact about the fixtures reached for, not
  evidence against it. Treat it as present. It is also the case where a cosine
  cannot hide anything — a lamp beside a wall lights the face it grazes — so it is
  the configuration to check *after* the per-sample cosine lands, and the reporter
  expects it to go the same way.

  Two things it should also settle, and both want measuring rather than assuming:
  `docs/occluders.md`'s `same_run` is broad precisely because it was papering over
  these below-horizon rays for panels, so this may retire its real reason; and the
  seam that plan hands to its merge (S3b) may be the same defect seen from the
  geometry side. **The reporter's own hypothesis, kept as one:** the artefact first
  reported with a *side* light may go the same way once this lands.

  It is a shading question, not an occluder one, which is why it lives here. The
  gate is the reference path tracer: it samples an area light with 64 paths and a
  real Lambert term, so per-sample cosine should move the frame *towards* it.

- ~~🚩 **A shadowed floor leaks a one-pixel line of full light along every tile
  boundary.**~~ **Fixed, and the fix is `light::starting_cell`.** The cause is
  the last measurement below: the carried tile was allowed to *contradict* the
  position rather than only to break its ties. `starting_cell` keeps the carried
  tile for every point of its own tile's closure — both edges included, which is
  the whole of what it was for — and takes `floor` for a point strictly outside
  it; both walks and `blit.wesl` now seed from it. On the frame that found it,
  the narrow leaks over the building's floors went from **303 to 0** (99 remain
  in the count, all at the wedges' own penumbra edges, where a one-pixel run is
  what a shadow boundary is). Three gates, each fault-injected to red before
  being trusted: `a_walk_starts_in_a_cell_its_own_start_point_is_in` over
  fractions either side of both edges;
  `a_ray_starting_just_past_its_own_tile_is_stopped_by_the_cell_it_is_in` on
  both CPU walks; and — for the shader's own second spelling of the rule —
  `a_fragment_a_hair_inside_a_wall_is_shadowed_by_the_cell_it_drifted_into`,
  which needed `Fixture` to grow a `drift`, since a parity fragment's fraction
  runs to `112/127` and could never reach an edge at all. Neutralised in the
  shader, that pixel reads `241` against its open neighbour's `241`.
  ⚠ **The rule is gone since S4 and the leak stays fixed by construction** — a
  walk seeded from `from.floor()` is always in a cell containing its own start
  point, which is what the leak broke. Of the three gates, the first was
  repointed at `dda_walk`, the second deleted (it had stopped gating anything)
  and the third kept as a fixture; `docs/occluders.md`'s § *The starting cell*
  has which and why.
  **The direction was half the fixture** and the first version of the CPU test
  got it wrong: a ray heading *away* from the carried tile seeds a negative
  distance, leaves at once and reaches the true cell anyway, so it stayed green
  with the rule removed. The leak is the other sign — a ray heading back over
  the carried tile, seeded a whole tile of slack. What is *not* fixed is the
  geometry underneath: a run of coplanar floors is still N solids on N tiles,
  which is the merge `same_run`'s own backlog entry wants and would have made
  this class of boundary rarer rather than answered it.
- **`starting_cell`'s own proptest was describing a point nobody had built**, and
  a fresh seed found it during phase 5b. It asked the generator for an offset,
  handed `starting_cell` the sum `tile + off`, and then judged the answer against
  the offset it had *asked for*: at `tile_y = -6`, `-6.0 + 1.0000002` is not
  representable and rounds to exactly `-5.0`, whose offset from its own tile is
  exactly `1.0` — on the edge, where the carried tile is the right answer. Fixed
  by reading the offset back off the point. The shape is the one worth keeping: a
  generator's number and the number the function sees are two different values
  wherever the sum between them rounds, and an oracle built on the first is
  testing arithmetic it did not perform.
- ✅ **`starting_cell` is a repair and not a construction — closed 2026-08-09 by
  deleting it, `docs/occluders.md`'s S4.** The entry read: it carries no constant
  and no tolerance, and three fault injections say it is load-bearing, but what
  it *is* is a rule for what to do when a fragment's two statements of where it
  is disagree — the instance's tile through the id plane, and the position plane.
  A rule that arbitrates between two spellings of one fact is the shape this repo
  has a name for. **The construction it named is the one that landed**, and
  almost verbatim: the set of cells a ray visits is a property of the segment, a
  start point on a boundary is a point of two cells, and `ray_vs_solid`'s
  origin-touch rule already discards a box met only at the ray's own start — so
  there is no tie to break and a cell entered for zero length can produce
  nothing.
  Two corrections the doing supplied. **The walk does not need to test both
  cells**: it starts at `from.floor()` and reaches the other at `t = 0` if the ray
  heads that way, so the predicted cost — up to four cells at a corner on the
  first step — is **zero cells**, and `tests/cost.rs` did not have to be able to
  price it. And **`Spot::tile` did not keep the job this entry left it**:
  `same_run` was deleted before this, so the tile's last reader in the whole
  lighting pass was the arbiter itself. What survives of `Spot::tile` is
  `sky_at`, a question about a column of the map rather than about a ray.
- **The lateral seams were checked, and they are the tile's own plane rather
  than a constant — with one named exception.** Measured on the same real place,
  reading the grid's own boxes: every tread of the stair is `x 100.000..101.000`
  and every storey's floor the same, so a stack's *lateral* end is the `+x` face
  of a whole-tile box, at `tile + 1` exactly. Every panel in that radius names
  `EDGE_SOUTH` or `EDGE_EAST` — `y 100.800..101.000` and `x 100.800..101.000` —
  whose **camera-facing** side is likewise the tile's own boundary, which is the
  plane the art draws the wall on. Nothing on the visible side of any of them is
  an invented number.

  The exception is the one already in this backlog, sharpened by the reading:
  `PANEL_THICKNESS = 0.2` fattens a panel *inward*, so a `NORTH` or `WEST`
  panel's camera-facing side is `tile + 0.2` while a `SOUTH` or `EAST` one's is
  `tile + 1`. Two walls of one run, drawn by the artist on one plane, get
  positions **four fifths of a tile apart** according to which edge the art
  happened to name — and the constant is invented outright, by its own doc: "the
  art still cannot measure a wall's depth, so any number is invented". It did
  not show in this frame because the radius held no north or west panel; it is
  in every frame that holds a building's far wall. The construction that removes
  it is the one that entry already names — **one `0.2` slab straddling the
  shared edge**, so a pair of neighbouring walls is one wall and both faces land
  on the plane the art draws — and it is a seam that stops existing rather than
  one that gets chosen a side.

  What no reading here settles is how far a real static's **art** overhangs its
  own box laterally. That is phase 6's own second number, still untaken, and it
  is the only remaining lateral question that a picture rather than the grid has
  to answer.
- **How that one was found, kept because the method is the finding — and because
  four of its six steps were wrong turns.** The lines look exactly like an
  exemption leaking, and they are not: **four fault injections each left the
  frame unchanged**, counted rather than eyeballed. `same_run` neutralised, 303
  narrow leaks against 303. The identity compare forced `false`, 282. The
  origin-touch rule (`entered == 0 && leaves == 0`) forced `false`, 303. And
  `RAY_TANGENT_TOLERANCE` widened ten thousandfold from `1e-6` to `1e-2`, 295 —
  which is what says the answer was never a razor.

  Then four measurements narrowed it. The runs are **one pixel wide at 1:1, at
  2:1 and at 4:1**, so the thing they draw has measure zero in the world; a
  world-space stripe doubles with each notch. They stand **inside one facing
  with the same facing either side** (365 of some 600 runs are `+z | +z | +z`
  off the normal plane), so they are not a step's own edge, which would butt
  against a change of facing. Against `View::Place`'s checkerboard, which is
  drawn from the tile, **303 of 305 straddle a tile change**. And the last one
  is the one that named it: `View::Place` repainted for one run as "is this
  fragment's position outside the tile its own instance carries" separates
  *exactly on the edge* — 5,759 pixels, the ordinary state of every south and
  east face since 6c — from **strictly outside**, which is 474 pixels of the
  frame, and **324 of those 474 leak.** Two thirds of a set that is a third of
  a percent of the picture. `View::Shadow`'s own neighbours are on a mismatch
  4% of the time, so the enrichment is twentyfold.

  What made the last step available was the CPU twin disagreeing with the
  shader for a reason that is not the walk: `isolated_scene`'s profile mode
  builds its `Spot::tile` with `floor()` **on purpose**, to keep showing what a
  naively-derived tile does — so it never reproduced the leak, and that is what
  said the tile was the variable. `docs/lighting_raymarch.md`'s tile-boundary
  hazard is the family; the specific defect is one rule that had drifted from
  its own contract.
- ~~🚩 **An emitter is black in its own light, and every free-standing one taller
  than `FLAME_LIFT` is.**~~ **Fixed 2026-08-11 — a body writes no facing**, which
  is the candidate the *next* entry named rather than any of the three this one
  listed, and it closes both. The record of the defect follows; the answer is at
  the end of it.

  Found by looking at a lit frame after phase 6c — the
  one instrument *How this is judged* names — and reproduced at one item and
  nothing else: `OPENSHARD_SCENE_RADIUS=0`, no ground, no statics, one lamp post
  by hand, `0 standing cells`, and the lamp lit by its own flame is a black
  silhouette with a green wick. The chain is three facts that were each right
  on their own. `light::burns` answers only for statics light gets *through*
  (`opacity == CLEAR`), so **an emitter is by definition not in the occlusion
  grid**. Phase 6c gives a shape to exactly those too — "a pane of glass has a
  shape whether or not it casts a shadow" — so a lamp post now has a volume.
  And `light::place` burns at the tile's own centre a `FLAME_LIFT` up, which is
  **inside** that volume: the impostor answers each of the sprite's own
  fragments with the camera-facing plane of its own box, whose normal points
  away from the flame, so `N · L ≤ 0` on every visible face. `View::Shadow`
  reads those pixels *visible*, which is what says it is the cosine and not the
  walk. `mounted_at` rescues the sconce alone — it moves a flame
  `MOUNTED_CLEARANCE` clear of a *panel's* plane, and a panel is another
  static's edge in the same cell, which a lamp standing in the open has none
  of. A campfire is unhurt because its box stops below half a tile and the
  flame clears its lid. **It arrived with 6c rather than being uncovered by
  it**: before the impostor a lamp post was `Stance::Upright`, whose normal is
  the zero vector, and the zero vector is the one value `blit.wesl` skips the
  cosine for. Three candidate answers, none of them measured yet: place the
  flame where the *art* draws it rather than at the tile's centre (the honest
  one, and the same unmeasured sprite reading `MOUNTED_CLEARANCE` wants);
  give an emitter no volume, which trades this for the billboard 6c retired;
  or say that a surface containing its own light source has no cosine, which is
  an exemption and therefore the shape this document exists to refuse. **Phase
  7's billboard question is this question one object over**, and the two should
  be answered by the same reading of a sprite.

  **And none of the three was the answer, because none of them was about the
  box.** A flame moved to where the art draws it is still inside a whole-tile
  box; an emitter with no volume trades this defect for the billboard 6c
  retired; the exemption is refused by construction. What is actually wrong sits
  one line above all three: the box a lamp post stands as is `Edges::ANY` — the
  tile's own walls, handed to a graphic whose facing the art would not name — so
  the face `impostor::meets` answers with is a plane **nobody drew**, and every
  fragment of a thin pole was being told it looks the way that tile's south or
  east wall does. A body writes no facing now (the entry below), and the emitter
  is lit by its own flame as a *consequence* rather than by a rule about
  emitters.
- **A wall a lamp stands against is barely lit, on a real place now.** Open
  question 1 had phase 3's synthetic frames under it; the lit frame above is the
  same shape at Britain: the plaster wall the lamp post is bolted beside takes
  almost nothing from it while the cobbles under it carry a full pool, because a
  flame half a tile out from a plane grazes it. Nothing here is a defect — it is
  the accepted cost, seen at last on art somebody drew — but it is the picture
  the exposure-and-ambient experiment should be judged against, and it is a
  better scene for that than any fixture in the tree.
- **The CPU's `Surface` is four fixed normals and land now has a fifth kind.**
  `light::sample`'s `Surface::Flat` looks straight up, which is exactly right for
  level land and wrong for a hillside — `ground.wesl` writes the bilinear patch's
  own normal per fragment and the CPU side cannot state one. It is not a
  regression: before phase 3 the two disagreed about *every* ground pixel, because
  the GPU wrote a zero there and the CPU wrote `(0, 0, 1)`. It is a new, smaller
  disagreement with a name, and what closes it is a `Surface` that can carry a
  measured vector rather than choose between four.
- **The reference tracer samples its own disc at random, and could stratify.**
  That single fact is why phase 5's penumbra gate is three aggregate statistics
  rather than a per-pixel one: at sixty-four samples the reference disagrees with
  *itself* by a third of a flame at the middle of a soft edge (measured — worst
  `0.3125`), so a per-pixel comparison there is a gate on the ruler. Stratified
  over its own sample index the error would be `O(1/N)` instead of `O(1/√N)` and
  sixty-four samples would be sharper than the engine's eight rays by an order of
  magnitude, at no extra cost — which would make the per-pixel claim available and
  would sharpen `penumbra`'s `over` count from a diagnostic into a gate. It needs
  `pathtrace::Emitter::sample` to know which of `settings.samples` it is being
  asked for, which is a signature and three of that crate's own tests.
- **Nothing on the GPU side tests the shader's own identity compare.** Forced to
  `false`, `tests/frame.rs` stays green from end to end while three tests in
  `light.rs` and `tests/lighting.rs` go red — so the rule the *shipped* walk uses
  is covered only through its CPU twin, which the phase's own commits also
  rewrote. What the one frame test in that shape reaches instead is `crosses`'s
  strictness: its fragment is flat and its own solid is a lid.
- ~~**`parity_frame`'s `Fixture` names an owner, and the shader compares a
  solid.**~~ **Done, 6f**, and not by the fix this entry proposed: the fixture
  names a `SolidId` now, because `gbuffer::Fragment` has a field for one and the
  plane it writes carries it. Writing a **mesh** row instead — this entry's own
  suggestion — would have been a third row table in that function *and* would
  have moved the fixture off the sprite path the shipped defect lived on.
  `the_shader_does_not_stop_a_vertical_ray_with_a_lid_it_is_not_under` states
  the bottom tread outright now, so the grid's reference order decides nothing.
- ~~**`statics.wesl` still clamps a face fragment to `INSIDE`.**~~ **Done, phase
  6c** — there is no clamp and no inverse projection to clamp: a face fragment is
  the point where its own view ray leaves its own panel's box, which is that
  panel's camera-facing plane exactly. The asymmetry the entry worried about is
  gone in the direction it did not expect — a *south* or *east* face lies on the
  tile boundary and a north or west one a `PANEL_THICKNESS` inside it, because
  that is where the slab's visible side is. See the next entry.
- **A north or west panel's visible face is a fifth of a tile inside its own
  tile, and that is the geometry rather than a bug.** `Solid::box_of` fattens a
  panel *inward* from the plane the art draws it on, so the camera-facing side of
  a north panel is at `y + PANEL_THICKNESS` while a south panel's is at `y + 1`.
  Phase 6c answers the picture with the volume, so a north wall's fragments moved
  a fifth of a tile into the room. Nothing compensates and nothing should — but
  the *geometry* has a better shape available: two neighbouring walls on a shared
  edge are two 0.2 slabs meeting, a doubled wall, where one 0.2 slab straddling
  the boundary would be one wall and would put both faces on the plane the art
  draws. `PANEL_THICKNESS`'s own doc argues the inward choice from the doubling
  it was avoiding, which the straddling form avoids better. Worth measuring
  before it is changed: it moves every panel in the grid.

  **And the merge does not answer it, which `occluders.md` first said it would.**
  A tile's north panel and its northern neighbour's south panel do share a whole
  face, but they are an `EDGE_NORTH` and an `EDGE_SOUTH`, and `Solid::edges` is
  documented as never naming two sides — the walk's panel arm reads it. So S3b
  refuses the pair, exactly, and the fattening stays where it was. What would
  answer it is a decision about what a two-sided panel means to `pierced` and to
  `on_the_lit_surface`: a change to what one primitive *is*, which is the same
  ground as the lateral fit (`facing::Prism` has no cross-axis term at all). The
  constant is still both *how thick a wall is* and *which side of its tile it
  sits on*.
- **Three scans a drawn static now, where there was one.** `statics::collect`
  asks `Occlusion::owner_at` for the quad, `Occlusion::id_of` per mesh face and —
  since phase 6c — `id_of` again per *box*, all linear scans of the cell; see
  `owner_at`'s own note about the join this design pays for. A four-tread flight
  scans its cell thirteen times. Nothing measures it as a cost yet, and
  `tests/cost.rs` cannot: it builds its frame against `Occlusion::EMPTY`, so no
  static there has a box to meet and the statics pass it times is the billboard
  fallback. **Both halves of that are work** — the scans, and a cost harness that
  prices the pass the client actually runs.
- **A corner is told apart by the screen half, and the boxes could say it.**
  `statics.wesl` still resolves a corner's two panels by `across > 0.0` — the
  half of the picture a pixel was drawn on — where the impostor already meets
  both boxes and picks between them for the *normal*. What stops the box from
  deciding is the **id**: the left half takes `in.twin`, the row
  `split_corners` appended, and a `Volume` carries a `SolidId` and no row
  number. Two answers to one question, and today they can disagree — the box in
  front and the screen half are not the same test near the tile's own corner,
  where the two slabs overlap. What would close it is the volume carrying which
  *instance row* it belongs to, which is one word in a struct that has a spare
  one.
- ~~**A sprite fragment's `stance` is still the art's reading, and `lit_plane`
  believes it.**~~ **Done, 6g** — see that phase's account. The objection this
  entry raised against the swap (for a wall, `lit_plane(FaceNorth)` is the panel
  box's `lo.y` and the normal names its `hi.y`) turned out to be the argument
  *for* it: the fragment is drawn on the box's camera-facing side, so `hi.y` is
  the plane it is actually in and `lo.y` was the far one. Every gate stayed
  green, which is what says the wall case moved by `PANEL_THICKNESS` and moved
  the right way.
- ~~**`own_solid` scans a cell to name a solid the fragment already met.**~~
  **Done, 6f** — see that phase's account. The missing piece this entry named
  ("a way to get it from the pass that knows it to the pass that asks") was the
  position plane's **fourth channel**, which every producer had been writing a
  constant `1.0` into: an id is three bytes and an `f32` carries every integer
  to twenty-four exactly. What this entry got wrong is the priority — it is
  filed here as a cost, and it was a live, visible defect from the hour 6d
  landed.
- ~~**A run of wall wants to be one solid, and until it is, `same_run` stands
  in.**~~ **Done, phase 6e** — `occluders.md`'s S3b merges a run of coplanar
  panels into one primitive (73 pieces to 9 on the crate's own two-storey house,
  and not one pixel moved) and S4 deleted `same_run`. What is worth keeping out
  of it is that the merge is *not* what retired the rule, which is what this
  entry predicted: `same_run` excused a neighbouring panel for rays dipping
  **behind** the surface's plane too, and those stopped being traced at phase 5b.
  The merge landed anyway, and the run being one primitive is what makes S3's
  half-space exemption enough on its own.

**Inherited from `occluders.md`, which is a record now.** Three of its findings
outlive the track and belong in this list, since this is the live one:

- ✅ ~~**An aperture is the last rule in the pass still stated in a tile, and it
  now costs a merge.**~~ **Closed 2026-08-09 — `occluders.md`'s S6**, and both
  halves went together as the entry said they would: `Aperture`'s `near`/`far`
  are world coordinates on the panel's own run axis, `light::run_v` is
  `along_the_run` with no `floor` in it, and the holes are a storage buffer of
  four `f32` indexed by `SolidId` rather than an `Rgba8Uint` texel folded into
  `LIST_ROW` rows. `Occlusion::list_rows`, `z_byte`, `Z_FLOOR`, `Z_CEILING` and
  the shader's `RUN_STEPS` and `aperture_at` are all deleted. **It fixed two
  live defects and refused the payoff this entry expected** — see
  `occluders.md` § *The aperture* for the readings:
  - a crossing exactly on a tile boundary floored into the *next* tile, so a
    window running to the far end of its own tile read as one at the near end of
    the tile beyond it — § *The oracle*'s own defect, one level up;
  - `z_byte` clamped a hole's two ends into the map's `i8`, and a hole's ends
    are not an `i8`: `Aperture::placed` adds the art's whole units to the
    static's base, so a window on a wall standing at 120 reaches 140 and the
    wire shut it at 127. The record and both CPU walks read it open, so this
    one showed on the shader alone. **The claim that a hole's `z` is quantised
    "and that is no defect" was wrong about the top end**, which is why it is
    written out here rather than merely ticked.
  - and **the merge gains nothing**, which the entry above assumed it would. Two
    pieces may only merge with an equal `Owner`, an `Owner` is a `(z, graphic)`
    and a hole is read off the *graphic* — so two mergeable pieces are windowed
    together or plain together, never one of each, and a wall with one window in
    it is a wall of two graphics that the `Owner` refuses whatever the aperture
    says. The refusal in `occlusion::merge` stays and its reason is now the true
    one: a primitive carries one hole and a run of windows is one per tile.
    That is the fifth time on this track that a step's decision held while its
    stated reason did not.
- ✅ ~~**Two instruments still cannot see a merge.**~~ **Closed, and "cannot see"
  was the wrong diagnosis for both** — measured 2026-08-09 under
  `occlusion::merge`'s own "the union does not grow" injection, live in a build
  where `tests/lighting.rs` goes 12 red. Neither instrument is unreached: the
  sweep carries five scenes that fold (a room 24 → 4 pieces, a carried beam
  24 → 4, a hole in a wall 9 → 3, a house corner 7 → 3) and `pictures.rs` draws
  six, so both walk the broken geometry.
  - `frame.rs`'s shader sweep stays green while its **own census** moves by up to
    934 pixels of 4,096 — a room 2,400 → 1,466 in shadow, a hole in a wall
    1,308 → 744, the room's penumbra 0 → 75. That is circularity with a number
    on it: it counts the wreck and cannot report it, because both sides read the
    same primitives. **Settled: a merged scene buys it nothing and none is
    added.** What gates a merged frame on the GPU is `traced.rs`'s twin.
  - `pictures.rs` was *drawing* the defect: the row behind the wall reads
    `0.094`/`0.111` against a flat ambient `0.063`, at the four columns either
    side of the one tile its assertion read. Closed by reading the band across
    the run — the shadow behind a wall is as long as the wall, nine columns each
    with its own lit-in-front control — which the injection now turns red at
    column 98. `docs/occluders.md` § *Neither instrument is unreached* has both
    readings.
- **`Solid::footprint`'s `i32` ranges are the one newtype the occluder sweep set
  aside on purpose.** Closing it means a real tile-coordinate type, whose call
  sites reach into `bake.rs`'s whole coordinate system (`origin`, `tile_of`,
  `spill_of`, block and cell indices) — D7's ground, and its own pass.
- **The hierarchy's cost is unmeasured on a real frame.** ~~S5 left `tests/cost.rs`
  reporting the tree, and the run itself is the user's — a heavy live run, not a
  suite gate. Until it is taken, "a BVH is cheaper than the grid" is an argument
  rather than a number.~~ **Taken 2026-08-11**, `tests/cost.rs`, Britain at the
  widest zoom (1/2×, world image 3840×2160), seven flames:

  | case | ms/frame | ns/pixel | over `dark` |
  |---|---|---|---|
  | copy | 0.482 | 0.232 | −29.1% |
  | dark | 0.679 | 0.328 | +0.0% |
  | far | 0.723 | 0.349 | +6.5% |
  | night | 1.865 | 0.900 | +174.5% |
  | sun | 1.165 | 0.562 | +71.4% |

  The tree itself is cheap: `far` (7 flames moved 1000 tiles off, so every
  fragment's broad-phase misses and no node is ever tested) sits 6.5% over the
  `dark` floor. The weight is the `night` row's own 174.5%, and that is the
  ray count and not the traversal — `arrival`'s eight rays per flame in reach,
  each its own `walk` of the tree — matching the same fixture's 4:1-zoom
  reading above (§ phase 5b, one ray vs eight). So "a BVH is cheaper than the
  grid" is now a number for the tree walk specifically, and the number the
  blit pass pays a real frame for is soft shadows' ray count, not the
  hierarchy under them.

  *Two levers follow from that, neither taken yet.* **`shadow_rays` itself** —
  already a runtime knob (`Tuning::shadow_rays`, default `SHADOW_RAYS = 8`) —
  is the cheap one: the cost above scales close to linearly with the count
  (phase 5b's table, one ray vs eight), and turning it down is a quality trade
  a person can look at, not a code change. **Packet traversal** is the one that
  is not a trade: `arrival`'s eight rays share an origin and nearly share a
  direction (a small disc on a distant flame), so `walk` pays for the same
  upper tree nodes eight separate times. Testing a node once against the
  bundle's own bound and only descending per-ray at the leaves would cut node
  visits without moving a single answer — packet/beam traversal, not a
  tolerance — but it touches `walk`/`arrival` in `blit.wesl` and their CPU
  mirror in `light.rs`'s `walk_primitives`/`arrival`, and every oracle both
  already answer to (`tests/lighting.rs`'s fuzz, `boxes.rs`, `synthetic_stair.rs`)
  would have to agree with it before it lands.
- **A sconce's own art says how far it stands out from its wall, and nothing reads
  it.** `MOUNTED_CLEARANCE` is `0.7` of a tile because half a tile reaches the
  plane and a fifth clears it; the sprite shows the real overhang and
  `crate::facing` already measures silhouettes for a living. That is what retires
  the constant honestly, and phase 4 found that deleting it without a replacement
  blacks out every wall carrying one.
- **A slope's normal now nudges its own shadow ray sideways.** `walk`'s `ahead`
  spends the normal's `x` and `y` on `STAND_OFF`, and until phase 3 a ground
  fragment's was zero on both. A hillside's is not, so a slope's ray starts a
  fiftieth of a tile out along the hill. That is more nearly right than not
  nudging at all — it is the direction out of the surface — but it is a behaviour
  nobody asked for arriving through a constant phase 4 deletes. **Closed at
  phase 4**: there is no `STAND_OFF` and no nudge of any kind, so a slope's ray
  starts where the slope is.
- **Two scenes moved because a flame stood in a surface's own plane, and the
  shape of that is worth keeping.** `z: 0.0` in a hand-built `Light` read as "a
  fire on the ground" for as long as the shading term was a half-space, which
  gave such a flame the band's own half. Under a cosine it gives nothing, and the
  tests said so at once. **Every hand-built `Light` in the tree should be asked
  whether it means a tile's `z` or `FLAME_LIFT` above it**; two were found by
  failing, and a scene that merely goes dim would not have said anything.
- **The origin-touch rule is stated three times and tested through none of them
  directly.** `if entered == 0.0 && leaves == 0.0 { continue; }` lives in both
  walks and in `blit.wesl`, and what says it is right is a *tool's* count going
  from 88 to 0 — `synthetic_stair`'s face oracle, which nothing runs under `cargo
  test`. The claim it makes is small enough to state as a unit test of
  `walk_cells_*` on a two-solid fixture (a lid whose edge is another solid's own
  plane, a ray leaving that edge), and that is what would catch it being deleted.
- ~~**A north or west face's normal contradicts the argument `outward` itself
  makes for it.**~~ **Done, phase 6c.** The impostor names the face the ray met
  and a ray from the camera can only meet `+x`, `+y` or `+z`, so there is no row
  left to be wrong; `place_format.wesl`'s `outward` is **deleted**, nothing else
  read it. `crate::place::Stance::normal` keeps the same table on the Rust side
  with the defect written down at its definition — its readers are hand-built
  G-buffers stating a scene by naming a stance, which is a question about the
  edge rather than about the picture.
- **Two pixels at flame height `1` survive the light oracle, and both sit where
  the reshaped tread put them.** `[tread 2's riser] at (100.80, 100.33, z 3.10)`
  — the engine reads it fully blocked and the geometry gives it `0.022`, a tenth
  of a tile above the top of the body that blocks it; and `[tread 1's riser] at
  (100.97, 100.67, z 1.02)` — both sides agree the flame is fully visible
  (`through 255/255`) and they differ by `0.017` in what it is lit *to*, four
  parts in 255. Neither is a visibility disagreement, so the face oracle is
  silent on both. What they share is a flame level with a tread's own top
  (`z 1` is tread 0's height), which is exactly the case
  `segment_clear_of_box`'s own doc calls out: every ray from that height runs
  *along* the plane of every surface at it. Worth one measurement each before
  phase 6d moves these fragments anyway — the mesh pass comes off real statics
  there, and their positions change with it.
- **`synthetic_stair.rs` rebuilds `statics::push_mesh`'s loop by hand**, and that
  is why it still asked the grid for `Part::nth(part)` a commit after the real
  pipeline started asking for `Part::nth(part / 2)`. It cannot call the real one
  — `push_mesh` is `pub(crate)` and an example is an external consumer — so the
  duplication is structural rather than lazy, and the join between a drawn face
  and the solid it names is now written in two places that have already
  disagreed once. The same shape as the seventh hand-built flight below.
- **The three-tread flight is now rebuilt by hand in a seventh place.**
  `statics::tests::flight` joined the five in `light.rs` and the one in
  `frame.rs`, and it is the same `Prism::new(Face::North, &[1, 3, 5])` again. The
  backlog entry below asking for one constructor is a line longer every time the
  scene is used, which is the argument for it.
- **A flame's size is a constant and belongs on the `Light`.** `FLAME_RADIUS` is
  one number for a candle, a torch and a campfire, and `Flame` already carries the
  reach, the colour, the intensity and the flicker — a size is the field that is
  missing, and a campfire is visibly wider than a candle. What stops it today is
  the uniform: `Light` on the GPU is three `vec4`s with no spare lane, so a fourth
  is 1 KB more at 64 lights. Worth doing when something else needs that lane.
- **`boxes.rs` now builds two mirrors of one scene** — the same `Mirrored` twice,
  differing only in the `LAMBERT_PI` on the flame's intensity — because the
  visibility comparison is in `Brdf::Flat` and the shaded strip in
  `Brdf::Lambert`. Phase 4 retires the first, and the second mirror should go with
  it rather than become a habit.
- ~~**The normal plane is sixteen bytes a fragment and needs four.**~~ **Done —
  see phase 2's own account.** An octahedral pair in an `R32Uint`, integers on
  both sides, and the two spare bits carry "nothing drew this" and "no facing"
  rather than the id word doing it. `ATTACHMENT_BYTES_PER_SAMPLE` is 32.
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
- ~~**There is no lit-against-lit picture, and three separate things stop one
  being drawn.**~~ **Done — this is phase 0, and its account is up there.**
  `<base>_lit_vs_traced.png` is the engine's shaded frame, the tracer's, and the
  difference amplified `8×`; `boxes.rs`'s `flat` scene is where it means
  something. All four blockers went: the albedos come from the frame, the flame
  is the engine's own, the encodes share `tonemap::encode`, and the ambient is
  nothing on both sides. The fourth — a mesh face has no albedo — is not fixed
  but *avoided*, by a scene with no boxes in it, and it is still phase 6's.
- **A body's albedo is still invented, and one scene is not a calibration.**
  What phase 0 now proves is that the engine and the reference agree about *one
  surface, flat, unoccluded, unhued*. Three things it says nothing about: a
  vertical face (no albedo on the engine's side until phase 6), a hued sprite
  (the ramp is decoded to linear before the light multiplies it, and nothing
  compares that against anything), and land that is not flat (`ground_albedo`
  panics on a textured floor rather than handling one — deliberately, because a
  single-albedo reference cannot judge one). Each is a scene the tracer could
  hold once the engine's side has a colour to compare.
- **A scene's flame reaching the whole canvas hid a conflation in two oracles for
  as long as every scene had one.** Fixed — see phase 0's account — but the shape
  of it is worth keeping: the oracles were right about every pixel they compared
  and wrong about *which pixels they had an opinion on*, and no amount of looking
  at their disagreement counts would have shown it, because the count was the
  thing that was wrong. What found it was a scene whose flame does not cover the
  frame. **Every detector in this crate that reads a `View::Shadow` pixel should
  be asked the same question**, and the two here are unlikely to be all of them.
- `examples/two_cubes.rs` still projects world points without asking whose pixel
  it got. Phase 2 moves every other reader to `ids`; this one should go with them.
- **`tests/traced.rs` and `examples/boxes.rs` still build the same scene twice.**
  The two gates inside `traced.rs` now share one `render(Shot)` fixture — which
  is what made the brightness gate cheap to add — but the tool has its own copy
  of the whole pipeline (floor art, synthetic map, atlases, mesh rows, blit), and
  a scene is authored in one and restated in the other. `line_scene` and
  `flat`'s flame are already two spellings of numbers that have to agree for a
  failure in the gate to be reproducible by the tool. The same argument as the
  three-tread flight below, one layer up.
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
- ~~**The parity apparatus was built on `place`'s packing, which is why it could
  not have survived phase 2 anyway.**~~ **Done.** `parity_frame` and `plan.rs`'s
  `drawn` both go through `gbuffer::Fragment` for all three planes now, and
  neither spells a layout. The one thing that changed shape rather than moving:
  an **id is not a fact a fragment knows** — a world pass has one per instance
  from the rasteriser, and a fixture's is a row number it can only hand out once
  it has seen every fragment it means to draw. So both harnesses gather their
  fragments whole, key a row per distinct tile, and only then pack. `Fragment`
  carries the tile and `Fragment::ids` takes the id, which is that asymmetry
  stated in the type.
- The three-tread flight is rebuilt by hand in five tests in `light.rs` and now a
  sixth in `frame.rs`, each restating the same `Prism::new(Face::North, &[1, 3,
  5])` and the same tile bounds. It is the scene every stair defect is found on
  and it should be one constructor.
- ~~`renderer.rs`'s `depth_state()` has lost its doc comment: `PLACE_TARGET` was
  inserted between the comment and the function.~~ **Fixed.** The constant moved
  below the function it had been spliced into, and both have their own doc again.
- ~~Hand-copies of the third channel.~~ **Fixed, and then the channel went.**
  `gbuffer::Fragment` is what a G-buffer texel *is* — tile, sub, `z`, kind,
  stance — and `ids()`/`position()`/`normal()` are the only three spellings of
  the layout outside the shaders. `plan.rs`'s two closures and `frame.rs`'s
  `parity_frame` went through it; they had three copies of the fraction's
  `<< 2`/`<< 9` between them, and the id plane deleted the fraction outright.
- **A met box is not the same claim as a named solid, and one view was written on
  the assumption that it is.** `View::NormalGeometry`/`NormalSprites` (the normal
  plane split by where the vector came from — measured shape versus the picture)
  first tested `mine != SOLID_NOBODY` for "this fragment is a point of geometry".
  A picture of it is what refuted it: a static may meet a real volume the grid
  holds no *solid* for — a floor's own box stands with `edges 0b0000` and stops
  no light — so every measured face of one read as an unmeasured sprite. The rule
  landed as `statics.wesl`'s own invariant instead (a static's normal is zero
  only where the grid holds it no volume at all), and the general point is worth
  keeping: the position plane's fourth channel answers *which occluder*, not
  *whether anything was measured*, and nothing in the G-buffer answers the second
  question outright. A producer that ever gives an unmeasured static a facing
  breaks the derived test, and there is no assertion anywhere that would notice.
- **A diagnostic layer that keeps a landmark is not a layer.** The two normal
  layers claim to hold one category each, and `KIND_NOTHING` broke the claim:
  every diagnostic passes a pixel nothing drew straight through, on purpose, so
  the world's silhouette stays findable — which put a speaker's letters in the
  *geometry* picture, reported from a client dump. The three views whose subject
  is which pixels are which now paint it black instead. Worth keeping as a shape:
  a view that answers "what is here" can afford the passthrough, a view that
  answers "is anything here that should not be" cannot.
- **A gate over a real frame can be vacuous in the direction that matters.** The
  test built for the above was green with the rule deleted from the shader: this
  fixture draws no text, so its only `KIND_NOTHING` pixels are the background,
  and the background is black either way. The fix was a positive control — one
  background pixel painted white in a *copy* of the world image, which is the text
  pass's own shape (write the image, leave the id plane alone) with none of its
  machinery. Measured both ways: red with the control and the rule removed, green
  with the rule. Any future test about "a pixel nothing drew" needs the same
  planted pixel, because no scene in this repository's fixtures has speech in it.
- **`frame::Draw` filters the drawing and never the lighting**, and the two are
  one line apart in `assemble` with nothing but a comment keeping them apart. The
  cheaper implementation — hand the function fewer statics — reaches the same
  picture and silently empties the occlusion grid with it, so a room whose walls
  are "not drawn" would light up. `ticking_a_producer_off_narrows_the_drawing_and_not_the_light`
  asserts both halves because the picture cannot tell them apart. What is still
  unmodelled: `Draw::mobiles` is honoured by the *caller* (the client's own mobile
  pass), since `assemble` does not collect mobiles at all — a second caller that
  collects them and ignores the field would differ from the client with nothing
  to notice, and `Inputs::summary` printing the field is the only thing standing
  in that gap.
- 🚩 **A climbable's tread is marked a body, and a staircase is the one shape
  whose art does say which way a surface looks.** Reported by a person looking at
  a lit frame at Britain's `(1454, 1728)` — stone stairs, `0x0752`, which the art
  table reads as `corner E S prism W 1 3 5`: the flight comes out with no shading
  of its own, and what is left on it is a hairline along each step.
  <br>
  `occlusion::boxes_of` hands every tread `Edges::ANY`, and its own comment says
  why: *"a stair is solid: a body, whose occlusion is `ray_vs_solid`'s exact slab
  test rather than a lid's crossing test and a panel's run masking."* That is a
  statement about **which occlusion test this box takes**. The same mask is also
  the only thing that says **whether the art named a face at all** — the question
  the three open defects above are one question about — and for a tread the two
  have different answers. A tread's lid is a plane somebody drew and so is its
  riser on the climb axis; only the two faces *across* the climb are the tile's
  own walls. One field, two domains.
  <br>
  **Measured**, `examples/isolated_scene.rs` at that place, radius 6, `800×600`
  at `2:1`: 20,321 fragments in the flight's own window stand on a stair box. The
  shadow term on those same fragments averages **248.8 of 255** — the flame
  reaches them almost unobstructed, so what darkens the staircase is the cosine
  and not the walk. With the met box face as the normal they mean **111.8** of
  765 and 23.2% of them fall below 60; with the zero vector, lit from every side,
  they mean **260.7** and are *flat* — a tread's top and its riser come out the
  same colour and the flight loses every step it has. Neither answer is the
  staircase's.
  <br>
  **And the answer already existed once.** Phase 2's own *done when* was
  `two_mesh_faces_carry_their_own_two_normals` — "a tread's top and its riser,
  one draw, two normals" — off `facing::Prism::mesh`, whose five normals
  `place.rs` still round-trips in a test. Phase 6 replaced that pass with one
  body box a tread and dropped both vectors; the measurement is still in the
  `Prism` and nothing downstream asks it for a facing. So whatever the body
  question is answered with, a tread wants to be outside its scope rather than
  inside it, and the split wants to be two fields rather than one mask read twice.
  <br>
  Unmeasured, and the number that would size it: how many placements over
  Britain's `121×121` stand as a fitted prism under `CLIMBABLE` or
  `PLATFORM` — `examples/geometry_census.rs` already walks that window and counts
  the fitted-prism class as one line (3.2%), without separating the climbable
  from the tables and counters that reach the same branch.
  <br>
  ✅ **Fixed 2026-08-11 — `occlusion::named_edges`.** One expression with two
  readers now: `boxes_of` starts from it and keeps its own override, and
  `statics::push_volumes` asks the *graphic* rather than the box. The gate is
  `a_flights_volumes_name_the_faces_its_art_named_and_a_bodys_name_none`, and it
  is a **pair on purpose** — the same tile, the same flags, the same prism, only
  the measured `facing` differing — so the rule it holds is "the art's answer is
  what this field carries" and not "a climbable is special". Fault-injected: put
  `boxes_of`'s mask back and the first half goes red at `Edges(15)` against
  `Edges(6)` while the second stays green.
  <br>
  ⚠ **And it does not change the frame the defect was reported on.** Measured on
  the flight's own 20,321 fragments: with the art's mask they come out at mean
  **111.8** of 765 and standard deviation **59.0** — *identical to the frame
  before the body rule landed*, which is the frame in the report. What the fix
  actually bought is that the staircase did not become the flat, formless 260.7
  the body rule was about to make it: the zero-normal share on the flight is
  **0.0%**, where it had been **100%**. The thing a person is looking at is the
  entry below, and this one had to land first for it to be visible at all.
- 🚩 **A silhouette score cannot see inside its own outline, and that is where
  the surfaces are.** The finding, and it is a general one: `silhouettes_agree`
  is the only measure any fitted shape in this renderer is scored by, and it
  compares two **filled outlines**. Everything interior to the outline — where a
  step's riser stands, how deep its tread is, a moulding, a recess — contributes
  nothing to the score. Two prisms with the same silhouette and different insides
  are the same number, so a fit can be *confident and still wrong about where the
  surfaces are*, and the lighting is the pass that finds out: a facing is exactly
  what a cosine is computed from.
  <br>
  **Measured, on the reported flight at Britain's `(1454, 1728)`.** The fit is not
  ambiguous — `examples/prism_axis.rs` (new, `artscan`) ranks the whole 261-candidate
  sweep per graphic, and `0x0751` takes `North [1,3,5]` at **0.9752** with its
  entire top six climbing north and a margin of **+0.0775** over the best
  candidate on any other axis. `0x0752` the same, `West`, +0.0775. `0x0754` and
  `0x0758` `East`, +0.0945. `0x0750` is a plain `box [5]`, +0.0520. And `0x0756`,
  which the table holds no prism for, is refused with a margin of **+0.0024** —
  a coin flip between axes, which is the search saying so. Six pictures, six
  confident answers.
  <br>
  **And the insides are wrong anyway.** Over 37 east-face bands sampled across
  the flight, the model's riser and the artist's own step joint are parallel and
  roughly equal in number — median **2** model bands per screen column against
  **3** drawn joints — but the model's riser stands **10.5 view px** where the
  art's joint is **2.5**: four times too tall. So each riser band covers the upper
  half of what the picture draws as the step's *tread*. `blit.wesl` gives a
  vertical face a full cosine where a lid takes a grazing one, measured on this
  very flight at **165.4** against **11.6** of 765, so the model's misplaced
  riser draws as a bright stripe up the middle of every stone slab — which is
  what a person reported as *something extra being drawn there*.
  <br>
  **Where to take it.** The measure, not the fit. A score over filled outlines
  cannot be repaired by more candidates or a higher `PRISM_FITS`; it wants a
  second term that sees inside — the art's own interior edges against the model's,
  which is the same alpha the silhouette detector already walks. `MAX_TREADS` is 4
  and is a cap on the *measurement*, so it belongs in the same question rather
  than beside it. And the reach is not staircases: **every** fitted prism is
  scored this way, which is `geometry_census`'s 3.2% fitted-prism class — the
  tables, counters and display cases `boxes_of`'s `PLATFORM` branch admits on
  exactly the same terms.
  <br>
  **Step 1 (measure) and step 2 (`interiors_agree`) done, 2026-08-11 — steps 3
  and 4 still open.** Step 1: over the whole install, 373 multi-tread fits, 0
  with no confident interior edge at all, mean residual 8.35 view px (median
  8.45, p90 10.28, max 12.02) — `docs/lighting_state.md`'s 🚩 entry has the full
  breakdown. Step 2: `interiors_agree` (`facing.rs`) is a coverage fraction —
  of a candidate's sampled interior-boundary columns, how many find a confident
  brightness edge nearby — used by `best_prism` **only as a tie-break between
  rival climb axes within `TIE_MARGIN = 0.01` of each other on outline alone**,
  never summed into `silhouettes_agree` and never moving a fit-or-refuse
  verdict. `prism_axis`'s own duplicated projection math (`project`,
  `boundary_columns`, `luma`, `strongest_edge`) moved into `facing.rs` with it,
  so the tool and the production scorer share one copy. Measured effect: **27
  of the 309 accepted near-ties (8.7%) flip axis** under the tie-break.
  `DETECTOR` is 5.
  <br>
  **Steps 3 and 4 done the same day, and step 3 rewrote step 2's own
  measurement.** The gate is a pair in `tests/prism.rs`: a **hermetic** fixture
  (a shaded drawing of a known prism this test makes itself, so plain
  `cargo test` runs it with no install) and the six graphics of `(1454, 1728)`
  held to four decimals, both fault-injected on the art side (flattened to no
  brightness step) and the model side (rotated to every rival axis). It failed on
  its first run for a real reason: a west-climbing stair and *the same stair
  mirrored* both scored `1.0`. Two causes — `luma` counted material-over-nothing
  as a step, so the interior term was re-scoring the **silhouette**
  `silhouettes_agree` already covers; and it counted an edge's *presence* inside
  a ±16-row window while one tread rises 8 px, which answers yes to every rival.
  Both fixed: a transparent pixel is an absence, and the term measures
  **closeness** to either end of the riser (a joint is a face with two drawn
  edges, not one of them by convention). `DETECTOR` is **6**. What moved: the
  tie-break now flips **16** of 309 near-ties rather than 27, and the residual
  this whole track is about reads **4.97 px** to the nearer riser end (7.07 to
  the crest) against the 8.35 first reported — the defect is real and about half
  its reported size.
  <br>
  **Step 4 is measured and answered no.** `MAX_TREADS` at 6 and at 8 buys 15
  more fitted pictures of 2,985 (0.5%) and no accuracy at all — residual 4.97 /
  5.00 / 5.13 px at cap 4 / 6 / 8, and by profile size the three-tread fits (the
  real flights) agree at 3.98 px while every size above four sits at 5.2–6.8.
  The crowd sitting exactly on the cap never clears (120 at 4, 71 at 6, 87 at 8),
  which is an even climb approximating a shape that is not a stair rather than
  stairs the model cannot hold. It stays at four, and `facing::MAX_TREADS`'s own
  doc carries the table. **What is left of this entry is the original defect**:
  the model's riser sits ~5 px from the drawn joint, which is a *placement*
  problem — where `boundary_columns` puts a crest — and neither the tie-break nor
  the cap moves it. The move that would is using the found edge to **correct the
  profile** rather than only to choose between axes: the same three calls that
  measure a residual can solve for the tread heights that minimise it, at which
  point `interiors_agree` stops being a tie-break and starts being the fit.
  <br>
  *Two smaller things the gate's own run turned up, both now closed.* **The
  exact-tie rule is stated.** `best_prism`'s interior tie-break used
  `Iterator::max_by`, which keeps the *last* equally-best candidate — the
  opposite of the outline-score tie one line above it (`if score > best.1`,
  strict, keeps the *first*). Two unstated conventions for the same kind of
  tie in the same function; replaced with `earliest_of_best_interior`, which
  keeps the first candidate on an exact match and is pinned by its own unit
  test built from hand-chosen `f32`s rather than a picture, since real art
  essentially never produces two candidates that agree with it to the last
  bit. **The two-tread residual gap is
  answered.** `prism_axis --debug` on its worst offenders (`0x4702` Magencia
  QuarterWall, `0x51DF`/`0x5237` Virtue Floor, `0xB11B` Zen Garden Large,
  `0x4627`/`0x4617`/`0x4621` the three Spire Slope/Base graphics, all two-tread)
  against `0x42FE`/`0x42FF` Large Stairs Carpet (three-tread) for contrast —
  looked at by eye, confirmed 2026-08-11: every worst two-tread offender is a
  floor, rug, or ramp, not a staircase. The wall detector's corner test and the
  outline-only `silhouettes_agree` score both pass a shallow brightness
  gradient across a flat or sloped surface as a two-step climb; a real flight
  is rarely just two treads, so the two-tread bucket is disproportionately
  these false positives rather than short real stairs, which is why its mean
  residual (5.76 px) reads worse than the three-tread bucket's (3.98) — the gap
  is population composition, not a geometric effect the model is missing.
  <br>
  **Two earlier framings died on the way here and are worth the lines.** *The
  boxes disagree about the climb axis* — six graphics, four axes — is true and is
  not a defect: `prism_axis` says every one of the six is confidently its own
  direction, and the structure really is a stoop with steps down three sides.
  *Interior faces at the joins between abutting tiles* — the "garbage on the
  vertical joins" `statics::push_volumes`' own doc records — does not reproduce
  either: `isolated_scene` prints `0x0751`'s treads at `x 99.000..102.000`, three
  tiles folded into one primitive, so `occlusion::merge` is doing its job here.
  <br>
  The face census that was the first evidence, kept because the ratio is the
  reason the error is visible at all:
  <br>

  | face met | fragments | share | flame term of 765 |
  |---|---:|---:|---:|
  | the **lid**, `+z` | 25,216 | 71.8% | 11.6 |
  | **east**, `+x` | 4,957 | 14.1% | **165.4** |
  | **south**, `+y` | 4,927 | 14.0% | 12.7 |

  <br>
  **The two side families are the same size** — 4,957 against 4,927, which is the
  symmetry the projection has. What separates them is 165.4 against 12.7: the
  lamp stands east, so of the two only the east one is lit, and every placement
  error in it is at full contrast against a lid beside it taking a fourteenth of
  the same flame. A misplaced *lid* would be invisible in this frame. That ratio
  is why an error inside the outline surfaces as "something extra drawn there"
  rather than as slightly wrong shading.
  <br>
  **What it is not**, all measured on the same frame. Not the ground poking
  through: `View::Kind` says *static* on every one of the strips' pixels. Not the
  art: the same albedo jump is in a frame rendered with
  `OPENSHARD_LIGHT_BRIGHTNESS=0`, and under flat light the flight is an ordinary
  grey staircase with no strips in it at all. Not the height: `0x0750`, `0x0751`,
  `0x0752`, `0x0754`, `0x0756` and `0x0758` are all 44×65 pictures and 43 + 4·5
  is 63, so the fitted five `z` are the art's own — this is not
  [`footprints.md`](footprints.md)'s missing height reaching a climbable. Not the
  shadow walk: visibility averages 248.8 of 255. Not the merge: `0x0751`'s treads
  stand at `x 99.000..102.000`, three tiles folded into one primitive.
  <br>
  ⚠ **The first census of this was taken on the wrong set and is superseded by
  the table above.** It classified only the fragments that *changed* between two
  builds, which is a set defined by a shader rule rather than by the staircase,
  and it came out 88.6% lid / 11.3% south / **9 pixels** east — from which the
  east faces looked absent and the defect looked like a tie in `meets`. They are
  not absent; they are half the sides and they are the lit half. A set chosen by
  what moved is not a set chosen by what is there.
