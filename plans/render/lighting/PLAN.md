# Lighting: what is left to build

The model is built, calibrated against a path tracer, and shipping. What is
described here is the work that is **not** built. Everything about how the
engine works today — the pipeline, the terms, the pixel spaces, the measured
numbers — is [`docs/render/README.md`](../../../docs/render/README.md) and the
design documents beside it; this file carries only intent and order.

The phase numbering is the rebuild's own, kept so the evidence and the code
comments still line up.

## Phase 6i — the impostor's last hole

A corner's two panels are told apart by the **screen half** a pixel was drawn
on, because a `Volume` carries a `SolidId` rather than the instance row that
`split_corners` produced. The normal is already picked by the box; only the
identity still comes from the picture.

The fringe question that used to sit here is **closed**: the clamp stays, and
what it costs is a position rather than a facing. It was measured three ways
and the record is in the design documents — do not reopen it without a new
measurement.

Done when: two panels of one corner are told apart by their own rows, on a
frame where the screen half would give the opposite answer.

## Phase 7 — a mobile's normal

A mobile's normal is one vector for the whole sprite, so a torch on a figure's
left reads no brighter than one on its right. Position and the camera-facing
normal landed; the inflated-silhouette candidate is unbuilt.

The choice between the two is settled by **a person looking at a figure beside
a lamp**, which needs a mobile pass in `examples/isolated_scene.rs` — that pass
does not exist and is the first step here.

Done when: a person can see a figure lit from one side, and has picked between
the two candidates.

## Phase 8 — the sun

The sun is added straight today: no `N·L` anywhere, no soft edge, no sky
visibility as ambient occlusion. The sky field is ambient occlusion by another
name, and this phase is where it is adopted rather than rebuilt.

Ambient is carried alongside it: no default frame has an ambient split, so a
house reads as bright as the street.

## The content layer

Scoped, not started, and each piece is landable alone:

- **The day curve.** Until it lands, a default frame carries no ambient split at
  all and a house reads as bright as the street.
- **Light carried by mobiles other than the local player**, with a
  serial-derived flicker phase.
- **A flame's own screen-space glow**, and the sunbeam shaft through a window.
- **Land as an occluder** — a hill casts no shadow today.
- **Leaded and lattice window apertures**, refused rather than measured: the
  aperture channel of the field is reserved and always zero.
- **`Builder::add` consuming an authored `Blocks` list.** The table format
  supports arches and lintels; nothing wires one into the live grid.
- **A mobile as a soft sub-tile occluder**, and a body's diagonal footprint the
  axis-aligned `Solid` cannot state.
- **Night Sight's interaction with a real day curve** is undecided.
- **UO's own light, as a mode you can pick.** The reference client draws light
  by blending sprites from `light.mul`, keyed by `lightidx.mul` and by a light
  id in the tiledata entry — a source's *shape* is a picture, not a radius,
  which is where a window's light patch on the floor comes from. Neither file is
  read by this client at all; `light::flame` is a stand-in of one warm default
  and a wider campfire, and it is the only invention left in the pass.
  Scoped 2026-08-10, not started: `crates/common/uofiles/src/tiledata.rs`
  already parses `TileFlags::LIGHT_SOURCE`, but `StaticTiles` carries no
  light-id field, so that parse is still missing; no reader for
  `light.mul`/`lightidx.mul` exists anywhere in the workspace. ClassicUO's
  `ClassicUO.Assets/LightsLoader.cs` is the reference — each entry is a small
  bitmap of 5-bit intensities (values above `0x1F` bit-inverted), turned into a
  greyscale RGB blended additively at a fixed *screen* position, with no 3D and
  no occlusion test beyond one binary check of the tile diagonally in front of
  the source (`GameScene.AddLight`). The natural composite point on our side is
  `crates/client/render/src/blit.rs`, where lighting is already applied once on
  the way to the surface; a toggle would follow the existing `App`
  boolean-plus-F-key pattern (F10 night, F8 sunlit, F6 sky field, F7 lantern;
  F5 is the solids debug overlay, and the next free key is open). **Undecided:**
  whether the mode fully replaces the deferred pipeline's shading or composites
  on top of it.

Doors — the ported open/shut occlusion table — are built already, and untouched
by any of the above.

## The instruments

Not a phase, and each of these is a real limit on the ones above:

- the tracer is single-threaded at 13 s a frame, which is too slow for a sweep,
  and a sweep is how the last three defects were found;
- nothing runs the tracer over a real map — every scene it has is hand-built
  boxes and one hand-built flat ground. The brightness calibration beside this
  is done (phase 0); a real map is the whole of what is left of it;
- `tests/dump.rs` draws at even extents only;
- no gate holds that a debug view is drawn from the same planes the lit frame
  is;
- buffer capacity is one flat `INITIAL_QUADS = 4096` for all kinds, so the
  widest real frame reallocates on its first frame, every run;
- a climbable the prism-fit cannot decompose still occludes as a whole-tile
  body;
- a courtyard overhang can make the sky-column test misread a tile — 28 of
  2,560 outdoor tiles in Britain read dark.

## Deliberately parked

Each of these is a *second* answer to "what does a lit frame look like", and a
second answer is only readable once the first one produces a picture worth
comparing against. None of them is a reason to soften a phase above.

- **The stylised end, revisited as an experiment.** The dial between a
  half-space and Lambert is deleted from the plan; the alternatives it came from
  are recorded in [`docs/archive/render/lighting_archive.md`](../../../docs/archive/render/lighting_archive.md).
  Once the frames are ones a person is happy with, trying a stylised BRDF
  against them is a comparison with a baseline, which is the only form in which
  it is worth anything. Not a knob shipped half-tuned in the meantime.
- **How much exposure has to give back.** Double contrast is a global effect and
  a global exposure may absorb most of it — but phase 3's frames say the loss is
  not global at all: open ground barely moves and a *grazed vertical face* moves
  a great deal, which is the case a global exposure is worst at absorbing. The
  experiment is one evening's work and is not inside any phase, because nothing
  in a phase is what the knob would be turned against.
- **The circle of transparency** — a radius around the body inside which walls
  go translucent. Not a lighting feature at all: it is the fifth item of the
  blended pass [`docs/client/design_picture.md`](../../../docs/client/design_picture.md)'s "What is still M3"
  describes, recorded here only because it was asked for in the same breath.

## Order

6i, then 7, then 8. The content layer waits for 8 because a day curve with no
sun term to modulate is a constant. The instruments are picked up when the
phase that needs them arrives — phase 7's own first step is one of them.
