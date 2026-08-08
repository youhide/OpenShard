//! What a rig did to a scripted walk, and how ragged it was.
//!
//! The bench of `docs/camera.md`, C1. A [`Script`] is a body's whole path as a
//! function of a virtual instant, [`run`] walks one at a chosen frame rate
//! through a [`Follower`], and [`Metrics`] is what comes out. No window, no
//! shard, no clock and no files: ten thousand frames cost under a millisecond,
//! and a failing run is a script name and a cadence rather than a story. What
//! writes the numbers and the pictures out is `tests/camera.rs`, because this
//! crate does not open files.
//!
//! # Two traces of the same eye, and each number takes one of them
//!
//! [`Sample`] carries the eye twice: [`Sample::eye`] is the whole pixel the
//! screen was given and [`Sample::state`] is what the filter had before the
//! quantiser rounded it. That is not redundancy, it is the only way either
//! number means anything.
//!
//! At one-pixel quantisation and sixty frames a second, a body walking at 55
//! pixels a second moves the eye 0.9 of a pixel per frame, so the *drawn* eye
//! moves `1, 1, 0, 1, 1, 0` — and the acceleration of that sequence is
//! ±3,900 px/s² of pure rounding, which is the same order as the reversal a
//! camera exists to smooth. Differentiating the drawn trace therefore measures
//! the quantiser and calls it the rig. So the derivatives are taken on the
//! unrounded trace, where they say what the filter did, and the quantiser gets
//! its own metric — [`Metrics::step_var`] — where the unevenness *is* the
//! quantity of interest.
//!
//! What stays on the drawn trace is everything about position: the lag, the
//! overshoot, the distance travelled. Those are what the player sees.
//!
//! # The same arithmetic, offline and live
//!
//! [`Metrics`] and [`readings`] take a slice of [`Sample`]s and nothing else, so
//! the offline runner, the DST harness and the scope in the window compute the
//! same numbers from the same code — which is the only reason a number from one
//! of them can be compared with a number from another. [`Scope`] is the ring the
//! window feeds: the last few seconds of frames, on a clock of elapsed spans
//! rather than an [`std::time::Instant`], so this crate still reads no clock.
//!
//! # The kinematics are the oracle's, deliberately
//!
//! A scripted step crosses one tile at constant speed over one hold, through
//! the same [`Gaze::back_towards`] a real glide uses. That is the simplest
//! model that could be right, and it is the same one `dst.rs` holds the real
//! walk against. It is *not* a substitute for the real walk — a rig that only
//! looks good here has been fitted to a body with no wire, no prediction and no
//! rollback behind it — which is why the same [`Metrics`] are run over the DST
//! harness's own trace as well.

use std::time::Duration;

use openshard_movement::WALK_HOLD;
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;

use crate::camera::WorldPixel;
use crate::follow::{Follower, Gaze, Rig};

/// Two `f64`s of world pixel space: a position, a step, a speed or an
/// acceleration, depending on what took the difference. Named because half of
/// what is measured here is one, and because two `Option`s of a bare pair in one
/// binding is a type nobody can read.
type Pixels = (f64, f64);

/// One crossing: where the body goes, from when, over how long.
///
/// A `takes` of zero is a body put somewhere between two frames — a rollback, a
/// recall, a floor changing under it — and not a very fast walk. That
/// distinction is the whole of what a replay in the window needs to know, which
/// is why the fields are public: a crossing is a step to glide and a jump is a
/// body to put down.
#[derive(Clone, Copy, Debug)]
pub struct Knot {
    /// When it starts, from the start of the script.
    pub at: Duration,
    /// How long it takes. Zero is a jump.
    pub takes: Duration,
    /// The tile left behind.
    pub from: Point,
    /// The tile arrived at.
    pub to: Point,
}

/// A body's whole path, as a function of a virtual instant.
///
/// Built by naming what the body does in order; the clock and the tile it is
/// standing on are carried along as it is built, so a script reads as the
/// scenario it is named after.
#[derive(Clone, Debug)]
pub struct Script {
    /// What this scenario is called, for the table and the file it is written
    /// to.
    pub name: &'static str,
    /// How long it lasts. Every cadence covers exactly this span, which is what
    /// makes two frame rates comparable at all.
    pub length: Duration,
    knots: Vec<Knot>,
    /// Where the body is now, while the script is being built.
    at: Point,
}

