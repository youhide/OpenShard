//! Creature behaviour: what a brain decides to do with its beat.
//!
//! A brain only *decides*. [`think_one`] reads the world, works out whether a
//! creature should chase, fight, cast or drift, and turns that into at most one
//! thing: a [`Beat`], returned to the caller. Engaging a foe it does itself (it
//! hands the creature a [`Combat`], and `combat::swings` fights with it exactly as
//! a player would); moving it and casting it leaves to the world, which owns the
//! step and the cast sequence both. So `ai` reuses combat, movement and the spell
//! table — it never reimplements them.
//!
//! The decision uses the world's seeded [`Rng`](openshard_state::Rng), so a fight
//! or a wander replays identically.

use openshard_combat as combat;
use openshard_combat::MobileDamaged;
use openshard_entities::EntityId;
use openshard_items as items;
use openshard_magic as magic;
use openshard_map::overlay::Doors;
use openshard_movement::{
    COARSE_MIN_DISTANCE,
    SearchExit,
    Weight,
    direction_toward,
    find_long_path,
    find_path,
    search_path,
    step_from,
};
use openshard_protocol::casting::SpellId;
use openshard_protocol::direction::Direction;
use openshard_protocol::serial::Serial;
use openshard_protocol::world::{
    Facet,
    Point,
    Sight,
};
use openshard_state::components::{
    Aggression,
    Brain,
    Casting,
    Client,
    Combat,
    Heading,
    Hitpoints,
    Mana,
    Pet,
    PetOrder,
    Position,
    RangedAttack,
    Repertoire,
    Route,
    RouteRefused,
    SPELL_COUNT,
    Spellbook,
};
use openshard_state::sectors::{
    distance,
    in_range,
};
use openshard_state::{
    WorldState,
    WorldTick,
};

/// The chance in eight, per beat, that an idle wanderer takes a step. Low enough
/// that a field of creatures drifts rather than marches.
const WANDER_IN_EIGHT: u32 = 3;

/// How many *nodes* an exact chase plan may finalise before it gives up.
///
/// Ample to round a building; an unreachable quarry is not worth more. It is
/// not a reach: since `docs/world/evidence/2026-08-25-the-span-layer.md`'s N3b a column with two
/// floors can be finalised twice, so this bounds the work rather than the
/// distance — and past it the answer comes from the coarse graph instead of
/// from a bigger number. See [`step_toward`].
///
/// Public because it is the subject of an assertion and not only a knob: the
/// test that pins the fall-back has to say which budget the exact search was
/// refused at, and a copy of `400` in the test would be a second place to
/// change it.
pub const PATH_BUDGET: usize = 400;

/// How far from the planner a route looks for bodies to walk around.
///
/// A plan is decided against the crowd within this of where the body is
/// standing, and the crowd is read out of the sector grid at the question
/// ([`WorldState::crowd_near`]). Bounded because the sweep is not free and
/// because the far half of a long route is not really being planned here: past
/// a few dozen tiles the exact search runs out of [`PATH_BUDGET`] and the coarse
/// corridor takes over, and the corridor is a statement about the facet's
/// topology that a bystander must not be able to rewrite.
///
/// **What the bound costs is a re-plan, never a wrong step.** A body outside it
/// is invisible to the route; the route walks into it; that step is refused by
/// its own crowd, which is read fresh for every step, and the next beat plans
/// again with the body now inside the reach. Thirty-two is a screen and a half
/// — comfortably past [`VIEW_RANGE`](openshard_state::sectors::VIEW_RANGE),
/// which is as far as anything a creature is chasing can be.
const CROWD_REACH: u32 = 32;

/// How long a route to a *moving* goal stays trusted before it is re-planned,
/// in ticks — the references' two-second repath cadence.
///
/// A route to a place has no such window at all: see [`Goal`], which is the
/// caller's statement of which of the two it is walking.
///
/// Public for the same reason [`PATH_BUDGET`] is: the test that pins the
/// blindness a kept route buys has to wait exactly this long for one to lapse,
/// and a copy of the number in the test would be a second place to change it.
///
/// Two seconds times the tick rate rather than the tick count it comes to: it is
/// a span of real time, and the bare `40` it used to be became one second the
/// day the tick halved.
pub const REPATH_TICKS: u64 = 2 * openshard_state::TICKS_PER_SECOND;

/// How far the quarry may drift from a route's goal before the route is stale.
const GOAL_DRIFT: u32 = 2;

/// What a body is walking toward — a place, or somebody.
///
/// The only thing it decides is whether the route the body keeps carries a
/// *time* window, and the reason the caller has to say is that the window is
/// worth something to one of the two and nothing at all to the other.
///
/// **What a time window buys is noticing a better way**, and nothing else.
/// Every other way a kept route could go wrong is caught on its own and
/// without a search: the body standing somewhere else is [`Route::at`], the
/// goal having moved is [`GOAL_DRIFT`], and the ground having changed under
/// the next step is [`probe`], which puts every step of every route to the
/// live world before it is taken. So a route to a post is re-planned to learn
/// that a shorter way opened — for a townsperson walking home, an answer
/// nobody asked for at the price of a whole [`PATH_BUDGET`] search.
///
/// **And for that caller the window never fired anyway.** [`REPATH_TICKS`] is
/// 40 ticks and `npc::BEAT_TICKS` is 40 ticks, arrived at in two files that do
/// not mention one another, and `npc::next_beat` never arms a gap *shorter*
/// than its interval — so the route of the caller this cache would help most
/// was stale on every beat it was ever read on. A bigger number would have
/// hidden that behind a different one; naming the two cases is what stops the
/// question being about numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goal {
    /// A place: a post, a night home, the spot a pet was sent to. It does not
    /// move, so the route to it is walked to its end — however many beats that
    /// is, and it is a townsperson's whole minute-long walk.
    Fixed,
    /// Somebody: a quarry, an owner, a master being escorted. The whole route
    /// is a guess about where that body will be, so it is re-planned every
    /// [`REPATH_TICKS`] even while the goal stays inside [`GOAL_DRIFT`].
    Moving,
}

/// How long a creature stands watch after a chase found no way through, in
/// ticks — ten seconds, ServUO's guard timeout. Watching, not wall-shuffling;
/// when it expires the creature goes back to its life, and a quarry that becomes
/// reachable is re-acquired the normal way.
const GUARD_TICKS: u64 = 10 * openshard_state::TICKS_PER_SECOND;

/// A chase is abandoned beyond this many times the creature's sight — chasing
/// forever across the map is nobody's behaviour.
const CHASE_RANGE_FACTOR: u32 = 2;

/// The floor on how far any fight is followed, so a defensive creature with no
/// hunting sight of its own still answers its attacker — ServUO's default
/// perception is 16, and this is the same idea.
const CHASE_RANGE_MIN: u32 = 12;

