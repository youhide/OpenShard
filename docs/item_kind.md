# Item kinds, materials, and recipe graph

Living implementation plan.  This is the canonical design and execution record
for replacing `Graphic + Hue` as the server's item identity.  Update it in the
same change that alters a settled decision or completes a stage.

Open task state is tracked in the [gameplay backlog](roadmap/backlog/gameplay.md).

## Why this exists

Today an item is an entity with `Drawn { id: Graphic, hue: Hue }`.  That is the
right projection for the classic client, but it is not a domain identity:

- `Recipe` names both its output and its inputs by `Graphic`; its material axis
  substitutes a hue.
- weapon, armour, tool, instrument and default-item lookups are keyed by a
  graphic;
- stacking considers equal graphic and hue to mean equal goods;
- persistence stores the same pair as the durable item identity.

The result is that a valorite longsword is currently a longsword picture with a
valorite hue.  Its material bonus, craft consumption and rendering agree only
because every subsystem independently knows the same hue convention.  It cannot
express a new material with a different art, an ingredient family, or a thing
whose gameplay type is independent of its visible tile without another special
case.

The target is a typed recipe graph:

```text
ItemKind::Longsword + Material::Valorite  ──> item instance
                                              └─ Drawn { longsword art, valorite hue }

Recipe::Longsword
  input:  ItemKind::Ingot, selected metal, 12
  output: ItemKind::Longsword, inherit selected metal
```

This is a graph in the *definition data*: recipes link semantic item kinds and
selectors.  It is not a mutable graph stored on entities.  A reverse index can
answer “what can this item contribute to?” without making every inventory item
carry graph edges.

## Settled model

### Definitions

`ItemKindId` and `MaterialId` are stable, opaque numeric newtypes defined in
`openshard-protocol`, because they are valid future client/UI and script-facing
identities.  They are never wire `Graphic`s and never database row numbers.

`openshard-state` owns a generated `ItemDefinition` registry from data files.
An item definition contains the base identity and declarative facts shared by
every instance of that kind:

- name/localization and presentation recipe;
- stack policy and semantic tags;
- base role data (weapon, armour, craft/harvest tool, instrument, container,
  spellbook, runebook, etc.); and
- its legacy graphic(s), solely for projection and migration.

`MaterialDefinition` is a distinct registry.  It contains material family,
presentation/hue policy and mechanical modifiers.  Metal, wood and leather are
families; iron, valorite, oak and barbed leather are material ids.  A hue is an
output of presentation, never the material key used by gameplay.

An **ore item** is therefore `ItemKindId::Ore + MaterialId::Valorite`, not a
different graphic or a hue-only alias.  The kind says what form the thing is
(ore, ingot, longsword); the material says what it is made of and owns shared
properties such as armour bonuses.  This keeps one valorite definition shared
by valorite ore, ingots and equipment.  By contrast, a wholly distinct object
such as a ruby, a spellbook or a backpack receives its own `ItemKindId`; it is
not made into a fake material merely because it has distinct gameplay behavior.

The initially supported, closed set of selector forms is intentionally small:

```rust
enum ItemSelector {
    Exact(ItemKindId),
    KindWithMaterial { kind: ItemKindId, material: MaterialRule },
    Tag(ItemTag),
}

enum MaterialRule {
    Any,
    Exact(MaterialId),
    SameAsInput(u8),
    InFamily(MaterialFamilyId),
}
```

Recipe output names an `ItemKindId`, amount, and an explicit material policy
(`None`, `Fixed`, or `InheritInput`).  It must not infer inheritance from a zero
hue or a boolean like `retain_color`.

### Instances

Every item entity gains mandatory `ItemKind(ItemKindId)` and optional
`Material(MaterialId)` components.  Existing instance facts remain instance
facts: `Amount`, `Quality`, `CraftedBy`, `ItemAffixes`, charges, tool uses, and
the explicit `Weapon`/`Armor` overrides.

`Drawn` stays while the classic protocol needs it, but is a checked projection
of `ItemKind + Material`; creation and material-changing paths obtain it only
from `item_definition::presentation_of`.  It is not a second semantic source of
truth.  Ground art, contained-item art, equipment art and client packets keep
their current `Graphic + Hue` payloads throughout this project.

