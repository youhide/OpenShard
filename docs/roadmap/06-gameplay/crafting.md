# Crafting

[Gameplay index](README.md) · [Roadmap](../README.md) · [Backlog](../backlog/README.md)

- [x] `crafting` — **making things, and the 532 recipes to make.** The pillar the
  harvest slice existed for: mining paid a player in ore and nothing in the
  engine consumed a raw material. A port of ServUO's `Scripts/Services/Craft/` as
  a system in the usual shape — `fn(&mut WorldState)` over `state`, its own
  `ItemCrafted`, no peer calls — with seven trades wired: **Blacksmithy**,
  **Tailoring**, **Carpentry**, **Tinkering**, **Alchemy**, **Fletching** and
  **Cooking**.
  - **The recipes are core data**, like `magic::spells` and `state::weapon`: a
    bare shard has to be able to forge. `tools/gen-craft-tables` reads ServUO's
    own `Def*.cs` once, its output is committed under `crafting/src/defs/`, and
    those files are ordinary source from then on. The generator's hard half is
    that ServUO names a crafted item by its **C# type** and this engine needs a
    **graphic**, so it indexes every class under `Scripts/` and walks the
    inheritance chain to whichever constructor finally passes a literal id.
    **A type that will not resolve is dropped and printed**, never guessed — the
    `resolveBody` lesson. Of 624 recipes parsed, **485 ship**; the 139 dropped
    are counted in the run's own summary (86 recipe-scroll gated, 37 theme pack,
    7 custom-craft, 5 on the scales axis, 4 whose art will not resolve). A
    further 211 of ServUO's 835 sit behind `Core.SA`/`HS`/`TOL`/`EJ` guards the
    parser removes whole, because `[gameplay] expansion` tops out at ML.
  - **The material axis is a hue swap.** ServUO needs nine `IronIngot` subclasses
    because a C# item *is* its class; an item here is a graphic and a hue, so the
    nine rows of `AddSubRes` collapse to nine hues against one graphic — the same
    nine `state::harvest::ORES` already pays a miner in, asserted equal in a test
    so a hue can never mean valorite on the ground and copper at the forge. That
    made **`items::take_from_backpack` hue-aware** (`take_from_backpack_of_hue`):
    hue *is* identity for a material, and a hue-blind take quietly pays a
    valorite order in iron.
  - **The chance is ServUO's, and its three corners are each a place a plausible
    simplification is wrong.** `chance_at_min + (val - min)/(max - min) *
    (1 - chance_at_min)`, in per-mille. Failing the *band* and failing the *roll*
    are different refusals — one costs nothing and gets cliloc 1044153, the other
    costs the materials — and folding them together eats the ingots of every
    player who clicked a recipe they were not yet good enough for. The
    exceptional draw is **independent of the success draw and made first**, so
    what follows a craft does not depend on how the craft went and the tick still
    replays. And a chance can be *negative*, which is not clamped up: a recipe's
    `min_skill_offset` licenses the attempt, it does not discount the odds.
  - **Every gate is checked twice**, which is design and not redundancy: ServUO
    dry-runs the whole of `ConsumeRes` before starting its timer and again when
    it ends. A craft takes seconds, and in those seconds a player can step away
    from the forge, hand the ingots to a friend, or wear the tongs out.
  - **The workshop scan reads statics as well as items.** A forge is sometimes
    decoration the converter placed and sometimes a tile baked into the map, and
    Britannia has both kinds in the same buildings — `DefBlacksmithy` scans the
    two separately for exactly that reason. Reading only the entities refuses a
    craft at half the forges in the game, and the refusal reads as a broken
    recipe rather than a missing scan. ServUO's per-candidate line-of-sight ray
    is deliberately *not* copied; the ±16 z band already throws out the forge on
    the floor above.
  - **Smelting had to land with it**, or Blacksmithy is unreachable from Mining:
    a miner is paid in ore and every smith recipe eats ingots. ServUO's
    `BaseOre.OnDoubleClick`, with one deliberate difference — its target cursor
    exists to pick which forge and to combine piles, and neither applies here
    (one predicate answers "is there a forge", and identical piles merge on their
    own).
  - **The window** is `CraftGump`/`CraftGumpItem` through the typed `GumpLayout`,
    the path `MondainQuestGump` took, with ServUO's `1 + kind + index * 7` button
    encoding kept verbatim — the decode has to agree exactly and a scheme of
    one's own is a second thing to get wrong. The reply is matched against
    **what the server remembers drawing** (`open_craft_gumps` beside
    `open_quest_gumps`), which carries more weight here than it does for a quest
    log: the tool, the category and the chosen metal all live in the context and
    never in the packet. One layout detail is load-bearing and was got wrong
    first: the **categories are drawn on page zero**, which is what puts them on
    every page of a paginated list — inside the pagination the whole left column
    vanishes the moment a category runs past ten rows, which most of them do.
  - **The way in is the tool's double-click**, through the same `use_item_skill`
    seam the bandage, the lockpick and the pickaxe come through. There is no
    craft packet at all. The tool table is `state::craft`, in `state` for the
    reason `state::weapon` is — two crates read it: `items` to give a fresh
    sewing kit its uses, `crafting` to know which of the seven windows to open.
    The vendors already stocked all of it (26 tongs, 28 sewing kits, 15 saws, 41
    scribe's pens) and every one was an inert prop, exactly as the bandages,
    lutes and pickaxes were before their slices.
  - **Quality and the maker's mark persist (schema v21).** `Quality` and
    `CraftedBy` are components on the item, and both are **read at the read
    site** — `state::armor::piece_rating` adds ServUO's `-8 + 8 * quality` and a
    material bonus derived from the hue (valorite +16 over iron, barbed +16 over
    plain leather), so nothing is folded into the wearer and a fine breastplate
    coming off leaves nothing to undo. That material ladder is what makes the
    metal axis worth offering at all. The maker is a **name and not a serial**,
    for the reason a corpse's killer is one: the smith logs out and the sword
    outlives the session. Without the two columns every masterpiece on the shard
    quietly becomes ordinary at the next boot — the `Murders` bug, over property
    somebody spent an hour earning.
  - **Cooking is a complete first gameplay pass.** A skillet, rolling pin, or
    flour sifter opens its own menu; the 40 selected ServUO recipes keep their
    individual mill, fire, and oven requirements. Both stone-oven deeds are
    Carpenter recipes (85 boards, 125 iron ingots, Carpentry 68.4 and Tinkering
    50.0): double-click one and target a tile wholly inside a house to place its
    two locked-down sections. Both one-tile elven oven facings are Carpenter
    recipes too (80 boards, Carpentry 85.0, never exceptional) and follow the
    same house rules, including when one entered through content rather than
    crafting. Releasing any component of an installed addon — or the collapse of
    the house it stands in — takes the whole addon down and refunds its deed.
    Every addon deed's type survives a restart. Food is an ordinary crafted item
    in this pass; hunger and eating effects remain a separate gameplay decision.
  - Deferred, each its own system hanging off crafting: **Repair**, **Enhance**,
    **AlterItem**, **Resmelt** (item back to ingots; *ore* smelting is in),
    **recipe scrolls**, **make-number / make-max** and the **last-ten list**
    (per-player UI state ServUO serializes, so it wants a decision about saving
    UI). The four remaining tables — Inscription, Glassblowing, Masonry and
    Cartography — are data the generator can emit when
    they are wanted; Inscription waits on the writable book it is already tied
    to. And two material chains stay unbuilt rather than implied: **hides →
    leather** (scissors on a hide) and **cotton → thread → cloth** (a spinning
    wheel and a loom), both of which are addon interactions in ServUO and not
    crafts at all — until they exist a tailor buys cloth and leather from the
    vendors that already stock them.
