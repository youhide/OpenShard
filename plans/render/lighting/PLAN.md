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

Scoped, not started, and each piece is landable alone: the day curve, lights
carried by other mobiles, a flame's own glow, the sunbeam through a window,
land as an occluder, and UO's own `light.mul` / `lightidx.mul` as a mode picked
beside the deferred pipeline (the tiledata light-id parse, both file readers,
the composite point, the toggle).

## The instruments

Not a phase, and each of these is a real limit on the ones above:

- the tracer is single-threaded at 13 s a frame and has never been run over a
  real map;
- `tests/dump.rs` draws at even extents only;
- no gate holds that a debug view is drawn from the same planes the lit frame
  is.

## Order

6i, then 7, then 8. The content layer waits for 8 because a day curve with no
sun term to modulate is a constant. The instruments are picked up when the
phase that needs them arrives — phase 7's own first step is one of them.
