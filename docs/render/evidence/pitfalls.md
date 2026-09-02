# Lighting pitfalls: how a frame misleads, and the order to ask it things in

**Not a plan and not a status page.** [`../README.md`](../README.md)
says where the engine stands and [`design_model.md`](../design_model.md)
holds the model. This is the third thing: a catalogue of the ways
a *lit frame* has lied to somebody working on it, each with the instrument that
caught it, so the next person spends the afternoon on the defect rather than on
the three wrong verdicts in front of it.

Every entry here was paid for. Where an entry records a verdict that was wrong,
the wrong verdict stays in it — that is the part with the reuse value.

## The amplifier, which is why any of this is visible

**`blit.wesl` gives a vertical face a full cosine where a lid takes a grazing
one.** A lamp standing *beside* a surface is nearly in the plane of that
surface's lid, so `max(N·L, 0)` is small over the whole of it, while a wall or a
riser turned towards the same lamp takes the lot.

Measured over one staircase at Britain's `(1454, 1728)` — 35,100 static
fragments below `z 6.6`, the flame term alone, out of 765:

| face met | fragments | share | flame term |
|---|---:|---:|---:|
| the lid, `+z` | 25,216 | 71.8% | 11.6 |
| east, `+x` | 4,957 | 14.1% | **165.4** |
| south, `+y` | 4,927 | 14.0% | 12.7 |

**A factor of fourteen between two surfaces of one object.** So an error in
*which face a fragment is given* does not read as slightly wrong shading — it
reads as a bright stripe drawn on top of the picture, and a person reports it as
garbage rather than as lighting. The same error on a lid would be invisible in
the same frame.

Keep this in front of you: it is the reason a facing error and a *drawing* error
look alike, and it is the reason the entries below all begin with the same
question.

## The ladder: what to ask, in order

A bright line on a dark surface is not evidence that anything is lit. Four
questions, cheapest first, and each one can end the search:

1. **What drew this pixel?** `OPENSHARD_FRAME_VIEW=2` (`Kind`). Land, a static, a
   mobile, or nothing — one of the four, before any theory about which. Land is
   `(51, 166, 76)`, a static `(64, 115, 255)`, a mobile `(255, 102, 38)`;
   `blit.wesl`'s `VIEW_KIND` is where the three literals live.
2. **Is it in the art?** The **albedo control**: the same scene with the flames
   turned off and the ambient turned up, which is albedo times a constant.

   ```sh
   OPENSHARD_LIGHT_BRIGHTNESS=0 OPENSHARD_LIGHT_GROUND=14 …
   ```

   If the feature survives, it was drawn by the artist and no amount of reading
   the lighting will explain it. This ends more searches than anything else on
   the list and costs one run.
3. **Is the lighting even changing there?** `OPENSHARD_FRAME_VIEW=12` (`Flames`)
   — what the flames added, with the ambient subtracted and the albedo out of it.
   A flat `Flames` across a boundary where the lit frame jumps twenty-fold means
   the jump is albedo, full stop.
4. **What face was the fragment given?** `OPENSHARD_FRAME_VIEW=4` (`Normal`), and
   `5`/`6` to split measured geometry from the picture's own. Only now is the
   answer about the model.

