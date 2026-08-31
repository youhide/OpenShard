# Item ownership, container indexes, and atomic crafting

Living implementation plan. This is the canonical execution record for making
item movement, splitting, merging, withdrawal, and crafting atomic at a tick
boundary, and for replacing whole-world container scans with an exact index.
Update this document in the same change that settles a decision or completes a
stage.

**Status:** A1–A5 and A2's access-boundary follow-up are built. Item ownership,
quantity changes, bounded recursive backpack stock, withdrawal, and successful
craft output now share prepare/commit doors and unchanged-state refusals. A4's
dense generated `CraftKey` projection gives selected-recipe checks ordered,
revisioned candidates without a request-time container walk. A6a's exact house
coverage, permissioned inventory backend, and client-owned Ctrl+I search window
are built. A6b is deliberately closed as
`SearchOnly`. A7's shared 492-recipe artifact, revision handshake, compact
context packet, client-side readiness, O(1) recipe lookup, and coalesced bounded
open lane are built. A8 now caps command and catalogue-open work, removes the
temporary whole-pack catalogue snapshot, and rejects corrupt restored stack
amounts before allocation. Release measurements, observability, the property
soak, and documentation cleanup remain open.

## Outcome

After this plan is complete:

- every item mutation is either rejected without changing gameplay state or
  commits a completely valid new state;
- stack split, merge, give, consume, and craft preserve quantities under every
  allocator and capacity failure;
- asking what a container holds costs the size of that container, not the size
  of the world;
- house contents are searchable through an indexed, permission-filtered QoL
  view; if direct storage crafting is later enabled, one craft may withdraw
  from explicitly opted-in recursive house storage as one deterministic
  transaction;
- neither opening the catalogue nor committing a craft performs an unbounded
  root/container/world scan inside the realtime tick;
- the client computes catalogue availability as presentation only, while the
  server performs the complete authoritative check at craft time; and
- model-based property tests exercise arbitrary mutation sequences and shrink
  every invariant failure to a reproducible case.

This work does **not** introduce a second world loop, a `Mutex<WorldState>`, or
full-world revisions. The tick remains the sole owner of mutable world state.

## Why this exists

`ItemLocation` is already the canonical ownership edge. `relocate_item`
validates a destination before replacing the edge and updates its `Position`,
`Contained`, `Equipped`, and cursor projections in one synchronous call. That
is a sound primitive, and the single-threaded tick means another command cannot
observe the middle of it.

Compound operations do not yet share the same guarantee:

- a partial stack lift creates a remainder and then reduces the original;
  serial exhaustion currently logs that the remainder was lost and the caller
  still reduces the original;
- a successful craft consumes ingredients before output placement, so output
  allocation may fail after payment;
- `give` deliberately reports partial progress when it runs out of serials;
- multi-line crafting checks ingredients first and consumes them line by line,
  but there is no first-class withdrawal plan proving that the commit cannot
  fail or overlap the same pile twice; and
- `contained_items` walks the complete `ItemLocation` column for every
  container query.

These are one problem: ownership and quantity mutations need explicit prepare
and commit phases, and prepare needs an indexed view of the candidate items.

## Settled boundaries

### One meaning of atomic

Atomicity is observed at a completed gameplay operation, not at individual ECS
component writes. For an operation returning a refusal or allocation error, the
following must be identical to the state before it began:

- every live entity and serial;
- canonical item locations and their projections;
- stack quantities and semantic identity;
- container membership;
- cursor ownership; and
- durable gameplay events.

Diagnostic logs may differ. A refusal packet may be queued. Successful partial
behaviour is allowed only where the domain explicitly asks for it and the
return type names it; it is never an accidental consequence of allocation
order.

### The world stays single-owner

No worker borrows or locks `WorldState`. Expensive presentation may later run
from an owned projection, but item validation and commit stay inside one tick.
This prevents races between a withdrawal and another command without adding a
world mutex.

### A bounded tick, not merely a faster average

Replacing a world scan with a recursive source scan is not sufficient. The
number of nearby roots and piles can still be adversarial, and several cheap
catalogue requests can still add up because the current tick drains its whole
inbox. Every realtime path therefore has an explicit work bound.

The limits are named domain constants selected by release-mode benchmarks:

- `MAX_CRAFT_SOURCE_ITEMS`: maximum indexed recursive subtree for one spatial
  source, enforced even for staff-created objects;
- `MAX_CRAFT_RESOURCE_LINES`: a build-time recipe-data assertion (the current
  492 recipes have at most four lines);
- `MAX_CRAFT_WITHDRAWALS`: maximum concrete piles one atomic craft may touch;
  and
- per-tick command/service work budgets, with untouched commands retained in
  FIFO order for a later tick.

An operation whose predicted cost cannot fit a fresh tick is refused before
prepare, or its game rule is explicitly batch-limited. It is never continued
halfway through a mutation. `use_all_res` in particular needs a documented
maximum batch/withdrawal count once nearby storage is admitted; "all piles in
an unbounded workshop" cannot be an atomic realtime operation.

### Recipes live on both ends; only the selected one is authoritative work

The full recipe catalogue is generated into both client and server artifacts
from the same source data and identified by a catalogue revision/hash. The
client may evaluate thousands of recipes locally. Opening the catalogue does
not iterate or serialize recipe rows on the server; it captures only a compact
resource/facility/skill context. A version mismatch is an explicit protocol
refusal or update path, never a reason to stream/re-evaluate all rows in a tick.

The client derives green/unavailable hints from its static catalogue and this
context. A Craft request sends a stable recipe id plus selections. The server
does an O(1) definition lookup and checks only that selected recipe against
current authoritative indexes. The client hint may be stale and has no
authority.

