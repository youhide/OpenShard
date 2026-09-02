# Evidence — the lighting rebuild, phase by phase

*Recorded 2026-08-11, lifted verbatim out of `design_model.md`'s phase journal.
This is the account of how phases 0–8 were built, what each one measured and
what each one got wrong. It is a record: the live queue is
[`../README.md`](../README.md) and what is unbuilt is in
[`plans/render/lighting/PLAN.md`](../../../plans/render/lighting/PLAN.md).*

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
| 6e | the grid stops being a rule | ✅ landed [`design_occluders.md`](../design_occluders.md) | **All six steps are green.** The grid is out of the walk on both backends, and S3b's merge folds a run of wall into one primitive — 73 pieces to 9 on the crate's own two-storey house, with no pixel moved. That document is a **record** now, and the four findings that outlive it — the aperture still measured in a tile, the instruments that could not see a merge (closed since — one of them was drawing it), `PANEL_THICKNESS`'s fattening the merge turned out **not** to answer, and `footprint`'s `i32` ranges — are in this document's backlog |
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
  reading did not survive**: `docs/render/design_occluders.md`'s S4 deleted `same_run` outright,
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
it*, the habit `docs/render/design_occluders.md`'s S3 paid for. Two of the three came back with
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
  `docs/render/design_occluders.md`'s S4 has the deletion.

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
  ceiling is WebGPU (`lib.rs`, `docs/archive/render/lighting.md` decision 30.5) and `blit.wesl`
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
undone `docs/archive/render/lighting.md`'s decision 27 — a lamp beside a wall lighting its cap
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
`docs/render/design_occluders.md`'s D6, which that plan decided and did not do.)* With 6f and
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

**Phase 6e — the grid stops being a rule.** 🚩 **[`docs/render/design_occluders.md`](../design_occluders.md)
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
  `docs/render/design_occluders.md`'s S1.** `occlusion::Solid::box_from_footprint` reconstructed
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
the grid itself at `docs/render/design_occluders.md`'s S5 rather than before it.

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