/// How long a refused coarse query stands before the graph is asked about that
/// goal again, in ticks (~10s).
///
/// It is how long an answer about the shape of the world stays worth believing,
/// and the *floor* under the number is that it must outlive a beat of everything
/// that reads it. **It used to be [`REPATH_TICKS`] and therefore did not.** A
/// townsperson beats every 40 ticks and `npc::next_beat` never arms a gap
/// shorter than its interval, so a memory that lapsed after 40 had always
/// lapsed by the time the body woke up to use it — and a townsperson whose post
/// is walled off is the caller this memory was written for. The same collision
/// [`Goal`] exists for, one layer down.
///
/// Ten seconds is four of a townsperson's beats and twenty-five of a creature's.
/// It is also what [`GUARD_TICKS`] is, which is the same judgement about the
/// same fact — but the two are *not* written as one number, because a shared
/// value arrived at separately is exactly what went wrong above.
///
/// A body is not held still by it the way [`GUARD_TICKS`] holds a chaser — only
/// the facet-wide search waits, while the exact one is asked again as soon as
/// there is no [`Route`] left to walk.
///
/// Public for the same reason [`PATH_BUDGET`] is: the test that pins the memory
/// has to wait exactly this long for it to lapse, and a copy of the number in
/// the test would be a second place to change it.
pub const REFUSAL_TICKS: u64 = 10 * openshard_state::TICKS_PER_SECOND;

/// A creature this tough never runs — ServUO's "500 hits does not flee" rule.
const BRAVE_HITS: u16 = 500;

/// How close a foe may press a ranged fighter before it backs off — the
/// keep-away distance an archer or mage maintains.
const KITE_GAP: u32 = 2;

/// The first step from `from` toward `to`, planned *around* obstacles so a chaser
/// does not wedge itself against a wall. Falls back to the straight-line direction
/// when there is no map, or no route the exact search and the coarse graph
/// between them can find — better to close the gap roughly and re-plan than to
/// freeze.
///
/// # Two searches, and the same order the client asks them in
///
/// [`PATH_BUDGET`] bounds the *exact* search, and a budget is what makes a
/// chase affordable rather than what a body can walk: past a few hundred nodes
/// the answer is "no route" whether or not the town has one. That is what the
/// baked navigation graph is for — it has the facet's whole connectivity in it,
/// and a route across a town costs a corridor of region hops instead of the
/// tiles between here and there.
///
/// So a refused exact search is asked again of the graph, over
/// [`COARSE_MIN_DISTANCE`] tiles, which is the [same fall-back the client
/// walks a click by](openshard_movement::find_long_path). The graph proposes
/// the corridor over the bare map ([`WorldState::guide`]) and every hop of it
/// is then refined through the live ground, so a crate dropped in a doorway
/// still refuses the step it is standing in — the corridor is the only thing
/// the bare map decides.
///
/// # A refusal costs the most and is worth the least
///
/// A body that has somewhere to *keep* an answer should ask through
/// [`step_body_toward`] instead. This one is a pure function of the world, so a
/// goal it cannot reach costs the whole endpoint join at both ends on every
/// beat, and the straight-line direction it hands back looks the same whether
/// the graph refused or was never asked.
///
/// `mover` is who is walking, and it is needed for the same reason `from` is:
/// the crowd a route is planned around is *this* body's — it is not in its own
/// way, and a ghost or a game master is in nobody's.
pub fn step_toward(
    state: &WorldState,
    mover: EntityId,
    facet: Facet,
    from: Point,
    to: Point,
    doors: Doors,
) -> Option<Direction> {
    plan_step(state, mover, facet, from, to, doors, Fallback::Ask)
        .planned
        .direction()
}

/// The same step, decided for a body that can remember what it planned.
///
/// **N7's finding, and the guard `step_toward` had nowhere to put.** A pet
/// following an owner behind a locked door, a townsperson whose post is walled
/// off, an escortable trailing a master across a bridge that is not there: each
/// of them asked the coarse graph the same unanswerable question every beat and
/// paid the whole join for it. `chase_step` never did, because a refused chase
/// goes through [`give_up`] and stands watch.
///
/// So the refusal is written on the body as a [`RouteRefused`], and while it
/// stands the graph is not asked again about that goal. A goal that moves
/// further than [`GOAL_DRIFT`] is a different question and clears it, exactly as
/// it invalidates a [`Route`].
///
/// **And the answer is written down as well as the refusal.** A search that
/// arrives returns the whole way there and this used to keep its first step and
/// drop the rest, so a body walking twenty tiles planned twenty routes to walk
/// one of each — the exact half of the same waste the refusal memory took out of
/// the coarse half. The route is a [`Route`] now, followed a step per beat and
/// re-planned when it goes stale, which is the same cadence [`chase_step`] has
/// always walked its own by and the references' own pattern.
///
/// `facet` and `from` are the body's own, and are arguments rather than reads
/// because every caller has just read them. `goal` is the caller's statement of
/// *what* it is walking to — see [`Goal`], which is what decides how long the
/// route it plans here is worth keeping.
#[must_use]
pub fn step_body_toward(
    state: &mut WorldState,
    mover: EntityId,
    facet: Facet,
    from: Point,
    to: Point,
    doors: Doors,
    goal: Goal,
) -> Option<Direction> {
    // A route already planned and still worth walking is one step and no search
    // at all. This is the whole of what a body has that `step_toward` has not:
    // somewhere to keep an answer.
    match cached_step(state, mover, facet, from, to, doors, goal) {
        Cached::Step(direction) => return Some(direction),
        Cached::Opening => return None,
        Cached::Stale => {}
    }
    let fallback = match state.registry.get::<RouteRefused>(mover).copied() {
        Some(refusal) if state.ticks < refusal.until && distance(refusal.goal, to) <= GOAL_DRIFT => {
            Fallback::Withheld
        }
        // Expired, or about a goal nobody is walking to any more.
        Some(_) => {
            state.registry.remove::<RouteRefused>(mover);
            Fallback::Ask
        }
        None => Fallback::Ask,
    };
    let plan = plan_step(state, mover, facet, from, to, doors, fallback);
    match plan.coarse {
        Coarse::Refused => {
            let until = state.ticks + REFUSAL_TICKS;
            state.registry.insert(mover, RouteRefused { goal: to, until });
        }
        // A corridor answered, so whatever was remembered is out of date by
        // construction — and nothing was remembered, since a memory that stood
        // is why the graph would not have been asked.
        Coarse::Routed => {
            state.registry.remove::<RouteRefused>(mover);
        }
        Coarse::NotAsked => {}
    }
    match plan.planned {
        Planned::Route(steps) => {
            // An empty route is a body already standing on its goal: nothing to
            // walk and nothing to write down.
            let &first = steps.first()?;
            // The plan is over the planner's reading of the ground and this is
            // over the live one — a route planned through shut doors
            // ([`Doors::AllOpen`]) has a first step the world may still refuse.
            // The landing is what the route is written down against, so it has
            // to come from the rule the world will actually apply.
            let taken = match way_ahead(state, mover, facet, from, first, doors) {
                Way::Open(landing) => will_move(state, mover, first).then_some(landing),
                // The beat is spent on the door and the body stands where it
                // is, so the route is kept with its first step still due —
                // exactly what the beat after a cached step's door does.
                Way::Opening => {
                    remember(state, mover, steps, from, None, to);
                    return None;
                }
                Way::Shut => None,
            };
            remember(state, mover, steps, from, taken, to);
            Some(first)
        }
        Planned::Straight(direction) => direction,
    }
}

/// Whether a refused exact search may fall through to the coarse graph.
///
/// Not a tuning knob but how a caller spends a memory: the join is the expensive
/// half of a long query, and a refusal is the case that pays it in full for an
/// answer nothing keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fallback {
    Ask,
    Withheld,
}

/// What the coarse fall-back did with one decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Coarse {
    /// Never reached: the exact search answered, the goal is inside
    /// [`COARSE_MIN_DISTANCE`], the facet has no graph, or a standing refusal
    /// withheld it.
    NotAsked,
    /// A corridor answered.
    Routed,
    /// The whole endpoint join was paid at both ends and no route came back.
    Refused,
}

