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
use std::time::{
    Duration,
    Instant,
};

use clap::Parser;
use openshard_map::grid::Tile;
use openshard_map::overlay::{
    Doors,
    Overlay,
};
use openshard_movement::reach::Reach;
use openshard_movement::{
    Footing,
    MapTerrain,
    NavigationGraph,
    Weight,
    bake,
    find_long_path,
    find_path,
    search_path,
};
use openshard_protocol::world::{
    Facet,
    Point,
};

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
    client:    PathBuf,
    #[arg(long, default_value_t = 0)]
    facet:     u8,
    #[arg(long, default_value_t = 1363)]
    x:         u16,
    #[arg(long, default_value_t = 1600)]
    y:         u16,
    /// Node budget for both the flat search and the corridor's exact hops.
    #[arg(long, default_value_t = 600)]
    budget:    usize,
    /// Times to repeat each query, keeping the fastest — see `map_path_probe`
    /// for why a shared workstation makes the minimum the honest reading.
    #[arg(long, default_value_t = 3, value_name = "N")]
    repeat:    usize,
    /// Also run the old open-world comparison this example used to be.
    #[arg(long, default_value_t = false)]
    synthetic: bool,
    /// What an *unbounded* exact search may spend, so the corridor's route has
    /// something to be long against. Zero leaves the detour unmeasured.
    ///
    /// Separate from `--budget`, which is what a *client* may spend and is
    /// therefore the number that makes the corridor necessary in the first
    /// place. This one is nobody's budget: it is the shortest route the ground
    /// holds, which is the only honest denominator for "how much further did
    /// the hierarchy send a body".
    #[arg(long, default_value_t = 200_000, value_name = "NODES")]
    exact:     usize,
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
    /// The shortest route the ground holds, when `--exact` bought one.
    exact_steps: Option<usize>,
    /// Whether the flood reached this tile — `None` when it was not run.
    walkable_from_origin: Option<bool>,
}

