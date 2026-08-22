# Lighting: a flame that a wall can stop

> **Consolidated into [`lighting_rebuild.md`](lighting_rebuild.md)** — the system being replaced.
> That document is the entry point: it lists what is still live here, which rebuild phase retires or inherits it, and what carries over untouched. This file stays as the record of how it was built and why.


Current state of the point-light/shadow pass: what it computes, the data
formats it reads and writes, and the engineering compromises it currently
ships with. The reasoning behind each choice — arguments made, alternatives
tried and rejected, session-by-session history — lives in
[`lighting_archive.md`](lighting_archive.md), organized under headings that
mirror this file's.

Three tracks split off this file as it grew and are not repeated here:

- [`lighting_raymarch.md`](lighting_raymarch.md) — boundary-precision
  correctness of the shadow ray walk itself (the DDA, tile-edge float
  hazards, CPU/GPU parity at exact ties). This file states the walk's
  *rules* (what a panel does, what a body does, exemptions); that file
  states how the walk is kept numerically exact against those rules.
- [`lighting_world.md`](lighting_world.md) — the ambient/sky field (baked
  indoor-vs-outdoor darkening independent of point lights).
- [`lighting_geometry.md`](lighting_geometry.md) — the occluding primitive
  moving from a fixed axis-aligned box to a general mesh where a box can't
  state the shape (curved roofs, terrain slope). The box described below is
  still the default and still free for everything it already covers.
- [`gbuffer.md`](gbuffer.md) — the G-buffer payload this pass reads (the
  "place" attachment) and the render-side per-face normal work that is
  slowly replacing the fixed-tag `Stance` normal described below.

## Overview

Lighting is computed in world coordinates, not screen pixels: a fragment is
lit according to the tile and height of *the thing drawn there*, not where
that thing landed in the image. This is what lets a wall's face be lit as
the wall's own tile is lit, and what lets a storey below stay dark while the
street above it is not — a screen-space circle of light (this pass's
predecessor) cannot do either, because the screen folds height into `y` and
a wall's sprite stands above the tile it occludes from, so any screen-space
mask that darkens the ground behind a wall also covers the wall's own lit
face.

Three world passes (`ground`, `statics`, mobiles) each write a second
attachment alongside colour: which tile and height the drawn pixel belongs
to (the "place" attachment, below). `crates/client/render/src/light.rs`
collects the flames a frame can see and the occluders in their way on the
CPU; the blit shader lights the frame in world coordinates, walking an
occlusion grid between each fragment and each flame. F10 toggles night: a
torch inside a house does not light the street, and a wall's own face
(turned toward a flame) is the brightest thing beside it.

The renderer is a three-dimensional scene whose primitives happen to be
billboards, not a sprite blitter with lighting bolted on: world space is
what the light reasons in, a per-pixel world position is already written by
every world pass, `Camera::project` is a view-projection matrix (written as
integer arithmetic — no rotation, so no trigonometry), and the depth buffer
is real hardware depth. A box drawn in world coordinates lands in the same
pixels the sprite for that tile lands in, to the pixel, with nothing fitted
— see "Solids as drawable geometry" below.

Shaders are WESL sources (an `import`-carrying superset of WGSL, compiled to
plain WGSL at build time by `crates/client/render/build.rs`) —
`shaders/statics.wesl`, `shaders/ground.wesl`, `shaders/mesh_face.wesl`,
`shaders/blit.wesl`, `shaders/select.wesl` — sharing the place-attachment
packing constants (`KIND_*`, `SUB_TILE`, `PLACE_STANCE_SHIFT`,
`STANCE_FLAT`/`STANCE_FACE_*`/`STANCE_CORNER`/`STANCE_MESH_FACE`, and the
`pack_place()` function) from `shaders/place_format.wesl`, imported rather
than each declaring its own copy.

## The G-buffer bridge: the place attachment

