//! Whether the shard's tick is as long as the shard says it is.
//!
//! # The defect this exists to make visible
//!
//! Every duration this shard puts on the wire is a tick count converted at the
//! declared rate: `CombatAction::wire_phase` announces `impact_in` as
//! `ticks_to_millis(impact - now)`, and `animate_timed` stretches the gesture
//! over the same arithmetic. Both are correct *in tick-space*, and they cannot
//! disagree with the schedule — the schedule is the same number.
//!
//! What they can disagree with is the second. A tick that overruns its budget is
//! not caught up by the next one — [`tokio::time::MissedTickBehavior::Delay`],
//! chosen deliberately in [`crate::shard::run_shard`], turns an overrun into a
//! slower clock rather than a burst of ticks — so a shard under load keeps
//! simulating correctly at a rate nobody published. A bow that announces 1600ms
//! and lands 6500ms later is not a combat defect at all: it is 64 ticks taking
//! four times as long as 64 ticks are declared to take, and every tick-by-tick
//! oracle in the tree is blind to it by construction, because both sides of what
//! it checks are counted in the unit that slipped.
//!
//! So this is the one place in the shard that is *allowed* to read the wall
//! clock, and it is outside [`openshard_world::World::tick`] on purpose:
//! `docs/style.md` § Randomness and time forbids the wall clock **inside** the
//! tick, because a system that reads `Instant::now()` while simulating has
//! broken replay. Measuring the tick from the loop that drives it breaks
//! nothing — the world never sees this, nothing branches on it, and a run with
//! the measurement and a run without it produce the same world.
//!
//! # Why the verdict has no tunable threshold
//!
//! A margin chosen by eye is the fudge constant `docs/style.md` forbids: it
//! could only be right on the machine it was tuned on. The unit here is the
//! **tick**, which is the thing being counted, so the comparison is made in
//! whole ticks — a window is *behind* when it delivered its ticks over a span
//! longer than their budget by at least one whole tick. A shard drifting by
//! microseconds is zero ticks behind and says nothing; a shard running at a
//! quarter of its rate is thirty ticks behind every second and says so at once.

use std::time::{Duration, Instant};

use openshard_world::TICK_INTERVAL;

/// How many intervals one verdict is measured over: one second's worth at the
/// declared rate.
///
/// A second rather than a number chosen for smoothness — it is the unit the
/// verdict is reported in (*ticks per second* against a declared
/// `TICKS_PER_SECOND`), and a window measured in one unit and reported in
/// another is one more conversion for a reader to check.
const WINDOW: u32 = openshard_state::TICKS_PER_SECOND as u32;

/// What one closed window found.
///
/// Every field is a measurement rather than a judgement, so that the line an
/// operator reads is the same one a person diagnosing it would have asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Window {
    /// How long the window's [`WINDOW`] intervals actually took.
    pub(crate) elapsed: Duration,
    /// How much of that was spent inside the tick itself.
    ///
    /// **This is the field that says whose fault it is.** Near the whole window
    /// means the tick body is too slow and the shard is the problem; a small
    /// share means the tick was ready on time and was not run — the runtime, the
    /// other threads in this process, or the machine.
    pub(crate) busy: Duration,
    /// The longest single tick in the window, which is what an average hides.
    pub(crate) worst: Duration,
}

impl Window {
    /// The budget the declared rate promised for this many intervals.
    const fn budget() -> Duration {
        Duration::from_millis(TICK_INTERVAL.as_millis() as u64 * WINDOW as u64)
    }

    /// How many whole ticks of time this window lost against its budget.
    ///
    /// Whole ticks because the tick is the unit that slipped: a window that came
    /// in a hair late lost none, and there is nothing to report about it.
    #[must_use]
    pub(crate) fn behind_ticks(self) -> u32 {
        let overrun = self.elapsed.saturating_sub(Self::budget());
        u32::try_from(overrun.as_nanos() / TICK_INTERVAL.as_nanos()).unwrap_or(u32::MAX)
    }

    /// Ticks per second, as this window actually delivered them.
    #[must_use]
    pub(crate) fn observed_rate(self) -> f32 {
        match self.elapsed.is_zero() {
            // A window with no time in it cannot have a rate. It cannot happen
            // with a real clock, and reporting an infinity would be worse than
            // reporting the rate that was asked for.
            true => WINDOW as f32,
            false => WINDOW as f32 / self.elapsed.as_secs_f32(),
        }
    }

