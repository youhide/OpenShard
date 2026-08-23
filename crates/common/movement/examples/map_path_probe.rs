//! Spatial pathfinding probe over a real UO map.
//!
//! ```text
//! cargo run --release -p openshard-movement --example map_path_probe -- \
//!   --client "/path/to/Ultima Online Classic" --x 1363 --y 1600 --radius 96
//! ```
//!
//! The probe deliberately measures individual destinations.  Averages hide
//! the useful answer here: a wall, shoreline, or narrow doorway can make one
//! destination much more expensive than its neighbours — so what it reports is
//! a distribution per *route class*, and the classes are the two facts that
//! decide the cost: how the search stopped ([`SearchExit`]) and how far it was
//! asked to go.
//!
//! It reports node counts beside the milliseconds, because the node budgets are
//! the thing the numbers are for — 400 for server AI, 600 for a client plan,
//! neither of them ever measured against ground.
//!
//! One search answers both questions a caller can ask of it: `find_path`'s
//! *is there a way* and `find_path_toward`'s *how far does the way go*. This
//! reads both off one [`search_path`] rather than searching twice.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use openshard_map::overlay::{Doors, Overlay};
use openshard_movement::{Footing, MapTerrain, PathSearch, SearchExit, search_path};
use openshard_protocol::world::Point;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(long, env = "OPENSHARD_CLIENT")]
    client: PathBuf,
    #[arg(long, default_value_t = 1363)]
    x: u16,
    #[arg(long, default_value_t = 1600)]
    y: u16,
    #[arg(long, default_value_t = 96)]
    radius: u16,
    /// Node budget; repeat to compare several in one sweep.
    ///
    /// The defaults are the two the shard actually ships: server AI plans at
    /// 400 and a client plan at 600.
    #[arg(long, value_name = "N")]
    budget: Vec<usize>,
    /// Times to repeat each search, keeping the fastest.
    ///
    /// A shared workstation drifts: the same sweep measured 40.6 s and 65.5 s
    /// of total wall clock on consecutive runs while every node count stayed
    /// bit-identical. The minimum of a few runs is the one reading that is
    /// about the search rather than about what else the machine was doing.
    #[arg(long, default_value_t = 3, value_name = "N")]
    repeat: usize,
}

#[derive(Clone, Copy)]
struct Reading {
    elapsed: Duration,
    x: u16,
    y: u16,
    /// Chebyshev tiles from the origin — the measure the search steers by.
    distance: u32,
    explored: usize,
    route_steps: usize,
    arrived: bool,
    /// Whether the same search has an *approach* to offer where it has no
    /// route: `find_path_toward`'s answer, read off `find_path`'s search.
    approaches: bool,
    exit: SearchExit,
}

impl Reading {
    fn new(x: u16, y: u16, distance: u32, elapsed: Duration, search: &PathSearch) -> Self {
        Self {
            elapsed,
            x,
            y,
            distance,
            explored: search.explored,
            route_steps: search.route.len(),
            arrived: search.arrived,
            approaches: search.arrived || !search.route.is_empty(),
            exit: search.exit,
        }
    }

    /// The class this destination falls in.
    ///
    /// Exit first, because it is what a bigger budget could or could not have
    /// changed; then the distance band, which is what the coarse router's own
    /// threshold (8 tiles) and one navigation region (32) cut the world into.
    fn class(self) -> &'static str {
        let band = match self.distance {
            0..=8 => "near",
            9..=32 => "region",
            _ => "far",
        };
        match (self.exit, band) {
            (SearchExit::Goal, "near") => "goal/near",
            (SearchExit::Goal, "region") => "goal/region",
            (SearchExit::Goal, _) => "goal/far",
            (SearchExit::Exhausted, "near") => "exhausted/near",
            (SearchExit::Exhausted, "region") => "exhausted/region",
            (SearchExit::Exhausted, _) => "exhausted/far",
            (SearchExit::Budget, "near") => "budget/near",
            (SearchExit::Budget, "region") => "budget/region",
            (SearchExit::Budget, _) => "budget/far",
            (SearchExit::Deadline, _) => "deadline",
        }
    }
}

/// Every class the table can print, in the order it prints them.
const CLASSES: [&str; 10] = [
    "goal/near",
    "goal/region",
    "goal/far",
    "exhausted/near",
    "exhausted/region",
    "exhausted/far",
    "budget/near",
    "budget/region",
    "budget/far",
    "deadline",
];

