//! What the coarse router costs on real ground, against flat A\* on the same
//! routes.
//!
//! ```sh
//! cargo run --release -p openshard-movement --example coarse_bench -- \
//!   --client "/path/to/Ultima Online Classic"
//! ```
//!
//! It is an example rather than an asserted test: elapsed time is a property of
//! the host, while the routed/refused counts and the node counts beside them
//! are properties of the map and hold across machines.
//!
//! **This used to be synthetic** — a 1024×1024 open world with no map in it, on
//! which the hierarchy measured *slower* than flat A\*. An open plain is the one
//! world where a coarse graph can only lose: flat A\* walks a straight line to
//! the goal and the corridor adds portals to a route that never needed one. The
//! question the shard actually asks is the opposite one, and only a real facet
//! poses it: a route long enough that flat A\* runs out of budget before it
//! arrives. `--synthetic` keeps the old run for comparison.
//!
//! Destinations are sampled off the map rather than named, so the run is
//! reproducible without lore: at each distance band, the ring around the origin
//! is walked and the first standable tiles on it are taken.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::Parser;
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::{
    CachedTerrain, MapTerrain, NavigationGraph, OpenWorld, Terrain, Tile, bake, find_long_path, find_path,
    search_path, step_allowed,
};
use openshard_protocol::direction::Direction;
use openshard_protocol::world::{Facet, Point};
use openshard_uofiles::tiledata::TileData;

/// Chebyshev distances the sampler aims at, in tiles.
///
/// The first is one navigation region across; the last is a quarter of the
/// facet. Flat A\* at a 600-node budget cannot reach past roughly 40 open tiles,
/// so every band but the first is a route only the hierarchy can answer.
const BANDS: [u32; 6] = [32, 64, 128, 256, 512, 1024];
/// Destinations sampled per band.
const PER_BAND: usize = 8;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    #[arg(long, default_value_t = 0)]
    facet: u8,
    #[arg(long, default_value_t = 1363)]
    x: u16,
    #[arg(long, default_value_t = 1600)]
    y: u16,
    /// Node budget for both the flat search and the corridor's exact hops.
    #[arg(long, default_value_t = 600)]
    budget: usize,
    /// Times to repeat each query, keeping the fastest — see `map_path_probe`
    /// for why a shared workstation makes the minimum the honest reading.
    #[arg(long, default_value_t = 3, value_name = "N")]
    repeat: usize,
    /// Also run the old open-world comparison this example used to be.
    #[arg(long, default_value_t = false)]
    synthetic: bool,
    /// Flood the facet once from the origin, so a refusal can be read.
    ///
    /// Without it every `NoCorridor` is ambiguous: an island across a bay has
    /// no land route, and the router refusing one is the right answer. With it,
    /// a refusal on a tile the flood reached is the router's own.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    component: bool,
}