Every world pass (ground, statics, mesh faces) writes a second colour
attachment, `Rgba16Uint`, packed as `(x, y, z + 128, kind)` per pixel: the
tile the pixel belongs to, the height it was drawn at (`+ 128` so a signed
`z` fits an unsigned channel), and what kind of thing wrote it. A `u16`
holds a coordinate on the largest facet a client ships (7,168) exactly, and
`Rgba16Uint` is colour-renderable under the WebGPU ceiling this crate
targets (see `crates/client/render/src/lib.rs`'s own module doc). A
fragment a sprite discarded writes nothing, so the channel names what is
*visible* — which is exactly the question lighting asks — and `kind == 0`
means "no world here" (the cleared background): ambient only, no flame.

**Fraction encoding.** A sprite's pixel carries where in its tile it sits.
A floor static (`TileFlags::FLOOR` — `Background` in ClassicUO terms, set
on floors/rugs/roads and nothing that stands up) spreads its pixels across
the tile's diamond, honestly reversing `Camera::project`'s pixel offset from
the tile's centre; a wall billboard's pixels run along the edge it stands on
(see "The art-measurement pipeline" for how that edge is found) with height
following one formula that covers all stances:
`z = z0 + ((sub.x + sub.y - 1) * 22 - dy) / 4`. Every fraction channel is
clamped to `INSIDE = 126/127` rather than allowed to reach exactly `1.0`:
`fract()`/`floor()` of an exact whole number reads as the *next* tile, so a
fragment sitting on its own tile's far edge would silently be attributed to
the wrong tile. `statics.wesl`, `ground.wesl` and `mesh_face.wesl` all clamp
through the shared `pack_place()` helper in `place_format.wesl` rather than
each computing the clamp inline — a producer that forgets to stamp a value
(the class of bug `pack_place()`'s required `stance` argument now makes
impossible to do *by omission*, though a wrong-but-present value is still
possible) is one of the two ways this format has actually broken; see
`lighting_raymarch.md`'s backlog for the omission-class history and its
still-open "wrong-but-present" half.

**Stance.** `place::Stance` (`crates/client/render/src/place.rs`) has ten
values: `Flat`, `Upright` ("standing but its facing is not known"), the four
cardinal faces, and four two-face corners. It rides in a `u16`'s spare bits
above the kind — `place::STANCE_SHIFT = 8` in the attachment's `z + 128`
channel, and at a separate shift in the instance's own place word that
`statics.wesl` reads per-instance (a different value for a different
purpose: the instance's stored stance versus the attachment's per-pixel
one). `blit.wesl` turns a stance into an outward normal: `Flat` looks
straight up (`(0, 0, 1)`), a face looks along its cardinal axis, and
`Upright` has no normal at all — "nothing is known, so every flame lights
it," which is what a tree, a body, or a wall the art detector refused to
read gets. A corner resolves to one of its two faces per fragment, by which
half of the tile's column the pixel is drawn on (`across` in the shader) —
`right = FaceNorth + (offset >> 1)`, `left = FaceSouth + (offset & 1)`, laid
out so the two faces come out by arithmetic rather than a table.

Which face(s) a stance encodes is itself measured off the art, not read
from `tiledata.mul` (nothing there records which edge of its tile a wall
stands on) — see "The art-measurement pipeline".

**Where this is still moving.** `gbuffer.md` is where the payload's shape
itself (this section) is being revisited — the honest per-face normal for
solids and stair treads (below) is landing there rather than as a
`Stance`-taxonomy extension. An earlier attempt at a computed normal for a
sloped tread, blended between two fixed directions, was retired in favor
of `gbuffer.md`'s real per-face mesh geometry (see `lighting_archive.md`
for that attempt and why it was replaced). This file's description above
is what is built and running today; `gbuffer.md` is where its next shape
is being decided.

## The occluding world: solids, the grid, and the bake

**What stops light is what stops an arrow, not what blocks movement:**
`WINDOW | NO_SHOOT`, not `BLOCK` — the same test ServUO's line-of-sight uses
(`Map.LineOfSight`, `Server/Map.cs`). A barrel or a fence is `BLOCK` and you
can see (and shoot) over both; a wall is `NO_SHOOT` and you cannot. The grid
carries an *opacity* byte rather than a boolean flag, with three answers:
`NO_SHOOT` stops everything, `WINDOW` stops four fifths
(`occlusion::PANE`), everything else stops nothing. `PANE`'s value has no
source in any client file — it, `light::flame`'s TORCH-by-default and
`light::midday` are the pass's invented numbers, alongside a handful of
tuning constants below (`FLAME_SPREAD`, `SOFT_CROSSING_MIN`/`_MAX`,
`HELD_BEAM_DEGREES`, `BEAM_EDGE`, `BEAM_SPILL`) — each held by a scene
rather than derived. A static the frame's `Cutaway` has removed from the
picture occludes nothing (`cutaway::shows`), the same predicate the world
passes use to decide what to draw.

