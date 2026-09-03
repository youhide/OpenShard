# Items and crafting: where they stand

The canon of the `items` domain — `crates/server/items` and
`crates/server/crafting`, plus the tables they are written against in
`server/state` (`item_definition`, `craft`, `weapon`, `armor`, `harvest`) and the
catalogue `common/protocol`'s build script generates for both ends of the wire.
This is everything a thing in the world *is* and everything a player can *make*:
where an item can be, what it weighs, what it stacks with, who owns it, and the
eight trades that turn one item into another.

What a house does with an item that is locked down belongs to `housing`; what a
weapon does when it swings belongs to `combat`; what the client draws belongs to
[`client/`](../client/README.md) and [`render/`](../render/README.md).

**One entry point.** This page answers "what can a player do with a thing today"
and says which document holds the reasoning for each line. Where this page and a
design document disagree, the design document is right and this page is stale.

## The one-line answer

**An item is in exactly one place, is exactly one kind of thing, and every
compound change to it is prepared before it is committed.** The three places —
on the ground, in a container, worn — are mutually exclusive projections of one
canonical `ItemLocation`. The identity is `ItemKindId + MaterialId`, and the
`Graphic + Hue` the classic client needs is derived from it. Nothing that can
fail runs after the point of no return.

```text
  ItemLocation ──projects──> Position | Contained | Equipped | on a cursor
       │
       ├─ ContainedItems         a container's own child list, not a world scan
       ├─ craft stock            (root, CraftKey) -> total + ordered piles
       └─ house inventory        (house, standing, identity) -> total + roots

  prepare(&state) -> Prepared…   allocates, validates, publishes nothing
  commit(&mut state, prepared)   cannot ordinarily fail
```

## What the area is, by part

| Part | State | What is left | Held by |
|---|---|---|---|
| Three exclusive places for an item, one canonical edge, projections maintained together | ✅ shipping | — | [`design_transactions.md`](design_transactions.md) § Ownership |
| Lift, drop, bounce, drag cancel; a partial lift that allocates its remainder before it reduces anything | ✅ shipping | — | the same, and [`evidence/2026-08-31-the-transaction-stages.md`](evidence/2026-08-31-the-transaction-stages.md) A1 |
| Stacking: one write door, a ceiling of `MAX_STACK`, semantic equivalence rather than art equality | ✅ shipping | — | the same, A3 |
| Capacity and weight: `MAX_ITEMS` and the recursive parent chain | ✅ shipping | — | `items/src/capacity.rs`'s own docs |
| Exact container membership: `ContainedItems` beside `Worn`, revalidated on read | ✅ shipping | — | [`design_transactions.md`](design_transactions.md) § `ContainedItems` |
| Secure trade, doors and keys, mounts, chairs, ground decay | ✅ shipping | — | the crate's own module docs |
| The item-trigger seam: the engine keeps no default behaviour for a bare item | ✅ shipping | — | [`evidence/2026-08-24-the-items-phase.md`](evidence/2026-08-24-the-items-phase.md) |
| Semantic identity: an opaque `ItemKindId`/`MaterialId`, a generated registry, typed construction that writes `Drawn` from the projection | ✅ shipping | the four role tables are closed; the rest of the catalogue is a pilot — row 1 | [`design_item_kind.md`](design_item_kind.md) |
| Semantic identity across a save: records and both SQL stores carry it, restore falls back to the audited legacy mapping only for a pre-`ItemKind` row | ✅ shipping | only SQLite is asserted — row 8 | the same, and [`evidence/2026-08-30-the-item-kind-migration.md`](evidence/2026-08-30-the-item-kind-migration.md) |
| Crafting: the trades of `crafting/data/craft_systems.json`, ServUO's odds, its gump encoding, a workshop scan that reads statics as well as items | ✅ shipping | the `Def*` tables not ported — row 6 | [`design_crafting.md`](design_crafting.md) |
| Every gate checked twice, band failure distinguished from roll failure, the RNG staged so a refusal consumes no randomness | ✅ shipping | — | the same, § 3 |
| Atomic crafting: a withdrawal plan that reserves each pile once, output prepared before ingredients are spent | ✅ shipping | — | [`design_transactions.md`](design_transactions.md) § Withdrawal plan |
| Catalogue availability derived on the client from a generated artifact and a compact context; the server checks only the recipe that was chosen | ✅ shipping | — | the same, and the record's A7 |
| House inventory search: indexed, permissioned, paginated, read-only — Ctrl+I | ✅ shipping | — | the record's A6a |
| Crafting directly out of house boxes | ⬜ deliberately absent | closed as `SearchOnly`; the unchecked list is an acceptance contract, not a queue | the record's A6b |
| House addon deeds: a typed item, a checked footprint, one addon out of many tiles, a refund on release or collapse | ✅ shipping | a dozen further deeds are inert — row 2 | [`evidence/2026-09-02-the-cooking-slice-and-oven-deeds.md`](evidence/2026-09-02-the-cooking-slice-and-oven-deeds.md) |
| The material chains, end to end: ore → ingots, hides → leather, fibre → thread → bolt → cloth | ✅ shipping | — | [`evidence/2026-09-02-the-cloth-chain.md`](evidence/2026-09-02-the-cloth-chain.md) |
| The head of those chains: a cotton field to pick, a sheep to shear | ✅ shipping | flax has no field, upstream included | [`evidence/2026-09-03-the-chains-head.md`](evidence/2026-09-03-the-chains-head.md) |
| Inscription: sixty-four scrolls on mana and a spellbook you own, and the runebook that had no source at all | ✅ shipping | necromancy and mysticism scrolls wait on their schools; Inscription's own use button copies books, which this engine has not got | [`evidence/2026-09-03-the-inscription-trade.md`](evidence/2026-09-03-the-inscription-trade.md) |
| Repair, Enhance, AlterItem, Resmelt, recipe scrolls, make-number/make-max, the last-ten list | ⬜ not built | each is its own system hanging off crafting — row 6 | [`evidence/2026-08-24-the-crafting-phase.md`](evidence/2026-08-24-the-crafting-phase.md) |

