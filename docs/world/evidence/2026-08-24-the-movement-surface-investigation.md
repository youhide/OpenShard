# Movement surface investigation

> A record. It was part of the roadmap's world phase until 2026-09-02; what is
> open in this domain is now ranked in [`world/README.md`](../README.md).

## Closed: a pier or bridge over low ground can drop a walker under it — the mechanism is refuted

`MapTerrain::check`'s `landCheck` guard (`movement/src/terrain.rs:207-217`) is
ServUO's own `Movement.cs` `landCheck`, ported variable-for-variable and
direction-for-direction — audited against the reference, not a porting bug. It
exists to discard a low decorative static the terrain visibly pokes through (a
rock embedded in a hillside): when the land under a platform static is walkable
and its average height (`land_center`) is close to or above the static's own
stand height (`our_z`), the static is dropped from the candidate list and the
walker falls through to the land instead.

ServUO's own `landCheck` does not exempt `Bridge`/climbable statics from this
either — the flag only changes `itemTop` (how high a step must reach to clear
the static), never the guard itself. That is fine as long as a bridge or pier
sits over water, where `land_is_ground` is false and the guard never fires. It
is not fine at the shore end of a pier or the bank end of a bridge over a
ravine, where the ground underneath is ordinary walkable land whose average
height can read close to the deck: the guard fires, the deck static is
discarded, and the walker lands on `land_center` — which for a structure
spanning a drop is often well below the deck. That reads as "fell under the
bridge," and matches a player report (2026-08-02) of falling underground
specifically on piers and bridges.

Not fixed yet because it is a real divergence from the cited reference, not an
arithmetic slip, and needs a decision rather than a silent patch: exempting
`is_climbable()` statics from this guard would be a deliberate deviation from
`Movement.cs`, and wants a repro against real client files (a pier whose shore
tiles are dry land, not water) confirming the fall before touching it.

**Re-evaluated 2026-08-16 and kept**, so the next reader knows it was looked at
rather than merely untouched. Two things changed around it and neither moves the
decision. [`boats.md`](../../housing/design_boats.md)'s B5 found that the repro **does not need client
files** after all — a synthetic multi carrying a climbable platform component at
a known z over land of known height reproduces the shore-end case, and
`Multi::new` is public. And it found a second consequence: turning
`MapTerrain::swimming` on would fire this same guard under every boat deck and
drop a boarding player into the sea, which is why that flag stays false. The
deviation is still a deviation, and taking it is still a decision nobody has
taken.

### 🚩 The repro was finally run, 2026-08-23, and **this mechanism does not exist**

Two surveys over the whole of facet 0, both `#[ignore]`d in
[`terrain.rs`](../../../crates/common/movement/src/terrain.rs) —
`land_check_survey` and `predicted_step_survey`. Neither is an assertion, for
`boat_step_cost`'s reason: an assertion over a facet's worth of shipped art is
an assertion about the art.

| | |
|---|---|
| the guard discards a platform | **2,381** pairs of (tile, static); **596** of them climbable |
| and the body then lands **below** the surface it discarded | **0** |
| and the tile is refused outright instead | 722 (378 climbable) |
| of those, walled *by the guard* — the body would have fit | **242** (71 climbable) |

**The fall cannot happen, and the guard's own third condition is why.**
`landCenter > ourZ` means the guard only ever fires where the *land is higher
than the deck*. So discarding the deck moves the body **up** onto the land, never
down — which is the opposite of what this entry has claimed since it was
written. The 2026-08-02 report is real; `landCheck` does not explain it.

**And the client is not dropping the body either.** The second survey walks
every step a body can take off a bridge or pier — 224,950 of them that the shard
*allows* — and compares `predict_step`, which is what the client draws
immediately, against `check`, which is what the shard decides. Only permitted
steps count: a refusal comes back as a `0x21`, which carries x, y **and** z, so
the client is corrected and never shows it. The permitted step is the one that
goes uncorrected, because its `0x22` carries no position.

| | |
|---|---|
| permitted steps off a bridge or pier | 224,950 |
| client and shard disagree at all | **77** (0.03%) |
| client draws the body **lower** | **0** |

**What the guard actually costs is 242 tiles a body cannot enter** — an invisible
wall, not a fall — and **that is parity, not a defect**: the port is
character-for-character ServUO's
(`Scripts/Services/Pathing/Movement.cs:238`, `landCheck = itemZ; if (Height >=
StepHeight) landCheck += StepHeight; else landCheck += Height;` and the same
four-clause guard), so the same 242 walls stand on a ServUO shard.

