# Silhouettes: the two edges that meet along one line

A living plan, and its own session. The backlog at the end is where the next one
starts.

## The root

**A magnified frame draws its outlines at two different resolutions, and they
are adjacent.** Measured at Britain on the client's own `4x` dump
(`1919x2077`), as the number of rows a silhouette holds one column before it
steps:

```
an impostor box's edge        every 1–2 rows      decided per fragment
a billboard's own alpha edge  4, 8, 12, 16, 20…   decided by the art's texel
```

The second is not a defect on its own: the quad is scaled by
`Projection::scale` and sampled `nearest`, so one art texel is `scale` real
pixels square and its edge *cannot* be finer. Neither is the first. What a
person sees — and dislikes — is the two of them meeting along one line: a wall's
box face and the same wall's drawn outline, one side crisp and one side in
four-pixel stairs.

**The colloquial name for it is the zigzags, and this plan is what turns that
into a measurement.**

**Z1 has since amended this.** The second line is not an outline at all: a box
miss is no longer discarded, so the picture's silhouette is the art's and all of
it, and what the fragment grid draws is a *seam inside* the picture. The section
below is the question as it was asked; the phase records what it turned out to
be.

## What has *not* been established, and it is the first thing to fix

The 4-row number above was measured on a **mobile**, and a mobile is the one
case that is certainly art-bounded: `statics.wesl` takes the `is_mobile` branch
straight to `billboard_normal()`, meets no box and clips against nothing. A
static with a volume is a different story — the same shader ends its box meet
with `if !hit(best) { discard; }`, so its outline is *already* clipped to the
box wherever the art overhangs it.

So the honest statement of what is known today is:

- a mobile's outline is art-quantised — **certain, from the code**;
- a static the grid holds no volume for is art-quantised — certain for the same
  reason, and it is already its own layer (`View::NormalSprites`, `5e52279`);
- a static with a volume is bounded by **whichever of the two ends first, per
  fragment**, and *nobody has measured which one that is anywhere*.

The zigzags a person points at have therefore not been attributed. That is the
whole reason the view comes before the fix.

## The target

**Two debug views in which a fragment is in exactly one**, in the shape
`View::NormalGeometry` / `View::NormalSprites` already established: a picture
that answers "what bounded this outline" rather than "what colour is this
outline". Then, with the attribution in hand, a decision about the coarse one.

## The decisions to make, and the candidates

**S1. What the split is read off. — SETTLED, and both of the cheap candidates
were refuted before a line was written.** The precedent held: the normal split's
first rule was "the solid the fragment names" and a dump of it *refuted* that
rule. Here the refutation came off the code rather than off a picture.

1. ~~**`Meeting::outside`, carried into the G-buffer.**~~ **Vacuous.**
   `impostor::hit` *is* `outside <= FRAGMENT` — `TANGENT`, `1e-4` of a tile,
   when this was written; one step of the sample grid now, and bounded either
   way, which is what makes the number a bound and not a rim.
   While the box-miss discard stood, every surviving fragment therefore measured
   at most that, and the plan's own sentence above — "a fragment at the box's rim
   sits at the tangent limit; one in the art's interior sits near zero" — is
   wrong in both halves: both sit at zero. The number carries no information
   about a neighbour, because it was never about one.
2. ~~**A neighbourhood test in the blit.**~~ **Cannot attribute.** The blit sees
   *that* a neighbour is not a static and never *why*: the G-buffer holds one
   answer per pixel and no record of the art rectangle a pixel came from, so it
   cannot re-ask either mask. It can find a silhouette; it cannot name what
   ended one.
3. **The texel grid, drawn.** Still unbuilt, and no longer needed for the
   attribution — see the backlog, where it survives as a picture of the quantum.

