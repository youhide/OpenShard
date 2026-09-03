# Housing and boats: where they stand

The canon of the `housing` domain — `crates/server/housing` and
`crates/server/boats`, plus the parts of `server/state` they own (`House`,
`HouseDesign`, `LockedDown`, `Boats`, `Sailing`) and the two design packets in
`common/protocol`. This is everything that puts a *building* in the world: a
multi placed on the ground, walls that stop you, a door that knows you, storage
that survives, decay that takes it away, a shape a shard can invent, and a hull
that carries all of it across water.

What an item does once it is locked down belongs to
[`items/`](../items/README.md); what the map thinks a wall is belongs to
[`world/`](../world/README.md); what a client draws belongs to
[`client/`](../client/README.md) and [`render/`](../render/README.md).

**One entry point.** This page answers "what can a player do with a building
today" and says which document holds the reasoning for each line. Where this page
and a design document disagree, the design document is right and this page is
stale.

## The one-line answer

**A house is an ordinary item whose graphic is a multi id, and everything else is
a component beside it.** The picture is free — every client owns every shipped
multi — so the shard owes only what the picture does not say: where the walls
stop somebody, who may open the door, what happens when nobody pays. The two
exceptions are the whole of the interesting work: a **designed** house has no id
in anybody's file, so the shard owes the picture too; and a **boat** moves, so
its shape is somewhere different every few seconds.

```text
  entity ── Drawn(0x4000 | multi id) ── Position ── House { owner, lists, allowance, age }
                │                                      │
                ├─ HouseDesign { components, revision } │  the shape the shard owns
                ├─ DesignSession (while editing)        │
                └─ Obstructions entries                 └─ LockedDown on every pinned item

  Boats (per facet) ── Plank { covers, boat } per tile      the deck the map has no word for
```

## What the area is, by part

| Part | State | What is left | Held by |
|---|---|---|---|
| A house placed, blocking, saved, restored without going back through the placement rules | ✅ shipping | — | [`design_house.md`](design_house.md) D1–D2b |
| The five placement rules, the yard measured wall to wall, the road | ✅ shipping | — | the same, D3 |
| Deed, `0x99` multi cursor, the preview drawn under the pointer on our own client | ✅ shipping | — | [`evidence/2026-08-31-the-house-phases.md`](evidence/2026-08-31-the-house-phases.md) H2 |
| Co-owners, friends, bans as one ordered `Standing`; the sign and its window; eviction | ✅ shipping | a name resolves only while its owner is online — row 8 | the record's H3 |
| Doors: fourteen classic house types place their own leaves, and a house adopts what stands inside it | ✅ shipping | — | the same |
| Lockdowns and secures as one component, the allowance derived from the multi's area | ✅ shipping | — | the record's H4 |
| Indexed, permissioned, paginated house-inventory search — Ctrl+I | ✅ shipping | — | [`items/design_transactions.md`](../items/design_transactions.md) § `HouseInventoryIndex` |
| Decay in six reference stages, refreshed at the sign, and the moving crate | ✅ shipping | the crate never decays and nothing collects it, deliberately | the record's H5 |
| `no_housing` read at last: twenty-one dungeons closed, judged over the whole footprint at the house's own height | ✅ shipping | — | [`design_house.md`](design_house.md) D9–D9b |
| The staff exemption, as a row of a table rather than an early return | ✅ shipping | — | the same, D10 |
| A house that is its own region — no recall in, no teleport out | ⬜ not built | blocked on `Regions`' shape — row 1 | [`plans/housing/house_region/PLAN.md`](../../plans/housing/house_region/PLAN.md) |
| A per-house component list, saved in its own table, restored by join | ✅ shipping | — | [`design_customisation.md`](design_customisation.md) C1–C4 |
| `0xD8` and `0xBF 0x1D`, both ends, with the sparse-by-elevation layout read out of the reference | ✅ shipping | — | [`evidence/2026-08-24-the-design-phases.md`](evidence/2026-08-24-the-design-phases.md) C1 |
| A foundation placed with a derived design, deed included; `.hdesign` copies a shape onto a house | ✅ shipping | — | the same, C2 |
| Imported house templates: legacy Sphere packs converted to JSON, read at boot, placed as `.house @name` | ✅ shipping | documented only here — row 4 | `housing/src/{template,wsc}.rs` and its two examples |
| The design session's brackets: the sign's Customise button, `0xD7 0x0C`, and the `0xBF 0x20` the editor's own client sees | ✅ shipping | — | [`design_customisation.md`](design_customisation.md) C7 |
| The `0xD7` editor — a player reshaping their own house | ⬜ not built | every verb that *changes* a design, and validation — row 2 | [`plans/housing/customisation/PLAN.md`](../../plans/housing/customisation/PLAN.md) |
| A ship moored, blocking as a hull and carrying as a deck, saved and restored | ✅ shipping | — | [`design_boats.md`](design_boats.md) B1–B4 |
| A ship that sails on a cadence, with the crew it is actually carrying | ✅ shipping | no plank, so nothing boards deliberately — row 3 | [`evidence/2026-08-25-the-boat-phases.md`](evidence/2026-08-25-the-boat-phases.md) B2 |
| `0xF6` smooth movement for High Seas clients | ⬜ not built | two packets per watcher per tile until then — row 6 | [`plans/housing/boats/PLAN.md`](../../plans/housing/boats/PLAN.md) |
| The boat as property — hold, plank, deed, decay, tiller | ⬜ not built | the same plan | the same |

