# A ship on the water, phase by phase

The implementation record of the boat work: what existed before it (almost
nothing), the two surveys that decided whether water could be made walkable, and
the two phases that put a ship in the harbour and then made it sail.

The decisions this was built against are [`design_boats.md`](../design_boats.md);
what is built and what is open today is [`README.md`](../README.md). **The phases
below are `B1`–`B4` and the decisions are `B1`–`B7` — the same numeral means two
different things**, which is what comes of one document having held both. Where
this record says "B3" it means the phase; where it quotes a decision it says so.

## What existed, which was almost nothing

No boat multi id, no constant, no comment. `FOUNDATION_IDS` was the only named
multi range in the tree and it is a refusal.

One thing did exist, and it turned out to matter: **`Feature::SmoothShip`**
(`protocol/src/feature.rs:87`) — *"Smooth boat movement (`0xF6`). Since
7.0.9.0."* The version gate was written and the packet behind it was not. That is
exactly the state `0x99` was in before housing's H2, and it meant the wire
question had a named answer rather than an open one.

| piece | server | client (ours) | classic client |
|---|---|---|---|
| multi components | **built** (`uofiles::multi`) — reads boats too, unnamed | **built** — `net_command::multi_pieces` | reads its own files |
| a boat on the water | nothing places one | draws multis already | draws multis already |
| the hull blocking a step | `Obstructions` **cannot express a deck** — B3 | — | n/a |
| water as a question gameplay can ask | **two notions, neither reachable** — B4 | — | n/a |
| `0xF6` smooth movement | **no packet**, `Feature::SmoothShip` names it | **no packet** | speaks it, ≥ 7.0.9.0 |
| a passenger moving with the deck | **no parent relation of any kind** — B1 | n/a | n/a |
| the tiller, the hold, the plank | — | — | ordinary items |
| decay, the deed | — | — | n/a |

## The two surveys under decision B5

Decision B5 predicted that turning `MapTerrain::swimming` on would fire the
`landCheck` guard under every deck and drop a boarding player into the sea. Both
halves were measured.

### 🚩 The measurement was taken, 2026-08-23, and the first bullet's cause is wrong

`terrain.rs`'s `land_check_survey` runs the guard over the whole of facet 0
twice, once as a walker and once **as a swimmer** — which is that bullet's case,
since a swimmer is exactly the body water is ground to. The guard fires 2,385
times for the swimmer against 2,381 for the walker, and in **neither** run does a
body land below the surface the guard discarded. Not once, out of every platform
static on the facet.

The arithmetic is the guard's own third condition, `landCenter > ourZ`: it fires
only where the land is *higher* than the deck, so discarding the deck moves the
body **up** onto the land. It cannot put a body under anything.

**And a boat's deck could never have been the thing discarded**, which is the
stronger half. That guard lives inside `MapTerrain::check`'s loop over the
*map's* statics, and a moored ship is not in the map — it is a `Cover` in the
overlay, reached through `walk::aboard` and `walk::climbed`, which is a different
function that never sees the guard at all. The bullet described a mechanism that
does not connect to the thing it is about.

**What was not refuted is the outcome**, only that route to it. With `swimming`
on, `check` stops refusing water and answers with its `land_center`, so a body
that cannot reach the deck stands on the sea instead of being refused — and the
deck is reached through `climbed`, which bounds the climb by `MAX_STEP_UP`. A
deck more than two above the water would leave a body floating under its own
ship. That was a different defect with a different owner, unmeasured, and it
needed the overlay this survey did not have. See
[the pier-and-bridge investigation](../../world/evidence/2026-08-24-the-movement-surface-investigation.md)
for the full numbers and for the suspects the player report has left.

### 🚩 And the open question was measured, 2026-08-24 — it was right, and it is closed

The overlay that survey did not have is `openshard-boats`'s `moored_boat`, which
moors a real small boat at a real berth beside every pier on facet 0 and then
**walks the swimmer in from the water rather than off the pier**. That
distinction is the whole of it: a body stepping off a pier reaches from the top
of the pier's own art and clears a deck easily, while the body this prediction is
about is floating alongside the hull, whose reach is the waterline plus two.

**3,866** tiles of water alongside a hull, **8,450** steps toward a ship:

| | refused | arrive on the deck | **arrive under it** |
|---|---|---|---|
| the reading now | 8,450 | 0 | **0** |
| the reading retired | 7,467 | 93 | **890** (worst 3) |