The protocol migration removes `CraftCatalogueRow::ready` once the client owns
the derived hint. Until both ends migrate together, the field may remain as a
compatibility value but the server must not perform a full catalogue-wide
inventory/facility scan to populate it.

### Pay recursion on mutation, not on a request

Craft lookup uses four exact projections:

```text
container serial -> candidate direct child EntityIds
(backpack/root, CraftKey) -> total + ordered pile serials
(root x, root y, access domain, CraftKey) -> total + ordered pile serials
(query x, query y, access domain, CraftKey) -> reachable total
```

Both spatial indexes live inside one `FacetState`, so `facet` is structural and
is deliberately absent from their keys. A global `(x, y)` index shared by all
facets would be incorrect; `FacetState.craft_stock[(x, y)]` is not. The grids
are sparse: every coordinate has the logical empty answer, but Felucca does not
allocate tens of millions of empty maps.

`CraftKey` is a catalogue-derived dense id for a semantic identity or an
audited legacy `(Graphic, Hue)` selector. A pile contributes to a constant
number of keys; overlapping keys are deduplicated by `WithdrawalPlan`.

A spatial pile contributes once, to the tile occupied by its eligible root.
A query at `(x, y)` reads the fixed
`(2 * CRAFT_SOURCE_RANGE + 1)^2` source-tile window around it. With range two
that is exactly 25 bucket lookups regardless of world size, nearby root count,
or unrelated item count. Facet and `x/y` participate; `z` deliberately does
not. The ordered pile collection must support non-linear removal (for example a
`BTreeSet<Serial>` or indexed slots), so removing one pile never scans all
piles on a crowded tile.

Direct membership makes changes cheap without copying each pile 25 times: an
amount change updates one total; adding/removing one pile changes one
membership. Moving a nested container or changing a policy boundary reprojects
its subtree once. Moving a root moves its already indexed subtree from one
source-tile bucket to another. `MAX_CRAFT_SOURCE_ITEMS` makes each such mutation
predictably bounded.

In parallel, the same amount delta updates the 25 `reachable total` cells from
which the root tile is in range. These cells contain only aggregated integers,
not 25 copies of every pile id. The client-context path reads one query-cell
aggregate. The authoritative selected-recipe path reads concrete piles from
the fixed 5x5 source-tile window. This keeps both request paths bounded while
avoiding the memory cost of duplicating withdrawal candidates.

The player's backpack has the same root stock but is non-spatial, so walking a
player never reindexes their inventory. Facilities use a parallel per-cell
fixed-size counter/bitmask index rather than a nearby item scan.

### House search first; automatic withdrawal is a policy gate

The required QoL feature is a read-only house inventory search. It uses the
same canonical containment/root/access projection but cannot consume or move an
item. Search results aggregate identity/amount by eligible root, are paginated,
and may be stale presentation; opening/highlighting a result revalidates the
house, standing, root, and item.

Direct crafting from boxes is a separate optional policy, not a consequence of
having the index. Ship with `SearchOnly` first. `OptInCraftStorage` may be
enabled later only after mutation cost, fragmentation UX, access rules, and
property tests are satisfactory. Disabling it removes spatial storage from
`WithdrawalPlan` while leaving house search and backpack crafting intact.

If `OptInCraftStorage` is enabled, a craft source set is represented, without
constructing it on request, by:

1. the player's non-spatial backpack stock, including eligible recursive
   descendants; and
2. the current house's permitted storage-domain bucket at the player's
   `(facet, x, y)`, already containing eligible recursive descendants of its
   nearby opted-in roots.

Root proximity is two-dimensional: a chest at the same `x/y` range but another
`z` counts, but distance alone never admits storage. Spatial ingredients are
available only when the player is standing inside a house, the root is inside
and locked down to that same house, the root is explicitly opted in as craft
storage, and the player's current `Standing` satisfies its configured access.
A loose/open chest, a chest outside every house, and a chest belonging to a
neighbouring house contribute nothing even when they are one tile away. Trade,
vendor, bank, corpse, and missing-house roots are always excluded.

The spatial access domain is therefore `HouseCraftStorage { house,
minimum_standing }`, not a generic public/account bucket. Ordinary nested
containers inherit the opted-in root domain. Backpack stock remains available
outside houses. A future public workshop would be a new explicit domain and
policy, not an accidental consequence of proximity.

Facilities follow the same boundary. While using house storage, a forge,
workbench, or other required facility must resolve to that same house; a
facility across a wall or inside the neighbouring house does not count. Outside
all houses, backpack-only crafting may retain explicitly public world
facilities under a separate policy.

Canonical `ItemLocation` rejects cycles. Index maintenance still carries a
visited set while reprojecting a subtree as defence against corrupt restored
state. Every pile is revalidated against canonical location, root, access
epoch, identity, and amount during authoritative prepare.

## Invariants

The implementation and its property model enforce all of these after every
generated action.

### Ownership

1. Every live item has exactly one canonical `ItemLocation`.
2. `Position`, `Contained`, `Equipped`, and cursor state exactly project that
   canonical edge and are mutually exclusive.
3. A container graph is acyclic and every contained parent is a live
   `Container`.
4. A container membership index may contain stale candidates, but it may never
   omit a live child. Reads revalidate every candidate against canonical
   `ItemLocation`, so stale entries cannot become ingredients or visible items.
5. A rejected establish/relocate/split/drop/equip operation leaves the logical
   item graph unchanged.
6. Every eligible pile contributes its exact amount and identity to the one
   source-tile bucket of its root under the correct access domain. Every query
   cell in range receives the same amount in its aggregate-only projection.
   Both indexes belong to the root's `FacetState`; query `z` cannot change the
   answer.
7. A current-epoch root/per-cell stock equals the eligible recursive transitive
   closure of direct container membership, without duplicates. A stale/rebuilding
   house projection is unavailable, never partially trusted.