/// One [`step_toward`], reported rather than answered.
///
/// The split [`search_path`](openshard_movement::search_path) is to
/// `find_path`: one decision, both readings. The direction alone cannot say why
/// it is what it is nor how far ahead it was decided, and both are things only a
/// caller with somewhere to keep them can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
struct StepPlan {
    planned: Planned,
    coarse:  Coarse,
}

/// What a decision has behind it: a way, or a guess.
///
/// The distinction is the whole of what makes a route worth keeping. A search
/// that arrives has said something about every tile between here and the goal,
/// and a body may walk all of it; the straight line has said nothing about any
/// tile at all, including the one it points at, and the beat after it is a
/// question that has to be asked again.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Planned {
    /// A search found a way, and these are the steps of it. Empty means the body
    /// is already standing on the goal.
    Route(Vec<Direction>),
    /// Nothing found a way, so the gap is closed roughly and asked again next
    /// beat — better than freezing, and not a plan. `None` where even the
    /// straight line has nowhere to point, which is a body on its own goal.
    Straight(Option<Direction>),
}

impl Planned {
    /// The step to take this beat, however it was arrived at.
    fn direction(&self) -> Option<Direction> {
        match self {
            Self::Route(steps) => steps.first().copied(),
            Self::Straight(direction) => *direction,
        }
    }
}

/// The bodies a route from `from` to `to` is planned around.
///
/// **Why a route needs one at all.** Before this, a route was planned over
/// ground with nobody on it and then walked one step at a time over ground that
/// had somebody on it: the step was refused, the next beat re-decided the same
/// direction, and nothing ever went round. A crate in the same place worked
/// fine, because a crate is in the overlay and the plan could see it.
///
/// **The goal tile is dropped**, and ServUO drops the same one
/// (`Movement.cs:411`, `xForward != m_Goal.X`). A creature's goal is
/// overwhelmingly the quarry it is chasing, which is itself a body: leaving it
/// in makes every chase unplannable, because the one tile the route is *for* is
/// the one tile it may not end on. Arriving *beside* the quarry is the caller's
/// business — `chase_step` stops once it is in reach — and a step onto the goal,
/// if one is ever attempted, is refused by its own crowd.
///
/// One function and not two call sites, because the two readings that must
/// agree are the long route's and the chase's own: a route planned around a
/// body the next beat cannot see is a route that will not be walked.
fn crowd_for_route(state: &WorldState, mover: EntityId, facet: Facet, from: Point, to: Point) -> Vec<Point> {
    let mut crowd = state.crowd_near(facet, mover, from, distance(from, to).clamp(1, CROWD_REACH));
    crowd.retain(|body| (body.x, body.y) != (to.x, to.y));
    crowd
}

fn plan_step(
    state: &WorldState,
    mover: EntityId,
    facet: Facet,
    from: Point,
    to: Point,
    doors: Doors,
    fallback: Fallback,
) -> StepPlan {
    // The live terrain, not the bare map: a route must not thread a placed
    // crate the step would then refuse. A door-opener plans through doors and
    // opens them on arrival. And the crowd — see [`crowd_for_route`], which is
    // the whole of why a chase used to butt into a bystander for ever.
    let crowd = crowd_for_route(state, mover, facet, from, to);
    let planner = state
        .footing(facet, doors)
        .among(openshard_movement::Bodies::standing(&crowd));
    let local = search_path(&planner, from, to, PATH_BUDGET, Weight::PLANNING);
    if local.arrived {
        return StepPlan {
            planned: Planned::Route(local.route),
            coarse:  Coarse::NotAsked,
        };
    }
    // A short search that exhausted the live component stays refused: joining
    // both endpoints to the graph costs more and cannot invent a way. A budget
    // refusal is different. A multi-house can put hundreds of floor places in
    // eight tiles, and the dynamic storey join is how a creature inside one
    // reaches the static graph outside it.
    let ask = fallback == Fallback::Ask
        && (distance(from, to) > COARSE_MIN_DISTANCE || local.exit == SearchExit::Budget);
    let graph = ask.then(|| state.facet_state(facet).coarse_router()).flatten();
    let Some(graph) = graph else {
        return StepPlan {
            planned: Planned::Straight(direction_toward(from, to)),
            coarse:  Coarse::NotAsked,
        };
    };
    let guide = state.guide(facet);
    match find_long_path(&guide, &planner, graph, from, to, PATH_BUDGET, Weight::PLANNING) {
        Some(path) => {
            StepPlan {
                planned: Planned::Route(path),
                coarse:  Coarse::Routed,
            }
        }
        None => {
            StepPlan {
                planned: Planned::Straight(direction_toward(from, to)),
                coarse:  Coarse::Refused,
            }
        }
    }
}

/// One step of a body's cached route, or what stopped it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cached {
    /// The route's next step, and the route advanced past it.
    Step(Direction),
    /// The beat is spent opening a door the route runs through; stepping past it
    /// is the next beat's, and the route is kept for it.
    Opening,
    /// Nothing to walk: no route, or one the world has moved out from under. The
    /// component is gone and the caller plans afresh.
    Stale,
}

/// The step a body's own [`Route`] offers this beat, if it still offers one.
///
/// **Four ways a route stops being one**, and they are the references' own list:
/// the body is no longer standing where the next step starts, the goal has moved
/// past [`GOAL_DRIFT`], the steps have run out, or — for a [`Goal::Moving`] only
/// — [`REPATH_TICKS`] have passed since it was planned. A route to a place is
/// held to the first three and walked to its end; see [`Goal`] for why the
/// fourth is about somebody rather than about time.
///
/// A route that survives them all is still only a *plan* —
/// the step it offers is put to the live ground through [`probe`] before it is
/// taken, so a crate dropped on the way, a door swung shut or a body standing in
/// it costs a re-plan and never a step the shard would refuse.
///
/// **The doors are read from `doors` and not from the body**, because that is
/// the argument the caller has already decided the question with:
/// [`Doors::AllOpen`] is the reading of a body that intends to open its way
/// along the route (see that type), so a door refusing a step of a route planned
/// on it is a door this body meant to open rather than the world changing.
/// [`Doors::AsTheyStand`] plans round shut doors in the first place, so one in
/// the way is news, and the route is dropped.
fn cached_step(
    state: &mut WorldState,
    body: EntityId,
    facet: Facet,
    from: Point,
    to: Point,
    doors: Doors,
    goal: Goal,
) -> Cached {
    let Some(route) = state.registry.get::<Route>(body).cloned() else {
        return Cached::Stale;
    };
    let lapsed = match goal {
        Goal::Fixed => false,
        Goal::Moving => state.ticks.saturating_sub(route.planned_at) >= REPATH_TICKS,
    };
    let stale = route.at != from
        || distance(route.goal, to) > GOAL_DRIFT
        || route.next >= route.steps.len()
        || lapsed;
    if stale {
        state.registry.remove::<Route>(body);
        return Cached::Stale;
    }
    let direction = route.steps[route.next];
    match way_ahead(state, body, facet, from, direction, doors) {
        Way::Open(landing) => {
            // Turn-as-step: a body not yet facing this way spends the beat
            // turning and stands where it was, so the same step is due again.
            if will_move(state, body, direction) {
                let mut advanced = route;
                advanced.next += 1;
                advanced.at = landing;
                state.registry.insert(body, advanced);
            }
            Cached::Step(direction)
        }
        Way::Opening => Cached::Opening,
        Way::Shut => {
            // The world changed under the route; the caller plans again.
            state.registry.remove::<Route>(body);
            Cached::Stale
        }
    }
}