**4. Both masks tested in the producer, four screen neighbours each. — BUILT.**
`statics.wesl` is the one place both are alive. Two bits ride at the top of the
id word (`place_format.wesl`'s `IDS_EDGE_ART` / `IDS_EDGE_BOX`, which cost the
row field two of its twenty-six bits and left twenty-four), and two views draw
them: `View::SilhouetteArt` and `View::SilhouetteBox`.

- `art_edge` — a neighbouring **texel** fails the alpha test this fragment
  passed, or lies outside the sprite's own rectangle in the atlas.
- `box_edge` — a neighbouring **fragment**, one real pixel away
  (`1 / viewport.scale` virtual pixels), meets none of this instance's boxes.

The cost is four `textureLoad`s and four extra runs of the selection loop per
static fragment, always on. It was gated on the view for one draft and the gate
was taken out: a G-buffer whose content depends on which picture is being asked
for is exactly the coincidence `docs/parity.md` is about, and it would have made
the flag travel from a diagnostic into a world pass.

**S2. What to do about the coarse edge, once it is attributed.** Not decided,
and the three are not variations of one answer:

- **Leave it and say so.** The art is pixel art and the client it is compatible
  with drew it this way. Then the work is one paragraph in `docs/style.md` and
  the views above, so the next person who notices reads the answer instead of
  re-deriving it.
- **Let the box bound more of it.** `docs/footprints.md` and
  `docs/occluders.md` are both making boxes fit the art better. A tighter box
  clips more of the outline, which moves fragments from the art's grid onto the
  fragment grid — *for free, as a side effect of work already planned*. This is
  why S3 below insists the ratio is measured before and after those land.
- **Estimate coverage instead of sampling `nearest`.** D11's argument for whole
  rungs is about *position* — a texel landing on a whole number of real pixels,
  so a whole pixel of camera movement translates the picture. It is not
  obviously an argument about *alpha*. An outline that resolves its own coverage
  at the magnified resolution is a different-looking engine and needs stating as
  such before anyone writes it.

**S3. The ratio is the measurement, and it has to be taken twice.** "How many
silhouette fragments are art-bounded" is one number per frame, and it will move
on its own as the footprint work lands. Take it now, take it after, and record
both — otherwise a change in the picture gets attributed to whatever was being
worked on at the time.

## Phases

### Z1 — the attribution — **done, and it answered a different question**

S1.4 wired through the G-buffer, both views in `debug::View::ALL`, and the
counts at Britain's `(1501, 1659)`. `tests/dump.rs`'s
`the_two_silhouette_layers_are_two_lines_and_a_frame_agrees_about_both` is the
gate: the six colours the branch spells and nothing else, the two views agreeing
pixel for pixel, land and background in neither layer, and — the rule made to
answer wrongly — **every fragment in the box layer carries a measured normal**,
which a `box_edge` that had quietly been reading the art's alpha would break on
the unmeasured remainder of every sprite.

**The finding, and it is not the one the plan expected.** While Z1 was being
built, a parallel change took the box-miss discard out of `statics.wesl` (its own
census: the discard threw away 11.09% of every panel's art and 32.44% of every
whole-tile one — a display case lost its whole top). So the two edges are no
longer two candidate bounds on one outline:

- the **art's** edge is the picture's silhouette, all of it, and it is
  texel-quantised — `scale` real pixels a step, and it cannot be finer;
- the **box's** edge is a seam *inside* the picture, one real pixel a step, where
  the region a box genuinely covers ends and the rest of the sprite carries on.

**Amended twice since, and the second half of that sentence is what moved.** The
remainder past the seam carried on "at the tile's centre with no facing" when Z1
was written, and a zero normal is lit from every side — which is what made the
seam a line between two *lighting rules* and, on a floor, a glowing grid. It is
clamped onto the box it came nearest now (see the two sections below), so both
sides of the seam carry a measured face and the seam marks a weaker thing: past
it, a fragment's position is a box's rim rather than a point its ray went
through. A wide band of it is a box that does not fit its art, which is
`docs/footprints.md`.

