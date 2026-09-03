# The creature brain

The model behind `crates/server/ai`: what a creature decides on its beat, what it
is allowed to know when it decides, and what it is allowed to keep from one beat
to the next. Every number here is a constant in `ai`'s own source with the
reasoning beside it; this page is the shape they hang on.

What a creature *looks* like and where it came from is
[`design_townsfolk.md`](design_townsfolk.md); what happens once it swings is the
[`combat`](../combat/README.md) domain's.

## A brain only decides

`think_one(state, creature) -> Beat` is the whole of the seam. It reads the
world, works out whether the creature should fight, chase, flee, kite, cast or
drift, and turns that into at most one thing: a `Beat`, handed back to the
caller. Three consequences follow, and they are why the crate is a crate:

- **Engaging is done directly**, because engaging is not a step. The brain hands
  the creature a `Combat` aimed at its quarry, and `combat::swings` fights with
  it using exactly the machinery a player's attack uses. There is no creature
  attack path.
- **Stepping is left to the world**, because a step is bound to terrain, to the
  walk budget and to the announcement that follows it. `world::tick` calls
  `step`. `ai` never moves anything.
- **Casting is left to the world too**, and for the same kind of reason: the cast
  sequence — the refusals, the mana, the skill roll, the gesture, the resist and
  the effect art — lives on `World`, and it is the one a player's cast goes
  through unchanged. `Beat::Cast` names a spell and a mark and `world::tick`
  calls `begin_creature_cast`. There is no creature spell path either.

`Beat` is an enum and not two fields because its arms are alternatives: a
creature that is casting is standing, and one that is stepping is not casting.

The decision spends the world's seeded `Rng` and reads `state.ticks`, never a
wall clock. A populated facet therefore replays: the same seed puts the same
creature on the same tile on the same tick.

## The beat, and why it is not the tick

`think_one` is the most expensive per-mobile work the tick has — it scans
sectors, casts a sight ray and may plan a path — so a creature thinks on a beat
rather than every tick, and the beat of a crowd is deliberately not in phase. A
fresh mobile's first beat is jittered across its interval and a restored one's is
jittered again, because a town whose every soul beats on the same tick is both a
spike in the tick and a tableau that moves in lockstep on the screen.

## Four phases, in order

Each beat runs the phases in order and stops at the first one that has something
to say.

1. **Fight.** There is a quarry. It is dropped if it died, fled the facet or
   drifted past the chase limit; otherwise the creature flees, **casts**, kites,
   closes, and swings when it is in reach — in that order, so a caster throws
   rather than closes and a mage with a bow is a mage first.
2. **Acquire.** No quarry: look for one. The nearest player within `Sight` that
   the creature can actually *see* becomes the quarry.
3. **Wander.** Nothing to fight and a `wander` flag: a three-in-eight chance of a
   step, low enough that a field of creatures drifts rather than marches.
4. **Nothing.** A creature with neither sight nor wander stands where it was put,
   which is what a shopkeeper's guard dog and a set-piece animal want.

## Sight is a ray, not a radius

Acquisition is gated on `Terrain::sight_clear` — a Bresenham ray at one eye
height, the same walk the ranged shot is allowed by
([`combat/design_sight.md`](../combat/design_sight.md)). A window passes, a wall
and a `NO_SHOOT` static do not, and a shut door is opaque at every height. So a
creature does not notice prey through a closed door, and a player who shuts one
behind them has done something.

Sight bounds acquisition; it does not bound the chase. A creature chases to twice
its own sight, with a floor of twelve tiles so that a defensive animal with no
hunting sight of its own still answers whoever hit it.

It bounds the **cast**, though, and with the ray as well as the radius. A spell
has no range of its own in this engine, so the honest bound on a creature's is
what it can pick out — the same number the fight was started on — and a bolt does
not bend round a wall any more than an arrow does. A caster that could throw
through a keep would be fighting from somewhere the player cannot fight back
from.

## A creature that casts

Two components make one, both authored by the spawn (`data/spawns.json`, the
`mana` and `spells` columns): a `Mana` pool and a `Repertoire` — the spells it
knows, in the same bit mask and the same numbering a player's spellbook uses. A
creature carries no book and no pack, so its spells sit on the mobile; nothing
can be stolen off a lich to stop it casting.