impl Script {
    /// A body standing at `start`, with nothing scripted yet.
    pub fn new(name: &'static str, start: Point) -> Self {
        Self {
            name,
            length: Duration::ZERO,
            knots: Vec::new(),
            at: start,
        }
    }

    /// Stand still for a while.
    pub fn stand(mut self, how_long: Duration) -> Self {
        self.length += how_long;
        self
    }

    /// Walk to a neighbouring tile over `takes`, at constant speed.
    pub fn step(self, direction: Direction, takes: Duration) -> Self {
        let (dx, dy) = direction.step();
        let to = Point::new(
            (i32::from(self.at.x) + dx) as u16,
            (i32::from(self.at.y) + dy) as u16,
            self.at.z,
        );
        self.cross(to, takes)
    }

    /// The same, but the tile also changes height — a stair, a slope, a bridge.
    pub fn step_up(self, direction: Direction, rise: i8, takes: Duration) -> Self {
        let (dx, dy) = direction.step();
        let to = Point::new(
            (i32::from(self.at.x) + dx) as u16,
            (i32::from(self.at.y) + dy) as u16,
            self.at.z + rise,
        );
        self.cross(to, takes)
    }

    /// Put the body somewhere else between two frames.
    ///
    /// A rollback, a recall, a resurrection: the discontinuities a rig either
    /// absorbs or cuts across, and the reason a camera needs a rule about which.
    pub fn jump(self, to: Point) -> Self {
        self.cross(to, Duration::ZERO)
    }

    fn cross(mut self, to: Point, takes: Duration) -> Self {
        self.knots.push(Knot {
            at: self.length,
            takes,
            from: self.at,
            to,
        });
        self.at = to;
        self.length += takes;
        self
    }

    /// Everything the body does, in order.
    ///
    /// For a replay that drives a real body rather than a gaze: the window walks
    /// a crowd through these, and [`Script::gaze_at`] is what the bench uses.
    /// Two readings of one script, and they agree because they are one list.
    pub fn knots(&self) -> &[Knot] {
        &self.knots
    }

    /// Where the body is at a virtual instant.
    ///
    /// Between two knots it stands on the tile the last one left it at, which
    /// is what makes `stand` a gap rather than an entry.
    pub fn gaze_at(&self, when: Duration) -> Gaze {
        let mut current = self
            .knots
            .first()
            .map_or(Gaze::default(), |first| Gaze::on(first.from));
        for knot in &self.knots {
            if when < knot.at {
                break;
            }
            let arrived = when >= knot.at + knot.takes;
            current = match (arrived, knot.takes.is_zero()) {
                // A crossing that is over, and a jump, which is over the moment
                // it starts. Written as one case because dividing by `takes`
                // below would be a `NaN` for the second, and a `NaN` gaze is
                // stored, compared falsely against ever after, and never
                // reported.
                (true, _) | (_, true) => Gaze::on(knot.to),
                (false, false) => {
                    let left = 1.0 - (when - knot.at).as_secs_f64() / knot.takes.as_secs_f64();
                    Gaze::on(knot.to).back_towards(Gaze::on(knot.from), left)
                }
            };
        }
        current
    }
}

/// How the frames are spaced.
///
/// A steady cadence is a metronome no real event loop is; the spread is what a
/// window manager, a swapchain and a garbage collector do to one. Seeded, so a
/// jittery run that finds something is a number and not an anecdote.
#[derive(Clone, Copy, Debug)]
pub struct Cadence {
    step: Duration,
    spread: Duration,
    seed: u64,
}

impl Cadence {
    /// A frame every `step`, exactly.
    pub const fn steady(step: Duration) -> Self {
        Self {
            step,
            spread: Duration::ZERO,
            seed: 0,
        }
    }

    /// A frame every `step`, plus up to `spread` of lateness.
    ///
    /// Never early: a loop woken by an operating system is not.
    pub const fn jittered(step: Duration, spread: Duration, seed: u64) -> Self {
        Self { step, spread, seed }
    }

