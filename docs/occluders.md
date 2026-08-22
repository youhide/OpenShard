# The occluders — one shape per surface, and no tile in the answer

A multi-session refactor with its decisions already made. **Nothing below is an
open question**; where a choice had alternatives, the choice is written down with
the reason, and the alternatives are recorded so they are not re-opened rather
than re-argued. A session that starts here starts at the first step whose gate is
not yet green.

`docs/lighting_rebuild.md` phase 6e is the one-paragraph version and points here.
This continues `docs/lighting_geometry.md`'s question — box occluders becoming
real geometry — with the part that document never had: a reason, a measurement
and an order.

✅ **All six steps are green as of 2026-08-09, and this document is a record from
here on.** A session looking for work does not start here: what is still live out
of this track is the § *Backlog* below, and the items that outlive it are carried
in [`lighting_rebuild.md`](lighting_rebuild.md)'s own backlog, which is the one
list. **S6 is one of them come back closed** — the aperture, which was the first
of those three and is written up in § *The aperture*; a step landed after the
table was full, because a record of a finished track is still where a finding
about it belongs.

## What we are fixing

**The ragged boundary between solids on neighbouring tiles.** Holes, fringe, and
stair-stepping at a tile edge, in everything that reads the occlusion geometry:
the shadow walk, the impostor's positions and normals, and every debug view that
draws either.

**Done when: on neighbouring tiles there are no holes, no fringe and no
stair-stepping between solids.** That sentence is the acceptance criterion, and
§ *The detector* turns it into a number a run can print and a gate can fail on.

**Which step delivers it: S3**, and it is the only one that moves this number.
S1 lifted the ceiling that made the shape unstateable, S2 is the ruler, S4/S5 and
S3b are deletions and optimisations that must move nothing. **S3 has since landed
and moved nothing either** — its exemption needs a ray in the surface's own plane
and the renderer has none, which is measured at S3's own acceptance. The seam a
person reports is a *shading* defect and it belongs to
[`lighting_rebuild.md`](lighting_rebuild.md)'s **phase 5b**, which has since
landed — and it is what licensed S4's deletion of `same_run`, since the rays that
rule existed for are the below-horizon ones 5b stopped tracing. So acceptance for the
sentence above is § *Acceptance for S3* — six things to run, each with a figure
to read, none of them resting on anybody's description of a picture.

## Why it is ragged — the root, measured

**A primitive is a tile's.** Not by any argument about geometry; by the shape of
the storage. Three consequences, each already seen in a frame:

1. **A primitive's coordinates on the wire are `tile + byte/255`.**
   `occlusion::Solid::box_from_footprint` rebuilds a box from a cell and four
   bytes of sub-tile fraction, so a primitive **cannot express a shape wider than
   one tile**, and its corners are quantised to a two-hundred-and-fifty-fifth of
   one. The *record* — `occlusion::Solid`, two `camera::WorldSpot`s of `f64` — is
   already absolute and exact. It is the upload that folds it back onto a cell,
   and `light::walk_cells_streaming` mirrors that quantisation on purpose, which
   is why the two CPU walks read different heights for one solid by design.
2. **One physical surface is N primitives with N−1 internal seams.** A run of
   wall is one wall to the artist and N statics on N tiles to us. A storey's
   floor is one slab and one box a tile. Every internal seam is a place where two
   boxes meet, where a fragment can stand exactly on the join, and where the
   silhouette steps at tile granularity.
