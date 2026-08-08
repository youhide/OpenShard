# Height as a continuous quantity

> **Consolidated into [`lighting_rebuild.md`](lighting_rebuild.md)** — the height track, whose backlog is mostly deleted rather than fixed.
> That document is the entry point: it lists what is still live here, which rebuild phase retires or inherits it, and what carries over untouched. This file stays as the record of how it was built and why.


A fragment's height, and an occluder's, were integers when this was written —
phases 1 and 2 below are what changed that, and `## Status` is where it
stands. Everything a
shadow decides — where a ray starts, which box it enters, whether a solid is
the fragment's own — is decided from those integers. On a floor or a lid
that is exact, because a lid *is* at an integer `z`. On anything standing
up it is a lie: height varies continuously down a wall's face, and rounding
it to the nearest unit turns one surface into a staircase of one-unit
treads, each lit as though it were a whole unit higher or lower than it is.

This plan makes height continuous end to end, and then removes the one
place that only ever needed integers to paper over: `exemption`'s guess at
which solid a fragment belongs to.

**All four phases have landed.** Height is continuous on both sides of the
wire; a fragment *says* which occluder it is a point of instead of having that
guessed from where it is (phase 3); and it is excused from that occluder's
surfaces only where it is genuinely a point of one of them rather than merely
sharing an owner with it (phase 4). What is left is `## Backlog`.

## The defect this comes from

`examples/boxes.rs`'s `tree` scene draws a closed dark patch **inside** the
lower box's own south face — below the joint, below the top edge, lit above
it and lit below it. It is not the upper box's shadow: a shadow cast by
something standing on top of a face must touch that face's top edge.

Three facts, each verified against the tree rather than argued:

1. `pack_place` (`shaders/place_format.wesl`) wrote
   `round(raw_z)` — eight bits, one unit a step; phase 1 below is what
   replaced it, and the three facts here are as they stood before it.
   `mesh_face.wesl:100`
   hands it `in.world.z`, which is interpolated down a face and genuinely
   continuous; `statics.wesl` does the same for a wall's sprite. So every
   fragment of a vertical face reports one of four heights on a box three
   units tall, and `View::Height` shows exactly that: bands one unit apart.
2. Where the rounded height lands on a neighbouring solid's own base,
   `on_surface` (`blit.wesl:467`, `light.rs`'s twin) reads that solid as
   the fragment's own surface, and `exemption` (`light.rs:1247`,
   `blit.wesl:898`) drops it from the walk entirely. That is the **lit band
   under the top edge** — the upper box exempted from shadowing the face it
   stands on, because the face's top row of fragments rounded to the upper
   box's own floor.
3. One unit lower the exemption no longer fires, and the ray now starts a
   half unit below where the fragment really is, which is enough to send it
   into the upper box's `z` span instead of under it. That is the **dark
   band**, and the lit band below it is where a ray starting that low
   genuinely does pass beneath.

The control: `OPENSHARD_TREE_H1=3.5` moves the joint off an integer, and
the whole face comes back clean.

**What this is not.** The GPU is not blind to a sub-tile footprint — that
was fixed already, `Occlusion::footprint_bytes` (`occlusion.rs:1421`) and
`box_of` (`blit.wesl:722`) carry the box's horizontal extent to a
hundred-and-twenty-eighth of a tile. Horizontal geometry is exact and
vertical geometry is not, which is the whole of the asymmetry.

## Phase 0 — a gate on the old layer, before anything moves

`examples/boxes.rs` already runs two independent oracles, and **neither can
see this class**: the box-top oracle samples tops only (a top is flat, so
its height is an integer and rounding is exact there), and the ground
oracle samples the ground (likewise). The bug lives precisely where nobody
looks — on the vertical faces.

- A **face oracle** in `boxes.rs`: a grid over each box's own four vertical
  faces, each point projected through the scene's camera to the pixel the
  renderer actually drew, compared against `segment_clear_of_box` — the
  same fresh slab test the other two oracles already trust, no arithmetic
  shared with `light.rs` or `blit.wesl`.
- It **counts what it checked**, not only what disagreed. A face point that
  projects to a pixel some other face owns is skipped, and a skip is not a
  pass: the printed line carries sampled/compared/disagreeing, and the
  comparison count is asserted non-trivial. A detector that silently
  compares nothing reads exactly like a detector that found nothing.

This must go in first and must be **red** on `tree` before any of the
phases below start. It is what says a phase worked, and it is the only
thing that will catch this class coming back.

## Phase 1 — the fragment's height

Four spare bits sit in the `place` attachment's third channel: it is a
`u16` holding `z + 128` in the low eight and a four-bit stance at
`PLACE_STANCE_SHIFT = 8`, leaving bits 12..15 unused.

- Third channel becomes `z + 128` (8 bits) · **fraction (4 bits)** · stance
  (4 bits): `PLACE_STANCE_SHIFT` moves 8 → 12, and a new
  `PLACE_Z_FRAC_SHIFT = 8` / `PLACE_Z_FRAC_MASK = 15` names the middle.
  Sixteenths of a `z` unit — a `z` unit is four screen pixels at zoom 1, so
  a sixteenth is a quarter pixel, well under anything visible, and no wider
  format or second attachment is needed.
- `pack_place` splits instead of rounding: `floor` into the integer field,
  the remainder into the fraction. The existing clamp is unchanged.
- `blit.wesl:1454` reassembles `at.z` from both fields. `place.rs`'s
  mirror constants and its round-trip test move with them.
- The three producers (`ground.wesl`, `statics.wesl`, `mesh_face.wesl`) do
  not change: each already hands `pack_place` a continuous `f32`, and the
  rounding it applied was never theirs.

Done when: `View::Height` no longer bands on a vertical face, and the face
oracle's disagreement count drops to whatever the penumbra's soft edge
accounts for. *(Landed. The first half held; the second was the wrong
criterion — what is left is a hard disagreement at the foot of every face, not
a soft edge. See `## Status`.)*

**Why four bits and not eight.** Eight would need the stance moved out of
the channel entirely, into the id channels — real work, for precision
below a quarter of a pixel. If phase 3 lands, nothing compares heights for
*identity* any more, and a quarter pixel is comfortably enough for
geometry. Revisit only if phase 3 is abandoned.

## Phase 2 — the occluder's height

`Solid::bottom`/`top` (`occlusion.rs:601`) `round()` to `i32`, and
`solid_bytes` (`occlusion.rs:1341`) ships those two integers as bytes. For
a static off `tiledata` that is exact — its height is a `u8` and its `z` an
`i8`. For everything else it is not: `Builder::add_raw`'s arbitrary AABB,
a mesh face, a slope, a tread.

- A **`solid_z` plane** beside `footprints`: one `Rgba8Uint` texel a solid,
  the fractional parts of `bottom` and `top`, indexed and folded exactly as
  `footprint_bytes` already is. Same trick, same format, same WebGL2
  ceiling — the integer bytes stay where they are and keep meaning what
  they mean, so nothing that reads only them breaks.
- `box_of` (`blit.wesl:722`) takes the fraction into `lo.z`/`hi.z`, the way
  it already takes the footprint into `lo.x`/`hi.x`.
- On the CPU, `ray_vs_solid` already reads `solid.space` — exact `f64`
  corners. Audit the walk for every remaining `bottom()`/`top()` call and
  route each to the exact span; the integer accessors survive only for the
  upload and for the merged-column grid, and their doc comments say so.

Done when: `tree` at an integer joint and `tree` at `H1=3.5` agree with the
face oracle to the same tolerance, instead of one being clean by luck.
*(Landed — 278 and 235, the same shape on both. See `## Status`.)*

## Phase 3 — identity instead of coincidence

`exemption` (`light.rs:1247`) answers "is this solid the fragment's own"
with a **guess**: does the fragment's height fall inside the solid's span
(`on_surface`), and does the solid's edge mask miss the fragment's own side.
Both are proxies. The lower box's top and the upper box's base are the same
plane, in the same cell, and no amount of precision separates them by
height alone — the ambiguity is structural, and phases 1 and 2 shrink it
without removing it. The comment at `light.rs:1261` already says as much,
in the case where it fires for `Surface::Flat`.

A fragment knows exactly which solid it belongs to. It should say so.

### The fixture, first — `tree` cannot show this

`examples/boxes.rs`'s `tree` stacks its two boxes, so their `z` spans meet at a
single plane and a fragment of one is inside the other's span for exactly one
quantum of height. Once the oracles stopped lying (see `## Status`), `tree`
reads 18 of 7008 and every one of those is `STAND_OFF`'s nudge: **this phase
has no number to move there**. A phase measured against a scene that cannot
show its defect is a phase that will read green whatever it does.

So `OPENSHARD_BOXES_SCENE=pair` was built to show it, and does — two boxes of
one height side by side on one tile, on the tile's own diagonal so neither
covers the other on screen, the flame on the line through both centres and
beyond the near one. Every fragment of either box is inside *both* spans, which
is precisely what `exemption` reads as ownership, so the near box is exempted
from shadowing the far box's face while standing squarely in front of it.
Three oracles are red at once and all of them "both walks together" — no
precision or parity work can reach any of it:

| oracle, `pair` | before phase 3 |
|---|---|
| box 0's `east` face | 1296 / 1296 pixels |
| box 0's `south` face | 1248 / 1248 |
| box 0's own top | 9216 / 9216 — the `caps_this` arm, same guess |
| box 1 (the near one) | 0, correctly |
| ground | 147 / 254248 — the same nudge/tangent floor `tree` has |

### The design, decided

The three questions this section used to leave open are answered here, because
each of them decides a format and none of them can be deferred to the typing.

**1. Identity is the *thing that was added*, not the solid.** One
`Builder::add` — one static — is one owner, and every solid it pushes (a
corner's two panels, a stair's tread tops and risers, a body) carries that one
owner. That is what makes "one static, several solids" a non-question: the run
*is* the owner, and `own_run`'s bookkeeping has nothing left to approximate
within a tile.