## What is enforced, and by what

- **A house and a hull are excluded from the item sweep by name.** Both carry a
  `Drawn` and a `Position` like any item, so without the exclusion each is saved
  twice and restored with its own serial already spoken for. It was a live defect
  once, with houses; the boat exclusion exists only because that had happened.
  Both are tested.
- **Every schema bump in this area is about the *writer*, and says which.** v27
  houses, v28 the access lists, v29 lockdowns, v30 the decay counter, v31
  designs, v32 boats. The one exception is v31, which is the reader's case: a v30
  build would compute a designed house's footprint off the foundation's bare
  platform and come up wearing the wrong walls with nothing to say so.
- **The staff exemption cannot reopen a refusal another decision closed.** It is
  a table with two rows — judgements about the plot are exempt, "there is nothing
  to place" is not — and three tests pin the second row: a marker that draws
  nothing, an id no client knows, and a foundation.
  `staff_are_still_refused_a_house_off_the_edge_of_the_world` and
  `staff_are_still_refused_a_ship_off_the_edge_of_the_world` are the same gate on
  both crates.
- **One test names Covetous, and it is the only one that carries a facet-sized
  world.** `a_shipped_no_housing_region_refuses_a_house` runs against the shipped
  dataset on purpose: the thing being proved is that twenty-one real rows reach
  the rule, which a fixture cannot say. Every other test here uses a fixture,
  because what they check is arithmetic.
- **A ship's own art has exactly one reading.** `Plank` holds a `Covers` filled
  only by `Plank::of_art`, with the field private, after a split on
  `is_blocking()` disagreed with the whole rest of the shard about eighty
  components of the twenty-four ships. `is_platform()` is the reading everything
  else makes, and it is ServUO's own test.
- **Being aboard is three questions, each with its own named test.**
  `someone_in_the_water_beside_the_hull_is_left_behind` (feet on a plank),
  `a_ship_under_way_does_not_carry_the_crew_of_the_ship_beside_it` (a plank of
  *this* ship), and `a_ship_is_not_blocked_by_the_tiles_it_is_leaving` (the
  course check is not the berth check). Each of the last two was a defect first.
- **Two hulls cannot share a tile**, moored or under way —
  `two_boats_do_not_occupy_one_tile` and its `_when_one_is_under_way` sibling.
  A hull is deliberately not in `Obstructions`, so the mechanism that stops
  everything else does not stop this, and the tests are the whole guard.
- **A redesign is transactional in both directions.**
  `a_design_that_draws_nothing_leaves_the_house_exactly_as_it_was`,
  `a_redesigned_house_takes_its_old_walls_out_and_puts_its_new_ones_in`,
  `demolition_refuses_an_unreadable_shape_before_touching_the_house`. Unblocking
  with the new shape leaves every unshared tile blocked for ever by an entity
  that is not there.
