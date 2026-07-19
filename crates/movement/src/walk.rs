//! Turning a walk request into a step, or a refusal.

use std::time::Instant;

use openshard_protocol::{Direction, Facing, Point, WalkRequest};

use crate::pace::{Pace, WalkPace};
use crate::sequence::WalkSequence;

/// What a walk request did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[must_use = "a walk outcome has to reach the client, or it will stop walking"]
pub enum Walk {
    /// The mobile turned on the spot and did not move.
    ///
    /// UO makes turning a whole step: a mobile facing north that is asked to go
    /// east *turns* east and stays put, and only the next request moves it.
    /// This is not a quirk to paper over — the client animates the turn and
    /// expects the ack, so collapsing it into a move desynchronises the two.
    Turned {
        /// The new facing.
        facing: Facing,
    },
    /// The mobile took a step.
    Moved {
        /// Where it is now.
        position: Point,
        /// Which way it is facing.
        facing: Facing,
    },
    /// The step is refused. The client snaps back and resets its sequence.
    Refused,
}

/// Whether a mobile may stand somewhere.
///
/// A trait because the answer needs the map, the statics, the multis and every
/// other mobile, and this crate should know about none of them. `openshard-world`
/// implements it over the client's files; [`OpenWorld`] is the answer when there
/// are none.
pub trait Terrain {
    /// Can a mobile at `from` step to `to`?
    ///
    /// `to`'s `z` is a guess from the caller; an implementation that knows the
    /// map should correct it and return the real height.
    fn can_step(&self, from: Point, to: Point) -> Option<Point>;

    /// The ground height at `(x, y)`, if this terrain knows one.
    ///
    /// Where a character spawns: the map holds the floor, not the config. An
    /// implementation with no map — [`OpenWorld`] — returns `None`, and the
    /// caller falls back to a flat default.
    fn ground_z(&self, _x: u16, _y: u16) -> Option<i8> {
        None
    }

    /// The static tiles standing at `(x, y)`, appended to `out` as
    /// `(graphic, z)` pairs.
    ///
    /// Only what the map holds; a terrain with no statics (an open world) adds
    /// nothing. The primitive tuple keeps this trait — which lives below `world` —
    /// free of the map's own types. Used to find door frames when generating the
    /// functional doors a building's static art only implies.
    fn statics_at(&self, _x: u16, _y: u16, _out: &mut Vec<(u16, i8)>) {}

    /// The z a mobile stands at on `(x, y)`, reached from near `near_z` — the top
    /// of the walkable surface there, a building's raised floor and all.
    ///
    /// Where a spawn drops onto the ground: the pack gives a tile and a rough
    /// height, and the map says which floor that lands on (asking from `near_z`
    /// rather than the sky, so it finds the floor and not the roof above it).
    /// `None` when the tile has no reachable surface, or the terrain has no map.
    fn stand_z(&self, _x: u16, _y: u16, _near_z: i32) -> Option<i32> {
        None
    }

    /// Whether an object `height` tall can sit at `(x, y, z)` — nothing solid in
    /// its body, and a surface under it to rest on.
    ///
    /// This is what keeps a generated door in a real doorway: a door belongs in an
    /// open gap with a floor, so a spot that is a solid wall (something in the way)
    /// or thin air (no surface) reports that nothing fits. An open world fits
    /// everything — it has no walls and, having no map, generates no doors anyway.
    fn can_fit(&self, _x: u16, _y: u16, _z: i32, _height: i32) -> bool {
        true
    }
}

/// A world with no floor and no walls: every step is allowed, z never changes.
///
/// What a shard runs with no client files configured, and what these tests run
/// against. Useful for proving the handshake in isolation; useless as a game, so
/// the server warns at startup when it is in use.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct OpenWorld;

impl Terrain for OpenWorld {
    fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
        Some(to)
    }
}

/// One mobile's walk state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Walker {
    /// Where it is.
    pub position: Point,
    /// Which way it faces.
    pub facing: Facing,
    /// Its walk sequence.
    pub sequence: WalkSequence,
    /// How fast it is allowed to move.
    pub pace: WalkPace,
}

impl Walker {
    /// A walker standing at `position`, facing `facing`, fresh.
    pub const fn new(position: Point, facing: Facing) -> Self {
        Self {
            position,
            facing,
            sequence: WalkSequence::new(),
            pace: WalkPace::new(),
        }
    }

