//! How a stop is asked for from outside the process.
//!
//! Everything inside the shard already agrees on what a stop *is* — a
//! [`Shutdown`] cloned into the listener, every connection and the tick. What is
//! here is the other side of that: the operating system's ways of saying it.
//!
//! # Why the signals are watched in one place
//!
//! `SIGTERM` and Ctrl-C mean the same thing to this process, and they end at the
//! same `Shutdown::stop()`; the only difference is how they are heard. Keeping
//! the `cfg(unix)` inside this module keeps [`crate::run`] a straight read, and
//! keeps the non-unix build from being a second arrangement that nobody
//! exercises — it is the same [`watch`] loop over a smaller set of signals.
//!
//! `SIGHUP` is deliberately neither a stop nor anything else. It conventionally
//! means "reload your config", this shard cannot reload one, and turning it into
//! a stop would surprise an operator whose terminal merely closed.
//!
//! # The second signal is a force-exit
//!
//! A stop awaits the save task, and the entire reason that task exists is that it
//! may be slow: a wedged Postgres, a disk that has gone away. Without a second
//! way out, the operator's only escape is `SIGKILL`, which from where they stand
//! is indistinguishable from a shard that hung on its own. So the first signal
//! asks and the second leaves, loudly and with a non-zero code. Two deliberate
//! signals is a clear instruction.
//!
//! The cost is that somebody who fat-fingers Ctrl-C twice loses the save they
//! were taking, which is why the *first* line says what the second signal will
//! do. An informed second signal is a decision; an unannounced one would be a
//! trap.

use std::io;

use super::*;

/// What the process exits with when it is told to stop a second time.
///
/// Not 1: a forced exit is not the same event as a shard that failed to start,
/// and an operator reading an exit status should be able to tell them apart.
const FORCED_EXIT_CODE: i32 = 2;

/// Install the signal handlers, before anything can send this process a signal.
///
/// Separate from [`watch`] on purpose. Installation is what races: until it has
/// happened, `SIGTERM`'s default disposition is to kill the process, so a caller
/// that spawns the watching task and then arranges for a signal has a window in
/// which the shard dies instead of stopping. Doing this synchronously in
/// [`crate::run`] closes it, and lets the test send itself a signal at all.
///
/// The error is the operating system refusing a handler. That is worth saying
/// out loud rather than swallowing: the shard still runs, but the only way left
/// to end it is to kill it — which is exactly the save this whole arrangement
/// exists not to lose.
pub fn install() -> io::Result<Signals> {
    Signals::install()
}

/// Ask for a stop on the first signal, and leave on the second.
///
/// Never returns: either the process is being stopped by something else and this
/// task is dropped with it, or the second signal takes the whole process out.
pub async fn watch(mut signals: Signals, reins: Reins) {
    let first = signals.next().await;
    info!(
        signal = first,
        "shutdown requested; saving the world. Signal again to exit at once, without the save."
    );
    reins.shutdown().stop();

    let second = signals.next().await;
    // Read before the line rather than inside it: the two counters are read one
    // after the other and this is the last thing that will ever read them, so
    // taking both at one point is worth the two locals.
    let unwritten = reins.unwritten();
    let (writes, rows) = (unwritten.writes(), unwritten.rows());
    // `error!` and not `warn!`: whatever the save task had not written is gone,
    // and this line is the only record that it was a choice rather than a crash.
    // It names the cost because the operator is the one paying it — a stop that
    // abandoned nothing and a stop that abandoned a full sweep are the same
    // keystroke and very different mornings.
    error!(
        signal = second,
        abandoned_writes = writes,
        abandoned_rows = rows,
        "asked to stop a second time; exiting without waiting for the save to finish"
    );
    std::process::exit(FORCED_EXIT_CODE);
}

/// The signals this process listens for, already installed.
///
/// Held as one value because the handlers must outlive the *first* signal: a
/// stream created fresh for the second wait would be deaf to anything delivered
/// between the two.
#[cfg(unix)]
#[derive(Debug)]
pub struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn install() -> io::Result<Self> {
        use tokio::signal::unix::{
            SignalKind,
            signal,
        };

        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    /// Wait for the next stop signal, and name it for the log.
    ///
    /// The name is `&'static str` rather than the signal number: an operator
    /// reading "SIGTERM" knows their service manager asked, and "SIGINT" knows
    /// they did.
    async fn next(&mut self) -> &'static str {
        // Both arms are `recv()`, which yields `None` only when the handler has
        // been deregistered — which tokio never does — so `pending()` on that
        // branch is "this signal will not arrive again", not a lost wakeup.
        tokio::select! {
            received = self.interrupt.recv() => match received {
                Some(()) => "SIGINT",
                None => std::future::pending().await,
            },
            received = self.terminate.recv() => match received {
                Some(()) => "SIGTERM",
                None => std::future::pending().await,
            },
        }
    }
}

/// The signals this process listens for — Ctrl-C alone, off unix.
#[cfg(not(unix))]
#[derive(Debug)]
pub struct Signals {
    /// Nothing to install: `ctrl_c()` registers on each call. The type exists so
    /// that [`watch`] and [`crate::run`] read the same on every platform.
    _private: (),
}

#[cfg(not(unix))]
impl Signals {
    fn install() -> io::Result<Self> {
        Ok(Self { _private: () })
    }

    async fn next(&mut self) -> &'static str {
        match tokio::signal::ctrl_c().await {
            Ok(()) => "Ctrl-C",
            // Nothing else can stop this process politely, so waiting forever is
            // more honest than returning and letting `watch` treat a failure as
            // a request to stop.
            Err(error) => {
                error!(%error, "cannot listen for Ctrl-C; this shard will only stop when killed");
                std::future::pending().await
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    use super::*;

    /// Sending a process a signal it has not registered a handler for kills it,
    /// and this test's process is the test harness. So the order below is the
    /// test: install first, and only then `kill`. Nothing here is a `spawn` that
    /// might not have been polled yet.
    ///
    /// The handler stays installed for the rest of the binary's run — tokio never
    /// deregisters one. That is harmless: no other test in this crate sends
    /// itself a signal, and a `SIGTERM` arriving from outside during a test run
    /// would have killed it before this test rather than after.
    #[tokio::test]
    async fn a_sigterm_asks_the_shard_to_stop() {
        let signals = install().expect("this platform lets a process handle its own signals");
        let reins = Reins::new();
        let shutdown = reins.shutdown();
        tokio::spawn(watch(signals, reins));

        let killed = std::process::Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .expect("`kill` is a POSIX utility");
        assert!(killed.success(), "the signal was sent");

        tokio::time::timeout(Duration::from_secs(5), shutdown.requested())
            .await
            .expect("SIGTERM reached the same stop an operator's Ctrl-C would");
    }
}
