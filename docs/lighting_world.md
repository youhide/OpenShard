# Lighting, part two: the light a place has

> **Consolidated into [`lighting_rebuild.md`](lighting_rebuild.md)** — ambient and the sky field, most of which survives.
> That document is the entry point: it lists what is still live here, which rebuild phase retires or inherits it, and what carries over untouched. This file stays as the record of how it was built and why.


Current state of the ambient/sky-field system: what it computes today, the
data format it shares with [`lighting.md`](lighting.md)'s occlusion grid, and
which parts of the design are built versus still a plan. The reasoning behind
each choice — arguments made, alternatives tried and rejected, and the
session-by-session narrative that produced the design — lives in
[`lighting_world_archive.md`](lighting_world_archive.md), organized under
headings that mirror this file's.

[`lighting.md`](lighting.md) is the flame/shadow pass: a wall stops a torch,
per fragment, by a real grid traversal. That subject is shadows. This file's
subject is not shadows — it is **where the light in a frame comes from when
nothing is burning**: why the inside of a house should be darker than the
street outside it with nothing in either, and why a windowless cellar should
still be more than pure black.

## Overview

The ambient a fragment is lit by is split into two colours that are summed,
weighted by how much of the sky the fragment's tile can see:

```text
ambient(tile) = ground + sky * share(tile)
```

`share(tile)` is a per-tile byte — how much of the sky an unobstructed column
above that tile's floor can see, `0` under an opaque roof and up to
`SKY_OPEN = 255` (`occlusion.rs:939`) in the open — computed by walking the
same occlusion grid [`lighting.md`](lighting.md)'s shadow rays walk, on the
CPU, once a frame. This is [`crate::light::Ambient`] (`light.rs:297`) and
[`Ambient::at`] (`light.rs:349`); the sky-share byte itself is
[`Occlusion::sky_at`] (`occlusion.rs:1142`).

Everything else in the plan this file used to be — a day/night colour curve,
mobiles and effects carrying their own light, an emitter that isn't dimmed by
its own darkness, a softer falloff and a fade at the light-count cap, a real
tonal response instead of a hard clip, a mobile as a soft occluder, a wall
face sampling the tile it looks into rather than its own, a fog-of-war
curtain — is design, not yet code. Each has its own section below, stated as
what it would do and why it hasn't landed, not as an argued case; the argued
case is in the archive.

**Out of scope, deliberately.** Everything here runs on the client; the
server is not asked for a single new byte, and `0x4F` (the overall light
level packet, `crates/common/protocol/src/world.rs:894`) remains the whole
protocol surface this reads. The per-pixel raggedness of a lit wall — the
fraction along an upright sprite's face — is
[`lighting.md`](lighting.md)'s subject, in `statics.wesl`, and nothing here
touches it.

**Also out of scope: which primitive underlies the occluder this plan reads.**
Whether a roof or a wall is a box or a mesh is decided in
[`lighting_geometry.md`](lighting_geometry.md), not here — the sky-column walk
described below reads whatever the grid holds, box or mesh alike, and does not
care which. One open question is left for this file rather than that one:
whether a sloped mesh roof still gives the column test a clean single-bit
in-or-out answer, or needs a fractional, partial-coverage answer the current
byte-per-tile multiplicative model doesn't have today. See "The sky field"
and "Status" below.

## The sky field

**Built.** A tile's sky share starts at `SKY_OPEN` and is multiplied down by
every occluder standing in its column above the tile's own floor height —
[`Builder::shade`] (`occlusion.rs:1770-1784`), called from the same walk that
already visits every static for the shadow grid, so this costs one land
lookup per static already touched rather than a second pass over the map.
Multiplicative and not additive: two roofs over one tile don't make it darker
than black, and a pane under a slate roof is as dark as the slate.

**Membership is the same test the shadow walk uses.** A roof is an occluder
here because a roof static carries `WINDOW | NO_SHOOT` — [`stops_light`]
(`occlusion.rs:126-128`), the same membership `lighting.md`'s shadow walk
reads, a fact about arrows and not a fact invented for lids. No separate flag
or lookup exists for "is this a roof"; the column test asks the same opacity
byte the ray walk does.