The choice is four questions and a pick: does it cast at all, is it off the
recovery its last spell armed, is the mark in sight with a clear line, and what
can it pay for. The pick is the strongest thing it can afford that is aimed at a
mobile — which is a rule and not a placeholder, though it is a thin one: harm,
heal, curse and escape as *categories* are the next phase of
[`plans/npc/creature_casting/PLAN.md`](../../plans/npc/creature_casting/PLAN.md).

Below the decision there is no creature-shaped code at all. `begin_creature_cast`
hands `start_cast` a mark instead of a cursor to raise, and everything from the
spellbook gate down runs unchanged — which is why a creature's cast fizzles,
resists, disturbs and draws its art exactly as a player's does. Two rules do read
"is there a person behind this mobile", and both are ServUO's own: a mantra is
said only by a player (`Spell.SayMantra` ends with `m_Caster.Player`), and
reagents are consumed only from one (`Spell.ConsumeReagents` returns true for
anything else).

A cast roots its caster the way it roots a player: a creature holding a `Casting`
stands, and its beat is spent doing so.

## Two searches, in the order the client asks them in

A step toward something is planned, not walked at. The exact search
(`movement::find_path`) is bounded by `PATH_BUDGET` — 400 finalised nodes — which
is ample to round a building and is deliberately a bound on *work* rather than on
distance: with the span layer a column with two floors can be finalised twice, so
node count and tile count are not the same measurement.

Past that budget the question is not asked again with a bigger number. It goes to
the baked navigation graph (`find_long_path`, over `COARSE_MIN_DISTANCE` tiles),
which holds the facet's whole connectivity: a route across a town costs a
corridor of region hops instead of the tiles between here and there. **The
corridor is the only thing the bare map decides** — every hop of it is refined
through the live ground, so a crate dropped in a doorway still refuses the step
it is standing in.

That is the same fall-back, in the same order, that the client walks a click by.
One planner, two readings.

## A route is kept, and what it is walking decides for how long

A search that arrives has said something about every tile between here and the
goal. Keeping only its first step and planning again next beat means a body
walking twenty tiles plans twenty routes to walk one step of each, so the whole
way is written on the body as a `Route` and followed a step per beat.

`Goal` is the caller's statement of *what* is being walked to, and the only thing
it decides is whether the kept route carries a time window:

- **`Goal::Moving`** — a quarry, an owner, a master to trail. The route lapses
  after `REPATH_TICKS` (two seconds, written as `2 * TICKS_PER_SECOND` rather
  than a bare tick count, because it is a span of real time and the bare `40` it
  used to be became one second the day the tick halved).
- **`Goal::Place`** — a post, a night home, a destination. No window at all: the
  route is walked to its end.

What a window buys is *noticing a better way*, and nothing else. Every other way
a kept route can go wrong is caught without a search — the body is not standing
where the next step starts (`Route::at`), the goal has drifted past `GOAL_DRIFT`,
the steps have run out — and the ground having changed under the next step is
caught by `probe`, which puts every step of every route to the live world before
it is taken. For a townsperson walking home, a window buys an answer nobody asked
for at the price of a whole `PATH_BUDGET` search.

## A refusal is remembered, because it is what costs the most

A pet following an owner behind a locked door, a townsperson whose post is walled
off, an escortable trailing a master across a bridge that is not there: each asks
the coarse graph the same unanswerable question every beat and pays the whole
endpoint join for it. So a refusal is written on the body as a `RouteRefused`,
and while it stands the graph is not asked about that goal again. A goal that
moves further than `GOAL_DRIFT` is a different question and clears it, exactly as
it invalidates a `Route`.

`REFUSAL_TICKS` is ten seconds, and the *floor* under that number is that it must
outlive one beat of everything that reads it — a memory that lapses inside a
townsperson's beat has always lapsed by the time the body wakes up to use it.
It happens to equal the ten seconds a chaser stands watch after giving up, and
the two are deliberately **not** written as one constant: a shared value arrived
at separately is the collision this rule exists to prevent.

A body with nowhere to keep an answer asks `step_toward`, which is a pure
function of the world; a body that can remember asks `step_body_toward`. The
difference is not a tuning knob, it is which of the two can afford a refusal.