- **The tower is walked where it was reported.** `housing/tests/tower_entrance.rs`
  is an install-gated acceptance test: a made-up ground plane can prove the stair
  arithmetic, and only the real template on the real map tile can prove a body
  gets in. `a_placed_house_stair_reaches_its_first_floor` is its fixture-sized
  sibling.
- **A design session outlives nothing, and each of the three enders is named.**
  `logging_out_ends_a_design_session` and `dying_ends_a_design_session` in the
  world crate, `demolishing_a_house_ends_the_session_over_it` in housing. Each
  is a state that would otherwise sit on a house for ever, refusing its own
  owner with `AlreadyOpen` at the next login — and the logout one is an ordering
  as much as a rule: the session names its editor by a serial the disconnect is
  about to release.
- **101 tests in `housing` and 25 in `boats`**, plus the `moored_boat` survey,
  which keeps the *retired* reading of a plank written out so the number that
  retired it stays reproducible.

## What is open, ranked

**1. 🚩 A house is not a region, and the decision that said it was is five phases
old.** No recall out, no teleport in, a hall a stranger cannot appear in the
middle of — none of it exists. It stopped for a reason worth reading rather than
a reason nobody noticed: `Regions` has no runtime insertion, `set` is
replace-all *by design*, `RegionId` is a `Vec` index that renumbers on removal,
and `restore_regions` runs seven lines after `restore_houses` and wipes whatever
it registered. What it needs first is a decision about the id space —
[`plans/housing/house_region/PLAN.md`](../../plans/housing/house_region/PLAN.md).

**2. 🚩 No player can change the shape of their own house.** Every design on this
shard is either a shipped multi, a staff `.hdesign` copy, or an imported
template. The seam, both packets, the save, the foundation and now the session's
own brackets are built and tested — an owner can open the editor over their own
house and close it again — but **the session has no verbs**: build, erase,
select-floor, commit and revert are all still missing, and so is the validation
behind them. A session that can be opened and cannot change anything is exactly
the half worth having first, because every one of those verbs assumes a session
that cannot be left dangling.
[`plans/housing/customisation/PLAN.md`](../../plans/housing/customisation/PLAN.md).

**3. 🚩 Nothing boards a ship on purpose.** A swimmer used to clamber over the
gunwale and no longer can, which is UO's own rule — you board over the plank —
except that this shard has no plank. What is left is walking on from a pier, and
a ship can be moored *through* one: `check_berth` asks only that every berth tile
is water, and a water tile can carry a pier plank, so **52 of the 352** boardings
the survey measured landed on the dock rather than on the deck under it. The
mooring fix is a second clause — no static platform over the berth — and it
belongs beside the "all sea" one rather than in the step rule.

**4. An entire imported-house track is built and appears in no design document.**
`housing/src/wsc.rs` reads the world-item section of a legacy Sphere `.wsc`;
`template.rs` holds the shard's catalogue of imported templates, read at boot
from `openshard-houses/` under the client directory; `.house @name` places one;
two examples convert old house packs; and the client's editor mode reads the same
JSON so a preview and a placement are the same shape. Nine tests cover embedded
signs, closed leaves that become functional doors, and a fitting foundation
chosen without moving the origin. It is real, it is tested, and until this line
existed the only description of it was the code — which is the same class as row
1: work that happened without a document to be true in.

**5. `Boats::deck_at`, `carries` and `blocks_at` are a third and fourth spelling
of `Overlay::surface_at`'s rule**, each missing a different part of it —
`deck_at` has no reach filter and no tie-goes-to-the-lower, `carries` asks for
equality. All three were the production path before the projection existed and
now have no caller outside tests. Either they read the overlay or they go.

**6. A ship under way costs two packets per watcher per tile, and the packet that
would cost one is written down and unbuilt.** `Feature::SmoothShip` names `0xF6`
and has since before there were boats. The redraw is correct for every client
below High Seas and must stay; the gate must be the feature and never an era
comparison. [`plans/housing/boats/PLAN.md`](../../plans/housing/boats/PLAN.md).

**7. `Obstructions` has never had a hundred entries added at once outside a
placement click.** It is filled from the map at boot and poked at by doors since.
A house is the first bulk write and a demolition is the same hundred coming back
out; whether the structure wants an undo cheaper than that has not been asked,
and B3 declined to give it a bulk API on purpose.

