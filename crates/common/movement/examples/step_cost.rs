//! Where a step's time actually goes, on a real facet.
//!
//! ```sh
//! cargo run --release -p openshard-movement --example step_cost -- \
//!   --client "/path/to/Ultima Online Classic"
//! ```
//!
//! `map_path_probe` says a search costs about 0.8 ms for 601 nodes and that
//! roughly fourteen `can_step` calls go into each node. That puts the whole
//! question on one call, and this splits it: the land read, the statics lookup,
//! the surface arithmetic on the tile being stepped *onto*, and the surface
//! arithmetic on the tile being stepped *off* — which A\* asks for again on
//! every one of a node's fourteen calls, from the same tile, with the same
//! answer.
//!
//! Everything is accumulated into a checksum that is printed, so nothing here
//! can be optimised away as dead.

use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use openshard_movement::{MapTerrain, SearchExit, Terrain, Tile, search_path, step_allowed};
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;
use openshard_uofiles::tiledata::TileData;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    #[arg(long, default_value_t = 1500)]
    x: u16,
    #[arg(long, default_value_t = 1900)]
    y: u16,
    /// Half-width of the square of tiles walked, in tiles.
    #[arg(long, default_value_t = 64)]
    radius: u16,
    /// Passes over that square, fastest kept.
    #[arg(long, default_value_t = 5)]
    repeat: usize,
}

/// Time one pass over every sampled tile, keeping the fastest of `repeat`.
fn measure(repeat: usize, tiles: usize, label: &str, mut pass: impl FnMut() -> u64) {
    let mut fastest = Duration::MAX;
    let mut checksum = 0;
    for _ in 0..repeat.max(1) {
        let started = Instant::now();
        checksum = pass();
        fastest = fastest.min(started.elapsed());
    }
    println!(
        "  {label:<34} {:8.1} ns/tile   (checksum {checksum})",
        fastest.as_secs_f64() * 1e9 / tiles as f64,
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let tiledata = TileData::load(cli.client.join("tiledata.mul"))?;
    let map = openshard_uofiles::map::read_facet(&cli.client, 0)?;
    let terrain = MapTerrain::new(&map, &tiledata);

    // Only tiles a body can stand on, so every measurement below is doing the
    // work a search would do rather than bailing out on the first `None`.
    let mut standing = Vec::new();
    for y in cli.y.saturating_sub(cli.radius)..=cli.y.saturating_add(cli.radius) {
        for x in cli.x.saturating_sub(cli.radius)..=cli.x.saturating_add(cli.radius) {
            let Some(z) = terrain.ground_z(Tile::new(x, y)) else {
                continue;
            };
            let point = Point::new(x, y, z);
            if let Some(point) = terrain.can_step(point, point) {
                standing.push(point);
            }
        }
    }
    let n = standing.len();
    println!(
        "facet 0, {n} standable tiles around ({}, {}), fastest of {} passes",
        cli.x, cli.y, cli.repeat
    );

    println!("the pieces, once per tile:");
    measure(cli.repeat, n, "map.land", || {
        standing
            .iter()
            .map(|p| u64::from(map.land(p.x, p.y).is_some()))
            .sum()
    });
    measure(cli.repeat, n, "map.statics_at (count)", || {
        standing
            .iter()
            .map(|p| map.statics_at(p.x, p.y).count() as u64)
            .sum()
    });
    measure(cli.repeat, n, "terrain.ground_z", || {
        standing
            .iter()
            .map(|p| u64::from(terrain.ground_z(Tile::new(p.x, p.y)).unwrap_or(0) as u8))
            .sum()
    });
    // `surface_at` is `check` with the start surface equal to the feet, so this
    // is the *landing* half of a step and nothing else.
    measure(cli.repeat, n, "terrain.surface_at (the landing)", || {
        standing
            .iter()
            .map(|p| terrain.surface_at(p.x, p.y, i32::from(p.z)).unwrap_or(0) as u64)
            .sum()
    });
    measure(cli.repeat, n, "terrain.can_step (one neighbour)", || {
        standing
            .iter()
            .filter_map(|&p| {
                let to = step_from_east(p)?;
                terrain.can_step(p, to)
            })
            .map(|p| u64::from(p.x))
            .sum()
    });

    println!("what one node expansion costs:");
    // Exactly what `search` does per popped tile: eight directions through
    // `step_allowed`, so the four diagonals also pay for their two flanks.
    measure(cli.repeat, n, "8 x step_allowed (a whole node)", || {
        standing
            .iter()
            .map(|&p| {
                Direction::ALL
                    .into_iter()
                    .filter_map(|d| step_allowed(black_box(&terrain), black_box(p), d))
                    .map(|p| u64::from(p.x))
                    .sum::<u64>()
            })
            .sum()
    });
    // The same eight answers, with the two things a node expansion currently
    // recomputes done once: the tile stepped off, and each distinct neighbour.
    measure(cli.repeat, n, "the same, landings computed once", || {
        standing.iter().map(|&p| hoisted_expand(&terrain, p)).sum()
    });
    println!("what the search itself costs, with terrain taken away:");
    let plain = Plain { wall_x: 3000 };
    let from = Point::new(2000, 2000, 0);
    let to = Point::new(4000, 2000, 0);
    for budget in [400_usize, 600] {
        let mut fastest = Duration::MAX;
        let mut explored = 0;
        for _ in 0..cli.repeat.max(1) {
            let started = Instant::now();
            let search = search_path(black_box(&plain), black_box(from), black_box(to), budget);
            fastest = fastest.min(started.elapsed());
            explored = search.explored;
            assert!(
                matches!(search.exit, SearchExit::Budget),
                "the goal must stay walled off"
            );
        }
        println!(
            "  budget {budget}: {explored} nodes in {:8.1} ns  =>  {:6.1} ns/node of pure A*",
            fastest.as_secs_f64() * 1e9,
            fastest.as_secs_f64() * 1e9 / explored as f64,
        );
    }
    Ok(())
}

/// Open ground with one impassable half-plane, and a step that costs nothing.
///
/// The floor under every measurement above. A search over this pops its whole
/// budget — the goal is walled off, so nothing arrives — while `can_step` is one
/// integer compare, so what is left on the clock is the *search*: a binary heap,
/// two `FxHashMap`s and a closed set. Terrain work is what a span grid shrinks;
/// this is what it shrinks it **towards**, and a ratio quoted without it is a
/// ratio that ignores its own limit.
struct Plain {
    wall_x: u16,
}

impl Terrain for Plain {
    fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
        (to.x < self.wall_x).then_some(to)
    }
}

