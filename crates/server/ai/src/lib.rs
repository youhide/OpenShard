//! Creature behaviour: what a brain decides to do with its beat.
//!
//! A brain only *decides*. [`think_one`] reads the world, works out whether a
//! creature should chase, fight, or drift, and turns that into at most one thing:
//! a direction to step, returned to the caller. Engaging a foe it does itself (it
//! hands the creature a [`Combat`], and `combat::swings` fights with it exactly as
//! a player would); moving it leaves to the world, which owns the step. So `ai`
//! reuses combat and movement — it never reimplements them.
//!
//! The decision uses the world's seeded [`Rng`](openshard_state::Rng), so a fight
//! or a wander replays identically.

use openshard_combat as combat;
use openshard_combat::MobileDamaged;
use openshard_entities::EntityId;
use openshard_items as items;
use openshard_map::overlay::Doors;
use openshard_movement::{COARSE_MIN_DISTANCE, direction_toward, find_long_path, find_path, step_from};
use openshard_protocol::direction::Direction;
use openshard_protocol::serial::Serial;
use openshard_protocol::world::{Facet, Point, Sight};
use openshard_state::WorldState;
use openshard_state::components::{
    Aggression, Brain, ChasePath, Client, Combat, Heading, Hitpoints, Pet, PetOrder, Position, RangedAttack,
    RouteRefused,
};
use openshard_state::sectors::{distance, in_range};

/// The chance in eight, per beat, that an idle wanderer takes a step. Low enough
/// that a field of creatures drifts rather than marches.
const WANDER_IN_EIGHT: u32 = 3;

/// How many *nodes* an exact chase plan may finalise before it gives up.
///
/// Ample to round a building; an unreachable quarry is not worth more. It is
/// not a reach: since `docs/map/navigation_spans.md`'s N3b a column with two
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

/// How long a planned route stays trusted before it is re-planned, in ticks —
/// the references' two-second repath cadence.
const REPATH_TICKS: u64 = 40;

/// How far the quarry may drift from a route's goal before the route is stale.
const GOAL_DRIFT: u32 = 2;

/// How long a creature stands watch after a chase found no way through, in
/// ticks (~10s) — ServUO's guard timeout. Watching, not wall-shuffling; when it
/// expires the creature goes back to its life, and a quarry that becomes
/// reachable is re-acquired the normal way.
const GUARD_TICKS: u64 = 200;

/// A chase is abandoned beyond this many times the creature's sight — chasing
/// forever across the map is nobody's behaviour.
const CHASE_RANGE_FACTOR: u32 = 2;

/// The floor on how far any fight is followed, so a defensive creature with no
/// hunting sight of its own still answers its attacker — ServUO's default
/// perception is 16, and this is the same idea.
const CHASE_RANGE_MIN: u32 = 12;

/// How long a refused coarse query stands before the graph is asked about that
/// goal again, in ticks (~2s).
///
/// The same cadence a planned route is trusted for, and for the same reason: it
/// is how long an answer about the shape of the world stays worth believing. A
/// body is not held still by it the way [`GUARD_TICKS`] holds a chaser — only
/// the facet-wide search waits, while the exact one runs every beat.
///
/// Public for the same reason [`PATH_BUDGET`] is: the test that pins the memory
/// has to wait exactly this long for it to lapse, and a copy of the number in
/// the test would be a second place to change it.
pub const REFUSAL_TICKS: u64 = REPATH_TICKS;

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
    plan_step(state, mover, facet, from, to, doors, Fallback::Ask).direction
}

