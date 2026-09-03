# Making the ray legible: the five phases

*A record, not a status. Written as the sight overlay was built and kept
verbatim; the decisions it was built against are
[`design_sight.md`](../design_sight.md), whose `D`-numbers are quoted below, and
what the rule still cannot do is that document's closing list.*

**All five are built.** What follows is the plan as it was taken, with each
phase's DoD as it was met. Ф1–Ф4 were the plan as written; Ф5 is the half of a
refusal the first four never drew, found by looking at a picture that said
"clear" about a shot that was not one.

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
the protocol's own direction, and the two strings sit one above the other in the
same journal.

**DoD.** `.sight`, click a spot behind a shut door: the shard names the door,
and the overlay's own line names the same door. Where they differ, the
difference is in the live layer, which is D3's stated limit.

### Ф5 — the reach, and the other half of a refusal

The ray is one of the two tests a shot passes, and the overlay drew only it: a
clear green line to an orc fourteen tiles off, over open ground, from a bow that
reaches ten. Both ends now say the whole thing.

- `Point::distance` ([world.rs](../../../crates/common/protocol/src/world.rs)) — UO's
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
