//! Where the eye goes, given where the body is.
//!
//! One pipeline, and every camera this client will have is a [`Rig`] — a value
//! of parameters rather than an implementation of its own. The argument is in
//! `docs/camera.md`; the short of it is that two cameras written as two
//! implementations are two quantisers, two cut rules and two rounding habits,
//! and a bench comparing them compares those as much as it compares the feel.
//! [`Rig::HARD`] is the reference camera — the eye is the body, to the pixel,
//! every frame — expressed as every time constant at zero, and if this pipeline
//! could not say that as a degenerate parameter set the pipeline would be wrong.
//!
//! # The ground and the height are two signals
//!
//! [`crate::camera::project`] folds `z` into the vertical axis: a tile one unit
//! higher is four pixels further up the same screen column. So a camera handed a
//! projected pixel cannot smooth height at all — filter that value and the walk
//! is filtered with it, leave it and every stair is a four-pixel jump of the
//! whole world. [`Gaze`] therefore keeps the ground position and the lift apart
//! and folds them back together once, at the end, in [`Gaze::eye`].
//!
//! # The order, including the stages that are empty
//!
//! 1. **Anchor** — what is being looked at. The caller's: it is the one that
//!    knows what a player is, and it hands over a [`Gaze`].
//! 2. **Intent** — additive offsets that say where the player wants to look
//!    rather than where they are: velocity look-ahead, cursor lean. *Empty.*
//! 3. **Zone** — the dead zone and the idle recentre. *Empty.*
//! 4. **Filter** — per-channel damping. Live, and identity under [`Rig::HARD`].
//! 5. **Cut** — the discontinuities the filter must not be dragged across, which
//!    reset its state rather than being eased over. *Empty apart from
//!    [`Follower::cut`]*, which is the one a caller raises by hand.
//! 6. **Clamp** — map edges, camera volumes. *Empty*, and it stays empty while
//!    the anchor is a body, which is on the map by construction.
//! 7. **Impulse** — shake, recoil, a hit's kick: additive, on the pose, never on
//!    the filter's state. *Empty.*
//! 8. **Quantise** — round to the pixel the *display* has and keep the remainder
//!    in the state. Not to a whole pixel of the art, which above 1:1 is several
//!    of the display's: see `docs/camera.md` D11. It is the one stage that does
//!    not happen here, because it is the one that needs the magnification —
//!    [`crate::camera::Camera::snap`] is where it lands.
//!
//! The empty ones are listed because order is where camera defects live: a clamp
//! above the filter springs off its boundary, an impulse mixed into the filter's
//! state drifts and never comes back, and a quantiser in the middle turns a
//! filter into a ratchet — the eye sits still until the accumulated error
//! crosses a pixel and then moves one. Fixing the whole order now means a later
//! milestone fills a stage in without anything else moving.

use std::time::Duration;

use openshard_protocol::world::Point;

use crate::camera::{WorldPoint, Z_STEP, project};

/// Where the eye is asked to look, before anything smooths it.
///
/// Three channels and not a point, because they are filtered on different
/// clocks — see this module's note on the fold. Sub-pixel, because this is a
/// filter's input rather than a sprite's placement: quantising it here is the
/// staircase that makes a smoother smooth nothing at low speed.
///
/// `f64` and not `f32`, and it is the rounding that decides it rather than the
/// filter. The far corner of a 7,168-tile facet is 157,000 world pixels out,
/// where an `f32` resolves to about a hundredth of a pixel — fine for a
/// smoother, and a hundred times the margin at which two roundings of the same
/// position disagree. The eye has to land on the pixel the sprite landed on.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Gaze {
    /// Where the body stands on the ground, in world pixels read at `z = 0`.
    /// Rightwards.
    pub x: f64,
    /// Downwards.
    pub y: f64,
    /// What its height lifts it by, in pixels — `z * Z_STEP`, positive upwards,
    /// kept apart so it can have its own clock.
    pub lift: f64,
}

impl Gaze {
    /// A body standing on a tile.
    pub fn on(point: Point) -> Self {
        // Read at `z = 0` so the lift is not in here twice.
        let plane = project(Point::new(point.x, point.y, 0));
        Self {
            x: f64::from(plane.x),
            y: f64::from(plane.y),
            lift: f64::from(point.z) * f64::from(Z_STEP),
        }
    }

