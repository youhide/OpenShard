//! Does a shard, on its own, keep the tick rate every duration it announces is
//! denominated in?
//!
//! # The question this settles
//!
//! `crates/server/server/src/pace.rs` measures the shard's tick against the rate
//! it publishes, and says so on the edges. What it cannot say is *whose* load
//! made it late, because it only ever sees one shard. This runs the same
//! `run_shard` with **nothing else in the process** — no window, no renderer, no
//! client — so the answer is the floor: whatever a shard costs when it is the
//! only thing running.
//!
//! Read it against the playground, which runs this identical shard beside a
//! renderer in one process:
//!
//! * behind **here too** — the tick loop itself cannot hold the rate, and no
//!   amount of rearranging the client will fix it;
//! * keeping the rate here and behind in the playground — the tick is ready on
//!   time and is not being run, and the defect is the process, not the shard.
//!
//! ```sh
//! cargo run -p openshard-e2e-shard --example tick_pace                 # a bare world
//! cargo run -p openshard-e2e-shard --example tick_pace -- openshard.toml
//! ```
//!
//! The second form is the one that isolates the *world*: it runs the operator's
//! own config — their client files, their map, the navigation graph that comes
//! with it — and so covers everything the playground loads except the renderer.
//! **Its database is redirected to a throwaway file**, because the point is to
//! measure a shard and not to touch a world somebody is standing in; a shard
//! already running against that database would refuse this one anyway.
//!
//! Built in `dev` on purpose, because `dev` is what
//! `cargo run -p openshard-playground` builds and the whole question is about
//! that build. Pass `--release` to ask the other question.

use std::time::{
    Duration,
    Instant,
};

use openshard_config::Config;
use openshard_e2e_shard::{
    in_process,
    stock_config,
};

/// How long to watch. `pace` closes a window every second's worth of ticks and
/// speaks only on an edge, so this has to be long enough for several windows —
/// otherwise a run that settles after its first second reports the startup.
const WATCH: Duration = Duration::from_secs(10);

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("openshard_server=info")),
        )
        .init();

    let named = std::env::args().nth(1);
    match &named {
        Some(path) => println!("watching a shard on {path} for {}s\n", WATCH.as_secs()),
        None => println!("watching an otherwise-empty shard for {}s\n", WATCH.as_secs()),
    }
    let started = Instant::now();
    // The in-process gate rather than a port: nothing here dials the shard, and
    // a listener would be one more thing between the tick and the answer.
    let (_dial, running) = match named {
        None => in_process::spawn(stock_config, Vec::new()),
        Some(path) => {
            let config = disposable_copy_of(&path);
            in_process::spawn(move |_| config, Vec::new())
        }
    };
    std::thread::sleep(WATCH);
    drop(running);
    println!(
        "\nwatched for {:.1}s. A run that printed nothing but \"keeping its declared rate\" is a \
         shard whose own loop is sound.",
        started.elapsed().as_secs_f32()
    );
}

/// The operator's own config, running on a **copy** of the operator's own world.
///
/// Everything is left exactly as written — the client files, the facet, the
/// gameplay table — because those are the load being measured. The database is
/// copied rather than merely redirected, and that distinction is the whole
/// value of this example: an empty database is an empty world, and an empty
/// world is not what a shard is slow on. This one measured 0.18ms a tick on a
/// fresh file and 150ms a tick on the same config's real one, whose difference
/// is 26,477 decorations, 1,428 spawn regions and everything they hold.
///
/// A copy and not the file itself because the operator may be standing in that
/// world: opening it twice is either a lock or a second writer, and neither is
/// a measurement.
fn disposable_copy_of(path: &str) -> Config {
    // The crate's own loader rather than a second `toml::from_str` here: that one
    // runs `Config::validate`, and a measurement taken on a config the shard
    // would have refused is a measurement of nothing.
    let mut config = openshard_e2e_shard::operator_config(path)
        .expect("the config file parses and validates")
        .expect("the config file names a file that exists");
    let copy = std::env::temp_dir().join(format!("openshard-tick-pace-{}.db", std::process::id()));
    let named = std::path::Path::new(&config.persistence.database);
    // A `postgres://` URL is not a path and there is nothing to copy; an absent
    // file is a world that has never been saved. Both are honest empty worlds,
    // and both are reported rather than silently measured as if they were the
    // operator's.
    match named.is_file() {
        true => {
            std::fs::copy(named, &copy).expect("the world's database can be copied");
            println!("measuring a copy of {}", config.persistence.database);
        }
        false => {
            println!(
                "note: {} is not a file on disk, so this is an EMPTY world — the numbers below are a \
             floor and not this operator's load",
                config.persistence.database
            )
        }
    }
    config.persistence.database = copy.to_string_lossy().into_owned();
    config
}
