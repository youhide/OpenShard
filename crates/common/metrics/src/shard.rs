//! What a running shard publishes about itself.
//!
//! # Why the values live behind a shared handle
//!
//! `docs/style.md` and the workspace's own habits want ownership visible in a
//! signature, and an `Arc` hides it — so this is the case that has to be argued
//! rather than assumed. The shard loop writes these values and a socket task
//! reads them, the two run on different Tokio worker threads, and neither may
//! own the other: the reader has to keep answering while the writer is inside a
//! long save, and the writer must not wait on a scrape. There is no owner among
//! them, exactly as there is none among the clones of a
//! `openshard_gateway::Shutdown` or of the save tally beside it, and for the
//! same reason. What is shared is genuinely shared mutable state and not a
//! borrow this crate could not spell.
//!
//! Everything numeric is an atomic read or write with [`Ordering::Relaxed`],
//! because nothing branches on these: they are what an operator looks at, and a
//! reader that catches two of them a microsecond apart has read a pair that was
//! true a microsecond ago. Paying for more would suggest something depends on
//! it.
//!
//! The one exception is the last closed pace window, which is five numbers and a
//! string that only mean anything *together* — a busy share from one second
//! beside a worst tick from another is a sentence about no second at all. That
//! goes behind a `Mutex`: one writer a second, one reader a scrape, never held
//! across an await.

use std::sync::atomic::{
    AtomicBool,
    AtomicU64,
    Ordering,
};
use std::sync::{
    Arc,
    Mutex,
};
use std::time::{
    Duration,
    Instant,
};

/// How long one tick is *declared* to be, straight from the world that runs it.
///
/// A newtype because this crate is handed several durations — a window's worst
/// tick, an uptime — and this is the only one that is a promise rather than a
/// measurement. It is also the only thing this crate is told about what a tick
/// *is*: the rate it publishes is derived from this and nothing else, so a shard
/// that changed its tick rate could not report the old one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TickInterval(pub Duration);

/// One closed tick-pace window, as the shard measured it.
///
/// The fields are the shard's own measurements moved across a crate boundary,
/// not a second opinion about them: what fills this in is
/// `openshard_server::pace::Window`, whose doc explains why each of them is
/// worth having and why the comparison is made in whole ticks.
#[derive(Clone, PartialEq, Debug)]
pub struct TickWindow {
    /// Ticks per second this window actually delivered.
    pub observed_rate: f32,
    /// The share of the window spent inside the tick body, `0.0..=1.0`. This is
    /// the field that says whose fault a slow window is: near one means the tick
    /// is too slow, near zero means it was ready and was not run.
    pub busy_share:    f32,
    /// The longest single tick in the window, which is what an average hides.
    pub worst:         Duration,
    /// What that longest tick was applying, already rendered to a line.
    ///
    /// A `String` and not a structure, because this crate has no business
    /// knowing what a command is, and because it is the one value here that a
    /// scraper must never see: a free-form label is unbounded cardinality, and a
    /// Prometheus series per command mix is how a monitoring system is brought
    /// down by the thing meant to watch it. It reaches [`crate::health`] and
    /// stops there.
    pub worst_work:    String,
    /// Whole ticks of time this window lost against its budget. Zero for a
    /// window that kept the rate.
    pub behind_ticks:  u32,
}

/// What the save task has been handed and has not finished writing.
///
/// Both halves together, because either alone misleads: "three writes" says
/// nothing about what a force-exit would cost, and "12,000 rows" says nothing
/// about whether one sweep is stuck or a hundred small writes are queued.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SaveBacklog {
    /// Snapshots handed over and not yet answered for. One is one transaction.
    pub writes: u64,
    /// How many rows are inside them.
    pub rows:   u64,
}