**A window passes its share rather than blocking outright.** `WINDOW`'s
opacity is `PANE = 51` out of 255 (`occlusion.rs:390`), so
`Builder::shade`'s multiply lets `(255 - 51) / 255`, four fifths, of what
was already there through — a glazed roof lets four fifths of the sky
through where a slate one lets none. This is deliberately the crude half of
what a window could do: it changes how much sky a tile *starts with* before
the blur below spreads it, not a directional patch of daylight seeded at the
window and falling off inward (see "A wall's ambient and a window's sky").

**The test always reads the map as it stands, never as it is drawn.** This is
the one reader of the occlusion grid that does not consult
[`cutaway::shows`] — every other consumer (the shadow walk, the wireframe
view) only sees what the frame's [`Cutaway`] left visible, because a shadow
cast by a static the cutaway removed from the picture is an artefact: nothing
in the image is making it. The sky test is the opposite: if it read the
*drawn* set of statics, standing indoors would delete the roof over the
player's own head and flood the room with noon, carrying daylight into every
building the player enters. A missing ambient from a roof the player knows is
there, because they walked under it, is the point rather than a bug.

**The field is blurred once, after every occluder has shaded it in.**
[`Builder::blur_sky`] (`occlusion.rs:1805-1825`) is a single 3×3 average over
the grid's own rectangle, edges repeated rather than falling off outside it —
averaging in an assumed-open tile past the grid's own edge would draw a
bright rim around the inside of every frame's border, a picture of where the
grid ends rather than of where a roof does. A raw column test steps from
`SKY_OPEN` to `0` exactly at the wall line; the blur is what turns that step
into a doorway brighter than the room behind it and an eave brighter than
the floor under it. It is not a simulation of light bouncing through a
doorway — it is the shape a blur of a small array happens to have, applied
once, never twice.

**Format.** The finished field is a second `Rgba8Uint` texture over the same
rectangle the occlusion grid's index covers, one texel a tile —
[`Occlusion::field_bytes`] (`occlusion.rs:1171-1177`). Its four channels are
`(sky, aperture, body, unused)`: what a *tile is*, read once per fragment,
as opposed to `Occlusion`'s own cell format, which is what a *ray* walks
through in a loop. Only the first channel is written today —
`field_bytes` emits `(sky, 0, 0, 0)` for every tile — the other three are
reserved, not padding: `aperture` for the fuller window pass-through
[`lighting.md`](lighting.md)'s own aperture measurement would seed, `body`
for a soft occluder's opacity when a mobile becomes one (see "Bodies as
occluders"), and one channel still unclaimed by either.

**Cost, measured on the built scenes and on Britain at the widest zoom:** one
land lookup per static in a walk that already touched it, and one 9-tap pass
over the grid — 187×187 tiles at the widest zoom, on the CPU, once a frame
(`light.rs:3112-3113`). Nothing per fragment at this stage: no shader reads
the field plane until it is uploaded (see "The two ambients").

**A wall tile has no sky of its own.** A wall shades its own column the same
as anything else standing in it, so a wall tile's own `sky_at` reads `0` —
the ring of tiles a house's outer walls stand on is exactly as "roofed" as
the room inside, by this test alone. That is not visible yet in the ordinary
lit frame (see "The day curve"), but it is visible under `View::Sky` once the
sky field is on, and it is exactly the gap "A wall's ambient and a window's
sky" below exists to close: a wall face samples its own tile's zero rather
than the tile the face looks into.

**Known, currently unmeasured edge cases.** A courtyard roof overhang is
sometimes drawn on the tile *next to* the one its static actually stands
over — a static is art rising from its own diamond, and an eave can overdraw
its neighbour — so the column test can read a covered tile as open, or an
open courtyard tile as roofed, depending on which tile the overhanging
static's own footprint is anchored to. Nothing has measured this against a
real house scene yet. Separately, a graphic whose sub-tile footprint
[`lighting.md`](lighting.md)'s facing detector could not read becomes a
whole-tile `EDGE_ANY` occluder there, and the sky column test inherits the
same over-blocking the shadow walk gets from it — a narrow post reads as a
solid tile of roof. A scan of the block of Britain
[`lighting.md`](lighting.md)'s own cutaway tests walk found 28 of 2,560
tiles the cutaway calls outdoors reading dark under the column test; most are
wall tiles (the paragraph above), the remainder the overhang case.

