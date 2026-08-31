//! How often a mobile is allowed to take a step.

use std::time::{
    Duration,
    Instant,
};

/// The shortest gap between two walking steps, on foot.
///
/// Sphere's `CClient::Event_Walking`.
pub const WALK_INTERVAL: Duration = Duration::from_millis(200);

/// The shortest gap between two steps when running on foot, or walking a mount.
///
/// Sphere uses this for a mount, for hovering, and for `speedMode == 1`; it is also
/// half ServUO's `RunFoot`/`WalkMount`, which are both 200ms. See
/// [`MOUNTED_RUN_INTERVAL`] for why half.
pub const RUN_INTERVAL: Duration = Duration::from_millis(100);

/// The shortest gap between two steps when running a mount — the fastest a mobile
/// legitimately moves.
///
/// # Why the references look like they disagree, and do not
///
/// ServUO names four rates (`Mobile.WalkFoot` 400, `RunFoot` 200, `WalkMount` 200,
/// `RunMount` 100) and they are the *real* step gaps a client uses. Sphere names one
/// walking interval, 200ms, which is half ServUO's foot walk — so read as a step rate
/// the two flatly contradict each other.
///
/// They are not the same quantity. Sphere's number is a **floor** in an anti-speedhack
/// check, and it is deliberately half the real rate so that jitter, batching and a bad
/// connection never trip it. That is the whole argument of this module: the check has
/// to be lenient or it punishes the wrong players.
///
/// So the floors here are ServUO's four rates halved: 200 on foot, 100 running on foot
/// or walking a mount, 50 running a mount. Before this a mounted mobile was charged the
/// on-foot rate, so a mounted runner — legitimately twice as fast as anything the
/// budget knew about — spent credit twice as fast as it earned it and rubber-banded on
/// a long gallop.
pub const MOUNTED_RUN_INTERVAL: Duration = Duration::from_millis(50);

/// How long one step actually takes, mounted and running — the real gap
/// [`MOUNTED_RUN_INTERVAL`] floors, the way [`RUN_HOLD`] is to [`RUN_INTERVAL`].
///
/// ServUO's `RunMount`, 100ms, and exactly half [`RUN_HOLD`]: a gallop crosses
/// a tile in the time an on-foot run crosses half of one. A walking mount
/// needs no constant of its own — ServUO's `WalkMount` equals its `RunFoot`,
/// so [`RUN_HOLD`] already is a mounted walk's real gap, the same way
/// [`WalkPace::allow`] charges the two the same [`RUN_INTERVAL`] floor.
pub const MOUNTED_RUN_HOLD: Duration = Duration::from_millis(2 * MOUNTED_RUN_INTERVAL.as_millis() as u64);

/// How long one step actually takes, on foot, at a walk.
///
/// [`WALK_INTERVAL`] twice, and derived from it rather than written out: the
/// interval above is a *floor* — how often the server will allow a step — and
/// twice it is the real pace, 400ms, which this module's own test pins against
/// ServUO's `WalkFoot`.
///
/// Here rather than in either end that needs it, because it is a rule about
/// movement and both ends read it: the client glides a body over it and holds
/// its walking animation for it, and the camera bench walks a scripted body at
/// it. Written down twice, the two copies drift and the bench is then tuned on
/// a walk nobody does.
pub const WALK_HOLD: Duration = Duration::from_millis(2 * WALK_INTERVAL.as_millis() as u64);

/// The same, for a body the wire says is running.
///
/// [`RUN_INTERVAL`] doubled for the reason [`WALK_HOLD`] doubles its own, and
/// ServUO's `RunFoot` is this. It has to be the real rate and not the floor
/// because it is also how long a glide takes: held for twice the step, a runner
/// would be a whole tile behind itself and would jump forward half a tile every
/// time the next step arrived.
pub const RUN_HOLD: Duration = Duration::from_millis(2 * RUN_INTERVAL.as_millis() as u64);

/// The nominal duration of one locally commanded step.
///
/// This is the shared timing primitive for the app's movement core and its
/// deterministic walk oracle.  Server acknowledgement latency is deliberately
/// absent: the client starts a predicted step now, and an answer only confirms
/// it or corrects it later.
///
/// `mounted` picks among the four real rates [`MOUNTED_RUN_HOLD`] documents:
/// a saddle halves the ordinary hold at a run, and at a walk it already
/// matches the on-foot run — so a mounted walk reuses [`RUN_HOLD`] rather
/// than naming a fifth constant equal to a fourth.
pub const fn step_hold(running: bool, mounted: bool) -> Duration {
    match (mounted, running) {
        (true, true) => MOUNTED_RUN_HOLD,
        (true, false) | (false, true) => RUN_HOLD,
        (false, false) => WALK_HOLD,
    }
}