/// Write down a route a body has just planned, so the beats after this one walk
/// it instead of planning it again.
///
/// `taken` is where the route's first step lands, when the body is taking it
/// this beat. `None` covers the two ways it is not: a step that only *turns* a
/// body not yet facing that way, and a first step the body is spending the beat
/// opening a door in front of. Either way the body has not moved and the route's
/// first step is still due — which is the whole of why [`Route::next`] and
/// [`Route::at`] are decided here rather than by each caller: they are one fact
/// between them, and a caller that got them apart would have a body walking from
/// a place it is not standing in.
fn remember(
    state: &mut WorldState,
    body: EntityId,
    steps: Vec<Direction>,
    from: Point,
    taken: Option<Point>,
    goal: Point,
) {
    let (next, at) = match taken {
        Some(landing) => (1, landing),
        None => (0, from),
    };
    let planned_at = state.ticks;
    state.registry.insert(
        body,
        Route {
            steps,
            next,
            at,
            goal,
            planned_at,
        },
    );
}

/// What a creature decided to do with its beat.
///
/// The world applies it, because both arms are things a brain cannot do itself:
/// movement is the world's, and so is the cast sequence — see
/// `World::begin_creature_cast`. An enum rather than two fields because they are
/// alternatives: a creature that is casting is standing, and one that is stepping
/// is not casting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beat {
    /// Step this way, or stand — in reach of its target, newly engaged, holding
    /// a cast, or simply idle.
    Move(Option<Direction>),
    /// Throw `spell` at `target`, and stand while it is thrown.
    Cast { spell: SpellId, target: Serial },
}

/// One creature's beat: chase, fight or cast at what it has, pick a fight if it
/// sees one, or drift.
///
/// Engaging is done here, by giving the creature a [`Combat`]; stepping and
/// casting are the caller's to apply, since the world owns movement and the cast
/// sequence.
pub fn think_one(state: &mut WorldState, creature: EntityId) -> Beat {
    let Some(&Position(pos)) = state.registry.get::<Position>(creature) else {
        return Beat::Move(None);
    };
    let Some(&brain) = state.registry.get::<Brain>(creature) else {
        return Beat::Move(None);
    };
    let facet = state.facet_of(creature);

    // A cast in progress roots the caster, exactly as it roots a player: the
    // mobile is committed until `Casting` resolves or something breaks it. A
    // creature that walked out of its own cast would be two systems each
    // believing they own it for the next second.
    if state.registry.has::<Casting>(creature) {
        return Beat::Move(None);
    }
    // Standing watch after a chase that found no way through: hold still until
    // the timer runs out, then go back to living. A quarry that becomes
    // reachable is re-acquired below, the normal way.
    if brain.guard_until > state.ticks {
        return Beat::Move(None);
    }
    // A creature a bard has calmed picks no fights and chases nobody — ServUO's
    // `BardPacified`, read where the decision is made rather than folded into the
    // brain, so it needs no undoing when the song wears off.
    if state
        .registry
        .has::<openshard_state::components::Pacified>(creature)
    {
        return Beat::Move(None);
    }

    match fight_phase(state, creature, pos, facet, brain) {
        FightPhase::NoFight => {}
        FightPhase::Decided(beat) => return beat,
    }
    if acquire_phase(state, creature, pos, facet, brain) {
        return Beat::Move(None);
    }
    Beat::Move(wander_phase(state, creature, brain))
}

/// Whether the fight half of a beat had nothing to do, or made the beat's
/// decision. `Decided(Beat::Move(None))` is deliberately distinct from
/// `NoFight`: standing to strike, shoot or cast must not fall through into
/// acquiring prey or wandering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FightPhase {
    NoFight,
    Decided(Beat),
}

/// Follow, fight or abandon the creature's current target.
fn fight_phase(
    state: &mut WorldState,
    creature: EntityId,
    pos: Point,
    facet: Facet,
    brain: Brain,
) -> FightPhase {
    // Keep after a target that is still alive and in sight — close in if out of
    // reach, and leave the hitting to `swings`.
    let Some(target_serial) = state
        .registry
        .get::<Combat>(creature)
        .and_then(|combat| combat.target())
    else {
        return FightPhase::NoFight;
    };
    if let Some(target_pos) = foe_in_sight(
        state,
        creature,
        target_serial,
        pos,
        facet,
        chase_limit(brain.sight),
    ) {
        if should_flee(state, creature, brain) {
            state.registry.remove::<Route>(creature);
            return FightPhase::Decided(Beat::Move(flee_step(state, creature, facet, pos, target_pos)));
        }
        // A caster throws rather than closes — the branch above the melee one,
        // and above the bow's too: a creature that has both a reach and a
        // repertoire is a mage with a wand, and the spell is the interesting
        // half of it. Standing, like a shooter in range: `Decided` and not a
        // step, so the beat is spent on the cast.
        if let Some(spell) = spell_choice(state, creature, pos, facet, brain, target_pos) {
            return FightPhase::Decided(Beat::Cast {
                spell,
                target: target_serial,
            });
        }
        // A ranged fighter kites: back off from a foe at its heels, stand
        // and shoot inside its reach (`combat::commit_actions` does the
        // shooting, and deliberately does not turn a shooter toward its
        // mark — a step in a direction the mobile is not facing turns
        // instead of moving, so re-aiming it every nock would pin it here),
        // and only close in when out of range or out of sight line.
        if let Some(&RangedAttack { range, .. }) = state.registry.get::<RangedAttack>(creature) {
            let gap = distance(pos, target_pos);
            if gap <= KITE_GAP {
                state.registry.remove::<Route>(creature);
                return FightPhase::Decided(Beat::Move(kite_step(state, creature, facet, pos, target_pos)));
            }
            let clear =
                openshard_movement::sight_clear(&state.footing(facet, Doors::AsTheyStand), pos, target_pos);
            if gap <= u32::from(range.get()) && clear {
                return FightPhase::Decided(Beat::Move(None)); // in reach: stand and loose
            }
        }
        if in_range(pos, target_pos, combat::MELEE_RANGE) {
            // Arrived; the route served.
            state.registry.remove::<Route>(creature);
            return FightPhase::Decided(Beat::Move(None));
        }
        return FightPhase::Decided(Beat::Move(chase_step(
            state, creature, facet, pos, target_pos, brain,
        )));
    }
    // Out of sight, or too far to keep after: the creature drops the fight
    // rather than aiming at a memory. `disengage` and not `clear_target`,
    // for two reasons that are one. The quarry is *not* gone — it is very
    // often standing in plain view behind a fence — so what ends the swing
    // is the creature abandoning it, and that is the word every watcher
    // gets. And a creature's combat state exists only while it is fighting:
    // left behind as a targetless war stance, it would stand there flagged
    // as a fighter with nothing to fight for the rest of its life.
    state.disengage(creature);
    state.registry.remove::<Route>(creature);
    FightPhase::NoFight
}