/// Percentile by nearest-rank, which is what a probe wants: every reported
/// number is a measurement that actually happened rather than an interpolation
/// between two that did.
fn percentile<T: Copy + Ord>(sorted: &[T], percentile: usize) -> T {
    let index = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted[index]
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// One class's distribution, printed as a row.
fn report_class(label: &str, readings: &[Reading]) {
    if readings.is_empty() {
        return;
    }
    let mut elapsed = readings.iter().map(|r| r.elapsed).collect::<Vec<_>>();
    let mut explored = readings.iter().map(|r| r.explored).collect::<Vec<_>>();
    let mut steps = readings.iter().map(|r| r.route_steps).collect::<Vec<_>>();
    elapsed.sort_unstable();
    explored.sort_unstable();
    steps.sort_unstable();
    let approached = readings.iter().filter(|r| r.approaches && !r.arrived).count();
    println!(
        "  {label:<17} n={:<6} ms p50={:7.3} p95={:7.3} worst={:8.3}  nodes p50={:<5} p95={:<5} worst={:<5}  steps p50={:<4} approach={approached}",
        readings.len(),
        ms(percentile(&elapsed, 50)),
        ms(percentile(&elapsed, 95)),
        ms(*elapsed.last().expect("the class is non-empty")),
        percentile(&explored, 50),
        percentile(&explored, 95),
        explored.last().expect("the class is non-empty"),
        percentile(&steps, 50),
    );
}

fn report(title: &str, readings: &[Reading]) {
    let mut elapsed = readings.iter().map(|r| r.elapsed).collect::<Vec<_>>();
    elapsed.sort_unstable();
    let total: Duration = elapsed.iter().sum();
    let arrived = readings.iter().filter(|r| r.arrived).count();
    let approached = readings.iter().filter(|r| r.approaches).count();
    println!(
        "{title}: n={} arrived={arrived} approach={approached} best_total_ms={:.1} p50={:.3} p95={:.3} worst={:.3}",
        readings.len(),
        ms(total),
        ms(percentile(&elapsed, 50)),
        ms(percentile(&elapsed, 95)),
        ms(*elapsed.last().expect("the sweep is non-empty")),
    );
    for class in CLASSES {
        let of_class = readings
            .iter()
            .copied()
            .filter(|reading| reading.class() == class)
            .collect::<Vec<_>>();
        report_class(class, &of_class);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cli = Cli::parse();
    if cli.budget.is_empty() {
        cli.budget = vec![400, 600];
    }
    let tiles = openshard_uofiles::tiledata::load_tiles(cli.client.join("tiledata.mul"))?;
    let map = openshard_uofiles::map::read_facet(&cli.client, 0)?;
    let terrain = MapTerrain::new(&map, &tiles);
    // The map and nothing over it. This probe measures the *ground*: a shard's
    // doors and crates are its own, and a facet's numbers have to be about the
    // facet to be comparable between runs.
    let nothing_placed = Overlay::default();
    let footing = Footing::new(Some(terrain), &nothing_placed, Doors::AsTheyStand);
    let standing = Point::new(
        cli.x,
        cli.y,
        terrain
            .predict_z(cli.x, cli.y, 0)
            .clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8,
    );
    // An origin nothing stands on answers every destination in a microsecond
    // with no route, and the sweep reports a tidy `arrived=0` rather than a
    // mistake. (1300, 1624) on Felucca is one such spot, and it took a whole
    // table of zeroes to notice.
    let from = terrain
        .can_step(standing, standing)
        .ok_or_else(|| format!("nothing stands at ({}, {})", cli.x, cli.y))?;

    let min_x = cli.x.saturating_sub(cli.radius);
    let min_y = cli.y.saturating_sub(cli.radius);
    let max_x = u32::from(cli.x.saturating_add(cli.radius)).min(map.width().saturating_sub(1)) as u16;
    let max_y = u32::from(cli.y.saturating_add(cli.radius)).min(map.height().saturating_sub(1)) as u16;
    let destinations = (min_y..=max_y)
        .flat_map(|y| (min_x..=max_x).map(move |x| (x, y)))
        .filter(|&(x, y)| (x, y) != (cli.x, cli.y))
        .map(|(x, y)| {
            let distance = u32::from(x.abs_diff(cli.x)).max(u32::from(y.abs_diff(cli.y)));
            (x, y, distance)
        })
        .collect::<Vec<_>>();

    println!(
        "map=Felucca from=({}, {}, {}) radius={} destinations={} budgets={:?}",
        from.x,
        from.y,
        from.z,
        cli.radius,
        destinations.len(),
        cli.budget,
    );

    println!("each search repeated {} times, fastest kept", cli.repeat,);
    for &budget in &cli.budget {
        let mut bare = Vec::with_capacity(destinations.len());
        for &(x, y, distance) in &destinations {
            let to = Point::new(x, y, from.z);
            let mut fastest = Duration::MAX;
            let mut last = None;
            for _ in 0..cli.repeat.max(1) {
                let started = Instant::now();
                let search = search_path(&footing, from, to, budget);
                fastest = fastest.min(started.elapsed());
                last = Some(search);
            }
            bare.push(Reading::new(
                x,
                y,
                distance,
                fastest,
                &last.expect("at least one repeat"),
            ));
        }

        report(&format!("budget={budget} bare"), &bare);
        bare.sort_unstable_by_key(|reading| std::cmp::Reverse(reading.elapsed));
        println!("  slowest destinations:");
        for reading in bare.iter().take(10) {
            println!(
                "    ({:4}, {:4}) d={:<3} {:8.3} ms nodes={:<5} steps={:<4} exit={:?} arrived={}",
                reading.x,
                reading.y,
                reading.distance,
                ms(reading.elapsed),
                reading.explored,
                reading.route_steps,
                reading.exit,
                reading.arrived,
            );
        }
    }
    Ok(())
}
