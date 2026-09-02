# Finishing the item identity migration: the catalogue, and the adapters it leaves behind

Replacing `Graphic + Hue` as the server's item identity is built as a *model*
and unfinished as a *catalogue*. The registry exists, the typed constructors
exist, persistence round-trips semantic identity, and the data build refuses a
recipe that names an unknown definition — but the catalogue covers 120 item
definitions against a world drawn from thousands of client graphics, so every
gameplay reader still carries a graphic-keyed adapter beside its kind-keyed one.

What is built is [`docs/items/README.md`](../../../docs/items/README.md) and the
model is
[`docs/items/design_item_kind.md`](../../../docs/items/design_item_kind.md).
This page is only what is not, and the order to take it in.

## The order, and what each step is waiting for

The steps are the unfinished halves of stages I2 through I5 in the record,
[`docs/items/evidence/2026-08-30-the-item-kind-migration.md`](../../../docs/items/evidence/2026-08-30-the-item-kind-migration.md).
They are listed by what unblocks the most, not by stage number: nothing after
step 1 can be finished while the catalogue is a pilot.

- [ ] **1. Grow the catalogue past the pilot.** `state/data/items.json` holds 120
      definitions and `materials.json` 20, deliberately chosen to cover the
      material-bearing chains — ore, ingot, board, leather, the blacksmith and
      tailor suits, the tools of every trade, the addon deeds. Everything else a
      player can hold is still an unregistered graphic on the compatibility seam.

      This is data work, not design work, and it is the step every one below is
      waiting for. The build already refuses a duplicate id, a raw-material kind
      with no family, a mismatched armour rating and a projection collision, so
      the cost of a wrong row is a build failure rather than a silent one. Ids
      are append-only reservations: a removed definition keeps its id and a
      rename does not change it.

- [ ] **2. Retire the graphic-keyed adapters, one reader at a time.** Fourteen
      production call sites still read `weapon_data(graphic)`,
      `armor_data(graphic)`, `tool_data(graphic)` or `craft_tool(graphic)`, and
      almost every one of them is already written as `None => …(graphic)` behind
      its kind-keyed sibling. That shape is what makes this step mechanical: a
      reader whose items all carry an `ItemKind` loses its `None` arm, and the
      day the last one does, the graphic-keyed function goes with it.

      Measure with the command, not with this number:

      ```sh
      rg -n 'weapon_data\(|armor_data\(|craft_tool\(|tool_data\(' crates --type rust
      ```

      The two hand-kept tool tables — `craft_tool` by graphic and
      `craft_tool_for_kind` by kind, in `state/src/craft.rs` — must agree until
      the first goes; a `defs` test says so, and Cooking's tools are in only the
      first because no cooking tool has a kind yet.

- [ ] **3. Give `SameAsInput` its input slot.** `MaterialRule::SameAsInput(u8)`
      is defined and deliberately unresolved: the build rejects a recipe that
      names it rather than inferring the inherited material from a hue or a
      material-axis index. It stays unresolved until recipe data has a reason to
      name a slot — a recipe whose output material comes from an input that is
      not the axis line. Nothing shipped needs it, which is why this is a step
      and not a defect.

- [ ] **4. Semantic identity in the item events and the script API.** `ItemUsed`
      carries optional `item_kind` and `material` beside its legacy graphic and
      the fields are `None` for an unmapped item, so a rule written against them
      is correct and incomplete in exactly the way step 1 is. Retaining the
      graphic is deliberate — a script that asks about presentation should get
      presentation — so this step is about the readers, not the payload.

- [ ] **5. Migration fixtures for the two SQL stores and a snapshot.** Both
      directions have fixtures for SQLite; PostgreSQL and the snapshot path go
      through the same audited legacy projection with none of it asserted. This
      is the same missing-CI-server shape as the `server` domain's row 5, and it
      is worth doing in the same session as that one.

- [ ] **6. Real catalogue and workbench captures.** The last unchecked box of I5
      asks for egui PNGs showing material and instance facts sourced from the new
      model. It is acceptance evidence rather than code, and it is last because
      it is worth capturing once the catalogue is not a pilot.

## What this plan does not carry

- **Progress.** When one of these lands, what changes is the state column in
  `docs/items/README.md` and, if the work found something, a record in
  `docs/items/evidence/`. This page loses a box and gains nothing.
- **Defects.** Everything wrong with what is already built — the umbrella rows
  the house catalogue emits and F1 filters out, the legacy `TakeItem(graphic)`
  that marks its event identity `None` — is ranked in `docs/items/README.md`
  § what is open. A defect is not waiting for a plan.
- **The transaction work.** Item ownership, the container index and atomic
  crafting are complete; their model is
  [`docs/items/design_transactions.md`](../../../docs/items/design_transactions.md)
  and their staged record is beside the one above. The one thing they left
  deliberately undone — direct crafting from opted-in house storage — is closed
  as `SearchOnly`, and its unchecked list is an acceptance contract for a
  feature nobody has approved, not a queue.
