# Plan: picking a mobile

Nothing that breathes can be clicked. Four of the five decisions below are
already taken by `items::pick` — the point of writing this down before building
it is that they must not be re-taken differently. The half that *is* built is
[`docs/client/design_picking.md`](../../../docs/client/design_picking.md).

## Picking a mobile — planned, not built

Today **nothing that breathes can be clicked**: `App::use_under_cursor` asks
`items::pick`, which walks ground items and nothing else, so the crowd is
scenery the cursor passes through. That was not a decision; a mobile simply has
no pick yet. The gap has a way of reading as a rendering bug — a creature drawn
from the wrong body looks like a broken sprite *and* refuses to answer the
mouse, and only one of those two is the defect (see the backlog entry below).

The shape of the answer is the item pick, one crate over, and the point of
writing it down before building it is that four of its five decisions are
already taken by `items::pick` and must not be re-taken differently.

1. **Pick against the picture, not the tile, and a hit is an opaque texel.** The
   same two rules and the same reasons as `items::pick`: a dragon's sprite is
   most of a screen of empty space, and a tile test names whatever the feet
   stand on. `AnimAtlas` needs the `opaque_at` that `StaticAtlas` has —
   `PackedFrame` already holds an origin and the atlas already keeps `pixels`
   on the CPU, so it is the same six lines keyed by `FrameKey` instead of
   `Graphic`.
2. **Go through `mobiles::place`.** It is already the one placement `collect`
   and `head_anchor` share, and a pick that computed its own rectangle would
   be a third answer that drifts. It also already carries the mirroring: a
   flipped frame is anchored from `width - center_x`, so the *texel* the cursor
   lands on has to be mirrored back before `opaque_at` is asked — the one piece
   of arithmetic here that the item pick does not have and cannot lend.
3. **A worn layer is a hit on its wearer.** Equipment draws with the wearer's
   `depth::Order` and no serial of its own; a click on a hat is a click on the
   head under it. So the pick tests the body frame *and* every layer
   `worn_graphic` resolves, and answers with the mobile either way.
4. **The topmost drawn wins, later-wins on a tie** — the largest `depth::Order`
   with `>=`, exactly `items::pick`'s tie-break, because it is exactly the same
   question about the same depth test. `Cutaway::shows_mobile` gates it with
   the same call `collect` makes: a body hidden under a roof this client is not
   drawing was not pointed at.
5. **The serial comes back by index**, `App::mobile_serials` beside
   `App::item_serials`, for the reason that one exists: the renderer is handed
   `Mobile`s with no identity in them, and putting the serial *into* the render
   struct would push a protocol type into the crate that draws.

Then three behaviours, in the order they are worth having:

- **Hover** — the outline, and the name. Both are local: the outline is the
  `outline.md` ring the items already get, and the name is what the view
  already knows, hung off `mobiles::head_anchor` rather than a fixed offset,
  because a rat and a dragon hold their heads at wildly different heights.
- **Double click → `0x06`** — built. `interact::use_object` writes it, the shard
  answers a mobile's use with a paperdoll (`0x88`), and the window that opens is
  M4's. Nothing on the way out says "paperdoll": it is the same packet an item
  gets, and which of the two it means is the shard's answer
  (`DoubleClick::interpret`). The serial comes from the pick — `App` keeps the
  `(Who, Mobile)` pairs rather than dropping the identity on the way into
  `mobiles::pick`, and a body with no serial (the offline viewer's placeholder)
  asks nothing.
- **Single click → `0x09`.** The shard has this end already —
  `Command::SingleClick` is dispatched and answered — so the whole cost on this
  side is the packet and the click. The answer is the name over the head, in
  the notoriety hue, which is the same overhead-message path the speech line
  already draws.

Done when: the cursor over any body in the crowd rings it and names it; a
double click on one sends a `0x06` naming its serial and a single click a
`0x09`; a click on a hat picks the wearer; a click on a creature standing
behind a wall picks nothing; and the pick and the draw cannot disagree, because
a test asserts that what `pick` answers for a cursor inside a frame is the
mobile `collect` drew there.

