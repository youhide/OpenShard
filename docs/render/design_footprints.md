# The footprint — a static's box is the box the art drew

A living plan. The backlog at the end is where the next session starts.

`docs/occluders.md` names this and puts it outside its own scope, in as many
words: *"the lateral fit … it changes **what one primitive's shape is**, where
this plan changes **how many there are**."* This is that change, and it is the
other half of `docs/lighting_rebuild.md`'s census line — the class counted there
as **"a whole tile, because the art would not say"**.

## What we are fixing

**A static whose art states a shape narrower than its tile is given the whole
tile.** Not as a fallback of last resort for a handful of graphics: over Britain
it is **31.6% of every static in the world** (`examples/geometry_census.rs`,
121×121 tiles around `(1501, 1659)`, 11,184 statics), and 19.2% on the smaller
window around `(1504, 1655)`.

**What that costs, in a picture a person reported.** Two bookcases
(`0x0A97`/`0x0A98`, server decorations at `(1505, 1656, 27)` and
`(1506, 1656, 27)`) draw as flat slabs against a wall. Their occluder is a cube
`1 × 1 × 12`, so `impostor::meets` answers the top 45% of each sprite with the
cube's **lid** — a `+z` normal over the pixels where the artist drew shelves —
and splits the rest down the sprite's middle column into `+y` and `+x` at the
cube's ridge. In `View::Normal` that reads as a large flat plane floating through
the furniture, the same colour as the floor, which is what was reported.

**Done when:** a static of this class stands as the box its own base edge
describes, and the frames of every other class do not move by a byte.

## Why it is a whole tile — the root

**The vocabulary, not the storage.** `occluders.md`'s D1 already made a
primitive's coordinates absolute `f64` end to end: `Solid` is two
`camera::WorldSpot`s, the wire carries `min`/`max`, `Builder::add_raw` stores an
arbitrary AABB in a tile bucket, and the BVH walks arbitrary boxes. **A
sub-tile parallelepiped is representable through the whole pipeline today.**

What is missing is at the two ends:

1. **Nothing measures one.** `facing::Facing` is `One(Face)` or
   `Corner { right, left }` — four tile edges and a pair of them. It has no term
   for an offset from an edge and none for a depth. So `facing_of` answers `None`
   for any box that does not *stand on* an edge, and `edges_of(None)` is
   `Edges::ANY`, the whole tile.
2. **Nothing consumes one.** `facing::Block` — `x`, `y`, `z` each a `(u8, u8)`
   span in eighths of a tile — is exactly the type, with a parser in
   `arttable.rs`, a `MAX_BLOCKS` cap, and `facing::blocks_silhouette` drawing one
   the way the projection draws it. `Shape::blocks` says outright: **"No detector
   writes one."** And `occlusion::boxes_of` has no branch that reads it: an
   authored block reaches the grid through nothing at all.

**The art does say it, and here are the numbers off one picture.** The base edge
of `0x0A97` (bottom drawn row per column, `examples/shape.rs`):

```
cols  0..10   11 .............. 35   36 ........ 43
        .     62 63 64 … 85 86      86 85 … 80 79
              └ 25 cols descending ┘└ 8 ascending ┘
```

Two 45° runs meeting at a near corner, which is what the projection makes of a
world-axis-aligned rectangle: the run descending right is the footprint's `+x`
edge, the run ascending right is its `+y` edge. Compare the same reading of
`0x0063` "stone wall", which *does* read as `One(South)`:

```
cols  0..21  36 37 … 56 57   22..30  57 56 … 50 49   31..43  .
```

The same V. The difference is not the shape, it is **where it sits**: the wall's
long run starts at column 0, so it stands on a tile edge and `Half::read` accepts
it; the bookcase's starts at column 11, so it stands in the middle of the tile
and both halves refuse it — the left holds 11 filled columns against
`MIN_FILLED` 18, and the right's base is a chevron with rise 6 over run 21
against `SQUARE` 3.

## The decisions

Made, with the alternative recorded where there was one.

