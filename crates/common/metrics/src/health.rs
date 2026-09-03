//! A [`Reading`] as a health document, for a person or a launcher.
//!
//! # Why there are two renderings of one reading
//!
//! A scrape is for a time series: flat, numeric, one name per value, and
//! deliberately without the free-form work summary that would give it unbounded
//! label cardinality. This is for whoever is looking at *one* shard at *one*
//! moment — an operator with `curl`, the launcher of
//! `plans/server/operations/PLAN.md`, a dashboard's status tile — and it can
//! afford structure and a sentence.
//!
//! Both come out of the same [`Reading`], so there is no second truth to keep in
//! step: what a person reads and what a scraper reads are the same instant.
//!
//! # The one verdict, and the thresholds that are not here
//!
//! `serving` is a fact the shard knows about itself — it has been asked to stop,
//! or it has not — and it is the only judgement this crate makes. Everything
//! else is a measurement, and *how slow is too slow* is a question whose answer
//! differs per shard and per operator. That answer belongs in an alerting rule,
//! where it can be changed without a rebuild; a number compiled in here would be
//! a margin picked by eye, on a machine that was not theirs.

use serde_json::json;

use crate::shard::Reading;

/// What a reader must be told this body is.
pub const CONTENT_TYPE: &str = "application/json; charset=utf-8";

/// Render one reading as the health document.
///
/// Absent values are `null` rather than omitted: a consumer that reads
/// `tick.window` on a shard in its first second gets an answer instead of a
/// missing key, and the shape of the document does not depend on how long the
/// shard has been up.
pub fn render(reading: &Reading) -> String {
    let document = json!({
        // The one field a caller may act on without understanding the rest. The
        // keys come out sorted rather than in the order written here, which is
        // what makes two scrapes of an unchanged shard byte-identical — so this
        // is written where a reader will look for it, not where it will land.
        "serving": reading.serving(),
        "stopping": reading.stopping,
        "uptime_seconds": reading.uptime.as_secs_f64(),
        "tick": {
            "declared_per_second": reading.declared_rate,
            "total": reading.ticks,
            "age_seconds": reading.since_last_tick.map(|age| age.as_secs_f64()),
            "window": reading.window.as_ref().map(|window| {
                json!({
                    "observed_per_second": window.observed_rate,
                    "busy_ratio": window.busy_share,
                    "worst_seconds": window.worst.as_secs_f64(),
                    // The one place this reaches. It is what turns "the worst
                    // tick took 200ms" into something to act on, and it is
                    // rendered rather than spliced because a command summary
                    // that happened to contain a quote would otherwise produce a
                    // body no parser accepts.
                    "worst_work": window.worst_work,
                    "behind_ticks": window.behind_ticks,
                })
            }),
        },
        "connections": reading.connections,
        "saves": {
            "completed": reading.saves_completed,
            "failed": reading.saves_failed,
            "backlog": {
                "writes": reading.backlog.writes,
                "rows": reading.backlog.rows,
            },
        },
    });
    document.to_string()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::render;
    use crate::shard::{
        ShardMetrics,
        TickInterval,
        TickWindow,
    };

    const DECLARED: TickInterval = TickInterval(Duration::from_millis(25));

    fn parsed(body: &str) -> serde_json::Value {
        serde_json::from_str(body)
            .unwrap_or_else(|error| panic!("the health body is not JSON: {error}\n{body}"))
    }

    #[test]
    fn a_shard_that_has_not_ticked_answers_with_nulls_rather_than_a_different_shape() {
        // A consumer written against a running shard must not have to handle a
        // missing key on a starting one.
        let document = parsed(&render(&ShardMetrics::declaring(DECLARED).read()));

        assert_eq!(document["serving"], serde_json::Value::Bool(true));
        assert!(document["tick"]["window"].is_null(), "no window has closed");
        assert!(document["tick"]["age_seconds"].is_null(), "no tick has run");
        assert_eq!(document["tick"]["total"], 0);
    }

    #[test]
    fn the_work_summary_survives_the_rendering_intact() {
        // The one field a scrape may not carry, and the reason a person reads
        // this document rather than the scrape: "the worst tick took 200ms" is
        // not actionable, and "200ms, and it was a craft catalogue" is.
        let metrics = ShardMetrics::declaring(DECLARED);
        metrics.tick_window(TickWindow {
            observed_rate: 10.0,
            busy_share:    0.25,
            worst:         Duration::from_millis(200),
            worst_work:    r#"2 command(s): OpenCraftCatalogue, Say "hello""#.to_owned(),
            behind_ticks:  30,
        });

        let body = render(&metrics.read());
        let document = parsed(&body);
        assert_eq!(
            document["tick"]["window"]["worst_work"],
            serde_json::Value::String(r#"2 command(s): OpenCraftCatalogue, Say "hello""#.to_owned()),
            "a quote in a work summary broke the document: {body}"
        );
        assert_eq!(document["tick"]["window"]["behind_ticks"], 30);
    }

    #[test]
    fn a_stopping_shard_says_it_is_not_serving() {
        // What a launcher deciding where to send a player has to hear before it
        // sends one, rather than after the connection is refused.
        let metrics = ShardMetrics::declaring(DECLARED);
        metrics.stopping();

        let document = parsed(&render(&metrics.read()));
        assert_eq!(document["serving"], serde_json::Value::Bool(false));
        assert_eq!(document["stopping"], serde_json::Value::Bool(true));
    }
}
