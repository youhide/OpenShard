# Crafting: how it works

A reader's map of `crates/server/crafting` and the seams it hangs off: where
every piece lives, what the data model is, what happens between a double-click
and a finished item, and what of it survives a restart. It is the model as
built, with no status in it — what is ready and what is open is
[`README.md`](README.md), and the dated records of how each slice landed are in
[`evidence/`](evidence/).

## 1. Where things live

| Piece | Where | Notes |
|---|---|---|
| Recipe tables (data) | `crafting/data/<trade>.json` | one file per trade, generated **once** from ServUO's `Def*.cs` by `tools/gen-craft-tables`, then edited as data |
| Trade headers (data) | `crafting/data/craft_systems.json` | skill, chance floor, exceptional curve, sound, workshop needs. **Array order is `SystemId`** — append, never reorder: the index rides in the `Crafting` component |
| Codegen + validation | `crafting/build.rs` | JSON → `const` tables in `OUT_DIR`; refuses a bad row at `cargo check` (group index, leading skill, selector shape, item-kind references, more resource lines than `MAX_CRAFT_RESOURCE_LINES`, an addon row typed as its own deed kind) |
| Types | `crafting/src/system.rs`, `recipe.rs` | `CraftSystemDef`, `Recipe`, `CraftRes`, `SubResAxis`, `Needs`, `Eca`, `Text` |
| Execution | `crafting/src/craft.rs` | `begin` → `advance_crafts` → `complete` |
| Odds | `crafting/src/chance.rs` | per-mille; band gate vs roll |
| Materials | `crafting/src/consume.rs` | dry-run `check`, then `prepare_withdrawal` / `commit` |
| Workshop scan | `crafting/src/environment.rs` | forge/anvil/heat/oven/mill/water, items **and** map statics |
| Window | `crafting/src/gump.rs` | ServUO's `CraftGump` through `GumpLayout`; context held server-side |
| Ore → ingots | `crafting/src/smelt.rs` | the one step between Mining and Blacksmithy |
| Hides → leather | `items/src/cut.rs` | the one step between butchering and Tailoring. In `items` rather than beside `smelt.rs`: it has no skill, no roll and no workshop, so it is `carve`'s shape (the module that makes the hides) and not a craft's |
| Bolt → cloth | `items/src/cut.rs` | the same scissors, one step further: fifty cloth per bolt, keeping the hue |
| Fibre → thread | `items/src/spin.rs` | the spinning wheel. An item action like the cut, with one addition nothing else in `items` has: a **timer**, ticked by `advance_spins` |
| Thread → bolt | `items/src/weave.rs` | the loom, five applications to a bolt. The count lives on the addon, not on the weaver |
| Field → cotton | `items/src/crop.rs` | the plant standing in a field and the double-click that picks it. A second timer in `items`, ticked by `advance_crops` |
| Sheep → wool | `items/src/shear.rs` | a blade on a live sheep. A branch of `carve`, because upstream reaches both through one target |
| The fields themselves | `world/src/crops.rs`, `world/src/tick/crops.rs`, `world/data/crops.json` | which patches of ground grow cotton, and the pass that keeps them planted — the spawn region's shape, for items |
| Reagents + mana → scroll | `crafting/data/inscription.json` | a scroll is an ordinary recipe with two extras: `mana`, and a gate derived from its own output art |
| The spell gate | `items/src/backpack.rs` (`carries_spell`) | a spellbook in your own pack with the bit set. Shared with casting, which asks the identical question |
| Scroll art ↔ spell | `state::components::scroll_spell` | the run `0x1F2D..`, whose **first circle is rotated** — not `base + spell` |
| Tool table | `state/src/craft.rs` | graphic → trade skill + uses; in `state` because `items` reads it too |
| In-flight state | `state::components::Crafting`, `Tool`, `Quality`, `CraftedBy` | components on the crafter / the item |
| Addon state | `state::components::AddonKind`, `AddonPart`, `AddonDeed`, `Spinning`, `LoomPhase`, `Fibre` | what a deed installs, which tiles are one addon, and what the wheel and the loom are in the middle of |
| Field state | `state::components::Crop`, `CropKind`, `Shorn` | whether a plant is standing or picked, what it grows, and when a sheep's fleece is back |
| World wiring | `world/src/tick/skills_wire.rs`, `tick.rs` | the double-click dispatch, and `advance_crafts` / `advance_spins` / `advance_crops` / `regrow_fleece` / `maintain_crops` once per tick |