impl Reading {
    /// How much further the corridor sends a body than the ground requires, as
    /// a percentage — `0` for a route of the shortest length there is.
    ///
    /// `None` when either half is missing: a destination the corridor refused
    /// has no detour, and one the exact search could not reach inside `--exact`
    /// has nothing to be measured against.
    fn detour(&self) -> Option<u32> {
        let (coarse, exact) = (self.coarse_steps?, self.exact_steps?);
        // A shortest route of no steps is a destination the body stands on.
        let exact = u32::try_from(exact).ok().filter(|steps| *steps > 0)?;
        let coarse = u32::try_from(coarse).ok()?;
        Some(coarse.saturating_sub(exact) * 100 / exact)
    }
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

/// How far past the shortest route the corridor sent a body, over one band.
///
/// The distribution and not the mean: a hierarchy that is a few percent long
/// everywhere is doing its job, and one that is exact on seven destinations and
/// half as long again on the eighth is the report a player makes. Only the tail
/// tells those apart.
fn detours(readings: &[Reading]) {
    let mut measured: Vec<u32> = readings.iter().filter_map(Reading::detour).collect();
    if measured.is_empty() {
        return;
    }
    measured.sort_unstable();
    let worst = readings
        .iter()
        .filter(|reading| reading.detour().is_some())
        .max_by_key(|reading| reading.detour().expect("filtered"))
        .expect("the band has a measured reading");
    let index = |percentile: usize| measured[(measured.len() * percentile).div_ceil(100).saturating_sub(1)];
    println!(
        "    detour  measured={}/{}  p50={:>4}% p95={:>4}% worst={:>4}% at ({}, {}) — {} steps against {}",
        measured.len(),
        readings.len(),
        index(50),
        index(95),
        worst.detour().expect("filtered"),
        worst.x,
        worst.y,
        worst.coarse_steps.expect("a detour has a corridor"),
        worst.exact_steps.expect("a detour has a shortest route"),
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

fn synthetic() {
    const WIDTH: u16 = 1024;
    const HEIGHT: u16 = 1024;
    const FROM: Point = Point::new(1, 1, 0);
    const TO: Point = Point::new(WIDTH - 2, HEIGHT - 2, 0);
    const SAMPLES: usize = 25;

    // No map and nothing on it: open ground, which is what this half of the
    // bench is for.
    let nothing = Overlay::default();
    let open = Footing::new(None, &nothing, Doors::AsTheyStand);

    let built_at = Instant::now();
    let router = NavigationGraph::build(&open, u32::from(WIDTH), u32::from(HEIGHT))
        .expect("the synthetic facet fits Point's coordinate space");
    let built = built_at.elapsed();

    let flat_at = Instant::now();
    let flat = find_path(
        &open,
        FROM,
        TO,
        usize::from(WIDTH) * usize::from(HEIGHT),
        Weight::EXACT,
    )
    .expect("open ground has a flat route");
    let flat_elapsed = flat_at.elapsed();

    let mut coarse_samples = Vec::with_capacity(SAMPLES);
    let mut coarse_steps = None;
    for _ in 0..SAMPLES {
        let coarse_at = Instant::now();
        let coarse = find_long_path(&open, &open, &router, FROM, TO, 600, Weight::EXACT)
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
    let ground = bake::open_facet(&cli.client, bake::WorldSource::Install, facet)?;
    // The artifact the shard loads, validated the way the shard validates it: a
    // graph that no longer matches its inputs is not a slower answer, it is a
    // different world's answer.
    let graph = ground.coarse()?;
    let (regions, nodes, edges) = graph.counts();
    let terrain = ground.terrain();
    // The map and nothing over it: a facet's numbers have to be about the facet.
    let nothing_placed = Overlay::default();
    let footing = Footing::new(Some(terrain), &nothing_placed, Doors::AsTheyStand);
    let map = ground.world.snapshot.map();
    let (width, height) = (map.width(), map.height());

    let origin = standable(&terrain, cli.x, cli.y).ok_or("nothing stands at the origin")?;
    println!(
        "facet {facet} {width}x{height}: {regions} regions, {nodes} nodes, {edges} edges; from ({}, {}, {}) budget={} repeat={}",
        origin.x, origin.y, origin.z, cli.budget, cli.repeat,
    );

    // The ground truth a refusal is read against: whatever a router says about
    // finding the way, this is ground a body can walk. `Reach` is the crate's
    // one flood over the step rule — this example wrote its own until the
    // hygiene pass, and what that copy paid for was the whole expansion eight
    // times over per tile.
    let component = cli.component.then(|| {
        let started = Instant::now();
        let reached = Reach::of(&footing, origin, width, height);
        println!(
            "  flood: {} tiles walkable from the origin ({:.1}% of the facet) in {:.1}s",
            reached.count(),
            100.0 * reached.count() as f64 / reached.tiles() as f64,
            started.elapsed().as_secs_f64(),
        );
        reached
    });

    for band in BANDS {
        let destinations = sample_ring(&terrain, origin, band, width, height);
        let mut readings = Vec::with_capacity(destinations.len());
        for to in destinations {
            let distance = u32::from(to.x.abs_diff(origin.x)).max(u32::from(to.y.abs_diff(origin.y)));
            // Flat A* first.
            let mut flat = Duration::MAX;
            let mut flat_search = None;
            for _ in 0..cli.repeat.max(1) {
                let started = Instant::now();
                let search = search_path(&footing, origin, to, cli.budget, Weight::EXACT);
                flat = flat.min(started.elapsed());
                flat_search = Some(search);
            }
            let search = flat_search.expect("at least one repeat");

            // Then the corridor, asked exactly as `steer::Ground::path` asks
            // it: the same ground guides, joins and approves the exact steps.
            let mut coarse = Duration::MAX;
            let mut coarse_route = None;
            for _ in 0..cli.repeat.max(1) {
                let started = Instant::now();
                let route = find_long_path(&footing, &footing, &graph, origin, to, cli.budget, Weight::EXACT);
                coarse = coarse.min(started.elapsed());
                coarse_route = Some(route);
            }
            // And the shortest route the ground holds, once: this is a length
            // and not a duration, so repeating it would measure the host.
            let exact = (cli.exact > 0)
                .then(|| search_path(&footing, origin, to, cli.exact, Weight::EXACT))
                .filter(|search| search.arrived)
                .map(|search| search.route.len());
            readings.push(Reading {
                x: to.x,
                y: to.y,
                distance,
                flat,
                flat_nodes: search.explored,
                flat_arrived: search.arrived,
                coarse,
                coarse_steps: coarse_route.expect("at least one repeat").map(|r| r.len()),
                exact_steps: exact,
                walkable_from_origin: component.as_ref().map(|reached| reached.holds(to.x, to.y)),
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
        detours(&readings);
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
            if let (Some(exact), Some(detour)) = (reading.exact_steps, reading.detour()) {
                println!("          shortest={exact} steps, detour={detour}%");
            }
        }
    }
    Ok(())
}