Then, and only then, the geometry: `9` (`Solid`, which primitive), `3` (`Height`,
what `z`), `8` (`SilhouetteBox`, where a box's own rim falls inside the picture),
`13` (`Shadow`, whether anything was in the way).

## The pitfalls

### 1. A fit is scored on its outline, and the surfaces are inside it

**Symptom.** Bright stripes running up the middle of every stone slab of a
staircase, at full contrast, in a frame where the rest of the same staircase is
ambient-dark. A person reads it as something extra being drawn.

**What it is.** `facing::silhouettes_agree` is the only measure any fitted shape
in this renderer is scored by, and it compares two **filled** silhouettes.
Everything interior to the outline — where a step's riser stands, how deep its
tread is, a moulding, a recess — contributes nothing to the number. Two prisms
with the same outline and different insides score identically, so a fit is
**confident and still wrong about where the surfaces are**, and the amplifier
above turns that into a drawn artefact.

**Measured.** Over 37 east-face bands across the flight, the model's riser and
the artist's own step joint are parallel and about equal in number — median 2
model bands per screen column against 3 drawn joints — but the model's riser
stands **10.5 view px** where the art's joint is **2.5**. Each riser band
therefore covers the upper half of what the picture draws as the tread.

**And the fit is not the ambiguous part.** `examples/prism_axis.rs` ranks the
whole 261-candidate sweep per graphic and prints the margin over the best
candidate that climbs a *different* way:

```sh
OPENSHARD_CLIENT=… cargo run -p openshard-client-artscan \
    --example prism_axis -- 1872 1873 1874 1876 1878 1880
```

```
0x0751  North [1,3,5]  0.9752  margin +0.0775   (the whole top six is North)
0x0752  West  [1,3,5]  0.9752  margin +0.0775
0x0754  East  [1,3,5]  0.9726  margin +0.0945
0x0758  East  [1,3,5]  0.9726  margin +0.0945
0x0750  box   [5]      0.9773  margin +0.0520
0x0756  refused        0.8952  margin +0.0024   ← a coin flip between two axes
```

**A near-tie between axes looks like the signature of a shape one prism cannot
hold** — two arms meeting at a corner, where `Prism::up` is one face by
construction. `0x0756` is the only one of the six that reads that way, and it is
the one the table holds no prism for. Unconfirmed, and the cheapest place to
confirm it is the census this wants anyway.

**The reach is not staircases.** Every fitted prism is scored this way — the
3.2% fitted-prism class `examples/geometry_census.rs` counts, which is the
tables, counters and display cases `occlusion::boxes_of`'s `PLATFORM` branch
admits on exactly the same terms.

**Reproduce.**

```sh
OPENSHARD_CLIENT=… OPENSHARD_SCENE_AT=1454,1728,1 OPENSHARD_SCENE_RADIUS=6 \
OPENSHARD_SCENE_ZOOM=2 OPENSHARD_SCENE_VIEWPORT=800x600 OPENSHARD_FRAME_VIEW=0 \
OPENSHARD_FRAME_DUMP=/tmp/lit.png \
    cargo run -p openshard-client-render --example isolated_scene
```

Then `_VIEW=4` beside it, and mask the `Normal` layer's east faces onto the
albedo control: the magenta lands on the top half of each drawn slab. The sprites
themselves, with the tile's own diamond stroked over them, are

```sh
OPENSHARD_CLIENT=… OPENSHARD_ART=0x0751,0x0752 \
    cargo test -p openshard-client-render --test artshot -- --ignored --nocapture
```

and what stands on the real tiles is `artscan`'s `column` example.

### 2. One mask, two questions

**Symptom.** Every fitted staircase, table and counter in the world comes out
flat and formless, lit evenly from all sides, with no shading of its own.

**What it is.** `impostor::Volume::edges` was filled from
`occlusion::boxes_of`'s mask. That mask answers *which occlusion test does this
box take* — and on a climbable it is `Edges::ANY` **by deliberate override**, to
pick the exact slab test a solid takes over a lid's crossing test or a panel's
run masking. `statics.wesl` reads `Edges::ANY` as *the art named no face* and
writes no facing at all. One value, two domains, and the conservative fallback
points opposite ways in them: for occlusion, "unknown" means block everything;
for a surface, it means claim nothing.

**Fixed** — `occlusion::named_edges`, the expression `boxes_of` already started
from, given a name and a second reader. The gate is a **pair on purpose**: the
same tile, the same flags, the same prism, only the measured `facing` differing,
so what it holds is "the art's answer is what this field carries" and not "a
climbable is special".

**The general shape**, which is the reusable part: when a field is read by a
second consumer, ask what its *fallback* means to each of them. A value that is
safe in one direction is a lie in the other, and nothing in the type says so.

### 3. A set chosen by what changed is not a set chosen by what is there

**Symptom.** A face census over "the staircase" came out 88.6% lid, 11.3% south
and **9 pixels** east — from which the east faces looked absent, and the defect
looked like a tie inside `impostor::meets` sending fragments to the lid.

**What it is.** The set was *the fragments that differed between two builds*,
which is a set defined by the shader rule under test rather than by the
staircase. Taken over the staircase instead — statics below `z 6.6` in the same
window — east and south come out 4,957 against 4,927, which is the symmetry the
projection has. The east faces were never missing; they were the half that did
not move.

**The general shape.** A diff is a fine way to *find* a population and a bad way
to *measure* one. Define the set by the thing, then measure; if the definition
mentions a build, a flag or a commit, it is a definition of a change.

### 4. Two framings that die on contact, and are worth knowing about

Both of these were reached honestly, from real observations, on this same
staircase, and neither survived its own measurement.

- **"The boxes disagree about which way the flight climbs."** True — six
  graphics, four axes — and not a defect. `prism_axis` says every one of the six
  is confidently its own direction, and the structure really is a stoop with
  steps down more than one side.
- **"Interior faces at the joins between abutting tiles."** This is the *"garbage
  on the vertical joins"* `statics::push_volumes`' own doc records, and it does
  not reproduce here: `isolated_scene` prints `0x0751`'s treads at
  `x 99.000..102.000` — three tiles folded into one primitive — so
  `occlusion::merge` is doing its job. Check the printed spans before reaching
  for this one.

### 5. What ruled itself out, and how, on the same frame

Worth keeping as a checklist, because each of these was a live hypothesis and
each took one run to kill:

| hypothesis | the run that killed it |
|---|---|
| the ground tile is poking through | `_VIEW=2`: *static* on every strip pixel |
| it is in the art | `OPENSHARD_LIGHT_BRIGHTNESS=0`: a plain grey staircase, no strips |
| the picture stands taller than its box | all six graphics are 44×65, and 43 + 4·5 is 63 — the fitted five `z` are the art's own |
| something is in the way | the shadow term averages 248.8 of 255 on the same fragments |
| the merged run's box is wrong | no solid under a strip runs wider than 0.8 of a tile |