The broad base tables for weapons and armour move behind `ItemKindId`.  Their
per-instance overrides remain exactly as they are.  A “longsword” is thus a
kind; a particular valorite, exceptional, player-made longsword is an instance.

### Staff/F1 construction

F1 remains able to request arbitrary client art for decoration and debugging,
but it is not a second item-construction path: it sends the administrator form
and the server uses the common item factory.  Registry-known art receives its
`ItemKind` and all required components.  Thus `0x0EFA` creates an empty
`Spellbook`, `0x22C5` a charged `Runebook`, `0x0E75` a `Container`, and tools,
instruments and potions receive their usual fresh state.  An unknown graphic is
an explicit legacy/debug item, rather than a falsely typed object; adding a
playable thing means adding its definition and factory facts first.

## Boundaries and invariants

1. Gameplay code compares `ItemKindId`, material and tags — never `Graphic` or
   `Hue` — when answering what an item *is*.
2. Only presentation, client-wire adapters, legacy-save migration and the item
   definition registry may name a `Graphic` as an identity lookup key.
3. `Drawn` must equal the definition registry's projection for the item's kind
   and material.  The one intentional exception is a documented legacy item
   awaiting migration, never a newly spawned item.
4. Stack equivalence is `kind + material + stack-compatible instance state`, not
   art equality.  Crafted/signature/affixed items cannot merge merely because
   their art is the same.
5. Every recipe selector and output kind resolves at build time.  A material
   inheritance slot must name a real compatible input; ambiguous or cyclic
   definitions fail the data build with the recipe name.
6. Save/load round-trips semantic identity.  Legacy `graphic+hue` is read only
   through a total, audited migration table; an unmapped legacy pair is retained
   as an explicit migration failure, never silently assigned a nearby kind.

## Migration stages

### I0 — inventory and contract tests

- [x] Catalogue every non-test gameplay lookup currently keyed by `Graphic`:
  spawn/defaults, containers, stack, equip, combat, armour, skills, crafting,
  vendors, persistence and script events.  The owning sweep is recorded below.
- [x] Characterize legacy stack identity, item persistence, craft consumption
  and material-derived armour before changing their data.
- [x] Define the numeric-id allocation and data-file format; reserve ids rather
  than deriving them from graphics or array position.

#### I0 record (2026-08-30)

The semantic-lookup sweep is deliberately narrower than a search for every
`Graphic`: map decoration, packet encoding, gumps and client rendering are
presentation boundaries and stay graphic-shaped.  These production owners are
the migration set:

| owner | current graphic identity work | later stage |
|---|---|---|
| `state::{weapon,armor,craft,harvest}` and `items::defaults` | base weapon/armour/tool/instrument facts and default components | I1, I4 |
| `items::{spawn,containers,backpack,stack,drag,equip,capacity,trigger}` | item construction, split/merge, pack search, equip and use | I2, I4 |
| `crafting::{recipe,consume,craft,system,gump,defs}` | recipe inputs/output, material axis and workbench projections | I3 |
| `combat::{weapons,armor}`, `skills::{harvest,appraise,poison}`, `npc::vendor`, `world::tick::{shipped_items,speech}` and `state::runtime` | gameplay classification and properties | I4 |
| `persistence::{record,sqlite,pg}` and `world::tick::persist` | stored item identity and restore | I2 |
| `items::trigger` and the script-event adapters | item use/event identity | I4 |

The legacy behaviour is held by existing, named tests before the representation
changes: `world::tick::tests::dropping_a_stack_onto_an_identical_one_merges_them`,
`world::tick::crafting_tests::a_valorite_order_cannot_be_paid_in_iron`,
`world::tick::crafting_tests::one_crafted_arrow_joins_the_stack_already_in_the_pack`,
and the valorite-armour assertion in
`world::tick::crafting_tests::material_and_exceptional_armour_are_worth_more`.
Persistence's item-record/restore round trip is covered by the world persistence
fixtures; I2 adds an explicit semantic-identity fixture alongside them.

