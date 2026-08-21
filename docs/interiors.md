# Buildings, rendered floor by floor

Three things a person asked for concern a building's picture, but they are not
one `Cutaway` predicate. `Cutaway` is the old, global height-and-roof rule. An
interior is a separate renderer policy over one building's cells and floors.

1. **The floor is a choice.** A person wants Auto or a floor they picked, on
   Page Up / Page Down, with the switch in the F1 window.
2. **A sealed room is a black area.** Not "its contents are not drawn" — *the
   room is not there*: no floor, no walls seen from inside, no furniture, no
   creature. And it is **any sealed room**, not only a house: a vault in a
   dungeon, a back office behind a shut door, a cellar. This needs an **index of
   rooms**, and it is the dear phase of the three.
3. **Walls at knee height.** Inside a building, or walking past one, the walls
   stop at the knee instead of standing full height, so the room behind them is
   visible.

They share an index and a frame snapshot, not a type. `Cutaway` continues to
answer its established question for every map object. The new interior path
answers `is this cell of this building shown on this floor?`; it is consulted
only for geometry belonging to an indexed building. This separation matters:
one global `max_z` cannot describe a cellar below a shop, two neighbouring
buildings at different elevations, or a selected floor without also cutting the
street beside it.

**The floor view and black rooms are one building picture.** With a roof on, a
sealed room is already hidden by the roof. Once the building renderer opens the
view to a floor, the room rule prevents that action from becoming x-ray vision.
While that picture is active, the facet-wide positive-space map also withholds
every *other* building in view, including its unlabelled wall and roof contour;
the player's own labelled building is the only one admitted to the
rooms-and-floors policy.

**Outside means no house contents.** Before a player is in any indexed
building, the renderer must not expose the contents of *any* indexed building
in the camera view. The facet's positive-space labels are enough for this cheap
exterior policy: labelled tiles contribute no land, static, item or mobile
geometry and therefore remain black; contour-wall tiles stay on the ordinary
path. Entering a labelled building switches back to that building's floor and
room policy. This exterior guard is deliberately independent of the door
reachability rule below: a front door is not permission for a person standing
outside to see an entire house.

## The order

**R0 first — the refactor, on its own, with no feature in it.** Then the map
index, then the building renderer, then the knee. R0 is not tidying: it keeps
one frame from answering the existing cutaway question differently for drawing
and picking, and gives the later building-frame input a single cache key.

| | what | why in this order |
|---|---|---|
| **R0** | One existing `Cutaway` computation and a `Key` for the geometry cache | Each is a no-picture-change precondition below |
| **R1** | The building index: cells, structural floors, rooms and portals | The expensive, inspectable map fact; debug it before it decides one pixel |
| **R2** | The separate building renderer: Auto/Manual floors, rooms and controls | It consumes R1 rather than extending `Cutaway` |
| **R3** | Walls at knee height | Smallest change, last, because its scope wants R2's rooms |

## What exists, and where

| | |
|---|---|
| The rule | `Cutaway { max_z, max_ground_z, no_draw_roofs }`, `Cutaway::at(map, tiledata, player, draw_roofs)` — `render/src/cutaway.rs:180-310`. A port of `GameScene.UpdateMaxDrawZ`, reading the player's own tile and the one diagonally in front of it. |
| Who asks it | `ground.rs:217` (`shows_land`), `mobiles.rs:507,648` (`shows_mobile`), `statics.rs:847` (`shows_static`), `occlusion.rs:1022` (`shows_at`, for the shadow grid). Four readers, one rule. |
| Who computes it | `App::cutaway()` computes the client's one frame/click answer. Tools and render tests call `Cutaway::at` directly with their explicitly supplied frame. |
| The switch that exists | `GraphicsSettings::cutaway_disabled` (`app/src/graphics.rs:51`), `OPENSHARD_DISABLE_CUTAWAY`, a checkbox in the F1 World tab (`shell.rs:912-936`), plumbed through `shell::Request` → `picking_query.rs:675`. |
| The keys | `Hotkey` (`app/src/keyboard.rs:272-320`), a 19-entry table with `key()` as the single forward statement and `of()` scanning it. **Page Up / Page Down are taken** — `PanUp`/`PanDown`, the camera pan. `of()` takes a bare `KeyCode` and cannot express a chord. |
| What a tile has over it | `Occlusion::sky_at(x, y) -> u8` against `SKY_OPEN` (`render/src/occlusion.rs:1338,1929`) — the sky field the ambient is built from. A **column**'s answer, not a floor cell's; see R1a for why that matters. |
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
- **The static geometry cache has one `StaticGeometryCacheKey`**, including the
  `Cutaway`. Any new input to "is this drawn" belongs in that key; otherwise it
  is a stale frame that only shows up while walking. R2 adds the interior-frame
  fingerprint.
