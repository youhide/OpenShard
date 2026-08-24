# Protocol and newtype findings

[Client backlog](README.md) · [Backlog](../README.md) · [Roadmap](../../README.md)

## Backlog from the shop that disconnected the player

Found by playing: saying "buy" to a shopkeeper drew no trade window and left a
session in which nothing further worked — the paperdoll would not open. Two
defects, one of each kind the seam has, both now fixed
(`a_shop_says_nothing_the_client_cannot_read` in `world`'s tick tests is the
oracle):

- **The framing table is the authority for every byte a shard writes, and one
  packet was not in it.** `0xD6`, the property list, is written as raw bytes by
  `PropertyList::finish` and named by no `ServerPacket` variant, so nothing in
  the enum would ever have added it — and `open_shop` sends one per stocked
  item. A length the client does not know is not a packet skipped but a
  connection ended (`Connection::poll`), which is why the *paperdoll* looked
  broken afterwards: there was no shard left to answer it. The client's own
  test used `0xD6` as its example of "an id the shard never sends", so the
  assumption was written down twice and true nowhere.
- **A decoder missing where an encoder exists is silent.** `0x2E`, `0x74`,
  `0x9E`, `0x27` and `0x6C` had `EncodePacket` and a row in the table but no
  arm in `ServerPacket::decode`, so `WorldView`'s vendor fold — `vendor_stock`,
  `pending_vendor_buys`, `vendor_buys` — could never run outside its own unit
  tests. The window opened over an empty shelf while every byte of the
  catalogue had arrived. **Worth a sweep**: nothing today asserts that a
  variant this engine *sends* is a variant the client can *read*, and the
  remaining unread ones should each be a decision rather than an omission.
  `0xDC` and `0xD6` were two of the four named here, and both were exactly this
  shape — an encoder, a table row, and no arm — for as long as the entry stood;
  they were read in full on 2026-08-15 (see [`client.md`](../../../client.md)'s
  "Tooltips, and the half that was never written"), and finding them again by
  hand rather than by a failing test is the argument for the sweep. `0x14` and
  `0xBF`'s subcommands are still open.

- **A lost shard is indistinguishable from never having had one, and the one
  thing that hides it is the one thing implemented twice.** `Update::Lost`
  writes `world.link = None` and an `eprintln!`, and nothing else
  (`net_command.rs`). `App::walk` then takes its *offline* arm — the map
  viewer's, gated on `link.is_none()` alone — and moves the body locally, so
  the client keeps walking over a dead connection while `open_own_paperdoll`
  returns silently, `say`/`use_object` log to tracing, and `authoritative.view`
  keeps drawing the world as it stood at the moment of the drop. That is
  exactly what made this bug read as "the state changed" rather than as a
  disconnect. The offline fallback wants a reason — *never connected* is a map
  viewer, *lost the shard* is an error — and the loss wants to reach the
  screen, not stderr. **Fixed**, in the shape the three answers asked for:
  `world::Shard` is one field with three states (`Viewer`, `Live`, `Lost`) in
  place of the `Option<Link>` whose `None` meant both of the two that matter,
  so `App::walk`'s offline arm, `start_replay`'s guard and the scenario panel
  all ask *is this the viewer* rather than *is there a link*;
  `WorldView::shard_lost` puts out every table the shard authored and writes
  the reason into the journal, which is the one thing it keeps; and the status
  strip reads the loss off `Shard` instead of going on saying "in world".
  Left open: nothing reconnects, so the only way out is a restart.

Still not built, and not a defect: the shop *interface*. What draws now is an
ordinary container window over gump `0x0030` with the stock icons in it — no
price column, no quantity, no Buy button, and `link::Link::buy`/`sell` have no
caller (the compiler says so). `0x0030` is a marker in the reference client
rather than container art; drawing the real shop gump is its own piece of work.

## Backlog from the client newtype sweep

A pass over `crates/client/{app,artscan,net,pathtrace}` (`render` excluded on
purpose — its own newtype pass is separate work) for bare numeric fields that
carry domain meaning. The strongest cases are places where a newtype the
protocol already defines (`Serial`, `Graphic`, `RawGumpId`, `RawSwitchId`) gets
unpacked back to a primitive just to cross a struct boundary — fixed below.
What is left is lower-priority: no existing type to reuse, or the fix reaches
into a struct with enough call sites that it deserves its own pass rather than
riding along with this one.

