# Silhouettes: the widths, and the decision that waits on them

The attribution of the two edges, the seam inside the picture and the clamp are
built and described in
[`docs/render/design_silhouettes.md`](../../../docs/render/design_silhouettes.md).
Two things are open, and the second waits on the first.

## Z1's unfinished half — the widths at `4x`

Now unblocked: a `4x` frame over `ON_THE_WALLS` assembles. The claim to measure
is that a run of `art_edge` pixels crossing the outline is `scale` real pixels
wide while a run of `box_edge` pixels is one. The instrument is the pair of
views the attribution already draws.

The habit this leaves behind, worth stating because it cost a session once: **a
scene chosen at `1:1` does not carry over to a magnified frame** — a diagnostic
that changes the zoom has to change the eye tile with it.

## S2 — the decision

Three candidates, to be argued with Z1's picture in hand rather than in the
abstract:

1. leave it as it is;
2. let the box bound more of the art;
3. estimate coverage.

## Z4 — the ratio, after

Re-take Z2's counts now that
[`design_footprints.md`](../../../docs/render/design_footprints.md) has landed
its fitted boxes. The prediction this makes, which Z4 either confirms or kills:
**the zigzags recede on their own**, because a box that fits the art clips more
of the outline.

## Order

Widths, then Z4's re-take, then S2 — S2 is a decision and both of the others
are measurements it should be made from.
