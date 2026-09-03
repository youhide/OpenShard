# A house, and the ground it stands on

Placement, the walls that stop you, the door that knows you, the decay that
takes it away — **one model, because they are one object**. A house is not a
feature made of parts that could ship separately: a house you can place and walk
through is not a house, and a house with a lock and no decay is a shard that
fills up and never empties.

**What is built and what is open is [`README.md`](README.md)**; how each piece
was built, and where the code came out differently from the decision, is
[`evidence/2026-08-31-the-house-phases.md`](evidence/2026-08-31-the-house-phases.md),
whose `H1`–`H6` some comments in the tree cite by number. This document is the
decisions, and it carries neither.

> Read [`architecture.md`](../architecture.md) for where a system crate sits and
> what it may depend on, and [`style.md`](../style.md) before writing any of it.
> This document does not restate either.

## What a multi is, and what is already read

A **multi** is one item that draws as many. The wire carries a house as an
ordinary world item whose graphic is `0x4000 + id`; the client looks that id up
in its own `multi.mul` or `MultiCollection.uop` and draws the hundred and
forty-eight statics a villa is made of. **The shard sends none of them.**

That is the whole reason this is tractable. The picture is free — every client
already owns every house. What the shard owes is everything the picture does not
say: where the walls are for the purpose of stopping somebody, who may open the
door, what happens when nobody pays the upkeep.

[`openshard_uofiles::multi`](../../crates/common/uofiles/src/multi.rs) reads the
components. Three things about that format are written down in
[`findings.md`](../findings.md) and are not worth re-deriving: the High Seas
widening and the arithmetic that detects it, the drawn/skip flag that runs
*opposite ways* in the two files, and the fact that the two files disagree about
how many multis exist (326 against 862 on one install, so the UOP wins).

## Decisions, taken here

**D1 — a house is an entity with a `Multi` component, not a new kind of thing.**
It has a `Position`, a `Drawn` whose graphic is `0x4000 | id`, and a serial from
the item pool. Everything that already walks items — the sector index, the save,
the `0x1A` that draws it — works on it unchanged. What makes it a house is
a `House` component beside those, not a separate table.

**D2 — the footprint is an obstruction, computed at placement and stored.**
`FacetState::obstructions` is the index a step already asks. A house adds its
drawn components to it, each at its own `dz` with its tiledata height, exactly as
a static would be. Not computed per step from `multi.mul`: a step is ten a second
and a house does not move.

The consequence, accepted up front: **a house is not in the map file**, so the
obstruction index is no longer purely a function of the client's files. It is the
files plus what the shard has placed. That is already true of doors and dropped
items; a house is the first thing that adds a *hundred* entries at once.

**D2a — `openshard-state` never holds the multi table.** It does not depend on
`openshard-uofiles` and must not start: the components are resolved **at
placement**, by a caller that has the table, and what is stored is the
obstruction entries and the `House` component. At boot the saved houses are
restored by the boot code, which already reads the client's files. This is D2's
"computed at placement and stored" spelled as a dependency rule, and it is what
keeps a *client file* out of the crate every gameplay system builds on. As built
the caller that has the table is `Terrain` itself — `Terrain::multi_components`,
the same seam `item_blocks` and `item_height` already reach gameplay through.

**D2b — one entity blocks one tile at several heights.** `Obstructions::block`
keyed an obstacle by its entity, because the case it was written for was a door:
one thing, one height, re-registered to refine it. A house is one entity whose
walls stand on top of each other, so the key is the entity **and the z** — done,
with a test, before anything above it was written. Keyed by the entity alone the
second registration overwrote the first, which does not read as a missing wall
but as the wrong floor being sealed, since which one survived depended on the
order the components came out of the file in.

**D3 — the placement rules are ServUO's five, and the fifth is the one to get
right.** From `HousePlacement.Check`: nothing impassable around the outside, no
impassable tile touching the house, five tiles clear front and back, the
foundation rests flat, and **no foundation tile over a road**. The road rule is
the one a player notices the absence of, because without it houses appear across
Britain's streets.

Staff place anywhere, which is ServUO's own first branch and is what makes the
rules testable before the deed exists. That exemption was written here and did
not exist in the code for five phases — `place` took an owner `Serial` and had
nobody to ask `is_staff` about; D10 is what made the sentence true, and which
refusals it covers is D10's own table rather than "all of them".

**D4 — a region, and it does not come free.** `Regions` already exists and
already carries flags, so a house *can* be a region with its own flags (no
teleport in, no recall out), placed and removed with the house. Nothing new is
needed for the lookup — `Regions` is per-facet on `FacetState` and `Regions::at`
is a bucketed lookup that has worked the whole time.

What this decision originally said was "it comes free", and that is the half that
was wrong: a decision with no phase under it is a decision nobody is assigned,
and five phases went by without one line of `openshard-housing` mentioning a
region. **A house's own region is still not registered**, and what it needs first
is a decision rather than code —
[`plans/housing/house_region/PLAN.md`](../../plans/housing/house_region/PLAN.md).
The lesson is cheap and worth keeping: a "this comes free" decision needs a phase
to be free *in*, or it is not a decision, it is a hope.

**D5 — the door is the door this engine already has.** `.key` and `KeyValue` and
the lock rules landed with the traps work, on the argument that Britannia locks
exactly one container and the rules would otherwise be unreachable. A house door
is that mechanism with the house's own key, and the reason it is D5 rather than
a phase is that there was nothing to build — only to connect.

