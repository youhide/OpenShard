# Render: where it stands

The canon of the `render` domain — `client/render`, `client/artscan`,
`client/pathtrace`. It answers "is the lighting finished" in one place and says
which document holds the reasoning for each line.

**A status document, not a plan.** Six tracks ran against one renderer — the
rebuild itself and five plans that came out of it — and each was written as a
*living plan with a backlog*, which is the right shape for doing the work and
the wrong shape for answering the question above. The work that is still ahead
is in [`plans/render/`](../../plans/render/lighting/PLAN.md), not here; what is
built is here and in the `design_*` files beside it.

Nothing on this page is new work or a new decision. Where this page and a design
document disagree, the design document is right and this page is stale.

## The one-line answer

**The model is built, calibrated against a path tracer, and shipping. What is
left is not the model — it is the geometry the model is fed, two terms that were
never written (the sun's BRDF, a mobile's normal), and the content layer
(a day curve, UO's own `light.mul` mode).**

Concretely: a fragment today carries an exact world position, a measured normal,
an albedo and the name of the primitive it is a point of; it is lit by
`albedo × max(N·L, 0) × colour × intensity × windowed-inverse-square × visibility`
summed over eight stratified samples of a spherical flame, in linear radiance,
tonemapped once by an ACES fit. Every constant that used to stand in for a
missing measurement is deleted, and every deletion was gated by injecting the
fault rather than by reading the code.

What still misreads on screen is, without exception, a **box that does not fit
its picture** — which is why the two newest tracks
([`design_footprints.md`](design_footprints.md), [`design_silhouettes.md`](design_silhouettes.md)) are about
boxes and art rather than about light.

## Readiness, by subsystem

| Subsystem | State | What is left | Held by |
|---|---|---|---|
| Colour: sRGB in, linear throughout, ACES out | ✅ shipping | — | [`design_model.md`](design_model.md) phase 1 |
| G-buffer: position, normal, ids, albedo — 32 B/sample, WebGPU's floor exactly | ✅ shipping | — | phase 2 |
| BRDF: `max(N·L, 0)`, no dial, no band | ✅ shipping | — | phase 3 |
| Attenuation: windowed inverse square | ✅ shipping | — | phase 3 |
| Shadows: self-hit by primitive id, bias `0` | ✅ shipping | — | phase 4 |
| Area light: a sphere, 8 stratified rays, world-space dither | ✅ shipping | — | phase 5 |
| Every term a function of the sample point (no flame centre in the loop) | ✅ shipping | — | phase 5b |
| Impostor: one silhouette, the box met per fragment | 🟡 shipping with one hole | a corner's two panels are still told apart by the **screen half** (the box carries no instance row). The **fringe** is decided: the clamp stays, and what it costs is a position rather than a facing | phase 6, 6f–6i |
| Occluders: absolute coordinates, merged runs, a BVH, no tile in the answer | ✅ landed | — | [`design_occluders.md`](design_occluders.md) (a record) |
| Footprints: a sub-tile box measured off the art | ✅ landed, partial by design | the **height** is never measured — a roof's picture stands 76 px over a box 3 `z` tall; the remaining `Crooked` class is furniture standing on more than one thing | [`design_footprints.md`](design_footprints.md) |
| Frame assembly: one `frame::assemble`, one `Inputs`, gated plane by plane | ✅ landed | P4 items 2–4 (a `CLEAR` piece's name, the whole-tile stand-in, `PANEL_THICKNESS`) | [`design_frame_assembly.md`](design_frame_assembly.md) |
| Pixel spaces: the census, the commensurability statement, the newtypes, the gates | ✅ landed | the **art texel** is the one grid with no type | [`design_pixel_spaces.md`](design_pixel_spaces.md) |
| Silhouettes: attribution of the two edges, the seam, the clamp | 🟡 attributed | the **widths** at `4x`; the decision S2 (leave it / let the box bound more / estimate coverage) | [`design_silhouettes.md`](design_silhouettes.md) |
| Billboards (mobiles) | 🟡 half | the inflated-silhouette normal and the choice between it and the camera-facing plane — its *done when* is a person looking | phase 7 |
| The sun | ⬜ not started | it is added straight, with **no `N·L` anywhere**; no soft edge, no sky visibility as ambient occlusion | phase 8 |
| Ambient: the day curve, the sky field reaching a lit pixel | ⬜ carried | no default frame has an ambient split, so a house reads as bright as the street | [`lighting_world.md`](../archive/render/lighting_world.md) |
| UO's own light: `light.mul` / `lightidx.mul` as a picked mode | ⬜ scoped, not started | the tiledata light-id parse, both file readers, the composite point, the toggle | phase's *Wanted after the model works* |

## The pipeline, phase by phase

`geometry pass → G-buffer → lighting pass → tonemap → screen`, the ordinary
deferred arrangement, with the decision everything rests on stated once: **the
art is albedo and the light is ours.** No term anywhere argues with the artist.

| | Phase | State |
|---|---|---|
| 0 | the reference path tracer | ✅ — engine and tracer agree on open ground to one step of 255 over 262,144 pixels |
| 1 | linear and HDR | ✅ |
| 2 | the G-buffer | ✅ — `place`'s packing is gone entirely |
| 3 | the BRDF | ✅ — `FACE_EDGE` deleted |
| 4 | shadows by identity | ✅ — `STAND_OFF`, `ON_TOP`, `exemption` deleted; the light oracle reads zero at every flame height |
| 5 / 5b | area lights, then no centre | ✅ — shadows ~8× crisper; the join wedge gone, signed mean `-0.0044` → `-0.0002` |
| 6 | the impostor | 🟡 — 6a, 6c, 6d, 6f, 6g, 6h landed; 6i's item 1 (a fixture driving `statics::collect` over a fitted climbable) is what is left |
| 6e | the grid stops being a rule | ✅ — [`design_occluders.md`](design_occluders.md), all six steps |
| 7 | billboards | 🟡 — position and the camera-facing normal landed |
| 8 | the sun | ⬜ |

**The instrument is a picture beside the path tracer's, looked at by a person.**
Twelve tests whose subject was the agreement of two of our own implementations
were retired for that reason; what survives is the brute-force occlusion
oracles, the world claims, and the pictures — which are the acceptance
instrument, not a side channel.

## The pixel spaces — the spec

Normative. The derivation, the per-site census and the pair-by-pair
commensurability table are [`design_pixel_spaces.md`](design_pixel_spaces.md); this is the statement of
what a person writing code here may assume.

### The grids

| Grid | Unit | Type | Origin |
|---|---|---|---|
| Real (screen) pixel | one physical pixel | `camera::RealPixel` (whole), `camera::RealPoint` (fractional) | the window; `ViewportRect.x/y` is **window-absolute** |
| View (virtual) pixel | one world pixel at `1:1` | `camera::ViewPixel` (whole), `camera::ViewPoint` (fractional) | the rounded eye, centred in `render_width()` |
| World pixel | one world pixel at `1:1`, camera-free | `camera::WorldPixel` (`i32`), `camera::WorldPoint` (`f64`) | the map |
| Tile / world units | `TILE_WIDTH` = 44 view px; `z` in `Z_STEP` = 4 view px | `camera::Point`, `camera::WorldSpot`, `light::WorldVec` | the map |
| Tile space (metrics) | all three axes in tiles — `z` divided by `Z_PER_TILE` = 11 | `light::TileVec` | a vector space; no point type yet |
| Art texel | one texel of an art file | **none** — the one grid with no type | the sprite's own rectangle in an atlas |
| Clip space | −1..1 | `vec4<f32>` | the viewport |

### The rules

1. **`z` is divided by `Z_PER_TILE` exactly twice**, in
   `TileVec::between` and `TileVec::in_world_units`. A metric — a distance, a
   cosine, a normal, a beam axis — lives in tile space; a *position* lives in
   world units. The two are one multiplication apart and the compiler now knows
   which is which.
2. **A tile corner is always a whole world pixel**, and a whole world pixel is
   always a whole view pixel. Both conversions are exact integer arithmetic
   upstream of the zoom ladder; no rung, parity or eye fraction can move them.
3. **No primary sample lands on a whole virtual pixel** at any magnifying rung,
   at either viewport parity, at any eye fraction — the nearest a sample comes
   is `0.5 / scale`. This is what stops a view ray passing exactly through a
   box's corner, where `impostor::meets`'s tie has no right answer. One
   exception is listed and asserted to *reproduce*: at `2/3x` the eye's quantum
   is `1.5`, so half of all camera positions there reach the corner.
4. **The fragment grid and the impostor's tile space are never commensurate, and
   the tolerance between them is a quantum rather than an epsilon.**
   `impostor::FRAGMENT` is `SQRT_2 / TILE_WIDTH` — the distance to the next
   sample, in the space the comparison is made in. A rounding epsilon in that
   role measured a floor's own seam as "outside its own box" and drew a glowing
   grid across every room.
5. **Below `1:1` there is no primary sample to land anywhere**: the world is
   drawn at `1:1` into an oversized image and the blit's linear sampler shrinks
   it. The whole commensurability question is about magnification.
6. **A constant that crosses into a shader is pinned from the shader's own
   source.** `TILE_WIDTH`, `Z_PER_TILE`, `Z_STEP`, `HALF_TILE_HEIGHT` and
   `FRAGMENT` have no compiler on either side of the wire, and a disagreement
   there does not fail to build and does not fail to draw — it draws a different
   frame from the one every test asserts about.

### The gates that hold them

| Rule | Gate |
|---|---|
| 2 | `grids.rs`'s `a_tile_step_is_a_whole_number_of_world_pixels`, `a_height_unit_is_a_whole_number_of_tile_space_units` |
| 3 | `camera::tests::no_primary_sample_lands_on_a_whole_virtual_pixel` (all seven rungs × both parities × every eye fraction, ~121k samples, with the `2/3x` exception counted); `tests/parity.rs`'s odd-versus-even frame comparison |
| 4 | `impostor::tests::every_pixel_of_a_blocks_picture_meets_that_blocks_own_box`, with a floor under the constant so halving it goes red |
| 6 | `grids.rs`'s `the_shaders_restate_the_cameras_constants_and_not_their_own` — reads the numbers back out of the `.wesl` source |
| the tie itself | `impostor::a_ray_through_a_boxs_own_corner_is_answered_by_the_order_of_three_ifs` — a record of the rule, not an endorsement of it |

### What is not typed, and why

- **The art texel.** Its hazard is real but is *between two conventions of one
  grid* — `LandAtlas` and the statics atlas divide exactly, `TexmapAtlas` insets
  by half a texel — so a newtype over the texel does not stop it. What would is
  a type carrying the convention, and that belongs with
  [`design_silhouettes.md`](design_silhouettes.md).
- **`geometry::Rect`** is a sprite's rectangle, an atlas rectangle, a gump's
  place and a plan pixel's. A shape shared by four spaces is a different problem
  from two spaces sharing one number.
- **A tile-space *point*.** `TileVec` is the vector; `Vec2` still means a
  tile-space position in `light.rs` (`Light.at`, `Spot::at`). The confusion is
  reachable — the `ViewPoint` sweep annotated `debug.rs`'s tile-space `middle`
  as a view point and the compiler caught it.

## What is left, ranked

**1. The height nobody measures.** The largest single number on the whole
track: the whole-tile class discards **32.7%** of its own art and roofs inside
it 44–53% — `0x05A2` "slate roof" is 48×76 pixels of picture standing on a box
three `z` units tall. Every fringe artefact below is downstream of it.
[`design_footprints.md`](design_footprints.md) deliberately measured the *footprint* and left
the height as a carried item, with `blocks_silhouette` named as the instrument
that would score it. **Measured 2026-08-10** (`geometry_census`, same window):
of the 3,388 whole-tile stand-ins, **2,825 (83.4%)** carry `ROOF` — a sloped
plate, which height alone does not turn into an AABB. The other **563
(16.6%)** are the actual target of "grow the box to the silhouette"; roofs are
a separate primitive question, not a height question, and stay out of A-2's
scope.

**2. ~~The fringe~~ — decided 2026-08-10: the clamp stays, and the serration
was never the larger number.** A pixel whose ray misses its box is clamped to
the nearest point on it. That was already better than the two alternatives tried
and measured (drawing nothing: 11.09% of every panel's art and 32.4% of a
whole-tile one; no facing: lit from every side, reverted). The last open
candidate — *give a miss the face the sprite's own volume presents* — was
written, run and refused: it does end the comb inside an overhang (0.22% → 0.02%
of neighbouring pairs) and it pays 0.30% → **32.59%** at the *join* to the art,
**97.68%** for panels, a hard line along the top of every wall. The reason is
one number nobody had: **91.79% of the art bordering an overhang is on the box's
own lid**, because an overhang hangs *above* its box. And the control from the
same walk says two neighbouring pixels that both **hit** disagree at 1.35% —
six times the rate two misses do. What remains is a *position* lie, bounded by
the overhang and so by item 1 above; the code is `impostor::presented_face`,
kept only so `examples/discard_census.rs` can re-take the number.

**And all three answers are a switch now**, because the acceptance instrument
here is a person looking at a frame and two of the three had only ever been
argued about. **F2 cycles it**, the way F11 cycles the lighting views —
`clamp → discard → volume`, with the state on a log line each time. For a run
that has to *start* somewhere — a frame dump, a screenshot, a bug report:

```sh
OPENSHARD_FRINGE=discard cargo run -p openshard-playground
```

**And what the switch actually shows, measured on one real frame** (Britain
`(1501, 1659)`, radius 12, `960×720`, `isolated_scene`, which reads
`OPENSHARD_FRINGE` too):

| against the clamp | pixels that change |
|---|---|
| `discard`, the lit frame | **46,427 of 691,200 — 6.7%**, and they are not scattered: a stripe out of every course of every roof, so the house is a colander with its own crates showing through |
| `volume`, the *normal* plane | 1,691 — **0.245%**, thin lines at the ends of wall runs |
| `volume`, the lit frame | **0** |

The last row is the one worth keeping: the refused rule changes a facing that
**nothing in that frame was lit by**, so it is invisible where the clamp's own
position lie is not. It is also the shape of the answer to "I pressed the key
and saw nothing" — `discard` is unmissable, so a frame where *nothing* moves is
a frame the switch never reached.

**Daylight is not the reason it might not reach.** A frame with no sky builds an
*empty* grid rather than no grid, and `statics::push_volumes` calls `boxes_of`
regardless, so a sprite is met against its own per-tile box either way — with
`_IMPOSTOR=0` the same two states still differ by 9.7% of the frame. What a grid
adds is the box's *name* and its merging, not its existence. (This paragraph is
here because the opposite was written down first and measured false.)

`impostor::Fringe` is the enum and `SpriteRenderer::set_fringe` is where a frame
takes it. `frame.rs`'s `the_fringe_switch_draws_three_different_frames` holds
that the switch reaches the picture at all, in the direction each state claims —
verified by injecting a dead uniform slot, not by reading the wiring — and
`grids.rs` pins the three numbers against the shader's own constants.

**And for its first day it reached every tool and nothing in the client**, which
is worth the paragraph because the report was exactly the sentence written above
as a joke: *I pressed the key and saw nothing.* ✅ Fixed 2026-08-10.
`SpriteRenderer::render_mask` — the silhouette pass, which shares this pass's
uniform block on purpose so that a ring lands where the picture did — wrote that
block with a literal `0.0` in the slot the fringe now lives in, left from when it
was padding. **A `queue.write_buffer` does not run where it is called**: every
write staged before a `submit` is applied at the start of that submission, so
the *last* write to a range is what every draw in the frame reads, including one
recorded into the encoder earlier. `App::draw` rings its silhouettes after
drawing statics and submits once, so the client's statics pass read `clamp` on
every frame whatever F2 said. No tool and no test ever called `render_mask`, so
all three states were honestly measurable everywhere except in the client — the
`docs/render/design_frame_assembly.md` thesis, arriving through a pass rather than through an input.
The gate now draws the ring: `render_places_with_fringe` runs the silhouette
pass with nothing to ring, which is what the client does on a frame that
highlights nothing, and the old code turns it red with the message it deserves
(`discard removes 0 and adds 0`).

**3. Phase 7's second half.** A mobile's normal is one vector for the whole
sprite, so a torch on a figure's left reads no brighter than one on its right.
The inflated-silhouette candidate is unbuilt, and the choice between the two
wants a picture of a figure beside a lamp — which wants a mobile pass in
`examples/isolated_scene.rs`, which does not exist.

**4. Phase 8, the sun.** No cosine, no soft edge, no sky visibility. The sky
field is ambient occlusion by another name and phase 8 is where it is adopted.

**5. The content layer.** The day curve, lights carried by other mobiles, the
flame's own glow, the sunbeam through a window, land as an occluder, and UO's
own `light.mul` mode as a switch beside the deferred pipeline.

**6. The instruments.** The tracer is single-threaded at 13 s a frame and has
never been run over a real map; `tests/dump.rs` draws at even extents only; no
gate holds that a debug view is drawn from the same planes the lit frame is.

## Open defects a person can see

Each of these has been reported by somebody looking at a frame, and each is
measured rather than guessed at. None is a defect in the model.

| | What it looks like | What it is |
|---|---|---|
| 🟢 | **A flame's own sprite is black.** Every free-standing emitter taller than `FLAME_LIFT` was | Fixed 2026-08-11, and not by anything about emitters: **a body writes no facing**. The flame burns at the tile's centre, *inside* the lamp post's own box, and the impostor was answering the sprite with the camera-facing face of a box the art named no side of — a plane nobody drew. `impostor::Volume` carries the box's own `Edges` now and `statics.wesl` writes the zero vector for `EDGES_ANY`, keeping the measured normal for the panels and lids where the art really does say which way a surface looks. Over the reported frame: **10.21% of 307,200 pixels**, worst step 168; over the lamp's own picture, black pixels **82.7% → 39.1%** |
| 🟢 | **A sprite's top edge is serrated** | Measured and closed 2026-08-10. The flip is real and it is **0.22% of neighbouring pairs inside an overhang**, against 1.35% for two pixels that both hit — the overhang is smoother than the picture it hangs off. The rule written to end it draws a worse edge at the join; see the rebuild's own entry |
| 🟢 | **A whole-tile body reads dark and striped** | Fixed 2026-08-11, the same change and the same line: a body no longer writes a camera-facing normal it has no right to. The answer to "what should a body write for a normal" is **nothing** — `normal_format.wesl`'s middle state, a measurement that was never taken said so, which is the rule `light::mounted_at` already spells for the same cell. The cost, named: a body is lit from every side, so a crate has no shading across its own faces. That is the pre-6c picture for exactly the set 6c had no measurement for, and what improves on it is a *measured* facing rather than a better guess |
| 🟢 | **Dashes along a table's own top** | Fixed 2026-08-11, `impostor::RIM`. Where counters abut at one `z` the boundary row of the **lid** was answered with a side face — 464 pixels of one client dump and 270 of another, `shadow` and `reach` identical across the line while `flames` went 15×, because `blit.wesl` gives a vertical face a full cosine where a lid takes a grazing one. Not the fringe (F2 moves the picture and leaves these), and not the missing name either: it is the box's **own top edge**, where the side's exit beats the lid's by less than the distance to the next sample. `FRAGMENT` a third time, after the hit tolerance and `shows_a_side`. Over Britain's 121×121: 2,916 fragments of 1.6M move to the lid, **2,322 fewer neighbouring hits disagree**, and every population the rule does not touch is unchanged to the pixel |
| 🟡 | **Specks and dashes on an indoor floor** | The other half of the same report, still open: furniture drawn wider than its own per-tile box, where the pixel over the boundary belongs to a static whose box is a tile away and its ray leaves through a side face. 32 of 66 are pieces the grid holds nothing for, so no identity can excuse them. The edge rule above does not reach these — they are a *different* box, not this one's rim |
| 🚩 | **A fit is scored on its outline, and the surfaces are inside it** | `silhouettes_agree` compares two **filled** silhouettes, so where a step's riser stands inside the shape contributes nothing to the score. At `(1454, 1728)` every one of the six stair graphics is fitted confidently — `prism_axis` ranks `0x0751` at 0.9752 with a +0.0775 margin over any other axis — and the risers are still 10.5 view px where the art's own joint is 2.5. A vertical face takes a full cosine and the lid beside it a grazing one (165.4 against 11.6 of 765 on this flight), so the misplacement draws as a bright stripe up each slab. **Not staircases only**: every fitted prism is scored this way, `geometry_census`'s 3.2%. **Measured 2026-08-11 over the whole install**, `prism_axis` grown into the residual tool: 4,362 pictures read as a corner, 2,985 fit a prism and 1,377 do not; every one of the 373 multi-tread fits was measured against the art at each internal step (0 found no confident edge at all), and the offset is not scattered — mean 8.35 view px, median 8.45, p90 10.28, max 12.02, the same order as the one flight this was found on. **And the axis question the handoff's open question asked has an answer, the same run gives it**: a coin-flip margin between rival climb axes (< 0.01) is rare among the pictures that fit (309 of 2,985, 10.4%) and common among the ones the search refuses outright (632 of 1,377, **45.9%**) — a near-tie is disproportionately a *refused* picture's signature, which is the "a single prism does not hold this shape" hypothesis surviving contact with the whole population rather than one staircase. 120 of the 373 multi-tread fits (32%) use all four of `MAX_TREADS`, which is `docs/render/design_model.md`'s backlog step 4's own gate: the cap is worth raising. **`interiors_agree`, step 2, built and wired 2026-08-11**: `facing::best_prism` now breaks a tie between rival climb axes (within `TIE_MARGIN = 0.01` of each other on outline alone) by which one's interior joints the art actually agrees with, rather than the first the sweep happened to find — `prism_axis`'s own duplicated projection math moved into `facing.rs` alongside it, so there is one copy of the alignment rule instead of two. Measured on the same install: **27 of the 309 accepted near-ties (8.7%) flip axis** under the tie-break — a real, if modest, effect on live content, not just a hypothesis. It answers *which* axis, not *where* the riser sits: the 8–12px offset this entry opened with is unchanged. **Steps 3 and 4 closed 2026-08-11, and step 3's gate immediately took two numbers off this entry.** The gate is a pair — a hermetic fixture (`tests/prism.rs`, a drawing this repo makes of a known prism, run by plain `cargo test`) and the six graphics of this flight held to four decimals — and it is fault-injected on both sides: the art flattened to no brightness step at all, and the model rotated to every rival axis. **What it caught the first time it ran**: a west-climbing stair and the *same stair mirrored* both scored a perfect `1.0`. Two causes, both real. `luma` counted a drawn pixel over transparency as a brightness step, so the interior term was scoring the **silhouette** — the very thing `silhouettes_agree` already scores; a transparent pixel is now an absence. And the term counted *presence* of an edge inside a ±16-row window, while one tread of a five-`z` flight rises 8 px — a window wider than the thing it resolves answers yes to every rival, so it now measures **closeness** instead, to either end of the riser rather than to one of its two edges by convention. On the fixture the right axis now scores `0.94` against `0.75`–`0.77` for its rotations. On the install: the tie-break moves **16** of 309 accepted near-ties, not 27, and those 16 rest on a measurement that can tell a stair from its mirror. **The residual this entry's headline number is, restated**: 7.07 view px mean to the crest (the old 8.35 was partly the silhouette), and **4.97 px to the nearer end of the riser** (median 4.87, p90 7.15) — the defect is real and is about half what it was reported as. **Step 4 measured and refused**: `MAX_TREADS` was run at 6 and 8. It buys 15 more fitted pictures of 2,985 (0.5%) and no accuracy — the residual is 4.97 / 5.00 / 5.13 px at cap 4 / 6 / 8, and broken out by profile size the three-tread fits (the real flights) sit at 3.98 px while every size above four sits at 5.2–6.8. The pile-up at the cap never clears (120 at 4, 71 at 6, 87 at 8), which is the signature of an even climb approximating a shape that is not a stair. The cap stays at four; `facing::MAX_TREADS`'s own doc carries the table |
| 🟢 | **…and its treads claimed to be a body** | Fixed 2026-08-11, `occlusion::named_edges`. `Volume::edges` was filled from `boxes_of`'s mask, which overrides the art with `Edges::ANY` on a climbable to pick the slab test — read by `statics.wesl` as *the art named no face*, it would have made every staircase flat and formless (the flight's zero-normal share was 100%, now 0%). One expression, two readers: what the grid occludes with, and what the art named. It restores the frame above rather than changing it |
| 🟡 | **A corner's two panels disagree near the tile corner** | The id follows `split_corners`' twin row and a `Volume` carries a `SolidId`, not a row number, so the *identity* is still picked by which half of the sprite a pixel was drawn on while the *normal* is picked by the box |
| 🟡 | **A north or west wall's face is a fifth of a tile inside its room** | `PANEL_THICKNESS` fattens inward, so two walls of one run drawn on one plane get positions four fifths of a tile apart. The construction that removes it is one slab straddling the shared edge |