`items.json` and `materials.json` use objects with an explicit positive `id`;
ids are append-only reservations, never a graphic value, hue, JSON position or
database primary key.  Removing a definition reserves its id and a rename keeps
it.  The generated registry is their sole reader and validates duplicate ids,
references and legacy `(graphic, hue)` mappings during the state build.
An item may also declare legacy graphic aliases (such as flipped tool art): all
aliases resolve to the same kind, while new semantic construction uses its one
canonical projection.

Done when the migration table has an owner and every existing graphic-based
semantic lookup is either assigned to a later stage or explicitly justified as
presentation-only.

### I1 — semantic registry and projection

- [x] Add `ItemKindId`, `MaterialId`, material families and item tags to
  `openshard-protocol`.
- [x] Add generated `items.json`/`materials.json` registry data in `state`, with
  `definition`, `presentation_of`, and legacy `kind_from_drawn` helpers.
- [x] Add `ItemKind` and `Material` ECS components plus construction helpers
  that install both semantic identity and `Drawn` in one operation.
- [x] Convert registered static weapon and armour definition rows to be keyed by
  kind while retaining legacy graphic adapters at their boundaries. Unregistered
  rows remain on those adapters until they receive definitions.

Done for the registered catalogue: every semantic constructor creates a kind,
and installing an identity rewrites `Drawn` from its registry projection. A
test proves that even a deliberately mismatched prior drawing cannot survive.

### I2 — item lifecycle and persistence

Current migration slice: typed spawn, harvest, smelting, selected crafting rows,
F1's known functional items, persistence and both ground/container split paths
carry `ItemKind + Material`. A split copies the original components directly;
it never runs its own `Drawn` → kind migration. Legacy callers and unregistered
art remain explicitly on the compatibility seam until their definition rows are
added.

- [ ] Convert spawn, vendor stock, loot, container helpers and scripts to spawn
  an `ItemKindId` plus optional `MaterialId`.
- [ ] Define semantic stack equivalence and migrate split/merge/give paths.
- [ ] Version persistence records and SQL schemas with `kind` and `material`.
  Old records populate those fields through the audited legacy mapping on load;
  new records write semantic identity.
- [ ] Audit every copy/split/restore path so it carries kind, material and
  stack-compatible instance facts.

Done when an old save loads to the same visible item, a new save reloads without
consulting legacy art, and stack tests cover equal art/different kind as well as
equal kind/different material.

### I3 — recipe graph and crafting

Current migration slice: `blacksmithy.json`'s longsword and plate chest name
output kinds 4 and 5, inherit material from input 0, and name that input as
`KindWithMaterial` ingot. Their legacy art/hue fields remain only for the
classic gump and old-save bridge. The data build rejects selector forms whose candidate-set or
cross-input evaluator is not implemented yet, rather than accepting a row that
would silently fall back to graphic identity.

- [ ] Replace `Recipe.graphic`, `CraftRes.graphic/hue/from_axis`, `SubResAxis`
  and `retain_color` with typed output and selector/material policies.
- [ ] Make consumption select instances by semantic selector; material choice is
  a `MaterialId`, not an axis index or hue.
- [ ] Build forward and reverse recipe indexes: recipe → inputs/output and
  `ItemKindId`/tag → candidate recipes.
- [ ] Make crafting create a typed output through the common item constructor;
  quality, maker's mark and affixes remain instance updates after creation.
- [ ] Update craft catalogue/workbench packets to expose kind/material identity
  while retaining art only as a rendering projection.

Done when a recipe can require an exact item, a material family or a selected
material; a different-looking but semantically equal material works; and a
same-looking but wrong kind cannot pay an input.

### I4 — gameplay readers and client surfaces

- [ ] Move combat, armour, equipment, harvest, instruments, poison, appraisal,
  tool opening and vendors to `ItemDefinition`/`MaterialDefinition` reads.
- [ ] Change item events and script APIs to include kind/material; retain
  graphic only where a script explicitly asks for presentation.
- [ ] Update tooltips, inventory, vendor, craft catalogue and craft workbench to
  present properties from definitions and instances, not by graphic-table
  inference.