**The prediction was correct** — 890 bodies in the sea with their own ship
overhead — and what closes it is not the flag but the deck's **blocking half**,
which the same day's fix restored: a plank three units thick is three units of
solid wood starting at the waterline, so there is no longer a gap under it to
float in. A swimmer can also no longer clamber aboard over the gunwale (93 before
and none now), which is UO's own answer rather than a loss: you board over the
plank, and this shard has not built one.

**The flag still stays off**, because nothing else about a swimmer has been
measured and decision B4 has not moved. What is gone is the one measured
objection to it.

## The phases

Four, and the first two are the honest split: the **index** is one phase's whole
content, and the **motion** is the next one's.

### B1 — a ship on the water, moored

**What a player sees:** a ship in the harbour, and they can walk its deck.

1. `Terrain::land_is_water`, and `MapTerrain`'s implementation over the flag it
   already reads.
2. `openshard-boats`: `place(state, actor, at, facet, multi, owner)` —
   `housing::place`'s shape, refusing anything not over water, staff-exempt on
   the judgement refusals the same way H6's is.
3. The `Boats` index on `FacetState`, and `LiveTerrain` consulting it for **both**
   questions — the hull blocks, the deck is a surface.
4. Saved: a `BoatRecord` (serial, multi, position, facet, owner), the index
   rebuilt at boot. Components **not** saved, for `HouseRecord`'s reason
   unchanged — a boat's shape *is* a pure function of its id, so unlike a
   customised house it is exactly the case that rule was written for.
5. `.boat <multi id>`, `.house`'s shape.

**Done when** `.boat` puts a ship on the water, both clients draw it, walking
onto the deck lands the player *on* the deck at the right z and is not refused,
walking into the hull is refused, and it is still there after a restart.

Nothing here needs a single decision from the motion phases. That is the point of
the boundary.

#### Built

All five steps, and the plan's "done when" holds: `.boat` moors a ship, both
clients draw it, walking onto the deck lands the player on it at the right z,
walking into the hull is refused, and it is still there after a restart.

What came out differently:

**The seam had a hole in it, and it was not the one B4 predicted.** `land_is_water`
went on the terrain exactly as planned. What was not planned is that
`LiveTerrain` — the wrapper *every running shard's movement goes through* —
forwarded seven methods and no more, so anything asking it whether a static
blocks, how tall it is, what it is called, what it weighs, which layer it is worn
on, or what a multi is made of got the trait's **no-client-files default**. Those
defaults are honest for a shard without a map and wrong for one that has a map
and wrapped it. It stayed invisible because placement, single-click and
encumbrance all hold the map terrain directly; a boat is the first thing that
asks through the live one.

**A `Plank` is the derived answer, not the component.** The decision said "entity
→ origin plus multi id", which would have meant walking the multi per step. The
index holds what the art *lays* instead — a floor, a solid body, or both, at what
height — derived once at the mooring. That is what makes the hot path a hash
probe rather than a component walk, and it is why `Boats::moor` takes tiles
rather than an id.

> **Corrected 2026-08-24, and the correction is *which* derivation.** This
> paragraph said "the *split* — hull or deck", and that is exactly what the code
> did: `is_blocking()` decided, and everything that did not block became a
> **floor**. Every other placement on this shard — housing, decoration, the
> persistence reload, the client — reads its art through `Cover::of_static`,
> which splits on `is_platform()`. That is ServUO's own test as well:
> `(flags & ImpassableSurface) == TileFlag.Surface`,
> `Scripts/Services/Pathing/Movement.cs:211`, where a candidate to stand on must
> carry `Surface` and not merely fail to carry `Impassable`.
>
> Over the shipped multi table the two disagree about **eighty** components of
> the twenty-four ships, every one of them rope, rudder or tiller art — and
> `walk::aboard` takes the *nearest* live surface with only the climb bounded,
> so an invented floor at the ship's own z is a floor two under the deck beside
> it. Each ship also had two or three tiles a body could walk on that no other
> reader believes in, and every deck was missing the solid half that keeps a
> body out of the planking.
>
> `Plank` now holds a `Covers`, filled only by `Plank::of_art`, with the field
> private so there is nowhere left to write a second reading; `hull_blocks`
> became `blocks_at` because a ship's own deck answers it now. The measurement
> is `openshard-boats`'s `moored_boat`, which keeps the retired rule written out
> so the number stays reproducible.

**The measurement, which the index decision asked for instead of an assurance.**
Release, 100,000 steps: **1.5ms with no boats, 5.5ms with one moored** — 15ns
against 55ns. The empty case is the `is_empty` length check working. The 3.6x is
stated as the least flattering framing available on purpose: the fixture's
`can_step` is one integer comparison, so the probe is nearly all of the measured
work, and against a real `MapTerrain::can_step` the same absolute 40ns is a
fraction rather than a multiple. A per-facet bounding box would remove it; 40ns
did not justify a second structure to keep in step.