## The map: which document holds what

The rebuild consolidated seven plans; five more came out of it since. All of
them stay — the reasoning is worth more than the code it justified — but they
are sorted by role now rather than by whether they are live.

**Design — how it works today:**

- [`design_model.md`](design_model.md) — the model itself, phases 0–8. Still the
  entry point for anything about light. *(Its phase journal and its backlog have
  not yet been split out into `evidence/` and the plans; see the note at the
  bottom of this page.)*
- [`design_pixel_spaces.md`](design_pixel_spaces.md) — the six grids, all four
  phases done. The spec above is its normative half.
- [`design_occluders.md`](design_occluders.md) — the grid stopped being a rule.
  All six steps green; the four findings that outlive it are in the model's
  backlog.
- [`design_footprints.md`](design_footprints.md) — a static's box is the box the
  art drew. Landed, with the height as its own next census.
- [`design_silhouettes.md`](design_silhouettes.md) — the two edges, the seam
  inside the picture, and the clamp.
- [`design_frame_assembly.md`](design_frame_assembly.md) — one frame however it
  was asked for. P1–P3 and P5 landed; P4's remaining three items are geometry,
  which is `design_footprints.md`'s ground and the model's.
- [`design_outline.md`](design_outline.md) — a hard edge round a sprite and the
  glow behind it, and how it composes with the highlight hue.