## The two ambients

**Built.** [`Ambient`] (`light.rs:297-302`) is two RGB triples, `sky` and
`ground`, rather than one colour and a brightness: a sky is blue where a
cellar's own floor light is bluer still, and a single number could only ever
say how *much* light a place has, never what kind.

```rust
// Ambient::at, light.rs:349-356
pub fn at(self, sky: u8) -> [f32; 3] {
    let share = f32::from(sky) / f32::from(SKY_OPEN);
    let mut lit = self.ground;
    for (channel, sky) in lit.iter_mut().zip(self.sky) {
        *channel += sky * share;
    }
    lit
}
```

Three constant `Ambient`s exist:

- **`Ambient::DAY`** (`light.rs:311-314`): `sky = [1,1,1]`, `ground = [0,0,0]`
  — the identity, full daylight under an open column and nothing at all
  under a lid, at which the blit is a copy of the world image.
- **`NIGHT`** (`light.rs:467-470`): `sky = [0.20, 0.22, 0.31]`,
  `ground = [0.10, 0.11, 0.14]` — the two terms sum to `[0.30, 0.33, 0.45]`,
  the single colour night was before the split, so a street at night reads
  exactly as dark as it always did and the whole of what changed is indoors.
- **`SKYLIGHT`** (`light.rs:482-485`): `sky = [0.43, 0.42, 0.44]`,
  `ground = GROUND_AMBIENT` — sums to `[0.55, 0.55, 0.62]`, likewise the
  daylit frame's old single colour, well short of white so a sun in the
  frame still has shadows to cast and well short of black so a shadow at
  noon reads as a shadow rather than a hole.

`GROUND_AMBIENT = [0.12, 0.13, 0.18]` (`light.rs:456`) is the one number this
plan has actually invented and shipped: small, because the whole of what the
split buys is a room darker than the street and a generous floor gives that
straight back; cold, because it stands in for bounced light off stone and
plaster rather than for a source, and a warm floor would take the one hue a
flame gets to keep for itself.

**`Ambient::flattened`** (`light.rs:333-342`) sums the split back into one
term — `sky: [0,0,0]`, `ground: sky + ground` — recovering exactly the
single-colour ambient this pass had before the field existed. This is what
the client uses by default (see below), because judging a point light's own
falloff wants one thing changing in the picture at a time, and the field is
the larger visual signal in a lit frame if left on.

**Wiring.** The client picks one of three skies — `light::NIGHT` (F10),
`light::SKYLIGHT` (F8, "sunlit"), or nothing (plain daylight, the identity) —
then, unless the sky-field toggle (`self.sky_field`, F6, off by default) is
held on, flattens whichever it picked before building the frame's `Lighting`
(`crates/client/app/src/lib.rs:4663-4681`). With no sky chosen at all, the
frame is `Lighting::NONE` and no grid is built (see "The day curve").

**Both sides read the same field.** `blit.wesl` reads the field plane as a
second grid texture, binding 5 (`shaders/blit.wesl:58`), uploaded beside the
occlusion grid's own textures and never apart from them
(`blit.rs:824`), and does `Ambient::at`'s same arithmetic per fragment via
one `textureLoad` plus a multiply-add (`shaders/blit.wesl:664`).
`light::sample`'s CPU walk does the identical lookup —
`lighting.ambient.at(lighting.occlusion.sky_at(spot.tile.0, spot.tile.1))`
(`light.rs:1623-1625`) — using the fragment's own tile, not the fractional
world position within it: the field is a byte a tile, and a second
interpolation on the CPU side would disagree with the shader's own read of
the same texel. [`lighting.md`](lighting.md)'s parity test is what keeps the
two held together, and it gained a third fixture scene for exactly this
field: `roofed_room` (`scene.rs:1021`, used in `tests/frame.rs:4327` and
`tests/frame.rs:4877`), because every earlier parity scene is lit by an
ambient that happens to be nearly uniform across almost all of it — a shader
reading the wrong plane could still agree with the CPU almost everywhere.
`roofed_room`'s own sky spread is asserted before its pixels are compared.