8. A spatial source and its querying player resolve to the same live house; the
   root is locked down to and opted into that house, and the player's standing
   meets the domain threshold. Crossing a footprint boundary removes all
   spatial stock from the answer without changing backpack stock.

### Quantity and identity

9. A live stack has an effective amount in `1..=MAX_STACK`; zero is represented
   by no item, never by an `Amount(0)` component.
10. Split and merge conserve the sum of `(ItemKind, Material, compatible
   instance state)` quantities.
11. Split copies semantic and stack-compatible instance state without
   reinterpreting `Drawn`.
12. An incompatible identity or per-instance fact never disappears through a
   merge.
13. Mint and burn are the only operations allowed to change total quantity,
    and their return/event records name the exact delta.

### Transactions

14. A withdrawal is all-or-nothing across every ingredient line and source
    container.
15. One physical pile may appear at most once in a withdrawal plan; overlapping
    selectors reserve from its remaining planned amount rather than counting it
    twice.
16. Plan order is deterministic: source class/access domain, root serial, then
    item serial. Replay does not depend on hash-map iteration order.
17. A successful craft commits ingredients, output, tool wear, training, and
    `ItemCrafted` as one operation. If output preparation fails, none of them is
    committed.
18. A failure after `prepare` is an invariant violation, not an ordinary branch
    halfway through `commit`.
19. Catalogue-open tick work is bounded by the context shape; recipe-wide
    evaluation happens only on the client, and optional context encoding happens
    outside the realtime loop.
20. A craft prepare/commit touches no more than the configured resource-line
    and withdrawal limits. Over-budget input is rejected without mutation.
21. Index maintenance has a computable work-unit cost before mutation and never
    admits a spatial craft-source subtree above its hard item ceiling.

## Target representation

### `ContainedItems`: an ownership projection

Add a `ContainedItems { candidates: Vec<EntityId> }` component to container
entities, parallel to the existing `Worn` projection on mobiles.

`apply_projection` maintains both sides:

- leaving `Contained(old)` removes the item from `old` when practical;
- entering `Contained(new)` inserts it in `new` exactly once;
- establishing a contained item performs the same insertion; and
- `despawn_item` removes or permits a stale candidate, but never leaves a live
  unindexed child.

`contained_items(state, serial)` reads only the named container's candidate
vector, then rechecks that each candidate is live and canonically contained by
that serial. This follows `Worn`'s safe asymmetry: stale entries cost a lookup;
missing entries are correctness defects.

Extend `audit_item_graph` with:

- `UnindexedContainedItem` for a live child absent from its parent index;
- `DuplicateContainedCandidate` for duplicate live candidates; and
- optionally a stale-candidate count for diagnostics, not a graph violation.

Restore continues through `establish_item_location`, so the index is rebuilt
from canonical persisted edges rather than persisted separately.

### `HouseCoverage`: the spatial trust boundary

Add a private sparse `Tile -> house EntityId/Serial` coverage index inside each
`FacetState`. House placement/restore/design replacement/removal maintain it
through the same footprint block/unblock doors. The existing `house_at` scan can
then become an indexed lookup with canonical revalidation.

Craft resolves the player's house once from this index. No house means
backpack-only crafting. A spatial root is indexed only when its root tile
resolves to the same house named by its `LockedDown` component and it carries an
explicit `CraftStorage` opt-in with a minimum standing. Query-cell totals remain
keyed by house even for cells just outside its footprint; those rows are
unobservable because a player outside cannot resolve that house. This avoids a
per-cell rewrite merely because the query boundary moved.

A design replacement/removal can invalidate many formerly legal roots and must
not synchronously walk an unbounded house inventory. Increment a per-house craft
epoch and mark that house's spatial projection unavailable immediately; old
rows no longer match the current epoch and cannot be consumed. Rebuild the
derived projection incrementally under the tick work budget, replaying/restarting
on concurrent source mutations. Until it reaches the current epoch, crafting
from that house's boxes is safely unavailable while backpack crafting remains
usable. This is a false-negative degradation, never consumption from stale
foreign storage.

### `HouseInventoryIndex`: the reusable QoL substrate

Maintain a sparse house-wide index independent of crafting mode:

```text
(house, access threshold, ItemIdentity) -> total + ordered root/pile refs
```

`ItemIdentity` is the semantic `ItemKindId + MaterialId` where known and the
audited legacy identity otherwise. The client resolves text/category filters
against its static item catalogue and sends a bounded selector request; the
server returns permission-filtered aggregate/root rows with pagination. This
makes search cost depend on selected identities and page size, not every item in
the house. Custom per-instance names can be added as a separate secondary index
rather than contaminating crafting identity.

The index includes recursive descendants of same-house storage roots and is
maintained by the same containment/amount doors. Search never turns its result
into mutation authority: opening, highlighting, or later withdrawing a result
revalidates canonical state.

### Optional `CraftStockIndex`: exact source piles plus per-point totals

When `OptInCraftStorage` is supported, add two sparse indexes to each
`FacetState`. The source index key includes the
root's `Tile { x, y }`, access domain, and dense `CraftKey`; its value is an
exact total and ordered pile candidates:

```text
(1200, 845), House(17, Friend), IronIngot
    -> total 480, piles [0x40001234, 0x40005678]
```

The reachable index is keyed by the player's/query `Tile { x, y }`, access
domain, and `CraftKey`; it contains totals only:

```text
(1201, 846), House(17, Friend), IronIngot -> total 480
```

A pile under an eligible root at `(x, y, any_z)` is projected once into that
root's source bucket. Its amount is also added to the 25 reachable-total cells
in range; no pile id is copied there. Amount changes apply integer deltas.
Identity, membership, root movement, access changes, and despawn remove the old
contribution before adding the new one. Missing contributions are correctness
violations; stale pile candidates are ignored by revalidation and repaired
through the normal mutation/rebuild door.