/// The spell this creature throws at its foe this beat, or `None` if it throws
/// none.
///
/// Four questions, and a `None` from any of them is a beat spent the ordinary
/// way — closing, kiting or swinging:
///
/// - **Does it cast at all**, which is having a [`Repertoire`] and a [`Mana`]
///   pool. A creature with neither is every creature the shard has today.
/// - **Is it off its recovery** ([`Repertoire::next_cast`]) — otherwise a mage
///   would throw one spell per beat until its mana ran out.
/// - **Is the mark in reach**, which is as far as the creature can *see* and no
///   further, with a clear line to it. Sight rather than a reach of its own: a
///   spell has no range in this engine, so the honest bound is "what it can pick
///   out", and it is the same number the fight was started on. The sight line is
///   the shooter's own test, for the shooter's reason — a bolt does not bend
///   round a wall, and a caster that threw one through a keep would be fighting
///   from somewhere the player cannot fight back from.
/// - **What can it pay for**, which is [`affordable`] below.
///
/// The *choice* is the thin half on purpose. Which spell a fight actually calls
/// for — harm, heal, curse, escape — is the next phase of
/// `plans/npc/creature_casting/PLAN.md`, and until it lands this picks the
/// strongest thing the creature can pay for, which is a rule rather than a
/// placeholder: a lich with the mana for a flamestrike throws one.
fn spell_choice(
    state: &WorldState,
    creature: EntityId,
    pos: Point,
    facet: Facet,
    brain: Brain,
    target_pos: Point,
) -> Option<SpellId> {
    let &Repertoire { spells, next_cast } = state.registry.get::<Repertoire>(creature)?;
    if state.ticks < next_cast {
        return None;
    }
    let &Mana { current, .. } = state.registry.get::<Mana>(creature)?;
    if distance(pos, target_pos) > u32::from(brain.sight.0) {
        return None;
    }
    if !openshard_movement::sight_clear(&state.footing(facet, Doors::AsTheyStand), pos, target_pos) {
        return None;
    }
    affordable(spells, current)
}

/// The strongest spell in `spells` that costs no more than `mana` and is aimed
/// at a mobile.
///
/// **Strongest** is highest id, which is highest circle: `magic`'s table is the
/// eight circles in order, eight spells each, so counting down from the end is
/// counting down from the eighth circle. It spends what it has rather than
/// hoarding for a spell it will never afford.
///
/// **Aimed at a mobile** because that is the only aim a creature has. A
/// self-cast or a location spell in a creature's repertoire is content asking
/// for something this phase does not do — the categories that make sense of a
/// heal or a field are the next phase — and casting one *at a foe* would land it
/// on the caster instead, which is worse than not casting it.
fn affordable(spells: Spellbook, mana: u16) -> Option<SpellId> {
    (0..u16::from(SPELL_COUNT))
        .rev()
        .map(SpellId)
        .filter(|&spell| spells.has(spell))
        .find(|&spell| {
            magic::info(spell)
                .is_some_and(|info| info.target == magic::SpellTarget::Mobile && magic::mana(info) <= mana)
        })
}

/// Acquire visible prey when this brain starts fights. Returns whether the
/// transition consumed this beat.
fn acquire_phase(state: &mut WorldState, creature: EntityId, pos: Point, facet: Facet, brain: Brain) -> bool {
    // Nothing to fight: look for prey — only a creature that starts fights
    // hunts; the defensive and the passive wait to be wronged.
    if brain.sight.0 == 0 || brain.aggression != Aggression::Aggressive {
        return false;
    }
    let Some(prey) = nearest_player_in_sight(state, creature, pos, facet, brain.sight) else {
        return false;
    };
    let next_swing = state.ticks + combat::swing_speed(state, creature);
    state
        .registry
        .insert(creature, Combat::creature_engaged(prey, next_swing));
    // A growl on the aggro transition — the creature announces itself the
    // moment it notices prey, and only a creature growls (a human does not).
    if let Some(growl) = combat::anger_sound(state, creature) {
        state.play_sound(creature, growl);
    }
    true
}

/// Wander, if this is the beat on which the idle creature drifts.
fn wander_phase(state: &mut WorldState, creature: EntityId, brain: Brain) -> Option<Direction> {
    if !brain.wander || state.rng.below(8) >= WANDER_IN_EIGHT {
        return None;
    }
    // Walk on in the way it already faces, so it actually drifts rather than
    // spinning: a step in a new direction only *turns* (turn-as-step), so
    // picking a random heading every beat would never move. A quarter of the
    // time it does turn, to a new heading, and drifts off that way.
    let facing = state
        .registry
        .get::<Heading>(creature)
        .map_or(Direction::South, |h| h.0.direction);
    if state.rng.below(4) == 0 {
        Some(Direction::from_bits(state.rng.below(8) as u8))
    } else {
        Some(facing)
    }
}

/// How far a chase follows before it is abandoned — wider than the sight that
/// started it, so a quarry backing off does not flicker in and out of the fight.
fn chase_limit(sight: Sight) -> u32 {
    (u32::from(sight.0) * CHASE_RANGE_FACTOR).max(CHASE_RANGE_MIN)
}

/// The position of `target` if it is still a visible, live foe within `range`
/// of `from` on `facet`, or `None` if it has died, fled or vanished. Range
/// only, no terrain sight line: both references acquire with line of sight and
/// *pursue* on the cheaper check, so a quarry that ducks behind a wall is chased
/// around it, not lost.  Mobile visibility still applies: a living creature
/// cannot see or pursue a ghost.
fn foe_in_sight(
    state: &WorldState,
    watcher: EntityId,
    target: Serial,
    from: Point,
    facet: Facet,
    range: u32,
) -> Option<Point> {
    let entity = state.registry.entity_of(target)?;
    let &Position(pos) = state.registry.get::<Position>(entity)?;
    let alive = state
        .registry
        .get::<Hitpoints>(entity)
        .is_some_and(|h| h.current > 0);
    (alive
        && state.can_see_mobile(watcher, entity)
        && state.facet_of(entity) == facet
        && in_range(from, pos, range))
    .then_some(pos)
}

/// Whether a step in `dir` from `from` will *move* — a mobile not yet facing
/// that way only turns, and a route must not advance past a step that has not
/// happened yet.
fn will_move(state: &WorldState, creature: EntityId, dir: Direction) -> bool {
    state
        .registry
        .get::<Heading>(creature)
        .is_some_and(|h| h.0.direction == dir)
}

/// Where a step from `from` in `dir` lands on the live terrain, or — when it is
/// refused — the door standing there, if that is what blocks.
///
/// **The landing and not a yes**, because a route has to be written down against
/// the place its next step leaves the body standing in: see [`Route::at`]. It is
/// the live rule's own answer, which is what makes it the same place the tick
/// will put the body.
///
/// `step_allowed` and not `can_step`: a diagonal may not clip the corner where
/// two blockers meet, and the flanks are not a question one landing can answer.
/// This is the reading [`find_path`](openshard_movement::find_path) plans with,
/// so a chase that walks straight at its quarry is held to the rule its own
/// route already obeys — see `docs/world/evidence/2026-08-25-the-span-layer.md`'s N3.
///
/// A diagonal refused by a *flank* has no door to name, even when a door stands
/// in that flank: the door half below asks about the tile being stepped onto,
/// which is what a creature would open to pass. A route round it is what the
/// caller falls through to.
///
/// A body in the way has nothing to name either, and is refused with the same
/// `(None, None)` a wall gets. That is the right answer for what the caller
/// does with it: there is nothing to open, and going round is exactly the
/// fall-through. What it is *not* is a reason for the creature to stand still —
/// `plan_step` plans over the same crowd, so the route it comes back with is one
/// that goes round the body rather than into it.
fn probe(
    state: &WorldState,
    creature: EntityId,
    facet: Facet,
    from: Point,
    dir: Direction,
) -> (Option<Point>, Option<EntityId>) {
    let Some(target) = step_from(from, dir) else {
        return (None, None);
    };
    // One tile of reach, which is every tile `steps_out_of` will look at — the
    // eight neighbours, the diagonal's two flanks among them.
    let crowd = state.crowd_near(facet, creature, from, 1);
    let live = state
        .footing(facet, Doors::AsTheyStand)
        .among(openshard_movement::Bodies::standing(&crowd));
    if let Some(landing) = openshard_movement::step_allowed(&live, from, dir) {
        return (Some(landing), None);
    }
    // Which door, and not just that one is there: the overlay says a door is in
    // the way, and only the obstruction index says which entity to open. That
    // is the identity half staying server-side — see `movement::overlay`.
    let door = state
        .facet_state(facet)
        .obstructions()
        .blocker_at(target.x, target.y)
        .filter(|o| o.door())
        .map(|o| o.entity);
    (None, door)
}