/// The same step, decided for a body that can remember a refusal.
///
/// **N7's finding, and the guard `step_toward` had nowhere to put.** A pet
/// following an owner behind a locked door, a townsperson whose post is walled
/// off, an escortable trailing a master across a bridge that is not there: each
/// of them asked the coarse graph the same unanswerable question every beat and
/// paid the whole join for it. `chase_step` never did, because a refused chase
/// goes through [`give_up`] and stands watch.
///
/// So the refusal is written on the body as a [`RouteRefused`], and while it
/// stands the graph is not asked again about that goal. The body still walks —
/// the exact search runs every beat as it always did, and the straight-line
/// fall-back is unchanged — so what waits is the facet-wide answer and nothing
/// else. A goal that moves further than [`GOAL_DRIFT`] is a different question
/// and clears it, exactly as it invalidates a [`ChasePath`].
///
/// `facet` and `from` are the body's own, and are arguments rather than reads
/// because every caller has just read them.
#[must_use]
pub fn step_body_toward(
    state: &mut WorldState,
    mover: EntityId,
    facet: Facet,
    from: Point,
    to: Point,
    doors: Doors,
) -> Option<Direction> {
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
    plan.direction
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
/// it is what it is, and *why* is the only thing a caller with a memory acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StepPlan {
    direction: Option<Direction>,
    coarse: Coarse,
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
    if let Some(path) = find_path(&planner, from, to, PATH_BUDGET) {
        return StepPlan {
            direction: path.first().copied(),
            coarse: Coarse::NotAsked,
        };
    }
    // Short and refused stays refused: joining both endpoints to the graph
    // costs more than the local answer that has already said no.
    let ask = fallback == Fallback::Ask && distance(from, to) > COARSE_MIN_DISTANCE;
    let graph = ask.then(|| state.facet_state(facet).coarse_router()).flatten();
    let Some(graph) = graph else {
        return StepPlan {
            direction: direction_toward(from, to),
            coarse: Coarse::NotAsked,
        };
    };
    let guide = state.guide(facet);
    match find_long_path(&guide, &planner, graph, from, to, PATH_BUDGET) {
        Some(path) => StepPlan {
            direction: path.first().copied(),
            coarse: Coarse::Routed,
        },
        None => StepPlan {
            direction: direction_toward(from, to),
            coarse: Coarse::Refused,
        },
    }
}

/// One creature's beat: chase and fight what it has, pick a fight if it sees one,
/// or drift.
///
/// Returns the direction the creature wants to step this beat — chasing a
/// foe or wandering — or `None` if it stood its ground (in reach of its target,
/// newly engaged, or idle). Engaging is done here, by giving the creature a
/// [`Combat`]; stepping is the caller's to apply, since the world owns movement.
pub fn think_one(state: &mut WorldState, creature: EntityId) -> Option<Direction> {
    let &Position(pos) = state.registry.get::<Position>(creature)?;
    let brain = *state.registry.get::<Brain>(creature)?;
    let Brain { sight, wander, .. } = brain;
    let facet = state.facet_of(creature);

    // Standing watch after a chase that found no way through: hold still until
    // the timer runs out, then go back to living. A quarry that becomes
    // reachable is re-acquired below, the normal way.
    if brain.guard_until > state.ticks {
        return None;
    }
    // A creature a bard has calmed picks no fights and chases nobody — ServUO's
    // `BardPacified`, read where the decision is made rather than folded into the
    // brain, so it needs no undoing when the song wears off.
    if state
        .registry
        .has::<openshard_state::components::Pacified>(creature)
    {
        return None;
    }

    // Keep after a target that is still alive and in sight — close in if out of
    // reach, and leave the hitting to `swings`.
    if let Some(target_serial) = state.registry.get::<Combat>(creature).and_then(|c| c.target) {
        if let Some(target_pos) = foe_in_sight(state, target_serial, pos, facet, chase_limit(sight)) {
            if should_flee(state, creature, brain) {
                state.registry.remove::<ChasePath>(creature);
                return flee_step(state, creature, facet, pos, target_pos);
            }
            // A ranged fighter kites: back off from a foe at its heels, stand
            // and shoot inside its reach (the volley system does the firing),
            // and only close in when out of range or out of sight line.
            if let Some(&RangedAttack { range, .. }) = state.registry.get::<RangedAttack>(creature) {
                let gap = distance(pos, target_pos);
                if gap <= KITE_GAP {
                    state.registry.remove::<ChasePath>(creature);
                    return kite_step(state, creature, facet, pos, target_pos);
                }
                let clear = openshard_movement::sight_clear(
                    &state.footing(facet, Doors::AsTheyStand),
                    pos,
                    target_pos,
                );
                if gap <= u32::from(range.get()) && clear {
                    return None; // in reach, sight line clear: stand and loose
                }
            }
            if in_range(pos, target_pos, combat::MELEE_RANGE) {
                // Arrived; the route served.
                state.registry.remove::<ChasePath>(creature);
                return None;
            }
            return chase_step(state, creature, facet, pos, target_pos, brain);
        }
        combat::clear_target(state, creature);
        state.registry.remove::<ChasePath>(creature);
    }

    // Nothing to fight: look for prey — only a creature that starts fights
    // hunts; the defensive and the passive wait to be wronged — or wander.
    if sight.0 > 0 && brain.aggression == Aggression::Aggressive {
        if let Some(prey) = nearest_player_in_sight(state, creature, pos, facet, sight) {
            let next_swing = state.ticks + combat::swing_speed(state, creature);
            state.registry.insert(
                creature,
                Combat {
                    warmode: true,
                    target: Some(prey),
                    next_swing,
                },
            );
            // A growl on the aggro transition — the creature announces itself the
            // moment it notices prey, and only a creature growls (a human does not).
            let growl = combat::anger_sound(state, creature);
            if let Some(growl) = growl {
                state.play_sound(creature, growl);
            }
            return None;
        }
    }
    if wander && state.rng.below(8) < WANDER_IN_EIGHT {
        // Walk on in the way it already faces, so it actually drifts rather than
        // spinning: a step in a new direction only *turns* (turn-as-step), so
        // picking a random heading every beat would never move. A quarter of the
        // time it does turn, to a new heading, and drifts off that way.
        let facing = state
            .registry
            .get::<Heading>(creature)
            .map_or(Direction::South, |h| h.0.direction);
        let dir = if state.rng.below(4) == 0 {
            Direction::from_bits(state.rng.below(8) as u8)
        } else {
            facing
        };
        return Some(dir);
    }
    None
}