## What is enforced, and by what

The order this domain keeps arriving at is the one the `server` invariants sweep
wrote down: **a type beats a build-time check, and a build-time check beats a
test.** Items have an unusual amount of the first two, because most of what can
go wrong here is a *table* being wrong rather than a function.

- **A live stack cannot hold an illegal amount.** `write_stack_amount` is the one
  door that writes `Amount`, and it panics outside `1..=MAX_STACK`. Zero is not
  an `Amount(0)`; it is the absence of an item. Two `#[should_panic]` tests hold
  both edges, and `spawn` refuses an invalid amount *before* allocating an
  entity, so a bad request cannot leak a serial.
- **A recipe that names something that does not exist is a build failure.**
  `crafting/build.rs` reads `state/data/items.json` and `materials.json` and
  refuses, at `cargo check`: a group index out of range, a recipe that does not
  lead with its system's own skill, a resource line count over the budget, an
  output whose `kind` and `output_material` contradict each other, an
  `InheritInput` that names a slot of the wrong family, an addon row whose kind
  is not that addon's own deed kind, and any selector form whose evaluator is not
  implemented yet. A typo is therefore never a row that quietly becomes
  unreachable at runtime.
- **The registry build refuses a catalogue that cannot be inverted.** Empty or
  duplicate ids and tags, duplicate projection rows, a materialized kind whose
  family has no grades, a raw material with no family, an armour tag that
  disagrees with its rating, a definition claiming two book roles, and a
  `shared_art` graphic colliding with one claimed uniquely. The test suite
  round-trips every valid kind/material projection through the legacy bridge, so
  a new row cannot make a non-invertible presentation pair.
- **The catalogue is checked for holes, not just for bad rows.** Fifteen tests in
  `crafting/src/defs/mod.rs` ask questions no single feature would: that every
  trade has exactly one system, that every tool on a vendor's shelf opens one,
  that an addon recipe outputs its own deed kind, that no addon deed is offered
  twice, that the material axis substitutes into a line that actually wants it,
  that the metals are the same hues the ground yields, and that every craftable
  ranged weapon has combat rules. A table nobody opened would pass "no bad rows"
  and fail these. Four sweeps in `state` ask the mirror question — for every art
  in `WEAPONS`, `ARMOR`, `craft_tool` and `tool_data`, the registry must name a
  kind whose row says the same thing — so a table keyed by art and its twin keyed
  by identity cannot drift apart, which is the one way an item behaves
  differently depending on where it came from.