- **The map does not change.** Rooms are a function of the map's statics, which
  are read-only files. That is the whole reason an index is affordable: it is
  baked per block and cached, and the only thing that moves is a door.

## Decisions, taken here

**D1 — two policies, each computed once.** `App::cutaway()` is the sole client
caller of the existing global cutaway rule, so picture and picking agree.
`Interiors::frame(...)` is a separate, immutable frame value for building
rendering. It contains the selected structural floors and shown rooms, and is
passed both to frame assembly and picking. Neither policy is encoded in the
other.

**D2 — the manual unit is a structural floor, not a `z` arithmetic rule.** The
index gives a building ordered `FloorId`s from its cells' floor bands. The UI may
present a relative level, but `Manual` resolves that level to a `FloorId` before
rendering. A constant 20-z ladder is useful map evidence, not the renderer's
definition: it breaks on a cellar, a raised threshold, or a building authored
with a non-standard height.

**D3 — level 0 is the structural floor of the player's cell.** Manual level 0
shows that floor and the floors below it; +1 adds the next indexed floor; −1
opens the cellar floor. It never means “world z plus a constant”.

**D4 — floors do not mutate the roof setting.** `no_draw_roofs` remains solely
the existing cutaway rule. The interior renderer decides which indexed roof or
ceiling cells are shown for its selected floors.

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
  `matches` receive one `StaticGeometryCacheKey`, which records every input to
  cached static geometry. R2 adds its interior-frame fingerprint there rather
  than making a second key list.
- **R0c — modifier bindings wait for the actual controls.** `Hotkey::of` takes a
  bare `KeyCode` today. When R2 claims Page Up / Page Down it must grow a gesture
  value (key plus modifier), preserving `key()` as the one forward table and
  `of()` as its inverse. Do not fossilise that API before R2 has established the
  exact controls.

## R1 — the building index

R1 changes no normal frame. It makes the durable map fact the building renderer
will consume: a `BuildingId`, ordered structural `FloorId`s, cells, rooms and
door portals. Each stage has a debug view, because the data must be judged before
it can hide a pixel.

### Implementation handoff — R1 in progress

**Implemented, uncommitted:** `uofiles::surfaces::stand_surfaces` remains the
shared *movement* walk. The interior bake deliberately uses only land and
static art marked `FLOOR`: a character can stand on a table, bed or crate, but
none is a structural storey. `render::interiors` has lazy per-block `CellId`s,
excludes a gap lower than `PLAYER_HEIGHT`, and caches the
same block's local closed-door `BlockRooms`: cardinally connected, vertically
compatible cells form a room; `TileFlags::DOOR` is kept as a `Door` and a
two-sided local doorway becomes a `Portal`. `StitchedRooms` composes a chosen
set of these immutable bakes without treating an 8×8 edge as a wall: it gives
the finished component a deterministic `StitchedRoomId`, resolves a doorway
whose sides lie in different blocks, and keeps the same outdoor/player/open-door
walk. The two-block test puts the door on the seam and pins shut versus open.

`interior_census` measured body-compatible neighbour floors over central Britain
as 92.90% within Δz ≤ 2 and over Wrong as 99.28% within Δz ≤ 2. That result is
map evidence, **not** a floor identity: an elevation threshold would flatten a
stair one edge at a time. `Buildings` therefore runs two independent detectors.
`MapTerrain::can_step` supplies the map's actual walk graph: a changed-z edge is
a stair between `FloorId`s, while a level edge is inside one floor. A shared map
column is structural evidence for one `BuildingId`, but never merges the two
floor ids. Closed portals join two sealed rooms into one building and never use
the all-outdoor component as a bridge. Tests pin a walkable stair and the
separate stacked-floor evidence.

**Now inspectable, still deliberately not connected to geometry:** F1 → World
has `interiors — baked wall topology; whole buildings`. Its source is now a
facet-wide offline artifact, `openshard-interiors-<facet>.bin`, rather than a
camera block cache. The bake starts at every map boundary and floods through the
open world until a wall or a door stops it: that is the **negative space**.
Every unvisited tile is positive building space. Internal doors then join only
their two positive sides, so this first pass deliberately paints an entire house
one stable colour, while a front door never joins a house to the exterior.