    /// This gaze moved back towards `from` by the part of a step not yet walked.
    ///
    /// Backwards from the destination rather than forwards from the origin,
    /// which is the form that lands *exactly* on the destination when the step
    /// is over: a lerp forwards leaves a rounding pixel behind at the end of
    /// every step, and a body that never quite arrives shimmers.
    pub fn back_towards(self, from: Self, left: f64) -> Self {
        Self {
            x: self.x - (self.x - from.x) * left,
            y: self.y - (self.y - from.y) * left,
            lift: self.lift - (self.lift - from.lift) * left,
        }
    }

    /// This gaze, one frame's worth closer to `target`, on every channel.
    ///
    /// The body's ease (`docs/camera.md` D10) is this, and the eye's filter is
    /// the same [`approach`] per channel — what the eye does that this does not
    /// is decide a *cut* on the height first, which is a rule about where the
    /// target came from and not about damping. So the two are one arithmetic and
    /// two policies, rather than two arithmetics.
    ///
    /// Here rather than beside [`Rig`] because the caller is not the camera: an
    /// eased *body* is `client/app`'s business and it holds the state, where a
    /// [`Rig`] is the parameter set of the eye. The arithmetic is shared; the
    /// two subjects are not.
    ///
    /// All three channels on one time constant, because a body is one object: a
    /// sprite whose feet and height eased at different rates would stretch on
    /// every stair.
    pub fn eased_towards(self, target: Self, tau: f32, dt: Duration) -> Self {
        Self {
            x: approach(self.x, target.x, tau, dt),
            y: approach(self.y, target.y, tau, dt),
            lift: approach(self.lift, target.lift, tau, dt),
        }
    }

    /// Where this lands in world pixel space, height folded back in, to a
    /// fraction of a pixel.
    pub fn exact(self) -> (f64, f64) {
        (self.x, self.y - self.lift)
    }

    /// The one point in the world this asks to be looked at.
    ///
    /// **No longer the quantiser**, and that is `docs/camera.md` D11: rounding
    /// here rounds to a *virtual* pixel, which at `3x` is three pixels of the
    /// display, and a camera that could only name one in three of the positions
    /// a screen can show moves the world in jumps coarser than the screen. The
    /// rounding that is left happens once, in [`crate::camera::Camera::snap`],
    /// against the pixel a display actually has — and it is the camera that does
    /// it because the camera is the thing that knows the magnification.
    ///
    /// What survives of D7 is the placement: the quantiser is still last, still
    /// after the filter, and the fraction still stays in whatever held this
    /// value. Only the size of its step changed.
    pub fn eye(self) -> WorldPoint {
        let (x, y) = self.exact();
        WorldPoint { x, y }
    }
}

/// Every number the eye's motion is made of. A camera is one of these.
///
/// `Copy + PartialEq` and kept apart from the state, for two reasons that are
/// both about the bench: a preset can be swapped while the client is running
/// without the world jumping, and a slider edit is a value that can be printed,
/// pasted into the source and committed as the preset it turned out to be.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rig {
    /// Seconds for the ground plane to close all but `1/e` of its gap.
    ///
    /// Zero is no filter at all. One knob for two channels today — the filter
    /// keeps `x` and `y` apart regardless, so splitting the knob later moves no
    /// state.
    pub plane_tau: f32,
    /// The same for the height, and its own because it is not the same
    /// question: a stair wants to be smoothed away and a walk does not.
    pub lift_tau: f32,
    /// How much the height may change between two frames before it is a cut
    /// rather than a climb, in pixels.
    ///
    /// A body's own height, by default — see [`FLOOR`]. Above it the ground did
    /// not change because somebody walked up it, and easing across it would draw
    /// the eye floating through a storey it was never on.
    ///
    /// In pixels and not in a fraction of the screen, which is the one place
    /// D3's rule does not apply: whether a change of height is a stair or a
    /// floor is a fact about the world, and it does not become a different fact
    /// because somebody zoomed in.
    ///
    /// [`f32::INFINITY`] never cuts. [`Rig::HARD`]'s value is arbitrary, since
    /// nothing is ever eased for it to interrupt.
    pub lift_cut: f32,
}