    /// Handle a `0x02` walk request.
    ///
    /// The caller sends `0x22` for [`Walk::Turned`] and [`Walk::Moved`], and
    /// `0x21` for [`Walk::Refused`].
    ///
    /// `now` is a parameter rather than read here, so that a whole walk — pace
    /// and all — is a deterministic test with no `sleep` in it.
    pub fn request(&mut self, request: WalkRequest, terrain: &dyn Terrain, now: Instant) -> Walk {
        if self.sequence.accept(request.sequence).is_err() {
            self.sequence.reject();
            return Walk::Refused;
        }

        // Turning is a step of its own. A mobile facing north asked to go east
        // turns to face east and stays where it is; the *next* request moves it.
        // The running bit is not part of this — a walking mobile asked to run
        // the way it already faces takes a step, it does not "turn".
        //
        // A turn is free: it costs no ground, so charging for it would let a
        // client be throttled for spinning on the spot, which is not a speedhack
        // and is something clients genuinely do.
        if self.facing.direction != request.facing.direction {
            self.facing = request.facing;
            return Walk::Turned {
                facing: self.facing,
            };
        }

        if self.pace.allow(now, request.facing.running) == Pace::TooFast {
            // Moving faster than a body can move. Refuse the step rather than
            // the connection: the client snaps back, which is what a legitimate
            // one needs and what an illegitimate one deserves.
            self.sequence.reject();
            return Walk::Refused;
        }

        let Some(target) = step_from(self.position, request.facing.direction) else {
            // Walked off the edge of the coordinate space. The client cannot
            // express where it wanted to go, so there is nowhere to allow.
            self.sequence.reject();
            return Walk::Refused;
        };

        let Some(landed) = terrain.can_step(self.position, target) else {
            self.sequence.reject();
            return Walk::Refused;
        };

        self.position = landed;
        self.facing = request.facing;
        Walk::Moved {
            position: self.position,
            facing: self.facing,
        }
    }
}

