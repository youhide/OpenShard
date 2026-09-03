# The role tables close, and every axe gets its trade back

> **This is a record.** It measures one pass over the identity catalogue and is
> kept as it was written. The model it works inside is
> [`../design_item_kind.md`](../design_item_kind.md) — where the two differ, the
> design is right — the staged account of the migration is
> [`2026-08-30-the-item-kind-migration.md`](2026-08-30-the-item-kind-migration.md),
> what is still open is ranked in [`../README.md`](../README.md), and what is not
> built is
> [`plans/items/item_identity/PLAN.md`](../../../plans/items/item_identity/PLAN.md).

## What moved

`state/data/items.json` holds **145** definitions rather than 120, and the
crafting data has **127** typed recipes of 599 rather than 102. The twenty-five
new rows are not a sample of the world; they are exactly the arts that the four
role tables — `weapon::WEAPONS`, `armor::ARMOR`, `craft::craft_tool` and
`harvest::tool_data` — could answer for and the registry could not name:

```
weapons   thin longsword · butcher knife · cleaver · skinning knife · hatchet
          club · quarter staff · black staff · gnarled staff · shepherd's crook
armour    leather shorts · skirt · bustier sleeves · the five bone pieces
          orc helm · wooden shield · wooden kite shield
tools     skillet · rolling pin · flour sifter · scribe's pen
```

Twenty-one armour definitions that already existed gained the flipped facing
their `armor.rs` row has always carried as a sibling — `0x1416` beside the plate
chest's `0x1415`, and twenty more like it. A flipped breastplate had been armour
on the legacy path and nothing at all on the semantic one.

## The finding: growing this catalogue is not additive

Every path that makes an item from a graphic runs
`spawn::install_legacy_identity`, which attaches the kind whenever
`kind_from_drawn` names the art. So **a definition added does not sit beside the
graphic tables; it moves live items off them.** A hatchet off a vendor's shelf
was an untyped `0x0F43` on Tuesday and carries `ItemKindId(125)` today, and it
reaches `tool_data_for_kind` where yesterday it reached `tool_data`. Any gap in
the kind-keyed twin of a table therefore becomes a live defect the moment the
definition lands — not later, and not only for crafted items.

That had already happened to the axes, and had been true for as long as the
blacksmith recipes have been typed. `tool_data_for_kind` had no arm for the
`weapon.is_axe` column that `tool_data` derives lumberjacking from, so every axe
that carried a kind answered "not a harvesting tool":

- `apply_core_defaults` gave it no `Tool`, so it had no swings in it;
- `equip`'s double-click saw no `Tool` tag and put it on the paperdoll instead
  of raising the chop cursor;
- `use_tool` and `begin_harvest` refused it outright.

Which axes carried a kind? All of them but one. Kinds 73–79 are the smith's
axe, battle axe, double axe, executioner's axe, large battle axe, two-handed axe
and war axe, off a shelf as readily as off an anvil. **Lumberjacking was a skill
only a hatchet could practise** — and the hatchet was the one axe art with no
definition, which is exactly why `harvest_tests.rs` chopping with a hatchet
stayed green through all of it. Registering the hatchet would have closed the
last door.

The fix is one arm, derived rather than listed: `tool_data_for_kind` asks
`weapon_data_for_kind(kind).is_axe`, the same question `tool_data` asks of a
graphic, and the seven axe definitions gain the `tool` tag that routes the
double-click. The pickaxe sits above that arm and stays Mining; it is an axe by
the weapon table's reckoning too.

## What now says the two halves agree

Four sweeps, each walking a whole table rather than sampling it, which only
became possible once the registry named every art in them:

| sweep | what it compares |
|---|---|
| `weapon::every_weapon_art_resolves_to_a_kind_that_fights_the_same` | class, axe flag and the pre-AoS damage block, by art against by kind |
| `armor::every_armour_art_resolves_to_a_kind_that_protects_the_same` | rating and meditation allowance |
| `craft::every_craft_tool_graphic_names_the_same_trade_by_kind` | 34 arts, whole `CraftToolData` |
| `harvest::every_harvest_tool_graphic_names_the_same_trade_by_kind` | 15 arts, the skill |

The two tool sweeps count what they checked, because their loop body is
conditional and a sweep that matched nothing would pass in silence. The
weapon and armour sweeps iterate a static table, where that cannot happen.

Beside them, `harvest_tests::give_tool` now installs the semantic identity a
bought tool has. That single line is what puts the existing pickaxe, fishing-pole
and hatchet cases on the kind-keyed path at last — they had been exercising the
legacy adapter and calling it coverage — and the new
`a_registered_axe_chops_the_way_the_unregistered_hatchet_did` holds the axe end
to end, cursor and swing. Both it and the harvest sweep fail with the derived
arm removed.

## How each new row's material family was decided

The rule used was ServUO's own: a class that carries a `CraftResource` gets a
family, and the family is the one its recipe's material axis actually
substitutes. Three rows are worth writing down because the honest answer is not
the obvious one.

- **The wooden kite shield is metal.** ServUO's `DefBlacksmithy` makes it out of
  ingots, on the ore axis, and its `BaseArmor.DefaultResource` is `Iron`. The
  wooden shield beside it is carpentry's, off the board axis, and is wood.
  Naming them after their own recipes rather than after the word "wooden" is
  what keeps a future `InheritInput` on either row from failing the build.
- **Bone armour and the orc helm are leather.** `BoneChest` and `OrcHelm` both
  override `DefaultResource` to `RegularLeather`, and their tailoring rows
  consume leather on the axis with bones as a fixed second line. `MaterialType`
  is `Bone`, but that is the meditation/absorb question, and this engine already
  answers it from `armor.rs`'s own column.
- **The rolling pin has no family, and its two neighbours do.** Tinkering makes
  the skillet and the flour sifter out of ingots on the axis; the rolling pin is
  made of boards on a line the metal axis does not touch, so nothing can give it
  a grade. It is `output_material: "none"`, the same shape the sewing kit has for
  the same kind of reason.

One row is a deliberate merge. `MapmakersPen` and `ScribesPen` are two ServUO
classes on one art, `0x0FBF`, told apart only by which craft system they open —
and this engine has no Cartography, so `craft_tool` has always answered
Inscription for both. Both tinkering rows are therefore typed as the one
registered pen rather than one of them being left untyped, which
`every_registered_craft_tool_recipe_has_a_typed_output` would have refused. Two
recipes naming one kind is not new: tinkering already has two rows for the
tinker's tools.

## What is still a pilot

The role tables are closed; the catalogue is not. 472 of 599 recipes still have
no `kind`, and they are not evenly spread — the untyped remainder is almost
entirely decor, containers, food, scrolls and expansion-era art:

```
alchemy      28 of 28 untyped     inscription  66 of 66
cooking      41 of 41             carpentry   128 of 144
tailoring   100 of 124            tinkering    64 of 94
blacksmithy  44 of 95             fletching     1 of 7
```

Nothing in that list is blocked on design. What each one wants is a definition
row and, for a dozen carpentry rows, the addon the deed installs — which is
[`../README.md`](../README.md) § what is open, row 3, and not this pass.
