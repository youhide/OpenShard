//! Walking a bench scenario in the window.
//!
//! `docs/client/evidence/2026-08-14-the-camera-rig-record.md`, C4. The bench flies a rig over a [`Script`] as a *gaze* — a
//! position as a function of time, with no crowd, no glide and no prediction
//! behind it — which is what makes ten thousand frames cost a millisecond. This
//! is the other reading of the same script: its [`Knot`]s are events, and each
//! one is handed to the real [`Crowd`](crate::crowd::Crowd) as the packet it
//! stands for. A crossing is a step to be glided and a jump is a body to be put
//! down, which is exactly the difference between `0x77` and a `0x22` that
//! refused one.
//!
//! Two readings of one list, and that is the point: the offline table says what
//! a rig does to a scripted body, and the window says what it does to the body
//! the client actually draws — through `Crowd`'s own glide, the animation clock,
//! and whatever the event loop's cadence turned out to be.
//!
//! # What this deliberately does not reproduce
//!
//! **The crossing is the client's, not the script's.** `Crowd::see` decides how
//! long a tile takes from the pace it is told and the gap it measures, and a
//! replay does not override it — a replay that forced the script's `takes` would
//! be measuring a glide nobody will ever see. The scripts are all written at the
//! walking pace, so the two agree; where they would not, the client's is the one
//! worth looking at.
//!
//! **A replay is offline only.** With a shard connected the body goes where the
//! `0x22` says it went, and a second writer would be two clients fighting over
//! one character.

use std::time::Duration;

use openshard_client_render::bench::{
    Knot,
    Script,
};
use openshard_movement::direction_toward;
use openshard_protocol::direction::{
    Direction,
    Facing,
};
use openshard_protocol::world::Point;

/// One knot, resolved into what the crowd is to be told.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Move {
    /// Where the body goes, with the script's height carried onto the ground it
    /// is being replayed over — see [`Replay::new`].
    pub to:     Point,
    /// Which way it is facing on the way. A jump that changes only the height
    /// keeps whatever it was facing: nobody turns to fall through a floor.
    pub facing: Facing,
    /// Whether to walk it there or put it there. A crossing is glided and a jump
    /// is snapped, which is the whole of what a `takes` of zero means.
    pub glided: bool,
}

/// A scenario being walked in the window, and how far through it is.
pub struct Replay {
    script: Script,
    /// Its own clock, advanced by the frame's elapsed span. Not an `Instant`:
    /// the script's instants are measured from its own start, and a replay that
    /// read the wall clock would drift from the trace the scope is recording.
    at:     Duration,
    /// The next knot to fire. Every knot before it has been handed over.
    next:   usize,
    /// What the script's `z = 0` means on this map: the ground under the tile it
    /// starts on.
    ///
    /// The scripts are written about a body at heights of their own — a stair
    /// climbs five units, a dungeon drops twenty — and the map has a terrain of
    /// its own under them. Anchoring the two here is what makes a flat script
    /// walk along the ground rather than through the air, and it is deliberately
    /// one offset for the whole run: re-reading the terrain under each tile
    /// would replace the script's height with the map's, which is the one signal
    /// these scenarios exist to deliver.
    ground: i8,
    /// Which way the body was last sent, for the jumps that name no direction.
    facing: Direction,
}

impl Replay {
    /// Start `script` over ground that is `ground` units up.
    pub fn new(script: Script, ground: i8) -> Self {
        Self {
            script,
            at: Duration::ZERO,
            next: 0,
            ground,
            facing: Direction::East,
        }
    }