/// Where one step from `position` lands, or `None` at the world's edge.
///
/// The map is addressed with `u16`s, so a step west from x=0 has no
/// representation. Returning `None` rather than wrapping matters: wrapping would
/// teleport a mobile from Britain's west shore to the far east of the map.
pub fn step_from(position: Point, direction: Direction) -> Option<Point> {
    let (dx, dy) = direction.step();
    let x = u16::try_from(i32::from(position.x) + dx).ok()?;
    let y = u16::try_from(i32::from(position.y) + dy).ok()?;
    Some(Point {
        x,
        y,
        z: position.z,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(direction: Direction, sequence: u8) -> WalkRequest {
        WalkRequest {
            facing: Facing::walking(direction),
            sequence,
            fastwalk_key: 0,
        }
    }

    fn walker() -> Walker {
        Walker::new(Point::new(100, 100, 0), Facing::walking(Direction::North))
    }

    /// A fresh instant. The pace bucket starts full, so a handful of steps in
    /// one test never trip it.
    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn walking_the_way_you_face_moves_you() {
        let mut walker = walker();
        let outcome = walker.request(request(Direction::North, 0), &OpenWorld, now());
        assert_eq!(
            outcome,
            Walk::Moved {
                position: Point::new(100, 99, 0),
                facing: Facing::walking(Direction::North),
            }
        );
        assert_eq!(walker.position, Point::new(100, 99, 0));
    }

    #[test]
    fn turning_is_a_step_of_its_own() {
        // The thing that surprises people. A mobile facing north asked to go
        // east turns and stays put; the next request moves it. The client
        // animates the turn and waits for the ack, so collapsing this into a
        // move puts the two ends a tile apart.
        let mut walker = walker();
        let outcome = walker.request(request(Direction::East, 0), &OpenWorld, now());
        assert_eq!(
            outcome,
            Walk::Turned {
                facing: Facing::walking(Direction::East)
            }
        );
        assert_eq!(walker.position, Point::new(100, 100, 0), "did not move");

        // Now it moves.
        let outcome = walker.request(request(Direction::East, 1), &OpenWorld, now());
        assert_eq!(
            outcome,
            Walk::Moved {
                position: Point::new(101, 100, 0),
                facing: Facing::walking(Direction::East),
            }
        );
    }

    #[test]
    fn a_turn_still_consumes_a_sequence_number() {
        // It is a step as far as the client is concerned, and it gets an ack.
        let mut walker = walker();
        let _ = walker.request(request(Direction::East, 0), &OpenWorld, now());
        assert_eq!(walker.sequence.expected(), 1);
    }

    #[test]
    fn starting_to_run_the_way_you_face_is_a_step_not_a_turn() {
        // The running bit changes but the direction does not, so there is
        // nothing to turn to. Treating this as a turn would cost a step every
        // time a player broke into a run.
        let mut walker = walker();
        let outcome = walker.request(
            WalkRequest {
                facing: Facing::running(Direction::North),
                sequence: 0,
                fastwalk_key: 0,
            },
            &OpenWorld,
            now(),
        );
        assert!(matches!(outcome, Walk::Moved { .. }));
        assert!(walker.facing.running);
    }

    #[test]
    fn every_direction_steps_the_right_way() {
        for direction in Direction::ALL {
            let mut walker = Walker::new(Point::new(100, 100, 0), Facing::walking(direction));
            let outcome = walker.request(request(direction, 0), &OpenWorld, now());

            let (dx, dy) = direction.step();
            let expected = Point::new((100 + dx) as u16, (100 + dy) as u16, 0);
            assert_eq!(
                outcome,
                Walk::Moved {
                    position: expected,
                    facing: Facing::walking(direction),
                },
                "{direction}"
            );
        }
    }

    #[test]
    fn a_fresh_walker_that_does_not_start_at_zero_is_refused() {
        let mut walker = walker();
        assert_eq!(
            walker.request(request(Direction::North, 5), &OpenWorld, now()),
            Walk::Refused
        );
        assert_eq!(walker.position, Point::new(100, 100, 0), "did not move");
        assert!(walker.sequence.is_fresh(), "and stays fresh");
    }

    #[test]
    fn a_refusal_resets_the_sequence() {
        let mut walker = walker();
        let _ = walker.request(request(Direction::North, 0), &OpenWorld, now());
        let _ = walker.request(request(Direction::North, 1), &OpenWorld, now());

        // A wall.
        struct Wall;
        impl Terrain for Wall {
            fn can_step(&self, _from: Point, _to: Point) -> Option<Point> {
                None
            }
        }

        assert_eq!(
            walker.request(request(Direction::North, 2), &Wall, now()),
            Walk::Refused
        );
        assert!(
            walker.sequence.is_fresh(),
            "the client resets on 0x21, so the server must too"
        );
    }

    #[test]
    fn terrain_can_move_a_step_somewhere_else() {
        // What real terrain does: the caller guesses a z, the map corrects it.
        // Walking up a hill lands you higher than you asked for.
        struct Hill;
        impl Terrain for Hill {
            fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
                Some(Point { z: to.z + 5, ..to })
            }
        }

        let mut walker = walker();
        let outcome = walker.request(request(Direction::North, 0), &Hill, now());
        assert_eq!(
            outcome,
            Walk::Moved {
                position: Point::new(100, 99, 5),
                facing: Facing::walking(Direction::North),
            }
        );
        assert_eq!(walker.position.z, 5, "the walker believes the terrain");
    }

    #[test]
    fn the_world_edge_refuses_rather_than_wrapping() {
        // A step west from x=0 has no u16 to land on. Wrapping would put the
        // walker at x=65535 — the far side of the map, instantly.
        let mut walker = Walker::new(Point::new(0, 0, 0), Facing::walking(Direction::West));
        assert_eq!(
            walker.request(request(Direction::West, 0), &OpenWorld, now()),
            Walk::Refused
        );
        assert_eq!(walker.position, Point::new(0, 0, 0));

        let mut walker = Walker::new(
            Point::new(u16::MAX, u16::MAX, 0),
            Facing::walking(Direction::SouthEast),
        );
        assert_eq!(
            walker.request(request(Direction::SouthEast, 0), &OpenWorld, now()),
            Walk::Refused
        );
    }

    #[test]
    fn step_from_refuses_every_edge() {
        for direction in Direction::ALL {
            let (dx, dy) = direction.step();
            if dx < 0 || dy < 0 {
                assert_eq!(
                    step_from(Point::new(0, 0, 0), direction),
                    None,
                    "{direction} from the origin"
                );
            }
            if dx > 0 || dy > 0 {
                assert_eq!(
                    step_from(Point::new(u16::MAX, u16::MAX, 0), direction),
                    None,
                    "{direction} from the far corner"
                );
            }
        }
    }

    #[test]
    fn step_from_keeps_the_height() {
        // Height is the terrain's business, not the step's.
        let point = Point::new(100, 100, -20);
        assert_eq!(
            step_from(point, Direction::North),
            Some(Point::new(100, 99, -20))
        );
    }

    #[test]
    fn a_walk_around_the_block_returns_home() {
        let mut walker = Walker::new(Point::new(100, 100, 0), Facing::walking(Direction::North));
        let mut sequence = 0u8;
        let mut step = |walker: &mut Walker, direction: Direction| {
            // Two requests per direction: one turns, one moves.
            for _ in 0..2 {
                let _ = walker.request(request(direction, sequence), &OpenWorld, now());
                sequence = sequence.wrapping_add(1);
            }
        };

        step(&mut walker, Direction::East);
        step(&mut walker, Direction::South);
        step(&mut walker, Direction::West);
        step(&mut walker, Direction::North);

        assert_eq!(
            walker.position,
            Point::new(100, 100, 0),
            "four turns and four steps come back to the start"
        );
    }
}