**2. The key is the world thing, not a walk order.** An owner is
`(tile, the static's own z, its graphic)`. Not a counter the builder hands out:
`occlusion::bake` builds a *block's* solids once and pastes them into frames
for as long as the atlas revision holds, so any number that depended on the
order a frame's walk found things in would be a number from another frame. Not
a "the n-th static of this tile" index either, tempting as it is at 8 bits —
the two walks that would have to agree on it refuse different statics (the
occlusion side drops `opacity == CLEAR` and anything above the draw ceiling,
the draw side drops what the atlas has no art for), so the indices diverge
exactly where a tile holds something invisible.

**3. What rides on the wire is *which occluder of this cell*, one byte.** The
comparison is only ever made between a fragment and a solid **on the fragment's
own cell** — `lit_end` and `caps_this` are both `own_cell`-gated — so the id
does not have to be unique in the frame, only in the tile, and a tile holds at
most `MAX_SOLIDS_PER_CELL` = 255 of anything. `Occlusion::id_bytes` already
uploads a `SolidId` as three bytes of four, so the fourth byte of a *reference*
is where this goes: no new plane, no format wider than it is, and the value is
read in the loop that is already reading that texel.

Which leaves the join, and it is the one real cost: the pass that draws a
static has to learn the number the grid gave it. `Occlusion::owner_at(tile, z,
graphic) -> Option<u8>` answers it by scanning the cell (four solids, not four
hundred), and `statics::collect`/`items::collect` stamp the answer into the
instance row beside the tile. That means **the frame's occlusion has to be
built before its statics are collected**, which is a reordering in
`app::render` and not a change to either pass's logic — today the statics go
first for no reason anyone recorded.

- `Solid` carries its owner key on the CPU (three bytes, never uploaded) and
  its per-cell number for the upload. `Builder::add_raw` takes the key from
  its caller — a hand-built scene has no `tiledata` to derive one from, and
  inventing one inside the builder would be a second identity.
- `MeshFaceRow` and the statics pass's row gain the byte. A fragment with no
  solid at all — the ground, a mobile — stamps `OWNER_NONE`, which matches
  nothing.
- `exemption` becomes `stands.owner == fragment.owner`. `on_surface` keeps
  only its geometric role (does this ray's `z` lie in this solid's span, for
  `pierces` and the lid rules); `own_run`'s heuristic and `ON_TOP`-as-identity
  go away. `STAND_OFF` stays — it is about where a ray starts, not who owns
  what.

**Two things the design does not answer, deliberately.**

- **`flame_end`.** The other end of the ray is a flame, not a fragment, so it
  has no owner to compare: the arm that exempts the solid the flame is mounted
  on stays a height test for now. It is `mounted_at`'s question rather than
  this phase's, and worth its own entry once the fragment side is identity.
- **A run of wall across tiles.** `own_run` also answers a *second* question —
  a ray leaving a wall pixel along the wall grazes the neighbouring tiles'
  panels of the same wall, which are different statics and therefore different
  owners. That is not identity, it is a surface being cut on a tile boundary,
  and it stays until something measures it. The `pair` fixture cannot see it
  (one tile), so a scene that can has to come with the change that touches it.

Done when: `pair` reads zero on all three of its red oracles, `tree` still
reads 18/7008 and 226/252105, `tests/lighting.rs` and `tests/frame.rs`'s parity
suite are green, and no exemption decision about a *fragment* reads a height.

### Landed, and what it measured

Every line of that bar is met, and the numbers are one measurement apart from
the ones above — same tool, same defaults, same instrument:

| oracle | before | after |
|---|---|---|
| `pair`, box 0's `east` face | 1296 / 1296 | **0 / 1296** |
| `pair`, box 0's `south` face | 1248 / 1248 | **0 / 1248** |
| `pair`, box 0's own top | 9216 / 9216 | **0 / 9216** |
| `pair`, ground | 147 / 254248 | 147 — unmoved, the tangent floor |
| `tree`, face oracle | 18 / 7008 | 18 / 7008 — unmoved |
| `tree`, ground oracle | 226 / 252105 | 226 / 252105 — unmoved |

The two `tree` columns are the control and they do not move at all: this phase
changes *which* solid a fragment is exempt from, and on a scene where the answer
was already right there is nothing for it to change. What moved is the scene
built to have a wrong answer.

**The bar the numbers rest on is a mutation, not a margin.** Putting `lit_end`
back to `on_surface(ctx.spot_z, low, high)` on *both* walks reads
1296/1296, 1248/1248 and 9216/9216 again — the recorded pre-phase numbers
exactly — and turns `light.rs`'s own
`a_fragment_is_exempt_from_its_own_solid_and_from_a_twin_of_it_beside_it` red
while leaving its first assertion green. A phase whose fixture cannot be made
red again is a phase measured against nothing.

How it is carried, where that differs from the design above:

- **`Owner` and `OwnerId` are two types**, and keeping them apart is what the
  design's decisions 2 and 3 are when written down: `Owner` is the world key
  (`z` and the placed graphic, three bytes, never uploaded) and `OwnerId` is
  which occluder of *this cell* that key is (one byte, the fourth channel of a
  reference). `Occlusion::owner_at` is the join between them and the one thing
  the drawing side calls.
- **The numbering lives beside the references, not beside the solids.**
  `Occlusion::owners` is one `OwnerId` per entry of `ids`, because the number is
  a fact about a reference: the first thing to reference one solid from two
  cells (decision 38.2's spill) gives it a different number in each. Nothing
  does today, which is exactly why the level was built now.
- **`OwnerId::NONE` matches nothing, including itself**, so every comparison
  goes through `OwnerId::same` rather than `==`. Two fragments that are each a
  point of nothing are not a point of the same thing, and the ground, a mobile
  and any static the grid refused all stamp it.
- **`Surface::shadowed_by_own_tile` is gone, and it was already vacuous.** The
  `lit_end` arm it masked also required the surface *not* to be `Flat`, and that
  function answers `0` for every surface that is not `Flat` — so the conjunct
  was true for every solid that ever reached it, and the real restriction was
  the `caps_this` arm beside it. Identity replaces both. The shader lost the
  per-cell loop that gathered the tile-wide union of sides with it.
- **`own_run` stays**, and it is now the only exemption that reads a height. It
  answers a *second* question — a ray leaving a wall pixel along the wall grazes
  the neighbouring tiles' panels of the same wall — which identity cannot answer
  at all, since those are different statics and therefore different owners. See
  the backlog.
- **The reordering is in `app::render` and in `examples/isolated_scene.rs`**,
  both for the same reason and both stated at the seam: a frame's occlusion is
  built before its statics are collected, so a drawn row carries the number this
  frame's grid gave it rather than the one before it.

**One test moved and it is worth saying why**, because the reason is the
behaviour change and not the test.
`frame.rs`'s `the_light_view_keeps_a_pools_shape_where_it_is_brightest` counted
how many pixels of a row across the room rise towards the flame, and wanted more
than a quarter of the whole row. Eight of the pixels it was counting are on the
room's own wall tile, and the parity fixture's pixels are `Surface::Upright`
points of no occluder — so they are now honestly behind the wall body they stand
in, where the height guess used to exempt them from it. The bar was measuring
the guess. It now asks that **every** step of the row *inside the room* rises,
which is the stronger claim and the one the test was written for.

## Order, and what gates what

0 gates everything. 1 and 2 are independent of each other and both precede
3 — not because 3 needs their precision, but because 3 removes the code
that hides whether they worked. Each phase lands with the face oracle's
count in its commit message.

4 came after all of them and repeated the shape at its own scale: an oracle
first, red, and only then the rule. It is also where the ordering paid for
itself twice — the oracle took back two claims about the defect before the fix
was written, and the fix's own measurement took back a third.

## Status

Phase 0 done: the face oracle lives in `examples/boxes.rs`
(`OPENSHARD_BOXES_FACE_ORACLE=0` to skip it), grids each box's own rendered
`east`/`south` face, and is red on `tree` as expected — 956/16384 compared
points disagree at the default `H1=3`, `light::sample` agreeing with the
independent oracle at every disagreement checked by hand (`through=1.000`,
"lit", against a rendered pixel reading "shadowed"), which places the fault
on the GPU side, not the CPU walk. It now also reports **where** up each face
its disagreements sat, as runs of grid rows — added in phase 1, because a
count alone cannot tell a defect made smaller from a different defect the
first one was hiding, and phase 1's residual turned out to be exactly that
distinction.

Phase 1 done: **956 → 278 of 16384** on `tree` at `H1=3`, and the shape says
what moved. Rounding was restored for one run to take the before-picture with
the same instrument, so these are one measurement apart and nothing else:

| face | before | after |
|---|---|---|
| box 0 east | 128, all in rows 0..3 (`z` 0.02..0.12) | 128, rows 0..3 — untouched |
| box 0 south | 746: 2 at the foot, **744 in rows 31..64** (`z` 1.48..2.98) | 68: 2 at the foot, 66 in rows 41..63 |
| box 1 east | 81, all in rows 0..4 (`z` 3.02..3.16) | 81, rows 0..4 — untouched |
| box 1 south | 1, at the foot | 1, at the foot |

So phase 1 removed 678 of the 744 points of the dark patch inside the lower
box's south face — the defect this plan opens with — and moved nothing at all
outside it. `View::Height` down one column of that face went from **4 distinct
values in runs of 15-17 pixels to 49, one step per pixel**; down box 0's east
face, 5 to 60. The banding is gone.

**The residual is not the penumbra**, and phase 1's own "done when" above
guessed wrong about that: `light::sample` reads `through=1.000` at these
points, a hard disagreement rather than a soft edge. 210 of the 278 are the
bottom one-to-five grid rows of a face — a fragment at the very foot of a face
being shadowed by the thing it stands on. That is `exemption`'s guess, phase 3,
and no amount of height precision reaches it; the ~5% soft-edge baseline the
box-top oracle reports against `walk_cells_exact` is a different measurement
against a different reference and should not have been borrowed as this
phase's floor.

`OPENSHARD_TREE_H1=3.5` **went the other way: 691 → 1103**, and that is
expected rather than a regression. With the fragment's height continuous and
the occluder's still rounded (`Solid::bottom`/`top`, phase 2), a box whose own
base is at 3.5 is uploaded as a solid spanning 4..7, so the bottom half-unit of
its own faces now sits *below* its own solid and stops being exempt from it.
Before phase 1 the two roundings cancelled and hid that. This is precisely what
phase 2's "done when" asks for — the two configurations agreeing rather than
one being clean by luck — and it now has a number to close.

Not from phase 1 and worth knowing before phase 2: at `H1=3.5` the box-top
oracle reads 3027/9216 and 9216/9216 against `light::sample`, a **CPU-side**
disagreement that no part of phase 1 touches (the `place` attachment is not on
that path). Same cause, one layer over: a solid whose `z` span is fractional.

Phase 2 done: the occluder's height is continuous end to end, and the two
configurations now agree instead of one being clean by luck.

| oracle, `tree` | `H1=3` before | `H1=3` after | `H1=3.5` before | `H1=3.5` after |
|---|---|---|---|---|
| face oracle | 278/16384 | **278** — identical, face by face and row-run by row-run | 1103/16384 | **235/16384** |
| box 0's own top | 0/9216 | 0 | 3027/9216 | **0** |
| box 1's own top | 0/9216 | 0 | 9216/9216 | **0** |
| ground oracle | 509/57600 | 509 | 1325/57600 | 574 |

*(Every face- and ground-oracle number in this table is from the old
instrument, and most of each is the instrument rather than the engine — see
the end of this section. The two box-top columns are unaffected: that oracle
never projected anything.)*

The integer column is the control and it does not move *at all*: at a whole `z`
every fraction this phase adds is zero, so the run is bit-for-bit the one before
it — which is what says the 868 points the fractional column lost were the
rounding and not a second change riding along with it. The two CPU-side box-top
numbers the last session flagged as phase 2's entry (3027 and 9216, a solid
whose span was rounded under a *flat* sample) are gone outright.