/// The height a change has to exceed, in one frame, to be a change of floor.
///
/// A human is `PLAYER_HEIGHT` — 16 units, Sphere's own constant — so this is
/// what a body is tall, in pixels of lift. The argument is not about comfort: a
/// walked change of height arrives through the glide, spread across the whole
/// step, so even a cliff delivers a few pixels per frame. Everything that
/// arrives *whole* in one frame is a floor changing, a teleporter, or a
/// correction — and the ones worth easing across are the small ones.
///
/// Frame-to-frame and not per second, deliberately. A loop that stalled for
/// half a second and then drew has already teleported the body on screen, and
/// cutting is the honest answer to that too.
pub const FLOOR: f32 = 16.0 * Z_STEP as f32;

impl Rig {
    /// The reference camera: the eye is the body, to the pixel, every frame.
    ///
    /// What ClassicUO does, and what this client did before there was a
    /// pipeline. It is here as the baseline every other rig is scored against,
    /// not as a default — see `docs/camera.md`, D9.
    pub const HARD: Self = Self {
        plane_tau: 0.0,
        lift_tau: 0.0,
        lift_cut: FLOOR,
    };

    /// The reference camera with a clock on the height.
    ///
    /// The ground is still the body's, to the pixel — a walk is not smoothed at
    /// all — and only `z` is filtered.
    ///
    /// **What that is for is not stairs.** A climbed step's rise arrives through
    /// the glide, spread across the whole 400ms, so there is nothing left in it
    /// to smooth; the bench says a filter makes a climbed stair marginally
    /// *worse*, because the rise was cancelling part of the walk's own motion
    /// down the screen and delaying half of a cancellation is a transient where
    /// there was none. What this rig absorbs is height that did **not** come
    /// from walking: a `0x22` revising the ground under a standing body, a
    /// surface a hair different from the one that was predicted. The reference
    /// camera relays every one of those whole, and there are a lot of them.
    ///
    /// `0.15` is chosen by what it costs rather than by what it buys: a stair
    /// rises 5 units over a step, which is 50 pixels a second, so the eye
    /// settles 7.5 pixels below the body — a third of one riser. Twice that and
    /// the eye is half a storey behind while climbing, which is the lift-shaft
    /// feel; half of it and a correction is back to arriving in two frames
    /// instead of one. See `docs/camera.md`, C2.
    pub const LIFT: Self = Self {
        plane_tau: 0.0,
        lift_tau: 0.15,
        lift_cut: FLOOR,
    };
}

/// What one frame left behind for the next one.
///
/// Both halves are [`Gaze`]s because both have exactly the channels the filter
/// runs on. The second is not a duplicate of the first: a cut is a *target* that
/// moved further than a body could have, which is a question about what was
/// asked for and not about where the eye got to. Asking it of the eye instead
/// would cut whenever a slow rig fell far enough behind — a camera that smooths
/// by giving up.
#[derive(Clone, Copy, Debug)]
struct Eased {
    /// Where the eye is, to a fraction of a pixel.
    at: Gaze,
    /// What it was asked for.
    target: Gaze,
}

/// A rig, and where the eye it drives has got to.
#[derive(Clone, Copy, Debug)]
pub struct Follower {
    rig: Rig,
    /// The last frame, or nothing before the first.
    ///
    /// [`Option`] in its proper sense: before the first frame the eye is not at
    /// a stale position, it has no position, and the frame that finds it so
    /// places it rather than easing from a zero nobody chose.
    last: Option<Eased>,
}

impl Follower {
    /// A follower that has not looked anywhere yet.
    pub fn new(rig: Rig) -> Self {
        Self { rig, last: None }
    }

    /// The parameters in force.
    pub fn rig(&self) -> Rig {
        self.rig
    }

    /// Change them, keeping where the eye is.
    ///
    /// Keeping it deliberately: swapping a preset mid-flight is the whole point
    /// of the bench being a picker, and a swap that also moved the eye would
    /// make every comparison start with a jump.
    pub fn set_rig(&mut self, rig: Rig) {
        self.rig = rig;
    }

    /// Where the eye is, to a fraction of a pixel — what the quantiser rounded,
    /// and `None` before the first frame.
    ///
    /// For a bench and for a scope, and the reason it is worth exposing is that
    /// the two traces answer different questions: at one-pixel quantisation and
    /// sixty frames a second, the acceleration of the *rounded* eye is
    /// dominated by the rounding and says nothing about the rig. See
    /// [`crate::bench`].
    pub fn exact(&self) -> Option<Gaze> {
        self.last.map(|last| last.at)
    }