**D6 — decay is a tick count, not a wall clock.** Everything in this engine that
measures duration counts ticks, because a tick count replays and a clock does
not. ServUO's five days is a tick count in `Gameplay`, an operator setting like
every other duration. A house refreshed by its owner resets it.

What the decision did not say is which end of the interval to store, and the
answer is the other one from every other timer here: `House::age` **counts up**,
because `WorldState::ticks` starts at zero every boot and a deadline written as
an absolute tick would come back meaning nothing — every house on the shard
freshly refreshed after a restart.

**D7 — customisation is a system of its own.** ServUO's `HouseFoundation` and the
`0xD7` design packets are a second system the size of this one: a design buffer,
a preview state, a commit, and a whole editor on the client. A *classic* house —
placed from a deed, fixed shape — is the whole of this document. The foundation
ids (`0x13EC`–`0x1D00`) are named here so that the placement code refuses them
loudly rather than placing a house with no stairs; what a designed house is
instead is [`design_customisation.md`](design_customisation.md).

**D8 — the moving crate is not deferred, it is the deletion rule.** When a house
goes, what was inside it has to go somewhere, and "somewhere" being the ground is
how a shard loses a player's belongings. ServUO's moving crate is a container the
contents land in. It is small and it belongs to the phase that can destroy a
house, not to a later one.

**D9 — the rule is stated over the whole footprint, not over the origin.** A
house is many tiles and a region is a set of rectangles with a height band, so a
villa can straddle a dungeon mouth. Testing the origin alone would let a player
build a house whose back half is inside Shame by standing one tile outside it —
and the failure is invisible until somebody notices the walls. `check_ground`
already walks every footprint tile for the road and `can_fit` rules; the region
check walks the same list, and one tile inside a `no_housing` region refuses the
house.

There is a third reason and it is the one that would have bitten first: **the
origin is not reliably part of the house at all.** `place`'s own doc says so —
`at` is the multi's origin and "is not the corner of its box". A multi whose
components all sit at positive `dx`/`dy` has an origin outside its own drawn
area, so an origin test can test a tile no wall ever stands on.

The set walked is **`tiles_of` and not the footprint**. A floor is inside a
dungeon as surely as a wall is, and `footprint_of` deliberately drops everything
that does not block. This costs nothing: `place` already calls `tiles_of` to size
the lockdown allowance, so hoisting that one call above the checks gives both
readers the same list.

The cost to name: `Regions::at` is a bucket lookup plus a linear rectangle test
per candidate, run once per covered tile rather than once. A hundred lookups when
somebody clicks is the bargain `check_yard` already takes.

**D9a — the height tested is the house's own `z`, once, and never the
component's.** `RegionRect` carries a `z_min`/`z_max` band and 247 of the shipped
rects use one — that is what keeps the open sky above a dungeon open. A villa's
roof stands twenty units above its foundation, so testing each tile at its
component's z would put the roof *outside* a banded dungeon region and answer
"not in Covetous" for the top half of a house that is unambiguously in Covetous.
A house is sited at one height.

**None of the 21 `no_housing` regions is banded today**, so this decision changes
no answer on the shipped data. It is written down because the failure it prevents
is a player building a tower in a banded dungeon, and that is discovered by the
player rather than by a reader.

**D9b — the region refusal comes before the ground refusal.** The order of the
checks is the *message*, not an implementation detail. `BadGround` means "try a
tile over"; inside Deceit that sentence is a lie, and a player who believes it
spends ten minutes proving it. As built the same argument applies word for word
to `Occupied`, so the region check sits above every judgement about the plot
rather than between two of them: it is the only refusal that is a statement about
the *place* rather than about this attempt.

**D10 — `place` takes the actor, not just the owner.** It cannot ask `is_staff`
about a `Serial`, which is why D3's exemption had never existed. The signature
carries an actor `EntityId` and the exemption is the shape every other one in
this engine has — `if state.is_staff(actor) { … }` as a first branch, as
`WorldState::may_teleport` and `magic::travel::may_travel` both do.

**Which rules the exemption covers is its own decision, and it is not "all of
them".** ServUO's is a single early return — `if (from.AccessLevel >=
AccessLevel.GameMaster) return Valid;` — and copying that shape literally would
be wrong here, because this engine's `Refusal` mixes two kinds of answer:

| refusal | what it is | staff-exempt |
|---|---|---|
| `Occupied`, `OnARoad`, `BadGround`, `TooCloseToAHouse`, `NoHousingHere` | a judgement about the plot | **yes** |
| `NoSuchMulti`, `DrawsNothing`, `NeedsCustomisation`, `OffTheMap`, `NoSerials` | there is nothing to place, or the shard is broken | **no** |

Exempting the second row spawns an invisible house out of a treasure-site marker,
or a foundation with no stairs — which is the exact failure `NeedsCustomisation`
exists to prevent. **A staff bypass that reopens a hole another decision closed
is not an exemption; it is a regression with a permission check on it.**

The `staff: bool` is computed once at the top of `place` and threaded, which is
this crate's own idiom: `trust`, `distrust`, `ban`, `unban` and `standing_of` all
take it that way rather than each asking `is_staff` again.

## What this does not cover

- **Boats.** They are multis too and this reader already reads them, and a boat
  is a house that *moves*, which is a different problem: every component's
  position changes together and the obstruction index has to follow. It is
  [`design_boats.md`](design_boats.md).
- **A designed house.** D7, and it is [`design_customisation.md`](design_customisation.md).
- **A house's own region.** D4's other half, and it is
  [`plans/housing/house_region/PLAN.md`](../../plans/housing/house_region/PLAN.md).
