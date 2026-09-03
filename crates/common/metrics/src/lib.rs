//! Structured logging, tracing, and Prometheus metrics.
//!
//! # What this crate is for
//!
//! A shard is watched from outside the process, and until this crate existed the
//! only thing outside the process was a log stream. A log line is an *event*: it
//! says that something changed at the moment it changed, which is exactly the
//! wrong shape for the two questions an operator actually asks — *what is it
//! doing right now*, and *what has it been doing for the last hour*. Both of
//! those are samples, and nothing in the shard published one.
//!
//! So the shard already measured the numbers and had nowhere to put them. The
//! tick-pace watchdog closes a window every second and knows the observed rate,
//! the busy share and the worst tick in it; the save task's tally knows how many
//! writes and rows the disk has been promised and not given. Both were spent on
//! a log line and thrown away. [`shard::ShardMetrics`] is where they go instead,
//! and [`endpoint::MetricsEndpoint`] is how something outside the process reads
//! them.
//!
//! # The three parts
//!
//! - [`shard`] — the live values a running shard publishes about itself, and
//!   [`shard::Reading`], the consistent snapshot everything else renders.
//! - [`exposition`] and [`health`] — two renderings of one `Reading`: the
//!   Prometheus text format for a scraper, and a JSON document for a person or a
//!   launcher.
//! - [`endpoint`] — the socket that serves both, and [`logging`] — the one place
//!   that decides what a shard's log looks like.
//!
//! # What is deliberately not here
//!
//! **No thresholds.** Nothing in this crate decides that a shard is unwell,
//! because every such decision would be a margin picked by eye — the fudge
//! constant `docs/style.md` forbids — and the place that margin genuinely
//! belongs is the operator's own alerting rules, where it can differ per shard
//! and be changed without a rebuild. What is published is measurements; the one
//! thing [`health`] states as a verdict is whether the shard is still taking
//! play, which is a fact the shard knows rather than a number compared against
//! anything.
//!
//! **No registry of arbitrary metrics.** A named struct of the values this shard
//! actually has beats a map of strings that anything may write anything into:
//! the compiler knows the whole set, the renderer cannot omit one by accident,
//! and a metric that stops being published stops compiling.

pub mod endpoint;
pub mod exposition;
pub mod health;
pub mod logging;
pub mod shard;