    /// Whether the eye still owes the screen a pixel it has not been given.
    ///
    /// What the event loop asks to decide whether a frame is worth drawing when
    /// nothing else moved: a rig that eases is still converging on frames where
    /// no packet arrived and no body glided, and a loop that only woke for
    /// gliding bodies would deliver the tail of every ease 80ms late, in one
    /// jump — which is the stutter the filter exists to remove, arriving after
    /// it.
    ///
    /// The test is the *drawn* pixel and not the remaining error, and that is
    /// exact rather than a tolerance somebody chose: each channel approaches its
    /// target monotonically, so if the eye and its target already round to the
    /// same pixel, every position between them does too — no later frame can
    /// change what the screen is given until the target moves again.
    ///
    /// `quantum` is how much of a virtual pixel one of the display's is —
    /// [`crate::camera::Camera::quantum`]. It is an argument and not a `Camera`
    /// for the reason D8 gives about [`Follower::advance`]: what this type is
    /// worth is that a bench can drive it without one. It matters here rather
    /// than being a detail: magnified, the eye owes the screen a pixel for
    /// longer, because the pixel it owes is smaller.
    pub fn settling(&self, quantum: f64) -> bool {
        let same = |a: f64, b: f64| (a / quantum).round() == (b / quantum).round();
        self.last.is_some_and(|last| {
            let (at, target) = (last.at.eye(), last.target.eye());
            !same(at.x, target.x) || !same(at.y, target.y)
        })
    }

    /// Forget where the eye was: the next [`Follower::advance`] places it on the
    /// gaze instead of easing to it.
    ///
    /// A cut, in the sense of stage 5 — a teleport, a facet change, a relock
    /// from across the map. It resets the state rather than moving it, so that
    /// nothing of the old position survives to be eased away afterwards.
    pub fn cut(&mut self) {
        self.last = None;
    }

    /// One frame: where the eye goes, given where it is asked to look.
    ///
    /// Two arguments and not a `Frame` struct, which is what `docs/camera.md`
    /// first wrote: the pipeline's input is a gaze and an elapsed time until
    /// stages 2, 3 and 5 fill, and a wrapper around two values earns nothing.
    /// What the shape does say is what is *not* an argument — no `Instant`, no
    /// `Camera`, no window, no map. That is what lets the bench run ten thousand
    /// frames in under a millisecond and the DST harness drive the same code the
    /// window does.
    pub fn advance(&mut self, gaze: Gaze, dt: Duration) -> WorldPoint {
        // Stages 2 and 3 sit here when they exist: the intent offsets are
        // summed onto `gaze`, and the zone reduces the gap before the filter
        // sees it.
        let at = match self.last {
            Some(last) => Gaze {
                // Stage 4, per channel. `x` and `y` share a time constant and
                // not a state: the isometric screen is twice as wide as it is
                // tall, so the day one of them wants a slower clock than the
                // other, the split is a field and not a rewrite.
                x: approach(last.at.x, gaze.x, self.rig.plane_tau, dt),
                y: approach(last.at.y, gaze.y, self.rig.plane_tau, dt),
                // Stage 5, on the one channel that has a rule yet: a height
                // that changed by more than a body is tall between two frames
                // did not change because somebody walked up it. The plane is
                // deliberately not cut with it — a floor giving way under a
                // body does not move it sideways, and jerking the whole world
                // sideways for a vertical event is a second defect answering
                // the first.
                lift: match (gaze.lift - last.target.lift).abs() > f64::from(self.rig.lift_cut) {
                    true => gaze.lift,
                    false => approach(last.at.lift, gaze.lift, self.rig.lift_tau, dt),
                },
            },
            // The other cut, and the one a caller raises by hand: nothing to
            // ease from.
            None => gaze,
        };
        self.last = Some(Eased { at, target: gaze });
        // Stages 6 and 7 sit here — the clamp on the filtered position, then the
        // impulse on the pose and never on what was just stored.
        at.eye()
    }
}