**An occluder is a `Solid`: a box in world coordinates**
(`crates/client/render/src/solid.rs`, `occlusion::Solid`), not clipped to
its own tile and never rotated (`solid.rs`'s own module doc: "no rotation
anywhere in this renderer... and it never will"). Every shape the grid
currently holds is that box with some of its six numbers pinned:

| shape | as a box |
|---|---|
| a lid (floor, rug, road) | zero height |
| a body (post, tree, unread wall) | the whole tile |
| a named panel (a wall on one edge) | a slab `PANEL_THICKNESS` (`0.2` tile) deep, fattened inward from the plane its art stands on |
| a stair tread | part of one axis (the climb), full width otherwise |
| an authored `Blocks` entry (an arch, a lintel) | whatever axis-aligned box a person placed |

A panel's box is a real, tested slab: `Solid::box_of`'s four named-edge arms
fatten the plane inward by `PANEL_THICKNESS`, and the walk tests the slab,
not a bare plane. The one thing not folded into a box is a **hole**
(a window) — it is a subtraction, kept as its own field alongside a
surface's box rather than expressed as geometry.

**A cell carries a span of heights and a 4-bit edge mask, not one merged
span.** `z` from the static and `z + height` from its tiledata entry
(`height` is ServUO's `CalcHeight`, halved for a climbable tile). A tile's
occluders are separate `Solid`s standing beside each other — a wall from
`z 0..10` and another from `z 30..40` no longer close the thirty units of
air between them — and a cell's mask names **which sides of its tile are
occupied** (`PRESENT | edges`, four bits): `EDGE_ANY` means "it stands up
and the art would not say which way" (a body), `0` means a lid, one to
three bits set means named panels. Two solids on one tile combine with
`max`, not a sum or a product — two panels on one tile are two faces of one
corner, and a ray crossing both has gone through one thing once.

**The tile grid is a broadphase index, not a container — a cell holds
*references* to solids, not the solids themselves.** A solid is anchored to
the tile its static stands on (naming which map block owns it), and how far
it extends past that anchor is unconstrained — measured, not limited to one
tile. A cell's texel is `(offset, count)` into an index plane of solid ids,
which addresses a separate solid plane; the walk reads one more indirection
per solid per cell than a format where cells owned geometry directly. This
is a deliberate cost of the *model*: a solid overlapping four cells is
referenced four times (four `u16`s) rather than cut into four separate
records with four sets of seam rules between them — every historical seam
bug in this pass (the corner spokes, the fraction-of-exactly-one bug, the
wall self-shadow seam) was manufactured by cutting geometry on a tile
boundary, and referencing removes the cut rather than patching around it. A
solid crossing a ray's path is not deduplicated against being tested twice
by two different cells — the slab test is exact and cheap enough that a
visited-set would cost more than the redundant test it avoids.

**A solid's own drawn pixel resolves to a face by the same slab test the
walk uses**, run from the camera instead of from a flame — projection is
orthographic, so "which face of the box is this pixel on" is a ray-vs-box
query with a different origin. This is what gives a stepped stair lid three
horizontal treads instead of two vertical half-walls, and what turns the
`Stance` taxonomy above into a derived answer rather than a hand-enumerated
one for anything that already has real box geometry. The solid is consulted
by the light and by the normal computation, never by the rasterizer — the
drawn sprite is still what the world passes place, unchanged by any of this.

**Baking.** The grid is not rebuilt from the map every frame. `Bake`
(`crates/client/render/src/occlusion/bake.rs`) holds one `Baked` per map
block — the surfaces its statics stand and the sky they take, in
block-local cell coordinates so the same bytes serve a frame at any camera
offset. A frame is built by pasting the blocks its rectangle overlaps
(`bake::collect`), then doing the three things that are genuinely per-frame:
folding in the server's live ground items, blurring, and packing against
the frame's own `Cutaway`. `Cutaway` (which storeys are visible from where
the player stands) is applied at this packing step, not at the block-level
map walk — a `Builder` walks **what a ray may cross** (every surface on the
map inside the rectangle) and `Builder::finish` applies the frame's cutaway
as a filter on top, which is what makes a per-block cache valid across
different camera cutaways rather than being one frame's answer.

A block is dropped from the cache and rebaked when `StaticAtlas::revision`
moves — a counter over exactly the three answers `occlusion::Shape` is
made of (a facing, a hole, a prism), bumped only when the atlas actually
packs a new graphic, not per frame it is asked about one. Without this, a
block baked before a graphic was packed into the atlas would hold the
whole-tile `EDGE_ANY` fallback forever, even after the atlas could name a
real face for it. `KEEP_BLOCKS = 4096` (about seven frames of walking)
bounds how many cached blocks are kept; a solid whose box reaches past its
anchor's own block puts a **spill** entry (`Baked::spill`, absolute map
coordinates) into every block it also touches, and a frame pastes a ring of
blocks around the ones it needs, wide enough to catch any spill —
`bake::ring_radius` reads the widest reach any solid in the art table
declares (currently `0`: nothing built today is wider than one tile) rather
than using an invented constant, and the ring width is logged so an
authored wide solid's cost is visible rather than silent.

**An occluder can also be authored rather than derived.**
`facing::Block`/`facing::Blocks` (`crates/client/render/src/facing.rs`) are
a fixed-size array of plain axis-aligned boxes in a graphic's own
tile-local coordinates, for a shape a single climb profile (`Prism`, below)
cannot describe — an arch, a lintel floating over the gap between two
posts. `occlusion::Shape` carries `blocks` beside `prism`; a graphic may
have both, independently. Nothing derives a block list automatically (there
is no search over box placements the way `facing::best_prism` searches
climb profiles) — a person places boxes by eye against the real sprite
through `tests/author.rs` (see "The art-measurement pipeline"), so there is
no wrong automatic reading to gate against, and it needs no `CLIMBABLE`-style
gate the way a derived `Prism` does. `arttable`'s row grammar carries zero
or more `block x0 x1 y0 y1 z0 z1` clauses at `FORMAT = 4`.
**`Builder::add` does not yet read `Shape::blocks`** — nothing is wired from
an authored block list into the live grid, and nothing has been authored
into the checked-in `data/overrides.table` yet either (see "Status").

**What is not yet a box.** A body's *footprint* — the sub-tile band a body
narrower than its tile occupies (measured as the first/last silhouette
column in screen space, `(fx - fy)`) — cannot become an axis-aligned
`occlusion::Solid` box the way a panel's or a tread's strip can: the
measured band is a **diagonal** stripe of the tile in world coordinates
(`u - v`), and `Solid` is never rotated. This is a genuinely open gap, not
merely unauthored code — see "Status".

**The occluding primitive stays a box by default.** A general mesh occluder
for shapes a box (or several composed boxes) cannot state — a curved roof,
a mountain's slope — is being added in
[`lighting_geometry.md`](lighting_geometry.md); the box described in this
section remains the default and the free case for everything it already
covers (a lid, a body, a tread, a footprint band). Land is not yet in this
occlusion grid at all: only statics occlude, so a hill between a campfire
and a valley stops nothing today (see "Status").

## The shadow ray walk

A shadow ray is a grid traversal (a DDA), visiting exactly the cells it
crosses and computing, per cell, how long the crossing is and what fraction
of the flame survives it. Three different rules apply depending on what a
cell holds, and the larger of two answers wins where a cell could be asked
both:

- **A panel is *pierced at a point*, not travelled through.** What it does
  to a ray is decided at the single point and height the ray crosses its
  plane — the tile's `z` span answers yes or no there. There is no length
  in it, which is what makes a named panel's shadow edge exact at any
  angle, rather than following the DDA's cell boundaries in a staircase.
  The vertical penumbra that survives (a ray grazing the top of a wall is
  dimmed, not switched) has width `FLAME_SPREAD * t / (1 - t)` (similar
  triangles from how far along the ray, in `t`, the crossing is), capped
  below one tile so a wall crossed squarely still stops *all* the light.