That answers the backlog's first 🚩 outright: the zigzag a person points at is
the art's outline, and the fragment-fine line beside it is not a silhouette at
all. It is still a visible line — the two sides of it are lit by different rules
— which is why both layers are kept.

**Britain `(1501, 1659)`, 900×700, night, roof cut, `1:1`:**

```
art only    155        the picture's outline where no box ran out under it
box only     96        the measured region's seam, strictly inside the art
both        473        the seam reaching the outline
            ---
            724 edge fragments of 8075 static ones, 0 mobile
```

Two thirds of the edge is *both*, which is the box fitting the art's outline
well; the 96 are where it does not, and they are the pixels `docs/footprints.md`
is about.

**What Z1 did not get: the widths.** The two edges are two *rules* at every
magnification and two different *widths* only above `1:1`, so the plan's root
claim — one steps by `scale` pixels, the other by one — needs a magnified frame.
A magnified frame is now assemblable and the measurement is the next step; what
stood in the way was not a defect. See "The `4x` frame, and the blocker that was
not one" below.

### The `4x` frame, and the blocker that was not one

Z1 recorded a 🚩🚩 blocker: `frame::assemble` at `4x` over Britain's
`(1501, 1659)` returns 595 quads of land and **zero** static quads, where
`statics::visible_graphics` over the same camera offers 140 graphics for the
atlas. The suspects named were `statics::place` returning `None` and `on_screen`
against `render_width`.

**Neither. The cull is right and the scene was the wrong scene.** Counted through
the same public arithmetic the collector uses, at that eye tile:

| zoom | drawn image | walked | placed | on screen |
|---|---|---|---|---|
| `1x` | 900×700 world px | 3513 | 1300 | **9** |
| `2x` | 450×350 | 2019 | 502 | **0** |
| `4x` | 225×175 | 1381 | 185 | **0** |

The nine statics `(1501, 1659)` draws at `1:1` are one wall run at tiles
`(1484..1487, 1663..1666)`, and every one of them lands in the **top-left corner**
of the image — `x` from −34 to 32, `y` from −20 to 112, four hundred-odd pixels
from an eye that sits at the middle of a 900×700 frame. Magnifying shrinks the
drawn image around that eye (`render_width` is `world_pixels(900)` = 225 at `4x`),
so the cluster leaves the frame, and a cull that kept it would be the defect.
Nothing between `visible_graphics` and `collect` rejects anything it should not:
the same three zooms over `(1486, 1664)` — standing *on* the wall run — collect
109, 54 and 30 statics.

The lesson is the plan's own, not the renderer's: **`docs/parity.md`'s coordinate
is a `1:1` coordinate.** It is where a person stands to look at a lit house
corner, and a magnified frame taken from it is a frame of ground. Every question
this plan asks above `1:1` is asked from `ON_THE_WALLS` — `tests/dump.rs`'s
`(1486, 1664)` — and `draw_britain` takes the eye tile as a parameter so that the
two cannot be confused. `the_magnified_frame_over_a_wall_run_still_collects_its_statics`
is the gate: every rung collects statics over the wall run, and the same `4x`
that collects none over `AT` still collects its land, which is what attributes the
zero to the place rather than to the zoom.

### The glowing grid, and the tolerance that was in the wrong units

Reported as *"the tiles inside the house are lit up now"*, off the client's own
F12 dump at Britain's `(1501, 1655)` — not a lighting defect at all, and this
plan's own subject: **the two grids, and a threshold stated in the wrong one.**

What the dump says, counted rather than looked at. In a 120×120 window over the
shop's floor, `normal.png` holds exactly three colours: `(128,128,255)` the
floor, `(128,255,128)` a wall, and `(128,128,128)` — **the zero normal**, 360
pixels of it, in a stepped dashed line along every tile seam. Those same pixels
read `SOLID_NOBODY` in `solid.png`, the art bit and not the box bit in the two
silhouette layers, and white in `shadow.png`: nothing is shading them. Across
the whole frame they are about 4% of it.