The wall predicate comes from the measured art table; `BLOCK` furniture,
tabletops and movement walkability are not inputs. A roof remains vertical
headroom only: **it never defines the horizontal outline of a room**. The app
only slices the already baked labels to the camera, so panning and zooming
cannot change topology or colours. Missing or stale output disables this
diagnostic with the exact bake command; it never rebuilds during a frame:

Some shard doors are generated as live ground items, rather than stored in the
map static list. Their one- and two-tile openings are inferred offline from the
opposed catalogued wall frames, so they stop the exterior flood even before a
player has received the item. The F1 marker then reads the item's current door
graphic: green is open and red is shut.

```sh
OPENSHARD_CLIENT=/path/to/client \
  cargo run --release -p openshard-client-artscan --bin openshard-interiors-bake -- --facet 0
```

The artifact is validated against the facet map/statics, `tiledata.mul`, and
the art table (including a hand edit). It is an R1 instrument only: it has no
renderer gate, geometry-cache fingerprint, picking input, Auto/Manual state, or
keyboard binding. Therefore the normal picture is unchanged and floors do not
yet switch.

### Handoff — whole-building topology complete

- The R1e artifact is current for facet 0. It is a whole-house diagnostic only:
  every painted tile has `floor = 0`, and the stair list is intentionally empty.
- The exterior flood uses catalogued walls only. Furniture, tabletop height,
  roofs and runtime walkability do not alter its topology.
- `0x00AD` west / `0x00AB` east at Britain `1434` / `1437`, `1599` are the
  two static frames for the server-generated double door. The shared
  `movement::door_frames` table, equal-z match, and `can_fit` guard make
  `1435–1436,1599` virtual closed door anchors; consequently `1433,1596` is
  labelled as building `874` in the rebuilt artifact. The item's live graphic
  controls only the F1 marker (green open, red shut), never the baked contour.
- The debug inspector is
  `openshard-interiors-inspect -- --facet 0 --at X,Y --radius N`; use it before
  changing an exceptional building rule.

**Next work — do it as one render phase, not as more R1 overlay:** derive and
persist `FloorId`s, then stair edges; introduce selected Auto/Manual floor state
and feed it to frame assembly as the sole gate for floors, statics, server
items, and mobiles. That phase owns geometry cutaway and Page Up/Page Down.
Do not expose second-storey labels or stairs separately beforehand: they have no
user-visible meaning until the renderer can show the selected storey and hide
the rest.

- **R1a — cells and floors.** A cell is one tile, one floor band and one ceiling
  band: the space a body could stand in. Build its floors from land and static
  art marked `FLOOR`, rather than the wider movement surface walk; its ceiling
  is the next structural surface, roof, or sky. The debug view colours cells by
  floor band.
  Connected compatible bands form ordered
  `FloorId`s inside a `BuildingId`; they are not a global z ladder. The precise
  compatibility tolerance is measured against Britain and dungeons in this view
  before it becomes an implementation constant.
- **R1b — rooms and portals.** Adjacent cells join when their bands overlap by a
  body height and their **shared edge** has no catalogued wall panel.
  The finished building is a root room (no parent); stitched rooms are nodes
  below it. A low surface wholly supported by another room's ceiling is a child
  room, so a dais or short stair remains visible with its enclosing room rather
  than becoming a separately selected storey.
  A short wall whose top is exactly the supporting platform of a too-low
  under-deck volume is that platform's riser, not a room or house boundary.
  Which edge a wall occupies comes from the art-table facing measurement, not
  from `tiledata.mul` (which has no direction); unread art is the conservative
  all-edges fallback. `BLOCK` furniture — tables, beds, crates — is deliberately
  not a room boundary: it matters to movement but never divides a room. Doors
  bake as walls and record portals instead. Union-find per block plus seam
  stitching builds rooms; cache the immutable result per map block.
- **R1c — shown rooms.** Every frame, walk the small portal graph from outdoor
  rooms and the player's room through currently open doors. The result is a set
  of shown `RoomId`s. The debug view paints every other indexed room black;
  ordinary frame assembly is still not gated.
- **R1d — index acceptance.** The cell, floor, room and portal views must remain
  separately selectable in F1. The ordinary World panel stays unchanged in R1:
  Auto/Manual controls belong to the renderer in R2, after there is a real
  `FloorId` to select.
