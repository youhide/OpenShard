# A building, and what it lets you see

Three things a person asked for, and they are one subject: **who decides how
much of a building is drawn.**

1. **The storey is a choice.** Today one function decides — the client's own port
   of `UpdateMaxDrawZ` — and nobody can overrule it. A person wants Auto or a
   storey they picked, on Page Up / Page Down, with the switch in the F1 window.
2. **A closed building keeps its inside to itself.** Standing outside, you should
   not see the furniture through the wall you cannot walk through.
3. **Walls at knee height.** Inside a building, or walking past one, the walls
   stop at the knee instead of standing full height, so the room behind them is
   visible.

They share a seam. All three are the same question asked of a different
predicate, and the whole of that predicate lives in one module today
([`cutaway.rs`](../crates/client/render/src/cutaway.rs)) — which is why this is
one plan and not three.

## What exists, and where

| | |
|---|---|
| The rule | `Cutaway { max_z, max_ground_z, no_draw_roofs }`, `Cutaway::at(map, tiledata, player, draw_roofs)` — `render/src/cutaway.rs:180-310`. A port of `GameScene.UpdateMaxDrawZ`, reading the player's own tile and the one diagonally in front of it. |
| Who asks it | `ground.rs:217` (`shows_land`), `mobiles.rs:507,648` (`shows_mobile`), `statics.rs:847` (`shows_static`), `occlusion.rs:1022` (`shows_at`, for the shadow grid). Four readers, one rule. |
| Who computes it | **Two places, copy for copy** — `app/src/presentation.rs:1483` and `app/src/ui_command.rs:451`, each spelling `if cutaway_disabled { OPEN } else { at(..) }`. Plus every tool and test: `render/tests/{post,dump,lid,parity}.rs`, `statics.rs:1964`. This is [`parity.md`](parity.md)'s complaint, on this exact rule. |
| The switch that exists | `GraphicsSettings::cutaway_disabled` (`app/src/graphics.rs:51`), `OPENSHARD_DISABLE_CUTAWAY`, a checkbox in the F1 World tab (`shell.rs:912-936`), plumbed through `shell::Request` → `picking_query.rs:675`. |
| The keys | `Hotkey` (`app/src/keyboard.rs:272-320`), a 19-entry table with `key()` as the single forward statement. **Page Up / Page Down are taken** — `PanUp`/`PanDown`, the camera pan. |
| Line of sight | `Terrain::sight_clear` exists on **both** ends: `common/movement/src/terrain.rs:657` (the map's own answer, with `WINDOW` as the deliberate hole) and `server/state/src/obstruct.rs:331` (that, plus a shut door). The client has it through `Cluttered` (`app/src/clutter.rs:362`) and **has never had a reader** — `docs/client.md:3491` files it as exactly that, "a rule with no reader is one nobody notices going wrong". |
| Is a tile indoors | Already measured: `Occlusion::sky_at(x, y) -> u8` against `SKY_OPEN` (`render/src/occlusion.rs:1338,1929`), the sky field the ambient is built from. |
| A fragment's world position | Per pixel, in the shader: `shaders/statics.wesl`'s `fs_main` meets the view ray with the boxes the instance stands as (`nearest`), and `at = best.at` **is the world point this pixel is a picture of**. Phase 6 of [`lighting_rebuild.md`](lighting_rebuild.md). This is what makes task 3 a shader line rather than a re-cut of the art. |

### Two facts that bound the scope

- **The client draws no multis.** Nothing in `client/render` expands
  `0x4000 | id` into components. A "house" in this client's picture is **map
  statics** — Britain's own buildings — and a player house placed by
  [`housing.md`](housing.md) is an item the client does not draw at all. Every
  rule below is written over map statics and server items, and the day multis
  are drawn they arrive as more of the first kind.
- **The static geometry cache is keyed on the `Cutaway`**
  (`app/src/world.rs:229,269`). Any new input to "is this drawn" that is not in
  that key is a stale frame that only shows up while walking. Three of the
  phases below add such an input, and each one says where it joins the key.

## Decisions, taken here

**D1 — one policy object, computed once.** `Cutaway::at`'s `draw_roofs: bool`
becomes a `Storeys` policy, and the two copies at `presentation.rs:1483` and
`ui_command.rs:451` become one `App::cutaway()`. This is a **precondition, not
a tidy-up**: a picture drawn under one storey and a click tested under another
is not a bug you find by looking, and V1 doubles the number of ways the two can
disagree. Tools and tests keep calling `Cutaway::at` directly with an explicit
policy — that is honest, they *are* a different frame — but the client has one
caller.

**D2 — the manual unit is a storey of 20 z, named and shown.** Not a ladder
inferred from whatever happens to stand in the player's column: that is a guess
whose answer changes as you walk, and a control that moves under you is worse
than no control. `STOREY: i32 = 20` is what a UO building steps by (housing's
own D9a says a villa's roof stands twenty above its foundation), the F1 panel
shows the resulting `max_z` in world units, and a slider next to it moves that
number directly for anyone who needs a cut the storeys do not name.

**D3 — level 0 is the floor the player stands on.** `Manual { level }` cuts at
`floor_z + (level + 1) * STOREY`, where `floor_z` is the surface under the
player — which the client already knows, it is what `cutaway_at` carries. So
level 0 draws the storey you are in and nothing above it; +1 adds the one above;
−1 takes the ceiling of the cellar off from inside it.

**D4 — manual mode does not set `no_draw_roofs`.** It has no need to: a roof
above the cut is removed by `max_z` alone, and a roof *below* it — a porch, a
lean-to on the storey you are looking at — is part of the picture you asked for.
`no_draw_roofs` stays what it is, the Auto rule's own flag.

**D5 — the mode is not persisted.** `Desk` remembers the light, the zoom, the
fonts. It will not remember this. A client that reopens with the roofs off is a
bug report, and the cost of not persisting is one key press.

**D6 — "closed" is client-side only, and it is a picture rule.** Asked and
answered: the server keeps sending what it sends. What this buys is the frame; it
buys **no** cheat resistance, and this document says so rather than letting
somebody discover it. When the server half is wanted it is a gate on interest
management using `obstruct.rs`'s `sight_clear` — the predicate is already
written, on the right side of the wire — and V2 below is what tells us whether
the rule is worth that.

**D7 — "closed" is `sky_at` plus `sight_clear`, and neither alone.** Covered
(`sky_at(tile) < SKY_OPEN`) says *there is a lid over this tile*; the line of
sight says *and you are not the one under it*. Covered alone hides a room you are
standing in; sight alone hides the far side of every hill. The two together are
"indoors, and not your indoors".

**D8 — the knee cut is the picture only.** Asked and answered. The fragment above
the clip height is discarded; the occlusion box keeps its full height, so the
wall still stops light and still casts its shadow. **The visible cost is a
shadow with no wall over it**, and that is the accepted trade for v1 — it is
written here so that the first person to see it does not open it as a defect.
[`lighting_pitfalls.md`](lighting_pitfalls.md) is the ladder for anything else
the cut appears to do.

**D9 — what counts as a wall is decided on the CPU, off the tiledata.** The
shader will not guess from a face normal that a box is a wall. `TileFlags::WALL`
is read where the instance is built and carried as a bit, the same way `roof`
already rides on a `Solid`. A stance-based guess would cut the side of every
crate and the riser of every stair.

## The phases

### V1 — which storey is drawn, and who chooses

**What a person sees:** F1 → World → *Storeys*: Auto or Manual, and in Manual,
Page Up and Page Down step a floor at a time. The camera pan moves to
Ctrl+Page Up / Ctrl+Page Down.

- **V1a — one computation.** `App::cutaway()` on `App`, returning the frame's
  `Cutaway` from `graphics` + `world.presentation.cutaway_at`. Both existing
  callers go through it. No behaviour change; this is D1, and it lands first so
  that everything after it has one place to change.
- **V1b — the policy.** In `cutaway.rs`:
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
- **V1c — the keys.** `Hotkey::StoreyUp` / `StoreyDown` on `PageUp`/`PageDown`;
  `PanUp`/`PanDown` become Ctrl-modified. The `ALL` table grows to 21. Note the
  shape of the existing table: `key()` is the one forward statement and `of()`
  scans it, so a modifier is a change to `of()`'s signature — it takes a
  `KeyCode` today and will need the modifier state that the event loop already
  has. That is the one non-mechanical part of this step.
  Pressing a storey key while Auto is on **switches to Manual at the level that
  reproduces the current picture**, so the first press changes nothing but the
  mode — a person stepping off Auto does not lose their place.
- **V1d — the panel.** A *Storeys* group in the World tab beside the existing
  cutaway checkbox: the Auto/Manual radio, the level, the resulting `max_z`, and
  — read-only — what Auto would have answered, so the two can be compared without
  toggling. The `Request` field follows the pattern `cutaway_disabled` already
  set (`shell.rs:99` → `picking_query.rs:675`).
- **V1e — the cache.** `Cutaway` is already the cache key and `Storeys` only
  reaches the frame through it, so this phase adds nothing to `world.rs:229`.
  Worth one test that says so.

**What a test pins:** that `Manual { level: 0 }` standing on a second floor cuts
above that floor and not above the ground floor (D3 is about `floor_z`, not the
player's raw `z`); that a storey key in Auto lands on the level whose `Cutaway`
equals the Auto one; that the picture and the pick are drawn under the same
`Cutaway` after V1a (one frame, one rule — [`parity.md`](parity.md)).

**Open, needs a person's answer:** whether Manual mode says so anywhere outside
the F1 window. A HUD line is the obvious answer and adding one is a UI decision,
not an implementation detail, so it is not taken here.

### V2 — a closed building keeps its inside to itself

**What a person sees:** walking down a street, the shop's contents are not
visible through its wall. Walk in the door and they are.

This is the phase with real design in it, and it is deliberately staged so that
the cheap half can be looked at before the dear half is written.

- **V2a — "covered", from the sky field.** A per-frame `Enclosure` over the
  visible tile rect, built beside the occlusion grid the lighting already bakes:
  `covered(tile) = sky_at(tile) < SKY_OPEN`. Nothing new is computed — this reads
  a byte the sky field already holds — and the phase's first deliverable is a
  **debug view that paints it**, because a rule about what is not drawn cannot be
  judged from a frame where it is not drawn.
- **V2b — "yours", from the line of sight.** For each covered tile,
  `Cluttered::sight_clear(player_eye, tile)`. This is the reader
  `docs/client.md:3491` has been waiting for, and it arrives with the other half
  of that entry: the client's `sight_clear` gains the shut-door rule the server
  has, since the door state is a fact this end now holds (`Blocker::door`). One
  rule, both ends, per [`architecture.md`](architecture.md)'s own direction —
  the shared half belongs in `common/movement`.
- **V2c — the gate.** A covered tile that fails the sight test draws nothing
  above its floor: statics, server items and mobiles on it are skipped. The floor
  and the walls themselves stay — you are meant to see the *building*, just not
  its inside. Which of the four readers this joins is the design question the
  debug view of V2a exists to answer, and the honest guess is a fifth predicate
  beside `shows_static` rather than a change to it: the cutaway is about height,
  this is about a place.
- **V2d — the cost, and the key.** A ray per covered visible tile, every frame
  the player moves. `Enclosure` becomes a field of the static geometry cache key
  (`world.rs:229`) — a fingerprint, not the grid — and the F1 switch turns the
  whole thing off, which is also the A/B a person judges it with. **If it does
  not measure out, this is where the plan stops and the server-side gate of D6
  becomes the way in** rather than an optimisation of this one.

**What a test pins:** a room with a door — shut, the contents are gated; open,
they are not; standing inside, they never are. A window is a hole (the `WINDOW`
flag is `sight_clear`'s deliberate exception and this must not lose it). A hill
between the player and an *open* tile gates nothing.

### V3 — walls at knee height

**What a person sees:** a flag in F1, and the walls in view stand knee-high with
the room behind them visible over the top.

- **V3a — the clip, in the uniform.** `Viewport` in `statics.wesl` has a spare
  word today — `_tail`, the block's own 16-byte padding, exactly where `fringe`
  came from. It becomes the clip height, and `f32::INFINITY`/a sentinel is "no
  clip".
- **V3b — which fragments.** Only a fragment whose met box is a wall, per D9: a
  bit read from `TileFlags::WALL` where the instance row is built and carried in
  the word `Volume::edges` already rides in. Then in `fs_main`, after
  `nearest()` has answered and `at` is the world point: `if wall && at.z >
  clip { discard; }`. A discarded fragment writes no depth and no G-buffer id, so
  what is behind the wall is drawn by the pass that owns it — no second draw, no
  ordering to arrange.
- **V3c — the height.** `clip = base_z + KNEE`, where `base_z` is the static's
  own `z` (the instance already carries it — `in.place.y & 0xFF` less 128) and
  `KNEE` is a named constant with a slider in F1 over `0..=16`. Sixteen is
  `CHARACTER_HEIGHT`, which makes the top of the range "as tall as a person" and
  gives the slider an end that means something.
- **V3d — the cut edge.** v1 cuts hard: the art's own texels stop, and the top is
  a raw cross-section. The soft version — the last few z units fading out — is
  not a new mechanism either: the cutaway already owns a late translucent layer
  and a per-object fade ramp (`TRANSLUCENT_ALPHA`, `Fades`, `cutaway.rs:60-160`).
  It is a second step because the hard cut is what tells us whether the feature
  is right at all.
- **V3e — the scope.** v1 is a global flag: every wall in view, which is what was
  asked for. The scoped version — only the building you are in or passing — has
  its input already, because that is V2's `Enclosure`. **This is where the three
  tasks meet**, and it is the reason V2 comes before V3 in the order even though
  V3 is the smaller change.

**Known consequences, named rather than discovered:**

- **Picking.** The click test is CPU-side and knows nothing about a shader
  discard, so a person will be able to click a wall they cannot see. Either
  picking asks the same clip (the honest fix, and it needs the clip on the CPU
  side too) or the flag is diagnostic-only. Decide it in V3, do not leave it.
- **The silhouette and outline passes** draw from the same fragments and will cut
  with them; that is right, and worth one look at a magnified frame because
  [`silhouettes.md`](silhouettes.md) is about exactly that boundary.
- **The shadow of a wall that is not there**, per D8.

## What this plan does not cover

- **Server-side visibility.** D6. The predicate exists (`obstruct.rs:331`); the
  gate on interest management does not, and nothing here writes it.
- **Multis.** The client draws none, so a player's own house is outside all three
  rules until it is drawn at all.
- **Regions as the notion of "inside a house".** [`housing.md`](housing.md)'s H6
  is where house-as-region lives, and it is a server fact. V2 deliberately
  answers "am I inside" from the picture's own substrate — the sky field and a
  ray — because the client must answer it for Britain's buildings, which are no
  region and no house.
- **`Profile.DrawRoofs` as a player setting.** The flag exists in the port as
  `draw_roofs` and is passed `true` everywhere. If it becomes a person's setting
  it is a line in V1d's panel and nothing below changes.

## Backlog, found while planning this

- **`Cutaway` is computed in two places and asked in four.** V1a fixes the client;
  the tools and tests each spell their own, which is `parity.md`'s standing
  complaint and is *correct* for a tool — but nothing today makes a new tool's
  author notice they have joined a list of seven.
- **`StaticGeometryCache` keys on eight fields through two identical argument
  lists** (`world.rs:240-282`), with its own comment saying they want to be a
  `Key` struct and that it belongs to "whoever next works on this cache". V2d is
  that person.
- **`Hotkey::of` takes a bare `KeyCode`** and so cannot express a modifier; the
  first chord in the table (V1c) is what forces the signature. Worth doing as its
  own change rather than inside a feature.
- **`Cluttered::sight_clear` is the map's answer only**, missing the shut-door
  half the server has, with the reason on record (`docs/client.md:3491`) being
  that it has no reader. V2b is the reader; when it lands, that entry is closed
  and the shared arithmetic wants to live in `common/movement` once rather than
  on both ends — the same finding `client.md` files one bullet earlier about
  `blocker_at_z` / `blocked_at`.
- **`Viewport._tail` is the second spare word this pass has spent on a knob**
  (`fringe` was the first). A third will not fit, and the block wants a real
  layout rather than a third rummage through its padding.