**D1 — the art states the footprint and the tiledata states the height, and
neither is asked for the other's answer.** The measurement below reads two
horizontal spans off the base edge and nothing vertical. The top stays
`occlusion::calc_height(tile)`, unchanged and untouched. *Rejected:* measuring
the height off the art in the same step — it is a second change to the same box
in the same census, and a run that moved would not say which half moved it.
The vertical is a carried item, below.

**D2 — a new measured type, and `Blocks` is left alone.**
`facing::Footprint { x: (u8, u8), y: (u8, u8) }`, in eighths like `Block`, on
`Shape::footprint`. *Rejected:* having the detector write `Shape::blocks` —
`Blocks` is a full parallelepiped list an *author* writes for a shape no
measurement can reach (an arch's posts and lintel), and a derived value in the
same field would make "who wrote this" unanswerable, which is the one thing
`ArtTable`'s authored-row precedence exists to keep answerable.

**D3 — eighths, not a finer grid.** An eighth of a tile is 5.5 screen pixels and
the art's own antialiased border is worth ±2. *Rejected:* a wider fraction — it
buys precision the picture does not have, and `Block` already fixed the unit for
the authored half of the same question.

**D4 — this replaces the whole-tile fallback and nothing else.** `boxes_of` reads
a footprint only where it would otherwise reach `edges_of(None) == Edges::ANY`:
not for a climbable (the prism wins, and `occluders.md` calls the prism's own
lateral fit a separate item), not for a `BACKGROUND` lid, not for anything
`facing_of` already named a face or a corner. So every wall, floor, roof and
stair in the world draws exactly as it does today, and that is a gate rather
than an intention.

**D5 — the fit is a residual, and a picture that does not fit keeps the tile.**
The gates are height-free, because the height is D1's other half and scoring
against it would refuse a correct footprint for a wrong top: a contiguous base,
two runs at 45° within `STRAIGHT` of their own fitted lines, a near corner
flatter than `PLATEAU` columns, and the picture drawn inside its own tile's
column — the last being the gate `Half::read` already opens with, and the one
that separates this from a picture of a whole building.

*Rejected:* scoring by `blocks_silhouette` IoU the way `best_prism` scores a
prism. It is the right instrument for a shape whose height is also measured, and
it is the instrument the carried vertical item should use.

**D5a — the footprint is clipped to its tile, not refused for leaving it.**
Measured, and it is why the first cut of this read nothing: `0x0A97`'s own base
states a box **1.11 tiles** across, since the artist drew twenty-five columns of
descent where a tile's edge is twenty-two. Real furniture is drawn a little wider
than its cell. Clipping gives back exactly what the whole-tile fallback already
gave on that axis and keeps the *other* one — which is the whole gain, and
refusing instead throws a measurement away over an axis that was never wrong.

**D5b — the near corner is not read from a pixel, and the flat one is skipped.**
Each run states one far coordinate as the *intercept the projection makes
constant along it*, taken as a median over the whole run, so no single
antialiased pixel moves the answer. The columns at the corner belong to neither
run. The first cut capped that at two columns and refused **47.3%** of the class
on it — the largest refusal by four times, every one of them measurable from the
two runs either side. `PLATEAU` is where a corner stops being a corner.

**D6 — the projection is not re-derived.** The measurement inverts
`impostor::ray_from`, which is the one spelling of this projection on the CPU:
`across = (u − v)·22`, `down = (u + v − 1)·22`, a sprite's bottom row on the
tile's own `(1, 1)` vertex per `statics::stand_on`. A unit test round-trips every
measured footprint through `facing::blocks_silhouette` — the existing reference,
written for the authored half — so the arithmetic here is checked against a
drawing rather than against itself. `docs/style.md`'s own rule about a formula
written twice.

**D7 — `DETECTOR` bumps and the table format bumps with it.** A footprint is a
new column, so a table written before this is a table that cannot say a bookcase
is a slab. `arttable`'s own version note covers the trap: a reader that half-read
one would answer `footprint: None` for every graphic and look like a detector
that found nothing.

**D8 — a measured box has to reach its own picture, and a base edge does not
always belong to the thing standing on it.** Added 2026-08-10, after a person
pointed at a long table and said we had put a hole in it. Everything above reads
**one line** — the base edge — and D5's gates all ask whether that line is a
clean one. None of them asks whose it is. A table's is a leg's, a counter's is a
foot narrower than its top, a sloped roof's is a V belonging to no box at all;
each measures cleanly and describes something the picture is *standing on*, so
the box comes out real, narrow and wrong.

The residual is `facing::OFF_BAND`: **how much of the picture is drawn outside
the screen columns its own box can reach.** Height-free by construction, which
is what lets it sit beside gates that must not know a top — a box spans `across`
from `(x₀ − y₁)·22` to `(x₁ − y₀)·22` and nothing outside that at any height, so
a pixel outside that band misses the box whatever `tiledata` says.

**It only ever fires where D5a's clamp did**, which is the sharper statement:
the footprint is derived from the base edge's own extremes, so the base's
columns are inside the band by construction *unless the box was clipped to the
tile*. `0x0B80` is drawn 66 columns wide with a clean V across 45 of them — a
box two tiles across, clamped to one. D5a is right for a bookcase drawn a little
wider than its cell and wrong for a slab drawn across two, and this is the line
between them.

*The cap is measured*, in `PLATEAU`'s own manner (`examples/discard_census.rs`
prints the sweep). The class is bimodal — a wooden post `0.0%`, an elven
bookshelf `0.0%`, this plan's own bookcases `7.9%`; a table `15.4%`, a counter
`29.3%`, a slate roof `44.1%` — and every cap from **8% to 12%** keeps the same
164 placements and leaves the same 1.1% of their art outside. Below 8% the
fixture this plan was written for goes; at 30% the counters come back and the
cost jumps to 9.4%. Ten per cent is the middle of that plateau.

