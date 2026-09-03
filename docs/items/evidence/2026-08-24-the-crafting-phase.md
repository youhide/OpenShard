# Crafting

*The roadmap's own record of the crafting phase. A record, not a status: what is
built and what is open today is [`README.md`](../README.md).*

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
    to.
  - **Hides become leather under scissors, and keep their grade.** The chain the
    butcher's end was missing: `carve` paid a player in hides and 56 tailoring
    rows ate leather that nothing on the shard produced, so more than half the
    trade was unreachable. ServUO's `Hides.Scissor` — the whole pile, one
    leather per hide — is `items/src/cut.rs`, an item action rather than a craft
    (no skill, no roll, no workshop, and no wear, since ServUO charges a use for
    it only on Siege). Hides are a registered kind now (`0x1078`, the `leather`
    material family) so the grade is durable rather than a hue, and `carve`
    stamps ServUO's `HideType`: the alligator and the dire wolf give spined. The
    table is keyed by body and so cannot split two creatures that share one —
    the hell cat and the housecat are both `0xC9`, and that body stays regular.
    Horned and barbed have no carvable source yet; every creature that wears
    them upstream is a dragon or a serpent.
  - **Cotton becomes cloth on a spinning wheel and a loom.** The other half of
    the same gap: 56 tailoring rows and a dozen carpentry and smithing ones eat
    cloth (`0x1766`) that nothing on the shard made, so a tailor bought it. The
    chain is ServUO's, and it is two **addon interactions** rather than crafts —
    cotton or flax → wheel → six spools of thread; wool → wheel → three balls of
    dark yarn; five spools or balls → loom → a bolt of cloth; the bolt →
    scissors → fifty cloth, in `items/src/{spin,weave,cut}.rs`. The wheel turns
    for ServUO's six seconds, drawing its turning art and refusing a second pile
    meanwhile; a hue survives the whole length of the chain, so dyed cotton ends
    as dyed cloth. Both facings of the loom, the spinning wheel and the elven
    spinning wheel are Carpenter deeds that install through the machinery the
    ovens built (deed kinds 115-120), and the loom geometry is read from the
    generated `decoration::ADDON_COMPONENTS` rather than written twice. The
    loom's half-woven count is saved (**schema v37**) because those spools are
    already spent; the wheel's timer deliberately is not, and a restored wheel is
    stamped back to its resting art the way ServUO's `OnComponentLoaded` does.
    Cotton, flax and wool were still bought rather than farmed or sheared when
    this landed; the entry below is that world slice.
  - **Cotton grows in a field, and wool comes off a sheep.** The head of the same
    chain: two Felucca cotton fields (Moonglow and Skara Brae, eight plants and
    six, read off the `<spawning>` blocks of ServUO's `Regions.xml`) whose plants
    are double-clicked for a pile of cotton, and a blade on a live sheep for two
    wool and a shorn animal for the next two hours. A **crop field is a spawn
    region for items** — a box, a crop and a ceiling, maintained beside the
    creature regions with the same level-of-detail rule and the same seeded
    picks, laid full on registration the way ServUO's own region `Respawn` is.
    None of it is saved: a plant is world furniture the `populate:` verb re-lays
    on every boot, and a restored *picked* plant would be a bare furrow with no
    timer left to clear it, which is why a spell's field tile is out of the save
    too. The shear rides ServUO's `ICarvable` and so answers a **blade**, not the
    scissors that lore would suggest, and it had to sit ahead of the carve's
    reach check: that one asks where an *item* is, and a mobile has none, so it
    refuses every sheep on the shard. The fleece timer is transient like the
    spinning wheel's and needs the wheel's other half — a sheep saved shorn is
    stamped back into fleece on restore, or it stays shorn for ever. **Flax has
    no field**, upstream included: `FarmableFlax` exists as a class that nothing
    in `Regions.xml` plants, so it stays vendor stock rather than becoming a crop
    this engine invented a home for.
  - Found while building that chain, not fixed, each worth its own small slice:
    - **A dozen further addon deeds are craftable and inert.** Carpentry group 7
      carries generated rows for ServUO's dartboard, water trough, bulletin
      board and the rest, all on the same generic `0x14F0` scroll and none with
      a `kind` or an `addon` — so a carpenter spends the boards and gets a
      scroll that does nothing. Each wants an `AddonKind`, a deed kind, and
      whatever the installed thing *does*; the ovens, the loom and the wheel are
      three worked examples of the same five-line pattern. Only rows that carry
      an addon are gated today (`no_addon_deed_is_offered_twice`), because
      "outputs `0x14F0` implies typed" would be an assertion about content this
      engine has not reached rather than about a defect.
    - **Cloth does not become bandages.** ServUO's `Cloth.Scissor` and
      `UncutCloth.Scissor` are both `ScissorHelper(from, new Bandage(), 1)`, and
      `items/src/cut.rs` is already the seam. Bandages reach a player from a
      vendor or a corpse today, so this is a missing *route* rather than an
      unreachable item — which is why it was left out of the cloth chain rather
      than folded into it.
    - **`LightYarn` and `LightYarnUnraveled` have no producer**, upstream
      included: a wheel makes `DarkYarn` whichever wool went on. Both are vendor
      stock here and both weave, so nothing is broken; noted so a later pass does
      not read it as a gap this engine opened.
  - Found while giving that chain its head, not fixed:
    - **A carved sheep pays no wool.** ServUO's `BaseCreature.Wool` is 3 on a
      sheep in fleece and the corpse carve hands it over alongside the meat;
      `items/src/carve.rs`'s `Yield` has no wool axis at all, so a butchered
      sheep gives ribs and hides and nothing else. Now that shearing exists the
      gap is visible rather than theoretical — the same animal pays wool alive
      and none dead. One field on `Yield` and one row in `yield_of`, plus the
      question ServUO answers by body and this table cannot: the sheep shares
      `0xCF` with nothing, so unlike the hide grades it splits cleanly.
    - **Flax's second facing would not spin.** `Fibre::from_graphic` knows flax
      as `0x1A9C` alone, and ServUO's `FarmableFlax.GetCropObject` draws the
      picked pile's art at random between `0x1A9C` and `0x1A9D` — harmless while
      no field plants flax (nothing on the shard makes the second facing), and a
      pile a player could not spin the day one does. The fix is the alias the
      hides already carry (`0x1079`), and it belongs in the same commit as the
      field, not before it.
    - **`in_reach` refuses every living thing.** It resolves an *item's*
      location, and a mobile has none, so it answers "not in reach" for a sheep
      standing on the next tile. That is correct for what it asks and a trap for
      anything that starts targeting mobiles through the item helpers — the
      shear hit it, and `equip` measures a mobile by hand for the same reason.
      Worth a named `mobile_in_reach` beside it before a third caller finds out
      the hard way.