**`openshard-boats` does not depend on `openshard-housing`.** The plan said
"`housing::place`'s shape" and that is what it is — but as siblings. The one
thing they share is `0x4000`, which belongs to the protocol and to neither of
them. A shared "multi placement" abstraction would have to be designed before
either caller needed it.

**Schema v32, and the bump is about the writer again.** An older reader is only
missing ships. An older *writer* does not know the `boats` table, saves a world
with none in it, and the fleet is gone on the next boot along with whatever was
standing on a deck. A house at least stays where it was.

**And a ship had to be excluded from the item sweep by name**, exactly as a house
was — it carries a `Drawn` and a `Position` like any item, so without the
exclusion it would be saved twice and restored as a hull with no deck under
anybody. That is a bug this engine has already had once, with houses, which is
the only reason it was looked for.

### B2 — it moves

1. `Boats::step(state, boat, direction)` — decide-then-apply, `World::step`'s
   structure with the terrain check replaced by *does the whole translated
   footprint fit*.
2. The manifest derived per move, each occupant moved absolutely.
3. The wire: forget-and-reveal for the hull, `move_to` per occupant.
4. The cadence gate in `tick.rs`, beside `collapse_houses` and
   `items::close_doors` — the two systems that already do the halves of this and
   never together.
5. Speech control and the tiller.

**Done when** "forward" moves the ship a tile, everyone standing on it arrives
with it, a player's own camera follows, and a ship steered into a rock stops
rather than passing through it.

**The one collision test this phase owes:** two boats. A hull is not in
`Obstructions`, so two hulls do not see each other through the mechanism that
stops everything else, and *two ships in one tile* is the failure that mechanism
would have caught for free. The step check must ask the boat index about **other
boats**, and the test is named `two_boats_do_not_occupy_one_tile`.

#### Built

Steps 1 through 5. `.sail <direction|stop> [fast]` steers the ship under your
feet, it holds its course on a cadence, everyone standing on the deck arrives
with it, and a ship steered into a rock stops rather than passing through.
**Step 6, the tiller, is not built** — `.sail` stands in for it, which is
`.hdesign`'s argument one noun over: the steering can be exercised without the
item and the speech path existing first, so a bug in the cadence is a bug in the
cadence.

What came out differently:

**The course check is not the berth check, and that is the whole of why a ship
moves.** `check_berth` refuses a tile any boat is in — which, for a move, is the
ship itself in the tiles it is leaving. Every step overlaps where the ship
already is, so reusing it would have refused a ship the right to move at all.
The two differ by one comparison, `plank.boat != boat`, and
`a_ship_is_not_blocked_by_the_tiles_it_is_leaving` is the test that fails when
they are folded together.

**Being aboard is three questions, not one**, and each of the two the plan did
not ask cost a defect.

The plan said "who is standing on a tile the boat covers". That is a third of it:

1. *On a covered tile.* A sector sweep answers this, and it is the only part the
   plan had.
2. *With feet on a plank* — a swimmer at the waterline and a body on a pier the
   ship is moored against are both on a covered tile and neither is a passenger.
   `someone_in_the_water_beside_the_hull_is_left_behind` is the test.
3. *A plank of **this** ship.* The manifest asked `Boats::deck_at`, which
   answers for the whole facet — *there is a floor here*, never whose. A tile
   belongs to a list of planks with a `boat` on each, and two ships sharing one
   is a case the index has always supported
   (`casting_off_one_boat_leaves_the_other`); the manifest was the one reader
   that did not look at that field. So a ship under way took the crew of any
   other ship its sweep reached and translated them by its own delta, off their
   deck and into the water. `Boats::carries(boat, x, y, z)` is the named half of
   `deck_at`, and
   `a_ship_under_way_does_not_carry_the_crew_of_the_ship_beside_it` is the test.

**And the sweep is the berth's box, not a square hung off its corner.** The
candidates come from one sector query rather than one per covered tile, and a
query is a square around a point. That point was `covered.first()` — the
north-west corner — with a radius reaching the far end of the hull, so a galleon
lying east-west put twenty-odd tiles of open sea on every side of its bow inside
the net. Centred on the bounding box the radius is half the longer span.

That is *not* what fixed question 3 — the filter is what decides, and the
control run confirms the passenger is still dragged under the new geometry with
the old filter. It is worth having because the surplus of a sweep is where wrong
answers live, and a surplus of twenty tiles of sea is one nothing notices is too
big.