Both indexes live privately beside `Sectors` inside `FacetState` and are fed by
the same location and amount transitions. They must not become a second authority:
`ItemLocation`, `Position + Facet`, identity components, amount, and access
facts remain canonical. A slow test oracle derives expected totals and pile
membership directly from them.

Each pile candidate records enough projection metadata to locate/revalidate its
root and access epoch without rediscovering them by a recursive request-time
walk. The player's worn backpack owns a non-spatial root stock of the same
shape and is merged explicitly by catalogue/craft queries.

### Recursive projection maintenance

Recursive traversal exists on mutation/rebuild paths, not catalogue/craft
request paths. It uses indexed direct children, deterministic serial order, a
visited set, and canonical-edge revalidation. Ordinary descendants inherit the
root domain; a policy boundary reprojects or excludes that branch.

For a one-pile amount change, the cached root/domain makes maintenance constant
apart from the fixed 25 cells. Moving a contained container or changing a
branch policy enumerates that subtree once. The cached recursive item count is
checked before mutation against `MAX_CRAFT_SOURCE_ITEMS`; a spatial source over
the ceiling is not indexed as usable crafting storage, even when staff bypass
ordinary `MAX_ITEMS` capacity rules.

### Craft context for client-side availability

The client cannot infer the contents of a closed nearby chest from ordinary
world packets. `OpenCraftCatalogue` therefore captures a compact, owned
snapshot from the one reachable-total cell at the player's `x/y` plus the
backpack:

```text
CraftMenuSnapshot {
    request_id,
    catalogue_revision,
    craft_projection_revision,
    backpack_revision,
    skills,
    facilities,
    amounts: [u32; CRAFT_KEY_COUNT],
}
```

Capturing it does not loop over recipes, source tiles, roots, containers, or
piles. It scales with nonzero `CraftKey`s in the one cell, not recipe count. A
bounded service channel may own context encoding; any transitional server-side
`ready` compatibility evaluation lives there too, never in the tick. Catalogue
opens from the same connection are coalesced and a full channel applies
backpressure instead of blocking the tick. A result returns with `request_id`;
the tick drops it if the connection/request is no longer current.

The client combines the context with the cached static catalogue to derive
availability. The two stock revisions are local diagnostic/cache identities,
not a world revision or authority token. Movement or container changes may make
the hint stale; the next open/refresh replaces it, and Craft always reads
current authoritative pile candidates.

### Prepared mutations

Compound item operations have two explicit stages:

```rust
let prepared = operation.prepare(&state)?; // no gameplay mutation
operation.commit(&mut state, prepared);    // cannot ordinarily fail
```

The concrete types should be small domain values rather than a generic ECS
transaction log:

- `PreparedSplit` names original, origin, taken amount, remainder amount, and
  the fully allocated remainder entity;
- `WithdrawalPlan` owns ordered `Withdrawal { item, source, expected,
  take }` rows;
- `PlacementPlan` names existing piles to fill and any newly allocated output
  entities; and
- `PreparedCraft` owns both plans plus the already-resolved output identity and
  result amount.

New result/remainder entities may be allocated during prepare because no other
command can observe them. They carry no location and emit no packet/event until
commit. If prepare fails, every temporary entity is despawned before returning.
`audit_item_graph` runs only at operation boundaries. This avoids adding a
second serial-reservation subsystem while still proving that commit cannot run
out of serials.

### Withdrawal plan

The planner resolves each recipe selector to one or more dense `CraftKey`s and
reads only the backpack/cell stock buckets for the player's allowed access
domains. Current data has at most four recipe lines; the data build rejects any
future row above `MAX_CRAFT_RESOURCE_LINES` until the budget is deliberately
raised and remeasured. Candidate piles are already ordered by serial.

The planner maintains a temporary remaining amount per candidate item. For
each recipe line it selects from that remaining amount, producing exact item
withdrawals and stopping at `MAX_CRAFT_WITHDRAWALS`. If a line cannot be
satisfied inside the bound, it returns a distinct too-fragmented/too-large
refusal and no mutation occurs. `use_all_res` uses a separately bounded batch,
not every matching pile in arbitrary nearby storage.

Commit only performs these pre-proved operations:

- decrement a stack to a positive amount; or
- remove a fully consumed item through `despawn_item` and update its container
  viewers/index.

The planner is reusable by crafting, spell reagents, quest turn-ins, vendor
payment, and any future multi-container payment. Policy decides which sources
are eligible; the transaction machinery only conserves quantities.

## Property-test strategy

Add `proptest.workspace = true` as a dev-dependency to the owning crates rather
than creating a separate test crate. Keep the slow reference model test-only.

### Reference model

The model is a set of small maps, independent of ECS implementation:

```text
ModelItemId -> { serial, location, identity, amount, stack facts }
ModelContainerId -> ordered child ids
ConnectionId -> optional held item
```

It implements the same public actions with deliberately simple full scans. The
system under test uses the real registry, location projections, and index. After
each action compare normalized logical states, not entity allocation order or
packet byte order.

### Generated actions

Generate state-machine sequences containing valid and invalid forms of:

- spawn item/container;
- establish ground/contained/equipped location;
- relocate between ground, containers, paperdoll, and cursor;
- attempt self/nested container cycles;
- lift whole stack and lift a partial stack;
- bounce/drop to ground/drop into container;
- merge compatible and incompatible stacks, including `MAX_STACK` overflow;
- consume none/part/all/more-than-all;
- give amounts spanning zero, one, `MAX_STACK`, and several piles;
- despawn an item or a container subtree;
- move root containers across craft-source cells and across facets, with
  unrelated `z` changes;
