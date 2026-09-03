//! One place that decides what a shard's log looks like.
//!
//! # Why this is a function and not four copies of five lines
//!
//! It was four copies. Every binary that runs a shard — the server, the
//! playground, an example that drives the tick — built its own subscriber, and
//! they had already drifted: the same `RUST_LOG` produced different output
//! depending on which one an operator happened to start, and nothing said so.
//!
//! It is also where the next decision about logging lands. `docs/architecture.md`
//! wants structured output eventually — JSON lines for a collector, spans a
//! trace viewer can read — and that is one change here rather than one change per
//! binary, made in the place whose name says it owns the question.

use tracing_subscriber::EnvFilter;

/// Turn on logging for this process.
///
/// `RUST_LOG` wins when it is set and parses; `default_filter` is what a shard
/// started with no environment at all logs at.
///
/// # Why an unreadable `RUST_LOG` falls back rather than fails
///
/// An operator who typed a filter wrong wants their shard, not a refusal — and
/// the alternative to falling back is a process that will not start for a reason
/// that has nothing to do with running a shard. The cost is that a typo is
/// silently the default, which is why it is said out loud below.
///
/// # Panics
///
/// If a subscriber is already installed for this process, or if `default_filter`
/// is not a filter. Both are the caller's mistake and both are made at the top of
/// `main`, where the panic is a message on the terminal before anything has
/// happened rather than a failure in the middle of a run.
pub fn install(default_filter: &str) {
    let (filter, typo) = match std::env::var("RUST_LOG") {
        Err(_absent) => (EnvFilter::new(default_filter), None),
        Ok(wanted) => {
            match EnvFilter::try_new(&wanted) {
                Ok(filter) => (filter, None),
                Err(error) => (EnvFilter::new(default_filter), Some((wanted, error))),
            }
        }
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();
    // After `init`, because saying it before there is a subscriber says it to
    // nothing at all.
    if let Some((wanted, error)) = typo {
        tracing::warn!(
            %error,
            RUST_LOG = wanted,
            "RUST_LOG is not a filter this can read; logging at {default_filter} instead"
        );
    }
}