*Rejected:* gating on the prism's fit score, which is the same instrument D5
rejected and would not have worked anyway — over Britain a stone wall scores
`0.936` against its best prism and a display case `0.902`.

## Steps

**S1 — the measurement.** ✅ 2026-08-10. `facing::Footprint`,
`facing::footprint_of` and `facing::measure_footprint`, which names its refusal;
`artscan`'s `examples/footprints.rs` is the census, in two passes.

*What it reads*, on Britain's `121×121` around `(1501, 1659)`, 11,184 statics:

| | |
|---:|---|
| **3534** | the class: whole tile, the art would not say — 31.6% of every static |
| **2825** | of it, `ROOF` — a **sloped slab**, which is not a box and is not meant to be |
| **709** | the rest, which a box could be |
| **213** | a footprint measured — **30.0%** of that rest, and every one narrower than its tile |

**The `ROOF` split is the finding, not a filter.** The first run of this census
read 6.2% and reported 76.6% of its refusals as "crooked"; naming the pictures
turned that into "roof, roof, shingles, roof" — eight of the twelve most-refused
graphics, 2,825 placements. A sloped plane's base edge is not two 45° runs and
never will be. `docs/lighting_rebuild.md`'s phase 6i is the open question about
those and it is untouched here.

*What it still refuses*, named because a share hides a tail: `Crooked` 356 —
counters, display cases, benches, rocks, a stone arch. Furniture whose base is
not a clean V, and the next place to look.

**And two gates were wrong before they were measured**, both recorded above as
D5a and D5b rather than quietly fixed: the tile-span refusal (it refused the
bookcase this plan was written for) and the two-column plateau cap (47.3% of the
class).