    /// The next frame's length. Floored at a millisecond, because a cadence of
    /// zero is not a frame rate, it is a loop that never advances.
    fn next(&mut self) -> Duration {
        let step = self.step.max(Duration::from_millis(1));
        if self.spread.is_zero() {
            return step;
        }
        // SplitMix64, four lines and no dependency.
        self.seed = self.seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.seed;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        step + Duration::from_nanos(z % (self.spread.as_nanos() as u64 + 1))
    }
}

/// One frame of a run.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// When, from the start of the script.
    pub at: Duration,
    /// Where the body was asking to be looked at.
    pub gaze: Gaze,
    /// The whole pixel the screen was given.
    pub eye: WorldPixel,
    /// What the filter had before the quantiser rounded it, still in channels —
    /// see this module's note on why both are kept. [`Gaze::exact`] folds it
    /// into the pair the derivatives are taken on; the `lift` field is what
    /// says how far the *height* is behind, which a folded number cannot.
    pub state: Gaze,
}

/// A rig, a script, and every frame of the two together.
#[derive(Clone, Debug)]
pub struct Trace {
    /// What was being flown.
    pub rig: Rig,
    /// What it was flown over.
    pub script: &'static str,
    /// Every frame, in order.
    pub samples: Vec<Sample>,
}

/// Fly a rig over a script at a cadence.
///
/// The clock lands exactly on the script's end whatever the cadence, by
/// shortening the last frame: two frame rates that covered different spans of
/// time would be incomparable, and the difference would look exactly like the
/// frame-rate dependence this is meant to detect.
pub fn run(rig: Rig, script: &Script, cadence: Cadence) -> Trace {
    let mut cadence = cadence;
    let mut follower = Follower::new(rig);
    let mut samples = Vec::new();
    let mut now = Duration::ZERO;
    // The first frame has no elapsed time behind it: it places the eye rather
    // than moving it, exactly as the first frame after a cut does.
    let mut dt = Duration::ZERO;
    loop {
        let gaze = script.gaze_at(now);
        // Rounded to a whole virtual pixel, which is the bench's own quantum:
        // it flies at 1:1, where that is exactly the display's. A bench at a
        // magnification would want `Camera::snap` and a zoom to hand it — see
        // the backlog in `docs/camera.md`.
        let eye = follower.advance(gaze, dt).pixel();
        samples.push(Sample {
            at: now,
            gaze,
            eye,
            state: follower.exact().expect("advance has just placed the eye"),
        });
        if now >= script.length {
            break;
        }
        let next = (now + cadence.next()).min(script.length);
        dt = next - now;
        now = next;
    }
    Trace {
        rig,
        script: script.name,
        samples,
    }
}

/// One frame's worth of the numbers a curve is drawn from.
///
/// What [`Metrics`] takes its maxima of, kept per frame because a chart needs
/// the shape and a table needs the peak, and the two must not be two
/// arithmetics: a number that disagrees with the picture beside it means one of
/// them is wrong, and that has to be visible rather than arguable.
///
/// There is one of these per frame *after* the first, and none for a frame that
/// took no time: the first has nothing behind it to difference against, and a
/// zero-length frame would divide by it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Reading {
    /// When, from the start of the run.
    pub at: Duration,
    /// How far the *drawn* eye was from the body — what the player sees.
    pub lag: f64,
    /// How fast the eye moved, off the unrounded trace. See this module's note
    /// on why not the drawn one.
    pub speed: f64,
    /// And how sharply that changed. `None` on the first reading, which has no
    /// speed before it.
    pub accel: Option<f64>,
    /// The third difference — what "ragged" is, as a number. `None` for the
    /// first two, for the same reason.
    pub jerk: Option<f64>,
}