What is left, 235 and 278, is now **one shape in both**: about 200 points in the
bottom one-to-five grid rows of every face, and a band of 44–66 just under the
lower box's top. Both are `exemption`'s guess — a fragment at the foot of a face
shadowed by the thing it stands on, and the lower box's top being the same plane
as the upper box's base — and neither is reachable by precision at all. That is
phase 3, and the plan's own opening paragraph said so.

How it is carried, since the answer differs from what this section sketched:

- **The whole span, sixteen bits an end, and nothing left behind.** The plane is
  `Solid::z_bytes` — each end a `u16` in steps of a two-hundred-and-fifty-sixth
  of a `z` unit from `Z_FLOOR`, which is exactly the `-128 ..= 127` a map's own
  `z` lives in. `Occlusion::solid_bytes`' first two channels, which held the
  rounded span, are **zero**.
- **`span_of` is the only place in `blit.wesl` that turns the wire into a
  height**, and `light::wire_span` its CPU twin. A reader that decodes a height
  itself still compiles and still looks like a height, which is the failure
  phase 1 hit in `plan.rs`.
- `on_surface`, `pierced`/`pierces`, `crosses` and `box_of` take the span as a
  parameter on both sides now, so each walk supplies the one it is entitled to:
  `walk_cells_exact` the record's own `f64` corners, `walk_cells_streaming` the
  quantised one off the wire — the vertical half of the discipline
  `Solid::fraction` already stated for the horizontal one.
- The audit the phase asked for is done: no `bottom()`/`top()` call is left on
  either walk. What survives is the cutaway and `Occlusion::at`'s merged view,
  and each says so in its doc comment. `solid::standing`'s painter-order key was
  a third and is now the exact span — two boxes half a unit apart used to tie.

**And then the instrument turned out to be wrong, which retired most of the
numbers above.** The next session pointed the face oracle at what the renderer
had actually drawn instead of at a reconstruction of it, and the residual both
phases had been reporting mostly stopped existing. In full, because the shape
of the mistake matters more than the arithmetic:

- The face oracle gridded world points over each face and projected them to
  pixels. Whether the pixel belonged to the face it was asking about was
  answered by re-deriving every face's screen quad on the CPU, with a
  point-in-quad test and a painter's-order tie-break — a reconstruction that
  knew nothing about the ground pass. Half a pixel below a face's own base is
  the ground, correctly shadowed by the box, and that read as the face being
  wrongly shadowed: **212 of the 278**. Those are the "~200 points in the
  bottom one-to-five rows of every face" this section attributes to
  `exemption`'s guess above, twice, in two sessions' handoffs. They were the
  instrument.
- The oracle also asked about points the shader never lights: a pixel's
  fragment sits at the pixel's centre, and the attachment quantises what it
  carries. The ground oracle had known this since it was written and
  quantised by hand; the face oracle never did.
- What was left after both fixes was 43, and 27 of those were the *shader*
  alone: `blit.wesl`'s `RAY_TANGENT_TOLERANCE`, a cross-implementation
  rounding guard, was set to `1.0e-2` of a whole ray — about a screen pixel of
  world — so every box was a pixel fatter than its geometry wherever a ray
  grazed it. At a rounding-scale `1.0e-6` the whole parity suite is still green
  and the two tests the tolerance was introduced for fail identically, which
  they also did at `1.0e-2`.

The reference scene's honest residual is **18 of 7008 drawn face pixels**, all
of them `STAND_OFF`/`ON_TOP`'s deliberate nudge at a grazing corner — zeroing
the two constants on both walks for one run reads `0/7008`. None of it is
`exemption`. See `docs/lighting.md`'s "One scene is the reference" for the
current table and `a4b698c`/`ccca681`/`f050c2d` for the work.

The lesson is not that phases 1 and 2 were wrong — they moved `View::Height`
from four values to forty-nine down a face, and closed two CPU-side box-top
oracles outright, and those are real. It is that **a residual is a claim about
a cause, and this plan twice let a plausible attribution stand as one**: the
count moved the way the phase predicted, so the remainder was assumed to be the
next phase's. Nothing checked which side of the comparison was out until
something did, and then it was the side nobody had instrumented.

**Two things this phase got wrong first, and what they cost.** Both were found
by being asked whether the work was a workaround, which is worth writing down as
plainly as the result:

- The span shipped as **a rounded unit plus a signed fraction**, on the argument
  that `solid_bytes`' channels had to keep meaning what they meant "for a reader
  not taught about the new plane". There was no such reader: after the phase
  `blit.wesl` reads a height through `span_of` and nowhere else. The
  compatibility had nothing to be compatible with, and it bought a second
  concept (a fraction *of* something), a second clamp, and a rounded copy of a
  number living better elsewhere — the exact shape of a format growing a field
  nobody dares change. Replaced by the whole span above; every oracle number in
  the table is identical either way, so this was cost without effect.
- The three `walk_cells_streaming_agrees_with_walk_cells_exact_*` tests were
  **blind to what this phase introduced**. They build every fixture through
  `Builder::add` off a `StaticTile`, so every span in them is a whole `z` — and
  the two walks now read *different* heights for one solid on purpose, which on
  a whole `z` are equal by construction. Mutating `wire_span` back to the
  rounded span leaves all three green; only the fractional-`z` body added here
  goes red. A fourth test, and the mutation is what says it earns its place.

Phase 3 done: `exemption` asks which occluder a fragment is a point of instead
of guessing it from a height, and the fixture built to be red for it reads zero.
The table and the account are in its own section above; the three questions this
section left open when phase 2 landed are answered there:

- **One static, several solids** — the owner is the static, so its solids share
  one, and there is no run to name.
- **Mobiles** — a billboard has no solid, so it stamps `OWNER_NONE` and is
  exempt from nothing. That is the honest answer and it is a *behaviour
  change*: today a mobile standing on a walled tile is exempted from that wall
  by the same height guess as everything else. Worth a look at a real frame
  when it lands, not a preemptive tolerance.
- **`lighting_geometry.md`'s mesh occluder** — read, and it changes nothing
  here: a mesh is a different *shape* test against the same `ray_vs_solid`,
  and identity is about which occluder a fragment came from, not what shape it
  is. The one line of that doc which does bear on this track is its warning
  that vertex data fits a fixed-size `Rgba8Uint` grid worse than a box's six
  numbers — which is why phase 3's own byte goes in a plane that already
  exists rather than in a fifth one.

## Phase 4 — a fragment is not shadowed by the lid it stands on

*(Landed. "The rule, landed — and the second defect it was hiding" below is what
it measured; everything before that is the case as it was built, including two
readings the oracle took back.)*

Phase 3 replaced the height *guess* with identity and left one thing standing
beside it: `exemption`'s `stands.edges != 0`, which refuses a lid **before** it
ever compares owners. It is the last categorical carve-out in a predicate whose
other arm is now a fact, and it is what two visible artefacts on
`examples/synthetic_stair` turned out to be — both of them, one cause.

### What it looks like, and what it measured

The scene is one flight, treads `1,3,5`, on one tile, its own occlusion, one
flame; nothing else exists in the frame to misread. Two shapes come out of it:

- **A hard hairline down every tread/riser join.** Cross-referencing the
  `shadow` dump against `place` and `height` at the same pixel puts them at
  `sub.y` `0.329` and `0.671` — the two riser planes — at the `z` of the tread
  each riser *stands on*, with `sub.x` running the whole width of the tile. It
  is the bottom row of a riser, entire. The pixels are not dimmed, they are
  `through <= RAY_CUTOFF`: a hard flip, not a soft edge.

  **This one is not the defect, and the oracle is what said so** — see "What the
  oracle said" below. Everything measured about it here is right and the reading
  put on it was not: those pixels are `Prism::mesh`'s own `SEAM_OVERLAP`,
  fragments of a riser drawn *under* the tread it stands on, and shadowing them
  is what the geometry there says. The whole paragraph is left standing rather
  than corrected in place, because "every number was right and the conclusion was
  not" is the thing worth being able to re-read.
