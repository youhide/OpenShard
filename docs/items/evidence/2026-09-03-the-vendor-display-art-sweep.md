# The shop window and the thing in the bag

> **This is a record.** It says what landed on 2026-09-03 and why. The model as
> built is [`../design_item_kind.md`](../design_item_kind.md) — where the two
> differ, the design is right — and what is still open is ranked in
> [`../README.md`](../README.md), the only status page for this domain.

## What was wrong

A `GenericBuyInfo` in ServUO carries **two** item facts, and only one of them is
the item. The constructor is

```csharp
Add(new GenericBuyInfo(typeof(BlankScroll), 12, 40, 0xEF3, 0));
//                     ^ what you get                ^ what the window draws
```

and nothing makes the two agree. The converter that built `townsfolk.json` read
the fourth argument throughout, because for most rows it *is* the item's graphic
and there was no second column to read. Our shelf line has no second column
either: one `graphic` per line, and the graphic **is** the identity — so wherever
upstream's window borrowed a picture, this shard hands over the picture.

The inscription slice caught two such lines by accident: the mage and the
real-estate broker sold a blank scroll drawn `0x0E34` while `BlankScroll` is
`0x0EF3`, so the scrolls a scribe buys where scribes shop could not be written
on. It surfaced only because a *recipe* finally asked what the item was. Nobody
had swept the rest. This is that sweep.

## How it was measured

Four oracles, in order, because each one alone gives a wrong answer somewhere:

1. **The buy-info row** — all 1,082 `new GenericBuyInfo(…)` calls across the 95
   `SB*.cs` files, 501 distinct types, matched to our 26 shelves through each
   vendor mobile's `InitSBInfo`.
2. **The class constructor** — what `new X()` really draws. Necessary and *not*
   sufficient: `Food` is `base(amount, itemID)` and `Drums` is
   `base(itemID, startSound, stopSound)`, so a naive "last integer" read invents
   defects. Every candidate was re-read in the C#.
3. **`[FlipableAttribute]`** — the one that does most of the work. A weapon or a
   garment the client draws two ways is *one item*, and upstream's window is
   entitled to show either face. This is what separates a borrowed picture from a
   second facing, and it retired more than half the raw candidates.
4. **`tiledata.mul`'s own name for the tile** — the tiebreaker where upstream has
   no attribute to state its intent. It is how `0x15FE` was shown to be a bowl of
   *carrots*.

Sixty raw candidates came out of oracle 1+2. Oracles 3 and 4 cut them to
**fourteen** real ones, on 28 shelf lines. One of the sixty was a defect in the
measurement rather than the data — `AquariumFishNet` assigns `this.ItemID` in the
constructor body instead of passing it to `base`, so a reader that only follows
the initialiser list reports a mismatch that is not there.

## The fourteen

Every one is "the window's picture, not the item":

| shelves | line | ships | is really | should be |
|---|---|---|---|---|
| armorer, blacksmith | close helm | `0x1409` | a second helm tile | `0x1408` |
| armorer, blacksmith | helmet | `0x140B` | a second helm tile | `0x140A` |
| armorer, blacksmith | norse helm | `0x140F` | a second helm tile | `0x140E` |
| armorer, blacksmith | plate helm | `0x1419` | a second helm tile | `0x1412` |
| baker, cook | bread loaf | `0x103C` | the other loaf tile | `0x103B` |
| baker, cook | muffins | `0x09EA` | the stacking muffin tile | `0x09EB` |
| barkeeper, cook, waiter | pewter bowl of corn | `0x15FE` | **a bowl of carrots** | `0x15FF` |
| barkeeper, cook, waiter | pewter bowl of lettuce | `0x15FF` | **a bowl of corn** | `0x1600` |
| jeweler | sapphire | `0x0F19` | the other sapphire tile | `0x0F11` |
| provisioner, tanner | backpack | `0x09B2` | the other backpack tile | `0x0E75` |
| fisherman | vacation wafer | `0x0971` | the other stew tile | `0x0973` |
| blacksmith | malleable alloy | `0x1BE3` | **a copper ingot** | `0x1BE9` |
| weaver ×4 | uncut cloth | `0x1761`–`0x1764` | **folded cloth**, not an `UncutCloth` facing | `0x1767` |
| farmer | hoe | `0x0F39` | a shovel — *left alone, see below* | `0x0E86` |

The helms were the expensive ones. `armor_data` is a lookup by exact graphic and
knows `0x1408`, `0x140A`, `0x140E`, `0x1412` and nothing beside them, so **half
of every armorer's and every blacksmith's helm stock was a hat with no armour
rating**. Four arts, two shelves, since the beginning.

The pewter bowls are the funniest and the most clearly wrong: upstream's window
is off by one down the whole bowl run, so the shard sold a bowl of carrots
labelled corn and a bowl of corn labelled lettuce, and the real bowl of lettuce
(`0x1600`) was sold by nobody at all.

## Why four lines went away instead of changing