/// Every frame's derivatives, in order.
///
/// The one place the differencing is written. [`Metrics`] folds these into its
/// peaks and a chart draws them as they are, so a scope in the window and a
/// table from the offline runner cannot drift apart.
pub fn readings(samples: &[Sample]) -> Vec<Reading> {
    let mut out = Vec::with_capacity(samples.len().saturating_sub(1));
    // The previous frame's derivatives, for the ones above them. Deliberately
    // *not* cleared by a skipped frame: a frame of no elapsed time did not
    // happen as far as a difference is concerned, and forgetting the last
    // velocity across one would report a spurious acceleration on the next.
    let (mut last_speed, mut last_accel): (Option<Pixels>, Option<Pixels>) = (None, None);
    for (index, sample) in samples.iter().enumerate() {
        let Some(previous) = index.checked_sub(1).map(|before| &samples[before]) else {
            continue;
        };
        let dt = (sample.at - previous.at).as_secs_f64();
        if dt <= 0.0 {
            continue;
        }
        let body = sample.gaze.exact();
        let eye = (f64::from(sample.eye.x), f64::from(sample.eye.y));
        let speed = scaled(minus(sample.state.exact(), previous.state.exact()), 1.0 / dt);
        let accel = last_speed.map(|was| scaled(minus(speed, was), 1.0 / dt));
        let jerk = match (accel, last_accel) {
            (Some(accel), Some(before)) => Some(length(scaled(minus(accel, before), 1.0 / dt))),
            _ => None,
        };
        out.push(Reading {
            at: sample.at,
            lag: length(minus(eye, body)),
            speed: length(speed),
            accel: accel.map(length),
            jerk,
        });
        last_accel = accel.or(last_accel);
        last_speed = Some(speed);
    }
    out
}

/// The last few seconds of a live run, and nothing older.
///
/// What the scope in the window draws and what the panel beside it measures.
/// Its own clock, advanced by the span each frame covered rather than read from
/// an [`std::time::Instant`], because this crate does not read clocks — which is
/// also what lets a test hand it a cadence and get a trace it can assert on.
#[derive(Clone, Debug)]
pub struct Scope {
    span: Duration,
    at: Duration,
    samples: Vec<Sample>,
}

impl Scope {
    /// A scope holding `span` of frames.
    pub fn new(span: Duration) -> Self {
        Self {
            span,
            at: Duration::ZERO,
            samples: Vec::new(),
        }
    }

    /// One frame, `dt` after the last one.
    ///
    /// The three traces a [`Sample`] carries are the caller's to supply, and
    /// they are what the camera already has: the gaze it was handed, the pixel
    /// it gave the screen, and what the filter had before the quantiser.
    pub fn record(&mut self, dt: Duration, gaze: Gaze, eye: WorldPixel, state: Gaze) {
        self.at += dt;
        self.samples.push(Sample {
            at: self.at,
            gaze,
            eye,
            state,
        });
        // Everything older than the span goes, from the front, so the trace is
        // a window on the present rather than a session-long log: a chart of
        // twenty minutes of walking is a solid block of ink.
        let cutoff = self.at.saturating_sub(self.span);
        let keep = self
            .samples
            .iter()
            .position(|sample| sample.at >= cutoff)
            .unwrap_or(self.samples.len());
        self.samples.drain(..keep);
    }

    /// Every frame still held, oldest first.
    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// How long a window this keeps.
    pub fn span(&self) -> Duration {
        self.span
    }

    /// Keep a different length of window from now on.
    ///
    /// The trace is not cleared: the frames already held were flown by the same
    /// camera and are still a measurement of it — which is the difference
    /// between this and a rig swap. A window that shrank drops what no longer
    /// fits on the next frame recorded, from the front, the same way as always.
    pub fn set_span(&mut self, span: Duration) {
        self.span = span;
    }

    /// The instant the last frame landed on, on this scope's own clock.
    pub fn at(&self) -> Duration {
        self.at
    }