/// How long a crossing should take, given how long is left before it is *due to
/// end*.
///
/// # Why a crossing is scheduled by its end and not by its start
///
/// A glide has to end exactly when the next one begins, or the walk is not
/// continuous. Both ways of being wrong read as a stutter once a tile: finish
/// early and the body stands on its tile until the next step is asked for,
/// finish late and that step yanks it forward from wherever it had got to.
///
/// A step is asked for when the event loop wakes, and a loop wakes on the
/// display's grid and never early. So a step due at `t` leaves at `t + w` for
/// some lateness `w` under one frame, and a crossing drawn for the nominal hold
/// *from there* ends at `t + w + nominal` while the next begins at
/// `t + nominal + w'`. The body therefore stands still for `w' - w` — the
/// difference of two latenesses, which is positive about half the time.
///
/// One frame of lateness is 4% of a walk, 8% of a run and **17% of a gallop**,
/// which is why a mount is what a person notices. Spending the lateness on the
/// crossing's *length* instead of banking it as standing still costs the same
/// few per cent in speed, where nobody can see it.
///
/// # The band
///
/// Believed only within half and double the nominal length, because outside it
/// the number is not a pace at all: much less is a body that had stopped and
/// started again, much more is a crossing whose schedule was lost. Both are
/// answered with the nominal hold, which is at least a walking speed.
///
/// Read by both ends of the client — `crowd.rs` for a body it only hears about,
/// and the app's own movement core for the one it commands — so the two cannot
/// drift into two different bands.
pub fn crossing_left(left: Duration, nominal: Duration) -> Duration {
    match left >= nominal / 2 && left <= nominal * 2 {
        true => left,
        false => nominal,
    }
}

/// How far a nominal step has advanced, clamped at its destination.
///
/// One shared interpolation fraction keeps the production movement core and
/// the DST oracle on the same constant-velocity rule. A zero duration is not a
/// legal player pace, but treating it as complete keeps diagnostic callers from
/// producing a NaN if fed malformed timing data.
pub fn step_progress(elapsed: Duration, takes: Duration) -> f64 {
    if takes.is_zero() {
        return 1.0;
    }
    (elapsed.as_secs_f64() / takes.as_secs_f64()).min(1.0)
}

/// How many steps of credit a mobile may bank.
///
/// The burst a client may spend at once after standing still. It has to be
/// generous: a real client sends several steps together when a stall clears, and
/// those are steps the player already took.
pub const WALK_BUFFER: u32 = 15;

/// Whether a step was allowed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pace {
    /// The step may proceed.
    Allowed,
    /// The mobile is moving faster than a mobile can move.
    TooFast,
}

impl Pace {
    /// Whether the step may proceed.
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// Decides whether a mobile is walking or teleporting.
///
/// # Why this is a budget and not a timer
///
/// The obvious design is a gate: refuse any step less than 200ms after the last.
/// It is wrong in a way that only shows up on other people's networks.
///
/// A client does not send one step per 200ms. It sends four in a burst when a
/// stall clears, then nothing for a second. Ping varies. The server's own read
/// loop batches. A hard gate refuses the burst — honest movement the player
/// already made — and the client rubber-bands. The players it punishes hardest
/// are the ones with the worst connections.
///
/// So this is a token bucket. Time earns credit; each step spends it. A burst is
/// fine while there is credit banked, and only sustained impossible speed empties
/// it. That is a client that is lying.
///
/// # Where the numbers come from, and where they do not
///
/// The intervals are Sphere's: 200ms on foot, 100ms running or mounted. Those
/// are two decades of tuning against real clients and worth taking.
///
/// The *arithmetic* is not Sphere's, deliberately. Its `Event_Walking` keeps a
/// running average in milliseconds and then clamps it against `WALKBUFFER`,
/// which defaults to `15` — comparing a duration against what its own
/// documentation calls a count of "points". Read literally, a normal walker sits
/// at a balance of 15ms and a single early step puts it at `15 - 200 = -185`,
/// refused instantly, with none of the burst tolerance the buffer is there to
/// provide. Either the constant means something undocumented or the check does
/// not do what it says.
///
/// A token bucket is the same intent, stated plainly: a bucket that holds
/// [`WALK_BUFFER`] steps, refilled by elapsed time. Copying arithmetic that does
/// not add up would be worse than not copying it.
///
/// # The clock is a parameter
///
/// Like `AuthKeys`. Testing a rate limiter with `sleep` is slow, flaky, and
/// cannot express "and then a minute passed".
///
/// ```
/// use std::time::{Duration, Instant};
/// use openshard_movement::WalkPace;
///
/// let mut pace = WalkPace::new();
/// let start = Instant::now();
///
/// // A burst after standing still is fine: that is what the bucket is for.
/// for step in 0..15 {
///     assert!(pace.allow(start + Duration::from_millis(step), false, false).is_allowed());
/// }
/// // Past the bucket, it is not.
/// assert!(!pace.allow(start, false, false).is_allowed());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WalkPace {
    /// When the bucket was last refilled. `None` before the first step.
    last_step: Option<Instant>,
    /// Credit in milliseconds.
    credit:    i64,
}

