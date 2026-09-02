# Plan: the three camera stages that are still empty

C0, C1, C2, C4 and C7 are built — the seam, the bench, the lift, the scope and
the real pixel. The model they were built against is
[`docs/client/design_camera_rig.md`](../../../docs/client/design_camera_rig.md)
and the record of how is
[`docs/client/evidence/2026-08-14-the-camera-rig-record.md`](../../../docs/client/evidence/2026-08-14-the-camera-rig-record.md).

What is left is three stages of one pipeline. They are independent of each other
and each is scored on the bench that already exists, so the order below is
preference rather than a dependency chain.

## C3 — the spring

Plane damping, the dead zone, and the idle recentre that stops the dead zone
stranding the body off centre. Scored on `back_and_forth` and `rollback`: the
claim is that overshoot stays under a bound while the rollback's jerk drops by an
order of magnitude against `HARD`, and **both halves are asserted**, because a
camera that absorbs a rollback by never keeping up is not the camera anybody
asked for.

Part of it landed early from the other end — the body eases and the eye does
not, `Ease::WALK` at τ 0.08, chosen off `dst::dump_the_ramp`'s table. That is a
setting and not a camera: the rig is still `HARD`, so the character does not
slide. What C3 has left is the dead zone, the idle recentre, and whether the eye
wants damping *on top of* an eased body — which is now a question with an
instrument behind it rather than a matter of taste. The table it would be argued
from is in the record, under C3.

**Moving on from it** is the two claims above holding on the bench, with the
`HARD` rig still selectable, because D9 says nothing here decides the default
camera.

## C5 — the intent

Velocity look-ahead and cursor lean, each smoothed separately and capped
together. `mouse_swirl` is the scenario that says whether the lean needs its own
filter, and it will.

A note that pays for itself here: the lead is also a **prefetch**. The atlases
grow from `Camera::visible_tiles`, so an eye that leads the body by a third of a
screen asks for the ground the body is walking into before it gets there, for
free.

**Moving on from it** is `mouse_swirl` scoring no worse than `HARD` on jerk while
the lead is measurably ahead of the body on `back_and_forth`.

## C6 — the anchors

The free camera as a first-class anchor rather than a lock that is off: origin
plus offset, edge scroll, a spring return, and the rule that a hand on the camera
outranks the automation until it lets go. This is the RTS and HotS camera, and it
is deliberately last, because it is the one whose shape is least constrained by
anything above.

**Moving on from it** is `Follow::Free` ceasing to exist as a mode:
[`docs/client/design_camera_shell.md`](../../../docs/client/design_camera_shell.md)'s
lock becomes a choice of anchor, and `Home` becomes a cut to the body anchor
rather than a second door to the eye.

## What this plan may not hold

Progress. A stage that lands updates
[`docs/client/README.md`](../../../docs/client/README.md)'s readiness row and
files what it measured under `evidence/`; this file is edited only when the plan
itself is revised.