- **A body (`EDGE_ANY`, no named side) is *travelled through*, scaled by
  the length of the crossing** over the same softening width — the rule a
  slab (a roof five `z` deep) needs, because a 45° ray can enter and leave
  a slab's cell without ever piercing either vertical side inside its
  span. A body is *also* pierce-tested on whichever of its sides the ray
  actually crosses, and the larger of the length-answer and the pierce-answer
  wins — a house corner (no named face, so `EDGE_ANY`) that a ray only
  clips for a sliver would otherwise leak through the gap between "the
  length rule barely registers" and "there is no named panel to pierce."
- **A lid (a horizontal plane, zero-height panel) is *crossed*, strictly.**
  Not pierced (a pierce is a point on a *vertical* plane at a height; a lid
  has no height to be pierced at) and not travelled through by length (a
  floor is typically `height 0`, so a length rule stops nothing). The test
  is whether the ray got from one side of the plane to the other inside the
  cell, with the softness measured **at the flame** rather than at the
  surface — a flame standing exactly in a lid's plane is half cut by it, one
  a full storey below it is wholly under it. The crossing is strict: a ray
  running exactly along the top of a lid (a candle standing on the floor it
  lights) has crossed nothing, or every room lit from inside it would carry
  half a floor's shadow.
- **A corner (a ray crossing where two cell boundaries meet at once) is a
  supercover walk, not a diagonal skip.** Where the two boundary crossings
  land together within a tolerance derived from `PANEL_THICKNESS`
  (`corner_tie`, `light.rs` and its shader mirror), the walk asks *both*
  cells sharing the corner, at the height the ray passes through it, before
  stepping past them diagonally — two extra samples on the rays that hit a
  corner exactly, not twice the samples everywhere. The exact numerical
  behaviour of this tie-break, and every boundary-precision bug found in
  it, is [`lighting_raymarch.md`](lighting_raymarch.md)'s subject, not
  repeated here.

**Self-shadowing exemptions.** Neither end of a ray is shadowed by the tile
it starts or ends on, but *which* cell counts as "the tile it is on" is
asked of the **surface**, not the whole tile:

- A face or an `Upright` billboard pixel exempts its own cell — its pixel
  lies *on* the panel it is a face of (or, for `Upright`, is inside its own
  tile), so that panel cannot be between it and anything. A **flat**
  (floor) pixel on the same tile is *not* exempt: it is inside the room,
  and the ray from it to a lamp in the street genuinely crosses the panel
  its own tile stands on.
- Only a **named** panel exempts — a whole-tile `EDGE_ANY` body (a tree, a
  post, an unread wall) is not exempted by any pixel on its tile, because
  there is no *measured* fact backing an exemption there, only a fallback.
- A run of same-facing panels (a straight wall) does not shadow itself:
  `own_run` treats a panel on the same edge, same row/column, as not an
  occluder for a ray ending on another panel of that same run — a
  perpendicular panel (a corner's other face) still occludes as normal.
- An exemption reaches only as high as the surface it is about
  (`on_surface`): a two-storey wall tile carries a wall per storey, and a
  pixel on the upper one is not exempted by the lower one standing under
  its feet.
- A mounted flame (a wall sconce) is placed **outside** the plane of the
  panel it is bolted to before the walk ever sees it (`light::mounted_at`
  — half a tile plus a small edge margin past the plane, on the side the
  art is drawn from), rather than being exempted from its own wall by a
  special case. This both lights the wall it hangs on at full strength and
  stops it lighting the room behind that wall — a flame on a tile with no
  panel (an ordinary torch or a street lamp) is left where it is, since
  there is no plane to move it off.
- Two ends of the ray are nudged off exactly what they are drawn on before
  the walk floors them into a cell (`stand_clear`): a face pixel a hair in
  front of its own plane, and every point a hair above whatever surface it
  lies on — both needed together, since either alone still lets the walk
  misread a ray running exactly along a boundary plane.

**Corner occlusion (two named faces on one tile) works like a panel pair,
not a body.** Since a corner's `Stance` resolves to two named edges (see
"The G-buffer bridge"), its cell mask carries the corresponding two edge
bits rather than `EDGE_ANY` — a ray running alongside a corner tile, down
the street it faces, crosses neither of its two panels and passes, exactly
as it does beside an ordinary run of wall. A free-standing pillar filling
its own tile reads identically to a building corner from its silhouette
alone (both are "two faces of a box"), so it is shaded correctly as two
faces but occluded slightly too generously at its far corner — a ray
clipping a pillar's far corner passes where a whole-tile body would have
stopped it (see "Status").

## Sunlight

The sun is a direction, not a position: an azimuth and elevation (in tile
units), walked the same grid as a flame's ray but with no endpoint — every
fragment walks the same direction until the ray leaves the grid or is
stopped. This is the same walk described above (both rays share one
implementation, `skip_last`/`spread` parameterizing the flame-specific
parts), which is also what gives the sun's ray the panel-pierce test rather
than a coarser point sample: the walk climbs `Z_STEP`-scaled `z` per tile of
travel and steps through occluder cells exactly as a point light's ray
does, rather than sampling only at tile centres (which used to let a 45°
noon ray step clean over the top of a wall without ever crossing its
plane). `Occlusion::tallest` bounds the walk — the ray stops once it is
above the tallest occluder the grid holds, two or three steps over open
ground rather than the grid's full width.