impl Default for WalkPace {
    fn default() -> Self {
        Self::new()
    }
}

impl WalkPace {
    /// A pace with a full bucket.
    ///
    /// Full rather than empty on purpose: a character that has just entered the
    /// world has not been running, and starting it in debt would refuse its
    /// first steps.
    pub const fn new() -> Self {
        Self {
            last_step: None,
            credit:    Self::capacity(),
        }
    }

    /// The bucket's size, in milliseconds.
    ///
    /// Measured in walking steps even for a runner: a run costs half as much, so
    /// a runner gets twice the burst out of the same bucket, which is the right
    /// shape — a runner really does take more steps per second.
    const fn capacity() -> i64 {
        WALK_BUFFER as i64 * WALK_INTERVAL.as_millis() as i64
    }

    /// How much credit is banked, in whole walking steps.
    pub const fn credit_steps(&self) -> i64 {
        self.credit / WALK_INTERVAL.as_millis() as i64
    }

    /// Ask whether a step may be taken now, and charge for it if so.
    ///
    /// `running` and `mounted` pick the interval — a mobile that is *allowed* to move
    /// faster is not cheating by doing so, and a horse is the fastest thing a player
    /// legitimately is. ServUO's four rates, halved; see [`MOUNTED_RUN_INTERVAL`].
    pub fn allow(&mut self, now: Instant, running: bool, mounted: bool) -> Pace {
        let cost = match (mounted, running) {
            (true, true) => MOUNTED_RUN_INTERVAL,
            (true, false) | (false, true) => RUN_INTERVAL,
            (false, false) => WALK_INTERVAL,
        }
        .as_millis() as i64;

        // Refill for however long has passed. Saturating because the clock is a
        // parameter and `duration_since` panics in debug on a backwards one —
        // which is not the client's doing, and refilling nothing is the strict
        // reading.
        if let Some(last) = self.last_step {
            let elapsed = now.saturating_duration_since(last).as_millis() as i64;
            self.credit = (self.credit + elapsed).min(Self::capacity());
        }
        self.last_step = Some(now);

        if self.credit < cost {
            // Empty. Refuse without charging: a refused step costs nothing, so a
            // client that stops flooding recovers as soon as time passes rather
            // than digging itself deeper.
            return Pace::TooFast;
        }
        self.credit -= cost;
        Pace::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Take `count` steps `gap` apart, and report how many were refused.
    fn walk(pace: &mut WalkPace, count: u32, gap: Duration, running: bool) -> u32 {
        ride(pace, count, gap, running, false)
    }

    /// The same, with the choice of mount.
    fn ride(pace: &mut WalkPace, count: u32, gap: Duration, running: bool, mounted: bool) -> u32 {
        let start = Instant::now();
        (0..count)
            .filter(|step| pace.allow(start + gap * *step, running, mounted) == Pace::TooFast)
            .count() as u32
    }

    #[test]
    fn a_mounted_gallop_is_never_refused() {
        // The regression this exists for: a mounted runner is legitimately twice as
        // fast as anything the budget knew about, so charged the on-foot rate it spent
        // credit faster than it earned and rubber-banded on a long ride.
        let mut pace = WalkPace::new();
        assert_eq!(
            ride(&mut pace, 5000, MOUNTED_RUN_INTERVAL, true, true),
            0,
            "a horse at a gallop"
        );

        // And on foot at the same cadence roughly every other step is refused, because
        // a person cannot keep it up: it earns 50ms a step and a foot run costs 100, so
        // the bucket drains and then allows one step in two for ever.
        let mut pace = WalkPace::new();
        let refused = ride(&mut pace, 5000, MOUNTED_RUN_INTERVAL, true, false);
        assert!(
            (2000..3000).contains(&refused),
            "{refused} of 5000 refused on foot; about half is the steady state"
        );
    }

    #[test]
    fn a_walking_mount_earns_the_running_rate() {
        // ServUO's `WalkMount` equals its `RunFoot`: a horse at a walk keeps pace with
        // a person at a run, and the floor has to say so or leading a horse through a
        // town at a walk is throttled.
        let mut pace = WalkPace::new();
        assert_eq!(ride(&mut pace, 2000, RUN_INTERVAL, false, true), 0);
    }

    #[test]
    fn the_floors_are_half_the_references_real_rates() {
        // ServUO names the real gaps (WalkFoot 400, RunFoot 200, WalkMount 200,
        // RunMount 100); Sphere names one 200ms walking floor, which is half ServUO's
        // foot walk. They are not the same quantity — one is a step rate, the other a
        // lenient anti-speedhack floor — and this is the relationship that reconciles
        // them, written down so the next person does not "fix" one to match the other.
        assert_eq!(WALK_INTERVAL.as_millis() * 2, 400, "ServUO WalkFoot");
        assert_eq!(RUN_INTERVAL.as_millis() * 2, 200, "ServUO RunFoot/WalkMount");
        assert_eq!(MOUNTED_RUN_INTERVAL.as_millis() * 2, 100, "ServUO RunMount");
    }

    #[test]
    fn the_holds_are_the_references_real_rates() {
        // The other half of the test above, from the side that has to be the
        // real gap rather than the floor: a glide and a walking animation last
        // exactly one step, so these are ServUO's numbers and not Sphere's.
        assert_eq!(WALK_HOLD.as_millis(), 400, "ServUO WalkFoot");
        assert_eq!(RUN_HOLD.as_millis(), 200, "ServUO RunFoot");
        assert_eq!(RUN_HOLD * 2, WALK_HOLD, "a run is a walk doubled");
        assert_eq!(MOUNTED_RUN_HOLD.as_millis(), 100, "ServUO RunMount");
        assert_eq!(MOUNTED_RUN_HOLD * 2, RUN_HOLD, "a gallop is a run doubled");
    }

    #[test]
    fn a_mounted_walk_keeps_the_run_holds_pace() {
        // ServUO's `WalkMount` equals its `RunFoot`: a horse at a walk is drawn
        // no slower than a person at a run, the same equivalence
        // `a_walking_mount_earns_the_running_rate` asserts for the pace budget.
        assert_eq!(step_hold(false, true), RUN_HOLD);
    }

    #[test]
    fn nominal_step_progress_is_constant_and_shared_by_the_client_and_its_oracle() {
        assert_eq!(step_hold(false, false), WALK_HOLD);
        assert_eq!(step_hold(true, false), RUN_HOLD);
        assert_eq!(
            step_hold(true, true),
            MOUNTED_RUN_HOLD,
            "a gallop is the fastest hold"
        );
        assert_eq!(step_progress(Duration::ZERO, WALK_HOLD), 0.0);
        assert_eq!(step_progress(WALK_HOLD / 4, WALK_HOLD), 0.25);
        assert_eq!(step_progress(WALK_HOLD, WALK_HOLD), 1.0);
        assert_eq!(
            step_progress(WALK_HOLD * 2, WALK_HOLD),
            1.0,
            "a step cannot overshoot"
        );
    }

    #[test]
    fn the_intervals_are_spheres() {
        assert_eq!(WALK_INTERVAL.as_millis(), 200, "on foot");
        assert_eq!(RUN_INTERVAL.as_millis(), 100, "mounted or running");
    }

    #[test]
    fn walking_at_a_human_pace_is_never_refused() {
        let mut pace = WalkPace::new();
        assert_eq!(walk(&mut pace, 5000, WALK_INTERVAL, false), 0);
    }

    #[test]
    fn walking_slower_than_the_minimum_is_never_refused() {
        let mut pace = WalkPace::new();
        assert_eq!(walk(&mut pace, 5000, Duration::from_millis(400), false), 0);
    }

    #[test]
    fn teleporting_is_refused_almost_entirely() {
        // A client sending walk packets as fast as it can. The bucket absorbs the
        // first burst — that is the point — and nothing after it.
        let mut pace = WalkPace::new();
        let refused = walk(&mut pace, 500, Duration::ZERO, false);
        assert_eq!(
            refused,
            500 - WALK_BUFFER,
            "exactly the bucket should get through"
        );
    }

    #[test]
    fn a_burst_after_standing_still_is_allowed() {
        // The reason this is a bucket and not a gate. A real client sends several
        // steps at once when a stall clears, and those are steps the player
        // already made.
        let mut pace = WalkPace::new();
        let start = Instant::now();

        for step in 0..10u32 {
            assert!(
                pace.allow(start + WALK_INTERVAL * step, false, false)
                    .is_allowed()
            );
        }

        let after_stall = start + Duration::from_secs(5);
        for step in 0..8u32 {
            assert!(
                pace.allow(after_stall + Duration::from_millis(step.into()), false, false)
                    .is_allowed(),
                "burst step {step} refused; a gate would do this and the client would rubber-band"
            );
        }
    }

    #[test]
    fn jitter_around_the_minimum_is_never_punished() {
        // Steps alternating slightly fast and slightly slow average to the
        // minimum. This is what a real connection looks like, and a limiter that
        // accuses it is worse than none.
        let mut pace = WalkPace::new();
        let mut at = Instant::now();
        let mut refused = 0;

        for step in 0..5000u32 {
            at += Duration::from_millis(if step % 2 == 0 { 190 } else { 210 });
            if pace.allow(at, false, false) == Pace::TooFast {
                refused += 1;
            }
        }
        assert_eq!(refused, 0);
    }

    #[test]
    fn a_slow_link_that_bursts_is_never_punished() {
        // Nastier than jitter: nothing for a second, then five steps at once,
        // forever. The average is honest; the arrivals are not. This is exactly
        // the player a hard gate punishes and a bucket does not.
        let mut pace = WalkPace::new();
        let mut at = Instant::now();
        let mut refused = 0;

        for _ in 0..500 {
            at += Duration::from_secs(1);
            for _ in 0..5u32 {
                at += Duration::from_millis(1);
                if pace.allow(at, false, false) == Pace::TooFast {
                    refused += 1;
                }
            }
        }
        assert_eq!(refused, 0, "a bursty but honest client was accused");
    }

    #[test]
    fn running_is_allowed_to_be_faster() {
        // A runner moves at 100ms. That is not cheating, and charging it the
        // walking rate would refuse every runner on the shard.
        let mut pace = WalkPace::new();
        assert_eq!(walk(&mut pace, 5000, RUN_INTERVAL, true), 0);
    }

    #[test]
    fn running_speed_claimed_as_a_walk_is_refused() {
        // The same 100ms pace, not running, is twice what a body can do.
        let mut pace = WalkPace::new();
        let refused = walk(&mut pace, 500, RUN_INTERVAL, false);
        assert!(refused > 200, "only {refused} of 500 refused");
    }

    #[test]
    fn standing_still_does_not_bank_a_fortune() {
        // Without a cap, a minute of standing still would buy 300 free steps and
        // a speedhacker would simply wait first.
        let mut pace = WalkPace::new();
        let start = Instant::now();
        pace.allow(start, false, false);
        pace.allow(start + Duration::from_secs(60), false, false);

        assert!(
            pace.credit_steps() <= WALK_BUFFER as i64,
            "banked {} steps after a minute; the cap is {WALK_BUFFER}",
            pace.credit_steps()
        );

        let after = start + Duration::from_secs(60);
        let mut refused = 0;
        for step in 0..100u32 {
            if pace.allow(after + Duration::from_millis(step.into()), false, false) == Pace::TooFast {
                refused += 1;
            }
        }
        assert!(refused > 80, "only {refused} of 100 instant steps refused");
    }

    #[test]
    fn credit_is_capped_while_walking_normally_too() {
        let mut pace = WalkPace::new();
        let start = Instant::now();
        for step in 0..1000u32 {
            pace.allow(start + Duration::from_millis(500) * step, false, false);
        }
        assert!(pace.credit_steps() <= WALK_BUFFER as i64);
    }

    #[test]
    fn the_first_step_is_always_allowed() {
        // No previous step to measure against, and a character that just entered
        // the world has not been running.
        let mut pace = WalkPace::new();
        assert!(pace.allow(Instant::now(), false, false).is_allowed());
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_panic() {
        // `Instant` is monotonic, so this should be impossible — but the clock is
        // a parameter, and a caller can hand over anything.
        let mut pace = WalkPace::new();
        let start = Instant::now();
        pace.allow(start + Duration::from_secs(10), false, false);
        let _ = pace.allow(start, false, false);
    }

    #[test]
    fn a_refused_step_costs_nothing() {
        // A client that floods and then behaves must recover on its own. Charging
        // for refusals would dig it deeper the harder it tried.
        let mut pace = WalkPace::new();
        walk(&mut pace, 500, Duration::ZERO, false);
        assert!(pace.credit_steps() >= 0, "left in debt by refusals");

        let start = Instant::now();
        let mut refused = 0;
        for step in 1..=50u32 {
            if pace.allow(start + WALK_INTERVAL * step, false, false) == Pace::TooFast {
                refused += 1;
            }
        }
        assert_eq!(refused, 0, "an honest client stayed blocked after a flood");
    }
}