## A route is planned around the crowd

Before the crowd was in the plan, a route was planned over ground with nobody on
it and then walked one step at a time over ground that had somebody on it: the
step was refused, the next beat re-decided the same direction, and nothing ever
went round. (A crate worked, because a crate is in the overlay and the plan could
see it.)

So a plan is decided against the bodies within `CROWD_REACH` — thirty-two tiles,
a screen and a half, comfortably past view range. What the bound costs is a
re-plan, never a wrong step: a body outside it is invisible to the route, the
route walks into it, that step is refused by its own crowd read fresh, and the
next beat plans again with the body now inside the reach.

**The goal tile is dropped from the crowd**, and ServUO drops the same one. A
creature's goal is overwhelmingly the quarry, which is itself a body; leaving it
in makes every chase unplannable, because the one tile the route is *for* is the
one tile it may not end on. Arriving *beside* the quarry is the caller's
business.

## A door in the way

`way_ahead` is one rule and it is applied on whichever step of a route meets a
door: a body whose type has hands (`body_opens_doors`, ServUO's
`!Body.IsAnimal && !Body.IsSea` read off the ported body table) opens an unlocked
one and walks through. A lock is a refusal at that same door, for the AI as for a
player — without which a townsperson strolls through a locked shopfront and the
lock is decoration.

## Giving up is a posture, not a failure

A chase with no way through ends in `give_up`: the quarry is dropped and the
creature stands watch for ten seconds before going back to its life. It is never
the fence-shuffle — a creature pressed against a wall re-deciding the same
direction forever — and a quarry that becomes reachable is re-acquired the normal
way.

## Postures

`Aggression` is what a creature does about a foe, and it is spawn data:

- **Passive** fauna flee when struck.
- **Defensive** creatures answer the first blow, through `retaliate`, which reads
  the blows combat announced rather than being called by it.
- **Aggressive** creatures hunt on sight.

All three break off badly hurt unless they are too big to scare — `BRAVE_HITS`,
ServUO's "five hundred hits does not flee".

A creature with a `ranged` reach fires through `combat::volleys`, which shares
the swing timer and is gated on the same sight ray, and holds `KITE_GAP` instead
of walking into melee.

## Pets and summons are the same brain with an owner

`pet_beat` is `think_one`'s sibling: a controlled creature heels, answers its
orders and fights what it is told to. A **summon** is a pet with a deadline —
everything a controlled creature does already exists as `Pet`, so the `Summoned`
marker beside it carries only the tick it vanishes on. Nothing that follows,
obeys or counts followers had to learn a second kind of creature.

## Level of detail, and the wake that has to come with it

In a populated world most creatures are nowhere near a player and nobody sees
what they do, so the `[gameplay] lod` flag skips their beat: a creature with no
player within `lod_radius` and not already in a fight does not think, and its
next beat is pushed out by `lod_idle_factor`. The gate reads `WorldState::
any_player_near`, which walks the player table rather than the sector grid
because players are few.

Two properties make it safe rather than merely cheap:

- **`lod_radius` sits above the view range and above the largest sight**, so a
  creature a player can *see* is never dozing. "No player near" implies "no
  player in sight", so nothing is missed by skipping.
- **A fight is never dozed**, or a fight would freeze the moment its target
  stepped one tile away.

The saving is only half of it. Nothing woke a dozing mobile at first — it simply
finished a long timer set while nobody was there, so a player walking into a town
found a still tableau that burst into life up to sixteen seconds later, all at
once, because mobiles that doze together wake together. The answer is Sphere's:
wake by *event*, on a sector crossing, and re-arm each woken mobile to a random
short delay. A player crosses a sector boundary once every sixty-four tiles, so
diffing the sector each player stands in costs one lookup per player per tick —
the same find-it-by-diffing shape `tick/regions.rs` and `tick/status.rs` use, and
for the same reason: a call beside every mover is a call somewhere it is
forgotten. The re-arm only ever pulls a beat *forward*; a mobile already beating
at its live rate keeps the beat it has, or every crossing would reset a whole
block's timers to one instant.

Spawners take the same gate: a spawn region no player is near stays dormant, its
timer held, until somebody approaches.

Determinism holds through all of it, because the gate reads only `state.ticks`
and positions.