- **A whole tread top, black,** wherever the flame is below it. This one is.

Neither is the display. Measured in the dump's own pixels across the zoom
ladder, the count goes 22 → 88 → 132 → 346 while the silhouette grows 75 → 242
px wide: the seam grows faster than the outline rather than staying a sampled
pixel. (Which is also what a fixed *world-space* feature does — `0.15` of a `z`
unit is `2.4` px at `4:1` — so this ruled out the display and nothing else.)

`Reach::stopped_by` names the culprit outright once it carries a `Stopper`:

| probe | answer |
|---|---|
| foot of a riser, `z 0.9375` | `stopped by (100, 100) owner 1, lid z 1.00..1.00` |
| foot of a riser, `z 1.0` | `through 1.00` |
| top tread, `z 5.0`, flame below | `stopped by (100, 100) owner 1, lid z 5.00..5.00` |
| middle tread, `z 3.0` | `through 0.14` — the same lid, partially |

**Owner 1 is the fragment's own owner**, and the top tread's case is the purest
statement available: it is stopped by *the very plane it is drawn on*. This is
self-intersection at `t = 0`, and it has exactly two canonical answers — an
epsilon, or dropping the primitive the ray left from. Phase 3 chose the second
and stopped one predicate short of it.

The epsilon is what stands there today, and it is the wrong size by
construction: `ON_TOP` is `1/128` of a `z` unit while the `place` attachment
quantises `z` to sixteenths, so a fragment's reported height sits up to `1/32`
below the surface it is drawn on and the nudge covers an eighth of that. A sweep
says so — `ON_TOP` at `1/128`, `1/64`, `1/32` leaves the seam at 346/347/350
pixels, `1/16` halves it to 176, `1/4` takes it to **0**. It narrows
continuously rather than switching off, because the error it fights is the
pixel's, not the format's. `ON_TOP`'s own doc comment still says "the attachment
quantises `z` to whole ones" — the sentence that justified the number, left
behind by phase 1.

### The rule, stated once

> A contact at the ray's origin does not count. A crossing at `t > 0` counts,
> whoever owns the solid.

Everything falls out of that: the lid of a fragment's own static does not shadow
it at the start, the *same* lid still shadows a ray that genuinely descends
through it later, and `ON_TOP` stops being "lift the point off its surface". It
is not a fifth special case, it is the four existing ones minus two.