A window passes some sunlight rather than being opaque: `occlusion::PANE`'s
four-fifths-stopped rule applies to the sun exactly as it does to a flame.
The visible shaft of light between a window and the lit floor behind it is
not drawn as geometry — nothing in this renderer draws air — it is a
screen-space blur of the sunlit mask along the sun's own screen-space
direction, and it is **not yet built** (see "Status"; this is the same
"screen-space glow" idea as a flame's halo, below, applied to the sun's own
mask instead of a point light's). The sun has **no facing test**: a wall's
two faces both read as lit under the sun regardless of which one actually
faces it, because `sunlight`'s walk never consults the surface normal the
way a point light's does (see the facing test below) — this is a known,
current gap.

F8 toggles the sun; it is off by default, not because of cost (measured at
0.057ms of a roughly 0.29ms lighting pass at the widest zoom) but because
nothing yet asks for it to default on.

## Point lights: falloff, beam, ambient, and the screen-space glow

**Distance is three-dimensional.** `Z_PER_TILE = TILE_WIDTH / Z_STEP = 11`
(`44 / 4`): one tile's width in `z` units, matching the projection's own
ratio, so a flame reaches as far up and down as it reaches sideways — this
is what keeps a cellar from lighting the street even where nothing
occludes.

**A carried flame is a cone, not an omnidirectional pool.** `light::Beam`
carries an axis and the cosine of a half-angle
(`HELD_BEAM_DEGREES = 60.0`); the pool a light casts is multiplied by how
far inside that cone the lit spot is, checked *after* the ordinary radius
test so a fragment outside the radius never pays for the angle comparison.
The rim softens over `BEAM_EDGE = 0.25` of the way in from the cone's edge
(a hard cutoff reads as a stencil, not light), and `BEAM_SPILL = 0.25` of
the flame escapes the cone in every other direction — without it, the one
thing the carrying player is looking at (their own body, on the flame's own
tile) would be the only unlit shape in the frame. `light::carried` builds
the held light from the player's own tile and facing (the only direction
this pass gets "for free," since facing is already on the wire for every
mobile); it is added to the frame after the ordinary map-sourced lights are
collected, and F7 toggles it (on by default, no visible effect in daylight
where the whole pass is a copy). Only the local player's own carried flame
exists — no other mobile's held item casts light (see "Status").

**The ambient is one flat colour per frame by default**, not the sky field
`lighting_world.md` can compute (a room under a roof darker than the street
outside it, before anything burns) — that field is off by default (F6):
`light::Ambient::flattened` sums the split ambient back into the single
term this pass had before the field existed, because judging a point
light's falloff wants one thing changing in the picture at a time, and the
field is the larger visual signal in a lit frame if left on.

**The screen-space glow (the halo around a flame's own sprite) is a
planned second layer that has not been built.** The world-space pass
described above answers *which surfaces are lit* — it cannot draw the flame
itself, because nothing in this renderer draws air, and a fire's brightness
is not a property of the ground under it. The intended design: a soft
radial falloff centred on the flame's own **screen** position (not its
tile — the sprite's position is what a person's eye expects the glow
centred on), *added* over the finished frame rather than multiplied into
it (multiplying would tint a black pixel and keep it black, which a halo
over a dark doorway must not do). This needs the flame's own viewport
position and a radius, one more `vec4` per light in the uniform, alongside
open questions about whether the halo should be occluded by anything
between the eye and the source. Not implemented (see "Status").

## Doors

An open door is a static that has stopped being an occluder, not a special
case in the walk. `tiledata.mul` does not record whether a door graphic is
open or shut (the flags of a matched open/shut pair are identical across
all measured ServUO door families) and no reliable geometric signal
distinguishes them either, so a lookup table is authoritative:
`crates/client/render/src/doors.rs` and `data/doors.json` port ServUO's own
thirteen door families (sixteen graphics each, even offset shut, odd
offset open — `base(closed + 2 * facing, closed + 1 + 2 * facing, ...)`),
and `occlusion::opacity` consults the graphic through this table **before**
falling back to the tiledata flag test. `server/world/src/doorgen.rs` ports
the same `+ 2 * facing` rule independently for shard-generated doors — two
copies, since `client/*` and `server/*` never depend on each other and a
table of client art indices is not something both ends of the wire agree
on. A graphic the table does not recognise keeps ordinary flag-based
behaviour exactly — a shard's own custom door goes on occluding, rather
than a wrong guess opening a room to the street.

An open leaf's own art also shades correctly without any door-specific
logic: an open door's silhouette sits on a tile edge as squarely as a plain
wall (median distance zero across measured graphics, none over two
pixels), on the **perpendicular** edge from the shut leaf's edge in every
measured pair — so the facing/occlusion machinery in "The shadow ray walk"
above handles a swung-open door for free once its edge is measured, with no
"door" concept in the geometry at all.

## The art-measurement pipeline

The client's `.mul`/`.uop` art files carry no structured record of which
edge of its tile a wall stands on, where a window's glass is, or what shape
a stair's steps are — every one of those facts is read out of the drawn
picture itself, once per graphic, and cached in a table rather than
re-derived at runtime.

