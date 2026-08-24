# A house that moves

`docs/housing.md` deferred boats in one sentence and it is the right one: *"a
boat is a house that **moves**, which is a different problem: every component's
position changes together and the obstruction index has to follow."*

Every hard decision below follows from the word **moves** rather than from the
word **boat**. The multi reader already reads them; the picture is free the same
way a house's is; the placement rules are housing's with the sign flipped on one
of them. What is new is that a boat's shape is somewhere different every few
seconds, and nothing in this engine was built for that.

> Read [`housing.md`](housing.md) first — H1's decisions about multis, footprints
> and the obstruction index are assumed here rather than restated, and two of
> them turn out not to survive contact with a thing that moves.

## What exists, which is almost nothing

No boat multi id, no constant, no comment. `FOUNDATION_IDS` is the only named
multi range in the tree and it is a refusal.

One thing does exist, and it turns out to matter: **`Feature::SmoothShip`**
(`protocol/src/feature.rs:87`) — *"Smooth boat movement (`0xF6`). Since
7.0.9.0."* The version gate is written and the packet behind it is not. That is
exactly the state `0x99` was in before housing's H2, and it means the wire
question below has a named answer rather than an open one.

## What is missing, in one table

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

## Decisions, taken here

### B1 — a passenger's position is absolute, moved the way `World::step` already moves one. No parent transform.

The alternative is real and worth naming before refusing it: a
`Carried { parent, dx, dy, dz }` and a resolver, so a passenger's position is
*derived* rather than written.

It is refused on the strongest evidence available, which is that **this engine
already tried the weaker version and declined it**. Mounting does not carry the
mount — it *removes it from the world*: `forget` from every watcher,
`sectors.remove`, `registry.remove::<Position>`
(`items/src/mounts.rs:82-83`). A ridden creature has no position at all, and the
saddle item is what the ride is rebuilt from at restore. Carrying was not
expressible, so the engine deleted instead.

The structural reason it was not expressible: `Position`, `Contained` and
`Equipped` are mutually exclusive and absolute, and **everything** reads
`Position` — `Sectors`, `watchers_of`, `broadcast_move`, `refresh_around`, the
save sweep, `region_at`, `house_at`, `evict_the_banned`, the step check's `from`.
A transform is a fourth kind of "where", and until every one of those learned it
each would answer the wrong tile *while looking correct*. That is `style.md`'s
argument against `Deref` in a different colour: the hole is spelled with the
empty string, and there is no line for a reviewer to object to.

