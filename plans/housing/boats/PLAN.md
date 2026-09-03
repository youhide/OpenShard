# A ship that is somebody's property, and one that does not flicker

A ship moors, blocks, carries its crew and sails on a cadence. Two things it is
not: **smooth** for the clients that could have it, and **owned** the way a house
is — no hold to put anything in, no plank that is a door, no deed, no decay.

The decisions are
[`docs/housing/design_boats.md`](../../../docs/housing/design_boats.md), what the
first two phases found is
[`docs/housing/evidence/2026-08-25-the-boat-phases.md`](../../../docs/housing/evidence/2026-08-25-the-boat-phases.md),
and what is built across the domain is
[`docs/housing/README.md`](../../../docs/housing/README.md). This page is what is
not built.

## B3 — smooth, for the clients that can

`0xF6`, behind `version.supports(Feature::SmoothShip)` and **never** behind an
era comparison. A High Seas client gets one packet per move; a 4.0 client keeps
the forget-and-reveal redraw, unchanged and still correct. Strictly better, and
it removes nothing.

The measured thing it replaces is exact: **two packets per client that can see
the ship**, a `0x1D` and the `0x1A` that draws it again, every move. The
per-occupant `0x20` and `0x77` are not what `0xF6` is about and stay.

- [ ] The packet itself, both ends, with the layout read out of the reference and
      cited at the constant.
- [ ] The branch at the send site, off the connection's own feature set.
- [ ] A test that a connection without the feature still gets the redraw — the
      failure this gate exists to prevent is a client that is told nothing at all.

## B4 — the boat as property

All of it is housing's H2–H5 with a different noun, which is why it is one phase
rather than four:

- [ ] **The hold**, which is a container on the ship — `items::capacity`'s
      shape, and the question to answer first is whether its contents ride the
      move or are addressed by the ship's own serial.
- [ ] **The plank as a door**, which is also how a UO player boards. The
      measurement under decision B5 turned on exactly this: a swimmer can no
      longer clamber over the gunwale, which is correct, and it leaves boarding
      with no gesture at all until a plank exists.
- [ ] **The deed**, `.boat`'s shape sold to a player, through the same multi
      cursor a house deed raises.
- [ ] **Decay**, which is `House::age`'s accumulator on a `Boat` — a tick count
      that counts *up*, because the world's clock starts at zero every boot.
- [ ] **The tiller**, the one step of the motion phase that was skipped: an
      ordinary double-click target naming its boat by serial, with speech
      keywords routed through `tick/speech.rs`. `.sail` stands in for it today.

## What has to be decided before B4 rather than during it

**A boat's crew is derived per move and its cargo would not be.** The manifest is
recomputed from who is standing on a plank of *this* ship, which is why nothing
about a passenger is saved. A hold is the opposite: it is a container with a
serial, its contents are saved, and they have to arrive at the new tile without
being swept by the sector index as loose ground clutter. That is the same
exclusion a house and a hull each needed by name, and it is the third instance —
worth deciding once rather than discovering a third time.