/// One channel, a step closer to where it is asked to be.
///
/// `alpha = 1 - exp(-dt / tau)`, which is frame-rate independent by
/// construction: the same span of time moves the eye the same distance whether
/// it arrives in one frame or ten. Never `from + (to - from) * 0.1` per frame —
/// that ties the feel to the frame rate, and this client already has two of them
/// on purpose, so the naive form would change the camera's character at the
/// exact moment somebody starts walking.
///
/// The `tau <= 0` branch is not an optimisation and not defensiveness. `dt / 0`
/// is infinity, `0 / 0` is `NaN`, and a `NaN` here does not fall over: it is
/// stored, every comparison against it downstream is false, and the camera
/// silently takes the other branch of every decision for the rest of the
/// session. The reference rig is exactly this branch.
pub fn approach(from: f64, to: f64, tau: f32, dt: Duration) -> f64 {
    if tau <= 0.0 {
        return to;
    }
    let alpha = 1.0 - (-dt.as_secs_f64() / f64::from(tau)).exp();
    from + (to - from) * alpha
}

#[cfg(test)]
mod tests {
    /// The quantum at 1:1, where a virtual pixel and a real one are the same
    /// thing. Every test in this module flies unmagnified — what a rig does is
    /// not a function of the zoom, and D8 is the reason a `Follower` can be
    /// driven without a `Camera` at all.
    const ONE_TO_ONE: f64 = 1.0;

    use super::*;

    /// A tile far enough out that `f32` would have started rounding.
    const FAR: Point = Point::new(6000, 4000, 0);

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    /// A rig that eases both channels at the same rate, for the tests that are
    /// about the filter rather than about a camera.
    fn eased(tau: f32) -> Rig {
        Rig {
            plane_tau: tau,
            lift_tau: tau,
            lift_cut: FLOOR,
        }
    }

    /// Where the eye is, for a test that is about the filter's own state.
    fn state(follower: &Follower) -> Gaze {
        follower.exact().expect("the eye has been placed")
    }

    /// The reference camera, and the property the whole of C0 is a transplant
    /// under: whatever the frame rate and wherever the body is, the eye is on it.
    #[test]
    fn the_reference_rig_is_the_body() {
        let mut follower = Follower::new(Rig::HARD);
        for (step, dt) in [(0, 0), (1, 16), (2, 33), (3, 0), (4, 400)] {
            let gaze = Gaze::on(Point::new(FAR.x + step, FAR.y, 12));
            assert_eq!(follower.advance(gaze, ms(dt)), gaze.eye());
        }
    }

    /// A zero time constant with a zero frame is the one division that would be
    /// `0 / 0`. A `NaN` here is not caught by anything downstream — it is
    /// stored, and every comparison against it is quietly false ever after.
    #[test]
    fn a_still_frame_at_no_time_constant_is_not_a_nan() {
        let mut follower = Follower::new(Rig::HARD);
        follower.advance(Gaze::on(FAR), Duration::ZERO);
        let state = state(&follower);
        assert!(state.x.is_finite() && state.y.is_finite() && state.lift.is_finite());
    }

    /// The definition of the time constant, held to its own arithmetic: one
    /// `tau` of elapsed time closes all but `1/e` of the gap.
    #[test]
    fn one_time_constant_closes_all_but_one_over_e() {
        let mut follower = Follower::new(eased(0.5));
        follower.advance(Gaze::default(), ms(16));
        let target = Gaze {
            x: 1000.0,
            y: 0.0,
            lift: 0.0,
        };
        follower.advance(target, ms(500));
        let left = 1000.0 - state(&follower).x;
        assert!(
            (left - 1000.0 / std::f64::consts::E).abs() < 0.5,
            "{left} left of 1000 after one tau",
        );
    }

    /// D5, and the reason the naive per-frame lerp is banned: the same span of
    /// time has to move the eye the same distance however many frames it
    /// arrives in. A `lerp(0.1)` fails this by a factor of three.
    #[test]
    fn the_same_span_lands_in_the_same_place_at_any_frame_rate() {
        let rig = eased(0.2);
        let target = Gaze {
            x: 900.0,
            y: -400.0,
            lift: 60.0,
        };
        let mut slow = Follower::new(rig);
        slow.advance(Gaze::default(), ms(1));
        slow.advance(target, ms(100));

        let mut fast = Follower::new(rig);
        fast.advance(Gaze::default(), ms(1));
        for _ in 0..10 {
            fast.advance(target, ms(10));
        }

        let (slow, fast) = (state(&slow), state(&fast));
        assert!(
            (slow.x - fast.x).abs() < 1e-9 && (slow.lift - fast.lift).abs() < 1e-9,
            "{slow:?} in one frame against {fast:?} in ten",
        );
    }