So a boat move computes the delta once and then moves each occupant absolutely,
reusing the tail `World::step` (`tick/motion.rs:207`) already reuses —
`disrupt`, `move_to` (which sends the player's own `0x20`), `refresh_around`,
`broadcast_move`.

**The cost, named:** a passenger's deck position is authoritative and rewritten
every move. Standing on a deck is not *derived* from the boat; it is
re-established each time. If the two ever disagree, the position wins, because
the position is what every other system reads.

### B1a — the manifest is derived per move, not stored.

Who moves when the boat moves is answered by *who is standing on a tile the boat
covers*, derived at the moment of the move from `tiles_of` and `Sectors::nearby`.

Not an `OnDeck` component. That is a second copy of a fact `Position` already
holds, and a copy that goes stale the moment somebody steps aboard, is teleported
aboard, logs in on a deck, or dies on one.

This is `adopt_doors`' rule reused rather than restated — *a door inside your
house is your house's door; a body on the deck is a passenger* — and
`evict_the_banned` is the worked example of the same scan. It is over one sector,
not the registry, and it runs on the move cadence rather than per tick.

### B2 — the wire is forget-then-reveal, and that is the reference's own answer for a classic client.

There is no incremental item-move packet in this repo. The only precedent for a
ground item changing tile is `items::doors::set_door`
(`items/src/doors.rs:173-253`): `forget` (`0x1D`) from every watcher, write
`Position`, `sectors.insert`, swap the obstruction, `state.reveal` (`0x1A`). It
flickers by construction, and its own doc says why — *"a client only redraws what
it was told to forget."*

Three facts settle this rather than one preference:

- **ServUO does the same thing.** Its `BaseBoat` pre-High-Seas removes and
  re-sends its components on each move. The flicker is not this engine's
  shortcoming; it is what a 2D client without `0xF6` gets from any server.
- **`0xF6` exists and is already gated.** So the smooth path is available — to
  High Seas clients only, and this shard's floor is AoS. It therefore cannot be
  the *only* answer, and it must be reached through
  `version.supports(Feature::SmoothShip)` and never through an era comparison,
  which is `architecture.md`'s rule with a table of counterexamples behind it.
- **The cadence is the mitigation, and it is a decision rather than a detail.**
  ServUO steps a boat on a timer. Here that is a `ticks.is_multiple_of(N)` gate
  at the call site in `tick.rs` — the existing idiom, beside `collapse_houses`
  (`tick.rs:707`), which is its nearest neighbour in kind. A redraw every N ticks
  is a boat that shudders; a redraw every tick is a boat nobody can look at.

The phase that lands this owes a **number**: packets per move is one
forget-and-reveal for the hull per watcher, plus one `move_to` per occupant.
Bound it and write it down.

### B3 — the hull stays **out** of `Obstructions`, and a boat gets an index of its own.

`Obstructions` is `HashMap<(u16,u16), Vec<Obstacle>>` keyed by *(entity, z)*,
with no translate, no bulk write and no entity→tiles reverse index. Moving an
N-tile footprint through it is 2N hashed vector operations plus two
`footprint_of` derivations, every move, every boat, for ever.

Refusing to add a bulk API is a design argument and not a performance one. The
index's own reason for existing says so, and so does housing's D2: *"a step is
ten a second and a house does not move."* A boat is the counter-case to the
premise that put houses in there. Bolting a fast path onto a structure whose
whole justification is that its contents are static is `style.md`'s fudge
constant one level up — a second mechanism closing a gap the first mechanism's
premise opened.

**And there is a stronger reason, which is the real one: `Obstructions` only ever
subtracts.** A house's entry says *this tile is closed*. A boat has to say two
things — the hull is closed, **and the deck is somewhere to stand, at height z,
over water that is otherwise not ground at all**. A house never had to add a
floor, because its floors sit on land the map already calls walkable.
`Obstructions` has no way to say "there is now somewhere to stand here", and
giving it one would make it a different structure with a different name.

So: a per-facet `Boats` index — entity → origin plus multi id — consulted by
`LiveTerrain`, which is already the composition seam (map + obstructions) and is
exactly the shape a third source belongs in.

**The hot-path warning, stated before anyone measures it:** `LiveTerrain::can_step`
runs for every step by every mobile, and its diagonal rule re-enters it twice
more. The boat consultation must be an integer comparison against an empty index
in the overwhelming case, and the phase that lands it owes a measurement rather
than an assurance.

### B4 — `MapTerrain::swimming` stays false, and is not deleted.

`swimming` (`movement/src/terrain.rs:65`) is a property of a *terrain*, and a
facet has one. Setting it true makes water walkable for **every mobile on the
shard** — that is not "boats work", it is "everybody walks on water". It is
documented "A boat or a fish says yes", has never been set true on any server
path, and false is the correct state for it.

What a boat needs is two narrower answers, and they are different questions.

**(i) May a boat be placed here?** A water test at placement, `is_road`'s shape
(`housing/src/lib.rs:645`) with the sign flipped. But there are already **two**
notions of water in this tree and neither is reachable from where a boat would
ask:

- `TileFlags::is_water()` in `openshard-uofiles` — the client's own truth —
  reachable only from inside `MapTerrain::land_is_ground`
  (`terrain.rs:386`), which is private.
- `WATER_TILES`, id ranges generated by `state/build.rs` and consumed by fishing.

Writing a third is what `style.md`'s "look for it before writing it" forbids, and
the fix is small: **`Terrain::land_is_water(tile) -> bool`**, defaulting `false`,
implemented on `MapTerrain` over the flag it already reads. That is the seam
`item_blocks`, `item_height` and `multi_components` all came through, and it
answers "what if the shard has no client files" for free the way every other
method on that trait does.

Fishing's `WATER_TILES` then becomes the no-client-files fallback rather than a
second truth — named as a backlog item, not done here.

**(ii) May a mobile stand on the deck?** Not a water question at all. A deck is a
climbable platform static at a z above the water, and `MapTerrain::check` already
stands bodies on platform statics — it simply never sees this one, because a
multi's components are not in the map file. That gap is B3's index, and it is
B3's *positive* half.

### B5 — the deck is the open pier/bridge bug, and this is the phase that supplies its repro.

`docs/roadmap/03-world/movement-surface-investigation.md` records the
investigated movement defect:
`MapTerrain::check`'s
`landCheck` guard, ported variable-for-variable from the reference and audited
rather than slipped, discards a climbable platform static when the land beneath
it is walkable and its average height reads close to the deck. What saves piers
and bridges today is that they sit over water, where `land_is_ground` is false
and the guard never fires. The roadmap says it is unfixed because it needs a
repro against real client files rather than a silent patch.

Two things follow, and the first is the concrete reason B4 goes the way it does:

- **Turning `swimming` on would fire the guard under every deck.**
  `land_is_ground` becomes true over water, the deck static is discarded from the
  candidate list, and a player walking aboard lands on the water's `land_center`
  — in the sea, under their own boat. That is not a tidiness argument against the
  flag; it is a measurable fall.
- **A boat with a deck is the repro the roadmap asks for, and it needs no client
  files.** A synthetic multi with a climbable platform component at a known z
  over a land tile of known height is constructible in a test — `Multi::new` is
  public and `Component`'s fields are public — and it reproduces the *shore-end*
  case (deck over walkable land) the player report of 2026-08-02 describes.

So the boat phases do not have to *fix* the divergence, and must not make it
worse. But they are what finally hands the fix its evidence, and that is named as
an output rather than left to be noticed.

#### 🚩 The measurement was taken, 2026-08-23, and the first bullet's cause is wrong

`terrain.rs`'s `land_check_survey` runs the guard over the whole of facet 0
twice, once as a walker and once **as a swimmer** — which is this bullet's case,
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
function that never sees the guard at all. The bullet describes a mechanism that
does not connect to the thing it is about.

**What is not refuted is the outcome**, only this route to it. With `swimming`
on, `check` stops refusing water and answers with its `land_center`, so a body
that cannot reach the deck stands on the sea instead of being refused — and the
deck is reached through `climbed`, which bounds the climb by `MAX_STEP_UP`. A
deck more than two above the water would leave a body floating under its own
ship. That is a different defect with a different owner, it has **not** been
measured, and it needs the overlay this survey does not have.

**So the flag stays off** — the conclusion is unchanged, and B4 does not move.
What changes is that the reason is now an open question rather than a settled
one, and the second bullet's repro is spent: it was run, and it refuted the thing
it was built to confirm. See `roadmap.md`'s pier-and-bridge entry for the full
numbers and for the suspects the player report has left.

#### 🚩 And the open question was measured, 2026-08-24 — it was right, and it is closed

The overlay this survey did not have is `openshard-boats`'s `moored_boat`, which
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
measured and B4 has not moved. What is gone is the one measured objection to it.

### B6 — control is speech, and the tiller is a double-click.

The reference's tillerman answers speech keywords — forward, back, left, unfurl
sail, stop. This engine has the machinery already: `tick/speech.rs` routes speech
to keyword answers, and `npc`'s keyword answers are the precedent. The tiller is
an ordinary double-click target with `HouseSign`'s exact shape — a component
naming the boat by serial, so a tiller left standing over a boat that has sunk
opens nothing.

**No packet numbers or keyword strings are asserted in this document.**
`style.md`'s "ports name their source" applies: they come out of the reference at
implementation time and are cited at the constant, not guessed in a plan.

### B7 — a boat's own footprint is not a `no_housing` region, and does not need to be.

It might look as though housing's H6 gives "no house on a boat" for free by
setting a flag on a region the boat carries. It does not need to, and a boat does
not carry a region at all: `check_yard` already keeps five tiles between a house
and anything, measured wall to wall, and a boat that is not in `Obstructions`
(B3) is not in the yard scan either — so the placement question is answered by
B-1's own water rule instead. A house may not go on water; that is one mechanism,
not two. Named so nobody adds the second.

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

Nothing here needs a single decision from B1, B2 or B5. That is the point of the
boundary.

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

**A `Plank` is the derived answer, not the component.** B3 said "entity → origin
plus multi id", which would have meant walking the multi per step. The index
holds what the art *lays* instead — a floor, a solid body, or both, at what
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

**The measurement, which B3 asked for instead of an assurance.** Release, 100,000
steps: **1.5ms with no boats, 5.5ms with one moored** — 15ns against 55ns. The
empty case is the `is_empty` length check working. The 3.6x is stated as the
least flattering framing available on purpose: the fixture's `can_step` is one
integer comparison, so the probe is nearly all of the measured work, and against
a real `MapTerrain::can_step` the same absolute 40ns is a fraction rather than a
multiple. A per-facet bounding box would remove it; 40ns did not justify a second
structure to keep in step.

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
2. The manifest derived per move (B1a), each occupant moved absolutely (B1).
3. The wire: forget-and-reveal for the hull, `move_to` per occupant (B2).
4. The cadence gate in `tick.rs`, beside `collapse_houses` and
   `items::close_doors` — the two systems that already do the halves of this and
   never together.
5. Speech control and the tiller (B6).

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

**The packets a move costs**, which this document asked for by name: **two per
client that can see the ship** — a `0x1D` and the `0x1A`/`0xF3` that draws it
again — plus, per occupant, **one `0x20`** to its own client and **one `0x77`**
to each client watching it. A sloop under way with one player aboard and one
watching from the shore is six packets a tile. The hull's two are exactly what
B3's single `0xF6` replaces.

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
Strictly better, and it removes nothing.

### B4 — the boat as property

The hold as a container, the plank as a door, the deed, decay. All of it is
housing's H2–H5 with a different noun, and none of it is on the critical path for
a ship that sails.

## What this plan does not cover

- **Docking, and mooring to a pier.** It is a relationship between two multis and
  it wants B5's bug fixed first.
- **Pets and NPCs following aboard.** The manifest carries whoever is *standing*
  on the deck at the moment of the move, which is already right for a pet that
  happens to be there. A pet that should re-board after being left behind is an
  AI rule, not a boat rule.
- **The tillerman as an NPC.** The reference's is a mobile with dialogue. Here
  the tiller is an item and the answers are speech keywords, which is the same
  intent out of machinery that exists.
- **Multi-facet oceans.** `WorldConfig.facets` defaults to `vec![0]` and the
  checked-in `openshard.toml` does not override it. The index is per-facet like
  everything else on `FacetState`, so this costs nothing to leave.
- **Fixing the pier/bridge divergence.** B5 supplies the repro and says so; the
  fix is a deliberate deviation from the reference's `Movement.cs` and stays the
  roadmap's decision to take.
- **A translate or bulk API on `Obstructions`.** B3, and the reason is written
  down so the next reader who notices the missing API knows it was declined
  rather than overlooked.

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
- **This client has no notion of a boat at all.** `clutter` expands no multis —
  only a designed house's `HouseShape` — so a ship is invisible to the client's
  own step rule *and* to `predict_step`, which draws the body the instant a key
  goes down. The survey's median pier stands **seven** above the deck it boards
  onto, so every boarding is seven units of disagreement that a `0x22` carries
  no position to correct. B2 is about what the *shard* sends; this is the other
  end of it, and `docs/client.md` does not mention boats anywhere.
