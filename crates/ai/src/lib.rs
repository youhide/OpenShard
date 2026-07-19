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
use openshard_entities::{EntityId, Serial};
use openshard_movement::{find_path, Terrain};
use openshard_protocol::{Direction, Point};
use openshard_state::components::{Brain, Client, Combat, Heading, Hitpoints, Position};
use openshard_state::sectors::{distance, in_range};
use openshard_state::WorldState;

/// The chance in eight, per beat, that an idle wanderer takes a step. Low enough
/// that a field of creatures drifts rather than marches.
const WANDER_IN_EIGHT: u32 = 3;

/// How many tiles a chase may plan across before it gives up and just heads the
/// straight way. Ample to round a building; an unreachable or very distant quarry
/// is not worth a bigger search each beat.
const PATH_BUDGET: usize = 400;

/// The first step from `from` toward `to`, planned *around* obstacles so a chaser
/// does not wedge itself against a wall. Falls back to the straight-line direction
/// when there is no map, or no route within the budget — better to close the gap
/// roughly and re-plan than to freeze.
pub fn step_toward(state: &WorldState, facet: u8, from: Point, to: Point) -> Option<u8> {
    // The live terrain, not the bare map: a route must not thread a closed door
    // or a placed crate the step would then refuse.
    let live = state.facet_state(facet).live_terrain();
    if let Some(path) = find_path(&live, from, to, PATH_BUDGET) {
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
    let &Brain { sight, wander, .. } = state.registry.get::<Brain>(creature)?;
    let facet = state.facet_of(creature);

    // Keep after a target that is still alive and in sight — close in if out of
    // reach, and leave the hitting to `swings`.
    if let Some(target_serial) = state
        .registry
        .get::<Combat>(creature)
        .and_then(|c| c.target)
    {
        if let Some(target_pos) = foe_in_sight(state, target_serial, pos, facet, sight) {
            if !in_range(pos, target_pos, combat::MELEE_RANGE) {
                // Plan around walls rather than shuffle into them.
                return step_toward(state, facet, pos, target_pos);
            }
            return None;
        }
        combat::clear_target(state, creature);
    }

    // Nothing to fight: look for prey, or wander.
    if sight > 0 {
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

/// The position of `target` if it is still a live foe within `sight` of `from` on
/// `facet`, or `None` if it has died, fled or vanished.
fn foe_in_sight(
    state: &WorldState,
    target: Serial,
    from: Point,
    facet: u8,
    sight: u8,
) -> Option<Point> {
    let entity = state.registry.entity_of(target)?;
    let &Position(pos) = state.registry.get::<Position>(entity)?;
    let alive = state
        .registry
        .get::<Hitpoints>(entity)
        .is_some_and(|h| h.current > 0);
    (alive && state.facet_of(entity) == facet && in_range(from, pos, u32::from(sight)))
        .then_some(pos)
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