**The instrument is the field on the ground, not the wireframe.**
`View::Sky` (`debug.rs:147`, `VIEW_SKY = 9u` in `shaders/blit.wesl:142`)
draws the sky share **on the ground itself** — white under open air, black
under a roof, a gradient across a doorway (`shaders/blit.wesl:1268-1275`) —
rather than as a colour on the occlusion wireframe's boxes
(`shell::draw_occluders`, `crates/client/app/src/shell.rs:2082`). The
wireframe still shades by opacity/kind (a lid amber, a panel red, a
whole-tile body violet, a pane cyan — `shell.rs:2072-2078`), unchanged by
any of this, and that is deliberate: the failure the sky field actually has
is a tile that is *wrongly open* — an eave that didn't cover the floor
under it, a roof static standing one tile over — and shading a box by its
sky share would only ever be visible where there is a box at all. A hole in
a roof is exactly where there is no box, and would be invisible in the one
view meant to find it.

**Cost, measured at the widest zoom:** one more `Rgba8Uint` texture of the
grid's own rectangle uploaded per frame — 140KB at the widest zoom, doubling
what the pass already uploads for the occlusion grid itself
(`light.rs:3112-3113`) — and one `textureLoad` plus a multiply-add per
fragment. No walk, no loop.

**A dangling predicate.** `Lighting::is_identity` (`light.rs:437-442`) now
accounts for the occlusion grid being non-empty (a grid with a roof in it may
darken a tile even with no flame burning, so it is no longer the identity
just because `lights` is empty) — but nothing calls it as an early skip
anywhere in the render path; the blit multiplies unconditionally regardless
of its answer. Only its own unit test (`light.rs:2873-2879`) reads it today.

## The day curve

**Built, with the server's existing clock as its source.** The server sends
`0x4F` as its light level from 0 to 31. `client/net::WorldView` keeps that
authoritative target and the app's `graphics::Daylight` eases to it over three
seconds, so the protocol's discrete dawn and dusk steps no longer read as a
row of switches. The client interpolates the existing `NIGHT` and `DAY`
ambients from that eased value; F10 remains an immediate diagnostic override.

`0x65` weather now travels beside the level. The shard derives a deterministic
six-UO-hour condition from its clock and current season, sends it on entry and
at each boundary, and the client applies rain, storm and snow as an
intensity-weighted ambient filter. This is deliberately atmospheric lighting,
not a second particle system: the classic packet's intensity is retained in
the client state so a precipitation pass can consume exactly the same answer
later.

**This is what the rest of the split actually waits on.** An ordinary
daylit frame today picks no sky at all and is built as `Lighting::NONE`
(`light.rs:405-411`), whose occlusion field is `Occlusion::EMPTY`
(`occlusion.rs:1037-1049`) — no grid, no sky field, nothing for a fragment to
read. The sky split is real and tested (see above), but it only reaches the
screen at night (F10) or under the sun key (F8), or with the sky-field key
(F6) held on top of one of those — a player in the ordinary daylit mode,
which is most of the time, sees a house lit exactly as brightly as the
street outside it, same as before this plan existed. The day curve is the
step that makes *every* frame carry an ambient, which is what turns the split
from a thing the tests can see into a thing a player does.

## Emitters and the light they carry

**Not built.** `light::collect` (`light.rs:660`) walks map statics and
server-dropped ground items only — it takes neither a mobile list nor an
effect list, so nothing standing on a mobile's own equipped layer, and no
spell, projectile or explosion in flight, casts any light today. The
player's own held torch is a separate, already-built mechanism
([`lighting.md`](lighting.md)'s carried-flame beam, added to the frame after
`collect` runs) — what's missing here is everything else that burns and
moves: another mobile's torch, a fireball mid-flight, a burning corpse.