- ~~**`app::shell::Hud` re-flattens `Serial`/`Graphic` into tuples of
  primitives.**~~ Fixed: `mobiles`/`items`/`serial` carry `Serial` and
  `Graphic` directly; `lib.rs` no longer calls `.raw()`/`.0` just to build the
  HUD snapshot.
- ~~**`app::gump::Windows` keys its maps on bare `u32`.**~~ Fixed:
  `by_dialog` and `placement`'s parameter use `GumpId` — the type
  `OpenGump::gump_id` already is, one field over — and `switches` uses
  `RawSwitchId`, what a layout's own `Switch::id` already is.
- ~~**Three copies of an implicit `Axis` enum in `pathtrace`.**~~ Fixed:
  `aabb.rs`, `vector.rs` and `camera.rs` each had their own `usize` 0/1/2 with
  a `match`-and-`panic!` on anything else. `pathtrace::Axis` replaces all
  three.
- ~~**`net`'s undecoded-packet id is a bare `u8`.**~~ Fixed: `PacketId` in
  `connection.rs`, used by `Event::Undecoded` and `LoginError::OutOfTurn`.
- ~~**`net::view::Item::amount` is the one untyped field next to
  `graphic`/`position`/`hue`.**~~ Fixed: `StackAmount`.
- ~~**`app::shell::PickedTile`, the coordinate half** — `x`/`y: u16`.~~ Fixed:
  the two fields are one `at: openshard_movement::Tile` now — "a tile's column
  and row, with no height", already the argument type of `Terrain::{ground_z,
  land_tile, statics_at, stand_z, spawn_z, can_fit}`, so this was class A and
  not a new type. Every reader (`tile_ring`, the two HUD panels,
  `draw_tile_highlight`, `App::{pick_tile, tile_info, walk_toward_cursor}` and
  the click handler) reads `.at.x`/`.at.y` now instead of two loose fields.
- ~~**`app::shell::PickedTile`, the graphic half.**~~ Fixed: `land` is
  `Option<Graphic>` and `statics` is `Vec<(Graphic, i8, Hue)>` — the types the
  neighbouring `Hud::mobiles`/`items` already carry, and no new type needed.
  The values come out of `openshard_map::map::{LandCell, StaticItem}`,
  which hold bare `u16`s of their own; `uofiles` is `common/`, so typing the
  format reader stays a separate decision and `App::tile_info` is the boundary
  the wrap happens at. The two HUD formatters destructure (`Some(Graphic(id))`,
  `for &(Graphic(id), Height(z), Hue(hue), PriorityZ(priority_z))`) rather than
  reading `.0` inline: a panel printing an id in decimal *and* hex is the
  presentation seam, the same licence the wire and SQL get. `statics` since
  gained a fourth element, `PriorityZ` (below), and `PickedTile` gained
  `tile_depth: TileDepth` and `mobile_order: Option<Order>` — the pair the Tile
  panel already read against each other in words, now typed the same way.
- ~~**`app::shell::PickedTile`, the Z half** — `land_z` / `stand_z` /
  `corners` / `levels` / `ceiling: i8`.~~ Fixed: `shell::Height(pub i8)`. The
  narrowing (`z.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8`) that used
  to run four times with its own copy of the "a corrupt block must not panic a
  HUD" comment still runs once per site — `Height` did not collapse that
  duplication, it named what the clamp was producing. `Point`, `Terrain` and
  every wire value keep their bare `i8`/`i32`: `Height` is unwrapped (`.0`) at
  exactly the two seams that meet them — `App::tile_info` building the struct,
  and `draw_tile_highlight`'s `at` closure building a `Point`. This contradicts
  nothing in `protocol_newtypes.md`: N1 amendment 2 allowlists `Point`'s
  components because *nothing reaches them except through a `Point`*, and
  `PickedTile`'s height fields are exactly the free-floating case the note
  there flagged — read by two panels and a painter, independently of any
  `Point`.
  Two more depth-sort newtypes came out of the same pass, both local to
  `shell.rs` rather than reused from `client_render::depth`: `TileDepth(pub
  i32)` for `PickedTile::tile_depth` (the `x + y` half of a draw-order key,
  alone) and `PriorityZ(pub i32)` for a static's own sort key inside
  `PickedTile::statics`. `mobile_order` reuses `depth::Order` itself rather
  than getting a third — its two fields are exactly `Order`'s `tile` and
  `priority_z`. `depth::Order`'s own fields stay bare `i32`, which is now the
  one visible seam: a mobile's sort key crosses into `PickedTile` typed, a
  static's does not, because nothing paired the two at the source. Worth
  closing if `Order` itself ever takes `TileDepth`/`PriorityZ` fields — not
  attempted here, since that reaches every caller of `Order` across the render
  pipeline, not just the HUD.
- ~~**`(u16, u16)` is the client's ad-hoc `Tile`, in ten remaining places.**~~
  Fixed: the tuple is `openshard_movement::Tile` now in
  `app::steer::{Steering::goal, go_to, plan}`, `Opening::at`/the command-line
  parser, `App::{in_bounds, tile_info, route_shown, hud}`, `app::dst`'s test
  walls, and `net::walk::Walk::step`'s height callback (`Fn(Point, Tile)`).
  `Point` still names tile-plus-height, and the new `.x`/`.y` reads sit at the
  existing seams: `WorldMap`/`MapTerrain` APIs, `Point::new`, and HUD
  presentation.

  `app::clutter` was the sharpest of them and is **fixed**: it *imported*
  `Tile` on line 41, used it in six trait methods, and then unpacked the `Tile`
  it was handed into `self.clutter.blocked_at(tile.x, tile.y, z)` to feed its
  own `HashMap<(u16, u16), _>`. `Clutter::tiles` is keyed on `Tile` now and
  `blocked_at` takes one, which deleted that unpacking outright — the protocol
  sweep's N2 amendment 1 result ("wrapping deleted `.raw()` calls"), arrived at
  from the other direction. Note what it did *not* need: no `Point` → `Tile`
  helper was added, because `Tile::new(p.x, p.y)` is already the idiom
  `movement::path` and `movement::terrain` use, and a second spelling of it
  would be the thing to avoid.
- ~~**`App::hud` takes two `Option<usize>` indices into different lists,
  positionally.**~~ Fixed: `ItemIndex` and `MobileIndex` travel separately
  through the picked-frame facts and `assemble_geometry`; `App::hud` now takes
  the named `Pick` snapshot rather than either positional index. Swapping an
  item and a mobile no longer compiles.
- ~~**`render::mobiles::Mobile::body` / `app::crowd::Tracked::body` were
  `u16`.**~~ Fixed: both are `Graphic` now, and the `Graphic` → `u16` → `Graphic`
  round trip through `crowd::Crowd::see`/`snap` is gone — `app` carries
  `Graphic` straight through. Also fixed as part of the same pass:
  `EquipConv::resolve(body: u16, item_anim_id: u16)` — the exact
  same-width-adjacent-params shape `docs/style.md`'s newtype section uses as
  its worked example — now takes `Graphic` and a new `AnimId` (below);
  `mobiles::EquipmentLayer::graphic` and `paperdoll::{Wearer::body,
  gump_of, body_gump}` followed the same body/anim-id split.
  ~~`Wanted::animations: BTreeSet<(u16, u8, u8)>` (lib.rs:792)~~ Fixed:
  animation requests now use the shared `AnimationKey`, whose body and group
  are typed; only the stored direction remains a file-format byte.
- ~~**`app::crowd::Tracked::group: u8`** — an animation group with no named
  type yet. `BodyKind::{standing, walking, running, ...}` all return bare
  `u8` from the same three-numbering table `docs/style.md`'s "three
  enumerations, same number means three different actions" comment already
  warns about — a `Group` newtype here would be a `BodyKind`-scoped one, not
  a global animation-group id.~~ Fixed: `AnimationGroup` now names the
  body-specific value throughout `BodyKind`, `Crowd::Tracked`, `Mobile` and
  `AnimationKey`; raw bytes remain only at protocol/file boundaries.
- ~~**`openshard_uofiles::tiledata::AnimId(pub u16)`** — new, this pass: the
  worn-item picture in the body-animation index space
  (`StaticTile::anim_id`, `EquipConvEntry::graphic`,
  `EquipmentLayer::graphic`), split out from `Graphic` because
  `paperdoll.rs`'s own module doc already named it a third, unrelated index
  space that `Graphic` was being reused for.~~ Fixed, and followed all the
  way into the atlas: `FrameKey::body`, `AnimAtlas`'s `asked` set and
  `build`/`add`'s `wanted` iterator, `AnimAtlas::frame_count`, and
  `Anim::{frames, has_frames}` (`common/uofiles`) all take `Graphic` now
  too, and `Wanted::animations` (lib.rs:926) followed since it feeds the
  same atlas. `animation_body` no longer opens back to `u16` at any of its
  call sites — `mobiles::place`, `needed_animations` and
  `App::advance_groups`'s `frame_count` lookup all carry `Graphic` straight
  through. What is left raw on purpose: the file-format bounds check inside
  `Anim`; `AnimationKey` now carries named `AnimationGroup` and
  `AnimationDirection` values, and `FrameKey` adds `AnimationFrameIndex`.
- **`app::desk`** — `Frame`'s `x`/`y`/`width`/`height` are physical window
  pixels, `Panel`'s are logical egui points; same shape, different unit, no
  type keeps them apart. Low priority — it's window-chrome geometry, not game
  state, but it is exactly the space-mixing `docs/style.md` warns about.
- ~~**`app::gump` held page and text-field identities as bare integers.**~~
  Fixed: `GumpPage` and `TextEntryId` carry them through the dialog state;
  `.raw()` occurs only at the renderer-layout and reply-packet seams. They
  remain local because neither name describes a protocol-wide domain yet.
- ~~**`net::walk`'s unanswered-step tally was a `usize`.**~~ Fixed:
  `InFlightSteps` names `Walk::in_flight`, `MAX_IN_FLIGHT`, and the
  `NotSent::Backlogged` diagnostic together. The internal `draining` count
  remains a separate implementation detail: it counts stale responses after a
  rollback, not the live pending queue.
- **`app::gump::text_color(hues: &Hues, hue: u32)` narrows with `as`.** Its
  body is `hues.get(Hue(hue as u16))` — a wire hue that arrived as a `u32`
  because `GumpLayout`'s builder methods (`label`, `croppedtext`, …) declare
  their hue parameter as `u32`, matching the layout language's decimal
  arguments. The `as` silently keeps the low sixteen bits of anything larger.
  Class A on this end (`Hue` exists); on the `protocol` end it is the same
  shape as the four `u32` cliloc parameters on `GumpLayout` that
  `protocol_newtypes.md`'s N-gump backlog already names, and probably wants
  fixing there rather than here.
- ~~**`pathtrace::Image::visibility(x: u32, y: u32, light: usize)`.**~~ Fixed:
  `ImagePixel` now names an image-grid coordinate and `LightIdx` names the
  light-list index, so the image owns the only bounds check over both. The
  tracer tests and renderer oracle carry both types instead of positional
  integers.
- ~~**`pathtrace`'s `width`/`height` travel as two loose `u32`s.**~~ Fixed:
  `trace::ImageSize` now crosses the tracer's public `render` API, lives in
  `Image`, and follows the renderer-side `Mirror`/oracle `Frame` all the way
  through comparison. The raw pair stops at the GPU and PNG seams, where those
  APIs require it. Not per-axis newtypes: the precedent is `MapSize` (N1
  amendment 3 of `protocol_newtypes.md`) — one named pair, because half a
  resolution is not a smaller number, it is a frame of the wrong shape.
- ~~**`app::desk::Desk::fits` throws away the struct it is about.**~~ Fixed:
  `desk::Monitor` names a screen's physical rectangle from winit through the
  saved-frame visibility check. It is deliberately distinct from `Frame`:
  monitor bounds are an outer physical rectangle; a saved frame is an outer
  origin plus an inner window size. Same low priority as the `Frame`/`Panel`
  unit-mixing item above, and the same pass.
- `crates/client/artscan` had no candidates — its public API is already fully
  typed. Re-checked in this pass: still true.