/// One destination, answered both ways.
struct Reading {
    x: u16,
    y: u16,
    distance: u32,
    flat: Duration,
    flat_nodes: usize,
    flat_arrived: bool,
    coarse: Duration,
    coarse_steps: Option<usize>,
    /// Whether the flood reached this tile — `None` when it was not run.
    walkable_from_origin: Option<bool>,
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn percentile(sorted: &[Duration], percentile: usize) -> Duration {
    let index = (sorted.len() * percentile).div_ceil(100).saturating_sub(1);
    sorted[index]
}

/// A row of the distribution, or nothing when the band sampled empty.
fn spread(label: &str, mut samples: Vec<Duration>, routed: usize, of: usize) {
    if samples.is_empty() {
        return;
    }
    samples.sort_unstable();
    println!(
        "    {label:<7} routed={routed}/{of}  p50={:8.3} p95={:8.3} worst={:8.3}",
        ms(percentile(&samples, 50)),
        ms(percentile(&samples, 95)),
        ms(*samples.last().expect("the band is non-empty")),
    );
}

/// The point a body would stand on at this tile, or `None` where none does.
///
/// The same two questions [`NavigationGraph::build`] samples the facet with, in
/// the same order — so a tile this accepts is a tile the graph has a node in
/// the region of, rather than one that merely has ground under it.
fn standable(terrain: &MapTerrain<'_>, x: u16, y: u16) -> Option<Point> {
    let near = terrain.ground_z(Tile::new(x, y))?;
    let point = Point::new(x, y, near);
    terrain.can_step(point, point)
}

/// The point one step around the square ring at Chebyshev `radius`.
///
/// `index` runs `0..8 * radius`, anticlockwise from the ring's north-west
/// corner through its four sides.
fn on_ring(origin: Point, radius: u32, index: usize) -> (i64, i64) {
    let side_len = (2 * radius as usize).max(1);
    let side = index / side_len;
    let along = (index % side_len) as i64;
    let radius = i64::from(radius);
    let (dx, dy) = match side {
        0 => (-radius + along, -radius),
        1 => (radius, -radius + along),
        2 => (radius - along, radius),
        _ => (-radius, radius - along),
    };
    (i64::from(origin.x) + dx, i64::from(origin.y) + dy)
}

/// [`PER_BAND`] destinations spread around the ring at Chebyshev `radius`.
///
/// One anchor per eighth of the ring, and from each anchor the first standable
/// tile forward — so a band whose north side is all water still reports what
/// its other three sides do. Taking the first `PER_BAND` standable tiles
/// instead is what this used to do, and it sampled eight *adjacent* tiles on
/// one side of the ring: eight readings of what is very nearly one route.
fn sample_ring(terrain: &MapTerrain<'_>, origin: Point, radius: u32, width: u32, height: u32) -> Vec<Point> {
    let perimeter = (8 * radius).max(1) as usize;
    let arc = (perimeter / PER_BAND).max(1);
    let mut found = Vec::with_capacity(PER_BAND);
    for anchor in (0..perimeter).step_by(arc) {
        let standing = (anchor..anchor + arc).find_map(|index| {
            let (x, y) = on_ring(origin, radius, index % perimeter);
            if x < 0 || y < 0 || x >= i64::from(width) || y >= i64::from(height) {
                return None;
            }
            standable(terrain, x as u16, y as u16)
        });
        if let Some(point) = standing {
            found.push(point);
        }
    }
    found
}

/// Every tile the origin can walk to, flooded once over the whole facet.
///
/// The ground truth a refusal is read against. Keyed by `(x, y)` and not by
/// the whole point: a tile first reached at one height is marked reached, and a
/// second route arriving at another height is not explored again. That
/// under-counts — a gallery over a street is one entry, not two — so a tile it
/// calls reachable really is, which is the direction that matters here.
fn land_component(terrain: &MapTerrain<'_>, origin: Point, width: u32, height: u32) -> Vec<bool> {
    let mut reached = vec![false; (width as usize) * (height as usize)];
    let index = |x: u16, y: u16| (y as usize) * (width as usize) + (x as usize);
    let mut queue = std::collections::VecDeque::new();
    reached[index(origin.x, origin.y)] = true;
    queue.push_back(origin);
    while let Some(at) = queue.pop_front() {
        for direction in Direction::ALL {
            let Some(next) = step_allowed(terrain, at, direction) else {
                continue;
            };
            let slot = index(next.x, next.y);
            if reached[slot] {
                continue;
            }
            reached[slot] = true;
            queue.push_back(next);
        }
    }
    reached
}

fn synthetic() {
    const WIDTH: u16 = 1024;
    const HEIGHT: u16 = 1024;
    const FROM: Point = Point::new(1, 1, 0);
    const TO: Point = Point::new(WIDTH - 2, HEIGHT - 2, 0);
    const SAMPLES: usize = 25;

    let built_at = Instant::now();
    let router = NavigationGraph::build(&OpenWorld, u32::from(WIDTH), u32::from(HEIGHT))
        .expect("the synthetic facet fits Point's coordinate space");
    let built = built_at.elapsed();

    let flat_at = Instant::now();
    let flat = find_path(&OpenWorld, FROM, TO, usize::from(WIDTH) * usize::from(HEIGHT))
        .expect("open ground has a flat route");
    let flat_elapsed = flat_at.elapsed();

    let mut coarse_samples = Vec::with_capacity(SAMPLES);
    let mut coarse_steps = None;
    for _ in 0..SAMPLES {
        let coarse_at = Instant::now();
        let coarse = find_long_path(&OpenWorld, &OpenWorld, &router, FROM, TO, 600)
            .expect("the coarse corridor has bounded exact hops");
        coarse_steps = Some(coarse.len());
        coarse_samples.push(coarse_at.elapsed());
    }
    coarse_samples.sort_unstable();
    println!(
        "synthetic {WIDTH}x{HEIGHT}: build {built:?}; flat {} steps in {flat_elapsed:?}; coarse {} steps, p95 {:?}, worst {:?} ({SAMPLES} samples)",
        flat.len(),
        coarse_steps.expect("samples are non-empty"),
        percentile(&coarse_samples, 95),
        coarse_samples.last().expect("samples are non-empty"),
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    if cli.synthetic {
        synthetic();
    }
    let facet = Facet(cli.facet);
    let tiles = TileData::load(cli.client.join("tiledata.mul"))?;
    let map: MapSnapshot = openshard_uofiles::map::load_facet(&cli.client, facet)?;
    // The artifact the shard loads, validated the way the shard validates it: a
    // graph that no longer matches its inputs is not a slower answer, it is a
    // different world's answer.
    let stamp = bake::stamp_of(&cli.client, facet, map.revision())?;
    let graph = bake::load(&bake::artifact_path(&cli.client, facet), &stamp)?;
    let (regions, nodes, edges) = graph.counts();
    let terrain = MapTerrain::new(map.map(), &tiles);
    let (width, height) = (map.map().width(), map.map().height());

    let origin = standable(&terrain, cli.x, cli.y).ok_or("nothing stands at the origin")?;
    println!(
        "facet {facet} {width}x{height}: {regions} regions, {nodes} nodes, {edges} edges; from ({}, {}, {}) budget={} repeat={}",
        origin.x, origin.y, origin.z, cli.budget, cli.repeat,
    );

    let component = cli.component.then(|| {
        let started = Instant::now();
        let reached = land_component(&terrain, origin, width, height);
        let count = reached.iter().filter(|&&flag| flag).count();
        println!(
            "  flood: {count} tiles walkable from the origin ({:.1}% of the facet) in {:.1}s",
            100.0 * count as f64 / reached.len() as f64,
            started.elapsed().as_secs_f64(),
        );
        reached
    });

    let (mut hits, mut misses) = (0usize, 0usize);
    for band in BANDS {
        let destinations = sample_ring(&terrain, origin, band, width, height);
        let mut readings = Vec::with_capacity(destinations.len());
        for to in destinations {
            let distance = u32::from(to.x.abs_diff(origin.x)).max(u32::from(to.y.abs_diff(origin.y)));
            // Flat A* first, through the per-plan cache the client wraps it in.
            let mut flat = Duration::MAX;
            let mut flat_search = None;
            for _ in 0..cli.repeat.max(1) {
                let cache = CachedTerrain::new(&terrain);
                let started = Instant::now();
                let search = search_path(&cache, origin, to, cli.budget);
                flat = flat.min(started.elapsed());
                let stats = cache.stats();
                flat_search = Some((search, stats));
            }
            let (search, stats) = flat_search.expect("at least one repeat");
            hits += stats.hits;
            misses += stats.misses;

            // Then the corridor, asked exactly as `steer::Ground::path` asks
            // it: the bare map guides and joins, and the live reading — here a
            // cache over the same map — approves the exact steps.
            let mut coarse = Duration::MAX;
            let mut coarse_route = None;
            for _ in 0..cli.repeat.max(1) {
                let cache = CachedTerrain::new(&terrain);
                let started = Instant::now();
                let route = find_long_path(&terrain, &cache, &graph, origin, to, cli.budget);
                coarse = coarse.min(started.elapsed());
                let stats = cache.stats();
                hits += stats.hits;
                misses += stats.misses;
                coarse_route = Some(route);
            }
            readings.push(Reading {
                x: to.x,
                y: to.y,
                distance,
                flat,
                flat_nodes: search.explored,
                flat_arrived: search.arrived,
                coarse,
                coarse_steps: coarse_route.expect("at least one repeat").map(|r| r.len()),
                walkable_from_origin: component
                    .as_ref()
                    .map(|reached| reached[(to.y as usize) * (width as usize) + (to.x as usize)]),
            });
        }
        if readings.is_empty() {
            println!("  band {band}: no standable tile on the ring");
            continue;
        }
        let flat_routed = readings.iter().filter(|r| r.flat_arrived).count();
        let coarse_routed = readings.iter().filter(|r| r.coarse_steps.is_some()).count();
        let nodes: usize = readings.iter().map(|r| r.flat_nodes).sum();
        // A refusal on a tile the flood reached is the router's; on one it did
        // not reach, the refusal is the map's and is the right answer.
        let missed = readings
            .iter()
            .filter(|r| r.coarse_steps.is_none() && r.walkable_from_origin == Some(true))
            .count();
        let walkable = readings
            .iter()
            .filter(|r| r.walkable_from_origin == Some(true))
            .count();
        println!(
            "  band {band}: n={} walkable={walkable} refused_but_walkable={missed} flat_nodes_mean={}",
            readings.len(),
            nodes / readings.len(),
        );
        spread(
            "flat",
            readings.iter().map(|r| r.flat).collect(),
            flat_routed,
            readings.len(),
        );
        spread(
            "coarse",
            readings.iter().map(|r| r.coarse).collect(),
            coarse_routed,
            readings.len(),
        );
        for reading in &readings {
            println!(
                "      ({:5}, {:5}) d={:<5} walkable={:<5} flat {:8.3} ms nodes={:<5} arrived={:<5}  coarse {:8.3} ms steps={}",
                reading.x,
                reading.y,
                reading.distance,
                reading
                    .walkable_from_origin
                    .map_or_else(|| "?".to_owned(), |flag| flag.to_string()),
                ms(reading.flat),
                reading.flat_nodes,
                reading.flat_arrived,
                ms(reading.coarse),
                reading
                    .coarse_steps
                    .map_or_else(|| "-".to_owned(), |steps| steps.to_string()),
            );
        }
    }
    let calls = hits + misses;
    println!(
        "TransitionCacheStats over the whole run: hits={hits} misses={misses} hit_rate={:.1}%",
        100.0 * hits as f64 / calls.max(1) as f64,
    );
    Ok(())
}