const fn step_from_east(point: Point) -> Option<Point> {
    match point.x.checked_add(1) {
        Some(x) => Some(Point { x, ..point }),
        None => None,
    }
}

/// One node expansion with the redundancy taken out, for comparison only.
///
/// Two things are hoisted, and neither changes an answer:
///
/// 1. **The tile stepped off.** `can_step` recomputes `start_surface(from)` on
///    every call, and all fourteen-odd calls in one expansion share `from`.
///    `check(x, y, start_z, start_top)` is the same rule with that half handed
///    in, so calling it directly hoists the work rather than skipping it.
/// 2. **The distinct neighbours.** The four diagonals ask about their two
///    flanking cardinals, which are four of the eight neighbours already being
///    asked about. Answering each tile once turns sixteen landing checks into
///    eight.
fn hoisted_expand(terrain: &MapTerrain<'_>, from: Point) -> u64 {
    let start_z = i32::from(from.z);
    let (_, start_top) = terrain.start_surface(from.x, from.y, start_z);
    // One landing answer per neighbour, in `Direction::ALL` order.
    let mut landing = [None; 8];
    for direction in Direction::ALL {
        let Some(to) = openshard_movement::step_from(from, direction) else {
            continue;
        };
        landing[direction.to_bits() as usize] = terrain
            .check(to.x, to.y, start_z, start_top)
            .and_then(|z| i8::try_from(z).ok())
            .map(|z| Point { x: to.x, y: to.y, z });
    }
    let mut sum = 0;
    for direction in Direction::ALL {
        let bits = direction.to_bits() as usize;
        if direction.is_diagonal() {
            let flanks = [(bits + 7) % 8, (bits + 1) % 8];
            if flanks.iter().any(|&flank| landing[flank].is_none()) {
                continue;
            }
        }
        if let Some(point) = landing[bits] {
            sum += u64::from(point.x);
        }
    }
    sum
}