**8. A name on the house sign resolves only while its owner is logged in.** A
serial resolves to an entity and an offline character has none, so the fallback
is the serial — which at least tells two absent friends apart. The guild roster
has the identical gap and the identical fix, a name read off the character store,
and neither has it.

**9. Two notions of water, and the seam only reaches one.**
`TileFlags::is_water()` is the client's truth, now reachable through
`Terrain::land_is_water`; `WATER_TILES` is a generated id-range table fishing
uses. Making the second the first's no-client-files fallback is the change that
would leave one truth, and it has not been made. `MapTerrain::swimming` is beside
it: correct, deliberately false, and set true only in a test helper since it was
written.

**10. Which components of a shipped house are floors is still content nobody has
swept.** The movement half is repaired — `Cover::of_static` reads a platform and
`walk::climbed` gets a body upstairs, with
`a_placed_house_stair_reaches_its_first_floor` and the tower acceptance test
behind it — but *which* components a given house calls a floor, and what a
demolition takes back out of the live layer, is the housing half of that repair
and was left when the world side closed.

**11. A design is the first per-house fact that is large.** Everything else a
house owns is a serial, a set of serials or a `u32`; a design is a few hundred
rows in a table joined on every restore. What the save cadence does with that has
not been measured, and `housing::place` still reads the multi table three times
per placement — cheap at a click, and the same shape of cost once it is three
reads of a `Vec<Component>` on the entity.

**12. The moving crate is permanent, on purpose.** It does not decay and nothing
collects it: ServUO internalises its own after three hours and banks it, which is
a real feature and not this one. A crate that rotted would eat somebody's
belongings on the day their house came down. Recorded so nobody reads the
accumulation as a leak.

**13. `RegionFlags::safe` is asleep deliberately.** Zero rows in the shipped
dataset carry it, and waking it would commit the engine to a PvP rule whose other
half does not exist. Written down because a dead flag with a reason is a
different thing from a dead flag nobody mentioned — which is exactly what
`no_housing` was for five phases.

## The documents

**Design** — the model as built, no status in them:

- [`design_house.md`](design_house.md) — what a multi is, the eleven decisions a
  classic house is made of, the five placement rules and which of them staff
  skip.
- [`design_boats.md`](design_boats.md) — why a passenger's position is absolute,
  why the hull is not in `Obstructions`, and why `swimming` stays false.
- [`design_customisation.md`](design_customisation.md) — where a per-house
  component list lives, why `Terrain::multi_components` cannot hold one, the
  revision as a cache key, and the six-step commit.

**Evidence** — measurements and closed records; none of them is a status:

- [`evidence/2026-08-24-the-housing-phase.md`](evidence/2026-08-24-the-housing-phase.md)
  — the roadmap's own record of the housing phase.
- [`evidence/2026-08-24-the-boats-and-customisation-phase.md`](evidence/2026-08-24-the-boats-and-customisation-phase.md)
  — the same for boats and designed houses.
- [`evidence/2026-08-24-the-design-phases.md`](evidence/2026-08-24-the-design-phases.md)
  — the seam, both packets, the sparse-by-elevation layout, and the foundation
  that turned out to need no per-house-type table.
- [`evidence/2026-08-25-the-boat-phases.md`](evidence/2026-08-25-the-boat-phases.md)
  — the two swimmer surveys, the plank that had one reading too many, and the
  three questions "who is aboard" turned out to be.
- [`evidence/2026-08-31-the-house-phases.md`](evidence/2026-08-31-the-house-phases.md)
  — six phases, the schema bumps and their arguments, and the flag that was
  plumbed end to end for five phases while nothing read it.

**Plans** — what is not built lives outside `docs/`:

- [`plans/housing/house_region/PLAN.md`](../../plans/housing/house_region/PLAN.md)
  — a house as its own region, and the `Regions` decision it waits on.
- [`plans/housing/customisation/PLAN.md`](../../plans/housing/customisation/PLAN.md)
  — the design session, from the brackets to the validation.
- [`plans/housing/boats/PLAN.md`](../../plans/housing/boats/PLAN.md) — `0xF6`,
  and the boat as property.
