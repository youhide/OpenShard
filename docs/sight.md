# Sight — the ray a shot is allowed by, drawn

A ranged attack asks one question before anything else happens: *is the target
in the open?* The shard answers it with `openshard_movement::sight_clear`, which
returns `true` or `false` and says nothing else. That is enough to fire an arrow
and not nearly enough to explain a refusal: a player standing in a clearing,
shooting at an orc six tiles away, is told nothing at all when the answer is no,
and a person debugging it has a boolean and a hunch.

This plan makes the ray **legible**: the same walk the shard performs, recorded
tile by tile with the reason it stopped, and drawn over the client's frame.

It is a diagnostic, not a rule change. **No phase here changes what
`sight_clear` answers** — the last phase in fact spends its whole budget on
proving that it did not.

## What the ray is, today

Two layers, in this order:

1. **The map** — `MapTerrain::sight_clear` ([terrain.rs:689](../crates/common/movement/src/terrain.rs#L689)):
   - the tiles are Bresenham's, `line_tiles` ([walk.rs:78](../crates/common/movement/src/walk.rs#L78)),
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
2. **The live world** — `sight_clear` ([walk.rs:799](../crates/common/movement/src/walk.rs#L799)) —
   which asks `Overlay::sight_blocker_at`. A crate is furniture; a shut door
   is opaque at every height; and a structural house wall stops the ray within
   its span. Wall-like components with zero tiledata height borrow one storey;
   platforms retain their real height.

Its callers: combat's `obstruction()`, which every action — a blow, a shot, a
breath — is asked at its commit and again every tick it runs, a
creature's aggro ([ai/src/lib.rs:613](../crates/server/ai/src/lib.rs#L613)), and
a vendor deciding whether it can see the customer
([vendor.rs:93](../crates/server/npc/src/vendor.rs#L93)).

## Decisions

**D1 — The trace is the rule, and the boolean is a reading of it.** Not a second
walk beside the first. `sight_clear` becomes `trace(...).clear()`, one loop, one
set of thresholds. A diagnostic assembled separately would be a picture of a
different ray, which is [`docs/parity.md`](parity.md)'s standing complaint about
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
([clutter.rs](../crates/client/app/src/clutter.rs)), and the same
`openshard-movement` crate — `crates/client/app` depends on it today, and
`route_shown` ([picking_query.rs:659](../crates/client/app/src/picking_query.rs#L659))
already plans an A\* route through `footing(&self.resources, ..)` for exactly
this reason. So the overlay costs **no packet at all** and updates while the
cursor moves, which a wire round trip could not.

The honest limit of that: the client's overlay is built from what the shard has
told it about, so a door it has not been sent is a door it cannot see. Ф4 is the
cross-check that turns this from an assumption into a measurement.

**D4 — Drawn in egui, beside the route, not as a new render pass.** `Route` is
the precedent (`draw_route`, [shell.rs:3883](../crates/client/app/src/shell.rs#L3883)):
a diagnostic line of tile centres, coloured by what it means, over the finished
frame. The blocking body is the one thing that wants a *volume* rather than a
line — its `[base, top)` span is half the answer — and `Solid::faces` already
projects a box into viewport pixels, so the polygon is filled by the same
arithmetic the occluder wireframe uses rather than a second projection.

The cost of this choice, stated because [solids.rs](../crates/client/render/src/solids.rs)'s
header states it: an egui overlay does not appear in headless frame dumps, so
this picture is not capturable by `render`'s own tests. `Route` accepted that
trade; so does this. What is tested instead is the trace itself, in
`common/movement`, where the arithmetic lives.

**D5 — The target is the shard's target when there is one, and the cursor
otherwise.** The client knows who it is attacking — `player.attacking`, written
from `AttackTarget` ([view.rs:1467](../crates/client/net/src/view.rs#L1467)) —
and that is the pair of points the shard is really asking about. With no fight
on, the ray follows the hovered tile, which is what makes the overlay usable for
surveying a piece of map rather than only for explaining one refusal.

**D6 — The overlay is a dev-HUD toggle, off by default.** Beside the route and
the occluder wireframe in the dev window's **Tile** tab, and paid for only while
it is on. It is not a player-facing feature and no gameplay reads it.

**D7 — The reach is a knob, because the wire does not carry one.** A shot is
refused by *two* tests and the ray is only the second: `obstruction`
([combat/src/lib.rs](../crates/server/combat/src/lib.rs)) asks `in_range` first,
Chebyshev against the reach of the weapon in hand. That number lives in the
shard's weapon table keyed by graphic, and nothing on the wire carries it — so
the client cannot know it, and reading it off the graphic in our own hands would
be a second copy of a rule the shard owns, which is exactly what D1 refused for
the ray. A person names it instead: `1` for arm's length, `10` for a bow. When
the shard does one day send it, this knob becomes the override rather than the
source.

## Phases

**All five are built.** What follows is the plan as it was taken, with each
phase's DoD as it was met; the closing list is the backlog it leaves behind.
Ф1–Ф4 were the plan as written; Ф5 is the half of a refusal the first four never
drew, found by looking at a picture that said "clear" about a shot that was not
one.

### Ф1 — `sight::trace`, and `sight_clear` reading it

New module `crates/common/movement/src/sight.rs`:

```rust
pub enum Stop {
    /// The land itself rose above the eye line.
    Ground { z: i32 },
    /// A static, with the span that stopped the ray and which reading gave it
    /// that span — a wall lent a storey, or a platform at its own height.
    Static { graphic: Graphic, base: i32, top: i32, wallish: bool },
    /// A shut door in the live world. It carries no span: it is opaque at any
    /// height.
    Door,
    /// A structural house wall in the live world.
    LiveWall { base: i32, top: i32 },
}

pub struct SightStep { pub tile: Tile, pub ray_z: i32, pub stop: Option<Stop> }

pub enum Extent { ToFirstBlock, WholeLine }

pub struct SightTrace {
    pub from: Point,
    pub to: Point,
    /// Every tile crossed, in order — empty under `ToFirstBlock`, which does
    /// not record what it does not need.
    pub steps: Vec<SightStep>,
    /// Where the ray first stopped, if it did.
    pub stopped: Option<SightStep>,
}

pub fn trace(footing: &Footing<'_>, from: Point, to: Point, extent: Extent) -> SightTrace;
```

- `MapTerrain::sight_clear` is **replaced** by `MapTerrain::sight_stop(tile,
  ray_z) -> Option<Stop>` — one tile's verdict at one height, with the loop, the
  `EYE` constant and the interpolation lifted into `sight.rs` where both layers
  can share them. The one caller of the old method that is not `sight_clear`
  itself is `tests/base_set_terrain.rs`, which moves to a `Footing` over an
  empty overlay — the same reading it was already making.
- `walk::sight_clear` becomes a `#[must_use]` one-liner over
  `trace(.., Extent::ToFirstBlock).clear()` and keeps its signature, so no
  caller in `combat`, `ai` or `npc` changes.

**DoD.** `cargo test -p openshard-movement -p openshard-state -p openshard-combat`
silent; the `base_set_terrain` comparison still runs and still counts blocked
looks; new unit tests in `sight.rs` covering, on a synthetic map: the window
hole, the storey lent to a zero-height wall, a platform stopping the storey
above, ground rising over the ray, a shut door, the excluded endpoints, and
`WholeLine` continuing past the first blocker while `stopped` names the first.

### Ф2 — the client's reading

- `crate::diagnostics::SightLine` — the drawable form: the trace's steps, the
  stop, and the two endpoints in world `Point`s.
- `App::sight_shown(&mut self, hover)`, next to `route_shown` and cached the
  same way (endpoints plus world snapshot), because the trace is recomputed
  every frame otherwise and the cursor moves every frame.
- Endpoints per D5. The archer's eye is their `Point` — `EYE` is the trace's
  own business and stays there.
- `graphics::Graphics::show_sight`, off by default.

**DoD.** `cargo test -p openshard-client-app` silent, and D5 itself under test:
`App::aim` takes no `self` precisely so the three cases — the quarry wins, the
cursor stands in, neither draws nothing — can be asserted without standing a
whole client up. What the *trace* answers is tested where the trace lives; a
second copy of those assertions here would be a test of `movement` wearing a
client's clothes.

### Ф3 — the picture

`draw_sight` in `shell.rs`, beside `draw_route`:

- the line, tile centre to tile centre: **green** to the stop, **red** past it,
  and solid green end to end when the ray is clear;
- a cross on the stopping tile, as an unreachable route already marks where it
  gave up;
- the blocking body as a translucent box, so `[base, top)` is visible against
  the art that drew it — this is what makes "the wall is height 0 and lent a
  storey" a thing a person can *see* rather than deduce. Built from
  `tile_corners` at the span's two heights, which is what `draw_occluders`
  already does a few lines below, rather than from `Solid::faces`: the occluder
  wireframe is the picture this one must read alike, and a box assembled the
  other way would be a second projection to keep in step;
- and the line drawn at the ray's **own height**, not on the ground — a look
  from a hill to a hollow crosses tiles it is metres above, and a line laid on
  the land draws a ray bending over every rise it passes;
- one HUD line: the verdict, and on a refusal the tile, the graphic, the span
  and the ray's own height there.

**DoD.** Run the playground, stand behind a tree, and read the overlay: it names
the tree, its span, and the ray height it stopped. `cargo clippy --workspace
--all-targets` and `cargo fmt --all` silent.

### Ф4 — the cross-check against the shard

The overlay's claim is that the client's walk and the shard's walk are the same
walk. D3 makes that true by construction for the map half and *assumed* for the
live half. So `.sight`: a cursor, and the shard tracing the same pair of points
and saying what its own ray met — the verdict, then every stop along the line
with its tile, its art, its span and the ray's height there, in the words the
HUD uses for the same facts.

**The comparison is a person's, and that is a decision rather than a shortfall.**
Making the HUD say "the two ends disagree" needs the shard's verdict to arrive
in a form the client can read as *data*, and there is no such packet: the reply
is chat text, and a client that scraped its own wording back out of a system
message would be a parser of English that breaks the day the sentence is
reworded. A diagnostic packet for one debugging command is a poor trade against
`docs/protocol_rewrite.md`'s direction, and the two strings sit one above the
other in the same journal.

**DoD.** `.sight`, click a spot behind a shut door: the shard names the door,
and the overlay's own line names the same door. Where they differ, the
difference is in the live layer, which is D3's stated limit.

### Ф5 — the reach, and the other half of a refusal

The ray is one of the two tests a shot passes, and the overlay drew only it: a
clear green line to an orc fourteen tiles off, over open ground, from a bow that
reaches ten. Both ends now say the whole thing.

- `Point::distance` ([world.rs](../crates/common/protocol/src/world.rs)) — UO's
  Chebyshev count, moved to the point itself because both ends of the wire
  measure and must agree. `sectors::distance` is now a reading of it, and the
  arithmetic exists once.
- `openshard_combat::reach_of` — the reach a fighter would commit to right now,
  which is the commit's own reading minus the action built around it.
- `.sight` says `14 tiles away, reach 10: out of reach` under its verdict. That
  line is the shard's, off the weapon actually in hand — the one number the
  client is guessing at.
- The overlay: `GraphicsSettings::sight_reach`, a knob per D7, and the line
  drawn **amber** past it with a small cross on the last tile inside it. Amber
  and not the blocker's red, because they are different refusals — red is "move",
  amber is "get closer" — and a person has to be able to tell them apart at a
  glance. The verdict line gains `— but out of reach, 14 > 10`, which is the
  sentence the picture used to leave unsaid.

**DoD.** `cargo test -p openshard-client-app -p openshard-world` silent, with
the reach's own two tests: `SightLine::within_reach`/`steps_within_reach` over a
synthetic ten-tile look, and `.sight` reporting reach 1 bare-handed and reach 10
with a bow on, over the same ground.

## What this exposes, and does not fix

Every item here is a property of the *existing* rule that the picture makes
visible. None is touched by this plan; each is a candidate for its own.

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
- **The client is not told how far its own weapon reaches.** Ф5's knob is a
  person standing in for a packet: the shard decides `in_range` off its weapon
  table and says nothing about it until asked by `.sight`. A reach on the wire —
  a field in the player's status, not a diagnostic packet — is what would let
  the overlay stop guessing, and it is a `docs/protocol_rewrite.md`-sized
  conversation rather than a line of this plan.
- **Melee reach is a constant, not a column.** `MELEE_REACH` is one tile for
  every weapon that is not a bow, which is the seam a polearm at two tiles falls
  on — `docs/combat_actions.md` owns that one.
- **A tree blocks only if its art carries the flags.** Trunks generally do;
  foliage and bushes generally do not. This is correct as far as the tile data
  goes, and it is the kind of thing a player reports as a bug.