**Facing (which edge a wall stands on).** `facing::facing_of`
(`crates/client/render/src/facing.rs`) reads a wall sprite's **base
edge** — the lowest drawn pixel of each column, the one part of a wall's
silhouette with no ornament — and derives two independent bits: which axis
the base runs along, and which half of the tile's column holds the mass.
Together those give one of four faces (`N`/`E`/`S`/`W`) or, if *both*
halves independently pass the single-face test and each was refused only
because the *other* half held more than a wall's own thickness, a
**corner** (two faces). The art only ever draws the two faces (`E`/`S`) an
isometric camera can see the face of — north/west graphics are a handful
out of over a thousand wall statics, kept in the model because the geometry
has four edges even though the art rarely uses two of them. A wall's own
measured thickness leaves a sliver of art past the tile's centre column
(`facing::OVERHANG`, 2 pixels tolerance; the confound is that a wall low
enough to look down on also shows its **top** surface in that same sliver
via `facing::SPILL`, 12 pixels — the two are not yet told apart by
measurement, only by the `OVERHANG` gate). `facing_of` refuses (falls back
to the whole-tile `EDGE_ANY` body) rather than guess on anything it cannot
read cleanly: a slab with no vertical rise, a base line landing more than a
few pixels from where the geometry predicts, a picture with no clean single
run.

**Aperture (a window's hole).** `facing::aperture_of` finds the largest
rectangle inscribed in a window graphic's transparent region (searched over
every sub-run of columns, `O(n²)` over at most 22 of them) — a bounding box
would let light through stone the artist drew, since UO's windows are
typically arched. Held in the surface's own coordinates (`v` along the run,
`z` above the static's own base — `facing::Hole`), placed absolutely via
`Aperture::above` once the instance's own `z` is known. Gated against a
handful of refusals: no wall either side of the hole along the run
(`HOLE_MARGIN`), more than one gap in a column (a lattice, not a
rectangle), a corner (refused outright — nothing in a silhouette says which
of the two faces a hole belongs to), anything under three columns by two
`z` (noise in the art). A **leaded/lattice window (multiple mullions) is
refused entirely** rather than measured as one merged or one chosen
rectangle — the conservative direction, but the wrong answer for an
ordinary window shape (see "Status").

**Footprint (a body narrower than its tile).** `facing::footprint_of`
measures the first and last silhouette column with a drawn pixel, mapped to
the tile's diamond and quantised — `None` for anything reaching both
corners (a full-width graphic, the overwhelming majority). See "The
occluding world" above for why the measured band cannot yet become a
`Solid` box.

**Climb profile (a stair).** `facing::Prism` is a height field over the
tile that varies along one named axis (`up`) and is constant across the
other — `treads: [f32; N]` plus a count, the same fixed-array shape
`facing::Blocks` uses so a `Shape` carrying one stays `Copy`.
`facing::best_prism` searches candidate profiles against a sprite's
silhouette (intersection over union, aligned by the bottom row and centre
column, no free placement parameter) and is only trusted where the
client's own `TileFlags::CLIMBABLE` bit is set *and* the score clears a
gate (`PRISM_FITS = 0.9`) — a wall that is not a prism at all still scores
around 0.81 against the best candidate, so the flag is what admits a prism
reading, the score is what confirms it. `facing::Prism::height_at(run)` is
monotonic by construction (that is what makes it a *climb*, and also why a
`Prism` cannot state an arch's post-gap-post discontinuity — see `Blocks`
under "The occluding world"). `tests/author.rs` scores/draws both `Prism`
and `Blocks` candidates against the real sprite via one shared instrument
(`facing::silhouettes_agree`).

**The tool, the table, and staleness.** All of the above ran inline in the
render loop, on the frame a graphic was first packed into the sprite
atlas, until it was moved into a standalone offline tool:
`crates/client/artscan` walks an installed client once and writes a table
beside it (`crates/client/render/src/arttable.rs` holds the table type and
its text grammar; the tool itself lives in `artscan`, since `client/render`
never opens a file). A stock install with no table works exactly as before
— the client re-derives per-graphic on first sight and says so in a log
line — it is a speed and search-ambition improvement, not a hard
dependency: the pass refuses to *require* the table the same way it
refuses to guess at unmeasured art elsewhere. A hand-authored row
(marked `authored`) always wins over a derived one and is never
overwritten by re-derivation; `data/overrides.table` is what this
repository ships, checked in with the tool, holding **no rows today** — the
generated table itself is never checked in (derived from copyrighted art).
The table's own staleness key is the art container's file name and byte
length plus a `facing::DETECTOR` version bumped whenever a measurement rule
changes; a mismatched key causes a full re-derivation rather than trusting
stale data. The row format version (`arttable::FORMAT`, currently `4`) is
bumped whenever the row grammar itself grows a field, and an
older-format table is refused outright rather than half-read.

## Solids as drawable geometry