The design each of those would need is the same one the player's own torch
already has, generalized: a light placed at the thing's **interpolated**
world position rather than its tile, because a mobile's sprite already
slides between tiles and a light snapped to the tile would jump a whole tile
at a time while the sprite carrying it slides continuously — and a flicker
phase keyed off the mobile's own serial rather than off its tile, so the
light's character doesn't visibly change the moment it crosses a tile
boundary.

**An emitter is not yet exempt from the darkness it's supposed to dispel.**
A campfire at night is currently art multiplied by the night ambient, plus
its own point light at distance zero — nothing clamps that sum from below,
so if the two together come out under `1.0` the fire renders as a dim fire,
which no fire is. No such clamp exists in `light.rs` or `blit.wesl` today;
the intended fix is one `max` in the per-fragment loop, so a fragment on an
emitter's own tile is lit by at least that emitter's own colour at full
intensity regardless of what the ambient multiplier would otherwise give it.

**The personal light level packet has no separate identity in this
codebase.** Real UO client protocols distinguish an "overall" light level
from a "personal" one; here only `0x4F` (`crates/common/protocol/src/world.rs:894`,
"overall light level") exists, and the **Night Sight** spell
(`server/world/src/tick/spells.rs`) is implemented by resending the caster
its own copy of that same packet at its brightest value — unicast, not
broadcast (documented as a "visual no-op until a day/night cycle exists" in
`docs/roadmap.md`). Once the day curve above makes `0x4F` drive a real
per-frame ambient, Night Sight's resend needs to mean something different
from an ordinary level change *for that one client only*, and nothing has
decided how the client would tell the two apart — a genuine open question
this section leaves for whoever builds the day curve, not a settled design.

## Falloff and the light set's edge

**Falloff is already smooth at the light's own radius.** A point light's
contribution is scaled by `fall * fall` where `fall = 1.0 - (distance /
radius)` (`light.rs:1664-1667`), continuous and exactly zero at the radius,
with a zero derivative there too — a light does not pop at its own rim
today. (An earlier version of this plan proposed a different curve to fix a
"still bright at the radius" problem; the falloff in the codebase already
tapers to zero smoothly and has for some time, so that half of the concern
no longer describes anything in the current source.)

**Not built: a fade at the light-count cap.** `Lighting::MAX = 64`
(`light.rs:402`) is a hard truncation — `collect` sorts the frame's lights
by distance from the camera's eye tile and calls
`lights.truncate(Lighting::MAX)` (`light.rs:719`) — so the 65th-nearest light
simply isn't in the list; there is no fade as one crosses the boundary from
inside the kept set to outside it as the camera moves. Whether this is ever
actually visible in practice — whether a real scene puts more than 64 lights
in view at once — is a measurement [`lighting.md`](lighting.md) owns (its own
notes on window-graphic light-source density); nothing here repeats it.

## The tonal response

**Not built — the real frame is a hard clip.** The composed pixel is
literally `min(color.rgb * lit, vec3(1.0))` (`shaders/blit.wesl:1594`): the
art multiplied by the accumulated light, clamped to `1.0` per channel. Since
multiplying can only ever darken the art, there is no such thing as a bright
pool on screen today — only a less-dark one — and whichever channel the
ambient is poorest in clips first, which is why a blue night ambient makes
warm art go grey before it goes dark rather than rolling off smoothly.

The intended replacement: accumulate in linear light (already true), then a
single tonal curve with a shoulder so a flame's centre rolls off warm
instead of clipping to white, a *toned* lift in the shadows (the ambient's
own colour, not a flat grey), and a triangular dither before the 8-bit
write — a large, smooth, dark-floor pool is exactly the picture 8-bit
banding is visible in.

A curve with a shoulder already exists in the shader —
`fn knee` (`shaders/blit.wesl:1241-1247`, `KNEE = 0.6`) — but it is wired
only into the `View::Light` debug view, to make that view's `0..1` range
readable against real values that run past `1.1`; it is never applied to
`fs_main`'s real composed output. The shape of the curve this plan wants is
arguably already solved and sitting unused by the path that would benefit
from it.