/// The cells themselves. Private: everything outside reaches them through
/// [`ShardMetrics`], so there is exactly one way to publish each value and one
/// way to read the set.
#[derive(Debug)]
struct Cells {
    /// When this shard came up, which is what every elapsed span here is
    /// measured from. Immutable, so it needs no atomic.
    started:          Instant,
    /// The rate the shard publishes, derived once from the declared interval.
    ticks_per_second: f32,
    ticks:            AtomicU64,
    /// Milliseconds since [`Cells::started`] at the end of the last tick. Read
    /// against `started.elapsed()` to say how long ago that was. Meaningless
    /// while `ticks` is zero, which is what [`ShardMetrics::read`] checks.
    last_tick_millis: AtomicU64,
    connections:      AtomicU64,
    saves_completed:  AtomicU64,
    saves_failed:     AtomicU64,
    unwritten_writes: AtomicU64,
    unwritten_rows:   AtomicU64,
    stopping:         AtomicBool,
    /// The last closed pace window, or nothing in a shard's first second.
    ///
    /// Absence here is the domain's: a window that has not closed has no rate,
    /// and publishing a zero would be a shard claiming to run at no ticks per
    /// second while it is starting up perfectly well.
    window:           Mutex<Option<TickWindow>>,
}

/// The live values a running shard publishes about itself.
///
/// Cloning hands out another hold on the same values; there is no owner among
/// the clones. See this module's header for why that is the shape.
#[derive(Clone, Debug)]
pub struct ShardMetrics(Arc<Cells>);

impl ShardMetrics {
    /// A shard that has just come up, running at the declared interval.
    ///
    /// # Panics
    ///
    /// If the interval is zero. A tick of no duration is not a slow shard or an
    /// unusual configuration, it is a rate of infinity, and every number derived
    /// from it downstream would be a lie rather than a surprise.
    pub fn declaring(tick: TickInterval) -> Self {
        assert!(
            !tick.0.is_zero(),
            "a declared tick interval of zero is not a rate this crate can publish"
        );
        Self(Arc::new(Cells {
            started:          Instant::now(),
            ticks_per_second: 1.0 / tick.0.as_secs_f32(),
            ticks:            AtomicU64::new(0),
            last_tick_millis: AtomicU64::new(0),
            connections:      AtomicU64::new(0),
            saves_completed:  AtomicU64::new(0),
            saves_failed:     AtomicU64::new(0),
            unwritten_writes: AtomicU64::new(0),
            unwritten_rows:   AtomicU64::new(0),
            stopping:         AtomicBool::new(false),
            window:           Mutex::new(None),
        }))
    }

    /// One tick finished.
    ///
    /// Called from the loop that drives the clock, forty times a second, so it
    /// is two relaxed atomic writes and nothing else. What it buys is the one
    /// thing a log cannot say: a shard whose tick has stopped entirely prints
    /// nothing at all, and this is what makes that silence visible from outside
    /// the process.
    pub fn tick_ran(&self) {
        self.0.ticks.fetch_add(1, Ordering::Relaxed);
        self.0
            .last_tick_millis
            .store(self.millis_now(), Ordering::Relaxed);
    }

    /// A pace window closed. Published on every window, not only on the ones
    /// worth a log line: a log is edge-triggered because a standing state
    /// restated every second buries it, and a sample is the opposite — a series
    /// with a hole in it where the shard was fine is a series nobody can average.
    pub fn tick_window(&self, window: TickWindow) {
        *self.window_cell() = Some(window);
    }

    /// What the save task owes the disk, as of now.
    pub fn save_backlog(&self, backlog: SaveBacklog) {
        self.0.unwritten_writes.store(backlog.writes, Ordering::Relaxed);
        self.0.unwritten_rows.store(backlog.rows, Ordering::Relaxed);
    }

    /// The store answered successfully for one snapshot.
    pub fn save_completed(&self) {
        self.0.saves_completed.fetch_add(1, Ordering::Relaxed);
    }

    /// The store refused one, and the world will be swept in full next time.
    pub fn save_failed(&self) {
        self.0.saves_failed.fetch_add(1, Ordering::Relaxed);
    }

    /// How many client connections the shard is holding.
    pub fn connections(&self, open: usize) {
        self.0.connections.store(open as u64, Ordering::Relaxed);
    }