- [`design_text_sizes.md`](design_text_sizes.md) — a real font size, not a scale.

**The rest of the domain:**

- [`evidence/pitfalls.md`](evidence/pitfalls.md) — the traps, each one having
  cost a session.
- [`reference/path_tracer.md`](reference/path_tracer.md) — the reference tracer
  the model is calibrated against.
- [`research/font_upscaling.md`](research/font_upscaling.md) — the `fonts.mul`
  super-resolution experiment.
- [`runbook_gump_render.md`](runbook_gump_render.md) — the offline gump preview
  renderer: no window, no GPU.
- [`../archive/render/`](../archive/render/README.md) — the twelve documents of
  the engine that was replaced, with their session logs. Read one for its
  reasoning, not to find work.

**What is not built** is in [`plans/render/`](../../plans/render/lighting/PLAN.md):
phase 6i's last hole, phase 7's mobile normal, phase 8's sun, the content layer,
and [silhouettes' own widths and S2](../../plans/render/silhouettes/PLAN.md).

## The numbers to re-take

They are the ones every "how much of this is a crutch" argument is made from,
and they are all produced by tools in the tree
(`examples/geometry_census.rs`, `examples/discard_census.rs`,
`examples/footprints.rs`). Britain, `121×121` around `(1501, 1659)`, 11,184
statics:

| | |
|---:|---|
| 3.2% | a fitted prism — the only box whose *shape* came out of the picture |
| 39.6% | a lid — measured, `LID_THICKNESS` deep since P4.1 |
| 25.4% | panels on the edges the silhouette named |
| ~1.5% | a measured footprint, narrower than its tile (164 placements, new) |
| ~29.6% | **a whole tile, because the art would not say** — was 31.6% |
| 32.7% | of statics are a point of no primitive at all |
| 15.1% | of the world is a `CLEAR` piece handed a box with real height |
| 13.55% | of drawn static art misses its own box (7.82% with the roof cut) |

Re-run all three after anything on a backlog lands, and record the arguments
beside the answer — a census whose radius nobody wrote down has already caused
one contradiction between two documents.