Rewriting `0x1409` to `0x1408` makes the armorer's shelf carry the close helm
twice, at one price, with one hue. That is not a listing this engine can hold.
`npc::vendor`'s `restock` finds the pile it is topping up by `(graphic, hue)` and
takes the **first** match, so the second such line is never reached: its amount
and price are dead text and the shelf holds one pile where the file says two.
The pair is the shelf's key whether or not anybody wrote it down.

So the four helm pairs, the two bread loaves and three of the four uncut cloths
were **dropped** rather than duplicated — thirteen lines. Where the pair
disagreed on price the surviving row is the one whose art upstream's own
constructor carries: the helmet stays at 31 and the baker's loaf at 6.

That property is now a build failure rather than a thing to remember.
`world/build.rs` rejects any shelf whose lines collide on `(graphic, hue)`,
naming both, beside the `amount > 0` check that was already there. The file had
no collision before this change and has none after; the gate exists because
ServUO's data *is* full of them and the next port of a shelf will bring one.

## The second family the sweep turned up

Once the shelf lines were right, the same question ran the other way: of the
lines whose ServUO type descends from `BaseWeapon` or `BaseArmor`, which ship an
art our own combat tables cannot read? Three, all weapons, all reaching a player
today:

- **`0x0F44`, the hatchet's other facing** — `[Flipable(0xF43, 0xF44)]`, and
  `SBWeaponSmith` stocks the second one. Upstream is right and we were wrong:
  one weapon, two arts, and `weapon.rs` answered for only one.
- **`0x0DF1`, the black staff's other facing** — `[Flipable(0xDF1, 0xDF0)]`,
  stocked by the blacksmith, the carpenter and the weaponsmith. The shelf ships
  the *first* graphic of the pair and the table held the second.
- **`0x13B8`, the thin longsword** — not a facing question at all. The
  blacksmith and the weaponsmith have always sold one and `weapon.rs` had no row
  for it in either facing, so it was a sword that swung for a fist's damage.
  Ported from `ThinLongsword : BaseSword` — old 35/5–33, AoS 30/15–16, ML 3.50.

The two facings became sibling rows, which is what `armor.rs` already does for
the bustier sleeves. Adding any of the three meant editing `weapon.rs` **and**
`protocol::items::is_classic_weapon`, held together by a test that walks all
65,536 graphics and demands the two agree — it caught the first row within a
minute of it landing, which is the test doing exactly its job.

## The one left alone

The farmer's **hoe** is upstream's own incoherence, not ours. ServUO's `Hoe` is
New Magincia plant equipment: a `BaseAxe` on `base(0xE86)` — the **pickaxe**
graphic — hued 2524, and `SBFarmer` draws it as `3897`, which `tiledata` calls a
**shovel**. Neither art is a hoe. This engine has no `Hoe` class and no plants to
use one on, and it reads `0x0F39` as a registered shovel (`items.json` id 17, a
mining tool with its uses rolled on stock), so the shipped line already hands
over a working tool. Rewriting it to `0x0E86` would trade a working shovel for a
working pickaxe and move nothing. Left verbatim, recorded here, for the same
reason `environment::is_mill` keeps its misprints: upstream parity is the point,
and a silent "fix" that improves neither side is drift.

## Coverage

- `a_flipped_weapon_fights_the_same_on_either_facing` — the hatchet and the
  black staff, compared field by field rather than against pinned numbers,
  because the failure being guarded is a row edited on one side only; pinning
  would let the pair drift together and still pass.
- `the_thin_longsword_the_smiths_sell_is_a_sword`.
- `the_shared_catalogue_filter_matches_the_gameplay_table` — already there, and
  it now covers three more arts across the crate boundary.
- `world/build.rs`'s collision gate, demonstrated by adding a colliding beeswax
  line and watching the build fail by name before it was taken back out.

## What is still open

- **The sweep is a moment, not a mechanism.** Nothing in the tree compares
  `townsfolk.json` against ServUO, because ServUO is not in the tree — the four
  oracles were run by hand against a checkout. A re-port against a newer upstream
  has to run them again; this document is the procedure.
- **Uncut cloth is inert whichever art it wears.** `cut.rs` cuts hides and bolts
  and nothing else, so the weaver's `0x1767` cloth has no use — the same missing
  route as [`README.md`](../README.md) row 9. The art is now right, so it will
  work the day the route exists.
- **The vacation wafer and the malleable alloy** are aquarium and Stygian Abyss
  content with no reader here. Their arts are now the items' own; nothing asks.
- **The second bustier facings carry no `item_kind`** (`armor.rs`, the
  `0x1C0B`/`0x1C0D` rows), so they are armour by graphic only and invisible to
  the kind-keyed path. Harmless while both rows exist; it is the same shape row 1
  of the README retires.
- **Only weapons and armour were checked the second way.** A shelf line whose art
  our engine cannot read is inert for food, containers and tools too — that sweep
  wants the identity catalogue of row 1 rather than another hand pass.
