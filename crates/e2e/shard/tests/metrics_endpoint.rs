//! What a running shard publishes about itself, read the way a scraper reads it.
//!
//! # Why this is an end-to-end test and not a unit one
//!
//! `openshard-metrics` has tests for every piece: the registry keeps what is
//! published, the exposition renders it, the endpoint answers HTTP. All of them
//! feed the registry by hand, and all of them would go on passing if
//! `run_shard` stopped publishing into it tomorrow — the numbers this is about
//! are produced by a loop driving a clock, and nothing that stubs that loop can
//! prove they are real.
//!
//! So this starts a shard, waits for it to close a real pace window, and scrapes
//! it over a socket. What it asserts is the one claim
//! `plans/server/operations/PLAN.md` § 1 was written about: the two things the
//! shard already measured — the tick-rate window and the save backlog — now have
//! somewhere to be published, and what comes back is the running shard's own
//! numbers rather than a fixture that looks like them.

use std::net::SocketAddr;
use std::time::Duration;

use openshard_e2e_shard::shard;
use openshard_metrics::endpoint::MetricsEndpoint;
use openshard_metrics::shard::ShardMetrics;
use tokio::io::{
    AsyncReadExt,
    AsyncWriteExt,
};
use tokio::net::TcpStream;

/// How long a shard is given to close its first pace window.
///
/// A window is a second's worth of ticks, so the wait is about a second on any
/// machine that is running at all. This is the bound on a machine that is not,
/// and it is deliberately far past it: what a tighter one would buy is a test
/// that fails on a loaded CI box, which is a shard being slow rather than a
/// shard being wrong.
const PATIENCE: Duration = Duration::from_secs(30);

/// Poll until the shard has measured a window, or give up.
async fn first_window(metrics: &ShardMetrics) {
    let deadline = std::time::Instant::now() + PATIENCE;
    while metrics.read().window.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "the shard ran for {PATIENCE:?} without closing a single tick-pace window"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// One GET over one real socket, head and body.
async fn get(address: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(address)
        .await
        .expect("the endpoint is listening");
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n").as_bytes())
        .await
        .expect("the request was sent");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("the response was read to the hang-up");
    response
}

/// One sample's value out of a scrape.
fn sample<'a>(body: &'a str, name: &str) -> &'a str {
    body.lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(' '))
        .unwrap_or_else(|| panic!("{name} is not in the scrape:\n{body}"))
}

#[tokio::test]
async fn a_running_shard_publishes_the_numbers_it_was_already_measuring() {
    let (_address, running) = shard();
    let metrics = running.metrics();

    // The endpoint the binary would have bound from `[metrics] listen`, bound
    // here on a loopback port the test can name. It reads the same handle
    // `openshard_server::run` hands its own, so this is the running shard.
    let endpoint = MetricsEndpoint::bind(SocketAddr::from(([127, 0, 0, 1], 0)), metrics.clone())
        .await
        .expect("a loopback port");
    let address = endpoint.local_address().expect("the bound address");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(endpoint.serve(async {
        let _ = stopped.await;
    }));

    first_window(&metrics).await;

    let scrape = get(address, "/metrics").await;
    assert!(scrape.starts_with("HTTP/1.1 200 OK\r\n"), "{scrape}");

    // The tick loop is running and says so. This is the number a log cannot
    // give: a shard whose tick has wedged prints nothing at all.
    let ticks: u64 = sample(&scrape, "openshard_ticks_total")
        .parse()
        .expect("a tick count");
    assert!(ticks > 0, "a shard that closed a window ran no ticks:\n{scrape}");

    // The pair the whole watchdog is about: what the shard promises, beside what
    // it delivered. `pace.rs` had both and could only put them in a log line.
    assert_eq!(
        sample(&scrape, "openshard_tick_rate_declared_per_second"),
        "40",
        "the declared rate is not the one openshard_world::TICK_INTERVAL declares:\n{scrape}"
    );
    let observed: f64 = sample(&scrape, "openshard_tick_rate_observed_per_second")
        .parse()
        .expect("an observed rate");
    assert!(
        observed > 0.0,
        "a closed window reported no rate at all:\n{scrape}"
    );

    // The save backlog. An idle shard with no database owes the disk nothing,
    // and "nothing" is a measurement rather than a missing series.
    assert_eq!(sample(&scrape, "openshard_save_backlog_writes"), "0");
    assert_eq!(sample(&scrape, "openshard_save_backlog_rows"), "0");

    // Nobody has logged in, and the count is the sessions table rather than a
    // tally kept on the open/close edges.
    assert_eq!(sample(&scrape, "openshard_connections"), "0");

    // The same instant, for a person rather than a scraper — and this is the
    // one place the worst tick's command mix is published.
    let health = get(address, "/health").await;
    assert!(health.starts_with("HTTP/1.1 200 OK\r\n"), "{health}");
    assert!(
        health.contains("\"serving\":true"),
        "a shard nobody has stopped said it was not serving:\n{health}"
    );
    assert!(
        health.contains("\"worst_work\":"),
        "the health document dropped the worst tick's commands:\n{health}"
    );

    // A stop is what turns the 200 into a 503, and the endpoint deliberately
    // outlives the shard so that there is something to answer with while the
    // world is being saved.
    running.stop();
    let stopping = get(address, "/health").await;
    assert!(
        stopping.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
        "a stopped shard still invited play:\n{stopping}"
    );
    assert!(stopping.contains("\"serving\":false"), "{stopping}");

    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(5), serving)
        .await
        .expect("the endpoint stopped when asked")
        .expect("and it did not panic")
        .expect("and it did not fail to accept");
}
