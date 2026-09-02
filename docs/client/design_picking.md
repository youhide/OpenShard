# Picking: what the cursor is on, and what a click sends

A hit is an opaque texel of the picture that was drawn, never the tile it stands
on, and the placement a pick replays is the one `collect` used — so what is
drawn and what is clicked cannot drift. This is the half that is built: ground
items, the double click, and the highlight. Mobiles are
[`plans/client/mobile_picking/PLAN.md`](../../plans/client/mobile_picking/PLAN.md).

Status and what is left are [`README.md`](README.md).

## M5 — interaction

Single and double click (`0x09`, `0x06`), drag and drop (`0x07`, `0x08`),
targeting (`0x6C`, `0x6B`), war mode. Speech (`0xAD`) landed early — see M4, and
so did **war mode**: the paperdoll's toggle asks for a stance and the client
folds the shard's answer (decision 8 in M4). What M5 still owes it is the
*picture* — a body drawn in its war stance, and an attack that follows from
standing in one.

**Double-click landed early too, because a door needs it.** A door is an entity
the shard placed, and the only thing that opens one is a `0x06` naming its
serial — there is no open-door packet, and what "use" means is decided entirely
server-side (`crates/server/items/src/doors.rs`). So the client's half is three
pieces and nothing about doors in any of them:

- `openshard_client_net::interact::use_object` — the `0x06`, with
  `DoubleClick::encode` in `common/protocol` as its other half. A test in each
  crate says what this client writes is what this server's own dispatch reads,
  and reads as a *use* rather than as the paperdoll request sharing the id.
- `openshard_client_render::items::pick` — **which item the cursor is over,
  picked against the picture**. Not against the tile: a door's leaf is drawn two
  tiles up the screen from the tile it stands on, so `App::pick_tile`'s answer —
  right for the Tile panel — names the tile *behind* the door and a player could
  never open the one they are pointing at. A hit is an opaque texel
  (`StaticAtlas::opaque_at`), because static art is mostly empty space and a
  bounding box picks whatever tall thing the cursor is merely inside; the
  topmost drawn wins, which is the largest `depth::Order` with the same
  later-wins tie-break the depth test gives the frame. `items::place` is the
  placement both `collect` and `pick` go through, so what is drawn and what is
  clicked cannot drift.
- `App::use_under_cursor`, on the second left click inside ClassicUO's own
  350ms (`Mouse.MOUSE_DELAY_DOUBLE_CLICK`), plus `App::item_serials` — the
  serial the renderer drops, put back by index.

The same pick runs every frame for the **highlight**: whatever the cursor is
over is drawn in `items::HIGHLIGHT_HUE` — ClassicUO's
`Constants.HIGHLIGHT_CURRENT_OBJECT_HUE`, replacing the item's own hue as the
reference does with `partial = false`, because a `hues.mul` ramp replaces a
pixel's colour rather than tinting it. Asked per frame and not remembered from
the last mouse event: the picture moves under a still cursor — the body walks,
the camera follows, a door swings — so what is pointed at is a question about
this frame's picture. `App::world_owns_pointer` gates the highlight, the tile
hover and the click alike, so a pointer over a panel lights nothing and uses
nothing.

A second way of saying the same thing — an **outline** round the sprite, pixel
first and glowing later — is planned in [`outline.md`](../render/design_outline.md). It is
additive: the hue highlight stays, and the two compose.

Nothing is done locally on the way out: the door swings when the `0x1A` that
redraws it arrives. A client that also opened it itself would show a door the
shard refused — a lock, or reach — standing open.