    /// The height has its own clock, which is the whole reason [`Gaze`] is three
    /// numbers: a rig can hold the ground exact and still smooth a stair away.
    #[test]
    fn the_height_is_filtered_apart_from_the_ground() {
        let mut follower = Follower::new(Rig {
            plane_tau: 0.0,
            lift_tau: 0.4,
            lift_cut: FLOOR,
        });
        let ground = Gaze::on(Point::new(500, 500, 0));
        follower.advance(ground, ms(16));
        // The same tile, ten units up: only the lift moved. Ten and not twenty,
        // because twenty is more than a body is tall and the cut below would
        // take it — which is that rule working, not this one failing.
        let stair = Gaze::on(Point::new(500, 500, 10));
        follower.advance(stair, ms(16));
        let at = state(&follower);
        assert_eq!((at.x, at.y), (ground.x, ground.y), "the ground did not wait");
        assert!(at.lift > 0.0 && at.lift < stair.lift, "{} of 40", at.lift);
    }

    /// A cut leaves no tail: what the eye was doing before one is not eased
    /// away afterwards, it is gone.
    #[test]
    fn a_cut_places_the_eye_rather_than_easing_to_it() {
        let mut follower = Follower::new(eased(0.3));
        follower.advance(Gaze::on(Point::new(100, 100, 0)), ms(16));
        let away = Gaze::on(Point::new(3000, 3000, 0));
        let eased = follower.advance(away, ms(16));
        assert_ne!(eased, away.eye(), "an ease is what it does without a cut");
        follower.cut();
        assert_eq!(follower.advance(away, ms(16)), away.eye());
    }

    /// Swapping a preset must not move the eye, or every comparison the bench
    /// is for would start with a jump.
    #[test]
    fn changing_the_rig_keeps_where_the_eye_is() {
        let mut follower = Follower::new(eased(0.3));
        follower.advance(Gaze::on(Point::new(100, 100, 0)), ms(16));
        follower.advance(Gaze::on(Point::new(140, 100, 0)), ms(16));
        let mid = state(&follower);
        follower.set_rig(Rig::HARD);
        assert_eq!(state(&follower), mid);
        assert_eq!(follower.rig(), Rig::HARD);
    }

    /// The lift cut, and the pair of cases it exists to tell apart.
    ///
    /// A stair arrives through the glide, spread over a whole step, so the
    /// filter sees a few pixels a frame and smooths it. A floor changing arrives
    /// whole, in one frame, and easing across it would float the eye through a
    /// storey the body was never on.
    #[test]
    fn a_stair_is_eased_and_a_floor_is_cut() {
        let ground = Point::new(500, 500, 0);
        let mut follower = Follower::new(Rig::LIFT);
        follower.advance(Gaze::on(ground), ms(16));

        // Five units up, delivered the way a glide delivers it: a fraction of
        // the rise per frame. The eye is behind, and it is behind by less than
        // the rise.
        let stair = Gaze::on(Point::new(500, 500, 5));
        for step in 1..=25 {
            let part = f64::from(step) / 25.0;
            follower.advance(
                Gaze {
                    lift: stair.lift * part,
                    ..stair
                },
                ms(16),
            );
        }
        let lag = stair.lift - state(&follower).lift;
        assert!(lag > 0.1 && lag < stair.lift, "{lag} of a 20 pixel rise");

        // And the same total change delivered whole: cut, on the frame it
        // arrives, with nothing left to ease away afterwards.
        let mut follower = Follower::new(Rig::LIFT);
        follower.advance(Gaze::on(ground), ms(16));
        let floor = Gaze::on(Point::new(500, 500, -20));
        assert_eq!(follower.advance(floor, ms(16)), floor.eye());
        assert_eq!(state(&follower).lift, floor.lift, "a cut leaves no tail");
    }