**S2 — the table.** ✅ 2026-08-10. `Shape::footprint`, on `facing: None` only
(D4, one level up from `boxes_of`'s own gate); `arttable`'s `footprint x0 x1 y0
y1` column, restricted to a `none` verdict in the grammar itself; `FORMAT` 4→5
and `DETECTOR` 2→3, both with the same trap the prism and the hole each closed
for themselves recorded in their own doc comments; and `derive`'s "was
anything read" test grown to three terms so a measured footprint with no face
and no prism is not mistaken for nothing having been read and dropped as a
refusal. The shipped `data/overrides.table` carries the bump too — an
`include_str!` parsed with `.expect`, so a forgotten sheet would have failed
at test time rather than at review.

**S3 — the consumption.** ✅ 2026-08-10. `boxes_of`'s `Edges::ANY` branch reads
`shape.footprint` where it is `Some` — the one branch `edges_of(None)` reaches,
so D4's gate falls out of the existing structure rather than needing a
separate check: a face or a corner already routes through `named`, a lid
through `Edges::NONE`, a climbable returns before either. `StaticAtlas` grew a
`footprints` map beside `holes` and `prisms` — the same lookup, the same
per-graphic seam — and `occlusion::shape_of` reads it instead of the S2
placeholder `None`.

*Gate:* `examples/geometry_census.rs` grew the row — "a measured footprint,
narrower than the whole tile" — and on Britain's `121×121` around
`(1501, 1659)` the "whole tile, the art would not say" line drops from 3534
(31.6%) to 3315 (29.6%), a fall of **219**. Six more than S1's own placement
count of 213: that census skips `ROOF` tiles before ever calling
`measure_footprint`, a reporting choice for its own narrative ("a roof is not
a box"), while `boxes_of` and `Shape::of` were never gated on the `ROOF` flag
— a handful of roof pieces whose base happens to read as a clean two-run V get
measured too, and now narrowed, exactly like any other picture. Not a defect:
`examples/footprints.rs at 1501 1659 60`'s own split confirms it — 2825 roof
placements minus 6 measured leaves 2819, plus 496 of the boxy 709 still
refused, is 3315.

**S4 — what it does to the picture, measured before it is believed.** ✅
2026-08-10, and **both numbers went the wrong way** — one by the amount the plan
predicted by name, the other where the plan predicted nothing at all.

*The instrument:* `client/render/examples/discard_census.rs`. It walks every
opaque pixel of every static in a window, builds that pixel's own view ray with
`impostor::ray_from` and meets it against the boxes `boxes_of` gives — the
shader's own two functions, on the CPU, in `measure_footprint`'s own convention
— and does it **twice per placement**: once with the shape as it stands and once
with `Shape::footprint` forced to `None`, which is exactly the box that shipped
before S3. So the comparison is two boxes against one picture in one run, rather
than two builds measured a session apart.

*Its own controls, printed before anything it measured:* `blocks_silhouette`'s
drawing of a whole-tile block, walked against that block's own box, misses **44
pixels at either of two heights** — a constant, one row of the tile's own width,
which is that reference painting from `head.round()` and not a disagreement
about the projection. The same box moved a hundred tiles misses **everything**.
A floor that did not grow with the box is what makes the shares below readable.

**The discard.** Britain's `121×121` around `(1501, 1659)`, 11,184 statics,
13.7M drawn pixels:

| | before S3 | today | |
|---|---|---|---|
| every static | 13.45% | 13.55% | +14,580 px |
| with the roof cut | 7.69% | 7.82% | +14,214 px |
| **the 219 placements a footprint narrowed** | **7.72%** | **15.88%** | 13,796 → 28,376 px |

**The class's own discard doubled**, and the pictures that pay it are the ones
this step named in advance — its words before the run were "a footprint that
eats a tabletop's overhang is a finding, not a cost to accept quietly".
`0x0B3D`/`0x0B3E` "counter" lose **248 pixels a placement, 21.6% of their art**,
`0x0865` "wooden fence" 30.2%, `0x0B80` "table" 14.8%. A counter's top is drawn
wider than its base and the base is what was measured, so the tabletop now hangs
over its own volume and the shader takes it off the screen.

**D4's gate is confirmed rather than asserted**, and by arithmetic rather than
by reading: the class's own delta (+14,580 px) *is* the whole window's delta, and
every other row of the census's per-claim table — prism, lid, panels,
whole-tile, unfitted climbable — reads the same share before and after, to the
pixel. Nothing outside the branch `edges_of(None)` reaches moved at all.

**The 2.38% on record is not this number and was never going to be.** It counts
pixels that *change in a drawn frame* — visible ones, camera-bound, and a
discarded pixel with another static behind it changes nothing; this counts every
sprite pixel in a window whether it is drawn over, off screen or under a roof.
The census is the upper bound of the same phenomenon and the right instrument for
*the delta*, which is what S4 had to decide on. Re-measuring the frame number
needs a plane diff that does not exist yet — backlog, below.

**The shadow, and the expectation was wrong.** "Every occluder in this class is
`CLEAR`, so the expected move is zero" is refuted by counting instead of
believing: **42 of the 219 placements are pieces the grid holds a primitive
for**, so each of them casts a shadow this plan has already narrowed. They are
`0x0009`/`0x00A9`/`0x012A` "wooden post" (16), `0x00CC` "stone post" (6),
`0x0036` "brick wall" (7), `0x0066` "ornate elven bookshelf" (7), and
`0x059A` "slate roof" + `0x05C7` "wooden shingles" (6).

Two of those groups read very differently and the split is the finding:

- **a post is a post.** `0x012A` measures `x (6,8) y (6,8)` — a quarter of a
  tile in the corner, which is what a post *is*, and its shadow was a whole tile
  before. That is a correction the plan did not claim, not a regression.
- **a roof piece is not a footprint at all.** `0x059A` measures `x (0,3) y
  (0,3)`, so four placements of a slate roof now occlude nine sixty-fourths of
  the tile they filled. S3's own note already recorded that a handful of roof
  pieces reach `boxes_of`'s footprint branch — `ROOF` is not asked there, only
  `BACKGROUND` is — and this is what that costs once the box is *believed*. See
  the backlog.

**And a number that is not this plan's but was found by its instrument.** The
whole-tile class discards **32.69%** of its own art today, and the roofs inside
it 44–53% (`0x05A2` "slate roof", 48×76 pixels of picture over a box three `z`
units tall). That is `docs/lighting_rebuild.md`'s D1 — the height nobody
measures — showing up in pixels, and it dwarfs everything this plan moves.

**S6 — the residual, and the class stops taking pictures that are not boxes.**
✅ 2026-08-10, out of S4's own finding and a person's own picture. D8 above is
the decision; what it does to the census is:

| | before | after |
|---|---:|---:|
| a measured footprint | 219 | **164** |
| whole tile, the art would not say | 3315 | **3370** |

Fifty-five refusals, and they are the right fifty-five: `0x0B3D`/`0x0B3E`
"counter" (41 placements), `0x0B80`/`0x0B74` "table", and **the roof pieces** —
`0x059A` "slate roof" and `0x05C7` "wooden shingles", which closes the backlog
item about a sloped slab being handed a footprint through a route that never
asks the `ROOF` bit, and closes it without a rule about roofs. Every picture
refused here is handed back the whole tile, which is what shipped before S3 and
is never *wrong*, only wide.

`DETECTOR` 3→4: a table written before this carries rows that stand a table on a
box its own top hangs outside of.

**And S4's own cost mostly goes with them.** The class's discard, which S4 read
as 7.72% → 15.88%, is **3.43% → 5.84%**, and what the measured footprint adds to
the whole window falls from 14,580 pixels to **2,867** — the tabletops S4 named
are not in the class any more. What is left is the ordinary edge of a picture
that is genuinely a box.

*Witnessed by mutation, and the first version was not.* It asserted `off_band`'s
own number, so unwiring the refusal from `measure_footprint` left it green — a
selector, not a gate. The fixture is now `0x0B80`'s own numbers painted out (66
columns, the V from 9 to 53), because the refusal only fires where the clamp
did and `blocks_silhouette` draws exactly one tile.

**S5 — the frame gate.** ✅ 2026-08-10. The bookcase pair, `0x0A97`/`0x0A98` at
`(1505, 1656)`/`(1506, 1656)` — `docs/parity.md`'s own "shard's own furniture"
coordinates, since that item is what unblocked this one. `tests/lid.rs`, two
tests: the mutation and the picture it proves is possible.

The mutation is at the geometry layer, not the pixel one, and by necessity —
`StaticAtlas`'s `footprints` table has no public way to lose an entry once real
art has measured one; `state_hole`/`state_prism` exist for a scene that never
had art to measure, and there is no `state_footprint`. So
`occlusion::boxes_of` — the one function `frame::assemble` reads a lid's box
from — is asked directly, twice: once with the `Shape` the real art measures
and once with `footprint: None`, `occlusion::shape_of`'s own documented
fallback. Both bookcases' slabs come back narrower than the whole tile
(`< 1.0` of it), and losing the footprint stands both back up to exactly
`1.0` — the injection, and it goes red.

The picture is the positive control the mutation cannot be: a real frame over
the pair, `View::NormalGeometry` read back, and the drawn pixels over each
bookcase's own tile held under half of the tile's own bounding rectangle — a
whole tile's diamond fills almost exactly half of it (`Solid::faces`'s own top
face is that diamond), so a slab narrower on both axes draws measurably fewer.
No second render forces the whole tile here; that claim is what the mutation
above already carries.

