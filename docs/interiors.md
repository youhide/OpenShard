# A building, and what it lets you see

Three things a person asked for, and they are one subject: **who decides how
much of a building is drawn.**

1. **The storey is a choice.** Today one function decides — the client's own port
   of `UpdateMaxDrawZ` — and nobody can overrule it. A person wants Auto or a
   storey they picked, on Page Up / Page Down, with the switch in the F1 window.
2. **A sealed room is a black area.** Not "its contents are not drawn" — *the
   room is not there*: no floor, no walls seen from inside, no furniture, no
   creature. And it is **any sealed room**, not only a house: a vault in a
   dungeon, a back office behind a shut door, a cellar. This needs an **index of
   rooms**, and it is the dear phase of the three.
3. **Walls at knee height.** Inside a building, or walking past one, the walls
   stop at the knee instead of standing full height, so the room behind them is
   visible.

They share a seam. All three are the same question asked of a different
predicate, and the whole of that predicate lives in one module today
([`cutaway.rs`](../crates/client/render/src/cutaway.rs)) — which is why this is
one plan and not three.

**And 1 and 2 are the same picture.** With the roof on, a sealed room is already
invisible — the roof is drawn over it. The black area is what you see *once the
roof comes off*, which is either the Auto cutaway doing its job or a person on
Page Up doing it by hand. So R2 is not a rule that fires on its own: it is what
stops R1 from turning every roof key into an x-ray.

## The order

**R0 first — the refactor, on its own, with no feature in it.** Then the keys,
then the rooms, then the knee. R0 is not tidying: two of its three findings are
places where this plan would otherwise add a second way for one frame to
disagree with itself, and the third is what the storey keys need before they can
be bound at all.

| | what | why in this order |
|---|---|---|
| **R0** | One `Cutaway` computation, a `Key` for the geometry cache, a `Hotkey` that can hold a modifier | Each is a precondition of a phase below, and each is a change nobody can see |
| **R1** | Storeys and roofs on the keyboard and in F1 | The cheapest of the three, and it is what makes R2 visible enough to judge |
| **R2** | The room index, and the black area | The dear one. Staged so the index can be looked at before anything is gated on it |
| **R3** | Walls at knee height | Smallest change, last, because its scope wants R2's rooms |

## What exists, and where

| | |
|---|---|
| The rule | `Cutaway { max_z, max_ground_z, no_draw_roofs }`, `Cutaway::at(map, tiledata, player, draw_roofs)` — `render/src/cutaway.rs:180-310`. A port of `GameScene.UpdateMaxDrawZ`, reading the player's own tile and the one diagonally in front of it. |
| Who asks it | `ground.rs:217` (`shows_land`), `mobiles.rs:507,648` (`shows_mobile`), `statics.rs:847` (`shows_static`), `occlusion.rs:1022` (`shows_at`, for the shadow grid). Four readers, one rule. |
| Who computes it | **Two places, copy for copy** — `app/src/presentation.rs:1483` and `app/src/ui_command.rs:451`, each spelling `if cutaway_disabled { OPEN } else { at(..) }`. Plus every tool and test: `render/tests/{post,dump,lid,parity}.rs`, `statics.rs:1964`. This is [`parity.md`](parity.md)'s complaint, on this exact rule. |
| The switch that exists | `GraphicsSettings::cutaway_disabled` (`app/src/graphics.rs:51`), `OPENSHARD_DISABLE_CUTAWAY`, a checkbox in the F1 World tab (`shell.rs:912-936`), plumbed through `shell::Request` → `picking_query.rs:675`. |
| The keys | `Hotkey` (`app/src/keyboard.rs:272-320`), a 19-entry table with `key()` as the single forward statement and `of()` scanning it. **Page Up / Page Down are taken** — `PanUp`/`PanDown`, the camera pan. `of()` takes a bare `KeyCode` and cannot express a chord. |
| What a tile has over it | `Occlusion::sky_at(x, y) -> u8` against `SKY_OPEN` (`render/src/occlusion.rs:1338,1929`) — the sky field the ambient is built from. A **column**'s answer, not a storey's; see R2a for why that matters. |
| What stops a step, and a look | `Terrain::sight_clear` on both ends (`common/movement/src/terrain.rs:657`, `server/state/src/obstruct.rs:331`), `Blocker::door` on the client's own `Clutter`, `Obstructions::blocker_at_z` on the server's. The client's `sight_clear` (`app/src/clutter.rs:362`) **has never had a reader** — `docs/client.md:3491` files it as exactly that. |
| A fragment's world position | Per pixel, in the shader: `shaders/statics.wesl`'s `fs_main` meets the view ray with the boxes the instance stands as (`nearest`), and `at = best.at` **is the world point this pixel is a picture of**. Phase 6 of [`lighting_rebuild.md`](lighting_rebuild.md). This is what makes R3 a shader line rather than a re-cut of the art. |
| What an undrawn pixel is | `renderer::CLEAR` — transparent black, `a = 0`, and every fragment shader writes `a = 1`. So "was anything drawn here" **is one byte**, which is the acceptance instrument R2 is measured with and the reason "black area" needs no new mechanism to paint. |

