# Crafting: how it works

A reader's map of `crates/server/crafting` and the seams it hangs off, plus a
review of the design as it stands (2026-09-02, with the Cooking, oven-deed and
cloth-chain slices in). The roadmap entry
([roadmap/06-gameplay/crafting.md](roadmap/06-gameplay/crafting.md)) says *what*
landed and why; this page says *how the pieces fit* so a later session does not
have to re-read six files to find out.

## 1. Where things live

| Piece | Where | Notes |
|---|---|---|
| Recipe tables (data) | `crafting/data/<trade>.json` | one file per trade, generated **once** from ServUO's `Def*.cs` by `tools/gen-craft-tables`, then edited as data |
| Trade headers (data) | `crafting/data/craft_systems.json` | skill, chance floor, exceptional curve, sound, workshop needs. **Array order is `SystemId`** — append, never reorder: the index rides in the `Crafting` component |
| Codegen + validation | `crafting/build.rs` | JSON → `const` tables in `OUT_DIR`; refuses a bad row at `cargo check` (group index, leading skill, selector shape, item-kind references, ≤4 resource lines, an addon row typed as its own deed kind) |
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
| Tool table | `state/src/craft.rs` | graphic → trade skill + uses; in `state` because `items` reads it too |
| In-flight state | `state::components::Crafting`, `Tool`, `Quality`, `CraftedBy` | components on the crafter / the item |
| Addon state | `state::components::AddonKind`, `AddonPart`, `AddonDeed`, `Spinning`, `LoomPhase`, `Fibre` | what a deed installs, which tiles are one addon, and what the wheel and the loom are in the middle of |
| World wiring | `world/src/tick/skills_wire.rs`, `tick.rs` | the double-click dispatch, and `advance_crafts` / `advance_spins` once per tick |

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
interpolate over; the rest are gates that also train), `resources` (≤4 lines),
and flags: `use_all_res` (batch: make as many as the pack affords), `hue`,
`retain_color`, `min_skill_offset`, `markable`, `never_/always_exceptional`,
per-recipe `needs`, `min_chance` (this row's own odds floor, overriding the
system's — see #6), and `addon: Option<AddonKind>` for the deeds that install a
house addon.

The **material axis** is a hue swap, not a type swap: nine ingot hues against
one ingot graphic, seven woods, the leather grades. Exactly one resource line of
a recipe is marked `from_axis`, and the chosen `SubRes` substitutes its hue (and
its `MaterialId`, on the typed path) into that line. A recipe with no
`from_axis` line ignores the axis entirely — that is why a fletcher with oak
selected still makes plain arrows. `defs/mod.rs` tests pin that the axis hues
equal `harvest::ORES` hues and the registry's own presentation.

Two identity models coexist by design (see [item_kind.md](item_kind.md)): a
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
        │          · workshop (system.needs ∪ recipe.needs) · skill band · materials dry-run
        ▼
insert Crafting{system, recipe, tool, sub_res, beats_left, next_beat}; first strike now
        │
        ▼ each tick: crafting::advance_crafts
   beat ──► strike (disrupt, break cover, sound)   … last beat ──► complete
        │
        ▼ complete: EVERY gate again → roll (exceptional draw first, then success)
           → fail: pay Share::All (or Half for use_all_res), train, wear tool
           → success: resolve output identity → prepare withdrawal plan
             → prepare placement in pack → commit both → Quality/CraftedBy/AddonDeed
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
  `commit` asserts nothing moved and consumes. Bounds: ≤4 lines, ≤`MAX_ITEMS`
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

## 4. Persistence

Saved on the item: `Tool` uses, `Quality`, `CraftedBy` (a name, not a serial),
`Name`, `LockedDown`, `AddonPart` (which installed addon a locked-down component
is a tile of), `LoomPhase` (schema v37). **Not saved:** the in-flight `Crafting`
component, the per-player `craft_gump` context, and a spinning wheel's
`Spinning`. The first two are benign losses (nothing is consumed before
`complete`), but a restart mid-craft ends the craft silently.

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

## 5. The Cooking slice and addon deeds (2026-09-02)

- Cooking is the seventh `SystemRow`: skill `Cooking`, floor 0, sliding ECA,
  no sound (ServUO's `PlayCraftEffect` is empty too), no system-level needs;
  recipes carry their own `mill` / `heat` / `oven`. Tools: skillet, rolling
  pin, flour sifter (`state::craft`). 41 rows against DefCooking's 88: 40
  generated, plus the hand-written `Dough` row of #1 below.
- **Oven deeds.** Four carpentry rows share the generic scroll art `0x14F0` and
  carry `addon: stone_oven_east|south` / `elven_oven_east|south`. A deed's
  identity is the row's own `kind` (110–113, the `shared_art` definitions in
  `state/data/items.json`), so it rides the ordinary typed-item path through
  creation, save and restore. `complete` additionally stamps
  `AddonDeed { addon }` — a cache of `AddonKind::from_deed_kind` — and a `Name`,
  which is cosmetic only. Restore (`persist.rs`) and the double-click dispatch
  (`skills_wire.rs`) both re-derive the addon from `ItemKind`; see #2 for why
  the display string is no longer the durable form.
- Double-click → `offer_addon_placement` raises a location cursor
  (`TargetPurpose::PlaceAddon`); the answer → `place_addon_from_deed`: deed
  still carried by the actor, tile inside a house (`house_at`), storage
  allowance has room, then every component's absolute tile resolved and checked
  — inside the *same* house and free (`addon_tile_is_free`, see #3) — before
  anything is spawned, then spawn each component as a ground item and
  `housing::storage::lock_down` it; roll back on any failure; consume the deed.
  The stone ovens' geometry is read from the generated
  `decoration::ADDON_COMPONENTS` (see #5); the two elven facings are inline in
  `houses.rs` (`0x2DDB` east, `0x2DDC` south), since no elven oven is pre-placed
  and `deco_addons.json` therefore has no row for either. §6's four spinning
  wheels are inline for the same reason, and its two looms are not — a loom *is*
  pre-placed on this facet, so the generated table already carries it.
- Placed components are ordinary locked-down items whose graphics fall in
  `environment::is_oven`/`is_heat`, so a cook standing by one passes the
  workshop scan. They also carry `AddonPart`, which is what makes a stone
  oven's two tiles one thing to release — see point 3 of the review below.

## 6. The cloth chain (2026-09-02)

Cloth (`0x1766`) is eaten by fifty-six tailoring rows, ten carpentry ones and
one smithing one, and until this slice **nothing on the shard made any** — the
leather gap of #11 again, one material over. ServUO's answer is not a craft at
all: it is two house addons a player uses items *on*, and a pair of scissors at
the end.

```
cotton (0xDF9) ─┐
flax  (0x1A9C) ─┴─► [spinning wheel, 6 s] ─► 6 × spool of thread (0xFA0)
wool   (0xDF8) ────► [spinning wheel, 6 s] ─► 3 × dark yarn (0xE1D)
tainted wool (0x101F) ► [    ″    ] ─► 1 × dark yarn

thread or yarn ×5 ─► [loom: 4 load, the 5th weaves] ─► bolt of cloth (0xF95)
bolt ─► [scissors] ─► 50 × cloth (0x1766)          ← what a tailor spends
```

- **Six new addons, no new machinery.** `LoomEast/South`,
  `SpinningWheelEast/South` and `ElvenSpinningWheelEast/South` are
  [`AddonKind`] variants with deed kinds 115-120, and they install through the
  path #3 and #5 built for the ovens: registered typed deeds on the shared
  scroll art, `AddonPart` grouping, whole-addon release, refund on collapse.
  The four carpentry rows that make them **already existed** in
  `carpentry.json`, generated from `DefCarpentry.cs` and crafting an inert
  `0x14F0` scroll because they carried no `kind` and no `addon`; giving them
  both is the whole of the data change. The loom's two-tile geometry comes from
  the generated `decoration::ADDON_COMPONENTS`, like the stone oven's; a wheel
  is one tile and reads its own resting art from `wheel_arts` rather than
  restating it (#5's defect, refused a second time).
- **The wheel is the first timed thing in `items`.** `Spinning` sits on the
  addon's root component; `advance_spins` runs beside `advance_crafts` in the
  tick. Every tile of the wheel redraws to its turning art and back — the
  `set_door` redraw, forget-then-reveal — and a turning wheel refuses a second
  pile (cliloc 502656). The art pairs are ServUO's own and **do not all move the
  same way**: the classic wheel counts up off its resting graphic and both elven
  ones count down, so a single "+1 while busy" rule would have drawn the elven
  wheels as different furniture entirely.
- **Hue is the through-line.** Cotton carries a dye, the thread keeps it, the
  bolt takes it from the *fifth* material rather than the four already loaded
  (ServUO's own choice), and the cut keeps it again. Nothing in the chain is a
  registered kind with a material family: a dye is not a grade, so these stay
  legacy art plus hue, which is also what the vendors already stock them as.
- **Where things land.** The wheel's yield goes to the spinner's pack, the
  loom's bolt to the weaver's pack, and the cut's cloth to whatever container
  the bolt was in — `ScissorHelper`'s parent rule, the same one the hides
  follow. A spinner who logged out inside the six seconds gets the thread on the
  wheel's own tile rather than nowhere: the fibre is already spent, and this
  engine's logged-out character has no pack to reach.
- **Still bought, not grown.** Cotton, flax and wool reach a player from a
  vendor's shelf. `FarmableCotton`, `FarmableFlax` and shearing a sheep are a
  world slice of their own, and none of them is what made cloth unreachable.

## 7. Review: problems found

Ordered by how much they mattered when found, except #11 and #12, which came out
of later passes and are appended rather than slotted in by weight. File
references are to the working tree at the time. Every item is closed
(2026-09-02); the fix is noted under each. 9 needed no action, and 8 was closed
the low-risk way — verbatim upstream values with a comment rather than a quiet
edit.

1. ~~**Most of the Cooking table is unreachable: there is no dough.**~~
   **Fixed.** Hand-wrote the `Dough` row in `cooking.json` (output `0x103D`,
   name 1024157): main resource `SackFlourOpen` (`0x103A`, the resolvable half
   of ServUO's `AddCraft`), second resource a filled water pitcher (`0x1F9D`,
   ServUO's `Pitcher.ComputeItemID` default for `BeverageType.Water` —
   `BaseBeverage` itself still cannot resolve to a graphic, so the concrete
   container is named directly). The fifteen rows that already consumed
   `0x103D` are reachable now.

2. ~~**An addon deed's identity is an English display string.**~~ **Fixed.**
   Every deed is now a registered, typed item: `state/data/items.json` ids
   110–112 (`stone oven east/south deed`, `elven oven deed`) — 113 joined them
   when #4 split the elven facing in two — all sharing the generic scroll art
   `0x14F0` under a new `shared_art` flag on `ItemDefinition`
   (`item_definition.rs`) — a graphic the registry deliberately will not
   reverse-resolve through `kind_from_drawn`, since several kinds legitimately
   share it. `AddonKind::deed_kind` /
   `from_deed_kind` (`state/components.rs`) hold the id mapping;
   `AddonDeed::from_item_kind` replaces `from_saved_name`. The two carpentry
   rows are now typed recipes (`"kind": 110/111, "output_material": "none"`),
   so `ItemKind` installs automatically through the ordinary typed-placement
   path and rides the existing generic item-record persistence — no bespoke
   save code. `persist.rs`, `skills_wire.rs`'s dispatch and
   `houses.rs::offer_addon_placement` all derive the addon from `ItemKind` now;
   `Name` is cosmetic only (`AddonKind::label`). `HouseDeed` still has the old
   problem and is unfixed — out of scope here.

3. ~~**Placement checks the house, not the tile.**~~ **Footprint half fixed.**
   `place_addon_from_deed` resolves and validates every component's absolute
   tile — including `dz`, which used to be read from `deco_addons.json` and
   then silently dropped in favour of the cursor's own `z` — before spawning
   anything. Each tile is checked with `World::addon_tile_is_free`: nothing
   solid already there (`openshard_movement::can_fit`, the same wall/door/floor
   question `housing::place` asks for a whole building's footprint, against the
   component's own tiledata height) and no other locked-down item already
   sitting on the same spot — asked directly against the house's storage list,
   because an ordinary locked-down item never registers itself in the facet's
   obstruction index the way a wall or a door does, so `can_fit` alone cannot
   see a second oven stacking on the first. Regression coverage:
   `a_second_stone_oven_east_refuses_to_stack_on_the_first` (verified to fail
   without the storage-list check); the two existing placement tests' cottage
   fixtures were widened from a single blocking wall tile at the placement
   target to a wall elsewhere plus real floor under the oven, since a real
   collision check correctly refuses what they used to place unchecked.
   **Grouping half fixed too** (2026-09-02, second pass). The parts are one
   addon now: `state::components::AddonPart { addon, root }` on every
   component, `root` being the first component's serial, which the root itself
   carries — so one component answers both "what am I part of" and "what else
   is". `place_addon_from_deed` stamps it; the release path in
   `world::tick::houses` intercepts a `HouseStorage::Release` aimed at any
   component and takes the **whole** addon down, handing one deed back to the
   actor's pack. The economy question is answered ServUO's way (`BaseAddon`
   deletes itself whole and re-deeds): a full refund of the deed, not of the
   ingots that made it. Order is load-bearing and commented as such — permission
   first (`housing::storage::may_change`, the gate `lock_down`/`release` open
   with, named so it can be asked *before* acting), then the deed into the pack,
   then the parts stop existing; a full pack keeps the oven rather than losing
   both.
   The grouping is durable: `ItemRecord.addon` (`AddonPartData`) and schema v36's
   `addon_kind`/`addon_root`, the addon named by its deed's `ItemKindId` so there
   is no second numbering to keep in step. It is deliberately dropped whenever a
   component goes loose — `WorldState::set_item_lockdown(item, None)` removes it
   — so a component swept into a collapsed house's moving crate comes back an
   ordinary item rather than half of an addon whose root no longer exists.
   Coverage:
   `releasing_one_stone_oven_component_takes_the_whole_oven_and_returns_its_deed`
   (aimed at the *non-root* component on purpose; verified to fail without the
   interception), `an_installed_oven_keeps_its_grouping_across_a_save_and_restore`
   (verified to fail with the save half dropped),
   `an_addon_component_keeps_its_group_across_a_reopen` and
   `version_35_gains_the_addon_grouping_columns_without_losing_the_database` in
   the SQLite store.
   **Demolition half fixed too** (2026-09-02, third pass). `decay::demolish`
   groups the pinned items by `AddonPart.root`, and `pack_into_a_crate` *makes*
   one deed per group in the crate rather than packing the component tiles,
   which then stop existing. The seam question — that crate cannot create a
   typed item, because it deliberately does not depend on `openshard-items`
   (`take_off_the_ground`'s own comment says why) — was answered by moving the
   identity installer down instead of adding the dependency:
   `state::item_identity::install_item_identity` is now the one door through
   which `Drawn` is derived from an `ItemKindId`, and `items::install_identity`
   delegates to it. Order is all-or-nothing: the refund entities are reserved
   *before* the crate exists, so a shard out of item serials refuses the whole
   packing, and the caller removes an addon's tiles only when a crate came back.
   Coverage: `a_collapsed_house_packs_an_installed_oven_as_one_deed` (verified
   to fail with the grouping disabled — the crate holds two oven items instead
   of one deed).

4. ~~**`AddonKind::ElvenOven` is an orphan.**~~ **Fixed.** The missing
   player-facing source turned out not to be a content decision at all: ServUO
   crafts both facings in `DefCarpentry.cs:878-882` (Mondain's Legacy, group
   1044298, Carpentry 85→110, 80 boards, `ForceNonExceptional`), so the fix is
   the same parity every other row here is. The collapsed facing went with it —
   `AddonKind::ElvenOven` is now `ElvenOvenEast` (`0x2DDB`, deed kind 113) and
   `ElvenOvenSouth` (`0x2DDC`, deed kind 112, the id the single kind already
   had). Both carry the `never_exceptional` flag their upstream rows set. The
   elven geometry stays inline in `houses.rs` — one tile at the origin either
   way — because no elven oven is pre-placed on this facet, so `deco_addons.json`
   has no row for #5's generated table to read. Coverage:
   `an_elven_oven_deed_places_the_single_component_its_facing_names` places both
   facings and pins that each draws its own graphic, and
   `an_addon_recipe_outputs_its_own_addon_s_deed` pins that every addon recipe's
   `kind` is that addon's own `deed_kind`. The two halves are hand-written data
   in different files, and the agreement is asserted on both sides of codegen:
   `build.rs` refuses the mismatched row at `cargo check`, and the test asks the
   same question of the generated tables.

5. ~~**The oven component layout is written twice.**~~ **Fixed.** World's
   `build.rs` now also emits `decoration::ADDON_COMPONENTS` (public, keyed by
   ServUO class name) straight from `data/deco_addons.json` — the same source
   the flattened world statics come from — and `decoration::addon_components`
   looks it up. `place_addon_from_deed` reads the stone-oven geometry through
   that instead of its own copy; `ElvenOven`'s single tile stays inline since
   `deco_addons.json` has no row for it at all (see #4). Regression coverage
   for the crafted placement path itself was missing entirely (only the
   flattened-decoration shape and an admin-created elven oven were tested) —
   added `a_crafted_stone_oven_east_deed_places_both_locked_down_components`,
   which now also stands as the dedup's own regression guard. The "still
   carried" duplication is fixed too: both deed placements now call one
   `deed_still_carried` helper.

6. ~~**Cooking loses ServUO's per-recipe chance floor.**~~ **Fixed.** Added
   `Recipe::min_chance: Option<u32>` (ServUO's `GetChanceAtMin(CraftItem)`
   special-casing a recipe rather than answering a system constant), read in
   `chance::chance` as `recipe.min_chance.unwrap_or(system.chance_at_min)`.
   `GrapesOfWrath` (`0x2FD7`) and `EnchantedApple` (`0x2FD8`) now carry
   `"min_chance": 500`.

7. ~~**`consume::take` has no callers.**~~ **Fixed.** Removed; `craft.rs` was
   already using `prepare_withdrawal` + `commit` directly.

8. **Faithfully copied typos.** `environment::is_mill` lists `0x1295` and
   `0x129F`, which are ServUO's own (`CraftItem.cs:344`) and are almost
   certainly `0x1925`/`0x192F` misprints. Harmless for gameplay (the other
   fourteen ids cover the mill). **Addressed the low-risk way**: left the
   values verbatim (upstream parity is the point of this port) and added the
   comment warning against a silent "fix" into parity drift, per the doc's own
   first option — a real divergence still belongs in `docs/findings.md`, not a
   quiet edit, and this session had no stronger evidence than the same
   suspicion already written here.

9. **Two hand-kept tool tables.** `craft_tool` (by graphic) and
   `craft_tool_for_kind` (by kind) in `state/src/craft.rs` must agree; Cooking
   was added to the first only, which is correct today because no cooking tool
   has a kind yet, and the `defs` test would catch the day one does. Noted so
   the next trade does not forget the second half. No action needed.

10. ~~**The Cooking slice pushed the catalogue packet over its size
    assertion.**~~ **Fixed.** The row table was never what grew the packet —
    rows are materialized on the client from `CRAFT_RECIPE_LOCATIONS`. The
    stock context was: `amounts` is one `u32` per craft key, and Cooking's rows
    brought `CRAFT_KEY_COUNT` to 125, so the stock alone was 502 bytes of a
    512-byte budget. The wire decision went to the encoding, not the
    budget: stock now travels **sparsely**, as `(u16 key, u32 amount)` for the
    non-zero keys only, and the decoder rebuilds the full-width table so every
    reader still indexes it by `CraftKey` with no lookup. A backpack holds a
    handful of craftable materials, so the packet is now compact in materials
    as well as in rows — the assertion's real subject. Coverage:
    `a_stocked_craft_key_round_trips_without_sending_the_empty_ones`. The
    literal `assert_eq!(rows.len(), 492)` beside it became a lower bound: it
    was a data snapshot that every new trade moves without saying anything.

11. ~~**Leather has no source, so fifty-six tailoring rows are unreachable.**~~
    **Fixed.** #1 again, one trade over, and worse: the dough at least had a
    recipe that could not resolve its ingredient, whereas leather (`0x1081`) had
    no producer at all — not a recipe, not a vendor, not a loot table — while 56
    of the tailoring rows eat it. The other half of the same gap was `carve`
    paying a butcher in hides (`0x1078`) that nothing in the engine consumed.
    ServUO's step between the two is scissors on the pile — `Hides.Scissor`,
    which is `ScissorHelper(from, new Leather(), 1)`: the whole pile, one leather
    per hide, keeping the hue — and it is now `items/src/cut.rs`, reached through
    the same double-click seam smelting is, with `TargetPurpose::Cut` carrying
    the scissors to the second packet. No skill, no roll, no workshop and no
    tool wear: ServUO charges a use only on a Siege shard, so an ordinary pair
    never wears out and this does not invent a durability it does not have.
    Three things were load-bearing:
    - **The grade has to survive the cut**, which is what made hides a *typed*
      kind rather than an art plus a hue: `items.json` id 114, `0x1078` with
      `0x1079` as its flipped alias, in the same `leather` material family the
      tailor's axis already uses. Cutting now carries `MaterialId` straight
      across, so barbed hides become barbed leather instead of the axis's three
      upper grades quietly collapsing into the cheapest one. A pile made before
      the registry knew about hides is still read back from its art, and only
      *within* the leather family — a bare global hue lookup answers plain iron
      and plain wood to the same `Hue::NONE`.
    - **The grades needed a source**, or the fix would have left the axis's
      upper three orphaned the way `ElvenOven` was in #4. `carve` stamps
      ServUO's `BaseCreature.HideType` now: spined for the alligator (`0xCA`)
      and the dire wolf (`0x0017`), regular for everything else it carves. The
      table is keyed by body, so a body two creatures share cannot be split —
      ServUO's hell cat is spined and its housecat is not, and both are `0xC9`,
      so `0xC9` stays regular rather than paying a housecat in monster leather.
      **Horned and barbed still have no source**, and that is content rather
      than a defect: every ServUO creature wearing them is a dragon, drake, wyrm
      or serpent, and none of those bodies is carvable here yet.
    - **Which pack, and where the leather lands.** ServUO's
      `IsChildOf(from.Backpack)` is recursive, so the check is
      `craft_stock_root_of_item` against the cutter's own pack: hides in a bag
      in the pack are in the pack, and hides still lying in the corpse — where
      carving leaves them — are not (cliloc 502437). The leather is given to the
      pile's *own* container, not to the pack root, which is what
      `ScissorHelper` does with the old item's parent.

    Coverage: `scissors_cut_a_pile_of_hides_into_leather_of_the_same_grade`
    (world tick, both packets, and deliberately cutting *spined* hides — with
    regular ones a cut that dropped the grade would still pass; verified to fail
    when the material is hard-coded),
    `scissors_refuse_a_pile_of_hides_still_lying_in_the_corpse`
    (verified to fail without the pack check — and the pile
    is put in a **container** on purpose, because on the bare ground the cut
    refuses anyway, for the duller reason that there is nowhere to put the
    leather, so a test written that way passes with the check deleted),
    `the_bodies_servuo_gives_a_better_hide_keep_it`,
    `every_hide_grade_is_a_leather_grade` (the two kinds must accept the same
    material set, or `cut` either panics or downgrades), and
    `both_hide_arts_read_back_as_the_same_kind`.

12. ~~**Both elven ovens are offered twice, and one of each pair is inert.**~~
    **Fixed.** Found while §6 was giving the loom and the wheel their `kind` and
    `addon`, because the same edit had been made once before and made
    differently: #4 gave the two elven oven facings typed rows by *adding* rows
    beside the generated ones rather than changing them, so `carpentry.json`
    carried two rows for cliloc 1073394 and two for 1073395 — identical skills
    and boards, one installing the oven and one crafting the bare `0x14F0`
    scroll every addon deed shares. A player saw the line twice in group 7 and
    had no way to tell which half worked. The two untyped rows are gone. What
    keeps it from happening a third time is `no_addon_deed_is_offered_twice`
    (`defs/mod.rs`), which asserts that a recipe carrying an `addon` is the only
    row in its trade with its display cliloc — the precise invariant, since
    duplicate names are legitimate elsewhere (tinkering has two, on purpose).
    The blunter reading, "a row that outputs `0x14F0` must be typed", would be
    true of the defect and false of the table: a dozen further ServUO addon
    deeds — the dartboard, the water trough, the bulletin board — are generated
    on that art and still inert, which is content this engine has not reached
    rather than a bug it introduced.

## 8. Deferred, by the roadmap's own list

Repair, Enhance, AlterItem, Resmelt, recipe scrolls, make-number / make-max,
the last-ten list, and the four remaining `Def*` tables (Inscription,
Glassblowing, Masonry, Cartography). Both material chains have landed: hides →
leather in #11, cotton → thread → cloth in §6. What is left of the second one is
its **head** — cotton and flax grow on `FarmableCotton`/`FarmableFlax` plants
upstream and wool comes off a sheared sheep, and here all three are vendor
stock. Hunger and eating effects are outside this slice: food is an ordinary
item.
