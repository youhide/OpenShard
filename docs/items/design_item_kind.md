# Item kinds, materials, and recipe graph

The model that replaces `Graphic + Hue` as the server's item identity: what an
item kind and a material are, what a recipe selector may say, and the rules the
registry holds. It is the model as built, with no status in it — how far the
migration got is [`README.md`](README.md), the staged record is
[`evidence/2026-08-30-the-item-kind-migration.md`](evidence/2026-08-30-the-item-kind-migration.md),
and what is not built is
[`plans/items/item_identity/PLAN.md`](../../plans/items/item_identity/PLAN.md).

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

F1's primary catalogue is semantic: it submits an `ItemKindId` and optional
`MaterialId`, and the server uses the common item factory. The rows are concrete
registry definitions and material variants, so a created spellbook, runebook,
container, tool, instrument or potion receives all of its usual gameplay state.
Arbitrary `(graphic, hue)` construction remains available only in an explicitly
labelled legacy/debug section. An unknown graphic creates a legacy item rather
than a falsely typed object; adding a playable thing means adding its definition
and factory facts first.

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

## Explicit non-goals

- Replacing the classic UO `Graphic + Hue` network payload.
- Inventing a free-form property bag for all gameplay.  New mechanics use typed
  components or validated affix variants.
- Rebuilding every art asset.  An item kind may initially use the same art and
  hue mapping it uses today.
- Treating recipe edges as mutable entity state.