### Three facts that bound the scope

- **The client draws no multis.** Nothing in `client/render` expands
  `0x4000 | id` into components. A "house" in this client's picture is **map
  statics** — Britain's own buildings, and every dungeon — and a player house
  placed by [`housing.md`](housing.md) is an item the client does not draw at
  all. Every rule below is written over map statics and server items, and the day
  multis are drawn they arrive as more of the first kind.
- **The static geometry cache is keyed on the `Cutaway`**
  (`app/src/world.rs:229,269`). Any new input to "is this drawn" that is not in
  that key is a stale frame that only shows up while walking. R2 adds such an
  input, and R0b is what gives it somewhere to go.
- **The map does not change.** Rooms are a function of the map's statics, which
  are read-only files. That is the whole reason an index is affordable: it is
  baked per block and cached, and the only thing that moves is a door.

## Decisions, taken here

**D1 — one policy object, computed once.** `Cutaway::at`'s `draw_roofs: bool`
becomes a `Storeys` policy, and the two copies at `presentation.rs:1483` and
`ui_command.rs:451` become one `App::cutaway()`. This is a **precondition, not a
tidy-up**: a picture drawn under one storey and a click tested under another is
not a bug you find by looking, and R1 doubles the number of ways the two can
disagree. Tools and tests keep calling `Cutaway::at` directly with an explicit
policy — that is honest, they *are* a different frame — but the client has one
caller.