**So the decision this entry was holding itself back for is not owed.** Nothing
should exempt climbable statics from the guard on the strength of a fall that
does not happen. What *is* still owed is the report's real cause, and these two
surveys say where not to look. The suspects left, in order:

- ~~**A boat moored at a pier.**~~ **Walked 2026-08-24 — real, and two units
  deep.** See the subsection below; the suspect held a defect, the defect is
  fixed, and it is still not this report.
- ~~**Arriving rather than walking** — a login, a spawn, a gate or a teleport
  onto a deck, which reach `spawn_z` and not `check`.~~ **Surveyed 2026-08-24 —
  real, and by far the largest of the three.** See the second subsection below:
  a body arriving on a pier or a bridge was put a median ten units under it on
  **25,816 of the facet's 27,052 decks**. Fixed, and it is *still* not this
  report — nothing a player does reaches the rule that did it.
- **A multi-step walk**, where a single step is right each time and the sequence
  drifts. Both surveys measure one step from a known surface. **The only suspect
  left on the shard's side** — and see the last subsection for a fourth that is
  not on the shard's side at all.

### 🚩 The first suspect was walked, 2026-08-24, and it does not reach

`openshard-boats`'s [`moored_boat`](../../../crates/server/boats/tests/moored_boat.rs)
is the shard-side sweep with a live overlay this entry asked for. Over facet 0:
**400** pier and bridge decks with sea beside them, **260** with room for a
small boat within four tiles. **One ship per pier, each in an overlay of its
own** — a harbour-wide pass would have to arbitrate between piers competing for
the same water, and whichever pier lost would go unmeasured, which is a cap on
coverage disguised as a fixture. Every step off the pier through `step_allowed`,
asked twice: of the reading the shard has and of the one it retired the same
day.

| | the ship makes legal | onto its deck | **under its deck** |
|---|---|---|---|
| the reading now | 352 | 300 | **0** |
| the reading retired | 403 | 298 | **3** (worst 2) |

**A moored ship did put a walker under a deck**, and the mechanism was not
`aboard`'s reach — that was fixed on 2026-08-23 — but a *second reading of the
ship's own art*: `Plank` split a component on `is_blocking()` alone, so every
rope and rudder in the multi table became a floor, some of it two under the deck
beside it. Eighty such floors across the shipped fleet. That is now
`Cover::of_static` like every other placement, which is ServUO's own test
(`Movement.cs:211`, `(flags & ImpassableSurface) == TileFlag.Surface`). See
[`boats.md`](../../housing/evidence/2026-08-25-the-boat-phases.md)'s correction to
the plank's own reading.

**Three times over a facet, two units deep, is a defect worth the fix and is not
this report.** A player who falls underground on a pier is not two units low.
The two remaining suspects are unchanged, and the cause is still unknown.

Two things fell out of the same walk:

- **What a real sloop's deck stands at over real water**, which `boats.md`'s own
  fixture calls an open question: a pier stands **−7..7 above the deck a body
  boards onto, median 7**.
- **`boats.md`'s other open question, answered** — see its B4. With
  `MapTerrain::swimming` on, a swimmer alongside a hull was predicted to end up
  in the sea under its own deck. It was right: **890 of 8,450** steps, against
  **0** now, because a deck's blocking half starts at the waterline.

#### Found while walking it

- **`Boats::deck_at` is a third spelling of `Overlay::surface_at`'s rule** —
  nearest surface to the body — without its reach filter or its
  tie-goes-to-the-lower, and `Boats::carries` is a fourth. Both are answered by
  the overlay in production now; `deck_at` and `blocks_at` have no caller left
  outside tests. Either they read the overlay or they go.
- ~~**This client has no notion of a boat at all.**~~ **Repaired 2026-08-25:**
  every known multi is expanded into components, `clutter::project` lays those
  into the live overlay, and both walk arms predict from the complete `Footing`.
  A ship deck and a player-house stair therefore reach the same immediate `z`
  prediction as the shard's step rule.
- **A ship can be moored through a dock.** `check_berth` asks only that every
  berth tile is *water*, which a tile carrying a pier plank can be. **52 of the
  352** boardings in the survey land on the plank rather than on the deck under
  it. Harmless as a landing and wrong as a placement.