**The packets a move costs**, which the wire decision asked for by name: **two per
client that can see the ship** — a `0x1D` and the `0x1A`/`0xF3` that draws it
again — plus, per occupant, **one `0x20`** to its own client and **one `0x77`**
to each client watching it. A sloop under way with one player aboard and one
watching from the shore is six packets a tile. The hull's two are exactly what
phase B3's single `0xF6` replaces.

**A ship needed somewhere to keep its course, and it is not on `Boat`.**
`Sailing { direction, next, every }` is its own component, absent on a moored
ship — so "is anything sailing" is a query over a sparse set that is empty on
every shard with no ship under way, and the tick's pass costs nothing on all of
them. It is **not saved**, deliberately: the manifest is derived per move, so a
course that survived a restart would sail without the crew who logged out at the
last berth. The reference writes a boat's facing and not its motion.

**A blocked ship furls rather than retrying.** A hull grinding against a rock
twenty times a second is the shape of a stuck NPC. `sail` returns the ships that
stopped and the tick tells their owners, which is the split `collapse_houses`
already uses — `openshard-boats` has no opinion about messages and the tick does.

**The cadence is the reference's two intervals and one tile each.** ServUO's
`BaseBoat` has `SlowInterval` 1000ms and `FastInterval` 250ms; under
`NewBoatMovement` both move a single tile, and the older three-tiles-per-fast-interval
arrangement is a different thing to port and not the one modern clients see. At
twenty ticks a second that is 20 ticks and 5.

**`World::step`'s tail is still not factored out, and this is now its fourth
caller** — the point the backlog below names as when it wants a name. Each
occupant goes through `WorldState::move_to`, which is that sequence with the
`0x20` already in it.

### B3 — smooth, for the clients that can

`0xF6`, behind `version.supports(Feature::SmoothShip)`. A High Seas client gets
one packet per move; a 4.0 client keeps B2's redraw, unchanged and still correct.
Strictly better, and it removes nothing. **Not built** —
[`plans/housing/boats/PLAN.md`](../../../plans/housing/boats/PLAN.md).

### B4 — the boat as property

The hold as a container, the plank as a door, the deed, decay. All of it is
housing's H2–H5 with a different noun, and none of it is on the critical path for
a ship that sails. **Not built** — the same plan.

## Backlog, found while planning this

- **Two notions of water, and a third would have been written.**
  `TileFlags::is_water()` is the client's truth and is private behind
  `MapTerrain::land_is_ground`; `WATER_TILES` is a generated id-range table that
  fishing uses. B4 adds the seam that reaches the first; making the second its
  documented fallback is a separate change and is not done here.
- **`MapTerrain::swimming` has been dead since it was written.** It is set true
  only in movement's own test helper. B4 keeps it and says what it is for, which
  is better than either deleting a correct abstraction or enabling it by
  accident — but it has now been unread long enough to be worth a note.
- **`World::step`'s tail is the reusable part and it is not factored out.** B1
  reuses `disrupt` → `move_to` → `refresh_around` → `broadcast_move` by copying
  the sequence, which is the third caller of that sequence after `npc::live` and
  `quests::advance_escorts`. A fourth would be the point at which it wants a name.

## Backlog, found while walking a moored pier (2026-08-24)

- **A ship can be moored through a dock.** `check_berth` asks only that every
  berth tile is *water*, and a water tile can carry a pier plank. **52 of the
  352** boardings the survey measured land on the plank rather than on the deck
  under it. Harmless as a landing and wrong as a placement: nothing should be
  able to moor a hull inside a dock. The fix is a second clause — no static
  platform over the berth — and it belongs beside the "all sea" one rather than
  in the step rule.
- **`Boats::deck_at` and `Boats::carries` are a third and fourth spelling of
  `Overlay::surface_at`'s rule**, the nearest surface to a body, each missing a
  different part of it: `deck_at` has no reach filter and no
  tie-goes-to-the-lower, `carries` asks for equality instead. Both were the
  production path before the projection existed; they and `blocks_at` now have
  no caller outside tests. Either they read the overlay or they go.
- ~~**This client has no notion of a boat at all.**~~ **Repaired 2026-08-25.**
  Every known multi is expanded into its drawn components, `clutter::project`
  lays their `Cover`s into the live overlay, and the online and offline walk
  paths now call `movement::predict_step` over the complete `Footing`. A deck or
  player-house stair therefore contributes the same predicted `z` the shard's
  step rule chooses; `0x22` no longer has an uncorrected height disagreement to
  carry.