- enter/leave a house footprint; place equally near own-house, foreign-house,
  loose, locked-down, and explicitly opted-in roots; change standing and house
  design epochs;
- change pile amounts/identity and verify fixed-cell deltas without a query-time
  recursive rebuild;
- create recursive container trees, explicit locked descendants, and corrupt
  cycle fixtures for reader defence;
- cross `MAX_CRAFT_SOURCE_ITEMS`, `MAX_CRAFT_WITHDRAWALS`, and service-queue
  boundaries on both sides;
- prepare/commit withdrawals with duplicate and overlapping selectors; and
- craft into an existing pile or one/many new output piles.

Use short stable ids in generated actions and resolve them through the model so
shrinking does not mostly produce stale opaque `EntityId`s.

### Failure injection

Every allocating action is exercised with the item serial watermark positioned
at and immediately below `ITEM_MAX`. Binding a high saved serial already moves
the real allocator watermark, so tests can deterministically obtain:

- failure before the first allocation;
- failure after exactly one of several allocations; and
- success at the final available serial.

For every injected prepare failure, compare a normalized before/after snapshot
and require equality. Pin each discovered minimal counterexample in ordinary
regression tests as well as the generated suite.

### Required properties

1. `audit_item_graph(state).is_empty()` after every successful or refused
   action.
2. Indexed `contained_items` equals a test-only slow scan of canonical
   `ItemLocation` for every container after every action.
3. Source-tile totals/pile sets and per-query-cell reachable totals both equal a
   slow two-dimensional recursive oracle inside every `FacetState`/access
   domain; varying only `z` preserves the result.
4. Root stocks and mutation-time recursive projection equal a slow visited-set
   traversal of canonical locations and access policy, including locked
   descendants.
5. The real state and reference model normalize to the same ownership graph,
   identities, amounts, and cursor state.
6. Split/merge conserve quantity and semantic identity for all amounts around
   `1`, `MAX_STACK`, and `u16::MAX` input boundaries.
7. Any refused operation is logically idempotent.
8. Withdrawal either applies its complete requested vector or applies nothing.
9. No item is reserved twice by overlapping withdrawal lines or recursive
   paths.
10. A failed craft placement consumes nothing and emits no `ItemCrafted`.
11. `CraftMenuSnapshot` totals equal the exact reachable-total cell plus
    backpack stock and never include a denied branch; capture visits no source
    tiles, roots, or piles.
12. Save/restore rebuilds indexes whose answers equal the pre-save answers.
13. Replaying the same generated action sequence from the same seed produces
    the same normalized state and domain events.
14. Every admitted realtime craft/index operation reports a work-unit estimate
    within its hard bound; every over-bound operation is idempotently refused.
15. A saturated menu-service channel neither blocks nor mutates the world, and
    stale/out-of-order worker results are discarded by request id.
16. Moving only the player across a house boundary changes spatial availability
    to the new house or none; it can never retain the previous house's totals or
    pile candidates.
17. Invalidating a house epoch immediately changes spatial availability to
    unavailable, and every incremental rebuild prefix remains non-authoritative.

Start CI suites at 256 sequences of 1–128 actions per property. Keep a separate
ignored/explicit soak configuration of at least 10,000 sequences and up to 512
actions for local pre-merge runs. Commit proptest regression seeds.

Property tests do not replace named scenario tests. Packets, sound, open-gump
updates, exact refusal reasons, secure trade ownership, and persistence wire
records remain ordinary example tests because the model intentionally excludes
presentation.

## Implementation stages

### A0 — characterize and expose failures

- [x] Add named ground/container serial-exhaustion regressions; after A1 they
      pin the corrected unchanged-state refusal rather than the former loss.
- [x] Add a named craft test proving output allocation failure currently spends
      ingredients, then mark its target expectation for A5.
- [x] Catalogue direct production writes to `ItemLocation`, `Contained`,
      `Equipped`, `Amount`, and raw `Registry::despawn` for item entities.
- [x] Add normalized test snapshots for ownership, quantities, identity,
      cursors, and domain events.
- [ ] Add release-mode microbenchmarks for source-tile projection, a 125-item
      root move/access change, fixed-window withdrawal preparation/commit,
      snapshot capture, and simultaneous catalogue opens.
- [ ] Select and document `MAX_CRAFT_SOURCE_ITEMS`,
      `MAX_CRAFT_RESOURCE_LINES`, `MAX_CRAFT_WITHDRAWALS`, service-channel
      capacity, and per-tick work budgets from those measurements.

Done when the two known loss paths fail for the intended reason and the
mutation inventory assigns every direct write to a later stage.

The 2026-08-31 mutation inventory found:

- `ItemLocation`, `Contained`, and `Equipped` production writes are already
  confined to `state::item_location`; restore uses
  `establish_item_location`, so A2's index rebuilds from canonical edges;
- bare `Amount` writes remain in item constructors plus `items::{drag, stack,
  containers}` and `skills::handlers::poison`; A3 now routes their live
  mutations through `items::stack`, leaving only construction/restore writes;
- the initial raw-despawn survey found located items in boats, housing decay,
  and world decor/fields/gates/spawners/death; A2 now routes those through
  `despawn_item`. The raw calls left are mobile destruction or rollback of an
  entity that failed before receiving `Drawn`/`ItemLocation`.

The named craft allocation regression exhausts the item serial pool only after
the tool, ingredients, and guaranteed-success skill are established. A5 has
flipped its former loss expectation: ingredients, output, tool, skill, ownership,
identity, cursor state, and `ItemCrafted` events now remain unchanged when output
allocation fails. A companion full-backpack regression pins the capacity
failure to the same normalized snapshot.

### A1 — make split allocation-first

- [x] Change ground and contained remainder construction to return a prepared
      result/error; never log-and-continue after allocation failure.