**The trap, worth keeping in view when this lands:** the client's own art is
already lit — every tile and sprite has baked highlights implying a fixed
sun — so real coloured light stacked on top of it is a double count, and the
more saturated the ambient gets the more obviously wrong that looks. The
curve's job, when it exists, is as much restraint as reach, and any constant
it needs is one to hold against a built scene rather than to argue into
existence (see "Constants held by a scene" below).

## Bodies as occluders

**Not built.** No mobile is pushed into the occlusion grid as a `Solid`
anywhere in `occlusion.rs` today, so a crowd standing around a campfire
stands in light that passes straight through every one of them. The intended
design is a mobile as a short, soft occluder — a partial opacity over a span
of roughly a body's height, taken by the grid the same way a static is,
placed and cleared as the mobile moves rather than baked with the map.

This is the first case in the grid whose contents would change while nothing
about the map does, which is why it needs its own scene and its own
constant rather than reusing one of `lighting.md`'s: whether the box is the
right height, snapped to the tile while the sprite slides between tiles (the
interpolation mistake "Emitters and the light they carry" above describes,
arriving from the other side), and cleared once the body moves on, is not
something a single screenshot answers. [`lighting.md`](lighting.md)'s own
occlusion wireframe (`shell::draw_occluders`) and its solids diagnostic view
(F5, `crates/client/render/src/solids.rs`) are both already built and would
both serve as that instrument once a mobile's box exists to draw — nothing
about this plan is waiting on either of them anymore.

## A wall's ambient and a window's sky

**Not built.** A wall tile's own sky share is always `0` (see "The sky
field"), and neither `light::sample` nor `blit.wesl` samples anything but
the fragment's own tile when computing its ambient — a wall's two faces, one
looking into a room and one looking into the street, both currently read the
same zero-sky ambient of the wall's own cell, rather than the ambient of
whichever tile each face actually looks into. Under `View::Sky` this shows
as a dark ring around the outside of every roofed building, indistinguishable
from "a wall in shadow" until it's pointed out.

