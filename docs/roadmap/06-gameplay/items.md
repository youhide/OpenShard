# Items

[Gameplay index](README.md) · [Roadmap](../README.md) · [Backlog](../../../plans/roadmap/PLAN.md)

- [x] `items` — containers, stacking, equipment layers, decay
  - [x] **On the ground and visible.** A script drops an item
    (`op_spawn_item` → `Command::SpawnItem`) and every client in range is sent
    the `0x1A` that draws it; walking up to one draws it, walking away sends the
    `0x1D`, exactly as for a mobile. Items are entities like anything else — a
    `Graphic` and a `Position`, drawn through the same `seen`/interest machinery
    as bodies. A stack carries an `Amount`. The `WorldItem` (`0x1A`) encoder is
    ported from Sphere's `PacketItemWorld`, flag bits and all.
  - [x] **Pick up and drop** (`0x07`/`0x08`). The client's own item loop: lift
    an item onto the cursor and set it back on the ground. The world holds it in
    limbo — off the sector grid, off every screen but the picker's — and
    remembers where it came from, so a drop out of reach or a logout mid-drag
    bounces it back rather than losing it. A refused lift or drop is a `0x27`
    drag-cancel with a reason. Server-authoritative reach (`ITEM_REACH`), no
    trust in the client's claim. Ground-to-ground only; dropping *into* a
    container is the next slice, and it bounces for now.
  - [x] **Containers** (`0x06` open, `0x24`/`0x3C`/`0x25`). A container is an
    item that also carries a `Container` (its gump); items inside carry a
    `Contained` and no `Position` — the two are exclusive, on the ground *or* in
    a container, never both. Double-click opens it (`0x24` + the `0x3C` contents
    list); dropping onto its serial puts the item inside (`Contained` + a `0x25`
    to the open gump); lifting a contained item drops the containment. A drop
    onto a non-container, or out of reach, bounces to origin — and origin is now
    "the ground *or* the container it was in", so a cancelled drag always undoes
    cleanly. Live updates go to the acting client only; a second viewer re-opens
    to refresh (a noted limitation, not a bug). The `0x24`/`0x25`/`0x3C` version
    seams (High Seas type word, `ItemGrid` grid byte) are gated on `Feature`, not
    era.
  - [x] **Equipment layers** (`0x13` wear, `0x2E` equipped). A worn item carries
    an `Equipped { mobile, layer }` and no `Position`/`Contained` — the third and
    last place an item can be, all three exclusive. Dragging an item onto a
    paperdoll (`0x13`) wears it: the layer is checked free, the wearer reachable,
    and a `0x2E` goes to everyone who can see the mobile. A newcomer sees a
    dressed mobile because the `0x78` now lists what it wears (it sent an empty
    list before). Lifting a worn item takes it off. A held item's origin is now
    "ground, container, *or* mobile", so every cancelled drag still undoes to
    exactly where it came from.
  - [x] **Stacking, split and decay.** A `Stackable` item merges with an
    identical pile (same graphic and hue) dropped onto it — amounts sum, clamped,
    the dragged one despawns, the survivor is redrawn past the `seen` set.
    Picking up part of a pile splits it: the `0x07` amount is honoured, and —
    read out of Sphere's `CItem::UnStackSplit` rather than guessed — the original
    keeps its serial and holds the taken amount on the cursor while a new dupe is
    left on the ground with the remainder, so the client's cursor and its drop
    still name the same object. Ground items carry a `Decays { at_tick }` and rot
    when the tick counter reaches it; lifting, containing or wearing takes the
    clock off, and `decay()` reads only its own counter, no wall clock.
    Containers do not decay with their contents inside.
  - [x] **Stack merge inside a container.** `merge_onto` (`items/stack.rs`) no
    longer bounces on a target with no `Position`: it branches on where the target
    lives, and a `Contained` target is reach-checked through its container
    (`container_in_reach`, the same gate `drop_into_container` uses), the amounts
    summed as on the ground path, and every open gump told the new total with a
    `0x25` (`tell_watchers_updated`, mirroring `give`). The drop already routed
    here — `drop_onto_item`'s `can_stack` arm fires regardless of location.
  - [x] **A pile has a ceiling, and nothing falls off it.** Both merge paths used
    a `saturating_add` on the `u16` an `Amount` is stored in, so dropping 50,000
    gold onto 50,000 left one pile of 65,535 and destroyed the other 34,465 — the
    engine's first item-loss bug, found in play. The cap is now an explicit
    `items::MAX_STACK` (60,000, ServUO's `Item.WillStack` number, kept clear of the
    `u16` edge) and the overflow goes back to the player, not to nowhere: Sphere's
    `CItem::Stack` fills the destination to its maximum and leaves the remainder on
    the source, which is the kinder of the two references (ServUO refuses the merge
    outright). A drag whose remainder will not fit bounces it home. Where the
    *world* hands goods over, `items::give` spreads a payout across as many piles
    as it needs — a container ends up with two gold piles, as in UO — and takes a
    `u32` now, because a large sale earns more than one pile holds and the old
    `u16` made the vendor clamp the payout to 65,535 and say nothing.
  - [x] **Partial lift honours the amount everywhere.** `pick_up`'s container
    branch (`items/drag.rs`) reads the `0x07` amount now: a partial lift of a
    `Stackable` contained pile leaves the remainder behind *in the same grid slot*
    as a new dupe (`items::spawn_contained_leftover`, the container sibling of
    `spawn_leftover`) and lifts the original reduced to what was taken — Sphere's
    `UnStackSplit`, the original keeping its serial for the cursor, the remainder a
    new serial drawn into the open gump. A whole lift is unchanged.
  - [x] **The item-trigger seam (Sphere's `@DClick`).** The engine handles the
    double-clicks it knows — door, container, spellbook, mount, mobile — and hands
    every *other* item to the pack as an `ItemUsed { item, graphic, by }` event
    (defined in `items`, re-exported by `world`, delivered to scripts like every
    domain event), with reach already checked server-side (`container_in_reach`).
    The engine keeps *no* default behaviour for a bare item: the meaning lives in
    the pack, which registers a handler per graphic and answers with ops. This is
    the "default in core, customise in the pack" split — except the core default
    here is nothing, because a graphic has no behaviour until a shard gives it one.
    The Community Pack ships a readable book as the example.
  - [x] **A consume op for one-shot items.** `op_consume_item` (→
    `Command::ConsumeItem` → `items::consume`) removes an item wherever it lives,
    behind one op with the three location-specific client updates: on the ground
    the decay path (off the sector grid, a `0x1D` to every screen, shared with
    `decay` through `remove_ground_item`); in a container the reagent-burn path (a
    `0x1D` to whoever has the gump open, `tell_watchers_removed`); worn it forgets
    the item on the wearer *and* every onlooker (`broadcast_unequip`, the mirror of
    `broadcast_equip` — no "remove from paperdoll" packet exists, so the client
    drops it by serial, and unlike a lift the wearer's own client is told too).
    `amount` 0 removes the whole item; a smaller amount decrements a stackable pile
    (one potion out of a lot) via `remove_from_stack`. Consuming a container
    cascades into its contents (`despawn_contents`, shared with decay), and a stray
    serial removes nothing — the `add_loot` guard. The Community Pack's `items.js`
    ships a heal potion: `op_heal` the drinker, then `op_consume_item(e.item, 1)`.