That is `statics.wesl`'s unmeasured branch — the tile's centre, no facing — and
`blit.wesl` lights a zero normal *from every side*, with no cosine. So each of
those pixels comes out brighter than the measured floor around it, and the seams
draw as a lit grid over the room. Z1 had already named the state; what nobody had
asked is **why a floor's own pixels were landing in it.**

**Because `impostor::hit` was measuring a sample against a rounding epsilon.**
`TANGENT` was `1e-4` of a tile, sized against the `3.5e-6` a ray rounds to at a
box's own corner. The misses are not rounding: `examples/discard_census`'s
positive control draws a whole-tile block's own silhouette against that block's
own box — one shape, read two ways, nothing to overhang — and reported **44
misses of 1936, every one under one fragment, the worst `1/TILE_WIDTH` of a
tile.** One row of the tile's width, which is the seam.

So the threshold is one **fragment** now (`impostor::FRAGMENT`,
`SQRT_2 / TILE_WIDTH`): the distance to the next sample, in the tile space the
comparison is made in. Not a fudge and argued from both ends — above what the
sample grid can produce (`0.71` of it), under where the picture itself starts
distinguishing (the next pixel). Over Britain's 121×121 it moves the discard
from 13.48% of drawn art to 11.83%, and the control from 44 to **0** at both
heights while the negative control — the same picture against a box a hundred
tiles away — still misses everything.

The gates are `impostor::tests::every_pixel_of_a_blocks_picture_meets_that_blocks_own_box`
(the control, plus a floor under the constant's size, so halving it turns red —
witnessed by mutation), the census's own control line, and `tests/grids.rs`'s pin
on the shader's copy of the number.

**And what it leaves.** The line was one of two populations, not the whole of
them: 11.83% of drawn art still misses, running out to 133 fragments, and that
is real overhang for `docs/footprints.md` rather than sampling. A roof gives up
40% of its art; a whole-tile claim, 30%.

### Which is why the clamp came back

With the seam gone, "no measurement" was left standing for the overhang alone —
and *lit from every side* is not a defensible answer for it either. So the rule
is the one the shape of the problem always asked for: **a fragment is a picture
of one static, the grid holds volumes for that static, so which volume is a
question the ray answers and whether the ray landed inside one is not a question
about whether anything was measured.** The nearest box wins outright, hit or
miss; "no measurement" now means a static the grid holds no boxes for at all.

The clamp had been tried and killed once, by the lattice of wall-shaded dots —
a fragment falling sideways off a *lid* takes whichever face exits first, which
is a side one. That is cured at the root rather than avoided:
`impostor::shows_a_side` refuses a face thinner than the grid that reads it, and
a lid's side face is `LID_THICKNESS` tall — a **sixteenth of a pixel**. The old
rule compared against zero, which was exact only while a lid was a plane.
Counted before the change: 338 pixels over Britain's 121×121 would have taken
one; after, none.

**Read back off a client dump rather than off the gates**, since the census and
the pass share `impostor::meets` and cannot independently confirm each other:

```
zero normals in one frame     2.07% of it   →   none
side faces ringed by lid      the lattice   →   none
```

What is *not* established is that a clamped position is a good position where
the overhang is large. A roof's art stands 76 pixels over a box three `z` units
tall, so a pixel of it is now answered four tiles from where it was drawn — a
different lie from the tile's centre, not obviously a smaller one, and the
shadow ray starts at whichever it is. That is the backlog entry below.

### Z2 — the ratio, before

The count from Z1 taken at the three places `docs/parity.md`'s gate uses, so
that "before the footprints work" is a number and not a memory.

### Z3 — the decision

S2, argued with Z1's picture in hand rather than in the abstract.

### Z4 — the ratio, after