- [ ] Remove compatibility helpers once the last gameplay reader has moved;
  leave only wire/render conversion at the client boundary.

Done when a grep for gameplay `weapon_data(graphic)`, `armor_data(graphic)`,
`craft_tool(graphic)`, `tool_data(graphic)` and graphic-based stack comparisons
has no production callers outside documented adapters.

### I5 — validation and evidence

- [ ] Build-time validation for all definition ids, recipe references, material
  policies, render projections and reverse-index coverage.
- [ ] Migration fixtures for pre-ItemKind SQLite, Postgres and snapshot saves.
- [ ] End-to-end fixtures: craft iron/valorite variants; persist/reload them;
  verify distinct combat/armour properties; stack only legal equivalents.
- [ ] Capture real egui catalogue and workbench PNGs that show material and
  instance facts sourced from the new model.

## Explicit non-goals

- Replacing the classic UO `Graphic + Hue` network payload.
- Inventing a free-form property bag for all gameplay.  New mechanics use typed
  components or validated affix variants.
- Rebuilding every art asset.  An item kind may initially use the same art and
  hue mapping it uses today.
- Treating recipe edges as mutable entity state.

## Current status

I0 is complete; I1–I4 are in progress. `openshard-protocol` now owns the opaque
`ItemKindId`, `MaterialId` and `MaterialFamilyId` newtypes; `openshard-state`
generates the first `items.json`/`materials.json` registry and validates the
`kind + material → Drawn` projection. Its initial catalogue deliberately covers
the material-bearing pilot (`ingot`, `ore`, `log`, `longsword`, `plate chest`)
rather than inventing ids for every legacy graphic. Typed construction and the
armour material reader use it; unmapped construction remains an explicit legacy
adapter until its definition row and lifecycle caller are migrated. `ItemRecord`
and both SQL stores now preserve optional semantic fields (schema v35); restore
verifies the saved projection and only falls back to the audited legacy mapping
when a pre-ItemKind record has no semantic identity.

Both directions have persistence fixtures: post-migration records restore their
stored ids without art inference, while a pre-ItemKind valorite-ore record
migrates through its audited projection and retains the same visible drawing. A
record whose saved ids contradict its stored drawing remains explicitly
untyped, rather than being silently reclassified as either value.

The first end-to-end material chain is typed: mining pays semantic ore and its
material id, and smelting validates that ore identity before creating ingots
with the same material. The registry now also names boards and leather, so the
existing wood/leather craft axes resolve their carried inputs as exact
`ItemKindId + MaterialId` pairs; common cloth and empty-bottle inputs now have
exact material-less kinds as well. The legacy graphic/hue read remains only for
instances created by older constructors and saves.

Smelting's difficulty table is keyed by explicit `MaterialId` entries, never by
an arithmetic conversion of an id into an ore-table position. This keeps the
append-only material namespace safe when a grade is added or reserved later.

Fletching now has a typed resource chain too: wood-material boards are consumed
into material-less shafts, and typed shafts plus feathers create typed arrows.
The output policy explicitly states `None`, preserving the existing rule that
these products do not inherit the wood grade just because their input did.
Bolts now follow the same material-less exact-input chain, while bow, crossbow
and heavy crossbow inherit the selected wood `MaterialId` and read their combat
rows directly by kind.

The same explicit policy now applies to a smith's tongs: tinkering consumes a
typed ingot and produces `ItemKindId::Tongs` with that ingot's `MaterialId`.
Thus a valorite pair is not merely tongs art painted valorite; it carries the
same material definition as its ore and ingot source.

The rest of the currently registered metal tinker tools follow that policy as
well (shovel, hammers, saws, mortar, tinker's and fletcher's tools, and the
carpentry hand tools). Their normal F1 art resolves to the plain iron material;
crafting from a selected metal grade preserves that grade as durable identity.
Every current recipe that yields a registered craft tool now also names that
tool's `ItemKindId`; sewing kits are material-less because their tinkering and
tailoring recipes do not share one material axis.