## Not in scope, deliberately

- **The height.** D1. A carried item: the art states it, `blocks_silhouette` is
  the instrument that would score it, and it is a second census.
- **`facing::Prism`'s lateral fit.** `occluders.md` names it; a climbable takes
  the prism branch and never reaches this one.
- **Authored `Blocks` reaching the grid.** `boxes_of` ignores them today and will
  still ignore them after S3 — a separate, smaller item, and one this plan's
  branch makes obvious rather than closes.
- **Whether furniture should stop light.** Every graphic this plan touches is
  `CLEAR`. Making it an occluder is a gameplay-visible change with its own
  argument, and S4's "expected move is zero" is a gate that depends on not
  making it here.

## Backlog

- ✅ **A roof piece is given a footprint and nothing stops it** — closed
  2026-08-10 by D8's residual, and without the rule about roofs this item was
  asking for. `0x059A` reads 44.1% of its art outside the box its base states
  and `0x05C7` 47.2%, so both are refused as pictures that are not boxes rather
  than as roofs. The `ROOF`-versus-`BACKGROUND` question `boxes_of` raises is
  untouched and still belongs to `docs/lighting_rebuild.md`'s phase 6i.
  Superseded text follows for the reasoning: **A roof piece is given a footprint
  and nothing stops it.** `0x059A` "slate
  roof" measures `x (0,3) y (0,3)` and `0x05C7` "wooden shingles" `x (0,2) y
  (0,3)`, so six placements at Britain now stand — and **occlude** — as roughly
  an eighth of the tile they used to fill. `boxes_of` asks `is_background` and not
  `is_roof` (its own comment says why, and says the alternative was tried and
  changed no pixel), and `Shape::of` offers a footprint to any picture
  `facing_of` refused. A sloped slab's base edge is two 45° runs like anything
  else's, so the measurement cannot tell itself apart from a box's — the gate
  has to be the client's own `ROOF` bit, at one of the two ends, and which end
  is the decision. `docs/lighting_rebuild.md`'s phase 6i is the same class's
  other open question. **The sharpest of the three findings below**: the other
  two cost pixels, this one moves shadows on pieces the plan says are not boxes.
- ✅ **A tabletop is drawn wider than the base that was measured, and the shader
  took the difference off the screen** — answered 2026-08-10, and by a person
  pointing at Britain's `(1496, 1663)` and saying the table had been chopped. It
  had, and by two separate things this step's own instrument then measured:
  - **the discard**, and the answer is that a miss is no longer one. A fragment
    whose ray meets no box now keeps the state a static with no box at all
    keeps — the tile's centre, the zero normal `blit.wesl` reads as "no facing,
    lit from every side", `SOLID_NOBODY`. The fringe the discard was introduced
    for lands in the same state, which is what makes it one answer rather than a
    third. `docs/lighting_rebuild.md`'s "One silhouette" is where that is
    argued; what this plan contributed is the number that reopened it — the
    2.38% on record counted *pixels that changed in one frame with the roof
    cut*, and per picture the discard was throwing away 11.09% of every panel's
    art and 32.69% of every whole-tile one.
  - **the geometry**, and the answer is that a table is a box. `0x0B06` reads as
    `Corner { East, South }` because a tabletop drawn as a diamond has the base
    edge two walls meeting leave, so it stood as two `PANEL_THICKNESS` slabs —
    while `Shape::of` had already fitted `prism E 4` to the same picture and
    `boxes_of` read a prism only under `CLIMBABLE`. It now reads one under the
    client's `PLATFORM` bit as well. **The score could not have been the gate**:
    a stone wall scores `0.936` against its best prism and this display case
    `0.902`. Twenty-one placements of three graphics over Britain's window, none
    of them occluders.

  What is *not* answered, and it is the narrower version of the same item: the
  measured footprint is still the box the **base** states, so a counter's top
  still overhangs its own volume by 21.6% of its art. That no longer costs a
  pixel — nothing is discarded — but it is still a surface whose normal and
  height are a plane's rather than the box's, and growing the box outward to the
  art is D5's rejected IoU fit by another name. Left standing, with the cost now
  stated in the right units.
- ✅ **A post's shadow narrowed and it is right** — looked at 2026-08-10, and
  the pictures settle it. `0x0009` "wooden post" at Britain's `(1465, 1683, 0)`
  is the subject `examples/tile_probe` picks out for standing alone on
  cobblestones, so a frame of it is one post, one flame and one shadow. With the
  box its own base edge measures — `x (6,8) y (6,8)`, the far quarter — the
  shadow is a thin ray about as wide as the drawn post; with the whole tile it is
  a wedge some four times wider, thrown by a volume **nothing in the frame
  draws**, and it swallows the tile the post itself stands on. The narrowed
  shadow is not merely likelier, it is the only one of the two that corresponds
  to a picture. `OPENSHARD_SCENE_SOLIDS=white` over the same camera is the other
  half of the answer: the measured box projects to almost exactly the post's own
  sprite, so the shadow starts where the post does rather than half a tile off it.

  **And the other thirty-five are the same kind of picture**, checked rather than
  assumed — `examples/shape` on all four graphics S4 named: a "brick wall"
  `0x0036` is drawn **8 columns of 44**, a pillar and not a wall (its own frame,
  rendered the same way, is a brick column with a column's shadow), a "wooden
  post" 11, a "stone post" 20, an "ornate elven bookshelf" 18. Every one of them
  is a narrow picture that used to cast a whole tile's shadow.

  *The instrument this needed, and it did not exist:* the "before" half of a
  before-and-after is not a scene anything could state, because a footprint is a
  property of the **art**. `StaticAtlas::forget_footprints` is the seam —
  the pair to `state_prism`/`state_hole`, pointing the other way — and
  `OPENSHARD_SCENE_NO_FOOTPRINTS=1` puts it on `examples/isolated_scene.rs`. One
  run of one tool now draws a place both ways.

  *The gate:* `tests/post.rs`, and it is the frame gate S5 is, on the shadow
  instead of the lid. Two frames of one synthetic place differing in exactly that
  call, `View::Shadow` read back both times, and the post shadows **2,676 pixels
  with its measured box against 9,576 with the whole tile** — the 3.6× a quarter
  of a tile should throw. Witnessed by mutation: `boxes_of`'s footprint branch
  made to return `Solid::box_of` again turns it red.

  🚩 *Found while wiring it, and paid for by one wrong run:* `View::Shadow` has
  **three** answers and only two of them are dark. Blue is "no flame reaches
  here at all", which is most of a 512-pixel frame around one torch at night and
  is not a shadow; counting it made the two frames above differ by 4% where their
  shadows differ by four times. `blit.wesl`'s own comment says so and the first
  version of the count did not read it.
- ✅ **The no-discard change is written, and it lands with the work beside it.**
  `statics.wesl`'s `if !hit(best) { discard; }` is `if hit(best) { … }` in the
  working tree, proven by a picture — `0x0B06` alone at `(1496, 1663)`,
  `View::Height`, before and after. It could not be committed from this plan's
  own session: that file was carrying two hundred lines of concurrent work at
  the time. It does not need to be — `docs/silhouettes.md` has since picked the
  change up by name and built its own box-edge layer on the state a miss now
  keeps, so it commits there. What is still owed to *this* plan is one re-run of
  `discard_census.rs` afterwards: its panel and whole-tile shares become a
  statement about **normals** rather than about pixels leaving the screen, and
  the wording in that tool still says "discarded".
- ✅ **`examples/discard_census.rs` read `boxes_of`'s box where the frame meets
  the grid's** — fixed 2026-08-10. It builds the window's grid, two of them
  (merging is a function of the boxes, so a "before" measured against today's
  grid would be half of each answer), and substitutes a named primitive's own
  solid exactly as `statics::push_volumes` does. Four shares moved with it:
  panels 14.23% → 11.09%, whole tile 45.75% → 32.69%, a lid 1.50% → 1.43%, a
  prism 0.51% → 0.35%. **S4's own number did not move at all** — the footprint
  class is `CLEAR` to a piece, so it has no merged solid to be measured against,
  and 7.72% → 15.88% stands as recorded.
- 🚩 **The frame's own discard has not been re-measured, and the census cannot
  do it.** The 2.38% on record counts pixels that change in a drawn frame;
  `discard_census.rs` counts every sprite pixel in a window, drawn over or not,
  so the two are not comparable and the census is the upper bound. The frame
  number wants two dumps either side of `OPENSHARD_SCENE_IMPOSTOR` and a count
  of the pixels whose `Place` differs — `dump::plane_bytes` (`docs/parity.md`'s
  P3) is that comparison already written for a different question, and reaching
  it from a tool is what is missing.
- ✅ **S5's parity item is closed** — `docs/parity.md`'s "The shard's own
  furniture", 2026-08-10. The reported picture is two *server* decorations and
  `examples/isolated_scene.rs` read no database, so the frame gate's fixture
  would have been two hand-transcribed `OPENSHARD_SCENE_EXTRA` rows standing in
  for the client's input. It reads `openshard.db` now, on by default, and the
  two bookcases at `(1505, 1656)`/`(1506, 1656)` come back out of it by name.
  S5 is unblocked.
- ✅ **`Crooked` is 356 placements and they have names** — read 2026-08-10,
  columns dumped straight off `base_edge`, no rule changed. `0x0B3F` and
  `0x0B40` "counter", `0x0AA0`/`0x0AFE`/`0x0B01` "display case", `0x0B5F`/
  `0x0B60` "bench", `0x1365`/`0x1366` "rock", `0x00CF`/`0x00D1` "stone arch".
  The sharpest pair turned out to be two different pictures, not one: `0x0B3D`
  and `0x0B3E` are a bare counter, a clean monotonic V both ends (now
  `Overhung`, D8's own refusal, not `Crooked` — S6 had already closed this
  half). `0x0B3F` and `0x0B40` are **pixel-identical to each other** — the same
  art shipped under two graphic ids — and their base is a counter *with wares
  on it*: the per-column bottom edge runs three separate humps (`32→44`,
  `64→67`, `49→51`, at columns 6, 19 and 37), which is three objects sitting at
  three different depths, not one box's edge read wrong. The rest confirm the
  same shape of answer rather than a new one: the bench's columns jump `36→19`
  at one column (a leg, then open air, then the far leg), the stone arch's
  jump `70→102` at another (the near jamb, then the opening behind it), the
  rock is jagged the way a rock is. `Crooked` is doing its job across the
  class — a picture standing on more than one thing, or on nothing between two
  things, and none of the nine graphics checked is a detector defect wearing
  that name.
- 🚩 **The outer ends are half a pixel wide and the rounding pays for it.** The
  two far coordinates are exact (D5b); the two near ones are read from the end
  column's centre, and the sweep paints the column a corner falls *inside*, so
  each is out by up to half a pixel — one eighth after outward rounding, which is
  what `a_footprint_measures_the_block_that_drew_it` asserts as slack rather than
  hides. Reading the column's inner boundary instead would halve it; worth doing
  only if S4 shows the impostor's discard cares.
- 🚩 **`Solid::box_of`'s own doc gives a stale reason for its visibility**, found
  while wiring S3 beside it. It says `pub(crate)` exists because
  `light::walk_the_wire` reconstructs a solid's box from `(tile, edges, bottom,
  top)` — true when `docs/lighting_raymarch.md`'s point 4 wrote it, not true
  today: `occlusion::Occlusion::primitive_bytes` writes `solid.wire_box()`, each
  primitive's own absolute corners, and `light::walk_the_wire` reads
  `stands.wire_box()` the same way — `docs/occluders.md`'s D1 moved the wire to
  absolute coordinates and nobody came back to this comment. Not fixed here:
  it costs nothing correctness-wise (the visibility itself is still earned by
  the same doc's second reason, `crate::impostor::Volume::of`), and touching it
  is a different doc's territory than this plan's.