/// What the live world does with the step a body is about to take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    /// Open, and this is where the step lands.
    Open(Point),
    /// A door was in the way and this body works latches: it is open now, and
    /// the beat went on opening it. The step itself is still due.
    Opening,
    /// Refused, with nothing there this body can do anything about.
    Shut,
}

/// [`probe`] with the door policy applied — the one place a body meets a door
/// standing on the step in front of it.
///
/// **It was three places**, which is one more than the number of times the rule
/// can be right. [`cached_step`] opened a door on the route it was following;
/// [`step_body_toward`] did not open one standing on the step it had just
/// planned, so `npc::walk_home` re-derived the door for itself out of the
/// obstruction index afterwards — and an escortable, which has no such code of
/// its own, planned through a shut door every beat and butted into it for ever.
/// [`chase_step`] had a fourth spelling for each of its two steps.
///
/// **The policy is `doors` and not the body**, for the reason that argument
/// exists: [`Doors::AllOpen`] is the reading of a body that intends to open its
/// way (see that type), so a door refusing a step planned on it is a door this
/// body meant to open rather than the world changing under the plan. A body
/// walking on [`Doors::AsTheyStand`] planned round shut doors in the first
/// place, and one in the way of it is news.
fn way_ahead(
    state: &mut WorldState,
    body: EntityId,
    facet: Facet,
    from: Point,
    direction: Direction,
    doors: Doors,
) -> Way {
    let (landing, door) = probe(state, body, facet, from, direction);
    if let Some(landing) = landing {
        return Way::Open(landing);
    }
    match (door, doors) {
        (Some(door), Doors::AllOpen) => {
            items::open_door(state, door);
            Way::Opening
        }
        _ => Way::Shut,
    }
}

/// One step of a chase: follow the cached route, walk straight when nothing is
/// in the way, plan when something is, and give up — guard, then wander — when
/// there is no way at all.
fn chase_step(
    state: &mut WorldState,
    creature: EntityId,
    facet: Facet,
    from: Point,
    to: Point,
    brain: Brain,
) -> Option<Direction> {
    // A cached route first: planned once, followed a step per beat. The quarry
    // is a body, so the route carries the repath window as well as the drift.
    let doors = Doors::for_opener(brain.opens_doors);
    match cached_step(state, creature, facet, from, to, doors, Goal::Moving) {
        Cached::Step(direction) => return Some(direction),
        Cached::Opening => return None,
        Cached::Stale => {}
    }

    // Nothing cached: walk straight at the quarry until something is in the
    // way — the naive-step-first shape both references use.
    let dir = direction_toward(from, to)?;
    match way_ahead(state, creature, facet, from, dir, doors) {
        Way::Open(_) => return Some(dir),
        Way::Opening => return None,
        Way::Shut => {}
    }

    // Blocked: plan a route around. A door-opener plans through doors and
    // opens them on arrival, and the route goes round the crowd as well as
    // round the walls — the same reading `plan_step` gives the long route, and
    // built by the same function so the two cannot drift.
    let planned = {
        let crowd = crowd_for_route(state, creature, facet, from, to);
        let planner = state
            .footing(facet, doors)
            .among(openshard_movement::Bodies::standing(&crowd));
        find_path(&planner, from, to, PATH_BUDGET, Weight::PLANNING)
    };
    match planned {
        Some(steps) if !steps.is_empty() => {
            let first = steps[0];
            match way_ahead(state, creature, facet, from, first, doors) {
                Way::Open(landing) => {
                    let taken = will_move(state, creature, first).then_some(landing);
                    remember(state, creature, steps, from, taken, to);
                    Some(first)
                }
                // The beat goes on the door and the route waits for the step.
                Way::Opening => {
                    remember(state, creature, steps, from, None, to);
                    None
                }
                // Planned into something that is neither open nor a door it can
                // work: give up rather than lunge.
                Way::Shut => give_up(state, creature),
            }
        }
        _ => give_up(state, creature),
    }
}

/// No way to the quarry: drop it, stand watch a while, then go back to living.
/// The alternative — shuffling into the fence forever — is the bug this exists
/// to end.
fn give_up(state: &mut WorldState, creature: EntityId) -> Option<Direction> {
    // The quarry is still there and still alive — the *way* to it is what failed.
    // See `think_one`'s lost-sight path for why this ends the fight outright
    // instead of clearing an aim.
    state.disengage(creature);
    state.registry.remove::<Route>(creature);
    let until = state.ticks + GUARD_TICKS;
    if let Some(brain) = state.registry.get_mut::<Brain>(creature) {
        brain.guard_until = until;
    }
    None
}

/// The nearest living player within `sight` of a creature, if any.
fn nearest_player_in_sight(
    state: &WorldState,
    creature: EntityId,
    from: Point,
    facet: Facet,
    sight: Sight,
) -> Option<Serial> {
    let facet_state = state.facet_state(facet);
    let live = state.footing(facet, Doors::AsTheyStand);
    let sectors = facet_state.sectors();
    let mut best: Option<(u32, Serial)> = None;
    for (id, pos) in sectors.mobiles_near(from, u32::from(sight.0)) {
        if id == creature || !state.registry.has::<Client>(id) || !state.can_see_mobile(creature, id) {
            continue;
        }
        // Noticing needs a sight line — both reference emulators gate the
        // *acquisition* on line of sight and keep the chase itself on the
        // cheaper range check, and so does this.
        if !openshard_movement::sight_clear(&live, from, pos) {
            continue;
        }
        if state.registry.get::<Hitpoints>(id).is_none_or(|h| h.current == 0) {
            continue;
        }
        let Some(serial) = state.registry.serial_of(id) else {
            continue;
        };
        let d = distance(from, pos);
        if best.is_none_or(|(best_d, _)| d < best_d) {
            best = Some((d, serial));
        }
    }
    best.map(|(_, serial)| serial)
}

/// Whether this creature runs from its foe rather than closing in: fauna
/// always does, and anything badly hurt does unless it is too big to scare.
/// There is no re-engage threshold yet because nothing regenerates hit points;
/// a fleer keeps running until the foe falls out of chase range.
fn should_flee(state: &WorldState, creature: EntityId, brain: Brain) -> bool {
    if brain.aggression == Aggression::Passive {
        return true;
    }
    state
        .registry
        .get::<Hitpoints>(creature)
        .is_some_and(|h| h.max < BRAVE_HITS && u32::from(h.current) * 5 < u32::from(h.max))
}