Craft consumption resolves a registry-known ingredient to its semantic identity
before counting or taking it; an unmigrated saved pile is accepted only when its
audited projection is exactly that identity. This already prevents a same-art,
same-hue item carrying a different `ItemKind` from paying a valorite input,
while rows without definition coverage remain on the explicit legacy adapter.

Definitions also carry closed semantic tags and the registry evaluates exact,
material-family and tag selectors. `SameAsInput` remains deliberately unresolved
until recipe data names the input slot it inherits from; it must not be inferred
from a hue or material-axis index.

The craft egui workbench still receives classic art for rendering, but its
display payload now also carries `ItemKindId` and `MaterialId`; each material
axis row names its resource `ItemKindId` and `MaterialId` directly, rather than
deriving either from graphic or hue. Art remains a presentation adapter, not
recipe identity.

The initial weapon and armour kinds are now read semantically by combat,
equipment and Arms Lore: the combat rows themselves carry the longsword and
plate-chest `ItemKindId`, so these reads do not round-trip through client art.
The blacksmith pilot now also covers the classic ringmail/chainmail suits and
the regular plate arms, gloves, gorget, legs and helm; their recipes inherit
the selected ingot `MaterialId` and their armour rows are keyed directly by
their own kinds. The same direct treatment now covers the classic metal helms
and shields made by the blacksmith, plus the classic broadsword, cutlass,
dagger, katana, kryss, scimitar, viking sword, axe-family, polearm, fencing
and macing recipes. Tinkering's classic pitchfork follows the same typed
metal-output policy.

Tailoring now has the corresponding base leather armour slice: gorget, gloves,
sleeves, leggings and chest inherit the selected leather `MaterialId`, and their
kind-keyed armour rows retain the material bonus. The classic studded suit has
the same typed lifecycle, retaining its own base rating and half-meditation
rule instead of being inferred from client art.
Harvesting, crafting and poisoning do the same for a declared tool, weapon or
instrument. The pilot catalogue now also
includes a pickaxe, smith's tongs and the full unique instrument set (lute,
harps, drums and both tambourines), plus the primary shovel and fishing pole,
and all current unique tools for every craft profession (including the full
carpentry set). F1 creates their real use components and their semantic
identity, and the typed tinkering pickaxe recipe preserves the consumed ingot
material. Their large legacy tables are still keyed by graphic behind named
registry adapters; only items with no `ItemKind` may use that compatibility
path. This is deliberately transitional until the remaining catalogue rows
move into the registry.

The world test suite runs F1 through every registry definition and checks both
the emitted `ItemKindId` and every declared role (weapon combat row, armour row,
tool's harvest/craft reader, instrument reader, container, spellbook or runebook
state). A new definition therefore cannot become a display-only item by accident.

The registered harvest tools, craft tools and instruments are now classified by
their `ItemKindId` directly; their old graphic tables are compatibility
adapters for unregistered legacy art only. This makes an explicit semantic tool
immune to a future presentation alias changing its behaviour.
Every registered craft-tool role is also checked to resolve to a craft system by
its kind, before a client can attempt to open a workbench.

The F1 form remains art-addressable so staff can inspect arbitrary installed
client assets. When its submitted `(graphic, hue)` is a registry projection it
constructs by `ItemKindId + MaterialId` directly; only unmapped art takes the
explicit legacy/debug path.

The generic world `SpawnItem` command and compatibility `GiveItem` reward path
follow the same rule, which keeps staff commands and scripted world setup from
accidentally creating a registered item without its semantic components.
New scripts can instead use `GiveItemKind` and `AddLootKind`, which name the
identity directly and have no art/hue input. `TakeItemKind` is the corresponding exact
quest/pack API: it selects `ItemKindId + MaterialId` and reports that identity
in `ItemsTaken`. It accepts a legacy pile only where its audited projection is
the same identity; a valorite ingot can therefore never pay an iron objective
merely because both use the ingot graphic. The older `TakeItem(graphic)` remains
an explicitly presentation-only compatibility command and marks its event
identity as `None`.