    /// Throw the trace away and keep the clock.
    ///
    /// For a discontinuity that is not the camera's doing — a preset swapped, a
    /// script started — where the frames either side of it are two different
    /// runs and a metric over both is a number about nothing.
    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

/// What a run came to, in numbers.
///
/// Each of these is in world pixels — which is screen pixels at zoom 1, and the
/// bench does not zoom — and per second where a rate is meant. Which trace each
/// is taken from is the subject of this module's second note and is not a
/// detail: taken from the other one, half of them measure the quantiser.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Metrics {
    /// How many frames were drawn. Every other number is worthless without it.
    pub frames: usize,
    /// How far the drawn eye moved in total. The other half of "the data is
    /// real": an eye that never moved is smooth, still and useless.
    pub travel: f64,
    /// The worst distance from the drawn eye to the body, and the RMS of it.
    ///
    /// The reference camera's is under `sqrt(0.5)`, which is the quantiser and
    /// nothing else. Anything above that is the rig trailing.
    pub lag_max: f64,
    /// Likewise.
    pub lag_rms: f64,
    /// The worst the *height* alone was behind, in pixels.
    ///
    /// Apart from [`Metrics::lag_max`] because it is a different question with a
    /// different answer: a stair is 20 pixels of rise, so an eye 10 behind is
    /// half a riser down and one 40 behind is in a lift shaft. Folded into the
    /// distance above it is indistinguishable from trailing on the flat.
    pub lift_lag_max: f64,
    /// The furthest the eye got *ahead* of the body along its direction of
    /// travel — overshoot, and negative everywhere means there was none.
    pub ahead_max: f64,
    /// How fast the eye itself moved, at most.
    pub speed_max: f64,
    /// And how sharply that changed, at most. This is the reversal: an eye that
    /// stops dead and sets off the other way has an unbounded one, and a
    /// filtered eye's is its speed over its time constant.
    ///
    /// Comparable between rigs on the same run, and **not** a physical quantity
    /// where the target *jumps*. A filter's answer to a step is a velocity that
    /// changes instantly in continuous time, so what is sampled is
    /// `size / (tau * dt)` — it ranks two time constants correctly and doubles
    /// if the frame rate doubles. Where the input is a jump,
    /// [`Metrics::speed_max`] is the number that means something: how fast the
    /// eye actually slid.
    pub accel_max: f64,
    /// The RMS of the third difference. What "ragged" means when it is a number
    /// rather than a feeling.
    pub jerk_rms: f64,
    /// How uneven the *drawn* eye's step was, over the frames where the body
    /// was moving: the variance, in pixels, of how far it moved each frame.
    ///
    /// The metric a continuous one cannot give. At a constant body speed an eye
    /// that moves `0, 0, 3, 0, 0, 3` and one that moves `1, 1, 1, 1, 1, 1` have
    /// the same mean velocity, and only the first is a ratchet.
    pub step_var: f64,
    /// Frames where the body moved and the drawn eye did not.
    pub still_frames: usize,
}

impl Metrics {
    /// Measure a run.
    ///
    /// Takes the samples and nothing else, so the offline bench, the DST
    /// harness and a live scope in the window all compute the same numbers from
    /// the same code — which is the only reason a number from one of them can
    /// be compared with a number from another.
    pub fn of(samples: &[Sample]) -> Self {
        let mut metrics = Self {
            frames: samples.len(),
            travel: 0.0,
            lag_max: 0.0,
            lag_rms: 0.0,
            lift_lag_max: 0.0,
            ahead_max: f64::NEG_INFINITY,
            speed_max: 0.0,
            accel_max: 0.0,
            jerk_rms: 0.0,
            step_var: 0.0,
            still_frames: 0,
        };
        let mut lag_squares = 0.0;
        let (mut steps, mut step_total, mut step_squares) = (0usize, 0.0, 0.0);

        // The derivatives, from the one place they are differenced.
        let (mut jerk_total, mut jerk_squares) = (0.0, 0u32);
        for reading in readings(samples) {
            metrics.speed_max = metrics.speed_max.max(reading.speed);
            if let Some(accel) = reading.accel {
                metrics.accel_max = metrics.accel_max.max(accel);
            }
            if let Some(jerk) = reading.jerk {
                jerk_total += jerk * jerk;
                jerk_squares += 1;
            }
        }

        for (index, sample) in samples.iter().enumerate() {
            let body = sample.gaze.exact();
            let eye = (f64::from(sample.eye.x), f64::from(sample.eye.y));
            let lag = length(minus(eye, body));
            metrics.lag_max = metrics.lag_max.max(lag);
            lag_squares += lag * lag;
            metrics.lift_lag_max = metrics
                .lift_lag_max
                .max((sample.state.lift - sample.gaze.lift).abs());

            let Some(previous) = index.checked_sub(1).map(|before| &samples[before]) else {
                continue;
            };
            let dt = (sample.at - previous.at).as_secs_f64();
            if dt <= 0.0 {
                continue;
            }

            let moved = minus(eye, (f64::from(previous.eye.x), f64::from(previous.eye.y)));
            metrics.travel += length(moved);

            // Was the body moving this frame? Everything below is about
            // following, and a still body is not being followed.
            let body_step = minus(body, previous.gaze.exact());
            let body_speed = length(body_step) / dt;
            if body_speed > 1.0 {
                let heading = (body_step.0 / length(body_step), body_step.1 / length(body_step));
                let ahead = minus(eye, body).0 * heading.0 + minus(eye, body).1 * heading.1;
                metrics.ahead_max = metrics.ahead_max.max(ahead);
                let step = length(moved);
                steps += 1;
                step_total += step;
                step_squares += step * step;
                if step == 0.0 {
                    metrics.still_frames += 1;
                }
            }
        }

        if metrics.frames > 0 {
            metrics.lag_rms = (lag_squares / metrics.frames as f64).sqrt();
        }
        if jerk_squares > 0 {
            metrics.jerk_rms = (jerk_total / f64::from(jerk_squares)).sqrt();
        }
        if steps > 0 {
            let mean = step_total / steps as f64;
            metrics.step_var = step_squares / steps as f64 - mean * mean;
        }
        if metrics.ahead_max == f64::NEG_INFINITY {
            // Nothing walked, so there is no direction to be ahead along. Zero
            // would read as "it kept up perfectly", which is a different claim.
            metrics.ahead_max = f64::NAN;
        }
        metrics
    }
}