### 🚩 The third suspect was surveyed, 2026-08-24: an arrival is not a step

`movement`'s
[`arrival_survey`](../../../crates/common/movement/src/terrain.rs) asks the shard's
*placement* rules the one question this entry is about: over facet 0, for each
of the 27,052 pier and bridge decks, where does a body that **arrives** there
end up? A step goes through `check`, reaching from the top of the art underfoot.
An arrival has nothing to reach from, so the shard had **four other spellings of
it**, and none of them was the same rule.

| the rule | who arrived through it | on the deck | **under it** | refused |
|---|---|---|---|---|
| `MapTerrain::ground_z` | a fresh character's first tile (`start_position`), the `.go` command, the region spawner's seed | 808 | **25,816** (median 10, worst 67; 6,266 of them over open water) | 0 |
| `MapTerrain::spawn_z` | `npc::spawn`, seeded from the ground | 18,862 | 3,139 (median 23) | 3,450 |
| `MapTerrain::stand_z` | the arrival test a recall, a gate travel and a sacred journey are approved by | 21,255 | 0 | 4,593 |
| `housing::doorstep` | a banned player put out of a house | — it named `at.z`, the house's own floor, and asked nothing | | |
| **`movement::arrival_z`** | **all four now** | 18,868 | **3,132** (only **7** of them over water) | 3,450 |

**`ground_z` is the land tile's own average and reads no static at all**, which
is the whole of the first row: on a pier it answers the sea, on a bridge it
answers the ravine. And **none of the four read the
[`Overlay`](../../../crates/common/map/src/overlay.rs)**, which is the same shape the
moored-ship subsection above found one layer down — a body put on a deck or on a
house's first floor lands in the sea or in the ground *by construction*, because
the only layer that knows those exist was not asked.

`arrival_z` is the one rule they go through now: `spawn_z`'s two arms with the
live layer folded into both — the ordinary landing taken **in place** (so a
placement finds the ground floor and cannot climb to the storey above), then
every surface either layer has, filtered by `can_fit`, nearest to the height
asked about with a tie to the lower. `can_step` for the first arm rather than a
second copy of it: *put here* is a step that goes nowhere.

**The 3,132 that remain are the seed's answer and not the rule's.** All but
seven are bridges over walkable land, where the ground under the bridge is what
the caller asked to be placed *near* — a spawner names a rectangle and no storey,
so the ground is the honest seed and a rat belongs under the walkway. The seven
over water are worth a look and are not a fall a player can take.

**And it is still not this report.** Every caller of `ground_z` is staff or
server-side: the configured start tile, `.go`, and where the spawner looks. A
player walking around Britannia reaches none of them. What the survey *is* worth
is the 25,816: a `.go` onto any dock put a game master in the water, and that is
now a rule rather than four.

#### Found while surveying it

- **A moongate crossing is not checked and a recall is.** `travel_through`
  (`world::tick::gates`) calls `move_to` with the destination verbatim; recall,
  sacred journey and gate travel all pass `can_stand_at` first. ServUO's
  `Moongate.UseGate` is verbatim too, so this is parity rather than a defect —
  but the two paths arriving under different rules is worth writing down before
  somebody assumes otherwise.
- **`.tele` honours the z the client picked.** Deliberate for staff, and the one
  arrival that *should* name a height: a game master clicking a spot means that
  spot. Left alone.
- **[the client's backlog](../../client/evidence/2026-08-30-the-client-backlog.md) has already attributed this report, twice, and the
  roadmap's suspect list never mentioned it.** Its "found while drawing the
  ground" and "found while joining the window to the wire" entries call the
  2026-08-02 report one bug with two client-side causes: `GroundQuad` builds its
  four heights from the **land layer only**, so a pier's deck has no ground plane
  of its own, and `Walk::step`'s predicted z came from the same place. The second
  half is fixed — `ui_command.rs` predicts with the full
  `movement::predict_step(Footing, ...)` on both the online and offline arm, so
  runtime floors, stairs and decks participate too — and the first is **still true today**
  (`ground.rs`'s `corners` is `WorldMap::land_corners`). Whether that draws as
  *sinking* is not obvious and wants a look rather than an argument: the body is
  drawn above the plane, not below it, so the visible failure would have to come
  from the depth sort putting the plank in front of the body. Two documents have
  been holding two different theories of one report; whoever takes the last
  suspect should read both first.