- **Two chains are pinned at the joint.** Every hide grade must be a leather
  grade, or the cut either panics or silently downgrades; the woolly and shorn
  sheep bodies must differ, or a sheep is an infinite fleece; the loom's four
  loading clilocs must be consecutive, because the code adds the phase to the
  first.
- **The ownership graph is audited after refusals, not only after successes.**
  `audit_item_graph` runs at operation boundaries and in the drag tests that
  exhaust the serial pool on purpose. Six property suites of 256 cases each
  cover split, merge, give, remove and consume around `1`, `MAX_STACK` and
  refused values; the release soak ran 10,000 sequences of up to 512 actions and
  produced no regression seed.
- **`prepare` and `commit` are two types, not two comments.** `PreparedPlacement`
  and `WithdrawalPlan` own already-allocated entities and asserted expectations;
  commit re-checks that nothing moved and then cannot fail. A prepare that fails
  despawns what it made before returning.

One crate-wide invariant sits above all of it, and it is stated as a dependency
rather than as prose: **`items` depends on neither `crafting` nor `skills`.**
Making a thing and eating its materials are both doors in `items`, so the graph
stays acyclic and the crate that owns a mutation is never the crate that wanted
it.

## What is open, ranked

**1. 🚩 The identity catalogue is a pilot, and every gameplay reader still
carries its graphic-keyed twin.** `state/data/items.json` holds 145 definitions
and `materials.json` 20, while the world is drawn from thousands of client
graphics; 472 of the 599 shipped recipes still name no kind, almost all of them
decor, containers, food, scrolls and expansion art. What is *no longer* a pilot
is the part with a gameplay role: every art the weapon, armour, craft-tool and
harvest-tool tables answer for is a registered kind, four whole-table sweeps say
the two halves agree, and the flipped facings are named beside their canonical
art. Production call sites still read `weapon_data(graphic)`,
`armor_data(graphic)`, `tool_data(graphic)` or `craft_tool(graphic)`, almost all
as the `None` arm behind a kind-keyed sibling. That shape is the good news: the
adapters retire one reader at a time. The order is
[`plans/items/item_identity/PLAN.md`](../../plans/items/item_identity/PLAN.md).
Measure with the grep, not with this paragraph.

Growing this catalogue is not additive, which is the thing to know before adding
the next row: `spawn_item` installs the semantic identity of every art the
registry names, so a definition added *moves* live items off the graphic path
onto the kind path. A gap in the kind-keyed twin becomes a live defect the day
its definition lands —
[`evidence/2026-09-03-the-role-tables-close.md`](evidence/2026-09-03-the-role-tables-close.md)
is what that cost the axes.

**2. A dozen addon deeds are craftable and inert.** Carpentry group 7 carries
generated rows for ServUO's dartboard, water trough, bulletin board and the rest
— all on the generic `0x14F0` scroll, none with a `kind` or an `addon` — so a
carpenter spends the boards and receives a scroll that does nothing. Each wants
an `AddonKind`, a deed kind, and whatever the installed thing *does*; the ovens,
the loom and the wheel are three worked examples of the same five-line pattern.
Only rows that carry an addon are gated today, because "outputs `0x14F0` implies
typed" would be an assertion about content this engine has not reached rather
than about a defect.

**3. Flax's second facing would not spin.** `Fibre::from_graphic` knows flax as
`0x1A9C` alone, and ServUO's `FarmableFlax.GetCropObject` draws the picked pile
at random between `0x1A9C` and `0x1A9D`. Harmless while nothing plants flax —
which is upstream's own state — and a pile a player cannot spin the day
something does. The fix is the alias the hides already carry; it used to wait on
the wool field beside it, and that field has landed, so it now stands on its own.

**4. The house item catalogue emits rows that cannot be built.** For metal, wood
and leather it emits a material-less semantic identity beside every concrete
material; that identity is not constructible and F1 filters it out. Search should
model "any material" as a selector distinct from an exact identity, or stop
emitting the invalid exact row — the current answer is a filter at one consumer,
which is the shape that goes wrong when a second consumer arrives.