- **R1e — whole-facet exterior bake.** Build the negative-space flood once
  outside the client event loop and persist its positive labels beside the
  client install. This is the stable base for R1b: it is already sufficient to
  colour each house, while the next pass replaces each house label with
  per-room labels and preserves doors/stairs as edges in that graph.

**What R1 pins:** a cellar and shop above it are different cells and can be
different floors; a courtyard is outdoor; a shut door keeps rooms separate; an
open door and the player's own room show the correct rooms. The alpha byte
remains the final visual assertion, but only after R2 connects the index.

## R2 — the building renderer, and the black area

**What a person sees:** F1 → World → *Buildings*: Auto or Manual. In Manual,
Page Up and Page Down select structural floors one at a time; camera pan moves to
Ctrl+Page Up / Ctrl+Page Down. A selected floor shows its rooms and the floors
below it, while sealed rooms remain black until a valid open-door path or the
player's own room reaches them.

### R2a — one explicit building-frame value

`render/src/interiors.rs` owns the pure vocabulary and predicate:
```rust
pub enum FloorView { Auto, Manual { relative: i8 } }
pub struct InteriorFrame { /* BuildingId, selected FloorIds, shown RoomIds */ }
impl InteriorFrame { pub fn shows_cell(&self, cell: CellId) -> bool { /* ... */ } }
```
`Interiors::frame(index, player, doors, view)` resolves the relative choice to
the player's `BuildingId` and `FloorId`, then produces the immutable frame value.
No `Cutaway` field, `max_z`, or global map coordinate appears in this interface.
Outside an indexed building it answers “not applicable”, so the ordinary render
path stays exactly as it is.

### R2b — attach geometry to indexed cells

Static assembly records the `CellId` (or “outside indexed building”) for every
floor, static and server item. Mobile collection obtains the cell at its world
position. `InteriorFrame::shows_cell` is then the single building-specific gate
for all four. It is intentionally a different predicate from `shows_static`:
the latter remains the existing global cutaway test.

Outer walls are not owned by a hidden room and stay drawable; a hidden cell's
floor, contents and occupants produce no geometry. No black pass is needed:
those pixels remain `renderer::CLEAR`, with alpha zero.

### R2c — controls, cache and picking

The F1 *Buildings* group owns Auto/Manual, the selected relative floor and a
read-only resolved `FloorId`/elevation. The regular roof setting remains its own
existing setting. Page Up / Page Down select floors; camera pan moves to the
explicit Ctrl chord when these bindings land.

Frame assembly and an immediate click receive the same `InteriorFrame`; its
fingerprint joins `StaticGeometryCacheKey`. The panel has one enable switch for
an A/B comparison. It is not persisted (D5).

### R2d — the gate

A cell that belongs to a sealed room draws nothing: its floor, its statics, its
server items, its mobiles. **Not its outer walls** — those belong to the tiles
around it and are what you are meant to see. Per D11 this paints itself: the
pixels stay at `CLEAR`, alpha zero.

This is not a fifth `Cutaway` reader. `InteriorFrame::shows_cell` is the public
boundary of the separate building renderer, and it is composed with the existing
height rule at the assembly call sites. Folding floor selection into
`shows_static` would again make a building decision indistinguishable from a
global height decision.

### R2e — the cost

Measured, not assumed. The bake is per block and amortised; the per-frame part is
the `InteriorFrame` walk plus a cell lookup per drawn object. If it does not measure out,
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

## Remaining backlog

- **`Hotkey::of` takes a bare `KeyCode`** and so cannot express a modifier. R0c.
- **A column's surfaces are walked in at least two places** — `cutaway::stack` on
  the client, `movement`'s `surfaces`/`spawn_z` on the server — with the same
  question asked of the same files. R1a is a third caller and the moment to
  decide whether it is one function.
- **`Cluttered::sight_clear` is the map's answer only**, missing the shut-door
  half the server has, with the reason on record (`docs/client.md:3491`) being
  that it has no reader. R1b gives it one — the wall test, at least — and when it
  lands, the shared arithmetic wants to live in `common/movement` once rather
  than on both ends, which is the same finding `client.md` files one bullet
  earlier about `blocker_at_z` / `blocked_at`.
- **`Viewport._tail` is the second spare word this pass has spent on a knob**
  (`fringe` was the first). A third will not fit, and the block wants a real
  layout rather than a third rummage through its padding.