/// A step away from the threat: straight away when the ground allows, else the
/// nearest open turn to either side. `None` when boxed in — cornered.
fn flee_step(
    state: &mut WorldState,
    creature: EntityId,
    facet: Facet,
    from: Point,
    threat: Point,
) -> Option<Direction> {
    // A runner does not also swing; drop the guard while running.
    if let Some(combat) = state.registry.get_mut::<Combat>(creature) {
        combat.flee();
    }
    let away = direction_toward(threat, from).unwrap_or(Direction::South);
    for turn in [0u8, 1, 7, 2, 6, 3, 5] {
        let dir = Direction::from_bits((away.to_bits() + turn) & 7);
        let (landing, _) = probe(state, creature, facet, from, dir);
        if landing.is_some() {
            return Some(dir);
        }
    }
    None
}

/// A step that opens distance without dropping the fight — the kiting half of
/// a ranged brain. Same search as fleeing, warmode kept.
fn kite_step(
    state: &mut WorldState,
    creature: EntityId,
    facet: Facet,
    from: Point,
    threat: Point,
) -> Option<Direction> {
    let away = direction_toward(threat, from).unwrap_or(Direction::South);
    for turn in [0u8, 1, 7, 2, 6] {
        let dir = Direction::from_bits((away.to_bits() + turn) & 7);
        let (landing, _) = probe(state, creature, facet, from, dir);
        if landing.is_some() {
            return Some(dir);
        }
    }
    None
}

/// Answer violence: a creature with a brain that is hit and idle turns on its
/// attacker — warlike if it fights at all, target-only if it is fauna (so the
/// flee logic knows what to run from). Reading the damage event is what keeps
/// combat ignorant of AI: combat emits, this reacts.
pub fn retaliate(state: &mut WorldState, blows: &[MobileDamaged]) {
    for blow in blows {
        let Some(by) = blow.by else {
            continue;
        };
        if by == blow.serial {
            continue;
        }
        let victim = blow.entity;
        let Some(&brain) = state.registry.get::<Brain>(victim) else {
            continue;
        };
        // Being struck ends any standing watch on the spot.
        if let Some(b) = state.registry.get_mut::<Brain>(victim) {
            b.guard_until = WorldTick::ZERO;
        }
        let engaged = state
            .registry
            .get::<Combat>(victim)
            .and_then(|combat| combat.target())
            .is_some();
        let Some(attacker) = state.registry.entity_of(by) else {
            continue;
        };
        if engaged {
            continue;
        }
        let next_swing = state.ticks + combat::swing_speed(state, victim);
        let combat = match brain.aggression {
            Aggression::Passive => Combat::creature_threatened(by),
            _ => Combat::creature_engaged(by, next_swing),
        };
        state.registry.insert(victim, combat);
        // Retaliation is a visible decision, not just a target stored for a
        // later swing.  Turn now so the creature acknowledges its attacker
        // during the swing delay (and passive fauna faces the threat it flees).
        state.face_toward(victim, attacker);
    }
}

/// How close a pet stays to its owner when following — ServUO's `WalkMobileRange`
/// with a gap of one, so it stands beside you rather than on you.
const PET_FOLLOW_GAP: u32 = 2;
/// And how far it will chase before giving up and coming back.
const PET_LEASH: u32 = 15;