3. **Rules are stated in cells to paper over 1 and 2.** `same_run` exists because
   a run is N solids. `starting_cell` exists because a fragment's position and
   its instance's tile can disagree. The vertical shortcut reads one cell. The
   per-cell `max` exists because two panels of one corner are two boxes of one
   wall. Four rules, all standing in for a shape that is not stated. **Three are
   gone at S4** — `same_run`, the vertical shortcut and `starting_cell` — and not
   one of them went by the merge or the hierarchy this plan expected to retire
   it.

   **And for a *body* not even that** — a correction to this list, measured after
   it was written. `same_run` is a run of **panels** along a row or column; the
   walk's `edges == EDGE_MASK` branch never asks it. A climbable's treads are
   declared as bodies (`occlusion.rs`'s own test: *"a tread is a body: a stair is
   solid"*), so a flight of steps has **no surface exemption of any kind** —
   only `mine == reference.x`, one primitive. So point 2 is not merely papered
   over here, it is bare, and § *The flight seams* is what it looks like on a
   frame. It also weakens D5's own order: `same_run` cannot be load-bearing for a
   shape that never reaches it.

Measured, on one real place at 4:1, before any of this: 474 fragments stood
strictly outside their own carried tile and **324 of them leaked a fully lit
pixel into a shadow**; the narrow leaks over one building's floors numbered 303.
`starting_cell` took that to zero, and `docs/lighting_rebuild.md`'s backlog
records that it is a repair rather than a construction — it arbitrates between
two spellings of one fact instead of removing the second spelling. ✅ **Removed
at S4, and the second spelling went with it**: the walk seeds itself from the
position, the carried tile is not passed to a walk on either backend, and what
the arbitration was worth is measured under § *The starting cell*.

## The decisions

Made, not to be re-opened. Each carries the reason and, where there was one, the
alternative it beat.

**D1 — geometry is absolute world coordinates, everywhere.** The wire carries a
primitive's own `min`/`max`, not a cell and a fraction of it.
`box_from_footprint`, `footprint_bytes` and `wire_span` go. No tile is the base
of any coordinate. *Rejected:* widening the fraction to sixteen bits — it keeps
the ceiling that a primitive is a tile's, which is the whole defect.

**D2 — a fragment is exempt from its own *surface*, not from its own primitive.**
*Rejected:* leaving the seam and widening the rules that hide it, which is
`WIDTH_OVERLAP`'s own family and what `docs/style.md`'s *No fudge constants* was
written from. Also rejected: an `ε` along the normal — the classic shadow bias,
which this renderer already owned as `STAND_OFF`/`ON_TOP` and already deleted.

**The defect this names.** A shadow ray starts *on* the surface, so it meets that
surface at `t = 0`. The textbook has two cures — offset the origin, or exclude
the primitive the ray came from — and this renderer took the second, correctly.
But the exclusion is spelled `mine == reference.x`: **one** primitive, where one
physical surface is N of them. The ray leaves its own box and enters the
neighbour's a thousandth of a tile later. It is the mesh tracer's own
self-intersection bug, one level up: excluding the source *triangle* does not
save a ray that grazes into the next triangle of the same polygon.

**The rule, and it is a theorem rather than a heuristic.** A primitive is
axis-aligned, so each of its faces lies at its own extremum — the whole box is
therefore in the closed half-space **behind** the plane of that face. So for a
fragment on that plane with outward normal `N`, and any ray with `d·N > 0`: the
ray leaves the half-space at `t = 0+` and never returns, and **no primitive whose
face lies in that plane facing the same way can ever occlude it.**

> Skip a candidate exactly when its extent along the fragment's own normal axis
> **ends at the fragment's own plane, on the fragment's back side** —
> `candidate.hi[axis] == plane` for an outward `+`, `candidate.lo[axis] == plane`
> for a `−`.

That set is provably empty of true occlusions, which is the whole difference from
a bias: `ε` trades acne against peter-panning, this discards nothing. The other
two cases close themselves — `d·N < 0` is a light behind the surface and `N·L` is
already zero there, and `d·N = 0` is measure zero and is precisely the graze the
exemption exists for.

⚠ **That middle clause was false when it was written and phase 5b is what makes
it true.** "`N·L` is already zero there" is a statement about the *shaded* frame;
the shadow term is compared without a cosine on either side, so a ray behind the
plane was traced and its crossing was real — which is why S3 had to take the ray's
direction as a parameter (`d·N >= 0`) rather than wave the case away, and why
`same_run` is broader than the theorem. Once every sample carries its own cosine
the clause is true by construction: a sample behind the plane is not traced at
all. **Landed, and it took the exemption's own reachability with it**: after
phase 5b, S3's gate reports `0` of 720 fragments blamed *with the rule
neutralised*, where the same neutralisation under the old centre cosine reports
480. The theorem is still the right statement of why a surface may not shadow
itself; what no fixture in the tree can now do is reach it.

**What it subsumes**, so the step is a deletion rather than an addition:
`mine == reference.x` (a fragment's own box ends at its own face — the special
case), `same_run` **with** its row/column cell test and its `on_surface` height
gate (a run of wall is coplanar same-facing panels), and — to be measured —
`ray_vs_solid`'s zero-length graze rule, which exists today for exactly this
reason and says so.

**Two halves, and the order between them is the decision.**

- **D2a — the identity.** The rule above. It moves no geometry and needs no
  merge. ~~**This is what cures the seam.**~~ **It is not — measured at S3, and
  neither is D2b.** The exemption is reachable only by a ray lying in the
  surface's own plane, and the shipped renderer has no such ray: S3 moves **0
  pixels** on the flights, 0 of 29,696 on the wall run, 0 of 262,144 on the stair
  under a front light. What cures the seam is `docs/lighting_rebuild.md`'s phase
  5b — see this plan's backlog, where all three arguments are written out. D2a is
  the rule that says *why* a surface may not shadow itself, and that is worth
  having stated whether or not a frame today can reach it.
- **D2b — the merge.** Contiguous same-surface neighbours become one box at build
  time. A **pure optimisation** once D2a holds: fewer primitives, no pixel moved.
  Last, not first.

*Reversed from this plan's first draft*, where the merge was the premise and the
identity fell out of it. The reason is measured: the merge is what forces a
primitive wider than a tile, which is what forces the hierarchy (D3) and breaks
the grid's superset property — so making it the premise buys a seam fix at the
price of three other steps. And a *derived* identity cannot work for a lid at
all: `edges == 0` gives `own == 0`, so `same_run` is unconditionally zero and a
floor gets no exemption in principle.

⚠ **Honest status of the theorem.** It was derived in the session that measured
the run of flights, not when this plan was written, and its one soft spot is
float equality: the fragment's plane arrives interpolated from the rasteriser and
the candidate's box from the storage buffer. If those are not bit-identical the
temptation is a tolerance, and the answer is **not** a tolerance but removing the
second number — carry the plane from the instance row, which already carries
`solid`. That has to be measured, and S3's gate is where.

**D3 — the broad phase is a bounding volume hierarchy.** A tree of axis-aligned
boxes over the primitives; a ray that misses a node skips its whole
subtree. The uniform tile grid goes. *The reason is D2b and not speed:* a uniform
grid must list a primitive in **every cell it spans** or it stops being a
superset, so the more a surface merges — which is the point — the worse a grid
fits it. A grid likes many small primitives of one size; a hierarchy likes few
large ones of different sizes, which is what a merged world is.

**D4 — the broad phase may not change the answer.** It returns a *superset* of
the primitives a segment might meet; the answer is `ray_vs_solid` over that
superset and nothing else. Every tuning knob in the hierarchy — leaf size, split
rule, node budget — is therefore a cost knob that **cannot** move a pixel, and
that is a property to be gated rather than asserted: see § *The oracle*.

**D5 — the cell disappears from every rule.** `same_run`, `starting_cell`, the
vertical shortcut and the per-cell `max` are deleted. The first two because D2
removes what they stand in for; the last two because they are statements about a
cell in a pass that no longer has one. Each goes only after its own measurement
says nothing depends on it — see § *Steps*, S4.

⚠ **The reason given for `starting_cell` was wrong, and it went anyway.** D2
removes nothing it stood in for: it is not an exemption at all but an arbiter
between a fragment's position and the tile it carries, and D2 has no opinion
about either. What licensed the deletion is a census — the case it was written
for is unreachable in every scene the crate draws, and the case it *does* decide
has one answer. A rule can be right to delete for a reason its plan never
guessed, which is now three for three at S4.

**And `same_run` is licensed by D2a rather than by the merge**, which is what the
reversal above buys: the identity replaces it outright, so S4 no longer waits on
a build-time transformation of the geometry. It is also less load-bearing than
this plan first assumed — the walk's body branch never consults it at all, so for
a climbable it was never holding anything up. See § *Why it is ragged*, point 3.

**And so does every *scan* of a cell**, which is the same defect wearing a
different coat: `blit.wesl`'s `own_solid` walks a cell's list to name the solid a
sprite fragment is a point of, and `occlusion::owner_at` is a linear scan of one
too — `docs/lighting_rebuild.md`'s backlog has both, and counts **thirteen scans
of one cell for a four-tread flight**. Under D6 the answer is carried: the
primitive a fragment met is the primitive it is a point of. They are in scope
here and land in S4 with the rest.

> **`own_solid` went, 2026-08-10, and not for the cost.** The prediction above
> was right about the answer — the primitive a fragment met is the primitive it
> is a point of, and it now rides in the position plane's fourth channel
> (`solid_format.wesl`) — but what forced it was correctness rather than a scan
> count. The scan was *ambiguous* for a fitted climbable, whose treads are one
> owner and name no side, and phase 6d took away the mesh pass that had been
> covering that everywhere it mattered. See `lighting_rebuild.md`'s own account.

**D6 — the impostor meets the merged primitive its instance is part of.** Phase
6c made a fragment's shape a property of its own instance, and `occlusion::Part`
is the join from an instance to the solids it pushed. After merging that join
points at the merged solid, so two neighbouring wall sprites are met against
**one continuous box**. The sprite is still drawn per instance and the silhouette
is still the art's; only the volume behind it becomes continuous. **This is the
reason the acceptance criterion is reachable at all**: position and normal stop
being able to jump at a tile edge, because there is no edge in the volume.

> **Not done when S3b landed, and it cost a visible defect.** `statics::
> push_volumes` went on handing the impostor `boxes_of`'s per-*tile* shapes while
> the grid had folded the run into one primitive, so two abutting statics stood
> as two boxes with a face buried between them — a face the merged solid does not
> have. A fragment met against it looked east where its neighbours looked south,
> and was excused from shadow by the very solid it was buried in (one merged
> primitive, one id), so it came out fully lit: **a bright one-pixel vertical
> stroke at every seam, once a tile**, reported from the client as garbage on the
> vertical joins. Closed 2026-08-10 — `push_volumes` takes the grid's own box
> wherever `id_of` names one and keeps `boxes_of`'s where it does not, which is
> what preserves 6c's answer for the half of a street the grid refuses. See
> `lighting_rebuild.md`'s phase 6h.

**D7 — the map's tile keeps its own job, which is placement.** A static arrives
at `(x, y, z)` on a tile; that is the wire and this plan does not touch it. The
tile-to-world mapping — the arithmetic that decides which world coordinates a
tile's corners are, and how to state it so that no reader needs a `floor` that
can land on the wrong side — is a **separate task** and explicitly out of scope
here. What this plan does is remove every reader that needed such a `floor` in
the first place.

**D8 — the wire is storage buffers, not textures.** The grid's texture encoding
(`occluders`, `footprints`, `solid_z`, the reference lists) predates the
allowance; `blit.wesl` already reads eleven storage buffers and phase 6a settled
that the crate's ceiling is WebGPU. A primitive is a struct and a node is a
struct, and neither should be spelled as channels of a texel.

**D9 — `z` stays in `z` units on the wire.** Phase 2 decided this deliberately:
the occlusion set, every span and every walk are stated in them, and a wire that
alone counted in tiles would be a second metric. Not re-opened.

**D10 — `f32` on the wire, `f64` in the record.** The record is authored and
merged on the CPU where exactness is free; the wire is what a shader can read.
The gap between them is a thing to *measure* (the oracle below runs on the wire's
own numbers), not to hide.

**D11 — tests are the deliverable.** Every step lands with a gate that has been
**fault-injected to red** before it is trusted. A step whose suite is green with
its own change reverted has not landed; it has been written. This is the
discipline phases 4, 5 and 6 already used and the reason each of them has a
number in it.

## How this sits in `lighting_rebuild.md`

Checked against it line by line rather than assumed, because that document is the
entry point and a plan that quietly contradicts it is worse than no plan.

**What it fulfils.** Two of its own promissory notes are this plan: `own_run`'s
row says the rule is retired "when a run becomes one solid", and
`lighting_geometry.md`'s row says the generic form of box-into-real-geometry
"continues" at `facing::Blocks` — an authored list of up to four boxes, written
and wired to nothing. **D1 is what makes `Blocks` wireable**: a shape of up to
four boxes cannot be uploaded at all while a primitive's coordinates are a
tile's, so the carried-over item "`Builder::add` consuming an authored `Blocks`
list" becomes available here rather than needing its own fight. It is not in
this plan's steps — it is content, and it stops being blocked.

**What it supersedes, and those documents now say so.** `MAX_WALK_STEPS`, which
"survives" in the *What goes* table, bounds cells stepped and is replaced by a
node budget in the same role. `lighting_raymarch.md`'s row read "survives as the
walk"; the walk it means is the DDA, and S5 retires it — what carries over is
`ray_vs_solid`, which was never about cells. And the corner-tie CPU/GPU parity
gap was listed under *Known gaps that outlive the rebuild*: it does not, because
a corner tie is two backends disagreeing about which **cell** a ray crosses
first, and there will be no cells.

**What governs this plan and is not restated in it.** *How this is judged* still
holds: **the instrument is a picture beside the path tracer's, looked at by a
person.** The census and the brute-force oracle below are detectors, and a
detector is what catches a defect between two lookings — neither replaces the
frame, and no step here is finished on a number alone.

**What it must not disturb.** Phase 4's self-shadow rule is identity between
primitives, and merging changes which primitive a fragment is a point of. The
three tests that go red when the identity compare is neutralised must stay red
under that injection through every step — a merge that quietly made a fragment
exempt from a genuine occluder would be trading this plan's defect for a worse
one. S4 states it as a gate.

**Where it sits in the order.** Phase 6d — the mesh pass coming off real statics
and gaining a colour target, which is phase 2's albedo — is still open, and the
two do not collide: 6d is about *drawing*, this is about the occlusion geometry
behind it. One place they touch, settled here so no session has to work it out
again: **a flight's treads do not merge.** S3b's rule requires an equal span, and
three treads are three heights by construction. A flight stays three primitives,
which is what its shape is.

## The detector

The acceptance criterion has to be a number a run prints and a gate can fail on,
or it is a hope. Two instruments, and they answer different halves.

**The seam census — the DoD itself.** Over a rendered frame: for every pair of
horizontally or vertically adjacent fragments that lie on **opposite sides of a
tile boundary** and belong to the **same primitive**, their shadow answer must
agree to within one shadow ray (`1 / SHADOW_RAYS`), and their normals must be
equal. Count the pairs that do not. **The DoD is zero**, and the count is printed
every run of `examples/isolated_scene.rs` beside the box census phase 6c added.

Two things it must report and not just use: how many pairs it *examined* — a
census that examined nothing passes — and the breakdown by what disagreed
(visibility, normal, or the fragments naming different primitives where the
geometry says one).

**Its own before-number is taken at S2, before anything is merged.** A detector
built after the fix has never been seen to fire; this one is built while the
defect is still there, and its first reading is the thing S3 is measured against.

### Reading the dump, in numbers rather than by eye

`tests/traced.rs` and `examples/boxes.rs` both write
[`Verdict::strips`](../crates/client/render/examples/oracle/pathtrace.rs) when
their dump variable names a directory: the frame's own shadow decision, the
tracer's, their difference, **why an uncompared pixel was not compared**, and
**which solid the frame drew**, one colour a body. `tools/mask_probe.py` reads
that composite back — an overlay of the shadow onto the body map, the
neighbourhood of one pixel as text, and a seam census across the joins.

**It exists because every wrong reading on this track came from looking instead
of measuring.** A dump older than the fix, read as a live lighting fault. A mask
laid over a picture from the *other* tool and so placed one tile east — the tool
centres on the scene's own tile bounds, the gate on a named tile, and for a run
of three flights those differ by one. A composite sliced by `width // 3` and read
as a three-pixel camera offset, when the slice was off by the ruler. Each of
those is a question with a numeric answer.

### The flight seams: **a continuous wall shadows itself, once per primitive**

The run of flights shows a hard shadow step landing exactly on the join between
two primitives of one continuous riser wall, which reads as "each new primitive
starts its shadow at its own corner". **It is that, and the mechanism is D2's own
argument, now with a fixture and numbers under it.** What it is *not* is anything
about draw order or precision, and ruling those out is what the measurement did.

The probe, in world coordinates, at the join across `x = 101`:

```
(299, 225) frame: box 0's FaceSouth at (100.992, 100.333, 4.333) shadowed
(300, 225) frame: box 3's FaceSouth at (101.008, 100.333, 4.417) lit
```

One plane, `y = 100.333`. One flat wall, `x` from 100 to 103, `z` 0 to 5, in
three primitives. The fragment at `x = 100.992` sends its segment toward a flame
that sits a sixth of a tile **behind** that plane, so the segment goes into the
wall: out through its own solid, which is exempt, and straight into box 3, which
is not — box 3's west face is nine thousandths of a tile away. The fragment nine
thousandths further east has box 3 as its *own* solid, and its next neighbour is
a whole tile off, by which point the segment has climbed past `z 5` and clears.
One tooth per primitive, and the tooth is exactly a tile wide.

Three things this pins down, each of which had to be measured rather than argued:

- **Not draw order.** Shadow is a deferred pass over the G-buffer; the order faces
  were drawn in decides which surface owns a pixel and nothing else. Reversing the
  flights changes nothing at all — they are three different tiles, so they are
  three different depths and there is no tie to break. Measured: identical
  verdict, `261682 / 0 / 11 / 462`, to the pixel. Reversing the *treads* within a
  flight does change 7,016 pixels, and that is a tie, but it is a tie about
  visibility and not about light — and it is a fixture artefact besides, since
  `box_mesh` gives every box a full-height riser where `Prism::mesh` builds each
  riser exactly between two treads.
- **Not precision.** The path tracer, which has no cells, no tiles and no walk,
  draws the identical picture: 261,682 compared, 0 in the interior, 11 on an edge.
  It agrees because it is handed the same nine boxes — which is the point. The
  model is what is wrong, not the arithmetic over it.
- **And it is live, not a fixture's invention.** `Builder::add` declares a
  climbable's treads as **bodies** (`edges == EDGE_ANY`, asserted in
  `occlusion.rs`'s own test: *"a tread is a body: a stair is solid"*), exactly as
  `add_raw` does here. The walk's body branch never consults `same_run` at all —
  that exemption is a run of *panels* along a row or column — so for bodies there
  is no surface exemption of any kind. Only `mine == reference.x`, one primitive.

**What the cosine hides, and what it will not.** In the shaded frame this costs at
most 43 steps of 255, mean under 9, over some 120 pixels: the wall here is
back-facing, and `N·L` darkens it whatever the visibility says. That is luck of
the arrangement. Turn the flame to sit just *in front* of the plane and the same
per-primitive exemption produces acne on a lit surface, where nothing downstream
saves it.

**The fixture for that is a run of *bodies*, and a run of wall cannot be it** —
recorded here because S3 spent an hour finding out. `scene::wall_run_lit_from_along_it`
*is* drawn, by `pictures.rs`'s `a_wall_lit_from_one_end_has_no_dark_stroke_at_its_seam`,
and it is green both before and after S3: a wall is panels, so its seam lands on
`same_run`, which has covered it since phase 4. A body had no exemption at all, which
is why the defect is a body's — so the fixture is nine treads and the gate is
`lighting.rs`'s `a_landing_cut_into_three_primitives_is_not_shadowed_by_its_own_pieces`.
See S3's acceptance, point 2.

## The oracle

**Brute force over every primitive in the scene.** No hierarchy, no cells, no
early exit: `ray_vs_solid` against the whole list, in the wire's own numbers.
That is the one non-circular check available — it shares no traversal with the
walk it judges — and D4 is exactly the claim it makes machine-checkable: the
walk's answer equals the brute-force answer for every ray, whatever the tree
looks like.

It is also what makes every knob in D3 safe to turn: a leaf size or a split rule
that changes a pixel turns this red.

`tests/lighting.rs`'s `brute_force_blocked` and `frame.rs`'s
`ground_truth_blocked` are the existing shape of this and stay — they are dumb
fixed-step point samplers, which is a *different* dumbness and worth keeping
beside an exact one.

### **"No cells" is the load-bearing word, and it cost a day to learn why**

Both samplers looked their boxes up by `solids_at(floor(x), floor(y))`. Everything
else about them was brute force — fixed steps, no DDA, a point-in-box test in
`f64` — and that one line made them not brute force at all: it is the walk's own
indexing with a slower loop inside it, and it inherits the one thing indexing can
get wrong.

**A point on a box's own `max` face floors into the next cell, which does not list
that box.** So a sampler standing *inside* a solid is handed an empty cell and
reports open ground. That is the whole of the corner graze pinned on 2026-08-09
and resolved on the same day (§ *Backlog*), and the damage it does is worse than
being wrong: an oracle arbitrates, so a wrong oracle convicts whichever walk was
right. Both walks were called defective for a day over a wall they had read
correctly.

So the rule, and it is not a preference:

> An oracle iterates **every primitive in the frame** — `Occlusion::solids()`,
> which exists for this and says so — and states its tile exemptions as
> **volumes**, closed on both sides, so a point on a boundary is exempt from both
> columns rather than assigned to one by `floor()`. After the repair there is no
> `floor()` left in either sampler.

**Nor was the step size ever the culprit here, and that matters** because it is the
thing this oracle *has* been patched for twice: the clip in question is `0.000225`
of a tile deep, larger than `BRUTE_STEP`, and the march really did land a point in
it. `the_pinned_corner_graze_is_blocked_and_all_three_oracles_say_so` asserts that
depth against `BRUTE_STEP` for exactly this reason — if a later fixture makes the
sliver thinner than the step, that test says so instead of quietly becoming a
resolution story again.

**And when a sampler and a walk disagree, neither of them is the arbiter.** A
fixed-step sampler can be defeated by a thin enough sliver at any resolution, so
the tie-break is an exact segment-versus-box test: `segment_inside_box` in
`tests/lighting.rs`, the textbook slab test in `f64`, written out in the test's own
arithmetic rather than calling `solid::ray_vs_solid` — being held to the crate's
own slab test would be the crate agreeing with itself. It answered the pinned case
in one run. Reach for it first the next time this shape appears.

### **And on 2026-08-22 it stopped being the tie-break and became the property**

The shape appeared again — same family, same verdict, and this time the step
*was* the culprit: `0.0000282` of a tile, seven times under a `BRUTE_STEP` that
had already been tightened twice. Two red suites for the same lesson is enough,
and the lesson is structural rather than numeric: a fixed step is defeated by a
thin enough clip at any resolution, so a sampler cannot be the thing a walk is
held to.

> The **property** the fuzz tests and the grid sweeps assert is the exact test —
> `deepest_crossing`, which is `segment_inside_box` over `Occlusion::solids()`
> with the two exempt tiles subtracted as volumes. The **sampler is a control on
> it**, and the carve-out runs one way only: the sampler missing a crossing
> thinner than its own step is arithmetic and excused, the sampler finding a
> crossing the exact test denies is never excused — a point in a box is in it,
> and that would be the exact test's own defect.

`Oracles` in `tests/lighting.rs` is both of them and the rule between them.
`BRUTE_STEP` is frozen at `0.0002` by the same argument: the next sliver is not
worth another proportional pile of point tests.

`frame.rs`'s `ground_truth_blocked` is the same fixed-step shape and has **not**
been moved, deliberately: it marches a fixed 64×64 grid of deterministic scenes,
so it cannot go red on a seed nobody chose, and it already answers `Option` —
"cannot say" is a verdict it has and the samplers here do not.

## Steps

Each is landable alone and leaves the tree working. A session starts at the first
one whose gate is not green.

| | Step | State |
|---|---|---|
| S1 | absolute coordinates on the wire | ✅ landed |
| S2 | the detector, before the fix | ✅ built and read |
| S3 | the surface exemption | ✅ landed 2026-08-09 |
| S4 | delete the cell rules | ✅ **all four gone.** `same_run`, the vertical shortcut, `starting_cell` — and the per-cell `max`, which S5 deleted with the cell it was a statement about; `first` went with the grid at the same time |
| S5 | the hierarchy | ✅ **landed 2026-08-09** — both backends walk the tree, the grid is out of the walk everywhere, and the two walks are named for the boxes they read. The one number left is the cost harness's, and it is the user's to run. See § *The hierarchy* |
| S3b | the merge | ✅ **landed 2026-08-09** — a run of wall is one primitive, a floor is one slab, and the crate's own scenes fold 73 pieces to 9. Not one pixel moved. See § *The merge* |
| S6 | the aperture in the primitive's own coordinates | ✅ **landed 2026-08-09** — the last rule stated in a tile, the last plane indexed by a `SolidId` and the last quantised number, in one change to one record. It fixed two live defects and did **not** buy the merge relaxation it looked like it would. See § *The aperture* |

And S5 is the same shape a fourth time: the plan asked for a node budget and
there is nothing to size, because the traversal's own monotonicity is the bound.
A step's *decisions* holding does not mean its *reasons* do.

None of S4's three deletions went the way this plan expected: `same_run` was
retired by three fixtures learning to name their own solid, the vertical
shortcut by a census finding it is entered zero times, and `starting_cell` by a
census finding that the case it was written for is unreachable while the case it
still decides has one answer. Each is written up under its own heading below,
because *how* a rule turned out to be unnecessary is the part a later step
inherits — and after three of them, **a census taken at the call site is the
instrument this step is actually made of.** The pattern in all three: the plan's
own reason for a deletion was not the reason it landed.

**S1 — absolute coordinates on the wire.** D1. ✅ **Landed.** The reconstruction
and the quantisation are gone; a primitive carries its own six numbers.
`walk_cells_streaming` no longer previews a quantisation that does not exist,
which collapses the documented difference between the two CPU walks to an `f32`
rounding. **The ceiling that a primitive is a tile's is lifted here**, and
nothing can merge before it is.

What it took, so a reader does not have to diff for it:

- `Solid::fraction`, `Solid::z_bytes`, `Solid::span_from_bytes`,
  `Solid::box_from_footprint` and `Solid::Z_STEPS` are deleted, and
  `Solid::wire_box` — the record's six corners through `f32` — replaces the lot.
  It is the **only** place the wire's rounding is stated, so the upload and the
  walk that previews it cannot disagree.
- `Occlusion::solid_bytes`, `footprint_bytes` and `solid_z_bytes` — three
  planes, three encodings of one box — become `Occlusion::primitive_bytes`: one
  32-byte struct a primitive, `(lo.xyz, flags, hi.xyz, opacity)`, in a **storage
  buffer**. That is D8 arriving with D1 rather than after it: an absolute
  coordinate does not fit in a channel of an `Rgba8Uint` texel, so the two were
  one change.
- `blit.wesl` loses `box_of`, `footprint_at`, `span_of`, `SolidBox` and the
  `SOLID_Z_STEPS`/`SOLID_Z_FLOOR` pair. `solid_at(id)` is an array index and
  returns the whole primitive; a box is two fields rather than a reconstruction.
  Bindings 13 and 14 are freed and the G-buffer's two planes move down into them.
- `Z_FLOOR`/`Z_CEILING` no longer bound a *span* — a spire through the top of the
  world reaches its own height on the wire now. They are the `Aperture`'s alone,
  which makes a hole's two whole-unit ends the last quantised number in the pass.
  (✅ **And S6 deleted all three**, having measured that the ends a hole is
  clamped between are not the ends a hole *has*: see § *The aperture*.)
- `solid::drawn` stops clamping a drawn box's `z` into an `i8`. It did that to
  draw where the *shader* believed a box was rather than where the map said, and
  with the pin gone from the wire the clamp had become the one thing an
  instrument may not be — a picture of somewhere the renderer is not. **Nothing
  in the suite went red when it was removed**, which is the honest state of that
  view: the rule was never gated.

*Gate, as built:* `light::a_primitive_at_no_fraction_a_byte_could_name_reads_the_
same_three_ways` — a box whose every face sits **half a step** off the byte grid
the old wire measured on, which is the point that grid is maximally wrong about,
with twelve rays aimed parallel to its faces a half-thousandth of a tile to
either side. Both CPU walks and a brute-force oracle over every primitive (no
cells, no traversal shared with either walk) must give one answer to each.
`frame.rs`'s `the_shader_reads_a_primitive_at_no_fraction_a_byte_could_name` is
the shader's third of it, on **the sun** rather than a flame: eight rays at a
sphere spread by `FLAME_RADIUS * t` at the crossing, forty times the half step
being aimed inside, so a flame cannot resolve this fixture at all and a single
directional ray can.

*Fault injection, run:* the `/255` rounding put back in `Solid::wire_box` turns
the CPU gate red (the exact walk against the oracle, on the first ray);
put back in `Occlusion::primitive_bytes` alone — the wire and nowhere else — it
turns the shader gate red, both frames sunlit where one must be shadowed.

**S2 — the detector, before the fix.** § *The detector*, built and read. Its
first number on a real place is recorded here, in this document, as the thing S3
moves.

*Gate:* the census fires on today's tree (it must — the seams are there), reports
how many pairs it examined, and its synthetic twin runs under `cargo test` on a
scene with a known seam.

**S3 — the surface exemption. ✅ Landed 2026-08-09.** D2a, and nothing else: no
geometry is built, moved or merged. `light::on_the_lit_surface` and its twin in
`blit.wesl` are the half-space predicate — a candidate is skipped when its extent
along the fragment's own normal axis ends at the fragment's own plane, from behind
it. Both CPU walks and the shader, one rule, stated once the way `Solid::wire_box`
is, at four call sites and two.

**Three things the step learned, each by a gate going red rather than by argument.**

*The theorem's precondition is load-bearing, and D2 as written left it out.* The
proof says `d·N > 0` — the ray must be *leaving* the plane — and dismisses `d·N < 0`
with "the flame is behind the surface, `N·L` is already zero". That is true of the
shaded frame and **false of the shadow term**, which the reference path tracer
compares directly with no cosine on either side. It said so immediately: 4,017
interior pixels of `line_scene`, every one a `y = 101` face of the west box with the
flame at `y = 98.5` behind it, drawn lit where the tracer had them shadowed by the
east box the exemption had just discarded. So the ray's direction is a parameter and
the precondition is a comparison of two signs. `d·N = 0` stays exempt: a ray lying
in the plane is the graze the whole rule exists for.

*The plane comes from the fragment's own **solid**, not from its position.* Both are
the same number — measured below — but reading it off the box puts both sides of the
comparison in one list and one precision, so the equality is exact by construction
rather than by a rasteriser's good behaviour. It costs nothing: the row a fragment
carries already names its solid.

*It does not eat `same_run`, and S4 may not delete that on this step's licence.*
D2's list of what it subsumes was right about `mine == reference.x` wherever a
stance names a side, and right about the cell arithmetic — but `same_run` is
*broader* than the theorem: it exempts a neighbouring panel of the run whatever the
ray's direction, including rays that dip **behind** the surface's plane, which the
theorem cannot license and the tracer will not allow. A flame is a sphere, so a lamp
standing level with a wall puts half its rays either side of that wall's plane; the
half going behind genuinely crosses the neighbouring panel. What removes those is the
*merge*, S3b — one primitive per surface leaves nothing to cross — not this step. See
§ *Backlog*.

Identity also survives for `Surface::Upright`, which has no plane at all: a tree's
sprite is excused from its own box by name and by nothing else.

What the step must settle, and it was the only open question in it: **where the
fragment's plane comes from.** Reading it off the interpolated position plane and
comparing to a stored box is a float equality across two sources; if it is not
bit-identical, the fix is to carry the plane from the instance row — which
already carries `solid` — and *not* to introduce a tolerance. Measure first, and
record which of the two it was.

✅ **Measured, and it is the interpolated position: they are identical, bit for
bit.** `traced.rs`'s `a_face_fragments_own_plane_is_the_primitives_own_number`
renders the run of flights and compares every face fragment's coordinate on its
own face's axis against that face's own bound: **39,930 fragments, zero off**, on
a scene whose faces sit on the thirds of a tile — the coordinates with no exact
`f32` at all.

The reason it holds is not luck. Every vertex of an axis-aligned face carries the
*same* coordinate on that face's axis, and interpolating a value equal at all
three corners returns it exactly under the `v0 + b·(v1−v0) + c·(v2−v0)` form a
rasteriser uses. So **the exemption is an equality, nothing is added to the
instance row, and S3 adds no number anywhere** — which is acceptance point 6
satisfied by construction rather than by inspection.

The measurement stays as a gate, because what it pins is the *pipeline*: a
projection that went perspective, a vertex format that lost a bit, or a driver
interpolating as `a·v0 + b·v1 + c·v2` would each break the equality while leaving
every picture looking right, and would turn the exemption into a rule that fires
on some pixels of a seam and not others.

### Acceptance for S3, as things to run and numbers to read

Each is a command, an artefact and a figure, so acceptance does not rest on
anybody's description of a picture.

Each is a command, an artefact and a figure, so acceptance does not rest on
anybody's description of a picture. **All six are run below, and two of them turned
out to be asking for the wrong thing** — recorded as found rather than quietly
restated.

1. 🔁 **The seam census. Asked for zero; zero was never the right target, and the
   census cannot be the gate.**
   ```sh
   OPENSHARD_TRACED_DUMP=target/traced/s3 cargo test --release -p openshard-client-render \
     --test traced -- the_frame_and_the_path_tracer_agree_about_a_run_of_flights --nocapture
   ./tools/mask_probe.py seams crates/client/render/target/traced/s3/run_of_flights_pathtrace.png
   ```
   🔴 **And the before/after this first read is not one — the correction is the
   point.** The census reports **87** (12 + 14 + 6 + 24 + 25 + 6) against a figure
   of 123 recorded in a previous session, and that difference was written up here
   as S3's own doing. It is not: with the exemption **neutralised in the shader**
   the census is **87 as well**, and the dumped mask is identical to the last
   pixel — 0 of 2568 × 512. The 123 came from a dump made in some other state, and
   attributing a difference to a change without injecting that change is exactly
   the trap this track has already paid for once (§ *Backlog*, "a dumped picture
   carries no mark of the code that made it"). One number, two sessions, no
   provenance.

   **What S3 moves on screen, measured: nothing.** Run of flights, 0 pixels; wall
   run elevation, 0 of 29,696; the stair fixture under a low front light, 0 of
   262,144. Its exemption is reachable only when a ray runs *in* the surface's own
   plane, which needs a point flame — the gate below uses one, and the shipped
   renderer never does, because a sphere of `FLAME_RADIUS` centred in the plane
   puts half its rays below it. So S3 is a rule made right and a picture unchanged,
   and the seam a person sees belongs to `docs/lighting_rebuild.md`'s backlog
   entry on the flame's own extent — the cosine is taken from the flame's centre
   while visibility is sampled over its whole sphere.

   The census pairs pixels by **which body drew them**, because that is all a dumped
   mask carries; it does not know which *face*. Probed, the first survivor is
   `(299, 218)`: box 0's `FaceSouth` at `(100.992, 100.333, 4.917)` shadowed, beside
   box 3's `Flat` at `(101.008, 100.333, 5.000)` lit. A **riser** beside a **landing
   top** — two surfaces, two normals, a real geometric edge, and the decision is
   supposed to flip there. So a flip across a join has three causes a picture cannot
   separate: a piece of a surface shadowing that surface (the defect), another
   surface's shadow boundary crossing the seam column (legitimate — four of them
   here, and the tracer draws each in the same place), and the walk inventing an edge
   (which `interior == 0` already rules out). The reference tracer cannot arbitrate
   the first against the second either: it holds the same nine boxes, so it
   reproduces a self-shadow as faithfully as a real shadow.

   **What tells them apart is which solid stopped the ray**, and only the walk can
   say. So the gate is `lighting.rs`'s
   `a_landing_cut_into_three_primitives_is_not_shadowed_by_its_own_pieces`: nine
   treads, both walks, forty fragments across each tread's own lid, and no solid of a
   fragment's own landing may be the one `Stopper` names. The Python census stays an
   instrument and now prints a pixel of each run, so a reading can be followed up
   with `OPENSHARD_TRACED_PROBE`.

2. 🔁 **The wall run lit along itself. Already drawn, already green — and it is a
   run of *panels*, so it could not have shown this.**
   `scene::wall_run_lit_from_along_it` *is* drawn by a tool:
   `pictures.rs`'s `a_wall_lit_from_one_end_has_no_dark_stroke_at_its_seam` renders
   its elevation, marks the seams and asserts monotonicity along the run. It passes
   today and passed before this step, because a panel run's seam is what `same_run`
   already covers. The uncovered defect was on a **body**, where there was no
   exemption at all — so the fixture that shows it is a run of bodies, which is what
   the gate in point 1 builds. A "before" picture of the panel scene would have shown
   nothing and proved nothing.

3. ✅ **The brute-force oracle stays equal.** The exemption's whole claim is that it
   discards no true occlusion, and the oracle is the non-circular check of exactly
   that. Green, both fuzz tests and both grid sweeps.

   The pinned corner graze that blocked this point was the *oracle's* own defect,
   resolved by deciding which side was right rather than by widening the fuzzer's
   carve-out; the seed stays pinned and passes. See § *The oracle*'s "no cells" rule.

   ⚠ **And the oracle cannot see this step at all**, which is worth knowing before
   leaning on it: its fixtures light a `Spot::flat` that is a point of *no* solid, so
   `own_box` is `None` and the exemption never fires. It is a check that S3 broke
   nothing, not evidence that S3 works.

4. ✅ **The path tracer stays at `interior == 0`.** Both gates, both scenes —
   261,682 pixels compared on the run of flights, 0 interior, 11 on an edge. It is
   also what caught the missing precondition, at 4,017 pixels.

5. ✅ **Fault injection, both directions, both run.**
   - *Neutralised* (`on_the_lit_surface` returns `false` outright): the landing gate
     reports **480 of 720** fragments shadowed by a piece of their own landing, on
     both walks. The three tests phase 4 found stay red under *their* injection —
     identity is untouched here.
   - *Widened past the theorem* (a candidate skipped when it merely **touches** the
     plane, from either side): `a_room_lights_its_own_wall_and_not_the_storey_over_it`
     goes red. A real occlusion discarded, which is what that injection is for.

   And one injection that came free: while the fixture in point 1 was written with
   the flame *above* a landing it passed **with the exemption neutralised** — a
   vacuous gate, because a ray leaving a lid upward touches the neighbouring piece
   only at `t = 0` and the zero-length touch rule already answers it. The flame had
   to go **into** the surface's own plane for the exemption to be the only thing that
   can excuse the crossing. A gate that passes under injection is not a gate, and
   this one nearly shipped as one.

6. ✅ **No new constant.** No tolerance, no epsilon, no widened box: the diff adds a
   plane comparison, a sign comparison and a table of five stances. The plane did not
   have to come from the instance row, so nothing was added there either.

**S3b — the merge, and it is last. ✅ Landed 2026-08-09.** D2b. Contiguous
neighbours that are the same surface become one primitive at build time, after
`occlusion::boxes_of` and inside `Builder::finish`. Two primitives merge exactly
when they share a whole face, have equal opacity, equal `edges` classification and
equal span — all exact comparisons, since the coordinates come from integers and
authored fractions, and **no tolerance is introduced anywhere**. `occlusion::Part`
keeps pointing every instance at the solid it is now a part of (D6).

🔴 **That list was three fields short, and the missing three are what keep the
join and the arithmetic**: the `Owner` and the `Part` themselves (D6's join is a
scan for the pair, so a merged primitive may not disagree about either), an
aperture (a hole is a fraction of *one tile* of its run — ⚠ true when this was
written, and S6's own correction is under § *The merge*), and opacity being
`OPAQUE` rather than merely equal (two panes dim twice, one merged pane dims
once). See § *The merge* for what each would have broken.

~~**`PANEL_THICKNESS`'s inward fattening is answered here** and not separately:
two walls on a shared tile edge are one surface, so they merge into one slab lying
on the plane the art draws, which is what the `docs/lighting_rebuild.md` backlog
entry asks for.~~ 🔴 **It is not answered here, and the reason is `edges`.** A
tile's north panel and its northern neighbour's south panel do share a whole face
— both are fattened inward from the plane between them — but they are a
`EDGE_NORTH` and a `EDGE_SOUTH`, and a merged primitive would have to carry both.
`Solid::edges` is documented as never two named sides ("a corner is two panels,
which is what the list is for"), and the walk's panel arm reads it. So the merge
as built refuses them, which is exact and leaves the fattening exactly where it
was. What would answer it is a decision about what a two-sided panel means to
`pierced` and to `on_the_lit_surface` — a change to what one primitive *is*, which
is what § *Not in scope*'s "lateral fit" entry is also about. It stays
`docs/lighting_rebuild.md`'s backlog item. The constant is still both *how thick a
wall is* and *which side of its tile it sits on*.

*Gate:* **not one pixel moves.** That is the whole of what a pure optimisation
means, and it is checkable: the shadow masks before and after are identical, and
the cost harness says the primitive count fell. It runs **after** S5, since a
merged primitive is wider than a tile and the grid stops being a superset the
moment one exists — see the backlog's first entry, which is this step's own
precondition and not a nuisance.

**S4 — delete the cell rules.** D5, in this order and each behind its own
measurement: ~~`same_run`~~ ✅ **deleted** (🔴 **not licensed by S3, and licensed by phase 5b rather than by the
merge** — S3 landed and measured that the exemption is *narrower* than this
function, which excuses a neighbouring panel of the run for rays that dip behind the
surface's plane as well as for rays leaving it. The theorem cannot license those and
the path tracer will not allow them. What was written here was that the merge
retires it; what actually does is that **those rays stop being traced**: a sample
behind the fragment's own plane has a zero cosine and contributes nothing, so there
is no crossing left for `same_run` to excuse. That is
`docs/lighting_rebuild.md`'s phase 5b, it is measured rather than argued, and this
deletion waits on it — not on S3b. ⚠ **Phase 5b landed and did *not* license it.**
Neutralised after it, `light_runs_along_a_wall_and_stops_across_it` and
`the_two_faces_of_a_corner_are_lit_from_the_side_each_looks_at` still go red,
exactly as phase 4 measured them. The reading is inconclusive rather than
negative, and the reason is a *fixture*: both build their spots with `Spot::face`
and no `part_of`, so `spot.solid` is `None` and D2 — which needs the fragment's
own box — is never consulted. Naming the solid in those two fixtures is one call
each and is what this deletion actually waits on now; it is the first entry of
`lighting_rebuild.md`'s backlog. See S3's own list of what it learned.
✅ **Made, and the deletion is licensed.** It was three places, not two — the
third being `plan::elevation`, which wrote `OwnerId::NONE` into every row of the
two wall *pictures* under a comment that a diagnostic is never walked for
shadows, while `View::Flames` is exactly a walk. With all three naming their own
solid, neutralising `same_run` on **both** sides leaves all 510 tests of the
crate green but `same_run`'s own unit test; the same injection turned four red
before. The controls: D2 neutralised with `same_run` live is also green, and both
neutralised turns two red, so the pair is load-bearing and the two are mutually
redundant on every fixture. The tie is broken by the argument rather than by the
suite — D2 is a theorem, `same_run` excuses more than the theorem allows, a
tile's north panel on the same row among it. `lighting_rebuild.md`'s backlog has
the numbers.
✅ **Deleted**, from `light.rs`'s two walks and from `blit.wesl`, with
`on_surface` — the height half of the mask, and a function `same_run` was the
only reader of — and the two unit tests whose whole subject was either. The panel
arm of all three walks is now `pierced` and nothing else. Its grave note lives
above `light::lit_plane`, and it states the narrowing as well as the deletion: the
mask excused a tile's **north** panel for a south-facing fragment on the same row,
a different plane and a real occlusion the theorem correctly keeps. The gate below
was taken in full — suite green with the rule neutralised on both sides before the
cut, suite green after it, brute-force oracles and the GPU parity sweep among
them, and the identity injection turns exactly the same six tests red before and
after: `a_fragment_is_shadowed_by_every_solid_of_its_own_static_but_the_one_it_is_
a_point_of`, `a_vertical_ray_is_not_stopped_by_lids_it_is_not_over`,
`a_carried_light_lights_the_way_it_is_pointed`,
`the_face_of_a_wall_is_lit_from_inside_the_room` and both path-tracer gates), the
per-cell `max` (🔴 **measured 2026-08-09, and it does not land on this
measurement** — see below), ~~the vertical
shortcut~~ ✅ **deleted 2026-08-09, and it was a live defect and not only a
branch** — see below, and the licence was a census rather than a green suite —
and ~~`starting_cell` with `first`~~ ✅ **`starting_cell` deleted 2026-08-09;
`first` stays until S5**, which is the honest split of what "nothing left reads a
cell" was asking for. The arbiter between a fragment's position and its carried
tile is gone, and so is the carried tile itself — off `LitEnd`, off `dda_walk`
and `candidate_tiles`, and off `walk`, `arrival` and `sunlight` in `blit.wesl`,
which threaded it down three functions for one reader. What remains is one line
of `from.floor()` at each of the three walks: a cell used as an **index**, which
is what S5 deletes when it deletes the grid. See below.

*Gate:* each deletion is preceded by neutralising the rule and finding the suite
green, and followed by the brute-force oracle staying equal. The three tests that
phase 4 found go red when identity is neutralised must stay red under that
injection — the self-shadow rule is **not** part of this and must not be
weakened by the merge.

### ✅ The starting cell: **the case it was written for is unreachable, and the case it decides has one answer**

Deleted from `light.rs`'s two walks and from `blit.wesl`, with everything that
existed to feed it: `LitEnd`'s `tile` field, the `tile` parameter of `dda_walk`
and `candidate_tiles`, and the `tile` parameter of `walk`, `arrival` and
`sunlight` — three functions carrying the place attachment's own tile down to a
single reader. Also gone, found while removing it: **`cell_stopped`'s `first`
parameter, which nothing in its body read** — the same shape as the `ground`
parameter the vertical shortcut left behind, and worth a line for the same
reason, that a dead parameter reads as a rule.

**The rule was an arbiter, not an exemption.** It read *"the carried tile, unless
the start point is strictly outside it"* — the carried tile to break the tie a
point on its own far edge is, `floor` to stop that tile contradicting the
position outright. Only the second half was ever load-bearing, and it is what
took the 324 leaked pixels above to zero.

**The census, taken at the two walks themselves** — inside the rule it would have
been drowned by its own proptest, which calls it four thousand times with
nothing else in the frame:

| | walks | `floor` ≠ carried tile | of those, the exact-edge tie |
|---|---|---|---|
| `lib.rs` | ≥65,536 | 34 | 16 ⇒ **18 strictly outside** |
| `tests/frame.rs` | ≥262,144 | 0 | 0 |
| `tests/lighting.rs` | ≥32,768 | 11,528 | 11,528 ⇒ **0 strictly outside** |

So the case the rule was written for happens **18 times, all of them in the two
fixtures written for it**, and there the rule *is* `floor`. Zero times in any
generated or rendered scene. What it still decided, 11,544 times, is the
exact-edge tie — the normal state of every south and east face since
`docs/lighting_rebuild.md` phase 6c.

**And that tie has one answer.** Both cells contain a point on the boundary
between them, and a walk seeds its distance to the next boundary from the cell it
starts in — so from either seed the other cell is reached at `t = 0` if the ray
heads that way, and touched at a single point if it does not. A primitive is a
tile's, so a solid listed on the merely-touched cell lies inside it and
`ray_vs_solid`'s zero-length touch rule already refuses to block on it.
⚠ **That last clause is the precondition, and S3b is what breaks it**: the first
box wider than its own tile makes "touched at a point" and "meets the segment"
two different things again. By then S5 has taken the cell away entirely, which is
the order this plan already fixes — but it is a second reason the merge may not
be brought forward, beside the superset hole in the backlog.

**A latent CPU/GPU divergence came out with it.** `blit.wesl` stepped its walk
from `first` while seeding `boundary` from the carried `tile`; `light.rs` has
always used its own `first`. The two are the same number for every ray the suite
draws and differ in exactly the strictly-outside case — the one no fixture
reaches. One cell on both sides now, and it cost nothing to fix because the
parameter it disagreed with was being deleted anyway.

*Gates, both directions, both run:*

- *Neutralised* — `floor` in both CPU walks and in `blit.wesl`: the crate is
  green **but the rule's own unit test**, `same_run`'s shape exactly.
- *Positive control* — seeded with a cell the point is **not** in (`floor + 1`),
  on both backends: 5 unit tests, 14 of `tests/lighting.rs`, `frame.rs`'s
  `the_shader_stops_a_vertical_ray_with_the_panel_it_stands_inside`,
  `pictures.rs`'s `a_wall_in_front_of_a_torch_darkens_the_ground_behind_it_and_
  not_beside_it` and all three `traced.rs` gates go red. The suite is amply
  sensitive to **which** cell a walk starts in; what it is indifferent to is
  which of two cells a point on their boundary is called, and that is the
  distinction the whole deletion turns on.
- *Identity* — the self-shadow injection turns exactly the same six tests red as
  before the cut.

**Two fixtures lost their own case, and one of them was kept.** The CPU twin,
`a_ray_starting_just_past_its_own_tile_is_stopped_by_the_cell_it_is_in`, was
built entirely out of a disagreement between a carried tile and a position;
with no carried tile it asserted that a ray inside a wall is stopped by it, and
it stayed **green under the positive control**. Deleted, with a grave note. What
replaces it is `a_walk_starts_in_a_cell_its_own_start_point_is_in`, repointed
from the deleted helper to `dda_walk`'s own first cell — the half of its claim
that outlives the rule, and it does go red under the injection. `frame.rs`'s GPU
twin is **kept and re-labelled**: it is the only place in the crate where a
fragment's position and its carried tile are made to differ, so it is worth
having as the scene that would show the tile being read again — and its doc now
says outright that it is a fixture rather than a gate, because it too stays green
under the injection.

This is the third fixture on this track whose subject was taken away by a later
phase and which went on passing. The other two are in the vertical shortcut
below. The sweep the backlog asks for is still not done.

### ✅ The vertical shortcut: **entered zero times, and wrong when entered**

Deleted from `light.rs`'s two walks and from `blit.wesl`, with `over_footprint`
— [`ray_vs_solid`]'s two horizontal axes spelled again, and a function the
shortcut was the only reader of — and the `ground` parameter `cell_stopped`
had stopped using. `dda_walk` answers a ray with no horizontal run with exactly
the one starting cell the branch returned by hand, so **which** cells are looked
at did not change.

**What did change is which shapes count, and there the branch was a defect.** It
skipped every **panel** outright, on the argument that a panel is a plane and a
vertical ray lying in a wall's own plane is a graze it had no rule for. Two
things were wrong with that. It has had a rule since S3 — `on_the_lit_surface`
is called inside the branch too — and a panel is not a plane in the grid: it is
a `PANEL_THICKNESS`-deep slab a fragment can stand inside and a ray can run the
whole height of. Measured: a fragment inside a wall's own thickness, lit from
twenty `z` straight overhead, was handed **the full flame** by both walks and by
the shader.

**The licence is a census, and the census is what makes this step honest.**
Instrumented — both answers computed for every straight-up ray, printed — the
whole crate enters the branch **zero times**. Not "agrees everywhere": never
runs. The reason is `docs/lighting_rebuild.md`'s phase 5: a flame is a sphere and
`light::flame_points` lays its samples at `sqrt((i + 0.5) / n)` of the radius, so
**no sample is the centre** and a flame directly overhead is eight rays each
leaning a `FLAME_RADIUS` out of the vertical. `walk_sun` answers an overhead sun
before any walk starts. So the branch has been unreachable in the shipped
renderer since phase 5, and the only configuration that still reaches it is
`flame_radius = 0` — the `OPENSHARD_FLAME_RADIUS` knob.

**Both tests named for the vertical case had stopped sending a vertical ray, and
went on passing.** `light::a_vertical_ray_is_not_stopped_by_lids_it_is_not_over`
and `frame.rs`'s `the_shader_does_not_stop_a_vertical_ray_with_a_lid_it_is_not_
under` were written before phase 5 and were measuring the ordinary walk
afterwards. Both set `flame_radius` to `0.0` now and both carry a **positive
control** — every point `flame_points` returns must have the fragment's own `x`
and `y` — so a fixture that stops reaching its own subject fails instead of
passing for the wrong reason. This is the per-cell `max`'s lesson one step on: a
detector must count what it checked, and a *gate* must show that it reached what
it is about.

*Gates, both fault-injected to red in the session that trusts them:*

- `tests/lighting.rs`'s `a_vertical_ray_meets_what_stands_over_it_whatever_shape_
  it_is` — a lid, a body and a panel over one fragment, the two CPU walks against
  `segment_inside_box` over every primitive in the frame. With the shortcut
  restored as the answer it reports the panel at `1.000` where the geometry says
  `0`.
- `frame.rs`'s `the_shader_stops_a_vertical_ray_with_the_panel_it_stands_inside`
  — the shader's own third, which nothing else reaches: the last pixel down a
  tile sits at `112/127` of it, inside a south panel's slab, and the control is
  the same pixel with the wall taken out of the grid. With the branch pasted back
  into `blit.wesl` it reads **230 against 230** — the wall makes no difference at
  all.
- The identity injection turns **exactly the same six tests** red as S4's own
  record lists for before the cut, so the self-shadow rule is demonstrably
  untouched.

⚠ **And the oracle lied first, which is the finding worth keeping.** The exact
oracle was first written as "some primitive takes a *positive length* of the
ray", and a **lid is a box flat in `z`** — no segment ever spends a positive
length inside one. It answered "open" for a fragment squarely under a plank and
would have convicted both walks of a defect they do not have. Crossing a surface
is a plane crossing and not an interval; `enter < 1 && leave > 0` — the primitive
meets the *open* segment — is the one statement that covers a slab and a plane
alike. Same shape as the corner graze in § *The oracle*: an oracle arbitrates, so
a wrong one convicts the walk that was right.

### ✅ The per-cell `max`: the suite was **blind** to it, and S5 removed what it grouped by

Neutralised the way `same_run` was — the `max` within a cell replaced by a
product, `1 - (1-stopped)(1-by_surface)`, in both CPU walks *and* in `blit.wesl`
— the whole crate stays green: 513 tests, the brute-force oracles, the GPU
parity sweep and both path-tracer gates among them.

**That green means nothing, and the number beside it is why.** Instrumented, a
second solid of one cell stops the ray **1,359 times** across the suite — so the
rule is amply *reached* — and every one of those is
`already 1.000, now 1.000, opacity 255`. Both stoppers are opaque and the first
has already saturated, where `max(1, 1)` and `1 - 0·0` are the same number by
arithmetic. The suite cannot tell the two rules apart because nothing in it puts
**two partial stoppers on one cell**, which is the only arrangement where they
differ: a pane (`opacity 51`) beside another partial occluder.

**And they are not interchangeable, so the missing fixture is not a formality.**
A product is right for two *independent* surfaces — a ray through two panes is
dimmed twice — and the `max` is right for two panels of **one** surface, which
is a corner: one wall, counted once. D5's licence for this deletion is "there is
no cell to group by", and that arrives with **S5**; what has to arrive with it is
the grouping becoming *by surface* rather than disappearing. S3b does not supply
it either — a corner's two panels are perpendicular, so no merge joins them, as
§ *Not in scope* already says.

⇒ ~~**This deletion waits**, and what it waits on is a fixture with two partial
stoppers on one cell and a decision about what the right answer there is.~~
✅ **Both arrived with S5, and the answer is that there is no grouping.** A cell
was the only thing that ever grouped two primitives, and the tree does not have
one — so what a segment crossing two volumes gets is a product, because it is
stopped by both. The fixture is `lighting.rs`'s
`a_segment_through_two_panes_on_one_tile_is_dimmed_by_both_of_them`: two `WINDOW`
statics on one tile, one ray through both, and the assertion is `0.8 · 0.8`.
Restoring the `max` turns it red **and nothing else in the crate** — which is the
blindness above measured rather than asserted, one phase later.

⚠ **The corner is not what that fixture answers, and it stays open.** Two
independent panes are the case where a product is unambiguously right: two
sheets of glass, two dimmings. A corner's two panels *overlap* in the square
where they meet, so a ray through that square crosses one wall's material twice
and the `max` had a real argument there — this is the one place the deletion
changes an answer rather than an arithmetic. It is unreachable today (it needs a
corner static carrying `WINDOW` and a ray through a fifth of a tile squared), and
what would close it is D6's join one level finer: a volume that knows which
instance it is part of can be counted once without a cell. Filed in § *Not in
scope*'s neighbour rather than answered by a fixture built to whichever rule
shipped.

**Two false readings on the way, both worth the line they cost.** The first
census read **0** — `eprintln!` inside a test is swallowed by libtest without
`--nocapture`, so the detector was printing into a closed pipe and reporting
"never happens". The second read **387 divergences**, every one of them
`already 0.000, now 0.200`: that is `1 - (1-0)(1-0.2)` failing to be exactly
`0.2` in `f32`, so the detector was measuring its own rounding. A detector that
prints where nobody reads and one that counts its own arithmetic are the same
defect as a vacuous gate — **it must count what it checked, and be shown a case
it is known to fire on.**

### ✅ The hierarchy: **both backends walk the tree**

Landed 2026-08-09, over two sessions: the build and the CPU walks first, then
the gate the shader port needed and the port itself.

**What is in the tree.** `occlusion::bvh` — median split on the longest axis,
leaves of up to four, depth-first layout with an escape index a node. Built in
`Builder::finish`, held **beside** the tile index rather than instead of it: the
grid keeps the job it is good at, which is answering about a *tile*
(`Occlusion::at`'s merged view, `owner_at`'s join, the wireframe, the plan view).
What moved to the tree is the walk, whose question was never about tiles.

**What went with it.** `dda_walk`, `candidate_tiles`, `DdaCell`, the
`from.floor()` each walk seeded itself with, and `MAX_WALK_STEPS`.

**And the two walks became one function.** After S1 they differed by exactly
which box a primitive is — `space` or `wire_box` — so a second copy of a hundred
and fifty lines was two chances for one rule to drift. `walk_cells_exact` and
`walk_cells_streaming` are one call each into `walk_primitives` now, with that
one difference as a parameter. (Their names still say *cells*; the rename is
outstanding.)

**Three of this plan's backlog entries close on this side by construction**, and
they are the reason D3 says the hierarchy is not about speed:

- a cell listed a primitive **once**, so the first box wider than its own tile
  would have been invisible to a ray crossing only the overhang;
- listing one from **two** cells would have double-counted it, since `through`
  was multiplied cell after cell. A primitive is under exactly one leaf;
- a `floor()` decided which cell a boundary point was in.

✅ **And they close on the shader too since the port below**, so S3b's own
precondition — one broad phase, on both sides, that a merged primitive cannot
outgrow — is met.

**No node budget, and that departs from this plan's letter** (below), which asks
for one "in the same role" as `MAX_WALK_STEPS`. There is nothing to size: a
traversal moves to `at + 1` on a hit and to that node's escape on a miss, and
**both are strictly forward**, so it visits each node at most once and is bounded
by the tree the frame has — which no radius can widen, where a cell count was the
*ray's* length. What could break that is a malformed tree out of a buffer this
side did not write, and the loop stops on exactly that rather than looping: a
constant-free guard where a budget would have been a number to defend. Measured
anyway, because the shape of a loop is not an argument: **the deepest traversal
over the whole suite visits 33 nodes of a 49-node tree.**

**Blame had to be computed rather than arrived at.** `walk_cells_exact` used to
sort its candidate cells by nearest crossing and blame the one that tripped the
cutoff, which was the first blocking cell in ray order. A tree hands its leaves
back in its own order, so `Stopper` is now the **earliest crossing** among the
primitives that stopped anything, and the cutoff is applied after the traversal
instead of exiting it — which is what makes the blame independent of traversal
order at all. `Stopper::cell` is derived from the blamed primitive's own low
corner: a report's coordinate, not a rule's.

*Gates, all run:*

- The whole crate green — **526 tests**, the brute-force oracles, both fuzzers
  and both path-tracer gates among them — with the CPU on the tree and the shader
  on the grid. So the two broad phases agree on every fixture the crate draws.
- *Positive control*: a leaf dropping one primitive turns **21 of
  `tests/lighting.rs`** red, both brute-force sweeps and both fuzzers among them,
  plus 2 unit tests.
- *Identity*: the self-shadow injection turns exactly the **same four CPU tests**
  red as S4 records. The other two of its six are the path-tracer gates, which a
  CPU-side injection cannot reach while `blit.wesl` is unchanged.
- The tree's own eight structural gates, each fault-injected in the session that
  wrote them — and **two of them were vacuous first**, which is recorded above
  the tests: "built twice, equal twice" cannot fail, because `Bvh::of` takes
  nothing but the list; and its replacement's first fixture was forty primitives
  at one place, which an unstable sort leaves exactly as it found them.

⚠ **`tests/frame.rs` stayed fully green under the leaf injection**, and that
decides what can gate the shader port. Its surviving CPU comparisons are the
exact walk against the streaming walk — which now *share* the traversal — so
they cannot see a broad phase losing primitives. What is left to gate the shader
is `traced.rs` against the path tracer, `pictures.rs`, and the shader's own
hand-built fixtures. A sweep of the shader against `light::sample` is what this
step should add, and it is the first thing the next session needs.

### ✅ The sweep that gates the port: **the shader against `light::sample`, on visibility alone**

Built first, before a line of `blit.wesl` moved, because the hole above is
exactly a missing gate: `shader_sweep` in `tests/frame.rs`, six scenes, 4,096
pixels each.

**The subject is `View::Shadow` and not the lit frame**, and that is what keeps
it about the walk. The shadow view draws `Arrival::visible` — visibility alone,
linear, no cosine, no falloff, no tone map — so a ray the broad phase stops
handing over is a whole **eighth** of the number. The lit frame would put that
same eighth through a cosine, a falloff and a curve that saturates on the white
art these fixtures are drawn with.

Nine `the_shader_..._agrees_with_light_sample` sweeps were **deleted on
2026-08-08** (`969c735`, "a test that compares us with ourselves has no
subject"), and that reasoning still holds for what it was about: both sides of
those were the shading model phases 2–5 were replacing. This one is the opposite
case and is why the shape came back — the *narrow* phase is settled and shared by
construction, and what is being replaced under it is one side's **broad** phase,
which D4 forbids to change any answer. Two spellings of one traversal with no
compiler between them is precisely S5's own stated gate.

Three states and not one number, because the view paints two of them a colour on
purpose: no flame in reach, a flame reaching and wholly stopped, and a
visibility. A sweep that let the first two collapse would agree about which
pixels are out of range and about nothing else.

*The census is asserted, not printed*, and it is the fixture's own
anti-vacuity: a scene must put pixels **both** behind something and in front of
a flame. It fired immediately — an earlier version also demanded a penumbra
pixel and four of the six scenes have none, because a flame is a twentieth of a
tile and a fixture pixel an eighth of one, so a shadow's edge legitimately falls
between pixels. Measured, per scene: 1,308–2,400 pixels in shadow and
1,192–2,683 seeing a flame; the house corner and the holed wall have 155 and 47
penumbra pixels between them and the other four have none.

*Fault injection, run:* a leaf dropping its first primitive — `if k == 0u
{ continue; }` in `cell_stopped` — turns **all six red**, 1,355 to 2,400 pixels
apiece. That is the failure the port can actually have, and it is now loud.

🔴 **And a second injection found the blind spot, which is worth more than the
green.** Deleting the shader's own unconditional diagonal probe — the DDA's
corner-tie candidate, the one thing in `walk` that exists for a ray that grazes a
neighbouring cell — changes **not one pixel of these six scenes, and not one test
in the whole crate**. So the suite has never gated that probe, and this sweep
cannot see a broad phase that misses a corner-grazing candidate either. The port
deletes the probe with the grid (a tree has no ties to break), and this says the
deletion is unmeasurable rather than measured — see § *Backlog*.

### ✅ The shader on the tree: **the grid is out of the walk on both sides**

Landed 2026-08-09, with the sweep above as its gate.

**Two storage buffers at the free bindings**, 15 and 16: the nodes
(`Occlusion::node_bytes`) and the permutation their leaves index into
(`order_bytes`). A node is `Primitive`'s own shape — two `vec3<f32>` corners with
a `u32` in the padding each leaves behind — carrying the escape index in one of
those words and the leaf in the other, packed `first << 3 | count`. Three bits is
the whole count, a leaf holding at most four; a second `u32` would have cost
sixteen bytes a node, since a struct whose widest member is a `vec3<f32>` rounds
up to a multiple of sixteen either way.

🔑 **A traversal ends at the root's own escape, and that is what makes the buffer
safe to grow and never shrink.** `arrayLength` would have been the obvious thing
and would have been wrong: these buffers keep the capacity of the largest frame,
so their length is last frame's tree, not this one's. The root's escape *is* this
frame's node count. It also removes the empty-world case — a frame with no
occluder uploads one node of zeros, whose escape is zero, so the loop runs no
iterations. `bvh`'s `a_nodes_escape_is_the_end_of_its_own_subtree` gained the
assertion that says the root escapes past the last node, since the shader now
rests on it.

**And the tree is uploaded before `upload_grid`'s own early return**, not after:
a frame with no grid at all still binds these, and a tree left from the last
frame would be a traversal of geometry the camera has walked away from.

**`cell_stopped` became `primitive_stopped`**, and the per-cell `max` went with
the cell it was a statement about — the product is the walk's now, one factor a
primitive, which is what `light::walk_primitives` has done since the CPU moved.
The DDA, its `first = floor(start.xy)`, its boundary arithmetic, its
unconditional diagonal probe and `MAX_WALK_STEPS` are all deleted.

*Gates, all run:*

- The whole crate green with both backends on the tree, the new sweep included.
- *Fault injection, into the new traversal*: a leaf skipping its first primitive
  turns **five of the six** sweeps red (377–519 pixels); a hit that does not
  descend — `next = node.escape` in place of `at + 1` — turns **all six** red,
  1,355–2,400 pixels. The one scene the first injection leaves green is
  `wall_with_a_torch_beside_it`, whose leaves' first primitives happen to stop no
  ray it draws.
- *The wire itself*: `the_wire_carries_the_tree_the_walk_reads` reads the bytes
  back from the offsets rather than through the writer — the root's escape is the
  node count, a leaf's `first` and `count` survive the packing, and the
  permutation names every primitive exactly once. Both halves fault-injected
  (`<< 4` for the pack, `escape + 1` for the escape) and both go red.

*Found on the way, and fixed:* `PRIMITIVE_BYTES`' own doc named a gate,
`the_wire_carries_a_primitives_own_six_numbers`, that **has never existed**. That
is this file's own decay pattern — a comment describing a rule nothing compiles
against — one level up: a comment describing a *test* nothing runs.

### ✅ The cost harness, and what this plan was wrong about

**`tests/cost.rs` prices the walk over real occluders and always has.** This
plan's own remainder said it "builds against `Occlusion::EMPTY` and therefore
cannot price this at all", and that is a misreading of the one `Occlusion::EMPTY`
in the file: it is handed to the **statics** pass, so a fragment there takes the
billboard fallback and the *impostor* is what goes unpriced. The blit's five
cases are lit with `light::collect`'s own grid off the middle of Britain —
`night` walks real rays through real primitives, and since the port, through the
tree.

What it genuinely lacked was any *report* of the thing being walked, so a reading
could not be read against the geometry that produced it. Added: the tree's node
count and its bytes a frame beside the standing-cell count, the tree's two
buffers in the upload accounting, and a companion assertion in the same spirit as
the two beside it — a frame whose tree is a single node prices four primitives
tested outright and no traversal, which would be a reading of the narrow phase
wearing the wide one's name.

🚩 **It is `#[ignore]`d and wants a client and an adapter, so the number is the
user's to take**, not this session's:

```sh
OPENSHARD_CLIENT=… cargo test --release -p openshard-client-render \
    --test cost -- --ignored --nocapture
```

### ✅ The merge: **a run of wall is one primitive, and the plan's rule was three fields short**

Landed 2026-08-09. `occlusion::merge` is the whole of it: a pass in
`Builder::finish` between the cells' own references and the primitives they name,
and the first thing in this crate to point **two cells at one primitive** — which
is what the reference level built at step 23.1 was for, and it cost exactly the
one function that step said it would.

**What it folds, measured on every scene the crate draws** —
`lighting.rs`'s `the_merge_folds_the_scenes_this_crate_draws`, which prints a line
a scene and fails if the total does not halve:

| scene | pieces | primitives |
|---|---|---|
| a torch on the ground floor of a two-storey house | 73 | 9 |
| a roofed, sunlit room with a window | 49 | 7 |
| a shut, roofed house at noon | 49 | 5 |
| a lit room with a second storey over it | 35 | 3 |
| a shut room with a torch in it | 24 | 4 |
| a sconce on a straight wall | 7 | 1 |

The census exists because **a green suite through a change nothing reached says
nothing**, and this track has read one that way twice. It is asserted rather than
printed, for the same reason.

**The condition is "share a whole face and be identical in every other field",
and three of those fields are not in this plan's own sentence.** Each is a thing
the merge would otherwise break, and each is written up in `merge.rs`'s header:

- **The `Owner` and the `Part`.** They are `Occlusion::id_of`'s join — a drawn
  instance finding the primitive its own pixels are met against, which is D6.
  A merged primitive carries one of each, so folding two pieces that disagree
  leaves the other instance naming nothing. With them equal the join is preserved
  *by construction*, since both cells scan for the same pair. It is a narrower
  rule than the plan wrote, and the honest cost is that **a run assembled out of
  two graphics does not merge**; an `Owner` carries no tile, so a run of one
  graphic at one height is one owner however long it is.
- **An aperture stops a merge outright**, and that is a real finding rather than a
  caution: `light::run_v` is `along - along.floor()`, so a hole is a fraction of
  **one tile** of its panel's run. It is the last rule in the pass still stated in
  a tile — D1 removed every other one — and a merged windowed run would put the
  window in every tile of it. In the backlog.

  🔴 **The refusal is right and this reason for it is not** — S6 states the hole
  in world coordinates, and a merged run would carry its window exactly where the
  window stands. What keeps the refusal is that a primitive has **one** aperture
  field and a run of windows is one hole per tile; and the relaxation that
  suggests itself is unreachable, because an equal `Owner` names one graphic and a
  hole is read off the graphic. See § *The aperture*.
- **Opacity equal is not enough; it must be `OPAQUE`.** Two panes crossed by one
  ray are dimmed twice and one merged pane dims once. That is a moved pixel, so a
  translucent primitive is left alone. S5's own fixture,
  `a_segment_through_two_panes_on_one_tile_is_dimmed_by_both_of_them`, is the
  thing that would have gone red.

**Only the horizontal axes**, and the reason is that the vertical cannot fire: an
equal `Owner` includes the `z` the static stands at, so two primitives stacked in
`z` are two owners by construction. A rule that cannot fire is a rule not worth
writing — the fourth time on this track that a step's *decision* held while its
*reason* did not.

*Gates, all run:*

- **The whole crate green**, both backends, with the census above saying the merge
  is amply reached. `traced.rs`'s two path-tracer gates are the non-circular half:
  the tracer is handed the scene's authored `BoxSpec`s and has never seen an
  `Occlusion`, so a merge that moved geometry would show up as a moved pixel
  against it.
- **The twin oracle**, `lighting.rs`'s
  `a_merged_run_answers_every_ray_the_way_its_own_pieces_did`: one geometry stated
  twice — seven whole-tile bodies of *one* graphic, which merge into one
  primitive, and the same seven with a graphic a tile, which cannot merge at all
  because of the owner condition above. Three flames and 17,424 spots swept at a
  quarter tile over three heights, and every ray must come back with the same
  number from both. The spots are points of **nothing** on purpose: a fragment of
  the run is exempt from all of it after the merge, and that is D6 arriving rather
  than a defect to gate at zero.
- *Fault injection, three, each red for its own reason*: a union that does not
  grow (the merged box stays the first piece's) turns **10 of `tests/lighting.rs`**
  red, the twin oracle among them; the owner dropped from the key turns the twin
  oracle and `the_two_faces_of_a_corner_are_lit_from_the_side_each_looks_at` red;
  a contiguity test loosened from `==` to `<=` — a gap folded into a run — turns
  10 of `tests/lighting.rs` and two of the merge's own unit gates red.
- *Identity*: the self-shadow injection turns exactly the **same four CPU tests**
  red as S5 records.

🔴 **And the injections found a blind spot worth more than the green.** Under the
"union does not grow" injection — geometry genuinely wrong, ten CPU gates red —
`tests/frame.rs`, `tests/pictures.rs` and `tests/traced.rs` all stayed **fully
green**. For `traced.rs` the reason is that its scenes are built by
`oracle::boxes::box_owner`, which gives every box its own graphic, so **nothing in
them ever merges**; for `frame.rs` it is that the shader sweep compares the shader
against `light::sample` and both walk the same broken geometry. So the two
instruments this track leans on hardest cannot see this step at all, and what
gates it is the CPU suite and the twin oracle. In the backlog — and the tracer's
half of it is closed by the section below. ⚠ **"Cannot see" is the wrong word for
the other two, measured afterwards**: both walk the merged geometry and one of
them *drew* the defect, so what they were short of was an assertion rather than a
scene. `pictures.rs` goes red under this injection now; the sweep never will, and
why is measured under the backlog's § *Neither instrument is unreached*.

**Two fixtures lost their subject, and both were repaired rather than deleted.**

- `light_runs_along_a_wall_and_stops_across_it`'s third ray was the same scene
  with no art, where every occluder is a whole tile and the along-ray died — the
  pre-decision-3 behaviour reproduced on demand. The merge folds a run of
  same-graphic whole-tile bodies into **one** primitive, so a fragment of the run
  is a point of all of it and the along-ray now survives whatever the art said. A
  face sample cannot reproduce the old behaviour in that scene at all any more.
  What replaces it is a fragment the run is **not** part of, standing at the same
  point: it is stopped, which is what says the along-ray above measures the
  exemption rather than an empty frame.
- `Stopper::cell` was the blamed primitive's own low corner, and a merged run's
  low corner names the end of the run rather than the place the ray met it. It is
  the **middle of the crossing** now — and the middle rather than the entry point,
  because an entry lies exactly on the box's own face and a face at a whole
  coordinate floors into the tile next door. That is § *The oracle*'s own defect,
  walked into again on the way to this fix and caught by two tests that name a
  wall by where it stands.

### ✅ The merge under the reference tracer, and what it costs exactly

The blind spot above is closed on the tracer's side, by the fixture the backlog
item itself named — and the fixture found something the ten CPU gates could not,
which is the whole argument for having an arbiter that shares no arithmetic with
us.

**Identity became a field.** `oracle::boxes::BoxSpec` carries its own `graphic`
and `box_owner` reads it, where it used to be the box's place in a list. A list
position can only ever say *everyone is different*, so every scene in
`tests/traced.rs` and `examples/boxes.rs` was a scene in which nothing could merge
under any circumstances. One field, ten literals, and a run of one wall becomes
sayable: `wall_run_scene(&|_| 7)` is three whole-tile boxes of one static, folded
into a single primitive spanning three tiles; `&|x| x` is the same geometry as
three statics, which folds nothing.

**The gate that is not circular.** `the_frame_and_the_path_tracer_agree_about_a_
merged_run_of_wall` hands the frame **one** primitive and the reference **three**
— the same point set, since the merge is a componentwise union of boxes sharing
whole faces — and asks for every interior pixel to come out the same. 261,400
pixels compared, 0 interior disagreements. Under the "union does not grow"
injection it is **60,960**, where before this fixture the whole file was green.
The flame stands off the run's east end so that rays run *along* the length of the
merged box, which is the arrangement a union that grew too far or not far enough
answers differently from three boxes.

**And the GPU twin, which is the other half.** `a_merged_run_of_wall_draws_the_
same_frame_as_its_own_pieces` renders both scenes and compares the two frames
**byte for byte** — no tolerance, because an opaque primitive stops a ray or does
not and each of the eight rays lands the same way, so a partly lit fragment comes
out the same eighth. Both sides are ours, so it is not evidence that the merge is
*right*; what it adds is reach, since it covers the silhouettes, the edges and the
penumbra that `compare` deliberately excludes. The injection moves 64,746 pixels
of it.

🚨 **The merge is not free, and the fixture is what found where.** The module
header says the one thing a merge changes is identity — a fragment of a piece is a
point of the whole run, so it is exempt from the volume its neighbour used to be —
and adds that "after phase 5b there is measurably no ray left that reaches it".
**That is true of the frame a player sees and false of the walk.** With the flame
in the run's own line, half a tile north of the drawn south faces, **5,742 pixels**
of `View::Shadow` differ between a merged run and its pieces. The tracer, holding
three bodies, agrees with the *pieces*: those fragments really are shadowed by the
rest of the run, and the merged frame calls them lit.

Every one of them is a face **turned away from the flame**, and that is what makes
it a cost rather than a defect: phase 3's cosine is nothing on such a face, so the
lit frame is byte-identical. Both halves are now a gate —
`a_merged_run_is_exempt_from_itself_only_where_the_cosine_is_already_nothing`
surveys every moved pixel and fails if one of them faces the light (under the
injection, 117,756 of 123,498 do), then renders the same scene in `View::Lit` and
demands the same bytes. So the claim in the header is pinned in the form it is
actually true in: *a merged run is exempt from itself only where nothing was going
to be lit anyway*.

It also decided where the tracer gate's flame stands. Half a tile **clear of the
south face's plane** — not cosmetic: with the flame north of it the comparison
would be measuring the exemption's reach rather than the merged box's geometry,
and the two gates would be the same gate.

**What the second injection says about the split.** Dropping the graphic from the
merge key leaves the tracer gate **green** — the run of one wall still merges into
the right box, so the picture is right — and turns the twin red on its own
precondition, "the run of three graphics merged, so this twin is not a twin". That
is the correct division of labour: the tracer gates geometry, and the `Owner`
condition is about the *join*, which is `Occlusion::id_of`'s business and is
checked where the join is (the traced gate asserts the three boxes name one
primitive; `render` panics outright if a box cannot find its own solid).

### ✅ The aperture: **the last rule stated in a tile, and it was hiding two defects**

Landed 2026-08-09, after all six steps were green — this document's own last
`floor`, taken out of the one record that still had one. `Aperture`'s `near` and
`far` are world coordinates on the panel's own run axis, `light::run_v` is
`along_the_run` with nothing to recover, and `Occlusion::aperture_bytes` is a
storage buffer of four `f32` a solid.

**What went with it**, so a reader does not have to diff for it: `z_byte`,
`Z_FLOOR` and `Z_CEILING`; `Occlusion::list_rows`, which existed for this plane
alone; `blit.wesl`'s `RUN_STEPS` and `aperture_at`. `occlusion::RUN_STEPS`
survives where it belongs — on the CPU, at build time, as the quantum the *art*
measures a `facing::Hole` in. `Aperture::above` becomes `Aperture::placed`, which
takes the static's base **and** the tile its run starts on, because those are the
two facts about an instance a measurement off a picture is missing.

**The item asked for this to unblock a merge, and it does not.** That is the
finding, and it is the fifth time on this track that a step's decision held while
its stated reason did not. A merge already requires an equal `Owner`; an `Owner`
is a `(z, graphic)`; a hole is read off the *graphic* — `occlusion::shape_of` is a
lookup and nothing else. So two mergeable pieces are windowed together or plain
together, never one of each, and the relaxation that looks available ("merge when
at most one has a hole") **cannot fire**. A wall with one window in it is a wall
of two graphics, and what refuses it is the `Owner`, before and after. The
refusal in `occlusion::merge` stays with its true reason: a primitive carries one
hole, and a run of windows is one per tile.

**What it does buy is two defects, both live and neither in the entry that asked
for the change.**

- **A crossing exactly on a tile boundary floored into the next tile.** A window
  running to the far end of its own tile — `far: 255` off the art — was open up to
  `x = 106.0` and shut *at* it, because `floor(106.0)` is 106 and the fraction
  came back `0.0`, which is the near end of the tile beyond. That is § *The
  oracle*'s own defect, the one that cost a day and convicted two walks that were
  right, arriving one level up in a rule instead of in a lookup.
- **`z_byte` clamped a hole's two ends into the map's own `i8`, and a hole's ends
  are not an `i8`.** `Aperture::placed` adds the art's whole units to the static's
  base, so a window measured 5 to 20 above a wall standing at `z = 120` reaches
  140 — thirteen past anything a byte offset by 128 can name — and the wire shut
  the top of it. The record and both CPU walks read it open, which is why this one
  was invisible to everything but the shader: the quantisation lived in the
  *upload*, and `light::pierced` reads the record on both walks. The backlog entry
  that called this "no defect, a hole is measured in whole units" was right about
  the step and wrong about the range.

*Gates, and each fault-injected in this session:*

- `light::a_windowed_panel_wider_than_a_tile_has_one_window` — a panel spanning
  `x` 105 to 108 with a window in its first tile, asked at five points along the
  run. Injected with the old arithmetic (the crossing's fraction against the
  hole's own tile fraction, which is what a byte was): the far end at `106.0`
  reads **wall** where the geometry says open, and `106.5`, `107.25` and `107.5`
  — three points of solid stone in the second and third tiles — read **open**,
  which is the window repeated in every tile of its own panel.
- `occlusion::a_hole_above_the_map_s_own_ceiling_is_not_clamped_on_the_wire` —
  the bytes read back from their own offsets, on a wall at `i8::MAX - 7`. With
  `z_byte`'s clamp put back it reports `127` where the art says `140`.
- `occlusion::a_hole_is_uploaded_at_its_own_surface_s_index` gains the placement
  as coordinates: an east face's run comes from its tile's `y`, a south face's
  from its `x`, and a corner's two panels carry the same rectangle as two
  different pairs of numbers — which is what one picture says about two
  perpendicular faces.
- **The port is gated by something that already existed**, and it was checked
  rather than assumed: `frame.rs`'s `the_shader_and_light_sample_agree_about_a_
  hole_in_a_wall` moves **187 of 4,096 pixels** both when the shader's `hole` is
  narrowed by half a tile and when it reads `apertures[0]` instead of
  `apertures[id]`. So the buffer's layout and its index are both measured.

The whole crate is green after it — 425 lib tests and every integration suite,
both path-tracer gates and the shader sweep among them — and no fixture in the
tree moved a pixel, because every aperture a `Builder` has ever made is exactly
one tile wide and every one of them sits under `z = 127`.

### ✅ The rename: neither walk has a cell in it

`walk_cells_exact` → **`walk_the_record`**, `walk_cells_streaming` →
**`walk_the_wire`**. What the two differ by is which *boxes* they read —
`Solid::space` against `Solid::wire_box` — and that is now the whole of the
difference, so it is the whole of the name. Test names went with them
(`walk_the_wire_agrees_with_walk_the_record_on_…`).

Three doc comments were left describing a world that had gone: five rustdoc links
to `walk_cells`, retired at point 4's cutover, and `ray_vs_solid`'s own paragraph
arguing from `candidate_tiles` probing a wider set of cells than the streaming
walk visits — an asymmetry S5 removed by giving the two one broad phase. Fixed
where they were wrong and marked where they are history.

**S5 — the hierarchy.** D3. A CPU build over the primitives and a
stackless traversal on both sides.

Pinned so the step has no decisions left in it:

- **Median split on the longest axis**, recursively, to start. Deterministic and
  free of a tuning constant. A surface-area-heuristic split is an optimisation,
  allowed later, gated on the cost harness and forbidden from changing a pixel by
  D4.
- **The build is a pure function of the primitive list and its order.** The tick
  is deterministic and the two backends must agree; a build that depended on
  anything else would make them two trees.
- **Stackless traversal, with an escape index per node.** WGSL has no dynamic
  stack, and a fixed-size array is a cap that would silently truncate — the shape
  `MAX_WALK_STEPS` has today, and the reason it is a *bound* rather than a
  budget. A node's escape index is where a miss continues, so traversal is a
  walk over an array with no stack at all.
- **Leaves hold up to four primitives.** A cost knob under D4, which is why it is
  a number here rather than a question.
- ~~**A node budget replaces `MAX_WALK_STEPS`**, in the same role and for the same
  reason: a loop over data must not become unbounded because somebody widened a
  radius.~~ 🔴 **There is no budget, and the reason is above**: the traversal is
  monotone in the node index, so it is bounded by the tree's own size and a
  number would have been one to defend rather than one to derive.

*Gate:* the brute-force oracle over the whole sweep, on the real place and on
every hand-built scene; a CPU-against-GPU test in the shape of
`a_sprite_pixel_meets_the_same_box_on_both_sides`, since the traversal is a
second spelling with no compiler between the two; and a cost measurement — which
needs `tests/cost.rs` to be able to price a frame **with real occluders**, since
it builds against `Occlusion::EMPTY` today and therefore cannot see this at all.
That harness fix is part of this step, not a follow-up — and it is also
`docs/lighting_rebuild.md`'s own backlog entry asking for "a cost harness that
prices the pass the client actually runs", inherited here rather than left in two
places.

## Not in scope, deliberately

Named so that a later session does not adopt them by accident:

- **A flame's own sprite reading black.** A real defect, found in the same frame,
  and it is about where a light *is* rather than about the shape of an occluder.
  `docs/lighting_rebuild.md`'s backlog owns it.
- **How far a real static's art overhangs its own volume.** Phase 6's own second
  number, still untaken. It is art against volume, not solid against solid.
- **The lateral fit.** `facing::Prism` is `up`, `heights` and `count` — it has no
  term for a cross-axis extent at all, so a fitted climbable is sub-tile along
  its climb and a whole tile across. Worth doing and not this: it changes what one
  primitive's shape is, where this plan changes how many there are.
- **The tile-to-world mapping.** D7.
- **Phases 7 and 8** of `docs/lighting_rebuild.md`, which are billboards and the
  sun and touch none of this.
- **Land as an occluder** — a hill casts no shadow today. A hierarchy over
  arbitrary boxes is the structure that would make terrain an occluder cheap,
  and that is a *reason to expect it later*, not a step here. It stays a carried
  item of `lighting_rebuild.md`.
- **A corner's two panels told apart by the screen half.** They are
  perpendicular, so no merge joins them, and what closes it is a volume carrying
  its instance row — D6's join one level finer. Named because it looks like this
  plan's business and is not.

## Backlog

Findings from this track that do not block a step. Kept here so the plan can be
read as work.

~~🔴 **The two instruments this track leans on hardest cannot see the merge, and
that is measured rather than suspected.**~~ **The tracer's half is closed** — see
§ *The merge under the reference tracer* — and it closed the way the item said it
would: `oracle::boxes::BoxSpec` states a box's `graphic` instead of taking it from
its place in a list, and a run of one graphic is a scene the merge folds. What is
left of the item is the second bullet, which is a different instrument:

- ~~`traced.rs` and `examples/boxes.rs` build their scenes through
  `oracle::boxes::box_owner`, which gives **every box its own graphic**~~ — a
  field now, and `wall_run_scene` is the run of one.
- ✅ `frame.rs`'s shader sweep compares the shader against `light::sample`, and
  both read the same primitives — so it gates the *port* and cannot gate the
  *geometry*, exactly as S5 recorded when it built the sweep. Unchanged: what a
  merged frame is now checked against is the path tracer and the GPU twin below,
  neither of which is the sweep. ~~Whether the sweep should also carry a merged
  scene is a question about *coverage of the port*, and the honest answer may be
  that it should not.~~ **It already carries five, and the question was the wrong
  one** — see § *Neither instrument is unreached* below. **Settled: nothing to
  add here.**
- ✅ ~~`pictures.rs` is untouched by this and still draws nothing that merges.~~
  **It draws six scenes that merge, and its picture already moved under the
  injection** — what did not move was the tile its assertion read. Closed by
  reading the band across the run instead of down one column of it; below, with
  both numbers.

### Neither instrument is unreached — they are *circular* and *unsampled*

Both bullets above said the instruments do not reach the merge. Measured
2026-08-09, and both were wrong in the same direction: the scenes merge heavily,
the instruments walk the folded geometry, and what fails is the last step —
turning what they see into something that can go red. Recorded as found, because
"it never gets there" and "it gets there and has no opinion" are different
repairs.

**What the scenes fold**, printed by `lighting.rs`'s
`the_merge_folds_the_scenes_this_crate_draws` over `scene::all()` — the sweep's
own five and every scene `pictures.rs` draws are in it: a shut room with a torch
24 → 4, a character holding a light in a shut room 24 → 4, a wall with a hole
through its middle tile 9 → 3, the corner of a house 7 → 3, a torch two tiles in
front of a straight wall 9 → 1, a wall run with a lamp along it 4 → 1.

**The injection** is § *The merge*'s own first one — the union does not grow, so a
merged group keeps its first piece's box — and it is live in the same build:
`tests/lighting.rs` goes **12 red** (the ten that step recorded, plus the two the
corner-graze fixture added since).

- **The sweep** — six shader sweeps and seven exact-walk sweeps, **all green**,
  while their own census moves by up to **934 pixels of 4,096**: a room 2,400 →
  1,466 in shadow, a carried beam 2,400 → 1,466, a surface looking up at `z 20`
  2,392 → 1,458, which side a wall is on 1,751 → 1,415, a hole in a wall 1,308 →
  744, a house corner 2,070 → 2,012 — and the room's penumbra count 0 → 75. That
  is circularity with a number on it: the sweep traverses geometry the injection
  has wrecked, *counts* the wreck to a quarter of the frame, and cannot say so,
  because both sides read the same primitives. A merged scene is not what it is
  missing, so adding one buys nothing.
- **`pictures.rs`** — 6 of 6 green, and the picture is **not** identical: in the
  plan view of the torch before a straight wall the row behind the run reads
  `0.063 0.063 0.094 0.111 0.063 0.111 0.094 0.063 0.063` under the injection
  against a flat `0.063` — the ambient's own value — at every column before it.
  The four that move sit **either side of the tile the assertion reads**, and
  that tile does not move at all. So the picture holds the defect and the
  assertion samples the one column of nine that the injection leaves standing.

**The gate is the claim the picture was already for**, read across the run
rather than down one column of it: the band of shadow behind a wall is **as long
as the wall**. Built into
`a_wall_in_front_of_a_torch_darkens_the_ground_behind_it_and_not_beside_it` —
nine columns, each carrying its own control, and the control is what makes it a
band rather than nine repetitions of the reading it already had: a column is
asserted **lit in front and at the ambient behind**, so a column the flame never
reached cannot pass by being dark on both sides.

*Injection, run in the same session:* the union that does not grow turns it red
at **column 98, reading `0.094` against the ambient's `0.063`** — where the whole
file was green under that injection before. A shape claim on the instrument that
is for shapes, and the first thing in `pictures.rs` that can fail on the merge.

~~🔧 **The merge's own indices are bare `u32`s and its axis is a bare
`usize`.**~~ Fixed in `occlusion::merge`: a private `Prim` newtype now carries
the frame's-primitive-list index and the union-find group name (one type on
purpose — every group name *is* a `Prim`, the one `root` resolves a lookup
to), and the third space — the merged *output* list — was already
`SolidId`; only `named: Vec<Option<u32>>` had been storing it unwrapped, and
now holds `Option<SolidId>` directly. The `0`/`1` axis selector is a
two-variant `Axis` enum, with no third variant to handle — see the module's
own "Why only the horizontal axes". Both types are private; nothing outside
the module sees them, so the fix stayed contained.

The same sweep taken wider, one item at a time: `bvh.rs`'s split axis (`0`/
`1`/`2`) is now a private three-variant `Axis` too, same reasoning, same
containment. `Builder`'s per-tile linked list (`occlusion.rs`) had the same
shape: `heads`/`arena` carried a place in the arena as a bare `u32` sentinel
`NO_SOLID`, sitting beside two *other* bare-`u32` domains in the same two
functions — a tile slot in `heads`/`sky` and a place in the output `solids`
list `finish` packs — with nothing to stop one being passed where another was
meant. A private `Link` newtype (mirroring `bvh::NodeIdx`) now covers it, and
`bake.rs`'s own read of the same arena — the one place outside `occlusion.rs`
that ever touched it — takes the type instead of the raw value.

One thing stayed open on purpose, surveyed and set aside rather than missed:

- `Solid::footprint`'s `i32` ranges. Closing that one properly means a real
  tile-coordinate type, and that type's call sites reach into `bake.rs`'s
  whole coordinate system (`origin`, `tile_of`, `spill_of`, block/cell
  indices), which is D7's ground, named "not in scope, deliberately" above.

~~The `edges: u8` bitmask (`EDGE_NORTH`/`EAST`/`SOUTH`/`WEST`/`EDGE_ANY`) on
`Solid`, `Cell` and `merge::Surface` — shared across four files and mirrored in
`blit.wesl`'s own wire layout, unlike the private, single-module fixes above,
so flagged for a pass of its own.~~ Fixed: a private `Edges` newtype (`Solid`,
`Cell`, `light::Stopper::edges`, `merge::Surface::edges`), the same shape as
`bvh::NodeIdx` — a private `u8` field, associated consts `NORTH`/`EAST`/
`SOUTH`/`WEST`/`ANY`/`NONE`, `contains`/`union` methods, `BitOr`, and `raw()`
as the one door out to the wire byte `primitive_bytes` packs and to
`blit.wesl`'s own mirror of the same four bits — which stays untouched: WGSL
has no type system for this side of the wire to carry. Landed across every
file the survey named plus three more the survey's own grep missed
(`crates/client/app/src/shell.rs`'s wireframe, and the two artscan examples,
`grid.rs`/`column.rs`) — the full set only surfaced by compiling after each
site, `cargo check --workspace --all-targets` catching what a text search
of one crate could not. `cargo check/clippy/fmt --workspace` and
`cargo test -p openshard-client-render` (all 417 lib tests plus every
integration suite) are green.

~~🔴 **Nothing in the crate gates a corner-grazing candidate, and that is
measured rather than suspected.**~~ ✅ **Closed 2026-08-09, on both sides.**
`blit.wesl`'s `walk` carried an *unconditional* diagonal probe — a second
`cell_stopped` on the neighbour the step does not take — put there so a ray that
grazes a corner without entering the cell's interior still meets what stands on
it. Replacing its result with `0.0` left **the entire crate green**: 526 tests,
both path-tracer gates, the shader sweep, all of it. The grid's own trajectory
never needed it on any scene the suite drew, so the port deleted the probe with
the grid — a tree is hit or missed by one slab test and has no tie to break —
and that made the deletion *unmeasurable* rather than measured. The one geometry
a hierarchy could plausibly mishandle was the one nothing would catch.

**What closes it is the fixture this entry asked for, built so that the corner is
a corner of the *tree* as well.** Eight whole-tile bodies down the diagonal: the
median split on the longest axis cuts the run in half, and the two leaf boxes
meet at exactly one point — which is also the shared corner of the two
primitives either side of it. Both facts are asserted rather than assumed, since
a run short enough to sit under one leaf would leave the fixture asking about
the narrow phase alone.

- **The two CPU walks**: `lighting.rs`'s `a_segment_through_the_corner_two_
  leaves_meet_at_finds_what_stands_there`. A ladder of offsets slides the
  anti-diagonal across that corner, in **powers of two** so every endpoint is
  exact in `f32` and the rungs measure the geometry rather than the rounding of
  their own coordinates; it bottoms out at `2^-17`, one ulp at `106`, with the
  shift asserted so a tighter rung cannot quietly become the one above it. The
  arbiter is `segment_inside_box` and **not** `brute_force_blocked` — § *The
  oracle*'s own rule, and the thin end here is four hundred times thinner than
  `BRUTE_STEP` — with the sampler asked anyway as a third voice wherever it can
  resolve the clip. The same ladder runs at a height inside the run and over it,
  so the walks have to say *open* as well as *stopped*: 52 walked rays, half of
  each.
- **The shader**: `frame.rs`'s `the_shader_meets_what_stands_at_the_corner_two_
  leaves_meet_at`, which is the side the probe was deleted from. One pixel, and
  everything about it exact: the fragment is a tile's own top-left corner and the
  flame stands on the anti-diagonal through the split, so the whole segment lies
  in the plane `x + y = cx + cy` — the one plane that touches those two boxes
  along a single vertical edge and misses the other six outright. So the only
  thing between that fragment and its flame is a graze of exactly zero length,
  and the control takes both bodies away. Either one alone still stops the ray,
  which is its own reading: the two grazes are one point rather than two chances
  at a thicker crossing.

*Fault injection, three, and the third is the number this entry was waiting
for:*

- *The node test made strict* (a zero-length node graze dropped) on the CPU: **2
  of 52** rays red, both of them the exact-corner rung. Across the whole crate
  the only other test that notices is `a_vertical_ray_meets_what_stands_over_it_
  whatever_shape_it_is`, and it notices because a **lid** is a box flat in `z` —
  a degenerate box, not a corner. So the corner case really was ungated.
- *A tolerance in the node test* (`1e-4` of the segment): **10 of 52**, the exact
  rung and the two thinnest either side of it. The `2^-12` rung survives, its
  crossing being `1.2e-4` of the segment — which is the ladder reporting where a
  tolerance stops mattering rather than a pass. The two fuzzers and the pinned
  corner graze go red at this width as well.
- *The same injection in `blit.wesl`'s own traversal*: the new GPU fixture is
  **the only thing in the crate that goes red** — the other 51 tests of
  `frame.rs`, all 6 of `pictures.rs` and all 8 of `traced.rs` stay green, the
  shader sweep and both path-tracer gates among them.

🔧 **And the two spellings of the slab test are not the same test at exactly this
geometry**, which is worth knowing now that something gates it. `blit.wesl`'s
`ray_vs_solid` rejects at `entered > leaves + RAY_TANGENT_TOLERANCE` where
`light::ray_vs_solid` rejects at `entered > leaves` outright — the shader's own
comment has the story, and it is a deliberate one-sided widening. So at a corner
graze the shader is **more** generous than the CPU by construction: it cannot
lose a candidate the CPU keeps, which is the safe direction and why the two gates
above pass together. What it does mean is that a *future* narrowing of either
side is a change to a comparison the other does not make, and the pair of
fixtures above is where that would show. Not a defect and not scheduled — written
down because "the two are one traversal" is true of the tree and not quite true
of the box test under it.

🚩 **The merge inherits the seam, and what it inherits is a sphere's own half.**
S3 cures a surface shadowing itself for every ray *leaving* that surface, which is
what its theorem licenses. What it cannot touch: a flame is a sphere, so a lamp
standing level with a wall — or in a landing's own plane — puts half of its eight
rays on the far side of that plane, and those rays genuinely cross the neighbouring
primitive of the same surface. The reference tracer, handed the same primitives,
agrees that they do. `same_run` papers over exactly this for a panel run by exempting
the neighbour whatever the ray's direction, and there is no equivalent for a body.

⚠ **And the merge may not be what answers it.** Those below-plane rays are only
traced at all because the shading takes its cosine from the flame's *centre* while
visibility is sampled over the flame's whole sphere: a sample point below the
fragment's horizon should contribute zero by `N·L` and never be asked about
occlusion. Fix that, and the set of rays a join can block is empty — no merge
required, and `same_run` loses its reason too. Prototyped and rendered on
2026-08-09; it lives in `docs/lighting_rebuild.md`'s backlog, since it is a shading
question rather than a geometry one. **Measure that before spending S3b on this**,
because the merge's own argument then falls back to what it always was — one
primitive per surface is cheaper and simpler, not a cure.

**Decided 2026-08-09, and it reverses the paragraph that used to stand here.** That
paragraph read "only the merge answers it, and it answers it completely", and it was
written before the shading side was measured. Three things say otherwise, and each is
enough on its own:

- **The merge does not reach the fixture the wedge was measured on.** S3b merges
  primitives that share a whole face *and have an equal span*, which is why this plan
  already writes down that a flight's treads do not merge. The wedge was measured at
  the joins of three flights that are geometrically one landing. They stay separate
  primitives, so the join stays, so the wedge stays.
- **Even where it merges, it cures a neighbour and not a set of rays.** A ray below
  the horizon can end on anything — a wall's base, the step below, a body. The merge
  removes the neighbour *of the same surface*; only `max(N · L, 0)` removes the set,
  because the set is "everything behind the plane".
- **The defect is not only seams.** The prototype moved 21,177 pixels and darkened
  20,308 of them: the centre cosine over-pays every grazed surface, join or no join.
  No step of this plan may touch that — D4 is "not one pixel moves". **A step
  forbidden to move a pixel cannot fix a defect whose symptom is moved pixels.**

  ⚠ **The sign in that sentence is wrong, and the conclusion survives it.** Phase
  5b landed and moved 163,492 pixels of its own gate's fixture — 162,921 of them
  **brighter**, 571 darker. There is nothing for a centre cosine to over-pay: an
  average of `max(N·L, 0)` over a body is never *less* than the centre's own
  cosine, because the clamp is convex. What it under-pays is exactly the wedge —
  darkness, at the joins. The argument above is unchanged either way: a step that
  may not move a pixel cannot fix a defect measured in moved pixels, and the count
  is now eight times larger than the one quoted.

So: `docs/lighting_rebuild.md`'s **phase 5b is the cure**, S3b is an optimisation —
one primitive per surface is cheaper and simpler — and it keeps its place last,
after S5, with its own gate unchanged: not one pixel moves.

✅ **The pinned corner graze: the walks were right and the oracle was wrong —
closed 2026-08-09.** `lighting.proptest-regressions`' newest line, found by a
fresh seed, red for a day. Nothing in the session that found it touched
`crates/*/src`; the case was always there and no seed had reached it.

```
spot  (104.6041, 100.9463,  2.00) tile (104, 100)
light ( 93.1834, 101.0253, 13.69) tile ( 93, 101)
walk_cells says blocked, the brute-force oracle says open
```

One whole-tile body at `(100, 100)`, `z` 0..20. The segment crosses the wall's
column while its `y` runs 100.971 → 100.978 — **three hundredths of a tile from
the corner at `(101, 101)`**, which is the region the sibling grid test excludes
by construction and this fuzzer aims at on purpose.

**Settled by the exact test rather than by either disputant**, which is what the
open question asked for: `segment_inside_box` over the eight flame points says all
eight enter the wall's box, so **blocked is the truth** and both walks had it.

What the sampler got wrong, in one line of numbers:

```
ray 5: enters at t 0.315466, leaves at 0.315485 — 0.000225 tiles of wall,
       and over that whole clip y runs 100.999997 → 101.000000
step 18023: point (100.9999084, 101.0000000, 5.52059) → tile (100, 101)
            inside the box on every axis, and that cell lists 0 solids
```

The clip's entire `y` extent is three millionths of a tile below `y = 101`, and no
`f32` exists in that gap — the ulp at 101 is `7.6e-6` — so the sampled point's `y`
is *exactly* `101.0`. `floor()` sends it to cell `(100, 101)`, which is empty, and
the oracle reported open ground from inside a wall.

So the step was never the culprit: `0.000225` is **deeper** than `BRUTE_STEP`, and
the march did land a point in there. Both walks failing identically was the clue
read backwards — they agreed because they were *right*, and the thing the two of
them share is not a DDA bug but a correct answer. The fix is § *The oracle*'s "no
cells" rule: both samplers iterate `Occlusion::solids()` and state their exemptions
as closed volumes, so neither has a `floor()` in it any more. The seed stays pinned
and passes, `the_pinned_corner_graze_is_blocked_and_all_three_oracles_say_so` pins
the verdict itself, and putting the cell lookup back turns exactly that assertion
red while the walks' two stay green.

**This also disarms half of the merge hazard below**: `brute_force_blocked` was
named there as "cell-based too, and would agree with the defect". It is not
cell-based any more, so a merged primitive wider than its registration cell is
now caught by the oracle rather than blessed by it. The two *walks* are still
cell-based, which is the rest of that entry and S3b's own problem.

**`frame.rs`'s `ground_truth_blocked` took the same repair with no coverage to
prove it.** It is only ever called on a `walk_cells`/`walk_cells_exact`
disagreement, and the sweep over all seven scenes reports `0 explained, 0
unexplained, 0 grazed` — the two walks no longer disagree anywhere, so the
arbiter is a standby that never runs. Its correctness rests on its twin in
`lighting.rs`, which the fuzzers do exercise. Worth knowing before trusting it as
a gate: it is not one today.

**A gate whose fixture puts the flame in the wrong place passes under injection.**
The landing gate S3 built passed *with the exemption neutralised* while its flame
stood above the landing rather than in its plane: a ray leaving a lid upward touches
the neighbouring piece only at `t = 0`, which the zero-length touch rule already
answers, so the fixture never reached the rule it was written for. It was caught by
running the injection, which is the only thing that can catch it — a green gate and a
vacuous gate are the same output. Worth stating as a habit rather than as an
incident: **every new gate on this track gets its injection run in the same
session**, and the flame's position relative to a surface's own plane is the
parameter that decides which rule a fixture is even asking about.

**A cell lists a primitive once, and D1 has just made that a hole S3b will fall
into.** `Builder::push` puts a solid in exactly the cell it was added on;
`Solid::footprint` — which answers *which tiles a box touches* and whose own doc
says "the day a box is wider, this is where the extra tiles come from" — has one
caller, and it is `bake`'s. Nothing before S1 could build a box reaching past its
own tile, so nothing noticed. **S3b's merge is exactly the thing that builds
one**, and the moment it does, the grid stops being a superset: a ray that
crosses the overhang without ever entering the registration cell is answered
"open" by both walks. `tests/lighting.rs`'s `brute_force_blocked` was cell-based
too and would have agreed with the defect; since the corner graze above it iterates
every primitive, so the oracle now **catches** this instead of blessing it — the
difference between a step that fails loudly and one that merges wrong geometry
quietly. This is D3's own argument
arriving early, and S3b has to answer it before it merges anything — either by
listing a merged primitive in every cell it spans, or by taking the hierarchy
first. It is why S1's own gate keeps its fixture inside one tile: the wire is
what that step is about, and a straddling box would have failed it for a reason
S1 does not own.

**And listing one primitive from two cells double-counts it.** The walk
multiplies `1 - stopped` cell after cell, and the per-cell `max` groups only
*within* a cell — so a solid a ray meets on two of its cells is applied twice.
Opaque either way, wrong for anything translucent (a pane, a `PANE` opacity of
51). Whichever way the item above is answered, this is the second half of it, and
D5's deletion of the per-cell `max` is where it lands.

⇒ ✅ **Both halves are closed on both sides, by S5 and neither by a rule**: a
primitive is under exactly one leaf whatever its size, so it is offered once and
its own tile has nothing to do with it. The shader's port closed the same two on
its side, which is what met S3b's precondition — and S3b has since built the first
primitive wider than its own tile without either half being reachable.

~~**The apertures are the last texture indexed by a `SolidId`.**~~ ✅ **Closed at
S6**, with the run coordinate and in the same record: the holes are a storage
buffer of four `f32`, `Occlusion::list_rows` is deleted and no plane is indexed by
a `SolidId` any more. It did not go "with the reference list in S5" as this entry
guessed — the references are indexed by a *reference* and are still an
`Rgba8Uint` folded into `LIST_ROW` rows, which is the one texture of this kind
left. What moved the aperture was not tidiness but the run coordinate needing a
`f32`, which is the same reason D1 moved the box at S1.

**Grey in a dumped mask meant three different things, and that cost a session —
fixed.** The two mask strips drew `None` as one grey level, and `None` is
"nothing drawn here", "the two disagree which surface is there, on a silhouette"
and "…and not on one" — the last of which is the only one that is a defect. A
field of grey slabs across the run of flights was carried into a second session
as evidence of a lighting fault, while the counts printed beside it already said
`0 with nothing drawn` and `0 not on a silhouette`. What settled it in one pass
was **laying the grey over the lit render**: the slabs fell exactly on the risers
below the tread in front — the paint-order defect this scene's own doc records,
not a walk defect.

So the dump has a **fourth strip** now, `Verdict::strips`'s own: black compared,
grey nothing drawn, teal a silhouette, **red the one that is a defect**. Built
where the judging is, so the tool and the gate draw one rule rather than two —
they had a copy of the three-strip code each, identical to the line. Checked by
injection rather than believed: putting the flights back in climb order paints
7,016 red pixels in exactly the shape that was argued about, with 2,179 teal
around them, and the histogram matches the printed counts term for term.

**A dumped picture carries no mark of the code that made it.** The same grey
slabs survived as evidence because the file predated the fix and nothing about it
said so — the name even carried `_fixed`, meaning a corrected *crop*, not a
corrected scene. A dump is the instrument this track is steered by, so it should
stamp what it is: the verdict's own counts, at least, written beside the pixels
they describe.

**A composite of strips cannot be cut by dividing its width.**
`png::write_strips` puts a `RULE_WIDTH` ruler between strips, so a three-strip
image of 512-pixel panels is 1540 wide and `w / 3` is 513. Cutting that way
shifts every strip after the first by a pixel per ruler, and a one-pixel shift
between two renders reads exactly like a camera that is off. It was reported as
"a systematic 3 px offset" once; there is no offset. Slice at
`k * (SIDE + RULE)`.

**A gate can be vacuous three times over, and each time for a different
reason.** S5's tie-break gate — that primitives sharing a centre are leafed in
their own name order — took three versions to fire. First it asserted that two
builds of one list are equal, which cannot fail because the build takes nothing
but the list. Then it put forty primitives at one place, and an unstable sort
handed nothing but equal keys leaves them exactly as it found them. Then it split
on the wrong axis: a ten-tall body's longest axis is `z`, where all forty sat at
one height, so the sort never read the axis the ties were on — and it failed, for
that reason, which would have read as the tie-break working. What makes it a gate
is ties *inside* a list the sort genuinely partitions.

Worth stating as a habit beside the flame-position one above: **an injection that
fails is not yet a gate that fires — read *why* it failed.** Three of these four
readings were red, and only the last one was red about the rule.

~~🔴 **An aperture is the last rule in the pass still stated in a tile**~~ and
~~**a hole's own `z` is still quantised**~~ — ✅ **both closed at S6, and both of
them were carrying a defect the entries denied.** The reasoning that stood here
is kept in § *The aperture*, beside what measuring it actually found: the run
coordinate cost a boundary crossing rather than only a merge, the `z` byte was
*not* the harmless quantisation this entry called it, and the merge it was
supposed to unblock is refused by a different field entirely.

✅ **A second fresh-seed disagreement, of exactly the family the pinned corner
graze was — found 2026-08-22, and closed the same day. The walks were right
again; the sampler was not wrong this time, it was blind.** The fuzzer went red
on a session that touched neither `light.rs` nor `lighting.rs`, in one run of
`cargo test -p openshard-client-render`, and both of its tests failed on the
same input:

```
spot  (103.3108, 100.1455, 6.49) tile (103, 100)
light ( 96.7425,  99.8349, 8.91) tile ( 96,  99)
walk_cells says blocked, the brute-force oracle says open
walk_the_record says blocked, the brute-force oracle says open
```

```
cc 9bdadf636c3cb9da1dd1e37405359d2436cfd56f3795e50d56fbb43ffef58263 # shrinks to spot_dx = 2.3108275, spot_frac = 0.14551114, spot_z = 6.487659, flame_dx = 2.2574825, flame_z = 8.913442, row = 100.0, frac = -0.16507894
```

**What it was.** `segment_inside_box` over the eight flame points, in one run and
against neither disputant: all eight enter the wall's box, so blocked is the
truth and both walks had it — the same verdict as 2026-08-09 and for the same
reason, that both walks agreeing is a clue about the *oracle*. What differs is
the sampler's excuse. Last time it had sampled the sliver and then misindexed it,
which was a defect and was fixed. This time the thinnest of the eight spends
`0.0000282` of a tile inside the body — **seven times under `BRUTE_STEP`**, which
had already been tightened twice for exactly this — and a fixed-step march that
steps over that is not wrong about anything, it simply cannot see it.

**So the number stops moving, and the oracles change places.** Tightening the
step a third time is a treadmill by construction: any step is defeated by a thin
enough clip, each round costs a proportional pile of point tests (33,000 per ray
at the current step), and both previous rounds were paid for with a red suite.
`tests/lighting.rs` now carries `deepest_crossing` — `segment_inside_box` over
every solid in the frame, with the walk's own two exempt tiles subtracted as
volumes — and the four comparisons (two grid sweeps, two fuzz tests) hold the
walks to **that**, exactly. It cannot be stepped over.

`brute_force_blocked` stays, demoted from the property to a **control on the
exact test**, because its dumbness is still worth something no slab test can
give: a point it lands inside a box really is inside one, whatever
`segment_inside_box` and `ray_vs_solid` might come to believe together. `Oracles`
holds both and states the carve-out in one direction only — the sampler calling
*open* where the exact test says blocked is excused when the crossing is thinner
than its own step, and the sampler calling *blocked* where the exact test says
open is never excused, since that would be the exact test's defect. Both
directions are gated: blinding `deepest_crossing` to clips under `0.01` turns the
fuzz red through the second rule, and dropping the carve-out turns it red through
the first, on this very seed.

The seed is **pinned** in `lighting.proptest-regressions`, green, beside its
2026-08-09 sibling, and
`the_second_corner_graze_is_blocked_and_the_sampler_is_the_blind_one` is its
fixture — asserting the mirror image of what the older one asserts: there the
sliver was *over* the step and the sampler's miss was a defect, here it is under
it and the miss is arithmetic. Between the two, both directions of the sampler's
error are held down by fixtures rather than by whichever one a random seed
reaches next.

Worth naming as a property rather than as an incident: **this suite can go red
on a run that changed nothing.** The fuzzers draw a fresh seed each run, so a
green workspace is evidence about the seeds that ran, and a red one is not
automatically about the diff in hand. The first question on a red fuzzer is
which file the failing case is even in.