**D2 — the manual unit is a storey of 20 z, named and shown.** Not a ladder
inferred from whatever happens to stand in the player's column: that is a guess
whose answer changes as you walk, and a control that moves under you is worse
than no control. `STOREY: i32 = 20` is what a UO building steps by (housing's own
D9a says a villa's roof stands twenty above its foundation), the F1 panel shows
the resulting `max_z` in world units, and a slider next to it moves that number
directly for anyone who needs a cut the storeys do not name.

**D3 — level 0 is the floor the player stands on.** `Manual { level }` cuts at
`floor_z + (level + 1) * STOREY`, where `floor_z` is the surface under the player
— which the client already knows, it is what `cutaway_at` carries. So level 0
draws the storey you are in and nothing above it; +1 adds the one above; −1 takes
the ceiling of the cellar off from inside it.

**D4 — manual mode does not set `no_draw_roofs`.** It has no need to: a roof
above the cut is removed by `max_z` alone, and a roof *below* it — a porch, a
lean-to on the storey you are looking at — is part of the picture you asked for.
`no_draw_roofs` stays what it is, the Auto rule's own flag.

**D5 — the mode is not persisted.** `Desk` remembers the light, the zoom, the
fonts. It will not remember this. A client that reopens with the roofs off is a
bug report, and the cost of not persisting is one key press.

**D6 — the room rule is client-side, and it is a picture rule.** The server keeps
sending what it sends. What this buys is the frame; it buys **no** cheat
resistance, and this document says so rather than letting somebody discover it.
When the server half is wanted it is a gate on interest management using
`obstruct.rs`'s `sight_clear` — the predicate is already written, on the right
side of the wire — and R2 is what tells us whether the rule is worth that.

**D7 — a room is a component of *cells*, not of tiles.** A tile is a column and a
column holds several rooms: the cellar, the shop, the bedroom above it. The unit
is a **cell** — one tile, one floor height, one ceiling height — and a room is a
connected set of them. `sky_at` cannot be the unit for the same reason: it is one
byte per column, and it would make a cellar and the room above it one place.

**D8 — doors are baked shut and opened per frame.** The index is built with every
door treated as a **wall**, so a door never merges two rooms in the baked data.
What is recorded beside the rooms is a **portal list**: which two rooms each door
joins. Per frame, the small portal graph is walked through the doors that are
actually open. The alternative — baking doors open and splitting per frame — is
the same answer computed the expensive way round, and it makes every door a
reason to rebake.

**D9 — sealed is "not reachable from the sky".** A cell whose ceiling is open sky
is an **outdoor** cell. A room containing one is outdoors and is never blacked.
Every other room is sealed until the portal walk reaches it from an outdoor room
through open doors. This is one rule and it covers the cases a list of cases
would miss: a courtyard is open, a porch is open, a cave mouth is open, and a
vault three doors deep is not.

**D10 — the player's own room is never blacked, and neither is anything joined to
it.** The walk starts from *both* ends: the outdoor rooms, and the room the
player is standing in. So a player inside a sealed vault sees the vault, and the
corridor behind the open door they came through, and not the sealed room next to
it.

**D11 — black is what is already there.** No black pass, no fill, no new
primitive: the gated cells simply draw nothing, and `renderer::CLEAR` is
transparent black with `a = 0`. That also makes the acceptance test exact — the
room's screen rectangle is pixels whose alpha byte is zero — which is what
[`parity.md`](parity.md) would otherwise have us eyeballing.

**D12 — the knee cut is the picture only.** The fragment above the clip height is
discarded; the occlusion box keeps its full height, so the wall still stops light
and still casts its shadow. **The visible cost is a shadow with no wall over
it**, and that is the accepted trade for a first version — written here so the
first person to see it does not open it as a defect.
[`lighting_pitfalls.md`](lighting_pitfalls.md) is the ladder for anything else
the cut appears to do.

**D13 — what counts as a wall is decided on the CPU, off the tiledata.** The
shader will not guess from a face normal that a box is a wall. `TileFlags::WALL`
is read where the instance is built and carried as a bit, the same way `roof`
already rides on a `Solid`. A stance-based guess would cut the side of every
crate and the riser of every stair.

## R0 — the refactor, with no feature in it

Three changes, none of which a person can see. Each is a precondition named by a
phase below, and each is worth its own commit.

- **R0a — one `Cutaway` computation.** `App::cutaway()` on `App`, returning the
  frame's `Cutaway` from `graphics` + `world.presentation.cutaway_at`. Both
  existing callers go through it. D1. A test asserts the picture and the pick
  were drawn under the same rule — which is a statement nothing makes today.
- **R0b — the geometry cache key is a struct.** `StaticGeometryCache::new` and
  `matches` take **the identical eight-argument list** twice
  (`world.rs:240-282`), with `#[allow(clippy::too_many_arguments)]` on both and
  a comment saying it wants a `Key` and that the change "belongs to whoever next
  works on this cache". R2 is that person, and it arrives wanting a ninth field.
  Two argument lists that must agree is exactly the shape of defect that gets
  found by walking around.
- **R0c — a `Hotkey` can hold a modifier.** `Hotkey::of` takes a bare `KeyCode`,
  so no chord can be bound. R1 needs Ctrl+Page Up the moment the storey keys take
  Page Up. Keep the table's own property while doing it: `key()` is the single
  forward statement and `of()` scans it, deliberately, so there is one fact and
  not two.

## R1 — which storey is drawn, and who chooses

**What a person sees:** F1 → World → *Storeys*: Auto or Manual, and in Manual,
Page Up and Page Down step a floor at a time. The camera pan moves to Ctrl+Page
Up / Ctrl+Page Down.

- **R1a — the policy.** In `cutaway.rs`:
  ```rust
  pub enum Storeys {
      /// The client's own rule: `UpdateMaxDrawZ`, with the roof setting it takes.
      Auto { draw_roofs: bool },
      /// A cut a person chose. `level` is storeys above the floor they stand on.
      Manual { level: i8 },
  }
  ```
  `Cutaway::at(map, tiledata, player, storeys)` takes it; `Manual` needs no map
  walk at all — it is `floor_z + (level + 1) * STOREY` into all three fields per
  D3/D4. `Cutaway::OPEN` stays what `cutaway_disabled` answers with.
- **R1b — the keys.** `Hotkey::StoreyUp` / `StoreyDown` on `PageUp`/`PageDown`;
  `PanUp`/`PanDown` become Ctrl-modified, on R0c. Pressing a storey key while
  Auto is on **switches to Manual at the level that reproduces the current
  picture**, so the first press changes nothing but the mode — a person stepping
  off Auto does not lose their place.
- **R1c — the roof, separately.** `Profile.DrawRoofs` exists in the port as
  `draw_roofs` and is passed `true` from everywhere. It becomes a switch of its
  own here — roofs on/off independent of the storey — because "take the roof off"
  and "look at the third floor" are two things a person wants separately, and the
  port already has the field for the first.
- **R1d — the panel.** A *Storeys* group in the World tab beside the existing
  cutaway checkbox: the Auto/Manual radio, the level, the resulting `max_z`, the
  roof switch, and — read-only — what Auto would have answered, so the two can be
  compared without toggling. The `Request` field follows the pattern
  `cutaway_disabled` already set (`shell.rs:99` → `picking_query.rs:675`).

**What a test pins:** that `Manual { level: 0 }` standing on a second floor cuts
above that floor and not above the ground floor (D3 is about `floor_z`, not the
player's raw `z`); that a storey key in Auto lands on the level whose `Cutaway`
equals the Auto one; that `Storeys` reaches the frame only through `Cutaway`, so
the cache key needs nothing new.

**Open, needs a person's answer:** whether Manual mode says so anywhere outside
the F1 window. A HUD line is the obvious answer and adding one is a UI decision,
not an implementation detail, so it is not taken here.

## R2 — the room index, and the black area

**What a person sees:** with the roof off — by the Auto cutaway or by Page Up —
the sealed rooms of a building are black. Walk in, and the room you are in is
there, with whatever is joined to it through an open door.

This is the phase with real design in it. It is staged so that each stage
produces something that can be **looked at** before the next one is written,
because a rule about what is *not* drawn cannot be judged from a frame where it
is not drawn.

### R2a — the cell, and what a column holds

A **cell** is one tile, one floor height, one ceiling height: the space a body
could stand in. Building them is a walk of the column's surfaces — the land, and
each platform static — which `cutaway::stack` already produces in the client's
own order, and which `movement`'s `surfaces` already computes for the server's
placement rules. The two are the same walk and this is a third caller, not a
third copy: whichever of them this is written against, it is written **once**.

A cell's ceiling is the next surface above it, or the **sky** if there is none.
`sky_at` is not the input (D7) — it is one byte per column and cannot tell a
cellar from the room over it — but it *is* the cheap prefilter: a column that is
open to the sky along its whole height holds no sealed cell, and Britain is
mostly that.

**Deliverable: a debug view that paints the cells**, one colour per floor height,
over the visible rect. Nothing is gated yet.

### R2b — the room, and the portal

Two adjacent cells are joined when their z-bands overlap by a body's height and
nothing walls them apart — the same `WALL | BLOCK | NO_SHOOT` reading that
`sight_clear` uses, with `WINDOW` staying the hole it already is. Union-find over
the cells of a block gives the rooms; block borders are stitched by the same test
applied across the seam.

Doors are baked as walls (D8), and each one is recorded as a **portal**: two room
ids and the door's own identity, so the frame can ask whether it is open.

**Baked per map block and cached**, like the occlusion grid and the LOD chunks
already are — this is what D8's "the map does not change" buys. The cache is
keyed on the block, not the frame, and a block is baked once per run.

**Deliverable: the debug view paints rooms instead of cells**, one colour per
room id, so a person can see a shop and its back office as two colours and a
courtyard as none.

### R2c — sealed, per frame

A walk of the portal graph from every outdoor room and from the player's own
room, through open doors only (D9, D10). What comes back is a set of room ids
that are *shown*; every other room is sealed. The graph is rooms and doors, not
tiles — a screenful is tens of nodes, and this is the only part of R2 that runs
every frame.

**Deliverable: the debug view paints sealed rooms black and the rest as they
are.** This is the picture the feature is judged on, and it exists before
anything downstream is gated.

### R2d — the gate

A cell that belongs to a sealed room draws nothing: its floor, its statics, its
server items, its mobiles. **Not its outer walls** — those belong to the tiles
around it and are what you are meant to see. Per D11 this paints itself: the
pixels stay at `CLEAR`, alpha zero.

Which of the four cutaway readers this joins is the question R2a–R2c exist to
answer, and the standing guess is **a fifth predicate beside them, not a change
to any**: the cutaway is about height, this is about a place, and folding two
different questions into `shows_static` is how a rule becomes impossible to
attribute.

The room set joins the geometry cache key as a fingerprint (R0b's `Key` field).
An F1 switch turns the whole thing off, which is also the A/B a person judges it
with.

### R2e — the cost

Measured, not assumed. The bake is per block and amortised; the per-frame part is
the portal walk plus a room lookup per drawn cell. If it does not measure out,
**this is where the plan stops** and D6's server-side gate becomes the way in
rather than an optimisation of this one.

**What a test pins:** a room with a door — shut, it is black; open, it is not;
standing inside, it never is. A window is a hole for *sight* and not for
*joining* — you can see into a room through a window, which means a window'd room
is a room that is drawn, and this is the one place the two predicates deliberately
differ. A courtyard is never black. A cellar under a lit shop is a different room
from the shop. The alpha byte is the assertion, per D11.

## R3 — walls at knee height

**What a person sees:** a flag in F1, and the walls in view stand knee-high with
the room behind them visible over the top.

- **R3a — the clip, in the uniform.** `Viewport` in `statics.wesl` has a spare
  word today — `_tail`, the block's own 16-byte padding, exactly where `fringe`
  came from. It becomes the clip height, with a sentinel for "no clip".
- **R3b — which fragments.** Only a fragment whose met box is a wall, per D13: a
  bit read from `TileFlags::WALL` where the instance row is built and carried in
  the word `Volume::edges` already rides in. Then in `fs_main`, after `nearest()`
  has answered and `at` is the world point: `if wall && at.z > clip { discard; }`.
  A discarded fragment writes no depth and no G-buffer id, so what is behind the
  wall is drawn by the pass that owns it — no second draw, no ordering to arrange.
- **R3c — the height.** `clip = base_z + KNEE`, where `base_z` is the static's own
  `z` (the instance already carries it — `in.place.y & 0xFF` less 128) and `KNEE`
  is a named constant with a slider in F1 over `0..=16`. Sixteen is
  `CHARACTER_HEIGHT`, which makes the top of the range "as tall as a person" and
  gives the slider an end that means something.
- **R3d — the cut edge.** The first version cuts hard: the art's own texels stop,
  and the top is a raw cross-section. The soft version — the last few z units
  fading out — is not a new mechanism either: the cutaway already owns a late
  translucent layer and a per-object fade ramp (`TRANSLUCENT_ALPHA`, `Fades`,
  `cutaway.rs:60-160`). It is a second step because the hard cut is what tells us
  whether the feature is right at all.
- **R3e — the scope.** The first version is a global flag: every wall in view,
  which is what was asked for. The scoped version — only the building you are in
  or passing — has its input already, because that is R2's rooms. **This is where
  the phases meet**, and it is why R3 is last despite being the smallest change.

**Known consequences, named rather than discovered:**

- **Picking.** The click test is CPU-side and knows nothing about a shader
  discard, so a person will be able to click a wall they cannot see. Either
  picking asks the same clip (the honest fix, and it needs the clip on the CPU
  side too) or the flag stays diagnostic-only. Decide it in R3, do not leave it.
- **The silhouette and outline passes** draw from the same fragments and will cut
  with them; that is right, and worth one look at a magnified frame because
  [`silhouettes.md`](silhouettes.md) is about exactly that boundary.
- **The shadow of a wall that is not there**, per D12.

## What this plan does not cover

- **Server-side visibility.** D6. The predicate exists (`obstruct.rs:331`); the
  gate on interest management does not, and nothing here writes it.
- **Multis.** The client draws none, so a player's own house is outside all three
  rules until it is drawn at all.
- **Regions as the notion of "inside".** [`housing.md`](housing.md)'s H6 is where
  house-as-region lives, and it is a server fact. R2 deliberately answers "which
  room is this" from the picture's own substrate, because the client must answer
  it for Britain's buildings and every dungeon, which are no region and no house.
- **Rooms as a gameplay fact.** A room id would answer a great deal on the server
  — who hears you, where a spawn belongs, what a lockdown covers. This index is
  the client's, for drawing. Whether the server wants its own is a question this
  plan raises and does not answer.

## Backlog, found while planning this

- **`Cutaway` is computed in two places and asked in four.** R0a fixes the client;
  the tools and tests each spell their own, which is `parity.md`'s standing
  complaint and is *correct* for a tool — but nothing today makes a new tool's
  author notice they have joined a list of seven.
- **`StaticGeometryCache` keys on eight fields through two identical argument
  lists** (`world.rs:240-282`), with its own comment saying they want to be a
  `Key` struct. R0b.
- **`Hotkey::of` takes a bare `KeyCode`** and so cannot express a modifier. R0c.
- **A column's surfaces are walked in at least two places** — `cutaway::stack` on
  the client, `movement`'s `surfaces`/`spawn_z` on the server — with the same
  question asked of the same files. R2a is a third caller and the moment to
  decide whether it is one function.
- **`Cluttered::sight_clear` is the map's answer only**, missing the shut-door
  half the server has, with the reason on record (`docs/client.md:3491`) being
  that it has no reader. R2b gives it one — the wall test, at least — and when it
  lands, the shared arithmetic wants to live in `common/movement` once rather
  than on both ends, which is the same finding `client.md` files one bullet
  earlier about `blocker_at_z` / `blocked_at`.
- **`Viewport._tail` is the second spare word this pass has spent on a knob**
  (`fringe` was the first). A third will not fit, and the block wants a real
  layout rather than a third rummage through its padding.