**5. A restart in the middle of a craft ends it silently.** The in-flight
`Crafting` component and the per-player gump context are deliberately not saved,
and the loss is benign — nothing is consumed before `complete`. What is not
benign is that nobody is told: the player watches a craft that will never
finish. The wheel and the loom split on the same question and the split is the
rule rather than an accident — a timer whose cost is already paid is saved
(`LoomPhase`), a timer whose cost is not is dropped and its art stamped back on
restore.

**6. The trades ServUO has and this shard has not.** Repair, Enhance, AlterItem,
Resmelt (item back to ingots; *ore* smelting is in), recipe scrolls,
make-number/make-max and the last-ten list — the last of which wants a decision
about saving UI state, because ServUO serializes it per player. The `Def*` tables
still unported are data the generator can emit when they are wanted. Read
`crafting/data/` for which trades exist rather than a sentence that names them,
because that list moves and a sentence does not.

**7. Two hand-kept tool tables must agree, and nothing but a test says so.**
`craft_tool` by graphic and `craft_tool_for_kind` by kind, in
`state/src/craft.rs`, and the same pairing in `harvest.rs`, `weapon.rs` and
`armor.rs`. What says so is now a sweep per table rather than a spot check —
every art the graphic-keyed half answers for must resolve through the registry
to a kind whose row says the same thing — and the two tool sweeps count the arts
they checked, because their loop body is conditional. Each pair disappears with
row 1, so this is a note for whoever adds the next trade rather than a defect to
fix on its own.

**8. Only SQLite is asserted across a save.** Both directions of the semantic
identity round trip have fixtures; PostgreSQL and the snapshot path go through
the same audited projection with none of it asserted. It is the same
missing-CI-server shape as [`server/`](../server/README.md) row 5 and is worth
doing in that session.

**9. The display-art sweep is done once, by hand, and cannot be redone by the
tree.** All 1,082 `GenericBuyInfo` rows were compared against their type's
constructor, its `[FlipableAttribute]` and `tiledata`'s own name for the tile:
fourteen shelf lines carried the shop window's picture instead of the item, and
they are fixed — the helms that had no armour rating, the bowl of carrots sold as
corn, the four "uncut cloths" that were folded-cloth decor. Thirteen lines went
away rather than change, because a shelf cannot hold two lines with one
`(graphic, hue)` — `restock` matches on that pair and reaches only the first —
and `build.rs` now refuses such a file. What is *not* solved is repeatability:
ServUO is not in the tree, so nothing here can re-run the comparison against a
newer upstream. The procedure is written down in
[`evidence/2026-09-03-the-vendor-display-art-sweep.md`](evidence/2026-09-03-the-vendor-display-art-sweep.md);
re-running it is a hand pass against a checkout. Only weapons and armour were
also checked the other way — that a shipped art is one our tables can read — so
food, containers and tools have had no such pass, and that one wants row 1's
catalogue rather than another sweep.

**10. Smaller, and each is written where it lives.** `environment::is_mill`
lists `0x1295` and `0x129F`, which are almost certainly ServUO's own misprints
for `0x1925`/`0x192F` — left verbatim on purpose, with a comment warning against
a silent "fix" that would be parity drift, because upstream parity is the point
of the port. `MAX_CRAFT_RESOURCE_LINES` is written on both sides of codegen, in
`crafting/build.rs` and `crafting/src/consume.rs`, deliberately: a change to
either is one measured budget change. `LightYarn` and `LightYarnUnraveled` have
no producer here or upstream — a wheel makes dark yarn whichever wool went on —
and both are vendor stock, so nothing is broken; it is noted so a later pass does
not read it as a gap this engine opened. **A weapon is written down twice**, in
`state::weapon`'s table and in `protocol::items::is_classic_weapon`, because the
client needs the second one to draw a paperdoll it will not be refused; the two
are held together by a test that walks all 65,536 graphics, which is what caught
the display-art sweep's three new arts the moment only one side had them.
**`equip`'s typed branch asks a tag where its legacy branch asks the table**:
`has_tag(kind, Tool)` stands in for `harvest::tool_data_for_kind(kind).is_some()`
when deciding that a double-click should raise a harvest cursor instead of
wearing the thing. The two agree today only because the overlap — an item that is
both a weapon and a tool — is exactly the axes and the pickaxe, and the axes had
to be given the `tool` tag when they were registered for that reason alone.
Asking the table would need no tag. **A mapmaker's pen and a scribe's pen are one
kind here**, because ServUO tells its two classes apart only by the craft system
they open and this engine has no Cartography; the day it has one, `0x0FBF` needs
a tell that art cannot give it, and `shared_art` is not that tell — it removes
*both* kinds from the reverse lookup rather than choosing between them.

