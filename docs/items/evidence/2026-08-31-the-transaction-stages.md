# Making item movement atomic: nine stages, and what each one proved

> **This is a record.** It is the staged plan that lived in
> `docs/item_transactions_plan.md`, kept as it was written; the work it tracked
> is complete. The model it produced is
> [`../design_transactions.md`](../design_transactions.md) — where the two
> differ, the design is right — and what is still open is ranked in
> [`../README.md`](../README.md), which is the only status page for this domain.
> `docs/housing.md` cites stage A6a, the house inventory search, by name.

**Status: complete (2026-08-31).** A0–A5, A6a, A7, A8, and A2's access-boundary
follow-up are built and verified. Item ownership, quantity changes, bounded
recursive backpack stock, withdrawal, and successful craft output share
prepare/commit doors and unchanged-state refusals. House inventory search is
permissioned, paginated, and client-owned. Catalogue availability uses the
shared 492-recipe artifact and a compact bounded context lane. A6b is
deliberately closed as `SearchOnly`; its unchecked list is the acceptance
contract for a future separately approved `OptInCraftStorage`, not unfinished
runtime behavior. Release measurements, structured observability, the
10,000-sequence soak, and documentation are complete.

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
- [x] Add release-mode microbenchmarks for source-tile projection, a 125-item
      root move/access change, fixed-window withdrawal preparation/commit,
      snapshot capture, and simultaneous catalogue opens.
- [x] Select and document `MAX_CRAFT_SOURCE_ITEMS`,
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

#### A0 measurement record (2026-08-31)

The explicit ignored benchmarks were run in release mode on an AMD Ryzen 9
7900X (12 cores/24 threads, x86-64). The stock fixture was one backpack root
with 125 one-unit items. Rebuilding that root took 96.3 microseconds per
operation with no unrelated entities and 95.5 microseconds with 50,000
unrelated registry entities; reading its dense stock snapshot took 27.5
nanoseconds. The equal timings are the important result: the mutation cost is
local to the bounded root rather than the world.

The transaction fixture compared one 125-unit pile with 125 one-unit piles.
Withdrawal preparation took 0.2 microseconds per operation for the dense pile
and 14.3 microseconds at maximum fragmentation. Committing all 125 withdrawals
took 68.6 microseconds. Before intermediate stock notifications were folded
into one final projection rebuild, that same commit took 10,568.5 microseconds;
the batching door made it about 154 times faster. A tick serving 32 distinct
simultaneous catalogue opens took 105.1 microseconds total (about 3.3
microseconds per open), including compact context construction and command-lane
handling.

A6b was declined as `SearchOnly`, so there is no shipped source-tile/access
projection to benchmark. Its planned benchmark is therefore not silently
claimed: the shipped non-spatial backpack-root projection is measured instead,
and source-tile/access-change measurement remains an acceptance condition if
`OptInCraftStorage` is separately approved.

Those measurements settle the shipped hard limits:

- `MAX_CRAFT_SOURCE_ITEMS = 125`, aligned with the ordinary backpack item cap;
- `MAX_CRAFT_RESOURCE_LINES = 4`, enforced by the data build and equal to the
  maximum in the 492 shipped recipes;
- `MAX_CRAFT_WITHDRAWALS = 125`, so even maximum fragmentation remains bounded;
- `MAX_CATALOGUE_OPENS_PER_TICK = 32` after per-connection coalescing and
  `MAX_COMMAND_WORK_PER_TICK = 256`, with untouched FIFO work deferred whole;
- `HOUSE_INVENTORY_REBUILD_BUDGET = 256` projection units per tick; and
- house search accepts at most 32 exact selectors and returns at most 50 rows
  per page.

Catalogue capture is synchronous fixed indexed work, so there is no worker
service channel or queue capacity to tune. The 32-open lane is its explicit
capacity and its deferred/coalesced counts are observable.

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
- [x] Add counters/timers for index projection, snapshot/service queue latency,
      withdrawal planning, output preparation, commit, deferrals, and
      over-budget refusals.
- [x] Benchmark index updates and craft requests against unrelated world size,
      local source density, fragmentation, and simultaneous clients; assert the
      request path stays inside its declared work bounds.
- [x] Run the 10,000-sequence property soak and commit all regression seeds.
- [x] Remove the temporary catalogue `MaterialStock` backpack snapshot once
      server readiness is gone.
- [x] Validate restored stack amounts before entity allocation in
      `crates/server/world/src/tick/persist.rs`; zero or above-`MAX_STACK` saved
      piles are corrupt external records and must be refused, never clamped or
      allowed to reach the in-memory quantity door.
- [x] Update `docs/item_kind.md`, architecture notes, and gameplay backlog with
      the settled ownership/index/transaction contracts.

Structured tracing records `craft_stock_projection`,
`house_inventory_projection`, `withdrawal_prepare`, `withdrawal_commit`,
`output_prepare`, `craft_commit`, and `catalogue_context` elapsed time and work
shape. `command_budget` records admitted command/catalogue work, coalesced
opens, and deferred commands; bounded preparation/projection refusals include
their reason. Because the catalogue lane is synchronous, `catalogue_context`
is the service latency and there is no hidden worker-queue wait to measure.

The release measurements and commands are recorded in A0. The mutation-model
soak ran 10,000 sequences of up to 512 actions with
`OPENSHARD_ITEM_TRANSACTION_CASES=10000` and
`OPENSHARD_ITEM_TRANSACTION_ACTIONS=512`; it passed, so proptest produced no
regression seed to commit.

Done when every individual operation and every tick admits only measured bounded
work, the property soak is clean, and no retired full-world or recursive request
scan remains on the craft path.

**Final verification (2026-08-31):** `cargo fmt --all -- --check`, workspace
clippy with `-D warnings`, and `cargo test --workspace --quiet` passed. Both
ignored release microbenchmarks passed with the A0 numbers above. The final
release mutation soak passed 10,000 sequences of up to 512 actions in 1.45
seconds and generated no regression seed.

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
