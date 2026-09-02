# Lighting, part two: session log, decision reasoning, and backlog archive

Companion to [`lighting_world.md`](lighting_world.md) — that file is the
current-state reference (what the ambient/sky-field system computes today,
its data format, its measured costs, what's built versus still a plan); this
file is the full reasoning behind it: every decision's argued case and
rejected alternatives, the numbered steps as they were written, and the
complete backlog (including entries later settled or fixed, kept for the
reasoning rather than only the outcome).

Nothing below was rewritten for style — it is `lighting_world.md`'s old body,
relocated and grouped under headings that mirror the current file's
sections, with decision and step numbers kept exactly as they were so a
reference to "decision 9" or "step 8" from another document still resolves
to real text. Where a topic doesn't map cleanly onto one heading below, the
full passage lives under the section its main subject most belongs to, with
a note left for readers arriving from an adjacent topic.

Read [`lighting_world.md`](lighting_world.md) first for what is actually
true today. Come here for *why* — what was argued, what was tried and
abandoned, and the numbers behind a claim that later changed (a "settled" or
"fixed" entry below is a backlog item whose question the record shows was
actually answered, not a currently open one — see the current file's own
Status section for what stands today).

## Overview

*(originally the whole of the old file's introduction, its "Where the next
session starts" section, its Nox comparison, its scope note, and its seam
with `lighting.md` — kept together since all four were one continuous
argument for why this plan exists and how it relates to its sibling)*

A living plan, in the shape the others here have: the decisions numbered so
one can be argued with alone, the steps, and a backlog. It stands on
[`lighting.md`](lighting.md), which is where the pass moved out of the
screen and into the world and learned that a wall stops a flame. That is the
hard half and it is built. This is the other half, and its subject is not
shadows — it is **where the light in a frame comes from when nothing is
burning**.

### Where the next session starts

**Steps 1 and 2 are built: a roofed room is darker than the street outside
it, in the shader and in `light::sample` alike.** `View::Sky` is what it is
looked at with.

**Step 3 is next, and it is the one that makes any of this visible in the
client.** The reason is in the backlog below: the ordinary daylit frame is
`Lighting::NONE`, which carries no grid at all, so today the split only
reaches the eye at night or with the sun key held. The day curve is what
makes *every* frame a lit frame, and until it lands step 2 is a thing the
tests can see and a player mostly cannot.

Steps 9 and 10 are blocked on [`lighting.md`](lighting.md)'s steps 15 and 16
and should not be started before them. **Step 8 is no longer blocked**: its
step 14 is built — the occlusion grid is drawn as boxes, under its own
checkbox in the Tile panel — so a soft body is now a thing that can be
looked at while it walks, which decision 9 says is the only way it can be
judged at all.

Step 7, the tonal response, is the one to take when the appetite is for a
screenshot rather than for a subsystem: it touches one shader, it is judged
by eye against a before/after pair, and it is independent of everything else
here.

*(Note for a reader arriving from the current file's "Status": by the time
`lighting_world.md` was rewritten, `lighting.md`'s steps 15 and 16 — the
facing and aperture measurements decisions 9 and 10 below were blocked
on — had themselves landed and are described as built in `lighting.md`'s own
"The art-measurement pipeline". That un-blocks decisions 9, 13 and 14 below;
none of the three had actually been started as of this writing.)*

### The thing worth copying

Nox (Westwood, 2000) is the isometric game whose lighting still reads as
right, and it is worth being exact about *why*, because the obvious answer
is wrong. The obvious answer is "shadows": light stopped at walls, beams
came out of doorways. We already do that, per fragment, with a real grid
traversal — strictly more than Nox, which flood-filled a per-vertex lightmap
once and interpolated it.

What Nox actually had that this client does not:

- **A room was darker than the street**, with nothing in the room and
  nothing on the street. Not because of a "dungeon flag" — because a roof is
  between the floor and the sky. Walking through a door was a change in
  light, and that one fact is most of the atmosphere.
- **Light moved.** Your torch, a fireball in flight, a burning corpse, a
  spell. The pool travelled with the thing, smoothly, and the world changed
  around it. Ours is nailed to map statics and to items lying on the ground.
- **A fire was the brightest thing in the frame.** Emitters were not subject
  to the darkness they were dispelling.
- **The picture had one tonal response.** Dark was blue and detailless,
  light was warm and blew out at the centre, and the two met on a curve.
  Ours multiplies the art by a colour, which can only ever darken, and clips
  whichever channel the ambient is poorest in.

None of these is a shadow. All four are about the light a place has before
anything happens in it — which is why this is a second plan and not a step
in the first one.

**Out of scope, deliberately.** Everything is computed on the client; the
server is not asked for a single new byte and `0x4F`/`0x4E` remain the whole
protocol surface (decision 3 says what that costs). The raggedness of a lit
wall — the per-pixel fraction along an upright sprite's face — is
[`lighting.md`](lighting.md)'s decisions 13 and 14 and its steps 15 and 16,
and is being worked elsewhere; nothing here should touch `statics.wgsl`.

*(Note: `0x4E` is named here as a second protocol surface alongside `0x4F`
on the assumption a distinct "personal light level" opcode exists in this
codebase. It does not — only `0x4F` is implemented; see "Emitters" below and
the current file's own correction.)*

**Also out of scope, and worth naming rather than leaving implicit:**
whether the occluder underneath this plan's sky field is a box or a mesh.
Decision 1 below computes the field by walking whatever occludes a column,
box or mesh alike — [`lighting_geometry.md`](lighting_geometry.md) is where
that choice is made, and this plan's own backlog carries the one open
question it leaves for here: whether a sloped mesh roof still gives the
column test a clean single-bit answer.

*(Added 2026-08-07, and carried into the current file's own Overview and
Status sections as a standing fact and open question rather than a dated
note.)*

### Where this meets the flame plan

[`lighting.md`](lighting.md) is not finished — its steps 14 (the occluder
boxes, drawn), 15 (a wall's facing measured from its art), 16 (the window's
aperture) and 6 (the measurement) are open, and three of the four are
load-bearing here. The seam, stated once so neither plan has to guess at the
other:

- **Step 14 is this plan's instrument too, and it is built.** The boxes are
  the grid drawn as wireframe in `shell::draw_occluders`, and everything
  below adds to that same grid: a sky byte per cell (decision 1), a body
  that moves through it (decision 9). Neither gets its own visualiser — that
  is [`lighting.md`](lighting.md)'s decision 8, and it holds here: a second
  copy of the unpacking answers about its copy of the frame.
- **Step 15 is what a wall's ambient waits for** — decision 13 below.
- **Step 16 is what a lit room at noon waits for** — decision 14 below.
- **One widening of the occlusion cell, not three — and it is made.** The
  cell is `Rgba8Uint` and full; step 16 needs room for its aperture, the sky
  byte needed room, and a soft body's opacity wants a fifth answer. One
  format decision with three callers, taken by step 1 because it landed
  first: a **second plane** over the same rectangle, `(sky, aperture, body,
  unused)`, rather than a wider cell. The occluder cell is what a ray walks
  through, cell after cell in a loop; the plane is what a tile *is*, read
  once. Step 16 writes the second channel and nothing about the first moves.
- **Step 6's measurement gates both.** Nothing here turns on by default
  before the number the other plan owes.

*(As of the current file's own writing, steps 15 and 16 have landed —
`lighting.md`'s facing and aperture measurement are both built — so decisions
9, 13 and 14 below are unblocked even though none of the three has been
started.)*

## The sky field

**1. A tile that cannot see the sky does not get the sky's light.**
The ambient is one colour for the whole frame today, so the inside of a
house is lit exactly as brightly as the street outside it, and a dungeon is
dark only because the server said the whole world was. Split the ambient in
two:

```
ambient(tile) = SKYLIGHT * sky(tile) * daylight  +  GROUND_AMBIENT
```

`sky(tile)` in `0..=1` is how much of the sky that tile can see;
`GROUND_AMBIENT` is a small, cold floor that a windowless cellar still gets,
so that a room with no torch in it is deep rather than pure black — an unlit
black rectangle is not atmosphere, it is a bug report.

`sky` is the cheapest question the occlusion grid can be asked: a column
test. Anything opaque standing over the tile above its floor takes the sky
away. The grid already carries a `z` span and an opacity per tile, so a roof
is an occluder that happens to be overhead, and the answer is a byte.

**2. The sky term is blurred by a tile, and that blur is the doorway.**
A raw column test steps from 1 to 0 at the wall line, and a step is the
artefact this whole track exists to remove. A 3×3 average over the sky
field — one pass, on the CPU, over a grid that is already a few hundred
tiles — makes the threshold of an open door brighter than the middle of the
room and the eave of a roof brighter than under it. It is not a simulation
of anything; it is the shape the right answer has, for one blur of a small
array.

**3. The cutaway takes a roof away from the eye, not from the sun.**
The cutaway removes the storeys the player is not on so that a house has an
inside. If the sky test read the *drawn* set of statics, standing indoors
would delete the roof and flood the room with noon — the player would carry
daylight into every building. So `sky` is computed from the map as it is,
not as it is drawn, and it is the one consumer of the occlusion walk that
ignores `cutaway::shows`.

This is a real inversion of [`lighting.md`](lighting.md)'s decision 4, whose
whole argument is that a static the cutaway removed must not cast a
shadow — and the inversion is right, for a reason worth stating: a *shadow*
is a thing the player would see falling from something that is not there,
and a *missing ambient* is the absence of light from a thing the player
knows is there, because they walked under it. One is an artefact, the other
is the point.

**14 (crude half only — the aperture half is under "A wall's ambient and a
window's sky" below). A window passes sky, not only sun.**
Decision 1 will read a room with four glazed walls as a cellar, because the
sky test is a column and the sky does not come through the roof. That is
right for the roof and wrong for the room: at noon a windowed hall is
*daylit*, and it is daylit by the sky rather than by the disc of the
sun — the sunbeam is a patch on the floor, the daylight is everywhere.

Before [`lighting.md`](lighting.md)'s step 16 (the aperture) lands there is
a cruder version that is still better than nothing and worth having in the
meantime: a cell whose opacity is `PANE` rather than `OPAQUE` passes its
share of the sky to the tile behind it. That is one line in the same column
test, it needs no new data, and it means a chapel is not a crypt.

**15. Nothing here lands ahead of the measurement.**
[`lighting.md`](lighting.md)'s step 6 — a frame time at the widest zoom — is
still open, and three decisions here add per-fragment or per-frame cost (1
and 2 a grid pass, 5 more lights, 8 a curve on every pixel). The number
comes first, and each step states what it cost.

**Step 1. The sky field.** `occlusion.rs` gained a per-tile sky byte from
the un-cut column test of decision 1/3, the `PANE` leak of decision 14, and
the blur of decision 2 — `Occlusion::shade`, `blur_sky` and `sky_at`, built
out of `collect`'s existing walk rather than a second one. The format is a
**second `Rgba8Uint` plane over the same rectangle**, `field_bytes`, whose
channels are `(sky, aperture, body, unused)`: the cell stays what a *ray*
walks through and the plane is what a *tile is*, which is the line the three
callers actually fall on. Decided once, as the seam above asks.

Tested on the built scenes — `roofed_room`, `roofed_room_with_open_door`
and `roofed_room_with_window` are new, and the last two differ from the
first by one graphic each — and on Britain, which is where the assumption
underneath the whole column test is checked: see the backlog.

**The cost**, per decision 15: one land lookup per static in a walk that
already touched it, and one 9-tap pass over the grid — 187x187 tiles at the
widest zoom, on the CPU, once a frame. Nothing per fragment: no shader reads
the plane until step 2.

**Left undone**: the drawn half. The sky byte was to shade the boxes
[`lighting.md`](lighting.md)'s step 14 strokes; step 2 drew the field on
the ground instead, as this plan's own backlog asked, and step 14's boxes —
now built — are strokes coloured by *opacity* and say nothing about the
sky. The two views are read side by side rather than one inside the other,
and the backlog below says why that is the right way round. It is not a
small omission: a field this cheap to compute is a field it is cheap to be
wrong about everywhere at once, and the backlog below says why the
wireframe is the wrong instrument for it anyway.

### Backlog

- **The sky field and the sun are asking one question twice.** Decision 1's
  column test is "can this tile see straight up"; `walk_sun` is "can this
  tile see the sun's direction". At noon they are the same walk with a
  different vector, and a shared traversal would answer both — which is
  also what the other plan's backlog wants for the sun's tile-at-a-time
  stepping. Worth doing when both are built, not before: two callers is
  when the shape of the shared thing is visible.
- **A roof over a courtyard is a lie the map tells.** Some UO houses have
  tiles that are roofed in the art but whose statics do not stand over the
  floor tile — overhangs are drawn on the tile *next* to the one they cover,
  because a static is a picture rising from its own diamond. Decision 1 will
  read those as sky. Whether it matters is a question for a real house, and
  the scene that answers it does not exist yet.
- **~~The occluder wireframe draws tiles, not surfaces.~~ Fixed, and it is
  the view a gap is looked for in.** It drew `Occlusion::boxes()` — the
  *merged* `at()` — which is the world as it was before
  [`lighting.md`](lighting.md)'s step 21.2: a floor and the wall on its tile
  came out as one box from the floor's `z` to the wall's top, two walls with
  a storey of air between them came out as one box through the air, and
  which edge a panel stands on was not in the picture at all. Every gap the
  view is opened to find is a gap between two of those things. It now draws
  each of the walk's three kinds as the shape it is — a lid is one
  horizontal quad, a panel is one vertical quad **on its named edge**, a
  body is the box — and the count beside the checkbox counts surfaces. The
  table between the diamond's corner order and `facing::Face`'s naming is
  derived in a test from `Face::place_at`, which is what the shader places a
  face pixel with, so the wireframe cannot draw a wall on a side its pixels
  are not on.
- **A narrow graphic becomes a whole tile of occluder, and a house's corner
  is one.** Britain's `1509,1635` — the tile that reads as lit when its
  neighbours are dark — carries `0x00CC`: a 44-wide picture whose silhouette
  occupies **columns 12 to 31**, a centred peak, with `0x00DF` above it the
  same shape thirty-three pixels tall (`artscan`'s `shape` example prints
  both). `facing::facing_of` refuses it by its own rule — a picture narrower
  than a tile cannot cover an edge of the half it belongs to — and the
  fallback is `EDGE_ANY`, so twenty columns of art become an occluder across
  the whole square, standing among neighbours that are panels on one edge.
  It over-blocks in every direction at once, and it is what the eye reads as
  the odd tile. The model has no narrow body: a surface is a plane on an
  edge or the whole tile. So this is a decision rather than a fix — what a
  centred picture wants is a body whose *footprint* is the columns it
  covers, which is a second measurement per graphic, a second number per
  surface and a third rule in the walk.
- **A box is drawn for what stands, and the sky is what does not.** The
  wireframe of [`lighting.md`](lighting.md)'s step 14 shows occluders; the
  failure this plan will actually hit is a tile that is *wrongly open* — an
  eave that did not cover the floor under it, a roof whose statics stand one
  tile over. Shading the boxes by the sky byte (step 1) shows the second
  kind only where there is a box at all, so a hole in a roof is invisible in
  the very view meant to find it. The honest instrument is the field drawn
  on the ground, as the terrain overlay already draws a per-tile number, and
  it is worth remembering before adding a third view rather than a second
  use of that one.
- **~~`FLOOR` may be the roof test that already exists.~~ Settled: a real
  roof is already in the grid.** The column test rests on a roof being an
  occluder at all, and membership is `WINDOW | NO_SHOOT` — a fact about
  *arrows*, which nothing said was also a fact about lids. Measured over the
  block of Britain the cutaway's tests walk: **203 roof statics, 203 of them
  `NO_SHOOT`**. So the height comparison stands, no flag lookup is needed,
  and `occlusion::britains_rooms_are_dark_and_its_streets_are_not` is what
  says so if a patch ever changes it. **The wider question that entry was
  really asking — whether an upper storey's floor is a lid for the storey
  below — was answered by measuring it, and the answer is that membership
  was never the problem.** A house's floor plank *is* `NO_SHOOT`: over the
  same block, 2,755 of the 4,647 `FLOOR` statics are opaque, and the open
  ones are rugs, grass and road decals. Every real floor was in the grid all
  along. What it was not was an *occluder*, because it is `height 0` and the
  walk scaled a lid by the length of the ray inside its span —
  [`lighting.md`](lighting.md)'s decision 32, and the storey above a torch
  was lit through its own floorboards until it landed. The sky half of this
  is unaffected: `Builder::shade` reads the same opacity and always did take
  a floor's column away.
- **28 of 2560 tiles the cutaway calls outdoors read dark.** The measurement
  above, in its other direction. Some are wall tiles (the entry below); the
  rest are the overhangs the courtyard entry names, arriving from the side
  that can be counted. Worth re-reading when that scene exists: the number
  is a bound on how wrong the eave case actually is, and it is small.
- **`field_bytes` has three channels nobody writes.** Deliberate — the
  format is decided once, per the seam above — but a plane of zeros
  uploaded every frame is a thing that can be forgotten. Whoever lands step
  16's aperture or step 8's soft body should find them already waiting; if
  neither has landed by the time something else wants the space, that is
  when to reconsider, not before.
- **Nothing in this plan knows about weather.** An overcast sky is exactly
  the sky term of decision 1 multiplied by a number, and rain is the same
  with a colour — which is to say this arrives almost for free once the sky
  field is a field, and it is worth not designing it away in the meantime.

## The two ambients

**Step 2. The two ambients.** `Lighting::ambient` is an `Ambient` — a sky
colour and a ground colour — and `Ambient::at` is the one place the two are
mixed by a tile's sky byte. `blit.wgsl` reads the field plane as a second
grid texture (binding 5, uploaded beside the occluders and never apart from
them) and does the same arithmetic per fragment; `light::sample` reads
`sky_at` at the fragment's own tile. The parity test of the other plan's
decision 9 keeps the two honest, and it gained a **third scene** for it:
`roofed_room`, because every other parity fixture is lit by an ambient that
happens to be uniform over almost all of it, so a shader reading the wrong
plane would still have agreed nearly everywhere. That scene's own sky spread
is asserted before the pixels are compared.

`NIGHT` and `SKYLIGHT` are split so that their two terms **sum to what each
was as one colour**: a street is exactly as bright as it was and the whole
of the visible change is indoors. The new number is `GROUND_AMBIENT`.

The instrument, which is step 1's "left undone" arriving by the route its
own backlog asked for: `View::Sky` draws the field **on the ground** rather
than as shading on the wireframe boxes — a hole in a roof is exactly where
there is no box, and would be invisible in the view meant to find it.

**The cost**, per decision 15: one more `Rgba8Uint` texture of the grid's
own rectangle uploaded per frame (140KB at the widest zoom, doubling what
the pass uploads), and one `textureLoad` plus a multiply-add per fragment.
No walk, no loop.

### Backlog

- **The daylit frame is `Lighting::NONE`, and decision 1 cannot reach it.**
  The client picks between three skies: night, a daylight with a sun, and
  plain daylight — and the third is `Lighting::NONE`, which carries an empty
  grid on purpose, so that the blit is a copy and the frame tests can
  compare it texel for texel. An empty grid is open sky everywhere, so a
  house's inside is lit as brightly as the road in exactly the mode a
  player is in most of the time. This is not a bug in step 2, it is what
  step 3 is for: once the day curve makes every frame carry an ambient, the
  third case stops existing. Whoever lands it should check what happens to
  the copy tests — they want a lighting that is the identity, and
  `Lighting::NONE` will still be it, but the *app* will no longer be a
  caller of it.

  *(This entry describes exactly the fact the current file's "The day
  curve" section states as still true today.)*
- **`Lighting::is_identity` now asks about the grid, and only one test
  calls it.** It has always been the answer to "may the blit skip
  everything", and nothing skips anything on it — the blit multiplies
  unconditionally. Either it should be wired to an early-out or it should
  go; a predicate that only its own test reads is a claim nobody is holding
  to.
- **A wall tile has no sky of its own, and decision 13 is what that
  costs.** A wall shades its own column, so every wall tile reads 0 and the
  ring of a house is as dark as the room inside it. That is invisible today
  — nothing samples the field — and it is exactly the case decision 13
  exists for: at step 2 the outer face of a house will take the ambient of
  a cell that never sees the sun. The fallback until step 15 offers a
  facing is the tile's own cell, which is this zero; whether that reads as
  "wrong" or as "a wall in shadow" is a question for the first screenshot
  of step 2, and it may want the wall's *brightest* neighbour rather than
  its own cell as the interim answer.
- **A wall tile has no sky and the interim answer is still its own cell.**
  The entry above predicted this and step 2 is where it is now visible: at
  noon the outer face of a house takes the ambient of a cell that never
  sees the sun, and a house therefore has a dark ring around it under
  `View::Sky`. It reads as *a wall in shadow* rather than as wrong, which is
  why it was left — but it is the first thing to look at in the first
  screenshot, and decision 13 is the fix.

  *(These last two entries are the same observation from before and after
  step 2 landed; the current file folds both into "The sky field"'s "A wall
  tile has no sky of its own" and "A wall's ambient and a window's sky".)*

## The day curve

**4. Day is a curve with a colour, not a level with a key.**
The server sends `0x4F` as a number from 0 to 31 in steps, and F10 is a
switch between two constants. Neither is a sunset. The client keeps its own
time-of-day scalar, driven by the server's level and **eased towards it over
a few seconds**, and maps it through a ramp that is a colour and not a
brightness: amber at dawn, white at noon, amber and lower at dusk, and at
night the blue that `light::NIGHT` already is. A step in the server's level
then reads as the sun moving rather than as somebody flipping a switch, and
no new packet is needed to get it.

The sun's direction ([`lighting.md`](lighting.md) decision 12) comes from
the same scalar: elevation and azimuth are the curve's other two outputs, so
the shadows on the street turn as the day passes and lengthen into the
evening — for free, because the machine that walks them is built.

**Step 3. The day curve.** A `Daylight` in `light.rs`: the server's `0x4F`
level in, an eased scalar, and out of it the ambient pair *and* the sun's
direction. F10 becomes an override of the scalar rather than a swap of two
constants, so the debug key and the real path are one code path.

*(Not started as of the current file's writing — no `Daylight` type exists
in `light.rs`.)*

## Emitters and the light they carry

**5. Anything that burns carries its light, and it carries it smoothly.**
`light::collect` walks map statics and ground items. It must also walk:

- **mobiles** — an equipped light source on a mobile's layer, which is how
  the reference does it (`GameScene.AddLight(this, item, ...)` from
  `MobileView`), and which is what finally makes a player holding a torch
  light the room;
- **effects** — a spell, a projectile, an explosion, each a light for as
  long as it draws;
- the player, whose torch replaces the "personal light level" fudge: `0x4E`
  is a floor under the darkness, so it brightens the whole screen including
  the far side of a wall. A real light on the player's own tile is the
  honest form of the same intent and gets shadows for nothing.

**Smoothly** is the part that is easy to get wrong. A mobile's *sprite* is
interpolated between tiles; if its light is placed at its tile, the pool
jumps a whole tile at a time while the thing carrying it slides. The light
takes the same interpolated world position the sprite is drawn from — which
the renderer already computes — and the flicker phase comes from the
mobile's serial rather than from its tile, or the pool changes its
character as it walks.

*(By the time the current file was written, the player's own torch had
already been built as a separate mechanism in `lighting.md` — a beam-cone
carried light, added to the frame after `collect` runs, not a plain
omnidirectional point light on the player's tile as described above. The
part of this decision that remains unbuilt is everything else: other
mobiles, and effects. See also the note below on `0x4E`.)*

**6. An emitter is not subject to the dark it dispels.**
A campfire at night is currently art multiplied by a night ambient, plus its
own light at distance zero. If that sum comes out below 1 the fire is a
*dim* fire, which no fire is. The rule: a fragment whose tile hosts an
emitter is lit by at least that emitter's own colour at full intensity — the
multiplier is clamped from below rather than accumulated to. It is one `max`
in the loop, and it is what makes a torch look like a torch instead of like
an orange sprite.

**Step 4. Emitters that move.** `collect` takes mobiles and effects; a
light takes an interpolated position and a serial-derived flicker phase.
The player's torch lights the player's room; `0x4E`'s floor comes out.

**Step 5. Emissive emitters.** Decision 6's clamp, in the shader and in
`sample`, with a night scene whose only subject is that the fire is the
brightest thing in it.

*(Neither step has been started. See the current file's own correction on
`0x4E`: no such opcode is implemented in this codebase's protocol crate —
only `0x4F`, "overall light level", exists, and Night Sight is implemented
by resending the caster a personal copy of that same packet rather than by
a distinct personal-light signal.)*

### Backlog

- **The personal light level has a second meaning.** `0x4E` is also how a
  shard says "this player has night sight" — a spell, an item. Replacing it
  with a torch on the player's tile (decision 5) is right for the torch and
  wrong for night sight, which is not a light at all but a change to how
  dark the dark is *for one viewer*. Both want to exist: a source, and a
  floor under the ambient.

  *(This entry's premise — a distinct `0x4E` opcode — does not hold in this
  codebase; see above. The underlying concern, that a torch's light and a
  personal darkness override are two different things and any future day
  curve needs to keep them separate, is still live and is carried into the
  current file's "Emitters and the light they carry" section using the
  actual mechanism this codebase has: `docs/roadmap.md`'s note that Night
  Sight resends `0x4F` to the caster alone.)*

## Falloff and the light set's edge

**7. Falloff is a shape that reaches zero, and the light set fades at its
edge.**
Two pops to remove, both structural:

- A falloff that is still bright at the radius switches off at the rim. The
  window `(1 - (d/r)^2)^2` — smooth, exactly zero at `r`, inverse-square-ish
  in the middle — has no rim.
- `Lighting::MAX` is 64 and `collect` truncates by distance from the eye, so
  the 65th torch appears and vanishes as the camera moves. The last few in
  the sorted list fade out over the tail rather than being cut, so a light
  leaves by getting dimmer.

**Step 6. Falloff and the fade at the cut.** Decision 7, both halves, plus
a test that walks a camera past a 65th light and asserts no discontinuity.

*(Checked against the current source while rewriting `lighting_world.md`:
the falloff in `light.rs` — `fall = 1.0 - d`, squared — already tapers
smoothly to exactly zero at the radius and has for some time; the first
bullet above no longer describes a real gap, whatever it described when
written. The second bullet, the hard truncation at `MAX = 64` with no fade,
is still accurate and still unbuilt. The current file states the corrected
picture directly rather than repeating the stale half.)*

### Backlog

- **`Lighting::MAX` at 64 is a guess that nobody has hit.** Britain at the
  widest zoom with every window burning (the other plan's backlog: 80
  window graphics carry `LIGHT_SOURCE`) is the case that finds out. The
  truncation is only worth fading (step 6) if it happens; the measurement
  of step 12 will say.

## The tonal response

**8. The frame is composed in linear light and mapped once, at the end.**
Multiplying the art by a colour can only darken it, so there is no such
thing as a bright pool — only a less dark one — and the channel the ambient
is poorest in clips first, which is why a blue night makes warm art go grey
before it goes dark. What is wanted is the ordinary photographic answer:
accumulate in linear, then one tonal curve with a shoulder, so a flame's
centre rolls off warm instead of clipping, plus a *toned* lift in the
shadows (the ambient's own colour, not grey), plus a triangular dither
before the 8-bit write — a large smooth pool on a dark floor is exactly the
picture 8-bit banding is visible in.

**The trap, said out loud:** the client's art is already lit. Every tile and
sprite has baked highlights implying a fixed sun, so real coloured light on
top of it is a double count, and the more saturated the ambient the more
obviously wrong it looks. Nox's art was drawn for Nox's lighting; ours was
not. The practical consequence is that the curve's job is as much restraint
as reach, and that any value here is held by a scene rather than by a
formula.

**Step 7. The tonal response.** Decision 8: linear accumulation, a
shoulder, a toned shadow lift, dither. This is the step most likely to be
argued about and the one most obviously judged by a screenshot — a
before/after pair of the same scene belongs in the commit.

*(Not started for the real composed frame. A shoulder curve with the shape
this decision describes — identity below a knee, an exponential approach to
1.0 above it — does exist in `blit.wesl` as `fn knee`, but only for making
the `View::Light` debug view legible; it is not applied to `fs_main`'s real
output, which is still the hard clip this decision describes as the
problem.)*

## Bodies as occluders

**9. A body between a flame and a wall makes a shadow, and it is a box that
moves.**
Mobiles are not in the occlusion grid, so a crowd around a campfire is a
crowd of things standing in a light that goes straight through them. A
mobile is a short, soft occluder — a partial opacity over a span of about a
body's height — and the grid takes it the same way it takes a static. The
reference does not do this; that is not an argument against it, it is the
reason it needs its own scene and its own value.

It is also the first cell in the grid whose *contents change while nothing
about the map does*, and that is why it waits for
[`lighting.md`](lighting.md)'s step 14: a box drawn over a walking body is
the only cheap way to see whether it is the right height, whether it is
snapped to the tile while the sprite slides (decision 5's mistake, arriving
from the other side), and whether it is left behind when the body moves on.
A soft occluder that is wrong is not a visible bug — it is a slightly
darker wall — so without the instrument this step cannot be judged at all.

**Step 8. Bodies as occluders.** Decision 9, behind its own scene, and
judged with [`lighting.md`](lighting.md)'s step 14 — which is now built, so
this one is open rather than blocked.

*(Still not started as of the current file's writing. `lighting.md`'s solids
diagnostic view, F5, is also now built and gives this a second instrument
beyond the wireframe, once a mobile actually has a box to draw.)*

## A wall's ambient and a window's sky

**13. A wall's face takes the ambient of the tile it looks at.**
Decision 1 gives every tile its own share of the sky, and then a wall tile
makes the split visible in the worst way: it is one tile with a face on
each side, one of which is a room and the other a street. Sampled at its
own cell it is either too bright indoors or too dark outdoors, and no
per-tile answer fixes that, because the two faces are not in the same
place — they are on opposite edges of one cell.

[`lighting.md`](lighting.md)'s step 15 is exactly the missing measurement: a
face read out of the art, and a pixel's `v` along it. With a facing, a
wall's pixels sample the sky field at the tile the face **looks into** —
`(x, y-1)` for a north face and so on — and a house's outer walls are lit by
the day while its inner walls are lit by whatever is burning inside. Without
a facing they sample their own cell, which is today's behaviour and stays
the fallback for every graphic the detector refuses. That refusal is the
important half: step 15's detector must be able to say *undecided*, and this
is the consumer that shows why — a corner post guessed wrong is a wall lit
from the wrong world.

**14 (aperture half — the crude pass-through is under "The sky field"
above). A window passes sky, not only sun.**
So the aperture of [`lighting.md`](lighting.md)'s step 16 has a second
consumer. Where a cell carries one, it seeds the sky field with the sky
visible through it, and decision 2's blur is what spreads it into the
room — a fall-off from the window inwards, which is what a window does.

**Step 9. A wall's face takes its own side's ambient.** Decision 13, after
[`lighting.md`](lighting.md)'s step 15 has a facing to offer. Held to a
frame test of a house at noon: the outer face of a wall is day, the inner
face of the same tile is not, and a graphic the detector refused looks
exactly as it does today.

**Step 10. Sky through the aperture.** Decision 14's second half, after
[`lighting.md`](lighting.md)'s step 16. The `PANE` approximation of step 1
is what it replaces, and the test is that it replaces it *upwards* — the
hall does not get darker when the real aperture arrives.

*(Neither step has been started, but both of their blockers — `lighting.md`'s
steps 15 and 16, the facing and aperture measurements — are now built. The
current file states this section's design as unblocked, not merely
"waiting", for that reason.)*

## The optional curtain

**10. Sight is not light, and this client cannot enforce either.**
Nox's other famous half is that you see only what your character sees. It
could do that because it was authoritative. Here the server has already
sent everything in range, so any "fog of war" drawn on the client is a
curtain over data the player's own memory holds — cosmetic, cheatable, and
dishonest if presented as anything else. It is worth having as an *option*,
because dimming what is behind a wall looks superb and costs one more use
of the walk that is already there, and it is worth never letting it decide
anything. If it should ever be a rule, the rule lives on the server and
this pass is not where it starts.

**Step 11. The optional curtain.** Decision 10, off by default, and
documented as cosmetic where a reader will see it.

*(Not started; no code exists.)*

## Constants held by a scene

**11. What each of these is held by is a scene, not a number.**
`render/src/scene.rs` is the pattern: a built map, a built tiledata, a list
of items, a camera, and an ASCII diagram a failing test prints. Every
decision above that invents a constant — `GROUND_AMBIENT`, the day ramp, the
body's opacity, the tonal curve's shoulder — gets one, and the constant is
tuned against the picture rather than argued into existence. The existing
list of invented values (`occlusion::PANE`, `light::flame`,
`FLAME_SPREAD`) is already the longest section of the other plan's backlog;
this one should not lengthen it silently.