Dependency direction is the usual one: `crafting` reads `state`, `items`,
`skills`; the world calls `crafting`; nothing calls back. The crate emits one
event, `ItemCrafted`, after the item is already in the pack.

## 2. The data model

A **system** is a trade: a main `Skill`, a `chance_at_min` (0 for a smith, 500
for a tailor), an `Eca` curve for exceptional odds, a beat delay (1.25 s
everywhere), a `Needs` set every recipe of the trade shares (only Blacksmithy:
forge + anvil), a list of `groups` (the gump's left column), a list of
`recipes`, and an optional `sub_res` **material axis**.

A **recipe** is: output (`graphic` + `hue`, or a typed `kind` +
`output_material` when the row has been migrated to the item-kind registry),
`group`, `skills` (first line is the system's own skill and is the one the odds
interpolate over; the rest are gates that also train), `resources` (at most
`MAX_CRAFT_RESOURCE_LINES` of them — the constant is the budget, and it is
written on both sides of codegen on purpose),
and flags: `use_all_res` (batch: make as many as the pack affords), `hue`,
`retain_color`, `min_skill_offset`, `markable`, `never_/always_exceptional`,
per-recipe `needs`, `min_chance` (this row's own odds floor, overriding the
system's — point 6 of
[`evidence/2026-09-02-the-crafting-review.md`](evidence/2026-09-02-the-crafting-review.md)),
`mana` (paid on top of the materials; zero on every row but inscription's
scrolls, and zero means "no requirement" rather than "free"),
and `addon: Option<AddonKind>` for the deeds that install a
house addon.

**One gate is derived rather than declared.** A row whose output art is a Magery
scroll may only be made by a crafter carrying a spellbook that holds that spell
— ServUO's `DefInscription.CanCraft`. The spell is read off the row's own output
(`state::components::scroll_spell`), so there is no column to disagree with it;
the same function is what a spellbook reads when a scroll is dropped on it, and
what casting checks. Its run is **not** `base + spell`: the first circle is
rotated, and the table says so in both directions.

The **material axis** is a hue swap, not a type swap: nine ingot hues against
one ingot graphic, seven woods, the leather grades. Exactly one resource line of
a recipe is marked `from_axis`, and the chosen `SubRes` substitutes its hue (and
its `MaterialId`, on the typed path) into that line. A recipe with no
`from_axis` line ignores the axis entirely — that is why a fletcher with oak
selected still makes plain arrows. `defs/mod.rs` tests pin that the axis hues
equal `harvest::ORES` hues and the registry's own presentation.

Two identity models coexist by design (see
[`design_item_kind.md`](design_item_kind.md)): a
**legacy** row names things by `Graphic + Hue`; a **typed** row names an
`ItemKindId` and resolves its material through `OutputMaterial`. `consume` and
`craft` handle both; the build script forbids a typed row from silently keeping
legacy material behaviour.

## 3. The runtime path

```
double-click tool ──► world::use_item_skill ──► state::craft::craft_tool(graphic)
        │                                        (or craft_tool_for_kind)
        ▼
crafting::open(CraftGumpContext{system, tool, group, sub_res, page})   [server remembers it]
        │  gump reply: button id = 1 + kind + index*7 (ServUO encoding, verbatim)
        ▼
gump::handle ──► make ──► craft::begin
        │   gates: not already Crafting · tool exists/has uses/is carried (no Position)
        │          · workshop (system.needs ∪ recipe.needs) · the spell, if the row
        │            writes one · mana, if the row costs any · skill band
        │          · materials dry-run
        ▼
insert Crafting{system, recipe, tool, sub_res, beats_left, next_beat}; first strike now
        │
        ▼ each tick: crafting::advance_crafts
   beat ──► strike (disrupt, break cover, sound)   … last beat ──► complete
        │
        ▼ complete: EVERY gate again → roll (exceptional draw first, then success)
           → fail: pay Share::All (or Half for use_all_res), train, wear tool
           → success: resolve output identity → prepare withdrawal plan
             → prepare placement in pack → commit both → pay mana
             → Quality/CraftedBy/AddonDeed/runebook charges
             → cliloc → bus.send(ItemCrafted) → wear tool
```

Points that are load-bearing rather than tidy:

- **Every gate runs twice** (begin and complete). A craft takes seconds; the
  ingots can leave the pack in between. This is ServUO's own shape.
- **Band failure ≠ roll failure.** Below `min - min_skill_offset` on any skill
  is a refusal (cliloc 1044153, no cost). A failed roll costs materials.
- **RNG is staged.** `complete` rolls on a *clone* of the world RNG and only
  adopts it once the craft is definitely happening (fail or success). A
  placement/capacity refusal therefore consumes no randomness, and the tick
  replays.
- **Payment is a plan, then a commit.** `prepare_withdrawal` reserves physical
  piles (sorted by serial, one row per pile, overlapping selectors share one
  remaining table) against a revision of the pack's craft-stock projection;
  `commit` asserts nothing moved and consumes. Bounds:
  ≤ `MAX_CRAFT_RESOURCE_LINES` lines, ≤`MAX_ITEMS`
  piles, batch ≤ `MAX_STACK`, source root ≤ `MAX_CRAFT_SOURCE_ITEMS`. Over any
  bound the answer is `Refusal::TooComplex`, before mutation.
- **Output is prepared before ingredients are spent.** A full pack refuses the
  craft without charging; a typed row whose `presentation_of(kind, material)`
  is missing is "not configured correctly", not a loss.
- **The gump trains nothing.** `chance()` (read-only, drawn on the detail page)
  is separate from `roll()`. Training happens only in `complete`, once per
  attempt, or once per item for `use_all_res`.
- **Workshop scan reads items and statics** in a Chebyshev box of 2 with a ±16
  z band; line-of-sight is deliberately not copied.
- **Tool wear** is one use per attempt, success or failure; at zero the `Tool`
  component goes and `items::consume(serial, 0)` removes the item.
- **Mana is a cost of the finished item**, not of the attempt: checked with every
  other gate at both ends, spent beside the materials once the craft has
  succeeded. A ruined scroll costs its reagents and no mana, and a refusal costs
  neither — ServUO consumes it in `CompleteCraft` and nowhere else.
- **A scroll says its own lines.** Success is cliloc 501629 and failure 501630,
  whatever the quality, because ServUO's ending effect branches on the type
  before it reads the quality. It is also why no scroll row is markable.

## 4. Persistence

Saved on the item: `Tool` uses, `Quality`, `CraftedBy` (a name, not a serial),
`Name`, `LockedDown`, `AddonPart` (which installed addon a locked-down component
is a tile of), `LoomPhase` (schema v37). **Not saved:** the in-flight `Crafting`
component, the per-player `craft_gump` context, a spinning wheel's `Spinning`, a
field's `Crop` (the plant *and* the stub, and the field itself), and a sheep's
`Shorn`. The first two are benign losses (nothing is consumed before
`complete`), but a restart mid-craft ends the craft silently. The last three are
the crop field's and the sheep's
([`evidence/2026-09-03-the-chains-head.md`](evidence/2026-09-03-the-chains-head.md)),
with the same second half the wheel needs: what the save *does* record — the
turning art, the shorn body — is stamped back on restore.

The wheel and the loom split on exactly that question, and the split is the
rule rather than an accident of which was easier:

- **`Spinning` is not saved** because it is six seconds long and ServUO does not
  serialize its own `SpinTimer` either. The cost is the pile of cotton, which
  `BeginSpin` has already eaten. What *is* handled is the second half of that
  loss: the save recorded the **turning** art, so `persist.rs` stamps the
  resting one back on restore — ServUO's own `OnComponentLoaded`. Without it a
  restored wheel turns forever with no timer left to stop it.
- **`LoomPhase` is saved** because a part-loaded loom has already eaten up to
  four spools. Forgetting the count charges the weaver for them twice, which is
  not a benign loss at all. An empty loom carries no phase rather than a zero
  one, so the column is NULL for every item on the shard that is not a loom
  mid-weave.

Both are dropped by `set_item_lockdown(item, None)` alongside `AddonPart`: a
component swept into a collapsed house's crate is an ordinary item again, and
the half-woven bolt goes with it — the same bargain the addon itself makes,
which refunds the deed and not the boards.