    /// And it cuts the height alone.
    ///
    /// A floor giving way under a body does not move it sideways. Cutting the
    /// plane with it would jerk the whole world across for a vertical event,
    /// which is a second defect answering the first.
    #[test]
    fn a_lift_cut_does_not_move_the_ground() {
        let mut follower = Follower::new(Rig {
            plane_tau: 0.2,
            lift_tau: 0.2,
            lift_cut: FLOOR,
        });
        follower.advance(Gaze::on(Point::new(500, 500, 0)), ms(16));
        // A trapdoor: a tile east and twenty units down, in one frame.
        let below = Gaze::on(Point::new(501, 500, -20));
        follower.advance(below, ms(16));
        let at = state(&follower);
        assert_eq!(at.lift, below.lift, "the height was cut");
        assert!(
            (at.x - below.x).abs() > 1.0,
            "and the ground was not: {} against {}",
            at.x,
            below.x,
        );
    }

    /// A correction the size of a kerb is absorbed rather than cut.
    ///
    /// The failure this guards is a cut rule that fires on everything
    /// instantaneous: a `0x22` revising the ground by a unit or two arrives in
    /// one frame like a trapdoor does, and it is the one case worth easing.
    #[test]
    fn a_small_correction_is_eased_rather_than_cut() {
        let mut follower = Follower::new(Rig::LIFT);
        follower.advance(Gaze::on(Point::new(500, 500, 0)), ms(16));
        let kerb = Gaze::on(Point::new(500, 500, 2));
        follower.advance(kerb, ms(16));
        let at = state(&follower);
        assert!(at.lift > 0.0 && at.lift < kerb.lift, "{} of 8 pixels", at.lift);
    }

    /// A settling eye asks for frames until it has nothing left to give the
    /// screen, and the reference rig never asks for one.
    ///
    /// Both halves matter: without the first the tail of every ease arrives late
    /// and whole, and a rig that answered "still settling" for ever would hold
    /// the loop at the glide rate while a client sits at a bank doing nothing.
    #[test]
    fn a_settling_eye_asks_for_frames_and_an_arrived_one_does_not() {
        let mut hard = Follower::new(Rig::HARD);
        hard.advance(Gaze::on(Point::new(500, 500, 0)), ms(16));
        hard.advance(Gaze::on(Point::new(501, 500, 0)), ms(16));
        assert!(!hard.settling(ONE_TO_ONE), "the eye is the body, every frame");

        let mut eased = Follower::new(eased(0.2));
        let target = Gaze::on(Point::new(500, 500, 0));
        eased.advance(target, ms(16));
        eased.advance(Gaze::on(Point::new(506, 500, 0)), ms(16));
        assert!(eased.settling(ONE_TO_ONE), "six tiles away and one frame in");
        // Left alone on the same target, it arrives and stops asking — and it
        // does so in a bounded number of frames rather than approaching for ever.
        let arrived = (0..200).any(|_| {
            eased.advance(Gaze::on(Point::new(506, 500, 0)), ms(16));
            !eased.settling(ONE_TO_ONE)
        });
        assert!(arrived, "an ease that never ends holds the loop at 60Hz");
    }

    /// The threshold is a body's height, which is Sphere's own constant and not
    /// a number somebody liked. Pinned next to it, because a value taken from a
    /// reference is worth nothing once it has quietly drifted.
    #[test]
    fn the_floor_is_a_body_tall() {
        assert_eq!(FLOOR, 64.0, "PLAYER_HEIGHT of 16, four pixels a unit");
    }

    /// `Gaze::on` is `project`, taken apart and put back together — held against
    /// `project` itself, which is pinned by its own tests and written from the
    /// client's numbers rather than from this file's.
    #[test]
    fn a_gaze_folds_back_into_its_projection() {
        for z in [i8::MIN, -40, -1, 0, 1, 44, i8::MAX] {
            for (x, y) in [(0, 0), (1, 0), (0, 1), (1495, 1629), (6143, 4095)] {
                let point = Point::new(x, y, z);
                assert_eq!(Gaze::on(point).eye().pixel(), project(point), "{point}");
            }
        }
    }

    /// And the step between two tiles ends exactly on the destination, at every
    /// height either side of it — the property `back_towards` is written the way
    /// it is for.
    #[test]
    fn a_step_ends_exactly_on_the_tile_it_was_going_to() {
        let from = Gaze::on(Point::new(1000, 1000, 0));
        let to = Point::new(1001, 1000, 20);
        assert_eq!(Gaze::on(to).back_towards(from, 0.0).eye().pixel(), project(to));
        assert_eq!(
            Gaze::on(to).back_towards(from, 1.0).eye().pixel(),
            project(Point::new(1000, 1000, 0)),
        );
    }
}