- [x] Validate the lift/cursor transition before allocating the remainder.
- [x] On failure, reject the lift with original entity, amount, location,
      visibility, and cursor unchanged.
- [x] Commit remainder, reduced original, and held location without a fallible
      step between them.
- [x] Cover typed/legacy identity and both ground/container origins.

**Built.** Serial exhaustion cannot change total quantity or ownership, and
named regressions cover both origins plus typed and legacy identities. A3's
generated conservation suite now ranges partial lifts across every valid pile
size and split point.

### A2 — add the exact container membership index

- [x] Add `ContainedItems` beside `Worn` and maintain it in the canonical
      location projection.
- [x] Rewrite `contained_items` to use the index with canonical revalidation.
- [x] Extend `audit_item_graph` with missing/duplicate membership checks.
- [x] Route restore and all production location writes through
      `establish_item_location`/`relocate_item`.
- [x] Migrate item destruction sites to `despawn_item` where they currently
      bypass cursor/container cleanup, leaving non-item despawns alone.
- [x] Add the indexed-vs-slow-oracle state-machine property.
- [x] Add policy-free deterministic recursive traversal with canonical edge
      revalidation and cycle defence.
- [x] Retire the legacy `items::contents_index` whole-world snapshot;
      container-specific reads use `ContainedItems`, while tick-wide player
      passes remain explicit budgeted A8 operations.
- [x] Add inherited root access and explicit descendant deny stops when the A6
      house-storage policy types make those meanings concrete.

Done when no container read scans the world and the index agrees with the slow
oracle after arbitrary mutation sequences, recursive traversal, and save/restore.

### A3 — centralize quantity mutation

- [x] Route production `Amount` changes through named stack operations rather
      than bare registry writes.
- [x] Make zero, singleton normalization, cap enforcement, redraw, and index
      notifications explicit responsibilities of those operations.
- [x] Preserve deliberately partial `GiveOutcome`, but require callers either
      handle it or use a new all-or-nothing prepared give.
- [x] Add conservation/model properties for split, merge, give, remove, and
      consume.

Done when a search for direct production `Amount` writes yields only constructors
and the named quantity module, with each exception documented.

**Built.** `items::stack` owns positive/capped writes, singleton
normalisation, fixed-cap filling, bounded taking, and location-derived ground or
container redraw. Prepared split commit has one narrower unpublished door so it
cannot re-add the lifted serial to the old gump before the cursor relocation.
The intentionally partial `GiveOutcome` remains `must_use`, and every production
caller reads completion or the exact amount delivered. Six properties run 256
cases each over the boundaries at one, `MAX_STACK`, and refused values through
`u16::MAX`; named allocation-exhaustion regressions remain beside them.

The full world suite exposed one stale assertion in the shipped healing-potion
test: it equated an absent sparse `Amount` component with deletion after A3 made
singletons canonical component absence. The regression now asks `amount_of == 1`
and separately asserts that the entity remains live.

The remaining direct production `Amount` writes are constructors: persistence
restore in `crates/server/world/src/tick/persist.rs` and test fixtures. Restore
currently trusts a saved stack amount above `MAX_STACK`; validating and refusing
that external record before entity allocation is recorded in A8 rather than
turning the in-memory mutation door into a panic on I/O.

### A4 — introduce recursive `WithdrawalPlan`

- [x] Define dense catalogue-derived `CraftKey`s for semantic and audited legacy
      matching, with a constant maximum number of keys per pile.
- [x] Implement deterministic preparation against ordered root/cell stock
      buckets, retaining each pile's root/domain/revision facts.
- [x] Prevent double reservation across duplicate/overlapping ingredient lines.
- [x] Enforce resource-line, withdrawal, and `use_all_res` batch limits before
      mutation, with an explicit fragmentation/complexity refusal.
- [x] Make commit infallible over the state preparation just validated.
- [x] Migrate crafting material consumption first; then evaluate reagents,
      quest turn-ins, and vendor payment as separate changes.
- [x] Add all-or-nothing and overlap property suites.

Done when a multi-line craft can never consume a strict subset of its planned
ingredients.

**Built.** Protocol's build artifact assigns dense keys to every semantic and
audited legacy selector used by the 492 recipes and asserts that one physical
pile contributes to at most four keys. Each backpack root owns totals and
serial-ordered piles for those keys plus a monotonic projection revision.
Recursive projection is paid on canonical location/identity/amount mutation,
refuses roots above 125 items, and never walks the root during a craft request.
Preparation reads only the selected recipe's key buckets, revalidates each
candidate's canonical root, identity, and amount, and commit asserts the source
revision before its infallible quantity changes. Named nested-move and limit
tests plus a 256-case slow recursive oracle cover the index.

### A5 — make successful crafting transactional

- [x] Resolve output identity, amount, capacity, merge targets, and required new
      entities before consuming ingredients.
- [x] Build a `PreparedCraft` containing withdrawal and placement plans.
- [x] Ensure every prepare failure removes temporary entities and leaves tool,
      skills, ingredients, output piles, and events unchanged.
- [x] Commit output, withdrawal, training/tool wear, messages, and
      `ItemCrafted` only after all fallible preparation succeeds.
- [x] Decide and document failure-craft material loss separately: it is a game
      rule, but its half/all withdrawal still uses the same atomic plan.

Done when output serial/capacity failure consumes nothing and successful craft
commit has no ordinary error return.

**Built.** `WithdrawalPlan` reserves each physical pile once in serial order and
folds overlapping recipe lines into that row. The generated recipe build and
runtime share a four-line ceiling; one craft touches at most 125 piles and one
`use_all_res` batch cannot exceed one representable physical stack. These are
conservative pre-benchmark limits and A0 must still measure them before A4's
indexed source replaces the current direct backpack candidate read.