fn minus(a: Pixels, b: Pixels) -> Pixels {
    (a.0 - b.0, a.1 - b.1)
}

fn scaled(a: Pixels, by: f64) -> Pixels {
    (a.0 * by, a.1 * by)
}

fn length(a: Pixels) -> f64 {
    a.0.hypot(a.1)
}

/// Somewhere in the middle of a facet, far enough out that the arithmetic is
/// working with the numbers a real position has.
const START: Point = Point::new(1495, 1629, 0);

/// The scenarios, and what each is for.
///
/// Named after the complaint they represent, because that is how a camera's
/// failures arrive: not as "the jerk metric is high" but as "it snaps when I
/// turn round".
pub fn scripts() -> Vec<Script> {
    vec![
        // A flat line. Any motion at all is shimmer, and it is the cheapest
        // possible test of the quantiser.
        Script::new("stand_still", START).stand(Duration::from_millis(2_000)),
        // The baseline walk: how far the eye trails, and whether it gets there
        // at a constant speed.
        ten_steps("ten_east", Direction::East),
        // The same going up: a rig with its own clock for the height has to
        // show a difference here and nowhere else.
        (0..10)
            .fold(Script::new("stairs", START), |script, _| {
                script.step_up(Direction::East, 5, WALK_HOLD)
            })
            .stand(Duration::from_millis(400)),
        // The kite. A reversal every step, which is the phase the eye is worst
        // at and the reason the spring exists.
        (0..12)
            .fold(Script::new("back_and_forth", START), |script, index| {
                let direction = match index % 2 {
                    0 => Direction::East,
                    _ => Direction::West,
                };
                script.step(direction, WALK_HOLD)
            })
            .stand(Duration::from_millis(400)),
        // A correction: three steps east and the shard says the third never
        // happened. Not a cut — the body did cross that ground — so a rig is
        // expected to absorb it rather than relay it.
        Script::new("rollback", START)
            .step(Direction::East, WALK_HOLD)
            .step(Direction::East, WALK_HOLD)
            .step(Direction::East, WALK_HOLD)
            .jump(Point::new(START.x + 2, START.y, 0))
            .stand(Duration::from_millis(200))
            .step(Direction::East, WALK_HOLD)
            .stand(Duration::from_millis(400)),
        // A floor giving way: the same tile, twenty units down, between two
        // frames. What tells a lift filter from a cut.
        Script::new("dungeon", START)
            .step(Direction::East, WALK_HOLD)
            .jump(Point::new(START.x + 1, START.y, -20))
            .stand(Duration::from_millis(600))
            .step(Direction::East, WALK_HOLD),
        // And the case that keeps the cut honest: a correction the size of a
        // kerb, arriving whole, exactly as a floor change does. A rule that
        // fires on everything instantaneous would cut this too, and a `0x22`
        // revising the ground by a unit is the one thing worth easing.
        Script::new("kerb", START)
            .step(Direction::East, WALK_HOLD)
            .jump(Point::new(START.x + 1, START.y, 2))
            .stand(Duration::from_millis(400))
            .jump(Point::new(START.x + 1, START.y, 0))
            .stand(Duration::from_millis(400)),
        // The largest height a filter is ever asked to absorb: fifteen units,
        // one short of the body that is the cut. Anything a rig does badly here
        // it does badly on the worst case it is allowed to see, which is what
        // picks a time constant — the small ones are easy for every setting.
        Script::new("ledge", START)
            .step(Direction::East, WALK_HOLD)
            .jump(Point::new(START.x + 1, START.y, -15))
            .stand(Duration::from_millis(800)),
        // A recall. Nothing between here and there is worth watching, and a rig
        // that eases across it draws a smear of world nobody is looking at.
        Script::new("teleport", START)
            .stand(Duration::from_millis(400))
            .jump(Point::new(START.x + 60, START.y + 40, 0))
            .stand(Duration::from_millis(1_000)),
    ]
}

