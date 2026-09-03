//! A [`Reading`] in the Prometheus text exposition format.
//!
//! # Why this is written out by hand
//!
//! The format is a line per sample with two comment lines above each family, and
//! this shard publishes thirteen of them. A client library would bring a
//! registry keyed by strings, a macro layer, and a dependency tree, to render
//! text that is shorter than the code configuring it — and it would move the set
//! of metrics out of the type system, where a family that stopped being
//! published would stop appearing rather than stop compiling.
//!
//! What that costs is that the format's rules are this module's to keep. They
//! are: a counter's name ends in `_total` and never decreases, a gauge is any
//! number, durations are seconds and nothing else, and `NaN`/`+Inf`/`-Inf` are
//! spelled the way the parser spells them rather than the way Rust does.
//!
//! # A family with nothing to say is absent, not zero
//!
//! A shard in its first second has closed no pace window, so it has no observed
//! rate. Publishing `0` there would be indistinguishable from a shard that had
//! genuinely stopped, and a scraper cannot ask which it was. An absent series is
//! the format's own answer to a value that does not exist yet.

use std::fmt::Write as _;

use crate::shard::Reading;

/// What a scraper must be told this body is.
///
/// The version is part of it: a scraper reads the content type to decide
/// between the text format and OpenMetrics, and a body labelled only
/// `text/plain` is guessed at.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Render everything a shard publishes, as one scrape.
pub fn render(reading: &Reading) -> String {
    let mut out = String::new();

    gauge(
        &mut out,
        "openshard_uptime_seconds",
        "How long this shard has been running.",
        reading.uptime.as_secs_f64(),
    );
    counter(
        &mut out,
        "openshard_ticks_total",
        "Ticks the world has run since this shard started.",
        reading.ticks,
    );
    gauge(
        &mut out,
        "openshard_tick_rate_declared_per_second",
        "The tick rate this shard publishes, and that every duration it puts on the wire is \
         denominated in.",
        f64::from(reading.declared_rate),
    );

    // The liveness signal. Absent before the first tick, because "the last tick
    // was zero seconds ago" and "there has been no tick" are opposite facts.
    if let Some(age) = reading.since_last_tick {
        gauge(
            &mut out,
            "openshard_tick_age_seconds",
            "How long ago the last tick finished. A tick loop that has wedged prints nothing in \
             the log; this is where it shows.",
            age.as_secs_f64(),
        );
    }

    // The pace window, all four numbers or none of them: they describe one
    // second, and a busy share from one second beside a worst tick from another
    // would be a sentence about no second at all.
    if let Some(window) = &reading.window {
        gauge(
            &mut out,
            "openshard_tick_rate_observed_per_second",
            "The tick rate the last closed window actually delivered. Below the declared rate \
             means every interval this shard announces is wrong by that ratio.",
            f64::from(window.observed_rate),
        );
        gauge(
            &mut out,
            "openshard_tick_busy_ratio",
            "The share of the last window spent inside the tick body. Near one is a tick too \
             slow to finish; near zero is a tick that was ready and was not run.",
            f64::from(window.busy_share),
        );
        gauge(
            &mut out,
            "openshard_tick_worst_seconds",
            "The longest single tick in the last window, which is what an average hides.",
            window.worst.as_secs_f64(),
        );
        gauge(
            &mut out,
            "openshard_tick_behind_ticks",
            "Whole ticks of time the last window lost against its budget. Zero for a shard \
             keeping its declared rate.",
            f64::from(window.behind_ticks),
        );
    }

    gauge(
        &mut out,
        "openshard_connections",
        "Client connections the shard is holding.",
        reading.connections as f64,
    );
    counter(
        &mut out,
        "openshard_saves_completed_total",
        "Snapshots the store has answered for successfully.",
        reading.saves_completed,
    );
    counter(
        &mut out,
        "openshard_saves_failed_total",
        "Snapshots the store refused. Each one costs a full sweep on the next save.",
        reading.saves_failed,
    );
    gauge(
        &mut out,
        "openshard_save_backlog_writes",
        "Snapshots handed to the save task and not yet written. What a force-exit would cost.",
        reading.backlog.writes as f64,
    );
    gauge(
        &mut out,
        "openshard_save_backlog_rows",
        "Rows inside those snapshots.",
        reading.backlog.rows as f64,
    );
    gauge(
        &mut out,
        "openshard_stopping",
        "1 once a stop has been asked for. The shard is still saving; it is no longer taking play.",
        u8::from(reading.stopping).into(),
    );

    out
}

/// One gauge family: a value that may move in either direction.
fn gauge(out: &mut String, name: &str, help: &str, value: f64) {
    family(out, name, help, "gauge", &number(value));
}

/// One counter family: a value that only ever grows, and whose name says so.
fn counter(out: &mut String, name: &str, help: &str, value: u64) {
    family(out, name, help, "counter", &value.to_string());
}

/// The three lines every family is made of.
///
/// `help` reaches the file verbatim, which is safe for exactly one reason: every
/// caller above passes a literal. A `\n` in one would end the comment early and
/// leave the rest of the sentence looking like a sample name, so this must never
/// be given operator text.
fn family(out: &mut String, name: &str, help: &str, kind: &str, value: &str) {
    // Writing to a `String` cannot fail — `fmt::Write` returns a `Result`
    // because a formatter over a socket can, and this one is over memory.
    writeln!(out, "# HELP {name} {help}").expect("writing to a String cannot fail");
    writeln!(out, "# TYPE {name} {kind}").expect("writing to a String cannot fail");
    writeln!(out, "{name} {value}").expect("writing to a String cannot fail");
}

