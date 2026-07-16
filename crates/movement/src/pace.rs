//! How often a mobile is allowed to take a step.

use std::time::{Duration, Instant};

/// The shortest gap between two walking steps, on foot.
///
/// Sphere's `CClient::Event_Walking`.
pub const WALK_INTERVAL: Duration = Duration::from_millis(200);

/// The shortest gap between two steps when running, or mounted.
///
/// Sphere uses this for a mount, for hovering, and for `speedMode == 1`.
pub const RUN_INTERVAL: Duration = Duration::from_millis(100);

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
///     assert!(pace.allow(start + Duration::from_millis(step), false).is_allowed());
/// }
/// // Past the bucket, it is not.
/// assert!(!pace.allow(start, false).is_allowed());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WalkPace {
    /// When the bucket was last refilled. `None` before the first step.
    last_step: Option<Instant>,
    /// Credit in milliseconds.
    credit: i64,
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
            credit: Self::capacity(),
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
    /// `running` picks the shorter interval — a running mobile is *allowed* to
    /// move faster, so it is not cheating by doing so.
    pub fn allow(&mut self, now: Instant, running: bool) -> Pace {
        let cost = if running { RUN_INTERVAL } else { WALK_INTERVAL }.as_millis() as i64;

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
        let start = Instant::now();
        (0..count)
            .filter(|step| pace.allow(start + gap * *step, running) == Pace::TooFast)
            .count() as u32
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
            assert!(pace.allow(start + WALK_INTERVAL * step, false).is_allowed());
        }

        let after_stall = start + Duration::from_secs(5);
        for step in 0..8u32 {
            assert!(
                pace.allow(after_stall + Duration::from_millis(step.into()), false)
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
            if pace.allow(at, false) == Pace::TooFast {
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
                if pace.allow(at, false) == Pace::TooFast {
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
        pace.allow(start, false);
        pace.allow(start + Duration::from_secs(60), false);

        assert!(
            pace.credit_steps() <= WALK_BUFFER as i64,
            "banked {} steps after a minute; the cap is {WALK_BUFFER}",
            pace.credit_steps()
        );

        let after = start + Duration::from_secs(60);
        let mut refused = 0;
        for step in 0..100u32 {
            if pace.allow(after + Duration::from_millis(step.into()), false) == Pace::TooFast {
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
            pace.allow(start + Duration::from_millis(500) * step, false);
        }
        assert!(pace.credit_steps() <= WALK_BUFFER as i64);
    }

    #[test]
    fn the_first_step_is_always_allowed() {
        // No previous step to measure against, and a character that just entered
        // the world has not been running.
        let mut pace = WalkPace::new();
        assert!(pace.allow(Instant::now(), false).is_allowed());
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_panic() {
        // `Instant` is monotonic, so this should be impossible — but the clock is
        // a parameter, and a caller can hand over anything.
        let mut pace = WalkPace::new();
        let start = Instant::now();
        pace.allow(start + Duration::from_secs(10), false);
        let _ = pace.allow(start, false);
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
            if pace.allow(start + WALK_INTERVAL * step, false) == Pace::TooFast {
                refused += 1;
            }
        }
        assert_eq!(refused, 0, "an honest client stayed blocked after a flood");
    }
}