    /// The share of the window spent inside the tick, `0.0..=1.0`.
    #[must_use]
    pub(crate) fn busy_share(self) -> f32 {
        match self.elapsed.is_zero() {
            true => 0.0,
            false => self.busy.as_secs_f32() / self.elapsed.as_secs_f32(),
        }
    }

    /// Whether this window failed to keep the rate the shard publishes.
    #[must_use]
    pub(crate) fn behind(self) -> bool {
        self.behind_ticks() > 0
    }
}

/// The interval a tick opened, held until the next tick closes it.
///
/// An interval is measured between two tick *starts*, because that — and not the
/// duration of the body — is the span the wire's arithmetic is denominated in.
/// The body is carried alongside it so that the tick occupying an interval and
/// the interval it occupied are accumulated together, rather than one window's
/// spans being summed against the neighbouring window's bodies.
#[derive(Clone, Copy, Debug)]
struct Open {
    began: Instant,
    body: Duration,
}

/// What has accumulated since the last verdict.
#[derive(Clone, Copy, Debug)]
struct Partial {
    intervals: u32,
    elapsed: Duration,
    busy: Duration,
    worst: Duration,
}

impl Partial {
    /// Nothing measured yet.
    const fn empty() -> Self {
        Self {
            intervals: 0,
            elapsed: Duration::ZERO,
            busy: Duration::ZERO,
            worst: Duration::ZERO,
        }
    }
}

/// The shard's own pace, measured against the one it publishes.
///
/// Fed one call per tick from the driving loop, it answers with a [`Window`]
/// once a second's worth of intervals have been measured, and otherwise with
/// nothing. Deciding what to *say* about a window is the caller's — see
/// [`Pace::verdict`], which is where the standing state and its edges live.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pace {
    /// The interval currently open, or `None` before the first tick.
    ///
    /// Absence here is the domain's — there genuinely is no open interval before
    /// a tick has run — and not a value waiting to be filled in.
    open: Option<Open>,
    partial: Partial,
    /// What the last window said, so that a standing state can be announced on
    /// its edges rather than restated every second. `None` until the first
    /// window closes, which is why the first verdict is always spoken.
    said: Option<bool>,
}

impl Pace {
    /// A pace that has measured nothing.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            open: None,
            partial: Partial::empty(),
            said: None,
        }
    }

    /// Record one tick: when it began, and how long its body took.
    ///
    /// Returns the window it closed, if it closed one.
    pub(crate) fn record(&mut self, began: Instant, body: Duration) -> Option<Window> {
        let previous = self.open.replace(Open { began, body })?;
        // One interval is complete: from the previous tick's start to this one's,
        // occupied by the previous tick's body.
        self.partial.intervals += 1;
        self.partial.elapsed += began.saturating_duration_since(previous.began);
        self.partial.busy += previous.body;
        self.partial.worst = self.partial.worst.max(previous.body);
        if self.partial.intervals < WINDOW {
            return None;
        }
        let window = Window {
            elapsed: self.partial.elapsed,
            busy: self.partial.busy,
            worst: self.partial.worst,
        };
        self.partial = Partial::empty();
        Some(window)
    }

    /// Whether this window is worth a line, given what has already been said.
    ///
    /// A shard that cannot keep its rate is a **standing state**, not an event:
    /// saying it once a second for as long as it lasts buries the log, and saying
    /// it once and never again hides the recovery. So it is announced on its
    /// edges — the same shape `docs/combat_actions.md`'s D11 settled on for a
    /// refusal, and for the same reason: what a reader needs is when it started
    /// and when it stopped.
    pub(crate) fn verdict(&mut self, window: Window) -> Option<Verdict> {
        let behind = window.behind();
        if self.said == Some(behind) {
            return None;
        }
        self.said = Some(behind);
        Some(match behind {
            true => Verdict::FellBehind(window),
            false => Verdict::CaughtUp(window),
        })
    }
}

/// An edge in the shard's ability to keep its declared rate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Verdict {
    /// The shard stopped keeping the rate every duration it announces is
    /// denominated in.
    FellBehind(Window),
    /// It is keeping it again — or, on the first window of a run, was all along.
    CaughtUp(Window),
}

#[cfg(test)]
mod tests {
    use super::{Pace, Verdict, WINDOW, Window};
    use openshard_world::TICK_INTERVAL;
    use std::time::{Duration, Instant};