/// How far a chase follows before it is abandoned — wider than the sight that
/// started it, so a quarry backing off does not flicker in and out of the fight.
fn chase_limit(sight: Sight) -> u32 {
    (u32::from(sight.0) * CHASE_RANGE_FACTOR).max(CHASE_RANGE_MIN)
}

/// The position of `target` if it is still a live foe within `range` of `from`
/// on `facet`, or `None` if it has died, fled or vanished. Range only, no sight
/// line: both references acquire with line of sight and *pursue* on the cheaper
/// check, so a quarry that ducks behind a wall is chased around it, not lost.
fn foe_in_sight(state: &WorldState, target: Serial, from: Point, facet: Facet, range: u32) -> Option<Point> {
    let entity = state.registry.entity_of(target)?;
    let &Position(pos) = state.registry.get::<Position>(entity)?;
    let alive = state
        .registry
        .get::<Hitpoints>(entity)
        .is_some_and(|h| h.current > 0);
    (alive && state.facet_of(entity) == facet && in_range(from, pos, range)).then_some(pos)
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

/// Whether a step from `from` in `dir` is open on the live terrain; when it is
/// not, the door standing there, if that is what blocks.
///
/// `step_allowed` and not `can_step`: a diagonal may not clip the corner where
/// two blockers meet, and the flanks are not a question one landing can answer.
/// This is the reading [`find_path`](openshard_movement::find_path) plans with,
/// so a chase that walks straight at its quarry is held to the rule its own
/// route already obeys — see `docs/map/navigation_spans.md`'s N3.
///
/// A diagonal refused by a *flank* has no door to name, even when a door stands
/// in that flank: the door half below asks about the tile being stepped onto,
/// which is what a creature would open to pass. A route round it is what the
/// caller falls through to.
///
/// A body in the way has nothing to name either, and is refused with the same
/// `(false, None)` a wall gets. That is the right answer for what the caller
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
) -> (bool, Option<EntityId>) {
    let Some(target) = step_from(from, dir) else {
        return (false, None);
    };
    // One tile of reach, which is every tile `steps_out_of` will look at — the
    // eight neighbours, the diagonal's two flanks among them.
    let crowd = state.crowd_near(facet, creature, from, 1);
    let live = state
        .footing(facet, Doors::AsTheyStand)
        .among(openshard_movement::Bodies::standing(&crowd));
    if openshard_movement::step_allowed(&live, from, dir).is_some() {
        return (true, None);
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
    (false, door)
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
    // A cached route first: planned once, followed a step per beat.
    if let Some(path) = state.registry.get::<ChasePath>(creature).cloned() {
        let stale = state.ticks.saturating_sub(path.planned_at) >= REPATH_TICKS
            || distance(path.goal, to) > GOAL_DRIFT
            || path.next >= path.steps.len();
        if stale {
            state.registry.remove::<ChasePath>(creature);
        } else {
            let dir = path.steps[path.next];
            let (open, door) = probe(state, creature, facet, from, dir);
            if open {
                if will_move(state, creature, dir) {
                    let mut advanced = path;
                    advanced.next += 1;
                    state.registry.insert(creature, advanced);
                }
                return Some(dir);
            }
            if let Some(door) = door {
                if brain.opens_doors {
                    // The route runs through this door on purpose: open it and
                    // step through next beat.
                    items::open_door(state, door);
                    return None;
                }
            }
            // The world changed under the route; plan again below.
            state.registry.remove::<ChasePath>(creature);
        }
    }

    // Nothing cached: walk straight at the quarry until something is in the
    // way — the naive-step-first shape both references use.
    let dir = direction_toward(from, to)?;
    let (open, door) = probe(state, creature, facet, from, dir);
    if open {
        return Some(dir);
    }
    if let Some(door) = door {
        if brain.opens_doors {
            items::open_door(state, door);
            return None;
        }
    }

    // Blocked: plan a route around. A door-opener plans through doors and
    // opens them on arrival, and the route goes round the crowd as well as
    // round the walls — the same reading `plan_step` gives the long route, and
    // built by the same function so the two cannot drift.
    let planned = {
        let crowd = crowd_for_route(state, creature, facet, from, to);
        let planner = state
            .footing(facet, Doors::for_opener(brain.opens_doors))
            .among(openshard_movement::Bodies::standing(&crowd));
        find_path(&planner, from, to, PATH_BUDGET)
    };
    match planned {
        Some(steps) if !steps.is_empty() => {
            let first = steps[0];
            let (open, door) = probe(state, creature, facet, from, first);
            if !open {
                if let Some(door) = door {
                    if brain.opens_doors {
                        items::open_door(state, door);
                        state.registry.insert(
                            creature,
                            ChasePath {
                                steps,
                                next: 0,
                                goal: to,
                                planned_at: state.ticks,
                            },
                        );
                        return None;
                    }
                }
                // Planned into something that is neither open nor a door it can
                // work: give up rather than lunge.
                return give_up(state, creature);
            }
            let next = usize::from(will_move(state, creature, first));
            state.registry.insert(
                creature,
                ChasePath {
                    steps,
                    next,
                    goal: to,
                    planned_at: state.ticks,
                },
            );
            Some(first)
        }
        _ => give_up(state, creature),
    }
}

/// No way to the quarry: drop it, stand watch a while, then go back to living.
/// The alternative — shuffling into the fence forever — is the bug this exists
/// to end.
fn give_up(state: &mut WorldState, creature: EntityId) -> Option<Direction> {
    combat::clear_target(state, creature);
    state.registry.remove::<ChasePath>(creature);
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
        if id == creature || !state.registry.has::<Client>(id) {
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
        combat.warmode = false;
    }
    let away = direction_toward(threat, from).unwrap_or(Direction::South);
    for turn in [0u8, 1, 7, 2, 6, 3, 5] {
        let dir = Direction::from_bits((away.to_bits() + turn) & 7);
        let (open, _) = probe(state, creature, facet, from, dir);
        if open {
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
        let (open, _) = probe(state, creature, facet, from, dir);
        if open {
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
            b.guard_until = 0;
        }
        let engaged = state
            .registry
            .get::<Combat>(victim)
            .and_then(|c| c.target)
            .is_some();
        let Some(attacker) = state.registry.entity_of(by) else {
            continue;
        };
        if engaged {
            continue;
        }
        let warmode = brain.aggression != Aggression::Passive;
        let next_swing = state.ticks + combat::swing_speed(state, victim);
        state.registry.insert(
            victim,
            Combat {
                warmode,
                target: Some(by),
                next_swing,
            },
        );
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
#[must_use]
pub fn pet_beat(state: &mut WorldState, pet: EntityId) -> Option<Direction> {
    let order = *state.registry.get::<Pet>(pet)?;
    let &Position(at) = state.registry.get::<Position>(pet)?;
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
            step_body_toward(state, pet, facet, at, target_at, Doors::AllOpen)
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
            step_body_toward(state, pet, facet, at, owner_at, Doors::AllOpen)
        }
    }
}