/// A float the way the exposition format spells it.
///
/// Rust writes `inf` and `-inf`; the parser on the other end reads `+Inf` and
/// `-Inf` and rejects the rest of the scrape when it finds anything else. None
/// of this shard's numbers should reach either — the pace window guards its own
/// division — so this is the difference between one impossible value and a whole
/// scrape lost to it.
fn number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return match value.is_sign_positive() {
            true => "+Inf".to_owned(),
            false => "-Inf".to_owned(),
        };
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        number,
        render,
    };
    use crate::shard::{
        SaveBacklog,
        ShardMetrics,
        TickInterval,
        TickWindow,
    };

    const DECLARED: TickInterval = TickInterval(Duration::from_millis(25));

    /// Every sample line in a scrape, as `name value` pairs.
    fn samples(body: &str) -> Vec<(&str, &str)> {
        body.lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| line.split_once(' '))
            .collect()
    }

    #[test]
    fn a_shard_in_its_first_second_publishes_no_rate_it_has_not_measured() {
        // The reason the window is an `Option` all the way through. A `0` here
        // reads exactly like a stopped shard, and a scraper has no way to ask
        // which one it got.
        let body = render(&ShardMetrics::declaring(DECLARED).read());
        let names: Vec<&str> = samples(&body).into_iter().map(|(name, _)| name).collect();

        assert!(
            !names.contains(&"openshard_tick_rate_observed_per_second"),
            "an unmeasured rate was published anyway: {body}"
        );
        assert!(
            !names.contains(&"openshard_tick_age_seconds"),
            "a shard with no tick behind it claimed one: {body}"
        );
        assert!(
            names.contains(&"openshard_tick_rate_declared_per_second"),
            "the declared rate is known before any tick and belongs in every scrape: {body}"
        );
    }

    #[test]
    fn every_sample_is_a_family_with_a_help_and_a_type_above_it() {
        // The format's own rule, and the one a hand-written renderer breaks by
        // adding a sample and forgetting its two comment lines.
        let metrics = ShardMetrics::declaring(DECLARED);
        metrics.tick_ran();
        metrics.tick_window(TickWindow {
            observed_rate: 10.0,
            busy_share:    0.25,
            worst:         Duration::from_millis(200),
            worst_work:    "1 command(s): OpenCraftCatalogue".to_owned(),
            behind_ticks:  30,
        });
        let body = render(&metrics.read());

        for (name, _) in samples(&body) {
            assert!(
                body.contains(&format!("# HELP {name} ")),
                "{name} has no HELP line"
            );
            assert!(
                body.contains(&format!("# TYPE {name} ")),
                "{name} has no TYPE line"
            );
        }
    }

    #[test]
    fn the_measured_window_reaches_the_scrape() {
        let metrics = ShardMetrics::declaring(DECLARED);
        metrics.tick_window(TickWindow {
            observed_rate: 10.0,
            busy_share:    0.25,
            worst:         Duration::from_millis(200),
            worst_work:    "1 command(s): OpenCraftCatalogue".to_owned(),
            behind_ticks:  30,
        });
        metrics.save_backlog(SaveBacklog { writes: 2, rows: 6 });
        let body = render(&metrics.read());
        let samples = samples(&body);

        let sample = |name: &str| {
            samples
                .iter()
                .find(|(found, _)| *found == name)
                .map(|(_, value)| *value)
                .unwrap_or_else(|| panic!("{name} is not in the scrape: {body}"))
        };
        assert_eq!(sample("openshard_tick_rate_observed_per_second"), "10");
        assert_eq!(sample("openshard_tick_rate_declared_per_second"), "40");
        assert_eq!(sample("openshard_tick_busy_ratio"), "0.25");
        assert_eq!(sample("openshard_tick_worst_seconds"), "0.2");
        assert_eq!(sample("openshard_tick_behind_ticks"), "30");
        assert_eq!(sample("openshard_save_backlog_writes"), "2");
        assert_eq!(sample("openshard_save_backlog_rows"), "6");
    }

    #[test]
    fn the_free_form_work_summary_never_reaches_a_scrape() {
        // Unbounded label cardinality is how a monitoring system is brought down
        // by the thing meant to watch it: one series per command mix, forever.
        // The summary belongs to the health document and to the log line.
        let metrics = ShardMetrics::declaring(DECLARED);
        metrics.tick_window(TickWindow {
            observed_rate: 10.0,
            busy_share:    0.25,
            worst:         Duration::from_millis(200),
            worst_work:    "1 command(s): OpenCraftCatalogue".to_owned(),
            behind_ticks:  30,
        });

        assert!(
            !render(&metrics.read()).contains("OpenCraftCatalogue"),
            "a command name reached the exposition as a label or a sample"
        );
    }

    #[test]
    fn a_number_the_parser_cannot_read_is_spelled_the_way_it_can() {
        // Rust writes `inf`, which loses the scrape it appears in rather than
        // the one sample.
        assert_eq!(number(f64::INFINITY), "+Inf");
        assert_eq!(number(f64::NEG_INFINITY), "-Inf");
        assert_eq!(number(f64::NAN), "NaN");
        assert_eq!(number(0.25), "0.25");
    }
}