`PreparedPlacement` records every existing-pile fill and allocates every new
unlocated output entity before publication. Capacity and allocator refusals
therefore leave the normalized ownership/quantity/identity/cursor/skill/tool
snapshot unchanged and emit no `ItemCrafted`; named regressions cover both
failure kinds and allocation after a planned existing-pile fill. A 256-case
overlap property varies two lines against one pile and proves the second line
either joins the first reservation or leaves the complete snapshot untouched.
Ordinary
failed crafts still lose all required material, while failed `use_all_res`
batches lose half of one craft's material as before, but both now prepare one
atomic withdrawal before committing it.

### A6a — index and search house inventory without consuming it

- [x] Add exact per-`FacetState` house coverage and replace the current
      whole-house `house_at` scan with indexed lookup plus canonical
      revalidation.
- [x] Define eligible house inventory roots: same-house `LockedDown` storage,
      filtered by standing; exclude loose, foreign-house, outside, trade,
      vendor, bank, and corpse roots.
- [x] Add `HouseInventoryIndex` by house/access/identity with recursive totals
      and ordered root/pile refs, maintained by canonical location/amount doors.
- [x] Add bounded selector search and paginated aggregate/root results; resolve
      ordinary text/category matching on the client from its static item
      catalogue.
- [x] Revalidate house, standing, root, and item before opening/highlighting a
      search result. Search results grant no mutation authority.
- [x] Give each house inventory projection an epoch. Footprint/design/removal
      invalidates it in O(1), makes search temporarily unavailable, and
      schedules a budgeted rebuild; stale epochs never leak foreign contents.
- [x] Add search/index properties against slow recursive oracles, including
      nested storage, permissions, pagination, adjacent houses, footprint
      transitions, and stale/rebuilding epochs.

Done when a player can quickly find eligible items in their current house and
the feature has no path that consumes, moves, or exposes another house's items.

**Coverage built.** Each facet now owns a private sparse map from every drawn
house tile—including floors and doorway gaps—to the covering entity candidates.
The same `block`/`unblock` doors maintain obstruction and coverage during
placement, restore, redesign, and demolition. `house_at` reads only the queried
tile and revalidates entity lifetime, serial, facet, position, and current
classic/custom shape before returning it; a stale derived row therefore grants
nothing. Overlap retains multiple candidates for the staff-placement exception,
and removal only removes the matching house.

**Inventory backend built.** A sparse house/access/semantic-or-legacy identity
projection holds aggregate totals plus serial-ordered root and pile references.
Only same-house ground `LockedDown` roots enter it; secure access becomes the
row threshold, plain lockdowns require co-owner standing, and loose, foreign,
outside, held/equipped (therefore vendor/bank/trade), trade-window, and corpse
branches are excluded. Canonical location, amount, identity, lockdown, house
shape, and removal doors invalidate the affected house. An epoch mismatch makes
both search and old result resolution unavailable immediately; the tick rebuilds
at most 256 root/item work units and publishes only a complete current epoch.

The shared build artifact generates material-aware item names, categories,
presentations, and exact semantic identities for the client. Ctrl+I opens a
local filter/selector window; an audited `0xGRAPHIC:0xHUE` form covers legacy
items without teaching the server text matching. The server accepts at most 32
exact catalogue identities and returns at most 50
serial-ordered root rows per cursor page without scanning unrelated identities.
Opening/highlighting revalidates the actor's current house and standing, the
root's lockdown and ground coverage, the complete containment path, item
identity, and epoch before passing the root through the ordinary checked
container-open door. Wire round trips pin the bounded request/page forms and
the client keeps pagination presentation-only. A 256-case slow-oracle property varies nested/loose/foreign/
outside/trade/corpse roots, legacy and semantic identities, amounts, and access;
named tests cover pagination, adjacent houses, release, partial rebuilds, and
design-footprint transitions.

### A6b — optional direct crafting from opted-in house storage

- [x] Keep the default mode `SearchOnly`; make `OptInCraftStorage` a separate
      feature/config decision with documented UX and performance evidence.
- [ ] Define `CraftSourcePolicy`: backpack everywhere; spatial storage only
      inside the player's current house and only from same-house roots carrying
      explicit `CraftStorage { minimum_standing }`.
- [ ] Add non-spatial per-root stock and sparse `CraftStockIndex` buckets keyed
      by `root x/root y/house/minimum standing/CraftKey` inside each
      `FacetState`, deliberately ignoring `z`.
- [ ] Project each eligible recursive pile once into its positioned root's tile;
      maintain exact totals and ordered pile refs on amount, identity, location,
      root movement, access change, and despawn.
- [ ] Maintain a second sparse
      `(query x, query y, house, minimum standing, CraftKey) -> total`
      projection: apply each source delta to the fixed 25 cells in range, but
      never copy pile refs into it.
- [ ] Cache root/domain/count metadata so a one-pile delta is fixed work and a
      subtree change is enumerated once, never rediscovered by a request.
- [ ] Enforce `MAX_CRAFT_SOURCE_ITEMS` on craft-source projection even for staff
      bypass paths; estimate the complete projection cost before mutation.
- [ ] Reuse the house projection epoch: stale/rebuilding storage is unavailable
      to `WithdrawalPlan`, never partially trusted.
- [ ] Add a per-source-tile facility counter/bitmask index scoped by house;
      same-house crafting cannot see a neighbouring facility. Keep any public
      outside-house facility policy explicit and separate.
- [ ] Feed backpack stock plus the 25 permitted source-tile/domain buckets to
      the same ordered withdrawal planner without enumerating roots or
      containers.