    /// This shard has been asked to stop and is on its way out.
    ///
    /// One way only, deliberately: a shard does not un-stop, and a setter that
    /// took a bool would let a caller say so.
    pub fn stopping(&self) {
        self.0.stopping.store(true, Ordering::Relaxed);
    }

    /// Everything at once, as one owned value.
    ///
    /// The renderers take a [`Reading`] rather than this handle so that a
    /// document cannot be assembled out of two different instants, and so that
    /// neither of them can write.
    pub fn read(&self) -> Reading {
        let ticks = self.0.ticks.load(Ordering::Relaxed);
        Reading {
            uptime: self.0.started.elapsed(),
            declared_rate: self.0.ticks_per_second,
            ticks,
            // Nothing to say about the last tick of a shard that has not had
            // one. The `0` in the cell is not a tick that happened at startup.
            since_last_tick: match ticks {
                0 => None,
                _ => {
                    let last = self.0.last_tick_millis.load(Ordering::Relaxed);
                    Some(Duration::from_millis(self.millis_now().saturating_sub(last)))
                }
            },
            connections: self.0.connections.load(Ordering::Relaxed),
            saves_completed: self.0.saves_completed.load(Ordering::Relaxed),
            saves_failed: self.0.saves_failed.load(Ordering::Relaxed),
            backlog: SaveBacklog {
                writes: self.0.unwritten_writes.load(Ordering::Relaxed),
                rows:   self.0.unwritten_rows.load(Ordering::Relaxed),
            },
            stopping: self.0.stopping.load(Ordering::Relaxed),
            window: self.window_cell().clone(),
        }
    }