/// A pet's beat: what it does about the last thing it was told.
///
/// The same shape as [`think_one`] and deliberately *not* part of it: a pet does
/// not decide anything for itself, it carries out an order. What it shares is the
/// return — a direction for the tick to step — so a pet moves through the same
/// `step` a wild creature and a townsperson use.
///
/// **A pet works a latch exactly as it did before it was tamed.** The door
/// policy is its own brain's, the same read [`chase_step`] makes of a wild one,
/// so a tamed orc opens the shop door and a llama stops at it — ServUO's
/// `BaseAI`, which asks the creature and not the order. This used to pass
/// [`Doors::AllOpen`] for every pet, which planned through doors the pet could
/// not work and, once routes were kept, opened them: the `opens_doors` a tamed
/// creature carries was a dead field, and it is set from the body at both the
/// taming and the restore.
///
/// The brain is read rather than defaulted because every pet has one: taming
/// gives a brainless prop horse one (`npc::pets`), a restore rebuilds it, and
/// the tick's own loop is over brains.
#[must_use]
pub fn pet_beat(state: &mut WorldState, pet: EntityId) -> Option<Direction> {
    let order = *state.registry.get::<Pet>(pet)?;
    let &Position(at) = state.registry.get::<Position>(pet)?;
    let doors = Doors::for_opener(state.registry.get::<Brain>(pet)?.opens_doors);
    let facet = state.facet_of(pet);
    let owner = state.registry.entity_of(order.owner)?;
    match order.order {
        PetOrder::Stay | PetOrder::Stop => None,
        PetOrder::Attack => {
            // The fighting itself is `combat::swings`, exactly as for any other
            // mobile with a `Combat`; this only closes the distance.
            let target = order
                .order_target
                .and_then(|serial| state.registry.entity_of(serial))?;
            let &Position(target_at) = state.registry.get::<Position>(target)?;
            if openshard_state::in_range(at, target_at, 1) {
                return None;
            }
            step_body_toward(state, pet, facet, at, target_at, doors, Goal::Moving)
        }
        PetOrder::Guard | PetOrder::Follow | PetOrder::Come => {
            let &Position(owner_at) = state.registry.get::<Position>(owner)?;
            if openshard_state::in_range(at, owner_at, PET_FOLLOW_GAP) {
                return None;
            }
            if !openshard_state::in_range(at, owner_at, PET_LEASH) {
                // Too far to walk back sensibly — a pet left behind waits rather
                // than pathing across the map, the same give-up the chase has.
                return None;
            }
            step_body_toward(state, pet, facet, at, owner_at, doors, Goal::Moving)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openshard_protocol::serial::SerialKind;

    use super::*;

    fn world() -> WorldState {
        WorldState::new(
            BTreeMap::new(),
            Facet(0),
            openshard_tiles::TileData::empty(),
            Default::default(),
            openshard_map::grid::Tile::new(0, 0),
            1,
        )
    }

    /// The same world with its default facet actually loaded, for the decisions
    /// that read the ground — a sight line is one, and `WorldState::footing`
    /// insists on a facet that exists.
    fn world_with_ground() -> WorldState {
        let tiles = openshard_tiles::TileData::empty();
        let facet = openshard_state::FacetState::new(
            None,
            None,
            64,
            64,
            openshard_state::facet_rules::FacetRules::classic(Facet(0)),
            None,
            &tiles,
        );
        WorldState::new(
            BTreeMap::from([(Facet(0), facet)]),
            Facet(0),
            tiles,
            Default::default(),
            openshard_map::grid::Tile::new(0, 0),
            1,
        )
    }

    fn mobile(state: &mut WorldState, at: Point) -> (EntityId, Serial) {
        let (entity, serial) = state
            .registry
            .spawn_with_serial(SerialKind::Mobile)
            .expect("a test mobile serial");
        state.registry.insert(entity, Position(at));
        state.registry.insert(entity, Facet(0));
        state.registry.insert(
            entity,
            Hitpoints {
                current: 10,
                max:     10,
            },
        );
        (entity, serial)
    }

    #[test]
    fn standing_to_fight_is_not_the_same_as_having_no_fight() {
        let mut state = world();
        let at = Point::new(10, 10, 0);
        let (creature, _) = mobile(&mut state, at);
        let brain = Brain {
            sight: Sight(12),
            ..Brain::default()
        };

        assert_eq!(
            fight_phase(&mut state, creature, at, Facet(0), brain),
            FightPhase::NoFight
        );

        let (_, target) = mobile(&mut state, Point::new(10, 11, 0));
        state
            .registry
            .insert(creature, Combat::creature_engaged(target, WorldTick::ZERO));

        assert_eq!(
            fight_phase(&mut state, creature, at, Facet(0), brain),
            FightPhase::Decided(Beat::Move(None))
        );
    }

    /// Spell ids from `magic`'s own table, named here so a test that changes
    /// meaning when the table is renumbered fails loudly rather than quietly
    /// asserting about a different spell.
    const MAGIC_ARROW: SpellId = SpellId(4);
    const HARM: SpellId = SpellId(11);
    const FIREBALL: SpellId = SpellId(17);
    /// `Create Food` — first circle, and cast at nobody.
    const CREATE_FOOD: SpellId = SpellId(1);

    fn knowing(spells: &[SpellId]) -> Spellbook {
        let mut book = Spellbook(0);
        for &spell in spells {
            book.learn(spell);
        }
        book
    }

    /// The named ids are the spells this file's tests think they are.
    #[test]
    fn the_named_spells_are_the_ones_the_table_holds() {
        for (spell, name) in [
            (MAGIC_ARROW, "Magic Arrow"),
            (HARM, "Harm"),
            (FIREBALL, "Fireball"),
            (CREATE_FOOD, "Create Food"),
        ] {
            assert_eq!(magic::info(spell).expect("a spell").name, name);
        }
    }

    /// It spends what it has: the strongest thing it can pay for, not the first
    /// thing it knows.
    #[test]
    fn a_caster_throws_the_best_it_can_afford() {
        let book = knowing(&[MAGIC_ARROW, HARM, FIREBALL]);
        let fireball = magic::mana(magic::info(FIREBALL).expect("a spell"));
        let harm = magic::mana(magic::info(HARM).expect("a spell"));
        assert_eq!(affordable(book, fireball), Some(FIREBALL));
        assert_eq!(
            affordable(book, fireball - 1),
            Some(HARM),
            "short of a fireball, it throws the next one down"
        );
        assert_eq!(affordable(book, harm - 1), Some(MAGIC_ARROW));
        assert_eq!(affordable(book, 0), None, "nothing is free");
    }

    /// A spell aimed at nobody is not a spell to aim at a foe: cast at one it
    /// would land on the caster, which is worse than not casting it.
    #[test]
    fn a_spell_that_takes_no_mark_is_never_chosen() {
        let book = knowing(&[CREATE_FOOD]);
        assert_eq!(affordable(book, u16::MAX), None);
        assert_eq!(
            affordable(knowing(&[CREATE_FOOD, HARM]), u16::MAX),
            Some(HARM),
            "and it does not stop the rest of the repertoire being reached"
        );
    }

    /// An empty repertoire is every creature the shard has today, and it must
    /// cost the fight nothing.
    #[test]
    fn a_creature_that_knows_nothing_casts_nothing() {
        assert_eq!(affordable(Spellbook(0), u16::MAX), None);
    }

    /// The four gates, one at a time: a creature with the mana, the spells and
    /// the mark in front of it casts, and each missing piece stops it.
    #[test]
    fn a_caster_casts_only_when_every_gate_is_open() {
        let mut state = world_with_ground();
        let at = Point::new(10, 10, 0);
        let target_pos = Point::new(13, 10, 0);
        let (creature, _) = mobile(&mut state, at);
        let brain = Brain {
            sight: Sight(12),
            ..Brain::default()
        };

        assert_eq!(
            spell_choice(&state, creature, at, Facet(0), brain, target_pos),
            None,
            "no repertoire, no cast"
        );

        state.registry.insert(
            creature,
            Repertoire {
                spells:    knowing(&[MAGIC_ARROW, HARM, FIREBALL]),
                next_cast: WorldTick::ZERO,
            },
        );
        assert_eq!(
            spell_choice(&state, creature, at, Facet(0), brain, target_pos),
            None,
            "a repertoire with no pool to spend is still no cast"
        );

        state.registry.insert(
            creature,
            Mana {
                current: 1000,
                max:     1000,
            },
        );
        assert_eq!(
            spell_choice(&state, creature, at, Facet(0), brain, target_pos),
            Some(FIREBALL)
        );

        // Out past what it can pick out: the fight is still on — `foe_in_sight`
        // pursues further than it acquires — but the spell is not thrown.
        let far = Point::new(10 + i32::from(brain.sight.0) as u16 + 1, 10, 0);
        assert_eq!(
            spell_choice(&state, creature, at, Facet(0), brain, far),
            None,
            "further than it can see"
        );

        // And within its recovery.
        state.registry.insert(
            creature,
            Repertoire {
                spells:    knowing(&[MAGIC_ARROW, HARM, FIREBALL]),
                next_cast: state.ticks + 10,
            },
        );
        assert_eq!(
            spell_choice(&state, creature, at, Facet(0), brain, target_pos),
            None,
            "still recovering from the last one"
        );
    }

    /// The branch order the plan asks for: a caster in reach throws rather than
    /// closes, and the beat is spent standing.
    #[test]
    fn the_cast_branch_stands_above_the_chase() {
        let mut state = world_with_ground();
        let at = Point::new(10, 10, 0);
        let (creature, _) = mobile(&mut state, at);
        // Out of melee reach, so without a repertoire this beat would be a step.
        let (_, target) = mobile(&mut state, Point::new(15, 10, 0));
        let brain = Brain {
            sight: Sight(12),
            ..Brain::default()
        };
        state
            .registry
            .insert(creature, Combat::creature_engaged(target, WorldTick::ZERO));

        let closing = fight_phase(&mut state, creature, at, Facet(0), brain);
        assert!(
            matches!(closing, FightPhase::Decided(Beat::Move(Some(_)))),
            "a creature with no spells closes in: {closing:?}"
        );

        state.registry.insert(
            creature,
            Repertoire {
                spells:    knowing(&[HARM]),
                next_cast: WorldTick::ZERO,
            },
        );
        state.registry.insert(
            creature,
            Mana {
                current: 1000,
                max:     1000,
            },
        );
        assert_eq!(
            fight_phase(&mut state, creature, at, Facet(0), brain),
            FightPhase::Decided(Beat::Cast { spell: HARM, target }),
            "and one with them throws instead"
        );
    }

    /// A creature holding a cast is committed to it: it does not walk out of its
    /// own spell, which is the rooting a player already has.
    #[test]
    fn a_rooted_caster_stands() {
        let mut state = world_with_ground();
        let at = Point::new(10, 10, 0);
        let (creature, _) = mobile(&mut state, at);
        let (_, target) = mobile(&mut state, Point::new(15, 10, 0));
        state.registry.insert(
            creature,
            Brain {
                sight: Sight(12),
                ..Brain::default()
            },
        );
        state
            .registry
            .insert(creature, Combat::creature_engaged(target, WorldTick::ZERO));
        assert!(
            matches!(think_one(&mut state, creature), Beat::Move(Some(_))),
            "it would close in"
        );

        state.registry.insert(
            creature,
            Casting {
                spell:       HARM,
                complete_at: state.ticks + 10,
                scroll:      None,
                aim:         Some(target),
            },
        );
        assert_eq!(think_one(&mut state, creature), Beat::Move(None));
    }
}