## The documents

**Design** — the model as built, no status in them:

- [`design_item_kind.md`](design_item_kind.md) — what an item kind and a material
  are, the closed set of selector forms, and the six boundaries that keep
  `Graphic` a projection instead of an identity.
- [`design_transactions.md`](design_transactions.md) — one meaning of atomic, the
  four exact projections that replace a world scan, twenty-one invariants, the
  prepare/commit types, and the property model that generates against them.
- [`design_crafting.md`](design_crafting.md) — where every piece of the crafting
  crate lives, the data model, the path from a double-click to a finished item,
  and what of a craft survives a restart.

**Evidence** — measurements and closed records; none of them is a status:

- [`evidence/2026-08-24-the-items-phase.md`](evidence/2026-08-24-the-items-phase.md)
  — the roadmap's own record of how an item became visible, liftable,
  containable, wearable and stackable, and the first item-loss bug.
- [`evidence/2026-08-24-the-crafting-phase.md`](evidence/2026-08-24-the-crafting-phase.md)
  — the port of ServUO's craft services: what the generator dropped and why, the
  three corners of the odds formula, and the chains that landed after it.
- [`evidence/2026-08-30-the-item-kind-migration.md`](evidence/2026-08-30-the-item-kind-migration.md)
  — the five stages of replacing `Graphic + Hue`, and the running account of
  which readers, recipes and save paths became typed.
- [`evidence/2026-08-31-the-transaction-stages.md`](evidence/2026-08-31-the-transaction-stages.md)
  — nine stages, the release measurements that chose every hard limit, and the
  one stage closed as deliberately not implemented.
- [`evidence/2026-09-02-the-cooking-slice-and-oven-deeds.md`](evidence/2026-09-02-the-cooking-slice-and-oven-deeds.md)
  — the seventh trade, and the machinery that turns a deed into an installed
  addon.
- [`evidence/2026-09-02-the-cloth-chain.md`](evidence/2026-09-02-the-cloth-chain.md)
  — six addons and no new machinery, the first timed thing in `items`, and a hue
  that survives four steps.
- [`evidence/2026-09-02-the-crafting-review.md`](evidence/2026-09-02-the-crafting-review.md)
  — twelve problems found by reading the crate against its upstream, and what
  closed each one. Four comments in `world/src/tick/` cite it by point number.
- [`evidence/2026-09-03-the-chains-head.md`](evidence/2026-09-03-the-chains-head.md)
  — a crop field as a spawn region for items, why none of it is saved, and the
  reach check that refuses every living thing.
- [`evidence/2026-09-03-the-inscription-trade.md`](evidence/2026-09-03-the-inscription-trade.md)
  — the eighth trade, the two mechanisms no other trade has, and the scroll
  rotation that had been teaching the wrong spell.
- [`evidence/2026-09-03-the-vendor-display-art-sweep.md`](evidence/2026-09-03-the-vendor-display-art-sweep.md)
  — the four oracles that tell a borrowed shop-window picture from a second
  facing, the fourteen lines that were the former, and why thirteen went away.
- [`evidence/2026-09-03-the-role-tables-close.md`](evidence/2026-09-03-the-role-tables-close.md)
  — the twenty-five definitions that finish the weapon, armour and tool tables,
  why adding one moves live items rather than only adding a row, and the axes
  that could not chop because of it.

**Plans** — what is not built lives outside `docs/`:

- [`plans/items/item_identity/PLAN.md`](../../plans/items/item_identity/PLAN.md)
  — growing the catalogue past the pilot and retiring the graphic-keyed
  adapters.