    /// Drive `pace` through `intervals` ticks spaced `every` apart, each tick's
    /// body costing `body`, and return every verdict it produced.
    fn run(pace: &mut Pace, intervals: u32, every: Duration, body: Duration) -> Vec<Verdict> {
        let start = Instant::now();
        let mut said = Vec::new();
        // One extra tick, because `intervals` intervals need `intervals + 1`
        // starts to be measured between.
        for step in 0..=intervals {
            let at = start + every * step;
            if let Some(window) = pace.record(at, body) {
                if let Some(verdict) = pace.verdict(window) {
                    said.push(verdict);
                }
            }
        }
        said
    }

    /// The whole point: a shard keeping its own declared rate says nothing about
    /// falling behind, however long it runs.
    #[test]
    fn a_shard_running_at_its_declared_rate_is_never_behind() {
        let mut pace = Pace::new();
        let said = run(&mut pace, WINDOW * 3, TICK_INTERVAL, Duration::from_millis(1));
        assert!(
            said.iter().all(|verdict| matches!(verdict, Verdict::CaughtUp(_))),
            "a punctual shard was reported as behind: {said:?}"
        );
    }

    /// The measured defect: 25ms ticks arriving 100ms apart. Thirty of every
    /// forty ticks are lost, and every duration the shard announces is four
    /// times shorter than the one it delivers.
    #[test]
    fn a_tick_four_times_too_slow_is_reported_as_three_quarters_of_a_second_behind() {
        let mut pace = Pace::new();
        let said = run(&mut pace, WINDOW, TICK_INTERVAL * 4, Duration::from_millis(1));
        let [Verdict::FellBehind(window)] = said[..] else {
            panic!("a quarter-speed shard did not report falling behind: {said:?}");
        };
        assert_eq!(
            window.behind_ticks(),
            WINDOW * 3,
            "four times too slow loses three ticks for every one it keeps"
        );
        assert!(
            (window.observed_rate() - 10.0).abs() < 0.1,
            "a quarter of forty ticks a second is ten, not {}",
            window.observed_rate()
        );
    }

    /// A drift far below one tick is not a slow shard, and a report of one would
    /// be a threshold tuned by eye pretending to be a measurement.
    #[test]
    fn a_window_late_by_less_than_a_whole_tick_is_not_behind() {
        let mut pace = Pace::new();
        let late = TICK_INTERVAL + Duration::from_micros(100);
        let said = run(&mut pace, WINDOW, late, Duration::from_millis(1));
        assert!(
            matches!(said[..], [Verdict::CaughtUp(_)]),
            "a window 4ms late over a whole second was called behind: {said:?}"
        );
    }

    /// One window's measurements, for the assertions that are about the numbers
    /// rather than about what was said.
    fn window_of(every: Duration, body: Duration) -> Window {
        let mut pace = Pace::new();
        let start = Instant::now();
        let mut closed = None;
        for step in 0..=WINDOW {
            closed = pace.record(start + every * step, body).or(closed);
        }
        closed.expect("a whole window's worth of intervals was driven")
    }

    /// The field that says whose fault it is. A tick that is ready on time and
    /// is not run looks nothing like one that overruns, and the log has to be
    /// able to tell an operator which of the two they have.
    #[test]
    fn the_busy_share_separates_a_slow_tick_from_a_starved_one() {
        let starved = window_of(TICK_INTERVAL * 4, Duration::from_millis(1));
        let overrunning = window_of(TICK_INTERVAL * 4, TICK_INTERVAL * 4);
        assert!(
            starved.busy_share() < 0.05,
            "a starved shard's tick was reported as busy: {}",
            starved.busy_share()
        );
        assert!(
            overrunning.busy_share() > 0.95,
            "an overrunning tick was reported as idle: {}",
            overrunning.busy_share()
        );
    }

    /// Announced on its edges. A standing state restated every second buries the
    /// log; one stated once hides the recovery.
    #[test]
    fn falling_behind_and_recovering_are_one_line_each() {
        let mut pace = Pace::new();
        let mut said = run(&mut pace, WINDOW, TICK_INTERVAL, Duration::from_millis(1));
        said.extend(run(
            &mut pace,
            WINDOW * 2,
            TICK_INTERVAL * 4,
            Duration::from_millis(1),
        ));
        said.extend(run(
            &mut pace,
            WINDOW * 2,
            TICK_INTERVAL,
            Duration::from_millis(1),
        ));
        assert_eq!(
            said.len(),
            3,
            "two seconds behind and two seconds recovered should be three lines, not: {said:?}"
        );
        assert!(matches!(said[0], Verdict::CaughtUp(_)));
        assert!(matches!(said[1], Verdict::FellBehind(_)));
        assert!(matches!(said[2], Verdict::CaughtUp(_)));
    }
}
