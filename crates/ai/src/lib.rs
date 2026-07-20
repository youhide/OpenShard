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
use openshard_entities::{EntityId, Serial};
use openshard_items as items;
use openshard_movement::{find_path, step_from, Terrain};
use openshard_protocol::{Direction, Point};
use openshard_state::components::{
    Aggression, Brain, ChasePath, Client, Combat, Heading, Hitpoints, Position, RangedAttack,
    Scripted,
};
use openshard_state::sectors::{distance, in_range};
use openshard_state::WorldState;

/// The chance in eight, per beat, that an idle wanderer takes a step. Low enough
/// that a field of creatures drifts rather than marches.
const WANDER_IN_EIGHT: u32 = 3;

/// How many tiles a chase may plan across before it concludes there is no way.
/// Ample to round a building; an unreachable quarry is not worth more.
const PATH_BUDGET: usize = 400;

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

/// A creature this tough never runs — ServUO's "500 hits does not flee" rule.
const BRAVE_HITS: u16 = 500;

/// How close a foe may press a ranged fighter before it backs off — the
/// keep-away distance an archer or mage maintains.
const KITE_GAP: u32 = 2;

/// The first step from `from` toward `to`, planned *around* obstacles so a chaser
/// does not wedge itself against a wall. Falls back to the straight-line direction
/// when there is no map, or no route within the budget — better to close the gap
/// roughly and re-plan than to freeze.
pub fn step_toward(
    state: &WorldState,
    facet: u8,
    from: Point,
    to: Point,
    through_doors: bool,
) -> Option<u8> {
    // The live terrain, not the bare map: a route must not thread a placed
    // crate the step would then refuse. A door-opener plans through doors and
    // opens them on arrival.
    let planner = state.facet_state(facet).planning_terrain(through_doors);
    if let Some(path) = find_path(&planner, from, to, PATH_BUDGET) {
        return path.first().map(|d| d.to_bits());
    }
    direction_toward(from, to).map(Direction::to_bits)
}