Re-run Z2 once `docs/footprints.md` has landed its fitted boxes. The prediction
this plan makes, and which Z4 either confirms or kills: **the zigzags recede on
their own**, because a box that fits the art clips more of the outline.

## Backlog

- ✅ ~~**A frame at `4x` collects no statics at all.**~~ Not a defect. The nine
  statics that eye tile draws all stand in the corner of its `1:1` image, and
  magnifying shrinks the image around the eye until they are outside it — the
  section above has the counts and the gate. What it leaves behind is a habit
  rather than a bug: **a scene chosen at `1:1` does not carry over to a magnified
  frame**, and a diagnostic that changes the zoom has to change the eye tile too.
- 🚩 **The widths, which is Z1's own unfinished half.** Now unblocked: a `4x`
  frame over `ON_THE_WALLS` assembles, and the claim to measure in it is that a
  run of `art_edge` pixels crossing the outline is `scale` real pixels wide while
  a run of `box_edge` pixels is one. The instrument is the two views Z1 already
  built; what is missing is the scan across the edge and a number.
- ✅ ~~**Whether the zigzag a person points at is even a silhouette.**~~ Answered
  by Z1: it is the art's outline. The finer line beside it is the measured
  region's own seam, not a silhouette. What is *not* ruled out is the third
  candidate the entry named — an interior edge between two art texels of
  different colours, which is not a defect at all and which neither layer marks.
  A person pointing at a magnified frame may still mean that one.
- 🚩 **S1.3, the texel grid drawn.** Colour by the art texel index's parity, so
  the `scale × scale` blocks are visible where the art rules and invisible where
  the fragments do. Not an attribution — a *picture* of the quantum, which is
  what a person actually wants to look at, and it is the one instrument that
  would show the interior-edge case the entry above cannot rule out.
- 🚩 **The minifying rungs are the untested half.** Everything above is about
  magnification, where one texel is several pixels. Below `1:1` several texels
  land on one pixel and the blit's linear sampler is the filter — a different
  regime, with its own artefacts, and no measurement in this repository.
- 🚩 **`docs/pixels.md` owes this plan the art texel's own row.** It is the one
  grid with no type and no document, and it is the grid this whole file is
  about.
- 🚩 **A clamped position is invented, and nothing bounds how far.** The state
  it replaced was *lit from every side*, which was worse, but the new one is not
  free: 11.83% of drawn art is answered at a box's rim rather than where it was
  drawn, and the worst is 133 fragments — four tiles. A roof is the whole class.
  The shadow walk starts from that point and the distance to a flame is measured
  from it, so a large overhang is now a *wrong* answer where it used to be an
  absent one. `docs/footprints.md` shrinks the population; what nobody has
  measured is whether the remaining lie shows on a lit roof.
  <br>
  **The other half of the clamp — the *facing* it names — is closed as of
  2026-08-10, and this entry is what is left of it.** `impostor::presented_face`
  gave a miss one face per volume instead of the first exit's, which ends the
  serration inside an overhang (0.22% → 0.02% of neighbouring pairs) and draws a
  hard line where the overhang meets the art (0.30% → 32.59%; 97.68% for
  panels), because **91.79% of the art bordering an overhang is the box's own
  lid**. Refused. So the clamp's facing is as good as the geometry allows and
  only its *position* is still a lie — which makes this entry downstream of the
  height, not of any rule about faces. The counts come out of
  `examples/discard_census.rs`'s `Comb` pass, which counts disagreeing
  neighbours rather than shares: an overhang shaded `+z` on its left and `+x` on
  its right has the same face counts as one that alternates every pixel, and
  only one of the two is a comb.
- 🚩 **The 1-to-2-fragment population, 135k pixels of Britain's window.** The
  tolerance cuts at one fragment because that is where a neighbouring sample
  exists to tell the difference. The bucket just past it is 8% of what is left
  and nobody has looked at whether it is overhang or the same sampling one step
  coarser — a magnified frame would say, and the instrument is the two views Z1
  built.
