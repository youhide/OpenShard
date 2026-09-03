# A house that moves

[`design_house.md`](design_house.md) deferred boats in one sentence and it is the
right one: *"a boat is a house that **moves**, which is a different problem:
every component's position changes together and the obstruction index has to
follow."*

Every hard decision below follows from the word **moves** rather than from the
word **boat**. The multi reader already reads them; the picture is free the same
way a house's is; the placement rules are housing's with the sign flipped on one
of them. What is new is that a boat's shape is somewhere different every few
seconds, and nothing in this engine was built for that.

**What is built and what is open is [`README.md`](README.md)**; how it was built,
and the two surveys that decided B5, are
[`evidence/2026-08-25-the-boat-phases.md`](evidence/2026-08-25-the-boat-phases.md).
The phases there are numbered `B1`–`B4` and the decisions here `B1`–`B7`: the
same numeral means two different things, which is an artefact of one document
having held both.

> Read [`design_house.md`](design_house.md) first — the decisions about multis,
> footprints and the obstruction index are assumed here rather than restated, and
> two of them turn out not to survive contact with a thing that moves.

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

**Being aboard turned out to be three questions rather than that one**, and the
two this decision did not ask each cost a defect: feet on a plank rather than
merely on a covered tile, and a plank of *this* ship rather than of the one
moored alongside. `Boats::carries` is the named half; the record has both.

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
  at the call site in `tick.rs` — the existing idiom, beside `collapse_houses`,
  which is its nearest neighbour in kind. A redraw every N ticks is a boat that
  shudders; a redraw every tick is a boat nobody can look at.

**The number this asked for by name, as measured:** two packets per client that
can see the ship — a `0x1D` and the `0x1A` that draws it again — plus, per
occupant, one `0x20` to its own client and one `0x77` to each client watching it.

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

So: a per-facet `Boats` index, consulted by `LiveTerrain`, which is already the
composition seam (map + obstructions) and is exactly the shape a third source
belongs in. What the index holds is not "entity → origin plus multi id" as this
decision first said, but what the art *lays* — a floor, a solid body, or both, at
what height — derived once at the mooring, so the hot path is a hash probe rather
than a component walk. The split is `is_platform()`, the same reading
`Cover::of_static` makes everywhere else on this shard, and it is ServUO's own
test: a candidate to stand on must carry `Surface` and not merely fail to carry
`Impassable`.

**The hot path, measured rather than assured:** release, 100,000 steps, 1.5ms
with no boats against 5.5ms with one moored — 15ns against 55ns, where the
fixture's own `can_step` is a single integer comparison and the probe is
therefore nearly all of the measured work.

### B4 — `MapTerrain::swimming` stays false, and is not deleted.

`swimming` (`movement/src/terrain.rs:65`) is a property of a *terrain*, and a
facet has one. Setting it true makes water walkable for **every mobile on the
shard** — that is not "boats work", it is "everybody walks on water". It is
documented "A boat or a fish says yes", has never been set true on any server
path, and false is the correct state for it.

What a boat needs is two narrower answers, and they are different questions.

**(i) May a boat be placed here?** A water test at placement, `is_road`'s shape
with the sign flipped. But there were already **two** notions of water in this
tree and neither was reachable from where a boat would ask:

- `TileFlags::is_water()` in `openshard-uofiles` — the client's own truth —
  reachable only from inside `MapTerrain::land_is_ground`, which is private.
- `WATER_TILES`, id ranges generated by `state/build.rs` and consumed by fishing.

Writing a third is what `style.md`'s "look for it before writing it" forbids, and
the fix is small: **`Terrain::land_is_water(tile) -> bool`**, defaulting `false`,
implemented on `MapTerrain` over the flag it already reads. That is the seam
`item_blocks`, `item_height` and `multi_components` all came through, and it
answers "what if the shard has no client files" for free the way every other
method on that trait does. Fishing's `WATER_TILES` is still a second truth rather
than that seam's fallback.

**(ii) May a mobile stand on the deck?** Not a water question at all. A deck is a
climbable platform static at a z above the water, and `MapTerrain::check` already
stands bodies on platform statics — it simply never sees this one, because a
multi's components are not in the map file. That gap is B3's index, and it is
B3's *positive* half.

### B5 — the deck is the open pier/bridge bug, and this is where its repro came from.

[The pier-and-bridge investigation](../world/evidence/2026-08-24-the-movement-surface-investigation.md)
records the movement defect: `MapTerrain::check`'s `landCheck` guard, ported
variable-for-variable from the reference and audited rather than slipped,
discards a climbable platform static when the land beneath it is walkable and its
average height reads close to the deck. What saves piers and bridges today is
that they sit over water, where `land_is_ground` is false and the guard never
fires.

This decision predicted that turning `swimming` on would fire that guard under
every deck and drop a boarding player into the sea. **Both halves of it were
measured, and the prediction was half wrong and half right:**

- The guard was never the mechanism. It fires only where the land is *higher*
  than the deck, so discarding a deck moves a body **up**; and a moored ship is
  not in the map's statics at all, so that loop never sees one.
- The outcome was real by another route. With the flag on, a swimmer alongside a
  hull reaches the waterline plus two and used to end up **under** the deck — 890
  of 8,450 attempted boardings, on the reading that has since been retired.

What closed it is not the flag but the deck's **blocking half**: a plank three
units thick is three units of solid wood starting at the waterline, so there is
no gap under it to float in, and a swimmer can no longer clamber aboard over the
gunwale either — which is UO's own answer, since you board over a plank this
shard has not built. **So the flag stays off and B4 does not move.** The numbers
are in the record.

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
B4's own water rule instead. A house may not go on water; that is one mechanism,
not two. Named so nobody adds the second.

## What this does not cover

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
  fix is a deliberate deviation from the reference's `Movement.cs`.
- **A translate or bulk API on `Obstructions`.** B3, and the reason is written
  down so the next reader who notices the missing API knows it was declined
  rather than overlooked.