/// One creature's beat: chase and fight what it has, pick a fight if it sees one,
/// or drift.
///
/// Returns the direction (0–7) the creature wants to step this beat — chasing a
/// foe or wandering — or `None` if it stood its ground (in reach of its target,
/// newly engaged, or idle). Engaging is done here, by giving the creature a
/// [`Combat`]; stepping is the caller's to apply, since the world owns movement.
pub fn think_one(state: &mut WorldState, creature: EntityId) -> Option<u8> {
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

    // Keep after a target that is still alive and in sight — close in if out of
    // reach, and leave the hitting to `swings`.
    if let Some(target_serial) = state
        .registry
        .get::<Combat>(creature)
        .and_then(|c| c.target)
    {
        if let Some(target_pos) = foe_in_sight(state, target_serial, pos, facet, chase_limit(sight))
        {
            if should_flee(state, creature, brain) {
                state.registry.remove::<ChasePath>(creature);
                return flee_step(state, creature, facet, pos, target_pos);
            }
            // A ranged fighter kites: back off from a foe at its heels, stand
            // and shoot inside its reach (the volley system does the firing),
            // and only close in when out of range or out of sight line.
            if let Some(&RangedAttack { range, .. }) = state.registry.get::<RangedAttack>(creature)
            {
                let gap = distance(pos, target_pos);
                if gap <= KITE_GAP {
                    state.registry.remove::<ChasePath>(creature);
                    return kite_step(state, facet, pos, target_pos);
                }
                let clear = state
                    .facet_state(facet)
                    .live_terrain()
                    .sight_clear(pos, target_pos);
                if gap <= u32::from(range) && clear {
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
    if sight > 0 && brain.aggression == Aggression::Aggressive {
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
        return Some(dir.to_bits());
    }
    None
}

/// How far a chase follows before it is abandoned — wider than the sight that
/// started it, so a quarry backing off does not flicker in and out of the fight.
fn chase_limit(sight: u8) -> u32 {
    (u32::from(sight) * CHASE_RANGE_FACTOR).max(CHASE_RANGE_MIN)
}

/// The position of `target` if it is still a live foe within `range` of `from`
/// on `facet`, or `None` if it has died, fled or vanished. Range only, no sight
/// line: both references acquire with line of sight and *pursue* on the cheaper
/// check, so a quarry that ducks behind a wall is chased around it, not lost.
fn foe_in_sight(
    state: &WorldState,
    target: Serial,
    from: Point,
    facet: u8,
    range: u32,
) -> Option<Point> {
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
fn probe(state: &WorldState, facet: u8, from: Point, dir: Direction) -> (bool, Option<EntityId>) {
    let Some(target) = step_from(from, dir) else {
        return (false, None);
    };
    let live = state.facet_state(facet).live_terrain();
    if live.can_step(from, target).is_some() {
        return (true, None);
    }
    let door = live
        .blocker_at(target.x, target.y)
        .filter(|o| o.door)
        .map(|o| o.entity);
    (false, door)
}

/// One step of a chase: follow the cached route, walk straight when nothing is
/// in the way, plan when something is, and give up — guard, then wander — when
/// there is no way at all.
fn chase_step(
    state: &mut WorldState,
    creature: EntityId,
    facet: u8,
    from: Point,
    to: Point,
    brain: Brain,
) -> Option<u8> {
    // A cached route first: planned once, followed a step per beat.
    if let Some(path) = state.registry.get::<ChasePath>(creature).cloned() {
        let stale = state.ticks.saturating_sub(path.planned_at) >= REPATH_TICKS
            || distance(path.goal, to) > GOAL_DRIFT
            || path.next >= path.steps.len();
        if stale {
            state.registry.remove::<ChasePath>(creature);
        } else {
            let dir = Direction::from_bits(path.steps[path.next]);
            let (open, door) = probe(state, facet, from, dir);
            if open {
                if will_move(state, creature, dir) {
                    let mut advanced = path;
                    advanced.next += 1;
                    state.registry.insert(creature, advanced);
                }
                return Some(dir.to_bits());
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
    let (open, door) = probe(state, facet, from, dir);
    if open {
        return Some(dir.to_bits());
    }
    if let Some(door) = door {
        if brain.opens_doors {
            items::open_door(state, door);
            return None;
        }
    }

    // Blocked: plan a route around. A door-opener plans through doors and
    // opens them on arrival.
    let planned = {
        let planner = state.facet_state(facet).planning_terrain(brain.opens_doors);
        find_path(&planner, from, to, PATH_BUDGET)
    };
    match planned {
        Some(steps) if !steps.is_empty() => {
            let bits: Vec<u8> = steps.iter().map(|d| d.to_bits()).collect();
            let first = Direction::from_bits(bits[0]);
            let (open, door) = probe(state, facet, from, first);
            if !open {
                if let Some(door) = door {
                    if brain.opens_doors {
                        items::open_door(state, door);
                        state.registry.insert(
                            creature,
                            ChasePath {
                                steps: bits,
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
                    steps: bits.clone(),
                    next,
                    goal: to,
                    planned_at: state.ticks,
                },
            );
            Some(bits[0])
        }
        _ => give_up(state, creature),
    }
}

/// No way to the quarry: drop it, stand watch a while, then go back to living.
/// The alternative — shuffling into the fence forever — is the bug this exists
/// to end.
fn give_up(state: &mut WorldState, creature: EntityId) -> Option<u8> {
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
    facet: u8,
    sight: u8,
) -> Option<Serial> {
    let facet_state = state.facet_state(facet);
    let live = facet_state.live_terrain();
    let sectors = &facet_state.sectors;
    let mut best: Option<(u32, Serial)> = None;
    for (id, pos) in sectors.nearby(from, u32::from(sight)) {
        if id == creature || !state.registry.has::<Client>(id) {
            continue;
        }
        if !in_range(from, pos, u32::from(sight)) {
            continue;
        }
        // Noticing needs a sight line — both reference emulators gate the
        // *acquisition* on line of sight and keep the chase itself on the
        // cheaper range check, and so does this.
        if !live.sight_clear(from, pos) {
            continue;
        }
        if state
            .registry
            .get::<Hitpoints>(id)
            .is_none_or(|h| h.current == 0)
        {
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
    facet: u8,
    from: Point,
    threat: Point,
) -> Option<u8> {
    // A runner does not also swing; drop the guard while running.
    if let Some(combat) = state.registry.get_mut::<Combat>(creature) {
        combat.warmode = false;
    }
    let away = direction_toward(threat, from).unwrap_or(Direction::South);
    for turn in [0u8, 1, 7, 2, 6, 3, 5] {
        let dir = Direction::from_bits((away.to_bits() + turn) & 7);
        let (open, _) = probe(state, facet, from, dir);
        if open {
            return Some(dir.to_bits());
        }
    }
    None
}

/// A step that opens distance without dropping the fight — the kiting half of
/// a ranged brain. Same search as fleeing, warmode kept.
fn kite_step(state: &mut WorldState, facet: u8, from: Point, threat: Point) -> Option<u8> {
    let away = direction_toward(threat, from).unwrap_or(Direction::South);
    for turn in [0u8, 1, 7, 2, 6] {
        let dir = Direction::from_bits((away.to_bits() + turn) & 7);
        let (open, _) = probe(state, facet, from, dir);
        if open {
            return Some(dir.to_bits());
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
        if state.registry.has::<Scripted>(victim) {
            continue;
        }
        // Being struck ends any standing watch on the spot.
        if let Some(b) = state.registry.get_mut::<Brain>(victim) {
            b.guard_until = 0;
        }
        let engaged = state
            .registry
            .get::<Combat>(victim)
            .and_then(|c| c.target)
            .is_some();
        if engaged || state.registry.entity_of(by).is_none() {
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
    }
}

/// The eight-way step that most reduces the gap from `from` to `to`, or `None`
/// when they share a tile. What a chaser walks along.
/// The eight-way direction from one tile toward another, or `None` when they are
/// the same tile. Shared by the creature brain and the townsfolk who turn to face
/// whoever they greet.
pub fn direction_toward(from: Point, to: Point) -> Option<Direction> {
    let dx = (i32::from(to.x) - i32::from(from.x)).signum();
    let dy = (i32::from(to.y) - i32::from(from.y)).signum();
    match (dx, dy) {
        (0, 0) => None,
        (0, -1) => Some(Direction::North),
        (1, -1) => Some(Direction::NorthEast),
        (1, 0) => Some(Direction::East),
        (1, 1) => Some(Direction::SouthEast),
        (0, 1) => Some(Direction::South),
        (-1, 1) => Some(Direction::SouthWest),
        (-1, 0) => Some(Direction::West),
        _ => Some(Direction::NorthWest),
    }
}