The fix this plan has in mind: sample a wall face's ambient at the tile the
face **looks into** — `(x, y-1)` for a north face and so on — rather than at
its own cell. [`lighting.md`](lighting.md)'s own facing measurement (which
edge of a tile a wall's art stands on, `facing::facing_of`) is already built
and available (see [`lighting.md`](lighting.md)'s "The art-measurement
pipeline"), so nothing external blocks starting this — it just hasn't been
wired into the ambient lookup yet. Where the facing detector refuses to name
an edge (a corner post it can't read cleanly), the fallback stays the wall's
own cell, exactly as it reads today.

**The window's aperture is the same shape of gap.** "The sky field" above
already gives a `WINDOW` tile a crude pass-through — its opacity lets four
fifths of whatever sky share was already at that tile continue past it. What
isn't built is the fuller version: seeding the field's own `aperture`
channel (currently always zero — see "The sky field") with what a measured
window opening actually passes, so the existing blur spreads a real
directional falloff inward from the window rather than a flat per-tile
share. [`lighting.md`](lighting.md)'s aperture measurement
(`facing::aperture_of`, which finds the largest rectangle inscribed in a
window's transparent region) is already built and available for this — the
gap is entirely in wiring its answer into `field_bytes`' second channel,
not in measuring anything new.

## The optional curtain

**Not built, and deliberately deferred.** Sight is not the same question as
light, and this client cannot enforce either: the server already sends
everything within range, so any fog-of-war drawn on the client would be a
curtain over data the player's own memory already holds — cosmetic and
cheatable, not a real information boundary. It would be worth having as an
option regardless, because dimming what's behind a wall looks good and reuses
the walk this plan already has, and it would be worth never letting it
decide anything that matters. If it should ever be a real rule, that rule
belongs on the server; this pass is not where it would start. No code exists
for this today.

## Constants held by a scene

Every number this plan invents — `GROUND_AMBIENT` today; the day curve's
ramp, a soft body's opacity, the tonal curve's shoulder, once each lands — is
tuned against a scene built in `crates/client/render/src/scene.rs`, the same
library [`lighting.md`](lighting.md)'s own tests use, rather than argued into
existence from first principles. A constant with no scene backing it is a
number nobody is actually holding to a picture.

## Status

**Built and in the live render path:**
- The sky field's column test (`Builder::shade`), its same-membership rule
  with the shadow walk, its 3×3 blur, and its deliberate independence from
  the frame's `Cutaway`.
- The `(sky, aperture, body, unused)` field format (`Occlusion::field_bytes`)
  as a second `Rgba8Uint` plane, with only the `sky` channel populated.
- The two-ambient split (`Ambient`, `Ambient::at`, `Ambient::flattened`),
  `NIGHT`/`SKYLIGHT`/`GROUND_AMBIENT`, wired through F10 (night), F8 (sun)
  and F6 (sky field, off by default and flattened when off).
- CPU/GPU parity for the field specifically (the `roofed_room` fixture).
- The `View::Sky` instrument, drawing the field on the ground rather than as
  a colour on the occlusion wireframe.
- A point light's falloff already tapering smoothly to zero at its own
  radius.

**Not yet built:**
- The day curve — the step that makes an ordinary daylit frame carry any
  ambient split at all. Until it lands, the default frame is
  `Lighting::NONE` with an empty occlusion grid, and a house's inside reads
  exactly as bright as the street outside it in the mode a player is in most
  of the time; the split is visible today only by holding F10 or F8 (and, on
  top of either, F6).
- Mobiles and effects carrying light, with an interpolated position and a
  serial-derived flicker phase; only the player's own held torch (a separate,
  already-built mechanism in `lighting.md`) casts light today.
- The emissive clamp that keeps a fire from reading as dim under a strong
  ambient.
- A fade for the light list's `MAX = 64` truncation as a camera crosses the
  boundary of which lights are kept.
- The real tonal response (a shoulder curve, a toned shadow lift, dither) for
  the actual composed frame — today a hard `min(color * lit, 1.0)` clip. A
  shoulder curve already exists in the shader (`fn knee`) but is wired only
  into a debug view.
- A mobile as a soft, sub-tile-height occluder in the grid.
- A wall face sampling the ambient of the tile it looks into rather than its
  own cell — unblocked (the facing measurement it needs is built in
  `lighting.md`) but not yet wired.
- The window aperture's fuller, directional pass-through into the field's
  `aperture` channel — unblocked (the measurement it needs is built in
  `lighting.md`) but not yet wired; the crude flat pass-through in "The sky
  field" is what stands in for it today.
- An optional fog-of-war curtain — no code exists, and it is deliberately
  scoped to stay cosmetic if it is ever built.

**Deferred to a sibling document:**
- Whether the sky-column test's occluder is a box or a mesh —
  [`lighting_geometry.md`](lighting_geometry.md) owns that choice, and this
  plan's own column walk is agnostic to it. Left open here: whether a sloped
  mesh roof still gives the column test a clean single-bit answer, or needs a
  fractional one the current per-tile byte can't represent.

**Known, standing gaps** (not bugs in progress — stable, current facts):
- `field_bytes`'s `aperture`, `body` and `unused` channels are always zero;
  the format reserves the room, nothing writes it yet.
- `Lighting::is_identity` accounts for the occlusion grid but is not wired
  to any early-out in the render path — only its own unit test reads it.
- A courtyard roof overhang drawn on the tile adjacent to the one its static
  actually stands over can make the column test misread that tile's sky,
  unmeasured against a real house scene.
- A narrow-footprint graphic the facing detector can't read becomes a
  whole-tile occluder and over-blocks the sky the same way it over-blocks a
  shadow — inherited from `lighting.md`'s own occlusion model, not specific
  to this plan.
- A scan of 2,560 tiles Britain's cutaway calls outdoors found 28 reading
  dark under the column test — mostly wall tiles, the rest the overhang
  case above.
- Night Sight resends the caster the same `0x4F` overall-light packet
  unicast rather than using any distinct "personal light" signal — how that
  should interact with a real per-frame day curve, once one exists, is
  undecided.