What the rule does **not** answer, and must not be stretched to: `own_run` (the
neighbouring tile's panel is a different static, so a different owner) and
`flame_end` (a flame has no owner at all). Both stay their own questions.

### The case the fix must not break, and it is measured

`OPENSHARD_STAIR_RUN=3` with the flame **above and beyond** the run
(`OPENSHARD_LIGHT_Z=5 OPENSHARD_LIGHT_AT=5.0,0.5`) turns the far end of a wide
staircase black, and it looks exactly like the defect above. It is not. Probed:

| face of the far flight | stopped by |
|---|---|
| riser 1, `z 0.5` | `(100, 100) owner 1, lid z 1.00..1.00` — **the fragment's own occluder** |
| riser 2, `z 2.0` | `(101, 100) owner 1, lid z 3.00..3.00` — another cell |
| tread 1's top, `z 1.0` | `(102, 100) owner 1, lid z 3.00..3.00` — another cell |

The first row is a fragment stopped by a lid of its own static and it is
**honest**: the ray leaves the bottom step's front face heading up and north, and
crosses that step's own top *well away from where it started*. A lamp standing
above and beyond a staircase genuinely cannot see the front of its bottom step —
the staircase's own body is in the way. The rule as stated keeps it blocked,
because the crossing is at `t > 0`; a fix phrased as "a fragment is never stopped
by its own static" would light it, and would be wrong.

So the mutation that says phase 4 works is this scene going **unchanged** while
the single flight's seams go to zero. A count alone cannot tell the two apart.

**And an instrument trap, found by falling into it.** Rows two and three above
read "owner 1" against a fragment that is also owner 1, which invites exactly
one conclusion and it is false: an `OwnerId` is a number *within a cell*, so two
cells' number ones are unrelated statics. `exemption` is not fooled — every arm
of it that reads an owner is gated on `own_cell` — but a person reading the
report was. `light::stands_to` now spells the relation out in the report rather
than leaving two equal numbers side by side.

### What the oracle said, and what it took back

Steps 1 and 2 below are done. `examples/synthetic_stair` now carries the face
oracle `examples/boxes.rs` has, and the two share it — `examples/oracle/mod.rs`,
one slab test, because two copies of a geometric primitive is the shape that
drifts. Every pixel the rendered `place` attachment says a flight's own face
drew is one comparison: the fragment's own world position off that attachment,
an independent segment-vs-AABB test about *that* point, against the rendered
`View::Shadow` pixel.

What the oracle has that `boxes.rs`'s does not is **which occluder a fragment is
excused from**. `boxes.rs` drops the whole box a point rests on. A flight is one
static, one owner and **six planes**, and a fragment is a point of exactly one of
them — the face the renderer drew it from. So the oracle drops that one plane and
counts every other, its own flight's included. That is the rule above with no
epsilon in it: a ray leaving a plane crosses that plane at its own origin and
nowhere else, so "a contact at the origin does not count" and "this primitive
does not count" are one sentence for a plane. Its geometry is re-derived from the
tread profile and then gated, plane for plane, against the grid's own solids and
against the drawn mesh's own normals and planes, so a divergence between two
derivations panics by name rather than arriving as a count.

It is red before any fix, and it took two of this section's own claims back.

**The hairline is not this phase's defect.** `Prism::mesh` grows every riser by
`SEAM_OVERLAP` — `0.15` of a `z` unit — at both ends, so the last-submitted face
wins a coincident edge outright instead of leaving it to a sub-pixel tie. Those
are real pixels of a riser drawn *under the tread it stands on*, at a place the
staircase's own body fills, and being shadowed there is honest. The oracle counts
them as their own class: **1120 pixels of a single flight are drawn beyond their
own plane's span, and 2 of them disagree.** The rest agree — renderer and
independent geometry both say shadowed. Two numbers already in this section
point the same way and were read as something else: `0.15` `z` is `2.4` px at
`4:1`, which is the measured 2 px width, and the seam growing with the zoom
ladder is a world-space feature behaving like one, not a defect outgrowing its
outline.

**Both flame placements written down here are degenerate**, each sitting exactly
in a plane of the geometry, and a grazing scene answers on the quantum rather
than on the geometry:

| scene | disagreeing | what moved |
|---|---|---|
| one flight, `2.5,1.0` (this section's default) | 5779/24106 | flame is at `y 101.0` — **riser 0's own plane** |
| the same, flame at `2.5,1.4` | 3031/24106 | riser 0 goes 456 → **0**; the picture is unchanged |
| the run, `LIGHT_Z=5` (§ the counter-example) | 7545/69508 | flame is at `z 5` — **the top treads' own height** |
| the same, `LIGHT_Z=6` | 2337/69508 | every tread top goes to 0 or 2 |

Off the degeneracies, the residual is two clean classes and they have opposite
signs:

- **Tread tops rendered too dark** — 1522 and 1346 of the middle and top treads,
  `2868` of the single flight's `2929`. This is phase 4's lid, and
  `light::sample`'s own report names it: `stopped by (100, 100) owner 1, lid z
  5.00..5.00 — THE FRAGMENT'S OWN OCCLUDER`. Both walks together on every one of
  them; no parity gap anywhere in any run.
- **Riser tops rendered too light** — the whole of the run scene's 2190, banded
  at the top sixteenth of every riser. Not this phase's: it is `STAND_OFF`
  walking a face pixel `2/127` of a tile in front of its own plane, which at the
  corner where a riser meets the tread above it is six times the geometric margin
  and clears that tread's lid outright. The backlog entry that says nobody has
  priced `STAND_OFF` at a grazing corner now has a price.

The counter-example scene is where the fix's verdict has to be "unchanged", and
the oracle says something better than that: with the flame genuinely above the
run, **every tread top there already reads 0 or 2** — the honest occlusions are
honest today. The fix has to leave that alone, and now there is a number saying
what "alone" is.

> **That last sentence is wrong, and the fix is what found it.** Eight of the
> run's nine tread tops read 0 or 2; flight 2's bottom tread read **510**, all
> of them "too light". Which also means the entry below that attributes the
> run's whole 2190 to `STAND_OFF` at riser tops is over by 512: 1678 are riser
> tops and 512 are tread tops. This one is a mis-read *number* rather than a
> mis-read conclusion, so it is corrected here rather than left standing — but
> it is the same failure as the two above it, which is reading a per-face table
> by looking at the total and the first few rows.

The proportion holds across the zoom ladder — 192/1498, 747/6012, 1725/13449,
3031/24106, all within a quarter of a point of 12.6% — so none of it is sampling.

### The rule, landed — and the second defect it was hiding

**Step 3 is done, and it took two predicates rather than one.** The lid half is
this section as written. The panel half the oracle demanded the moment the lid
half landed, and it is the same sentence one level down.

`light::drawn_on` is the lid half, and it is two facts with no tolerance in
either. The lid is a **plane** (`low == high`), which is not a special case but
the condition that makes "a contact at the origin does not count" and "this
primitive does not count" one sentence — a plane is crossed at a single point,
so a ray leaving one crosses it at its own origin and nowhere else, and there is
no later crossing an exemption could swallow. A lid with a real depth, a sloped
roof section, is a slab a ray genuinely descends into, and it answers `false`
for one. And the fragment is drawn at that plane's own height, asked of the
height the *fragment* reports rather than of the ray's start: `stand_clear`'s
`ON_TOP` is the walk's own nudge, and phase 4's whole defect is a
hundred-and-twenty-eighth answering "which surface am I" as though it were
"where does this ray go". `ExemptionContext` carries both heights now, and the
field's own comment says which question each answers.

The **owner gate stays**, and it is load-bearing rather than tidy. A wall's face
pixel at exactly the `z` of the floor its wall stands on is drawn at that
floor's height too, so the bare geometric form of the rule excuses it — and that
is the bright stroke a house wore along its floorboards that `ON_TOP` was added
to close. Being at a plane's height is not being a point of it; being at it
**and owned by it** is.

**Then the oracle took the next claim back.** With the lid excused, the middle
and top treads went from 1522 and 1346 *too dark* to 3224 and 3893 *too light*.
Both classes are the same defect wearing opposite signs, and the black tread
tops had been hiding the second one: an `OwnerId` is per `Builder::add` and a
flight's six solids share one, so identity alone was also excusing the **risers**
that genuinely stand between a tread top and a flame below it. A staircase's own
body is in the way of its own treads; that is not a surface shadowing itself.

So `exemption` now asks each shape the one exact question the wire can answer
about it — a `match` on `edges`, and every arm is two facts compared rather than
one guessed at:

| shape | is the fragment a point of it |
|---|---|
| lid | it is drawn at this plane's own height (`drawn_on`) |
| body (`EDGE_ANY`) | yes — a body has no face to be a point of one of, and a fragment of one carries `Stance::Upright` for that reason |
| panel | its own stance names the side this panel stands on (`edges & own`) |

The panel arm needs no new field: `own` was already on `ExemptionContext` for
`own_run`. It is exact because a static that pushed a named panel gave its
fragments a face to carry — `place::Stance::of` hands a face to exactly the
statics `occlusion::edges_of` hands a named edge — and because **a flat fragment
names no side at all**, so a tread top is a point of no riser of its own flight.
It also restores something phase 3 dropped without noticing: a corner is two
panels under one owner, and a fragment of the north face is a point of the north
one only. `docs/lighting.md` decision 23 says a corner's perpendicular panel is
a different surface and stops the ray as it always did; between phase 3 and here,
it did not.

**Measured, against the classes the section below named rather than against a
total:**

| scene | before | after |
|---|---|---|
| one flight, `2.5,1.4`, tread 1's top | 1522 too dark | **2** |
| the same, tread 2's top | 1346 (1345 too dark) | **1** |
| the same, whole flight | 3031/24106 (2929 too dark) | **136/24106 (4 too dark)** |
| the same, zoom notches 0/1/2 | 192 / 747 / 1725 | **17 / 34 / 76**, tread tops 0 |
| the run, `LIGHT_Z=6` | 2337/69508 (147 dark, 2190 light) | **1834/69508 (147 dark, 1687 light)** |
| `boxes.rs` `tree` / `pair` / `line` | 18/7008 · 0/5088 · 15/16324 | **unchanged** |

All 147 of the run's honest "too dark" are unmoved, which is what the
counter-example was for, and the probe that defines the phase still reads
`stopped by (102, 100) owner 1, lid z 1.00..1.00 — THE FRAGMENT'S OWN OCCLUDER`:
the same *kind* of solid the rule excuses, still stopping the ray because that
crossing is at `t > 0`.

> 🚨 **And every number in that table was counted on pixels the flame stands
> behind.** The backlog's first entry has the measurement: on this scene tread
> 1's top and tread 2's top — the two faces the `1522` and the `1346` are
> counted on — set aside **5516** and **5423** fragments as back-facing and
> compare **none**. `light::faces` gives them `(−1/11)/0.2 + 0.5 ≈ 0.045`, so
> whatever the occlusion term does there reaches the picture at a twentieth of a
> flame's contribution.
>
> **What that takes back:** "a whole tread top, black" was black mostly because
> the flame is under it, not only because of the lid, and this section's headline
> counts are of a class the picture barely shows.
>
> **What it does not take back:** the rule. An occlusion term is either right or
> wrong independently of what multiplies it afterwards, the counter-example probe
> is a *riser* with the flame above and beyond — in front of it, and still
> compared — and the three-ray mutation test in `light.rs` is geometry with no
> facing term anywhere near it. What the phase needs is a scene that shows the
> same defect on a face the flame is in front of; it does not have one, and the
> backlog says so.

> **Both columns are with `Prism::mesh`'s `SEAM_OVERLAP` still in.** It was
> removed immediately after — see the backlog's first entry — and that changes the
> "after" column again, because the seam pixels stop being drawn and one band of
> every tread top starts being: the single flight reads **316/23912** and the run
> **1706/68962 (23 too dark)**. The probe above is unchanged, the tread tops stay
> at zero away from that band, and `boxes.rs` has no prism in it. The two changes
> are recorded apart because they are two, and because the second one is only
> legible against the first.

**And one number in the section above was wrong, which the fix is what
found.** "With the flame genuinely above the run, every tread top there already
reads 0 or 2" — flight 2's bottom tread read **510**, all too light, and the
backlog entry that attributes the run's whole 2190 to `STAND_OFF` at riser tops
is over by the 512 of them that are tread tops. Re-measured: 510 → 7. The
residual 1687 is riser tops and is `STAND_OFF`'s, unmoved in kind.

The mutation is `light.rs`'s
`a_fragment_is_shadowed_by_every_plane_of_its_own_static_but_the_one_it_is_drawn_on`
— one flight, three rays, both walks, and each ray red under a different
mutation: the contact at the origin, the same lid at `t > 0`, and the riser a
flat fragment is not a point of.

**What is left is the "too light" class**, 132 of the single flight's 136 and
1687 of the run's 1834. It is `STAND_OFF`'s price at a grazing corner, it has its
own backlog entry with a scene and a number, and it is not this phase.

### How the rule was chosen, and the order it went in

The instrument is in — `light::Stopper`, `light::stands_to`,
`synthetic_stair`'s `OPENSHARD_STAIR_PROBE`, and now the face oracle. The rule
is not.

That ordering is this plan's own phase 0 restated, and it is worth restating
because everything above is a reason to skip it: the defect is understood, the
cause is named, the fix is one predicate. But every number phase 4 has is a
count of pixels this renderer drew, judged by eye against geometry worked out on
paper. That is precisely the arrangement that let phases 1 and 2 report a
residual for two sessions which turned out to be the instrument — and there, as
here, the arithmetic was right and the thing nobody had instrumented was the
side doing the judging.

So, in order:

1. ~~**A face oracle for `synthetic_stair`**~~ — done, and it is the section
   above. It counts what it compared, counts the pixels no flame reaches apart
   from those it judged, and asserts the total non-trivial.
2. ~~**The same oracle over the run**~~ — done. Its verdict there is better than
   "unchanged": with the flame genuinely above the run, the tread tops read 0.
3. ~~**Then the rule.**~~ — done, and the section above is what it measured.
   Done when: the single flight's **tread tops** read zero with `ON_TOP` at its
   own value, at every zoom notch, on a flame *off* every plane of the geometry;
   the run's tread tops stay at zero; and the mutation that says so is the
   `t > 0` crossing rather than the count.

   The target is the 2868 too-dark tread-top pixels of the single flight
   (`OPENSHARD_LIGHT_AT=2.5,1.4`), and the two classes that must **not** move
   are the 1120 seam pixels (`Prism::mesh`'s own overlap, honest) and the
   run's 2190 too-light riser tops (`STAND_OFF`, a separate backlog entry). A
   fix that took the grand total to zero would have eaten at least one of them.

   *(The seam class no longer exists: `SEAM_OVERLAP` was removed right after this
   phase landed — the backlog's first entry — so those 1120 pixels are not drawn
   at all now. "Must not move" was the right instruction for a lighting fix, and
   it held: the rule left them where they were, and a separate, deliberate
   geometry change is what took them off the screen.)*

   **That done-when is what caught the second defect.** Written against the
   grand total it would have read green at 3031 → 7250, since the tread tops
   *had* stopped being too dark; written against the class it said the tread
   tops had merely changed sign.

**Where the rule should live is a design question, not a patch site.** The three
candidates, so the next session does not re-derive them: a `t`-threshold inside
`ray_vs_solid` (cheapest, and an epsilon again — the thing being removed); a
`skip` carried on `ExemptionContext` naming the solid the ray left from (exact,
but the ray leaves a *surface*, and a surface is not always one solid); or
`exemption` learning that a solid on the fragment's own cell **with the
fragment's own owner** is exempt *at the origin only* — which is the one that
puts the fact next to the other fact, and needs "at the origin" to be a
geometric statement rather than a tolerance. Decide it with the oracle already
red, not before.

**The third is what landed**, and "at the origin" came out geometric after all:
for a plane it is an equality between two exact quantities on the wire, and
`ON_TOP` never enters it because the question is asked of the fragment's own
height rather than of the ray's start. The epsilon the section feared was
avoidable, not because a tolerance was found small enough, but because the
question was being asked at the wrong end of the nudge.

**And the oracle was a fourth candidate's argument.** What it does is the
second option, and it needed no tolerance to do it — because it has something
`exemption` does not: the fragment names the *plane* it is a point of, not the
static. `Spot::owner` is an `OwnerId` per `Builder::add`, so a flight's six
solids share one, and every candidate above is an attempt to recover per-plane
identity from a per-static number plus geometry. The mesh row already carries
which face drew the pixel. Whether the fragment should carry the solid rather
than the owner is the question the oracle's own shape asks, and it is worth
pricing before an epsilon is chosen: `Occlusion` would have to hand out a
per-solid id the way `owner_at` hands out a per-static one, and every producer
of a `place` row would have to know which of its own faces it is pushing —
which `statics::selected` and `items::outlined` cannot, and already stamp
`OwnerId::NONE` for.

**That question is still open, and the landed rule is the measurement of how
much of it is needed.** Per-plane identity was recovered for two of the three
shapes out of facts already on the wire — a height for a lid, a side for a panel
— and neither is a proxy. Where it is *not* recovered is inside one shape: a
flight's three risers all face the same way, so a fragment of one is excused
from all three, and two coplanar lids of one static are excused together. The
first is a real gap and the run scene's residual is where to look for it; the
second is correct by construction, since a fragment on one of two coincident
planes is a point of both. So the honest price of a per-solid id is now "the
riser-to-riser case, plus whatever else a scene finds" rather than "the whole of
phase 4" — which is a much weaker reason to pay it than it looked before.

## Backlog

Picked up while phases 1 and 2 landed, while the oracles were repaired, and
while phase 3 landed, and while phase 4's oracle was built, and while phase 4's
rule landed; none of it blocked any of them.

- ✅ **A flight's bottom riser was on the wrong side of its own tile, on the
  wire.** Found by asking the picture a question a count cannot be asked: *sweep
  the flame's height and watch the shadow move.* Five renders at `z 0..4` beside
  five reference frames, and the engine's shadow stopped matching below `z 1` —
  the bottom step's own front face shadowing nothing at all, 2295 pixels of one
  flight.

  `Solid::fraction` measured a solid's footprint from `space.min.floor()`. That
  is right for a box with extent, whose `min` is inside its own tile, and wrong
  for a **plane**: a plane at a whole coordinate is the boundary *between* two
  tiles and `floor` always picks the far one. A climbable's first riser is
  exactly that — `tread_riser_box_of`'s plane for the tread at the low end of the
  climb sits on its tile's own far edge — so a north-climbing flight on
  `(100, 100)` had a riser at `y == 101.0` measured as fraction `0` of tile `101`
  and rebuilt by `box_from_footprint` at `y == 100.0`: a tile's width away, on
  the opposite side of its own cell.

  `Solid::footprint` already had to decide this and states the rule — a
  degenerate axis at a whole coordinate belongs to the tile below when the
  solid's own `edges` name that axis's high side — and `fraction` was written
  without it. Both spell it once now.

  **Nothing could have caught it.** `walk_cells_exact` reads `space` and was
  right all along; only `walk_cells_streaming` and the shader read these bytes,
  and the two walks' agreement proptests build their panels with `Solid::box_of`,
  whose slab is `PANEL_THICKNESS` deep and therefore never a plane — the fixtures
  could not pose the question. The gate is now a round trip over every solid a
  climbable makes, all four climb directions, in `occlusion.rs`'s
  `every_solid_comes_back_off_the_wire_on_the_cell_it_was_put_on`; mutating the
  rule back to a bare `floor` turns it red by name.

  Measured across the sweep, single flight, disagreements out of 23912:

  | flame `z` | 0 | 0.5 | 1 | 1.5 | 2 | 2.5 | 3 | 3.5 | 4 | 4.5 |
  |---|---|---|---|---|---|---|---|---|---|---|
  | before | 2297 | 2291 | 2279 | 8 | 316 | 283 | 3679 | 94 | 94 | 272 |
  | after | **66** | **60** | 2279 | 8 | 316 | 283 | 3679 | 94 | 94 | 272 |
  | with facing counted apart | 60 | 54 | **42** | 6 | 41 | 8 | **0** | 0 | 0 | 89 |

  The third row is the same sweep re-run after the oracle got its half-space test
  (the entry two below), and it is why the last sentence of this one is struck
  out. It used to read: *the two spikes sit at `z 1` and `z 3`, which are two of
  this flight's own tread heights, and nowhere else* — and both spikes were the
  oracle's own missing test. Every pixel of them is a tread's **top** with the
  flame at or below its plane, which the oracle now sets aside instead of blaming
  on the renderer. The denominator moves with the flame for the same reason:
  `compared + behind` is `23912` at every height, and how it splits is which
  faces the flame is in front of.

- ✅ **The light-judging oracle exists, and the first thing it did was take back
  a class of its own.** `write_light_reference` computes what the engine computes
  — `colour × intensity × (1 − d)² × visibility × facing`, summed over the flames
  — out of the scene's own parameters, and `write_light_difference` lays it
  against a rendered `View::Flames` frame, which is the pools' contribution with
  the ambient left out and no curve over it, so a byte in it is a number rather
  than a threshold. `View::Light` was the backlog's own suggestion and is the
  wrong frame for this: `knee(lit)` has the ambient in it and a curve over it, and
  an oracle would have to invert both.

  Six classes, five of which are not "the renderer is wrong": agreement,
  brighter, darker, the engine's own penumbra (read off the `View::Shadow` frame,
  not guessed), inside `FACE_EDGE`, at the frame's ceiling, and — the one that had
  to be added after the first run — **the two rasterisers gave this pixel to
  different planes**. That last class was 88 pixels reading as "the renderer is
  half as bright as it should be", a clean factor of two, at a tread's own top
  edge: the engine drew the **lid** there and this file's painter order drew the
  **riser**, the lid's normal points up, the flame stood at exactly that lid's
  height, so the engine's `faces` was `0.5` and the reference's `1.0`. Asking the
  `place` attachment whose pixel it is — the renderer's own answer, the same gate
  the counting oracle has always had — takes the whole "darker" column to **zero**
  at every flame height.

  What survives is one-signed and now has a magnitude instead of a pixel count:
  the engine is **brighter** than the geometry allows, on the top band of the
  topmost riser, by up to **`0.51` of a channel**. Across the sweep: 175, 139, 57,
  50, 57, 25, 0, 0, 0, 0 pixels for flame `z` `0 … 4.5`. That is `STAND_OFF`'s
  entry below, which lost its number to the half-space correction and has one
  again — in brightness, which is what a person sees, rather than in pixels of a
  term.

- ~~**Every oracle on this track judges a *term*, not the light, and the next one
  should judge the light.**~~ **Done, above.** `View::Shadow` is `through` alone — no `faces`, no
  falloff, no cone — and `write_reference` draws pure visibility beside it. That
  pairing is honest as far as it goes and it is the reason the half-space bug
  above could live: a quantity that is multiplied by something before it reaches
  a pixel can be wrong in ways the pixel never shows, and can be *judged* wrong
  where the pixel would not have cared. The engine already has the other view —
  `View::Light`, "the lighting alone, with the art thrown away" — and the
  reference already has the geometry to compute the same thing: visibility ×
  half-space × inverse-square falloff × the pool's radius. What it does not have
  is the flame's own size, and it should not grow one: the point-source
  difference is the *question* those two pictures are for. Then a disagreement
  means "this pixel would look wrong", which is what nobody has been able to say
  yet.

- **Phase 4 has no scene that shows its own defect on a surface the flame faces,
  and that is the work its section is missing.** Its default puts the flame under
  the tread tops it is about, so `light::faces` gives them about `0.045` and the
  occlusion term reaches the picture at a twentieth. The rule is right and its
  mutation test is geometry, but the *picture* argument for it was made on a
  class the picture barely shows. A fixture where a lid of the fragment's own
  static stands between it and a flame it is turned towards would settle it; the
  run scene with the flame above and beyond is the nearest thing in the tree and
  it exercises the counter-example rather than the defect.

- 🚨 **`FACE_EDGE` is one constant at two incomparable scales: ±4 px across a
  wall's face, ±1.1 `z` above a lid.** `light::faces` is not a step but a band
  `FACE_EDGE = 0.2` tiles wide centred on the plane, and `along` is a **distance
  in tiles** and not a cosine. For a vertical face that is a tenth of a tile
  either side — four screen pixels at `4:1`, exactly the softening it was written
  to be. For a horizontal one the same tenth of a tile is `0.1 × Z_PER_TILE`,
  and `Z_PER_TILE` is `44/4 = 11`: **`1.1` `z` units**, which is more than half
  the height of a stair's step and about a third of a table's. A lid does not get
  a soft rim from this — it gets a graded answer over the whole of it.

  Seen the moment the light oracle drew it: on the sweep's own scene, with the
  flame at `z 2` between treads at `z 1` and `z 3`, **7059 pixels** — the entire
  drawn area of both lids — fall inside the band, against `3940` of genuine
  penumbra. The picture shows the two lids solid green from edge to edge.

  What it costs *there* is small, because the flame is near the middle of the
  band on one lid and near its end on the other: `0.020` of a channel on average
  and `0.029` at worst. The costly arrangement is the degenerate one — a flame
  exactly in a lid's plane reads `faces = 0.5`, so **half** that surface's light
  is a decision by a constant rather than by geometry, and every static in the
  client's files stands at a whole `z`. That is the same class the entry below
  used to record as a spike; measuring it is what the sweep with this oracle is
  for.

  The reason for a band is real and is stated where the constant is: a hard edge
  is what the eye finds first, and a lamp walking past the end of a wall would
  switch its face off between two frames. What is not stated anywhere is that the
  number buying four pixels of softness on a wall buys a whole step on a stair.

- 🚨 **The oracle had no half-space test, and most of this track's residuals
  were that.** Found by orbiting the flame around the flight — eight positions on
  a circle of `2.5` tiles at `z 2.5` — and reading the *sign* of what came out:

  | flame | E | SE | S | SW | W | NW | N | NE |
  |---|---|---|---|---|---|---|---|---|
  | as first read | 41 | 550 | 841 | 541 | 42 | 449 | 748 | 455 |
  | sign | mixed | light | light | light | mixed | **dark** | **dark** | **dark** |
  | with facing counted apart | 21 | **2** | 93 | **2** | 18 | **0** | **0** | **0** |

  The first reading looked like one defect with a sign — under-shadowing in front
  of the flight, over-shadowing behind it, mirrored counts of `654` on the same
  face from either side. It was not a defect at all. **A one-sided surface cannot
  be lit from behind**, so for a fragment the flame stands behind, "is anything in
  the way" is not a question about that fragment's shade: `light::faces` decides
  it before occlusion is ever asked. The oracle had no such test, drew those
  fragments as *lit*, and reported every one of them as the renderer being wrong.

  `Slab::faces` is that test now, and back-facing pixels are counted apart rather
  than folded in — the identical argument `Shade::Unreached` already carries one
  axis over, that a fragment outside every pool is dark because of a *radius* and
  a visibility oracle has no opinion about radii. The whole back half of the orbit
  goes to zero.

  **What that costs every number this section records**, re-measured:

  | scene | as recorded | with facing counted apart |
  |---|---|---|
  | single flight, `2.5,1.4` | 316/23912 | **41/12973** |
  | the run, flame above | 1706/68962 | **133/56034** |

  And the sharpest of it: on phase 4's own default scene, tread 1's top and tread
  2's top now compare **0 pixels** and set aside **5516** and **5423** — *every*
  fragment of the two faces the phase was about has the flame behind it. Those
  faces are what the `1522` and `1346` were counted on. See the phase's own
  section for what that does and does not take back.

  The band the engine gives between a plane and `FACE_EDGE/2` behind it is
  deliberate softening and is not judged here either way; the oracle's rule is the
  geometric one — strictly behind means strictly unlit — and where the two differ
  the engine is being generous beyond geometry rather than wrong.

- ~~**A flame at exactly a surface's own height loses most of that surface's
  shadow.**~~ 🚨 **The spikes were the oracle again, and what is left is a class
  nobody judges.** This entry read the sweep's `z 1` → 2279 and `z 3` → 3679 as a
  defect *at* the degeneracy, and both numbers were taken before the oracle had a
  half-space test. Re-run with one: **42** and **0**. Every pixel of both spikes
  was a tread's own top with the flame at or below its plane — the oracle drew
  those lit, the engine did not, and the oracle called it the engine's fault.

  What is true and is *not* a number about the renderer: at `z 1` the flame lies
  exactly in tread 0's top plane, the oracle's `Slab::faces` is a strict `> 0.0`,
  so all `5517` pixels of that face go to "the flame is behind this" and are
  **not compared at all**. The engine gives that same face `faces = 0.5`, because
  `FACE_EDGE` is a band and a flame in the plane sits at its middle. So the two
  differ by half the light over the whole face and nothing counts it — a
  degenerate arrangement is not a spike here, it is a **hole in the instrument**,
  and it is the common case: every static in the client's files stands at a whole
  `z` and so does every torch on one. This is the same hole as the `FACE_EDGE`
  entry above, and the light-judging oracle below is what closes both, because
  there `faces` is a factor in the answer rather than a reason to refuse it.

- **A riser's shadow on the tread behind it is graded over most of that tread,
  and the picture reads it as a light in the wrong place.** Found by looking at
  `synthetic_stair`'s reference frame beside the rendered one: the two put the
  shadow in the *same region*, and where the reference has an edge the engine has
  a gradient several pixels wide. The arithmetic, so it is a number rather than an
  impression: a panel's crossing is graded by `pierces` over a band
  `tall = soft * FLAME_DEPTH` in `z`, `soft` is `clamp(spread * middle / (1 -
  middle), 0.05, 0.7)` and `FLAME_DEPTH` is `Z_PER_TILE / 4` = `2.75` `z`, so an
  occluder close to the fragment — which is what a flight's own riser always is —
  gives a band of `0.14` to `0.69` `z`. A riser is two `z` tall, so the softest
  case grades a third of the face, and a tread top is seen nearly edge-on, which
  spreads that band across most of its drawn width. Every number in it is
  deliberate (`FLAME_DEPTH`'s own doc measures it against a wall in Britain);
  what nobody has asked is whether a penumbra sized for a wall's top edge, three
  or four tiles from the flame, is the right size for an edge a fifth of a tile
  away. The reference frame is now the way to judge it — it draws the hard shadow
  the same geometry casts, so the two pictures are the question stated.

- ~~**The seam is honest lighting on pixels that should not be on screen.**~~
  **Done — `SEAM_OVERLAP` is gone.** The oracle settled that its pixels were
  *shadowed* correctly — they are a riser drawn inside the staircase's own body
  — and that turned out to be a statement about the lighting only. What the
  picture showed was a one-pixel **dark hairline across every lit tread**,
  because those riser pixels win the depth tie over the tread they stand on and
  are the ones a person sees; and, measured against a build with the constant at
  `0.0`, a **3 px** overrun at both `z` ends of every riser, so each step's
  corner sat `2.4` px off where the geometry puts it in both directions. That is
  the "the planes look offset" reading of the picture, and it was right.

  The constant existed to make the last-submitted face win a coincident edge
  rather than leave it to a sub-pixel tie. **There is no tie.** A tread's top and
  its own riser are built from the same `footprint` expression and the same
  `top_z`, so their shared corners are bit-identical in world space, and
  `statics::push_mesh` projects a corner with a pure function of that corner —
  identical corners cannot land on two screen positions, and the rasteriser's own
  fill rule gives every pixel of the edge to exactly one triangle. The face map
  measured the consequence directly: **zero** pixels inside a flight's silhouette
  belonging to no face, over four climb directions × four zoom notches × five
  tread profiles, the tread count being what moves that edge's sub-pixel phase.
  `facing.rs`'s `a_tread_and_its_riser_share_an_edge_bit_for_bit` is the gate.

  `docs/gbuffer.md` carried the reading that justified it and now carries the
  correction beside it. The hairline that motivated it was real — it was the
  *outer* silhouette, which is `WIDTH_OVERLAP`'s own doc's measurement, an edge
  bordering no other face at all.

  **What removing it uncovered**, because a defect can hold another: the single
  flight goes 136 → 316 disagreements and the run 1834 → 1706 (its "too dark"
  147 → 23, since 123 of those *were* seam pixels). The 273 new ones are the last
  band of every tread top — the row where it meets its own riser — reading lit
  where the geometry says shadowed, and the sample's own report names the point:
  a fragment at `(100.00, 100.66, z 3.0)`, which is the riser's plane at the
  riser's own top. `ON_TOP` lifts the ray a hundred-and-twenty-eighth above that
  top, so it clears a crossing the geometry has by a hair. Same class as the
  `STAND_OFF` entry below, same corner, the other axis — and those pixels were
  always computed that way, they were just being drawn by the riser.

- **`WIDTH_OVERLAP` costs `1355` pixels of silhouette, and it is not a tooth —
  it is a border all the way round.** 🚨 The measurement the entry below asked
  for, taken by *splitting the difference frame's yellow in two*: the class "only
  one of the two drew anything here" was one colour, which says the two shapes
  differ and cannot say **which is wider**. Two colours — orange for the renderer
  alone, cyan for the reference alone — and the answer is immediate: a solid
  orange band about two pixels wide runs the whole length of both silhouette
  edges across the climb, on every frame of the flame-height sweep, `1458`
  unshared pixels of which `1370` are the renderer's. It does not move with the
  flame, because it is not about light at all.

  Zeroing the constant is the control: `1370 → 15` the renderer's way and
  `88 → 117` the other. So **`1355` pixels**, roughly a tenth of the flight's own
  drawn area at `4:1`, are the price — and in this scene they buy nothing, since
  what the overlap exists to hide is the seam between a mesh and the *sprite*
  drawn under it and `synthetic_stair` draws no sprite. The remaining hundred-odd
  are single-pixel dashes along the diagonal edges and are **not attributable**:
  the reference's rasteriser samples pixel centres with no top-left rule, so a
  one-pixel disagreement on a diagonal edge is two fill rules differing and not
  the engine being wrong. Pricing the sliver it *does* hide needs a scene with a
  sprite in it.

  The face map shows the same face poking `0.03` of a tile past its
  own tile, which is a **2 px** tooth at `4:1` — measured against a build with it
  at `0.0`, the east silhouette sits at column 317 with it and 315 without. Unlike
  the retired `SEAM_OVERLAP` beside it, the edge it is about borders no other
  face: it is the fitted prism against the art's true silhouette, and those two
  genuinely differ (`best_prism`'s score is never exactly `1.0`). So the leak is
  real and an overlap does hide it — it is still a fudge, one side of the trade
  now has a number and the other does not. The honest alternatives are to stop
  drawing the sprite behind a meshed static, or to clip it to the mesh.

- ~~**The `ground < 1e-6` shortcut in both walks ignores a lid's own
  footprint.**~~ **Fixed.**
  A ray with no horizontal run takes a shortcut past the candidate-cell loop and
  applies `crosses` to *every* lid on the cell, with no test of whether the ray
  is over that lid at all. The main path stopped doing that when sub-tile
  footprints landed — `walk_cells_exact`'s own comment says a tread's top is a
  lid narrower than its tile — and the shortcut did not follow. On a stair it
  means a fragment standing on one tread and lit from straight above or below is
  occluded by the *other* treads' lids, which are strips of the tile it is not
  over. Three copies to fix (`walk_cells_exact`, `walk_cells_streaming`,
  `blit.wesl`'s own). Found writing phase 4's mutation test, which had to slant
  its first ray to avoid it.

  All three gate on the footprint now — `light::over_footprint` and
  `blit.wesl`'s twin, the horizontal half of `ray_vs_solid`'s parallel-axis rule
  and only the horizontal half, because the height answer is `crosses`'s soft one
  and `ray_vs_solid` would answer it hard. The slanted ray is joined by a
  straight one that no longer needs to be: `light::a_vertical_ray_is_not_stopped
  _by_lids_it_is_not_over`, and `frame::the_shader_does_not_stop_a_vertical_ray
  _with_a_lid_it_is_not_under` for the shader's copy — which had no coverage at
  all until now, and stayed green with its fix deleted. See
  `docs/lighting_rebuild.md`'s backlog for what that says about the parity
  harness.

- **A flight's risers are still excused as a group.** Phase 4's panel arm asks
  whether the fragment's own stance names the panel's side, which is exact and is
  what separates a tread top from every riser — but a flight's three risers all
  face the same way, so a fragment of one is excused from all three. A ray
  leaving a low riser towards a flame beyond and above the *north* of the flight
  can cross a higher riser's plane, and would be let through. This is the honest
  remainder of the per-solid-id question in phase 4's own section, and the run
  scene's 1687 residual is where to look for a case of it.

- **`STAND_OFF` lights the top sixteenth of every riser from behind, and that is
  its price at a corner.** The backlog entry below says nobody has priced
  `STAND_OFF`/`ON_TOP` at a grazing corner; phase 4's oracle priced one of them.
  A face pixel is walked from `2/127` of a tile in front of its own plane, and at
  the inner corner where a riser meets the tread above it that is about six times
  the geometric margin — enough for a ray to clear that tread's own lid outright.
  Measured: **1678 of the run scene's 2337 disagreements** — the entry first said
  2190, which was the whole "too light" column and included 512 tread-top pixels
  that phase 4's rule then took to 7 — every one of them "rendered too light",
  banded at the top of every riser, on a flame standing above and beyond. Both
  walks together, so it is the engine's arithmetic in both implementations and
  not a parity gap. After phase 4 it is **1687 of 1834**.

  🚨 **And then almost all of it turned out to be the oracle**, which had no
  half-space test: on that scene the flame stands at `y 100.5`, north of every
  riser's plane, and a riser looks south. With back-facing fragments counted apart
  the run reads **133 of 56034**, so `STAND_OFF`'s measured price at a grazing
  corner is at most that and not `1687`. The entry stays because the mechanism is
  real and `docs/lighting_height.md`'s own `ON_TOP` twin is measured on a face the
  flame *is* in front of — but it has no scene with a number on it any more, and
  finding one is the work. The shape a fix would take is the one the
  entry below already guesses at — a nudge scaled to the surface rather than to
  the attachment's format — and it now has a scene and a number to be judged on.

- **Both of phase 4's own flame placements sit in a plane of the geometry**, and
  a scene that grazes answers on the quantum rather than on the geometry.
  `OPENSHARD_LIGHT_AT`'s default `2.5,1.0` puts the flame at `y 101.0`, which is
  the first riser's own plane; the counter-example's `OPENSHARD_LIGHT_Z=5` puts
  it at exactly the top treads' height. Each costs a class of pure tangency —
  456 pixels and 5208 pixels respectively, both of which vanish when the flame
  moves a fraction off the plane, with no visible change to the picture. The
  defaults are **left as they are** on purpose: they are what phase 4's own
  recorded numbers were measured on, and moving them silently retires those. What
  wants deciding is whether a fixture default should ever be a degenerate
  arrangement, given that every number taken on one has to be read twice.

- **`boxes.rs` still reads a fragment outside every pool as a shadowed one.**
  `Shade` (in `examples/oracle/mod.rs`) decodes the three answers `blit.wesl`
  writes into the shadow frame; `boxes.rs` calls `Shade::lit`, which is the
  half-channel test it always used and answers `false` for `Unreached` as well as
  for `Blocked`. On `tree`'s own scene nothing is out of reach so nothing is
  wrong today, and adopting the distinction there would move that tool's recorded
  counts for a reason unrelated to what they record. `synthetic_stair` counts
  `Unreached` apart and reports it; `boxes.rs` should, next time its numbers are
  being re-baselined for another reason.

- **`own_run` is the last exemption that reads a height. It now has a scene, and
  on that scene it holds.** A ray leaving a wall pixel *along* the wall grazes
  the neighbouring tiles' panels of the same wall — different statics, therefore
  different owners, so identity cannot answer it and `own_run`'s
  same-row/same-column mask gated on `on_surface` is what still does. The `pair`
  fixture is one tile and cannot see it; `OPENSHARD_STAIR_RUN=n` on
  `examples/synthetic_stair` can — `n` flights side by side *across* the climb,
  so their risers are one plane cut on tile boundaries and their treads abut at
  equal `z`. Measured with the flame **in the riser plane and at its height**,
  which is the one arrangement where a ray runs along the wall without also
  climbing through the treads over it: one flight and three produce the same
  shape, and the blocked count scales with area (5012 → 15088) instead of gaining
  a stroke per seam. So the guess is not visibly wrong here — it is still a
  guess, and what the fixture did surface was phase 4's lid, once per flight.
  What is still missing before this entry can close: the same run built out of
  *wall* statics rather than climbable ones, and an oracle pointed at the seams,
  so the claim rests on an independent reference rather than on a stroke nobody
  saw.
- **`flame_end` is still a height test, and it is `mounted_at`'s question.** The
  far end of a ray is a flame, not a fragment, so there is no owner to compare —
  the arm that exempts the solid a sconce is mounted on asks `on_surface(to_z,
  ...)`. Deliberate and stated in phase 3's design; worth its own entry now that
  the fragment side is identity, because it is the one place a *second* thing at
  the flame's height is exempted for no reason but sharing it.
- **A mobile standing on a walled tile is now shadowed by that wall**, where the
  height guess used to exempt it. It is the honest answer — a billboard is no
  occluder, so it is a point of nothing and exempt from nothing — and it is a
  *behaviour change on a real frame* that no test here can be the judge of. Look
  at a lit room with somebody standing against its wall; if it reads wrong, the
  question is what a mobile's own footprint is, not what tolerance to add.
- **`Occlusion::owner_at` is a linear scan, once per drawn static.** Two or
  three solids a cell, so it is nothing today — but it is a scan inside the
  per-static loop of two collectors, and the first tile that holds a hundred
  solids pays for it once per static on it. Named rather than fixed: the shape
  that would replace it (a map keyed by `Owner`, built once at `finish`) costs
  an allocation a frame on the side of this pass that is already thirteen times
  the GPU.
- **`statics::selected` and `items::outlined` stamp `OwnerId::NONE`**, which is
  correct exactly because the select and outline passes do not light anything.
  Nothing says so at the type level: a row is a row, and the day one of those
  masks is fed through the blit it will draw a static that shadows its own face,
  silently. The rows are the *same placement* as the drawn ones by design —
  `statics::quad_of` is shared — so the owner could be shared too; it is not,
  because those callers have no grid in hand and giving them one to satisfy a
  field they never read is the wrong trade until something reads it.

- **`STAND_OFF`/`ON_TOP` are the reference scene's whole residual, and nobody
  has priced them.** Zeroing both on both walks takes `tree`'s face oracle from
  18/7008 to 0 and its ground oracle from 226 to 137. They exist for a reason
  that is written down and measured (a wall wore a bright stroke along its
  floorboards without them), so this is not a proposal to remove them — it is
  that "how far off its own surface a ray starts" is a number chosen once, in
  units of the *attachment's* quantisation (`2/127` of a tile, `1/128` of a `z`
  unit), and what it costs at a grazing corner has never been looked at. A
  smaller nudge, or one scaled to the surface rather than to the format, might
  cost nothing.
- **The wire's span rounds to nearest, so a solid can be a hair *shorter* than
  it is.** `Solid::z_bytes` rounds each end to the closest step, so
  `walk_cells_streaming`'s box can be smaller than the record's on either end —
  and a smaller occluder is a shadow with a hole in it, which is the one
  direction of error the rest of this pass takes care to avoid (`z_bytes`'s own
  clamp says so in words: "it stops at least what it really stops"). Rounding
  *outward* instead — floor the base, ceil the top — costs one more step of
  span and buys a one-sided property: the wire box always contains the exact
  one, so `walk_cells_streaming` can never let through what `walk_cells_exact`
  stops. That is a stronger claim than the numeric agreement the parity tests
  assert today, and a cheaper one to hold at a tangent.
- **The exact-tangent case is a definition, and the two sides differ.** The
  other 137 ground pixels are rays that touch a box's corner at exactly one
  point. `light::ray_vs_solid`'s doc says a zero-length crossing is the
  caller's decision and then no caller decides it; `boxes.rs`'s independent
  oracle counts it as blocked. One of the two should move, and which is a
  question about what a hard shadow's corner should look like.
- **`examples/two_cubes.rs` still carries the old oracle idiom.** It projects
  world points and reads pixels without asking the `place` attachment whose
  pixel it got — the same blindness `boxes.rs` just shed, in a tool that is
  still used to answer the same kind of question.

- **`tests/cost.rs`'s "what the upload sends" measures three planes of five.**
  Its `black_box` sums `bytes` + `field_bytes` + `id_bytes` + `solid_bytes`, and
  has never included `footprint_bytes`; `solid_z_bytes` is now a second one
  missing. A cost line that names most of a thing reads as the whole of it.
- **`plan::Wall::top` is an `i32` the caller invents**, so an elevation of a wall
  standing to `z 3.5` is drawn in a frame four units tall with half a unit of
  nothing at the top. Only `tests/pictures.rs` builds one today, always at a
  whole `z`, so nothing is wrong on any picture that exists — but the field is
  the picture's own vertical extent and there is no reason for it to be whole.
  (An earlier draft of this entry said the value came from `Occlusion::at`'s
  rounded `Cell`. It does not; it is a parameter.)
- **`Occlusion::at`'s `Cell` is still whole units.** Its three readers are the
  wireframe, the plan view and `mounted_at` (which reads `edges` only), so
  nothing that decides a shadow reads it — deliberately left, and worth
  revisiting if a fourth reader ever wants a height rather than a picture.

- **Two hand-copies of the third channel are left**, and both are correct
  today only by accident: `tests/select.rs`'s `place_texel` and
  `tests/frame.rs`'s parity-fixture builder each fold `(z + 128) | stance <<
  STANCE_SHIFT` themselves, and each happens to pass an integer `z`, so the
  fraction they never write is zero. `place::packed_height` is what they
  should go through. A third copy — `plan.rs`'s elevation picture — was the
  one that *did* bite: an instrument with its own copy of the format rounded
  the height and drew, in the diagnostic meant to show a wall's face, the
  very treads this plan is about.
- **The face oracle's projection idiom is now stated five times** in
  `examples/boxes.rs` (box-top oracle, ground oracle, the main mesh dump, the
  face oracle, and its `ScreenFace` corners): `camera.to_view_exact(
  project_exact(..))` with `projection.origin`/`.scale` applied by hand. One
  named function, once.
- **`mesh::Face` and `facing::Face` collide by name** inside one crate, and
  `boxes.rs` aliases one of them (`as WallFace`) to say which it means. Not
  phase 1's business, but the next file that needs both will pay it again.
- **The `owned_by_someone_nearer` tie-break has never executed.** Its
  `f.depth == box_depth[i] && f.box_index > i` arm needs two faces at equal
  depth with overlapping silhouettes, and no scene here produces that. It has
  been read against `renderer.rs`'s `LessEqual` and nothing more.
