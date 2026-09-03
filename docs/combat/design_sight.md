# Sight — the ray a shot is allowed by

A ranged attack asks one question before anything else happens: *is the target
in the open?* The shard answers it with `openshard_movement::sight_clear`, which
returns `true` or `false`. That is enough to fire an arrow and not nearly enough
to explain a refusal: a player standing in a clearing, shooting at an orc six
tiles away, used to be told nothing at all when the answer was no, and a person
debugging it had a boolean and a hunch.

So the walk is **legible**: `sight::trace` records it tile by tile with the
reason it stopped, `sight_clear` is a *reading* of that trace, and the client
draws the same walk over its own frame.

It is a diagnostic, not a rule change. **Nothing here changed what `sight_clear`
answers** — the last phase of the build spent its whole budget on proving that it
did not. How it was built is
[`evidence/2026-08-27-the-sight-overlay.md`](evidence/2026-08-27-the-sight-overlay.md);
what is open across the domain is [`README.md`](README.md).

## What the ray is

Two layers, in this order:

1. **The map** — `MapTerrain::sight_stop` ([terrain.rs](../../crates/common/movement/src/terrain.rs)):
   - the tiles are Bresenham's, `line_tiles` ([walk.rs:78](../../crates/common/movement/src/walk.rs#L78)),
     and **both endpoints are excluded** — an archer and their quarry do not
     stand in their own way;
   - the ray runs at `EYE = 9` above a `z` interpolated between the two ends by
     tile *index*, so a look up a hill follows the slope;
   - **ground** above the ray stops it;
   - a **static** stops it when it carries `WALL | BLOCK | NO_SHOOT`, or is a
     platform (an upper floor is what stops you seeing the storey above), and
     the ray falls inside `[base, top)`. A wall's `top` is `base +
     max(height, 15)` — tiledata gives walls height 0 and the client draws them
     a storey tall. A platform keeps its real height, and the difference is the
     reason an open doorway is a sight line at all;
   - `WINDOW` is the deliberate hole: a look passes, a step does not.
2. **The live world** — `sight_clear` ([walk.rs:799](../../crates/common/movement/src/walk.rs#L799)) —
   which asks `Overlay::sight_blocker_at`. A crate is furniture; a shut door
   is opaque at every height; and a structural house wall stops the ray within
   its span. Wall-like components with zero tiledata height borrow one storey;
   platforms retain their real height.

Its callers: combat's `obstruction()`, which every action — a blow, a shot, a
breath — is asked at its commit and again every tick it runs, a
creature's aggro ([ai/src/lib.rs:613](../../crates/server/ai/src/lib.rs#L613)), and
a vendor deciding whether it can see the customer
([vendor.rs:93](../../crates/server/npc/src/vendor.rs#L93)).

## Decisions

**D1 — The trace is the rule, and the boolean is a reading of it.** Not a second
walk beside the first. `sight_clear` is `trace(...).clear()`, one loop, one
set of thresholds. A diagnostic assembled separately would be a picture of a
different ray, which is [`docs/render/design_frame_assembly.md`](../render/design_frame_assembly.md)'s standing complaint about
this codebase's seven ways of assembling one frame — and here the drift would be
invisible, since a wrong overlay looks exactly like a right one.

**D2 — Extent is a parameter, because a shot and a picture want different
walks.** A shot wants the first blocker and nothing after it; a picture wants
every tile, including the ones behind the wall, or the line ends in mid-air with
no indication which end the player is on. One `Extent` enum, one branch inside
the loop over whether to record the step. The early return survives — combat runs
this per tick per running action and aggro runs it per creature per tick.

**D3 — The client computes it, and does not ask the shard for it.** The
client already owns everything the walk reads: the same map files, the same
tiledata, an `Overlay` it builds itself out of ground items
([clutter.rs](../../crates/client/app/src/clutter.rs)), and the same
`openshard-movement` crate — `crates/client/app` depends on it today, and
`route_shown` ([picking_query.rs:659](../../crates/client/app/src/picking_query.rs#L659))
already plans an A\* route through `footing(&self.resources, ..)` for exactly
this reason. So the overlay costs **no packet at all** and updates while the
cursor moves, which a wire round trip could not.

The honest limit of that: the client's overlay is built from what the shard has
told it about, so a door it has not been sent is a door it cannot see. `.sight`
is the cross-check that turns this from an assumption into a measurement.

**D4 — Drawn in egui, beside the route, not as a new render pass.** `Route` is
the precedent (`draw_route`, [shell.rs:3883](../../crates/client/app/src/shell.rs#L3883)):
a diagnostic line of tile centres, coloured by what it means, over the finished
frame. The blocking body is the one thing that wants a *volume* rather than a
line — its `[base, top)` span is half the answer — and `Solid::faces` already
projects a box into viewport pixels, so the polygon is filled by the same
arithmetic the occluder wireframe uses rather than a second projection.

The cost of this choice, stated because [solids.rs](../../crates/client/render/src/solids.rs)'s
header states it: an egui overlay does not appear in headless frame dumps, so
this picture is not capturable by `render`'s own tests. `Route` accepted that
trade; so does this. What is tested instead is the trace itself, in
`common/movement`, where the arithmetic lives.

**D5 — The target is the shard's target when there is one, and the cursor
otherwise.** The client knows who it is attacking — `player.attacking`, written
from `AttackTarget` ([view.rs:1467](../../crates/client/net/src/view.rs#L1467)) —
and that is the pair of points the shard is really asking about. With no fight
on, the ray follows the hovered tile, which is what makes the overlay usable for
surveying a piece of map rather than only for explaining one refusal.

**D6 — The overlay is a dev-HUD toggle, off by default.** Beside the route and
the occluder wireframe in the dev window's **Tile** tab, and paid for only while
it is on. It is not a player-facing feature and no gameplay reads it.

**D7 — The reach is a knob, because the wire does not carry one.** A shot is
refused by *two* tests and the ray is only the second: `obstruction`
([combat/src/lib.rs](../../crates/server/combat/src/lib.rs)) asks `in_range` first,
Chebyshev against the reach of the weapon in hand. That number lives in the
shard's weapon table keyed by graphic, and nothing on the wire carries it — so
the client cannot know it, and reading it off the graphic in our own hands would
be a second copy of a rule the shard owns, which is exactly what D1 refused for
the ray. A person names it instead: `1` for arm's length, `10` for a bow. When
the shard does one day send it, this knob becomes the override rather than the
source.

## What the rule cannot do, as built

Every item here is a property of the *existing* rule that the picture makes
visible. None is touched by the overlay; each is a candidate for its own change.

- **A shut door stops the ray at any height.** `blocker_anywhere` is asked
  without `ray_z`, so a look over a low gate is refused, and a look at the
  cellar under a door is refused too.
- **`EYE = 9` is everybody's eye.** A dragon, a rabbit and a mounted knight all
  sight from the same height above their own `z`.
- **The interpolation is by tile index, not by distance.** On a diagonal the
  ray's height at a given tile is slightly off where the straight line in world
  space actually is.
- **Items and mobiles are not in the way at all.** Structural house walls are
  the live-layer exception; ordinary furniture remains transparent to the ray,
  so a barricade of crates — a thing that visibly stops a body — stops no arrow.
- **The client is not told how far its own weapon reaches.** D7's knob is a
  person standing in for a packet: the shard decides `in_range` off its weapon
  table and says nothing about it until asked by `.sight`. A reach on the wire —
  a field in the player's status, not a diagnostic packet — is what would let
  the overlay stop guessing, and it is a protocol-sized conversation rather than
  a line of this document.
- **Melee reach is a constant, not a column.** `MELEE_REACH` is one tile for
  every weapon that is not a bow, which is the seam a polearm at two tiles falls
  on — [`design_actions.md`](design_actions.md) owns that one.
- **A tree blocks only if its art carries the flags.** Trunks generally do;
  foliage and bushes generally do not. This is correct as far as the tile data
  goes, and it is the kind of thing a player reports as a bug.