    /// What it is called.
    pub fn name(&self) -> &'static str {
        self.script.name
    }

    /// How far in, and how long altogether — for a progress bar and for a caller
    /// that wants to say which second of the scenario a number came from.
    pub fn at(&self) -> Duration {
        self.at
    }

    /// Likewise.
    pub fn length(&self) -> Duration {
        self.script.length
    }

    /// Where the body has to stand before the first frame.
    ///
    /// The tile the first knot steps off. A caller puts the body there and cuts
    /// the camera to it: easing across a facet to the start of a scenario is a
    /// second motion on top of the one being measured.
    ///
    /// `None` for a script that never moves — `stand_still` is one — and that is
    /// absence rather than a default: it has no start tile, and the body it is
    /// about is whichever one is already standing there.
    pub fn start(&self) -> Option<Point> {
        self.script.knots().first().map(|knot| self.anchored(knot.from))
    }

    /// Whether there is nothing left to do.
    pub fn finished(&self) -> bool {
        self.next >= self.script.knots().len() && self.at >= self.script.length
    }

    /// One frame: everything the script asks for as of the instant it starts.
    ///
    /// A [`Vec`] and not an `Option` because a stalled frame can cover several
    /// knots at once — a replay is not allowed to fall behind its own script, or
    /// a scenario would mean something different on a machine that dropped a
    /// frame.
    ///
    /// # The clock is read before it is advanced, and it matters
    ///
    /// A knot is due against the instant this frame *begins*, and the increment
    /// comes after. The caller has already moved the crowd's clock by the same
    /// span, so a step handed over now starts its glide at the crowd's present
    /// instant and ends one crossing later — which means consecutive knots have
    /// to reach the crowd exactly one crossing apart.
    ///
    /// Advancing first breaks that on the very first one: the knot at zero would
    /// fire on the frame ending at 16ms and the knot at 400 on the frame ending
    /// at 400, so the first gap is a frame short. The crowd then sees a new step
    /// while the last one still has a frame to run, the body is yanked onto the
    /// tile it had not quite reached, and the eye's speed doubles for one frame —
    /// which is exactly the stutter this whole plan is about, manufactured by
    /// the harness that is meant to be measuring it.
    pub fn advance(&mut self, dt: Duration) -> Vec<Move> {
        let mut moves = Vec::new();
        while let Some(knot) = self.script.knots().get(self.next) {
            if knot.at > self.at {
                break;
            }
            self.next += 1;
            moves.push(self.resolve(*knot));
        }
        self.at += dt;
        moves
    }

    /// One knot, in this map's heights and with a direction to face.
    fn resolve(&mut self, knot: Knot) -> Move {
        let (from, to) = (self.anchored(knot.from), self.anchored(knot.to));
        // `direction_toward` reads the two axes a map has and answers `None` for
        // a body already standing there — which is exactly the kerb case, where
        // only the height changed.
        if let Some(direction) = direction_toward(from, to) {
            self.facing = direction;
        }
        Move {
            to,
            facing: Facing::walking(self.facing),
            glided: !knot.takes.is_zero(),
        }
    }

    /// A scripted tile, lifted onto the ground this replay is being walked over.
    ///
    /// Saturating rather than wrapping: a script that drops twenty units under a
    /// mountain top is a scenario about the camera, and an `i8` that wrapped
    /// would put the body in the sky.
    fn anchored(&self, point: Point) -> Point {
        Point::new(point.x, point.y, point.z.saturating_add(self.ground))
    }
}

#[cfg(test)]
mod tests {
    use openshard_client_render::bench::scripts;
    use openshard_movement::WALK_HOLD;

    use super::*;

    fn named(name: &str) -> Script {
        scripts()
            .into_iter()
            .find(|script| script.name == name)
            .expect("a script the bench ships")
    }

    /// The frame a knot falls in is the frame it fires on, and every knot fires
    /// exactly once — including the ones a stalled frame jumped over.
    #[test]
    fn every_knot_fires_once_and_in_order() {
        let script = named("ten_east");
        let knots = script.knots().len();
        let mut replay = Replay::new(script, 0);
        let mut fired = Vec::new();
        // Ten frames of 16ms, then one of a whole second: a stall does not lose
        // the steps it covered, it delivers them.
        for _ in 0..10 {
            fired.extend(replay.advance(Duration::from_millis(16)));
        }
        assert!(fired.len() == 1, "only the first step is due yet");
        // A stall of a second, and then the frame that reads the clock it left:
        // what it covered is delivered whole and in order rather than dropped.
        fired.extend(replay.advance(Duration::from_secs(1)));
        fired.extend(replay.advance(Duration::from_millis(16)));
        assert_eq!(fired.len(), 3, "a second covers two more holds");
        while !replay.finished() {
            fired.extend(replay.advance(WALK_HOLD));
        }
        assert_eq!(fired.len(), knots, "every knot, once");
        // Ten steps east, so the body ends ten tiles east of where it started,
        // one tile at a time and never twice on the same one.
        let start = replay.start().expect("ten steps have a first one");
        assert_eq!(fired.last().unwrap().to.x, start.x + 10);
        for (index, step) in fired.iter().enumerate() {
            assert_eq!(step.to.x, start.x + index as u16 + 1);
            assert!(step.glided, "a crossing is walked");
        }
    }

    /// A jump is put down rather than walked, which is the distinction the whole
    /// module exists to carry: gliding into a tile the body never crossed draws
    /// it strolling backwards.
    #[test]
    fn a_rollback_is_snapped_and_a_step_is_glided() {
        let script = named("rollback");
        let mut replay = Replay::new(script, 0);
        let mut fired = Vec::new();
        while !replay.finished() {
            fired.extend(replay.advance(Duration::from_millis(16)));
        }
        let jumps: Vec<_> = fired.iter().filter(|step| !step.glided).collect();
        assert_eq!(jumps.len(), 1, "one correction");
        assert_eq!(fired.iter().filter(|step| step.glided).count(), 4);
        // And it puts the body back one tile west of where the third step left
        // it, which is what a `0x21` refusing a step looks like.
        assert_eq!(jumps[0].to.x, replay.start().unwrap().x + 2);
    }

