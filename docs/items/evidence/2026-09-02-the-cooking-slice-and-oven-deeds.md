# The Cooking slice, and the deeds that install an oven

> **This is a record.** It was written as part of `docs/crafting.md` and is kept
> as it was written. The model it describes as built is
> [`../design_crafting.md`](../design_crafting.md) — where the two differ, the
> design is right — and what is still open is ranked in
> [`../README.md`](../README.md), which is the only status page for this domain.
>
> **Its section numbers are that document's, not this file's.** §1–§4 are
> [`../design_crafting.md`](../design_crafting.md); §6, §7 and the numbered
> review are the three sibling records beside this one.

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