- [ ] Send a clear refusal when materials exist only in an ineligible box.
- [ ] Add stock-index and recursive-projection properties against slow oracles,
      including a player outside all houses, adjacent/foreign houses, footprint
      edges, standing changes, stale/rebuilding house epochs, and equal `x/y`
      with very different `z`.

Done only if the feature is accepted: one craft atomically consumes across
explicitly opted-in recursive house storage without traversing it on request,
index cost is independent of unrelated world items, and no ineligible branch
can contribute an item. If the decision remains `SearchOnly`, this stage is
closed as deliberately not implemented rather than blocking house search.

**Closed as `SearchOnly`.** House inventory search is useful without granting a
new mutation authority. Direct house crafting would add a second access-policy
surface, fixed-cell stock projections, facility scoping, rebuild semantics, and
fragmentation UX before the product has evidence that players want automatic
consumption from explicitly marked boxes. The shipped/default policy therefore
remains backpack-only crafting; no `CraftStorage` component, source/reachable
grid, or ineligible-box refusal exists to imply otherwise. The unchecked items
above are the rejected `OptInCraftStorage` design, retained as the acceptance
checklist for a future separately approved feature rather than partially built
runtime behavior.

A6a's root standing is inherited by ordinary descendants. A nested
`LockedDown` item is an explicit policy boundary: projection and result
resolution both stop before it, even for an owner. A named regression now pins
both the inherited-access case and that deny stop, closing A2 without inventing
the declined craft-storage policy type.

### A7 — make catalogue availability client-owned

- [x] Stop deriving every catalogue row's readiness from live server inventory
      and facilities.
- [x] Generate version-identical client/server recipe artifacts and add a
      catalogue revision/hash handshake; never send/evaluate all recipe rows on
      catalogue open.
- [x] Capture a compact owned `CraftMenuSnapshot` from the player's one
      reachable-total cell plus backpack totals, skills, and facilities, with
      no recipe/source-tile/root/pile loop.
- [x] Add a separately bounded catalogue-open lane that coalesces opens per
      connection and emits request-id-tagged compact contexts. Because capture
      is a fixed indexed read and row materialization is client-only, no worker
      result exists to race or become stale inside the tick.
- [x] Give the client that context to evaluate all local recipe rows; a Craft
      request carries one stable recipe id and the server looks up/checks only
      that row.
- [x] Remove or version-gate `CraftCatalogueRow::ready` across protocol, server,
      and client together.
- [x] Refresh context on catalogue open/explicit refresh; tolerate later
      staleness because the server always replans Craft authoritatively.
- [x] Retain an end-to-end test proving an invented green state cannot bypass
      authoritative server preparation.

Done when opening the catalogue performs no world-sized read and a stale or
malicious client still cannot spend unavailable ingredients; queue saturation
cannot block the realtime loop.

**Built.** `protocol/build.rs` consumes the same crafting, item, and material
JSON as the server and generates 492 stable presentation rows, recipe locations,
dense stock selectors, skill ids, and a content hash. An open packet contains
only revisions, facilities, relevant skills, and dense backpack totals; the
decoder rejects a mismatched artifact and materializes readiness locally. The
server no longer loops recipes or serializes rows and maps a selected flat id to
`(system, recipe)` in O(1). At most 32 coalesced opens are admitted per tick.
This synchronous bounded lane is intentionally simpler than the originally
proposed worker channel: capture already performs fixed indexed reads, so a
worker would add request/result ordering without removing any world-sized work.
The authoritative craft path still replans the selected recipe, and the forged
catalogue-reply regression remains green.

### A8 — measurement and cleanup

- [x] Split/limit inbox draining by deterministic work units while preserving
      FIFO order inside the gameplay and coalesced catalogue-service lanes;
      reserve the realtime simulation budget and bound service work separately.
- [x] Never suspend inside a gameplay mutation: defer the whole command before
      prepare when its predicted cost does not fit the remaining budget.
- [ ] Add counters/timers for index projection, snapshot/service queue latency,
      withdrawal planning, output preparation, commit, deferrals, and
      over-budget refusals.
- [ ] Benchmark index updates and craft requests against unrelated world size,
      local source density, fragmentation, and simultaneous clients; assert the
      request path stays inside its declared work bounds.
- [ ] Run the 10,000-sequence property soak and commit all regression seeds.
- [x] Remove the temporary catalogue `MaterialStock` backpack snapshot once
      server readiness is gone.
- [x] Validate restored stack amounts before entity allocation in
      `crates/server/world/src/tick/persist.rs`; zero or above-`MAX_STACK` saved
      piles are corrupt external records and must be refused, never clamped or
      allowed to reach the in-memory quantity door.
- [ ] Update `docs/item_kind.md`, architecture notes, and gameplay backlog with
      the settled ownership/index/transaction contracts.

Done when every individual operation and every tick admits only measured bounded
work, the property soak is clean, and no retired full-world or recursive request
scan remains on the craft path.

## Required verification per stage

Every implementation stage runs, in proportion to what it touches:

```text
cargo fmt --all -- --check
cargo check -p openshard-state -p openshard-items -p openshard-crafting -p openshard-world
cargo test -p openshard-entities
cargo test -p openshard-state item_location
cargo test -p openshard-items
cargo test -p openshard-crafting
cargo test -p openshard-world tick::crafting_tests
cargo test -p openshard-world tick::tests::<affected item filters>
```

The final stage runs the full affected workspace suites and the explicit
property soak. Performance claims must name debug/release mode, fixture world
size, container size, and before/after timings; a smaller synthetic test alone
does not close A8.

## Explicitly deferred

- A second mutable world loop or shared world mutex.
- Full-world copy-on-write revisions.
- Persisting the container index; it is rebuilt from canonical locations.
- Letting a worker read live ECS/world state or treating its stale menu result as
  craft authority.
- Any request-time recursive scan of nearby source trees.
- Treating client availability as permission to craft.