    /// The script's heights are carried onto the ground the replay is walked
    /// over, rather than replacing it or being replaced by it.
    #[test]
    fn the_scripts_height_is_added_to_the_ground_it_is_walked_over() {
        let mut flat = Replay::new(named("ten_east"), -40);
        assert_eq!(flat.start().unwrap().z, -40);
        assert!(flat.advance(WALK_HOLD).iter().all(|step| step.to.z == -40));

        let mut stairs = Replay::new(named("stairs"), -40);
        // Ten seconds, and then the frame that reads the clock they left: the
        // whole flight arrives at once — see `advance` on why it is read first.
        let mut climbed = stairs.advance(Duration::from_secs(10));
        climbed.extend(stairs.advance(Duration::from_millis(16)));
        assert_eq!(
            climbed.first().unwrap().to.z,
            -35,
            "five units up from the ground"
        );
        assert_eq!(climbed.last().unwrap().to.z, 10, "ten risers of five");
    }

    /// And the whole of it, driven through the units the window drives: the
    /// crowd's glide, `mobiles::gaze`, and a real [`Follower`].
    ///
    /// The claim this pins is the one the scope in the HUD is worth anything
    /// for — that a scenario replayed against the *client's* body is the same
    /// walk the bench flies against a scripted one. The peaks agree within a few
    /// per cent, and a rig tuned on one is therefore tuned on the other. Without
    /// it the panel is a chart of a body that arrived by magic.
    ///
    /// The companions are asserted with it, because a still body is perfectly
    /// smooth and this repository has shipped that green before.
    #[test]
    fn a_replayed_scenario_walks_the_same_body_the_bench_scripts() {
        use openshard_client_render::bench::{
            self,
            Cadence,
            Metrics,
            Scope,
        };
        use openshard_client_render::follow::{
            Follower,
            Rig,
        };
        use openshard_client_render::mobiles;
        use openshard_protocol::wire::{
            Graphic,
            Hue,
        };

        let frame = Duration::from_millis(16);
        let script = named("ten_east");
        let mut replay = Replay::new(script.clone(), 0);
        let start = replay.start().expect("ten steps have a first one");

        let mut crowd = crate::crowd::Crowd::default();
        // Our own steps: the crossing is the nominal one, which is what the
        // window does for the body it issues steps for.
        crowd.commanding(None);
        let (body, hue) = (Graphic(400), Hue::NONE);
        let mut mobile = crowd.snap(
            None,
            start,
            body,
            Facing::walking(Direction::East),
            hue,
            false,
            false,
        );
        let mut follower = Follower::new(Rig::HARD);
        let mut scope = Scope::new(Duration::from_secs(60));

        let mut elapsed = Duration::ZERO;
        while elapsed <= script.length {
            crowd.advance(frame);
            for step in replay.advance(frame) {
                mobile = match step.glided {
                    true => crowd.see(None, step.to, body, step.facing, hue, false, false),
                    false => crowd.snap(None, step.to, body, step.facing, hue, false, false),
                };
            }
            // Read every frame and not stored: a glide is a position off a
            // clock, and one read once freezes.
            mobile.drawn = crowd.drawn_for(None).expect("the crowd knows this body");
            let gaze = mobiles::gaze(&mobile);
            // Rounded to a whole virtual pixel, which is what the scope and the
            // bench both measure in — see `bench::run`.
            let eye = follower.advance(gaze, frame).pixel();
            scope.record(frame, gaze, eye, follower.exact().unwrap());
            elapsed += frame;
        }

        let walked = Metrics::of(scope.samples());
        let scripted = Metrics::of(&bench::run(Rig::HARD, &script, Cadence::steady(frame)).samples);
        assert!(walked.frames > 200, "{} frames", walked.frames);
        assert!(walked.travel > 300.0, "{} pixels", walked.travel);
        assert!(
            (walked.speed_max - scripted.speed_max).abs() < scripted.speed_max * 0.05,
            "{} against the bench's {}",
            walked.speed_max,
            scripted.speed_max,
        );
        // The reference rig is the body on both, so neither trails by more than
        // the quantiser — the half-pixel that says the eye and the sprite were
        // rounded from the same number.
        assert!(walked.lag_max < 0.71, "{}", walked.lag_max);
    }

    /// A change of height alone is not a turn: nobody faces a different way to
    /// fall through a floor, and a `direction_toward` of `None` is what says so.
    #[test]
    fn a_height_only_jump_keeps_the_facing() {
        let mut replay = Replay::new(named("kerb"), 0);
        let mut fired = Vec::new();
        while !replay.finished() {
            fired.extend(replay.advance(Duration::from_millis(16)));
        }
        assert!(fired.len() >= 3);
        assert!(
            fired.iter().all(|step| step.facing.direction == Direction::East),
            "the step east is the only thing that named a direction",
        );
    }
}
