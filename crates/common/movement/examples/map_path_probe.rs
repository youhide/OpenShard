//! Spatial pathfinding probe over a real UO map.
//!
//! ```text
//! cargo run --release -p openshard-movement --example map_path_probe -- \
//!   --client "/path/to/Ultima Online Classic" --x 1363 --y 1600 --radius 96
//! ```
//!
//! The probe deliberately measures individual destinations.  Averages hide
//! the useful answer here: a wall, shoreline, or narrow doorway can make one
//! destination much more expensive than its neighbours.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use openshard_movement::{MapTerrain, find_path, find_path_toward};
use openshard_protocol::world::Point;
use openshard_uofiles::tiledata::TileData;

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
    #[arg(long, default_value_t = 600)]
    budget: usize,
}

#[derive(Clone, Copy)]
struct Reading {
    elapsed: Duration,
    x: u16,
    y: u16,
    route_steps: usize,
    toward: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let tiles = TileData::load(cli.client.join("tiledata.mul"))?;
    let map = openshard_uofiles::map::read_facet(&cli.client, 0)?;
    let terrain = MapTerrain::new(&map, &tiles);
    let from = Point::new(
        cli.x,
        cli.y,
        terrain
            .predict_z(cli.x, cli.y, 0)
            .clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8,
    );

    let mut readings = Vec::new();
    let mut total = Duration::ZERO;
    let mut slowest = Duration::ZERO;
    let mut reached = 0usize;
    let min_x = cli.x.saturating_sub(cli.radius);
    let min_y = cli.y.saturating_sub(cli.radius);
    let max_x = u32::from(cli.x.saturating_add(cli.radius)).min(map.width().saturating_sub(1)) as u16;
    let max_y = u32::from(cli.y.saturating_add(cli.radius)).min(map.height().saturating_sub(1)) as u16;

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if x == cli.x && y == cli.y {
                continue;
            }
            let to = Point::new(x, y, from.z);
            let started = Instant::now();
            let route = find_path(&terrain, from, to, cli.budget);
            let elapsed = started.elapsed();
            let route_steps = route.as_ref().map_or(0, Vec::len);
            reached += usize::from(route.is_some());
            total += elapsed;
            slowest = slowest.max(elapsed);
            readings.push(Reading {
                elapsed,
                x,
                y,
                route_steps,
                toward: false,
            });
        }
    }
    readings.sort_unstable_by_key(|reading| std::cmp::Reverse(reading.elapsed));

    let count = readings.len();
    println!(
        "map=Felucca from=({}, {}, {}) radius={} budget={} destinations={} reached={} total_ms={:.3} mean_ms={:.3} max_ms={:.3}",
        from.x,
        from.y,
        from.z,
        cli.radius,
        cli.budget,
        count,
        reached,
        total.as_secs_f64() * 1000.0,
        total.as_secs_f64() * 1000.0 / count as f64,
        slowest.as_secs_f64() * 1000.0,
    );
    println!("slowest destinations:");
    for reading in readings.iter().take(20) {
        println!(
            "  ({:4}, {:4}) {:8.3} ms route_steps={} answer=find_path",
            reading.x,
            reading.y,
            reading.elapsed.as_secs_f64() * 1000.0,
            reading.route_steps,
        );
    }

    // Re-run the worst destinations with the same fallback question used by
    // click-to-walk when no complete route exists.
    let mut fallback = readings.iter().take(20).copied().collect::<Vec<_>>();
    for reading in &mut fallback {
        let started = Instant::now();
        let route = find_path_toward(
            &terrain,
            from,
            Point::new(reading.x, reading.y, from.z),
            cli.budget,
        );
        reading.elapsed = started.elapsed();
        reading.route_steps = route.as_ref().map_or(0, Vec::len);
        reading.toward = true;
    }
    fallback.sort_unstable_by_key(|reading| std::cmp::Reverse(reading.elapsed));
    println!("slowest fallback destinations:");
    for reading in fallback.iter().take(10) {
        println!(
            "  ({:4}, {:4}) {:8.3} ms route_steps={} answer=find_path_toward",
            reading.x,
            reading.y,
            reading.elapsed.as_secs_f64() * 1000.0,
            reading.route_steps,
        );
    }
    Ok(())
}
