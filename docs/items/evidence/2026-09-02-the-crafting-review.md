# Crafting, reviewed: twelve problems and what closed them

> **This is a record.** It was written as part of `docs/crafting.md` and is kept
> as it was written. The model it describes as built is
> [`../design_crafting.md`](../design_crafting.md) — where the two differ, the
> design is right — and what is still open is ranked in
> [`../README.md`](../README.md), which is the only status page for this domain.
> Four comments in `crates/server/world/src/tick/` cite this review by point
> number; point 3 is the addon placement and grouping one.
>
> **Its section numbers are that document's, not this file's.** §1–§4 are
> [`../design_crafting.md`](../design_crafting.md); §5, §6 and §7 are the three
> sibling records beside this one.

## 8. Review: problems found

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