    /// How long this shard has been up, in whole milliseconds.
    ///
    /// Saturating rather than wrapping: `u64` milliseconds is 584 million years,
    /// so the clamp is unreachable, and reaching for it would be a clock read
    /// that went backwards rather than a shard that ran that long.
    fn millis_now(&self) -> u64 {
        u64::try_from(self.0.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// The window cell, recovered if a writer panicked while holding it.
    ///
    /// A poisoned lock here means some earlier publisher panicked between taking
    /// the lock and releasing it — and what is inside is a measurement, not an
    /// invariant somebody was halfway through repairing. Refusing to report
    /// anything ever again because a value is one second stale would be the
    /// monitoring failing louder than the thing it watches.
    fn window_cell(&self) -> std::sync::MutexGuard<'_, Option<TickWindow>> {
        match self.0.window.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Every value a shard publishes, read at one instant.
///
/// Owned and inert: what produces one is [`ShardMetrics::read`], and what
/// consumes one is a renderer. Nothing here can write back.
#[derive(Clone, PartialEq, Debug)]
pub struct Reading {
    /// How long the shard has been up.
    pub uptime:          Duration,
    /// The rate the shard promises, and every duration it puts on the wire is
    /// denominated in. Beside the observed rate rather than left implicit,
    /// because the whole point of the pair is the gap between them.
    pub declared_rate:   f32,
    /// Ticks run since the shard came up.
    pub ticks:           u64,
    /// How long ago the last tick finished, or nothing before the first one.
    ///
    /// This is the liveness signal proper. A shard whose tick has wedged goes on
    /// answering this endpoint from another task, keeps its connections, and
    /// says nothing in the log — and this number grows without bound. What
    /// counts as *too long* is deliberately not decided here; see the crate
    /// header.
    pub since_last_tick: Option<Duration>,
    /// Client connections the shard is holding.
    pub connections:     u64,
    /// Snapshots the store has answered for, successfully, since boot.
    pub saves_completed: u64,
    /// Snapshots the store refused. Each one costs a full sweep next time.
    pub saves_failed:    u64,
    /// What the save task owes the disk right now.
    pub backlog:         SaveBacklog,
    /// Whether a stop has been asked for. The shard is still saving; it is no
    /// longer taking play.
    pub stopping:        bool,
    /// The last closed pace window, or nothing in the shard's first second.
    pub window:          Option<TickWindow>,
}

impl Reading {
    /// Whether this shard is still taking play.
    ///
    /// The one verdict in this crate, and it is a fact rather than a threshold:
    /// a shard that has been asked to stop will refuse what arrives next, so
    /// anything deciding where to send a player wants to hear it now rather than
    /// after the connection is refused.
    pub const fn serving(&self) -> bool {
        !self.stopping
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        SaveBacklog,
        ShardMetrics,
        TickInterval,
        TickWindow,
    };

    /// The shard's own rate, as `openshard_world::TICK_INTERVAL` declares it.
    const DECLARED: TickInterval = TickInterval(Duration::from_millis(25));

    fn a_window(observed_rate: f32, behind_ticks: u32) -> TickWindow {
        TickWindow {
            observed_rate,
            busy_share: 0.5,
            worst: Duration::from_millis(30),
            worst_work: "1 command(s): Walk".to_owned(),
            behind_ticks,
        }
    }

    #[test]
    fn a_shard_that_has_not_ticked_reports_no_window_and_no_last_tick() {
        // The distinction the whole `Option` pair exists for. A zero here would
        // be a shard claiming to run at no ticks per second and to have last
        // ticked at startup, both while it is coming up perfectly normally —
        // and a scraper cannot tell an honest zero from a placeholder one.
        let reading = ShardMetrics::declaring(DECLARED).read();

        assert_eq!(reading.ticks, 0);
        assert!(reading.since_last_tick.is_none(), "no tick has run");
        assert!(reading.window.is_none(), "no window has closed");
        assert!(reading.serving(), "a shard that is starting is taking play");
    }

    #[test]
    fn the_declared_rate_comes_from_the_interval_and_nowhere_else() {
        // 25ms is 40 a second. Derived rather than passed in beside the
        // interval, so the two cannot disagree.
        let metrics = ShardMetrics::declaring(DECLARED);
        assert!((metrics.read().declared_rate - 40.0).abs() < 0.001);
    }

    #[test]
    fn every_published_value_reaches_a_reading() {
        let metrics = ShardMetrics::declaring(DECLARED);
        metrics.tick_ran();
        metrics.tick_ran();
        metrics.tick_window(a_window(10.0, 30));
        metrics.save_backlog(SaveBacklog { writes: 2, rows: 6 });
        metrics.save_completed();
        metrics.save_failed();
        metrics.save_failed();
        metrics.connections(3);

        let reading = metrics.read();
        assert_eq!(reading.ticks, 2);
        assert!(reading.since_last_tick.is_some(), "a tick has run");
        assert_eq!(reading.window.expect("a window closed").behind_ticks, 30);
        assert_eq!(reading.backlog, SaveBacklog { writes: 2, rows: 6 });
        assert_eq!(reading.saves_completed, 1);
        assert_eq!(reading.saves_failed, 2);
        assert_eq!(reading.connections, 3);
    }

    #[test]
    fn a_clone_reads_what_another_clone_published() {
        // Which is the whole reason this is a shared handle: the shard loop
        // writes through one and the socket task reads through another.
        let metrics = ShardMetrics::declaring(DECLARED);
        let elsewhere = metrics.clone();

        metrics.connections(7);
        metrics.tick_window(a_window(40.0, 0));

        let reading = elsewhere.read();
        assert_eq!(reading.connections, 7);
        assert_eq!(reading.window.expect("a window closed").observed_rate, 40.0);
    }

    #[test]
    fn a_shard_that_has_been_asked_to_stop_is_no_longer_serving() {
        let metrics = ShardMetrics::declaring(DECLARED);
        assert!(metrics.read().serving());

        metrics.stopping();
        assert!(
            !metrics.read().serving(),
            "a stopping shard still answers, and what it answers is that it is going"
        );
    }

    #[test]
    fn the_latest_window_replaces_the_one_before_it() {
        // Not accumulated: a window is a second's worth of measurement and the
        // history belongs to whatever scrapes this, which is the only thing that
        // can keep it without bound.
        let metrics = ShardMetrics::declaring(DECLARED);
        metrics.tick_window(a_window(10.0, 30));
        metrics.tick_window(a_window(40.0, 0));

        let window = metrics.read().window.expect("a window closed");
        assert_eq!(window.behind_ticks, 0, "the shard caught up and says so");
    }
}