A debug view exists that draws the occlusion grid's own solids as real,
lit boxes in the scene — `crates/client/render/src/solids.rs` and
`solids.wgsl` — rather than as a wireframe stroke. It is a genuine render
pass (`render`'s tests take pictures headless, so anything drawn only by
the client's UI toolkit could never appear in a `tests/cost.rs` dump), not
an overlay: translucent, drawn over the finished frame, writing no depth,
so a static's own sprite stays visible *inside* the box that claims to
contain it and the box's top face makes its thickness legible. Toggled
with F5 in the client or `--solids` on the offline viewer (`--at X,Y`
opens the viewer at a named world point).

Geometry stays entirely in Rust (`Solid::faces`, `Solid::outline`) rather
than being reconstructed in the vertex shader from two corners — a second
implementation of the same projection arithmetic every sprite in the frame
is placed by is exactly what `statics.wesl` already refuses to do for
depth, for the same reason. `Camera::project_exact`/`WorldSpot` give a
continuous place between whole tiles (`project` delegates to it at a whole
tile); `WorldSpot` is deliberately the tile **corner** lattice, not the
centre lattice `project` returns for a `Point` — a box's extent, stated in
the same tile numbers used elsewhere, would otherwise land half a tile off
in both ground axes.

**Depth ordering for a multi-tile solid is not fully solved.** The client's
own draw order (`depth::Order`, `(x + y, priority_z)`) is per-*instance*
and discrete; a box spanning several tiles has one instance depth but
several tiles' worth of geometry under it. Today's answer is the cheap one:
translucent, over the whole frame, writing no depth — correct for a
diagnostic (the sprite inside the box stays visible), wrong for a solid
that should be occluded by ordinary sprites drawn in front of it. A
per-fragment depth computed through the same `depth::Order` (keyed off the
fragment's own world point rather than a new formula) is the other known
answer, not yet built.

**Only three faces are ever drawn** (`+x`, `+y`, and the top) — with no
rotation anywhere in this renderer, an axis-aligned box always shows
exactly those three, so this is an instanced quad pass shaped like the
`statics` pass (six numbers and a colour per instance, corners emitted in
the vertex shader), never a mesh pipeline with index buffers or
back-face culling.

**A second-order view control, `solid::Cut`, decides what a view of the
grid is a picture of.** Two values: `BelowFeet(z)` (what could shadow
someone standing at this height — the default, and why a pier's two
thousand floor-slab occluders do not make the wireframe or the solids
view unreadable) and `Nothing` (the whole grid, unfiltered — deliberately
unreadable in a town, useful for one tile at a time). Resolved once per
frame and never stored, governing both the wireframe overlay and the
solids pass together (F4), because the two are read against each other and
cannot be compared if cut differently. A third value ("this storey") is
deliberately absent — it needs a notion of *ceiling*, which nothing in this
world states yet.

**Cost, measured at the widest zoom over Britain: 3.61ms a frame** drawing
3,768 of the grid's 16,729 boxes (the rest fall outside the clip rectangle
and are dropped before a vertex is written), against 0.34ms for the whole
lighting pass on the same frame and 0.18ms for a plain blit. It stays a
debug view for that reason — most of the 3.61ms is the pass's own vertex
buffer being rebuilt and re-uploaded from the CPU every frame (kept there
deliberately; the fix, if the cost is ever worth removing, is the same one
the occlusion grid itself already took: keep the buffer and rebuild only
when the camera moves).

## Testing and instrumentation

**Scenes are built, not loaded.** `crates/client/render/src/scene.rs` holds
a library of hand-built rooms (a closed room, a doorway, a window, a sconce
on a wall, a cellar under a street, a staircase, and more) — each a
synthetic `WorldMap`, `TileData` and item list this workspace constructs from
nothing, with no client files. This is not a concession to the
no-client-files rule: it is more precise than a real house, since a test
can say exactly *which* cell should have stopped a given ray, and a failing
test can print the room rather than a coordinate.

**One scene is the reference, and its numbers are the baseline.**
`examples/boxes.rs`'s `tree` — two boxes on one tile, the lower a half-tile
footprint, the upper a third-tile footprint standing on the lower's own lid —
at *its own defaults* (`H1=3 H2=3 W1=0.5 W2=0.33333`, the top of the zoom
ladder, the flame up and to the boxes' `+x` side) is the scene this workspace
looks at lighting through. It is the smallest scene that states all three
things a whole-tile box cannot: a footprint narrower than the tile bucket that
owns it, one solid standing on another's lid, and a vertical face whose height
varies continuously down it. Its four oracles at those defaults report

| oracle | disagreeing / drawn pixels |
|---|---|
| face (both boxes, `east` + `south`) | 18 / 7008 |
| ground | 226 / 252105 |
| box 0's own top | 0 / 9216 |
| box 1's own top | 0 / 9216 |

**A run that reports different numbers with no intended cause is a regression
report** — read it before reading anything else. Ask the scene a question by
overriding a knob for that run; overriding a *default* silently retires every
number above, so don't.

**A second scene, for the one thing `tree` cannot show.**
`OPENSHARD_BOXES_SCENE=pair` stands two boxes of one height *side by side* on
one tile, on the tile's own diagonal, with the flame on the line through both
centres and beyond the near one. Where `tree`'s two boxes meet at a plane, these
two span the same heights outright — so every fragment of either is inside both
spans, which is exactly what "is this solid the one the fragment is drawn from"
used to be answered from. It is `lighting_height.md` phase 3's own fixture and
it was fully red before that phase:

| oracle, `pair` | before phase 3 | now |
|---|---|---|
| box 0's `east` face | 1296 / 1296 | **0** |
| box 0's `south` face | 1248 / 1248 | **0** |
| box 0's own top | 9216 / 9216 | **0** |
| box 1 (the near one) | 0 | 0 |
| ground | 147 / 254248 | 147 — the same tangent floor `tree` has |

What is left is two named things, and **neither is `exemption`**, which these
numbers were read as meaning for two sessions — see `lighting_height.md`'s own
account of how that happened. Both are measured, not argued: setting
`STAND_OFF` and `ON_TOP` to zero on *both* walks for one run is what separates
them.

- **The stand-off nudge, at a grazing corner.** Every one of the face oracle's
  18 (`0/7008` with the nudge zeroed) and 89 of the ground's. A ray is walked
  from a fifty-eighth of a tile in front of the plane its fragment is drawn on
  and a hundred-and-twenty-eighth of a `z` unit above whatever it lies on —
  decision 26's own pair, without which a wall wears a bright stroke along its
  floorboards. Where the ray then grazes a corner, that nudge is the whole
  answer, and the engine is honestly answering about a point a hair from the
  one the independent oracle asks about.
- **An exact tangent at a box's own corner.** The remaining 137 ground pixels,
  a diagonal line along the shadow's own corner: the ray touches the box at
  exactly one point, `t` in and `t` out equal, and whether a zero-length
  crossing blocks is a definition rather than a fact — `light::ray_vs_solid`'s
  own doc comment says as much and leaves it to the caller. The oracle counts
  it as blocked and the walk does not.

**Both were buried under an instrument that was wrong by an order of
magnitude**, and the record of that is worth as much as the numbers. The face
oracle reported 278 and the ground oracle 509 at their old sampling; 212 of the
first were pixels *other surfaces* drew, read as the face's own, and most of
the rest of both were a shader tolerance sized a hundredth of a ray rather than
a rounding. See `a4b698c` and `ccca681` — and note that neither error was
visible as anything but a plausible number until the oracles were asked what
they had actually compared.

**CPU/GPU parity is enforced, not assumed.** `light::sample`
(`crates/client/render/src/light.rs`) is the shader's ray walk re-implemented
on the CPU, returning the *reasons* a shader alone cannot produce (which
flame, how far, what survived the walk, which cell stopped it). It exists
because "why is this pixel lit" needs a list, and a rendered picture cannot
be a list — but a second implementation of the same formula only earns its
keep if the two are actually held together: `tests/frame.rs` uploads a
synthetic place attachment, runs the real blit shader over it, and asserts
every sampled pixel agrees with `light::sample` fed the same input. The
parity test is what makes the CPU implementation trustworthy as a debugging
oracle rather than a second, silently-diverging guess.

**Debug views are branches of the one blit shader, not a second render
pipeline.** `debug::View` (`crates/client/render/src/debug.rs`) is one field
in the lighting uniform and a `switch` at the end of the fragment shader,
so every view answers about the *actual* values the lit frame was built
from: the place attachment, the occlusion grid, distance and walk survival
per flame. F11 in the client cycles them; `View::ALL`'s **index** (not the
shader's own raw view discriminant) is what environment variables like
`OPENSHARD_FRAME_VIEW` select — the two numberings differ. There is also a
standalone **plan view** (`render/src/plan.rs`) that draws the real blit
over a synthetic flat-ground attachment, one screen-space square per tile,
with every occluding cell's panel and every flame's radius stroked on top
(`Picture::mark`) — the shape a light casts is otherwise hard to judge from
an isometric frame alone, and an **elevation** unroll
(`plan::elevation`) does the same for one run of wall face, unrolled flat
so a seam artefact shows as a vertical stroke at a specific point along the
run. Neither view computes lighting itself — both are the real shader run
over a synthetic input, for the same reason the parity test insists on one
formula.

## Status

Built and in the live render path:
- World-coordinate lighting with per-pixel place attachment (all three
  world passes), the occlusion grid described above (solid-referencing,
  block-baked, spill/ring for wide solids), the shadow ray walk with panel/
  body/lid/corner rules and all listed self-shadow exemptions, sunlight as a
  full grid walk with window transmission, carried-flame beam/cone lighting
  for the local player only, flattened (non-field) ambient by default,
  door open/shut occlusion via the ported table, CPU/GPU parity testing,
  and the debug view/plan/elevation instrumentation.
- The art-measurement pipeline (facing, aperture, footprint, climb profile)
  as an offline tool + cached table, with staleness detection and
  authored-row override.
- The solids diagnostic view (F5), as a debug-only pass.

Not yet implemented:
- The screen-space glow layer (a flame's own halo) — no code exists for it
  yet; see "Point lights" above.
- The screen-space shaft for a sunbeam through a window (the equivalent
  glow idea applied to the sun's own lit mask).
- The sun's facing test — a wall's shaded side is not distinguished under
  sunlight the way it is under a point light.
- Light carried by any mobile other than the local player.
- `Builder::add` consuming an authored `Blocks` list — nothing wires an
  authored arch/lintel into the live occlusion grid yet, even though the
  table format supports it and `tests/author.rs` can score a candidate
  against the art.
- A `Solid` box for a body's sub-tile footprint — the measured band is a
  diagonal stripe in world coordinates that the current axis-aligned
  `Solid` cannot state; needs either a lossy axis-aligned approximation, a
  new non-axis-aligned primitive, or a different measurement that anchors
  the band to a nearby wall's axis.
- Leaded/lattice window apertures — refused outright rather than measured.
- Land (terrain slope) as an occluder — only statics occlude today, so a
  hill casts no shadow.
- Any shadow cast by a mobile standing between a light and a wall (matches
  the reference client's own behaviour, not treated as a defect).

Deferred, pending something else:
- The occluding primitive generalizing from box to mesh for curved/sloped
  shapes a box cannot state — scoped in `lighting_geometry.md`, which the
  box-vs-mesh reasoning here explicitly defers to.
- The render-side per-face normal replacing the fixed-tag `Stance` enum for
  solids generally (only stair treads have had this attempted, and it was
  retired in favor of `gbuffer.md`'s real per-face geometry) — tracked in
  `gbuffer.md`.
- A pillar's far corner occluding as generously as a building corner does
  (both read identically from silhouette alone; only the surrounding map
  — open ground vs. an interior — tells them apart, and nothing consults
  that yet).
- A stated (non-zero) wall thickness beyond `PANEL_THICKNESS`'s current
  nominal value — art has a derivable-but-unverified signal for it (a
  measured sliver of "top of wall" art past the centre column), not yet
  used to fit a real value against the sprite.