Vendor `StockLine` now likewise accepts an optional direct identity and upgrades
an audited legacy presentation at stocking time. The shelf entity, restock
record and bought backpack item retain that kind/material; a typed shop's
buy-back price matches the same identity rather than treating its graphic as a
wildcard. Alongside its legacy-format restock projection, new snapshots persist
authoritative typed stock lines and restore their projection from those IDs.
The tuple remains only a backward-compatible fallback for old snapshots, where
restore upgrades registry-known rows.

The craft workbench continues to send classic art for rendering, but its live
"carried" material totals now resolve a known `(graphic, hue)` through the
registry and count `ItemKindId + MaterialId` (with the same audited legacy
bridge as recipe consumption). A same-art wrong kind no longer inflates the
material number the player sees.

`CraftWorkbenchMaterial` now sends the selected resource `ItemKindId` and
`MaterialId` alongside its graphic/hue projection. The client need not
reverse-engineer either the resource or tint; the packet round-trip and a real
tongs-opened workbench both assert the default iron row is
`ItemKindId(1) + MaterialId(1)`.

`CraftWorkbenchComponent` carries optional `ItemKindId` as well. A migrated
recipe result uses its declared `Recipe.kind`; registry-known inputs expose
their kind beside the art, while still-legacy recipe rows explicitly leave it
absent rather than inventing an ID in the client.

The full craft catalogue has the same optional identities on each result and
component. Its default material-axis row is the explicit regular
`ItemKindId + MaterialId` entry, while unmigrated recipe rows remain visibly
`None` instead of pretending that their art is a durable type.

Container kinds carry their opening gump in the item definition. The common
factory installs `Container` from that field, so `GiveItemKind(backpack)` is an
openable backpack rather than a correctly named inert picture; containers also
do not retain a ground-decay clock.

Backpack capacity uses the same semantic stack predicate as insertion. A typed
award does not claim a free merge slot merely because an unmigrated or
wrong-kind pile happens to draw with the same graphic/hue.

That predicate also rejects items carrying maker, quality, affix, weapon-override
or poison-charge state. Such data belongs to an individual item, so it cannot be
lost merely because two objects share a kind, material and client presentation.

Legacy hue bridging is family-qualified where the caller knows a materialized
kind. This matters even for `Hue::NONE`: it is shared by iron, regular wood and
regular leather, so a log recipe's selected plain material is `MaterialId(20)`,
not the first global hue match (iron).

The registry build now rejects empty/duplicate IDs and tags, duplicate
projection rows, materialized kinds whose family has no grades, raw-material
kinds without a family, mismatched armour tag/rating declarations, and a
definition claiming both distinct book roles.
The test suite also round-trips every valid registered kind/material projection
through the legacy bridge, so a future material or item cannot silently make a
non-invertible presentation pair.

The crafting build independently reads those id reservations and rejects every
typed recipe output, selector, material axis and fixed material that names an
unknown definition. A recipe typo is therefore a build failure, not a row that
silently becomes unreachable at runtime.
It also checks that a material axis uses the target kind's family and canonical
hues, and that `None`/`Fixed` output policies are compatible with the output
kind's declared material family. `InheritInput` is likewise required to name a
typed material-bearing input of that same family.

`ItemUsed` now exposes optional `item_kind` and `material` beside its legacy
graphic, so scripts can migrate rules without reverse-engineering client
presentation. The fields remain `None` for still-unmapped legacy items.

The item-transaction layer now consumes the generated recipe/identity graph as
a shared client/server artifact. Each exact semantic or audited legacy selector
has a dense `CraftKey`; recursive backpack stock maintains totals and ordered
pile candidates under that key on mutation. Catalogue context carries those
totals and the shared catalogue revision, while the client derives readiness
for all 492 rows locally. A craft request still names one stable recipe and the
server prepares its withdrawal from current canonical `ItemLocation`, amount,
kind, and material facts before any output or input is committed.

Restore rejects zero or over-`MAX_STACK` saved amounts before allocating an
entity. Live amount changes go through the stack mutation door, and item
ownership projects into exact `ContainedItems`; neither the semantic identity
bridge nor a stale index can authorize consumption. House inventory search uses
the same generated identity catalogue for bounded exact selectors, but its
permissioned results are read-only and direct house-storage crafting remains a
separate, declined policy.
