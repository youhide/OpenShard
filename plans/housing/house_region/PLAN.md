# A house that is its own region

A house is supposed to be a region: no teleport in, no recall out, registered
with the walls and removed with them. It was decided as D4 of
[`docs/housing/design_house.md`](../../../docs/housing/design_house.md), left
without a phase to be built in, and rediscovered five phases later as the third
sub-phase of H6 — where it stopped again, this time for a reason worth writing
down rather than a reason nobody had noticed.

**It is not a session of typing.** `Regions`' whole public surface is `new`,
`set`, `clear`, `at`, `get`, `iter`, `len`, `is_empty`: there is no runtime
insertion or removal, and `set` is replace-all *by design* — its own doc argues
for that, because "a registration carries the whole set, so registering twice
cannot leave a stale half behind". A house registering itself contradicts a
stated property rather than filling a gap, so the first thing this plan owes is a
decision about `Regions`' shape.

What is already built, and what the flags are for, is
[`docs/housing/README.md`](../../../docs/housing/README.md); the phase that got
this far is
[`docs/housing/evidence/2026-08-31-the-house-phases.md`](../../../docs/housing/evidence/2026-08-31-the-house-phases.md)'s
H6.

## The decision this is waiting on

**D11 — a house's own region is derived from its footprint, not stored beside
it.** D4 said a house *is* a region and left the shape open. The shape follows
from H1's own rule: what is saved is where a house stands and which multi it is,
and the footprint is recomputed at boot from the same table placement read it
from. A stored rectangle would be a second copy of that, free to disagree with
the walls after an install update — the exact failure H1 refused for the
components.

So the region would be registered from `tiles_of` at placement and at restore, on
the same call that blocks the walls, and removed by `decay::demolish` on the same
call that unblocks them. `Regions` is per-facet on `FacetState`, which is where
the house's own `Obstructions` entries already live, so the two halves cannot end
up on different facets.

**What flags a house's region carries** is the smaller half and D4 already named
it: `no_teleport` and `no_recall`. Not `no_housing` — a house is not a place
another house may not go, that is what D3's yard is for, and setting both would
make the yard rule unreachable.

## The four blockers, and the third is decisive on its own

1. **`RegionId` is a `Vec` index and `at()` indexes it unchecked.** A removal
   that shifts the vector renumbers everything above it, silently invalidating
   every live `InRegion` and every saved `RegionRecord::id`. Removal has to be a
   tombstone, which changes the type rather than adding a method.
2. **The save sweep would write the house's region.** `region_records` maps
   `regions.iter()` straight out, so a derived region is saved, restored, *and*
   re-derived — two regions over one house, and the saved one outlives the
   demolition for ever as a no-recall zone in an empty field.
3. **The boot order destroys it.** `restore_houses` rides inside `restore_guilds`
   at `boot.rs:263`; `restore_regions` runs at `:270` and ends in `Regions::set`,
   which replaces everything. Any house region registered at restore is wiped
   seven lines later, on every boot, silently.
4. `reindex()` is a full rebuild, so an insert either pays it per placement or
   re-implements the grid fill.

## The shape that would keep every property D11 states

A second, **house-keyed layer inside `Regions`** — consulted by `at()`, invisible
to `iter()`, with ids from a range that is not a `regions` index. That answers
all four: the ids cannot collide with a `Vec` index or renumber one, the save
sweep walks `iter()` and therefore cannot see it, the boot's `set` replaces only
the map-derived layer, and an insert touches the house layer's own index rather
than the grid.

It is written here rather than in the tree because it changes a type every
gameplay crate reads, and because the whole lesson of the note under D4 is that
this belongs in a document before it is in a file.

## The order

- [ ] **1. Decide the id space.** How a derived region gets an id that
      `InRegion`'s crossing diff and `RegionRecord` can both live with. Everything
      below is blocked on this and nothing above it is.
- [ ] **2. The second layer in `Regions`**, with `at()` consulting it and
      `iter()` not, and a test that the save sweep does not see a house's region.
- [ ] **3. Registration and removal** from `place`, `restore_houses` and
      `decay::demolish` — the same calls that block and unblock the walls, so the
      two halves cannot diverge.
- [ ] **4. The boot order**, which is the one change that has to happen whatever
      shape step 2 takes: houses are restored before `restore_regions` replaces
      the set.

## What a test would pin

- A house's region present after placement and **gone after
  `decay::demolish`** — a region outliving its house is a permanent no-recall
  zone in an empty field, and nothing else would notice.
- A house restored from a save owning exactly one region, not two.
- A saved database whose `regions` table holds no house-derived row.
- Recall into a house refused, and teleport into one refused, off the same flags
  the dungeons already use.