fn ten_steps(name: &'static str, direction: Direction) -> Script {
    (0..10)
        .fold(Script::new(name, START), |script, _| {
            script.step(direction, WALK_HOLD)
        })
        .stand(Duration::from_millis(400))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Cadence = Cadence::steady(Duration::from_millis(16));

    /// The script is the oracle everything else is measured against, so it is
    /// pinned first: a step is one tile over one hold at a constant speed, and
    /// it is where it should be at the quarter, the half and the end.
    #[test]
    fn a_scripted_step_crosses_one_tile_over_one_hold() {
        let script = Script::new("one", Point::new(100, 100, 0)).step(Direction::East, WALK_HOLD);
        assert_eq!(script.length, WALK_HOLD);
        assert_eq!(script.gaze_at(Duration::ZERO), Gaze::on(Point::new(100, 100, 0)));
        assert_eq!(script.gaze_at(WALK_HOLD), Gaze::on(Point::new(101, 100, 0)));
        // Half a tile east is eleven pixels right of where it started.
        let half = script.gaze_at(Duration::from_millis(200));
        assert!((half.x - 11.0).abs() < 1e-9, "{}", half.x);
        // And past the end it stands on the tile it arrived at, rather than
        // walking on for ever.
        assert_eq!(
            script.gaze_at(Duration::from_secs(9)),
            Gaze::on(Point::new(101, 100, 0))
        );
    }

    /// A jump takes no time and is not a very fast walk: at the instant it
    /// happens the body is already there, and the arithmetic that would divide
    /// by its duration is never reached.
    #[test]
    fn a_jump_is_not_a_division_by_zero() {
        let script = Script::new("hop", Point::new(100, 100, 0))
            .stand(Duration::from_millis(100))
            .jump(Point::new(160, 100, 0))
            .stand(Duration::from_millis(100));
        let before = script.gaze_at(Duration::from_millis(99));
        let after = script.gaze_at(Duration::from_millis(100));
        assert_eq!(before, Gaze::on(Point::new(100, 100, 0)));
        assert_eq!(after, Gaze::on(Point::new(160, 100, 0)));
        assert!(after.x.is_finite() && after.y.is_finite());
    }

    /// Every cadence covers the same span of time, whatever it does to the
    /// frames in between. Without this the frame-rate property compares two
    /// runs of different lengths and reports the difference as a defect.
    #[test]
    fn every_cadence_ends_on_the_scripts_last_instant() {
        let script = ten_steps("ten", Direction::East);
        for millis in [1, 7, 16, 33, 250] {
            let trace = run(Rig::HARD, &script, Cadence::steady(Duration::from_millis(millis)));
            let last = trace.samples.last().unwrap();
            assert_eq!(last.at, script.length, "at {millis}ms a frame");
        }
        let jittered = run(
            Rig::HARD,
            &script,
            Cadence::jittered(Duration::from_millis(16), Duration::from_millis(9), 4),
        );
        assert_eq!(jittered.samples.last().unwrap().at, script.length);
    }

    /// The metrics over a body that never moved: everything is zero, and the
    /// one number that must *not* be zero is the one saying so.
    ///
    /// This is the shape of every false green a metric can produce — a still
    /// scene is perfectly smooth — so the companion is asserted here rather
    /// than trusted at the call sites.
    #[test]
    fn a_still_body_travels_nowhere_and_says_so() {
        let script = Script::new("still", START).stand(Duration::from_millis(500));
        let metrics = Metrics::of(&run(Rig::HARD, &script, FRAME).samples);
        assert!(metrics.frames > 10, "{} frames", metrics.frames);
        assert_eq!(metrics.travel, 0.0);
        assert_eq!(metrics.speed_max, 0.0);
        assert!(metrics.ahead_max.is_nan(), "nothing walked, so there is no ahead");
    }

    /// The table's peaks and the chart's curve are one arithmetic.
    ///
    /// The failure this guards is the one that makes a bench useless rather than
    /// wrong: a number that disagrees with the picture printed beside it leaves
    /// nobody able to say which of the two is lying.
    #[test]
    fn the_metrics_are_the_peaks_of_the_readings() {
        let trace = run(Rig::LIFT, &ten_steps("ten", Direction::East), FRAME);
        let metrics = Metrics::of(&trace.samples);
        let readings = readings(&trace.samples);
        assert!(readings.len() > 20, "{} readings", readings.len());
        let peak = |of: fn(&Reading) -> Option<f64>| readings.iter().filter_map(of).fold(0.0f64, f64::max);
        assert_eq!(metrics.speed_max, peak(|r| Some(r.speed)));
        assert_eq!(metrics.accel_max, peak(|r| r.accel));
        assert_eq!(metrics.lag_max, peak(|r| Some(r.lag)));
        // And the first frames are the ones with nothing behind them, rather
        // than a zero that would read as "it did not accelerate".
        assert_eq!(readings[0].accel, None);
        assert_eq!(readings[1].jerk, None);
        assert!(readings[2].jerk.is_some());
    }

    /// A scope keeps its span and drops what fell out of it, and what it keeps
    /// measures exactly as the same frames measure offline.
    #[test]
    fn a_scope_holds_the_last_few_seconds_and_nothing_older() {
        let script = ten_steps("ten", Direction::East);
        let mut scope = Scope::new(Duration::from_millis(500));
        let mut follower = Follower::new(Rig::HARD);
        let mut now = Duration::ZERO;
        let step = Duration::from_millis(16);
        // The first frame has no elapsed time behind it, as a first frame does.
        let mut dt = Duration::ZERO;
        // Stopped mid-walk rather than at the script's end: the last half second
        // of `ten_east` is the body standing, and a window over that would be
        // green with a travel of nothing — the false green this repository has
        // produced before.
        while now < Duration::from_millis(3_000) {
            let gaze = script.gaze_at(now);
            // Rounded to a whole virtual pixel, which is the bench's own quantum:
            // it flies at 1:1, where that is exactly the display's. A bench at a
            // magnification would want `Camera::snap` and a zoom to hand it — see
            // the backlog in `docs/camera.md`.
            let eye = follower.advance(gaze, dt).pixel();
            scope.record(dt, gaze, eye, follower.exact().unwrap());
            now += step;
            dt = step;
        }
        let held = scope.samples();
        assert!(held.len() > 20, "{} frames", held.len());
        let span = held.last().unwrap().at - held.first().unwrap().at;
        assert!(span <= scope.span(), "{span:?} of a 500ms window");
        assert!(span > Duration::from_millis(450), "{span:?}, and not a stub");
        // The eye walked while the window slid, which is what says the trace is
        // a live one rather than the first half-second kept for ever.
        assert!(scope.at() > Duration::from_secs(2), "{:?}", scope.at());
        assert!(Metrics::of(held).travel > 20.0);

        scope.clear();
        assert!(scope.samples().is_empty());
        assert!(scope.at() > Duration::from_secs(2), "the clock is not the trace");
    }

    /// And over a body that walked: the eye's speed is the body's, because the
    /// reference rig is the body.
    ///
    /// A tile is 44 pixels across the diagonal and a step east covers half of
    /// one on each axis, so 400ms of walking east moves the eye
    /// `sqrt(22² + 22²)` — 31 pixels — which is 78 a second.
    #[test]
    fn the_reference_rigs_eye_moves_at_the_bodys_own_speed() {
        let metrics = Metrics::of(&run(Rig::HARD, &ten_steps("ten", Direction::East), FRAME).samples);
        assert!((metrics.speed_max - 77.8).abs() < 0.5, "{}", metrics.speed_max);
        assert!(metrics.travel > 300.0, "{}", metrics.travel);
        // It is the body, so it never trails by more than the rounding.
        assert!(metrics.lag_max < 0.71, "{}", metrics.lag_max);
    }
}
