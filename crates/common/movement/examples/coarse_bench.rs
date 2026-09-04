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
//! **And on a shared machine it is a property of who else is running.** P1 was
//! first measured here at a load average of 30–50 and the durations came out
//! 30–60% high and — worse — high *unevenly*, which reversed the sign of one
//! comparison. Every repeat is a minimum for that reason (`--repeat`), and a
//! reading meant to be believed wants three things beside it: elevated priority
//! (`sudo nice -n -15 setpriv --reuid=$UID --regid=$UID --init-groups …`, which
//! raises the process without making it root's), the configurations being
//! compared run **interleaved** rather than one after the other, and the load
//! average written down. Step counts need none of this: a route is a property
//! of the graph.
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
//!
//! # Two readings, and they answer different questions
//!
//! `--rings` (the default) is the one above: a bare facet, a spread of
//! distances, and what the hierarchy costs against flat A\* on ground nobody
//! has built on.
//!
//! `--houses` is the other one, and it is the case a player is in: a design
//! laid live the way a client lays a house it is shown, and a click on it from
//! a body standing outside. Those destinations fail the bounded search for a
//! reason that is **not** distance — a roof is ten tiles away and a hundred
//! steps of stairs — so the corridor answers them however near they are, and
//! the detour is what the player watches. See [`houses`].
//!
//! `--handover` is a third and asks a different kind of question: not what the
//! router costs, but what it would cost to ask it **somewhere else**. See
//! [`handover`], and `plans/world/pathfinding/PLAN.md`'s P3.

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
    COARSE_MIN_DISTANCE,
    Footing,
    MapTerrain,
    NavigationGraph,
    Weight,
    bake,
    destination_place,
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
    /// Also read the detour over ground somebody has built on — see [`houses`].
    #[arg(long, default_value_t = false)]
    houses:    bool,
    /// Also read what one query's ground costs to hand to another thread — see
    /// [`handover`].
    ///
    /// Over the same building `--houses` uses, because the query being handed
    /// over is that one: a click on a roof is what makes a client pay for a
    /// corridor on the frame thread in the first place.
    #[arg(long, default_value_t = false)]
    handover:  bool,
    /// The distance-band reading over the bare facet: what this example is
    /// unless it is told otherwise.
    ///
    /// A knob rather than an implication of `--houses`, because the two answer
    /// different questions and a run that wants both should not have to guess
    /// which one it is getting.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    rings:     bool,
    /// The design `--houses` lays, as `dx,dy,dz,graphic,flags` rows.
    ///
    /// Absent is the castle the route journal's session was walked at, which
    /// ships with this crate's tests.
    #[arg(long, value_name = "CSV")]
    design:    Option<PathBuf>,
    /// Where that design's origin goes, as `X,Y,Z`.
    #[arg(
        long,
        value_name = "X,Y,Z",
        default_value = "1333,1882,0",
        value_parser = origin_at
    )]
    design_at: Point,
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
    /// Where the body stood. The ring reading asks every destination from one
    /// origin; the houses reading asks each from wherever a body could stand
    /// that far from the building, and a detour is a property of the pair.
    from: Point,
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
        "    detour  measured={}/{}  p50={:>4}% p95={:>4}% worst={:>4}% from ({}, {}) to ({}, {}) — {} steps against {}",
        measured.len(),
        readings.len(),
        index(50),
        index(95),
        worst.detour().expect("filtered"),
        worst.from.x,
        worst.from.y,
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

/// The castle the route journal's session was walked at — 2196 components at
/// `(1333, 1882, 0)` on Felucca, exported from a shard's `house_designs`.
///
/// The same file `real_routes.rs` asserts against, so the number this prints
/// and the route that test pins are about one building.
const CASTLE_DESIGN: &str = include_str!("../tests/data/castle-1333-1882.csv");
/// Where a body stands to click on the building, as Chebyshev distance from its
/// centre.
///
/// All of them past [`COARSE_MIN_DISTANCE`]: nearer than that a client refuses a
/// destination the bounded search could not reach without asking the graph at
/// all, which is a different defect (`docs/world/README.md`'s finding 24) and
/// not a detour.
const HOUSE_RINGS: [u32; 4] = [16, 24, 32, 48];
/// Clicks sampled over the building, spread across its footprint.
const HOUSE_CLICKS: usize = 6;

/// `X,Y,Z` — where a design's origin is laid.
fn origin_at(text: &str) -> Result<Point, String> {
    let fields: Vec<&str> = text.split(',').map(str::trim).collect();
    let [x, y, z] = fields.as_slice() else {
        return Err(format!("{text} is not X,Y,Z"));
    };
    Ok(Point::new(
        x.parse().map_err(|_| format!("{x} is not an x"))?,
        y.parse().map_err(|_| format!("{y} is not a y"))?,
        z.parse().map_err(|_| format!("{z} is not a z"))?,
    ))
}

/// One design laid live over the bare facet, and the places a body outside it
/// would click on and stand at.
///
/// **The scene both building readings are taken over**, so that what
/// [`houses`] measures about the route and what [`handover`] measures about
/// carrying that route's ground to another thread are about one building rather
/// than two spellings of a castle.
struct Building {
    design:  openshard_movement::design::Design,
    /// Where the design's origin was laid.
    at:      Point,
    /// Everything the design put on the map: the live layer of a client whose
    /// view has this one building in it.
    overlay: Overlay,
    /// The tiles it covers, so a body is never stood inside it.
    covered: std::collections::HashSet<(u16, u16)>,
    /// The middle of its footprint, which the rings are measured from.
    centre:  Point,
    /// How many tiles it covers, for the line each reading prints.
    tiles:   usize,
}

impl Building {
    /// Lay the design `--design` names, or the castle that ships with this
    /// crate's tests.
    fn lay(cli: &Cli, tiles: &openshard_tiles::TileData) -> Result<Self, Box<dyn std::error::Error>> {
        let text = match &cli.design {
            Some(path) => std::fs::read_to_string(path)?,
            None => CASTLE_DESIGN.to_owned(),
        };
        let design = openshard_movement::design::Design::parse(&text)?;
        let at = cli.design_at;
        let mut overlay = Overlay::default();
        design.lay(&mut overlay, tiles, at);

        let footprint = design.footprint(at);
        let covered: std::collections::HashSet<(u16, u16)> =
            footprint.iter().map(|tile| (tile.x, tile.y)).collect();
        let centre = {
            let (x, y) = footprint.iter().fold((0_u64, 0_u64), |(x, y), tile| {
                (x + u64::from(tile.x), y + u64::from(tile.y))
            });
            let count = footprint.len().max(1) as u64;
            Point::new((x / count) as u16, (y / count) as u16, at.z)
        };
        Ok(Self {
            design,
            at,
            overlay,
            covered,
            centre,
            tiles: footprint.len(),
        })
    }

    /// The places on it a cursor could hit and a body could stand on.
    ///
    /// The highest thing each tile draws, which is what a cursor hits, resolved
    /// to a place to stand the way the client resolves it.
    ///
    /// **Filtered to places a body actually stands**, and only then spread over
    /// what is left. A castle is mostly wall and roof edge, and a click that
    /// resolves into masonry is a refusal every router agrees about — sampling
    /// those measures the sampler. Striding the raw footprint is worse still:
    /// the tiles come in rows, and every stride that shares a factor with the
    /// row width picks one column of the building.
    fn clicks(&self, live: &Footing<'_>) -> Vec<Point> {
        let standing: Vec<Point> = self
            .design
            .tops(self.at)
            .into_iter()
            .map(|top| destination_place(live, self.centre, top))
            .filter(|place| openshard_movement::can_step(live, *place, *place).is_some())
            .collect();
        let stride = (standing.len() / HOUSE_CLICKS).max(1);
        let clicks: Vec<Point> = standing
            .iter()
            .copied()
            .step_by(stride)
            .take(HOUSE_CLICKS)
            .collect();
        println!(
            "  design at ({}, {}, {}): {} components, {} tiles, {} live tiles, centre ({}, {}); {} of them stood on, {} clicks sampled",
            self.at.x,
            self.at.y,
            self.at.z,
            self.design.components().len(),
            self.tiles,
            self.overlay.len(),
            self.centre.x,
            self.centre.y,
            standing.len(),
            clicks.len(),
        );
        for click in &clicks {
            println!("    click ({}, {}, {})", click.x, click.y, click.z);
        }
        clicks
    }

    /// Where a body could have been standing at Chebyshev `ring` from the
    /// building's centre when it clicked on it — on the bare terrain, and never
    /// inside the building itself.
    fn starts(&self, terrain: &MapTerrain<'_>, ring: u32, width: u32, height: u32) -> Vec<Point> {
        sample_ring(terrain, self.centre, ring, width, height)
            .into_iter()
            .filter(|start| !self.covered.contains(&(start.x, start.y)))
            .collect()
    }
}

/// What the corner rule costs **on ground somebody has built on** — the reading
/// `plans/world/pathfinding/PLAN.md`'s P1 is gated on.
///
/// The ring reading above measures the bare facet, where a long route amortises
/// a corner and the worst band is 13%. The case a player meets is the other one:
/// a destination a few tiles away that the bounded search cannot reach *for a
/// reason that is not distance* — a roof, a courtyard, a cellar behind one door
/// — where the corridor is asked despite the destination being near, and every
/// tile of detour is a tile the player watches their body walk the wrong way.
///
/// So: the building laid live the way a client lays a house it is shown, the
/// bare facet as the guide (which is what the graph was baked over), and a
/// click on the building from each of several distances. The detour is the
/// corridor's route against the shortest one the ground holds, exactly as in
/// the ring reading.
fn houses(
    cli: &Cli,
    terrain: &MapTerrain<'_>,
    tiles: &openshard_tiles::TileData,
    graph: &NavigationGraph,
    width: u32,
    height: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let building = Building::lay(cli, tiles)?;
    // The two grounds the client's own query reads: the live world a body walks
    // and opens doors in, and the bare map the graph was baked over. Handing
    // `find_long_path` the live layer as its guide would measure a graph nobody
    // has.
    let live = Footing::new(Some(*terrain), &building.overlay, Doors::AllOpen);
    let nothing_placed = Overlay::default();
    let guide = Footing::new(Some(*terrain), &nothing_placed, Doors::AsTheyStand);

    let clicks = building.clicks(&live);

    for ring in HOUSE_RINGS {
        let starts = building.starts(terrain, ring, width, height);
        let mut readings = Vec::with_capacity(starts.len() * clicks.len());
        let mut bounded_answered = 0_usize;
        let mut too_near = 0_usize;
        for from in &starts {
            for to in &clicks {
                let distance = u32::from(to.x.abs_diff(from.x)).max(u32::from(to.y.abs_diff(from.y)));
                // The client's first question, at the client's own budget. When
                // it arrives the graph is never asked and there is no detour to
                // measure — that is the case this reading is *not* about.
                let mut flat = Duration::MAX;
                let mut bounded = None;
                for _ in 0..cli.repeat.max(1) {
                    let started = Instant::now();
                    let search = search_path(&live, *from, *to, cli.budget, Weight::PLANNING);
                    flat = flat.min(started.elapsed());
                    bounded = Some(search);
                }
                let bounded = bounded.expect("at least one repeat");
                if bounded.arrived {
                    bounded_answered += 1;
                    continue;
                }
                // And the case the client refuses outright rather than asking
                // the graph: near in a straight line, unreachable in fact.
                if distance <= COARSE_MIN_DISTANCE {
                    too_near += 1;
                    continue;
                }
                // `--repeat`, and the fastest of them, for the ring reading's
                // own reason: this workstation is shared and the slow readings
                // are the ones somebody else's build was running through. The
                // steps do not vary — the route is a property of the graph — so
                // what the repeats buy is the millisecond and nothing else.
                let mut coarse = Duration::MAX;
                let mut route = None;
                for _ in 0..cli.repeat.max(1) {
                    let started = Instant::now();
                    let answer =
                        find_long_path(&guide, &live, graph, *from, *to, cli.budget, Weight::PLANNING);
                    coarse = coarse.min(started.elapsed());
                    route = Some(answer);
                }
                let route = route.expect("at least one repeat");
                let exact = (cli.exact > 0)
                    .then(|| search_path(&live, *from, *to, cli.exact, Weight::PLANNING))
                    .filter(|search| search.arrived)
                    .map(|search| search.route.len());
                readings.push(Reading {
                    from: *from,
                    x: to.x,
                    y: to.y,
                    distance,
                    flat,
                    flat_nodes: bounded.explored,
                    flat_arrived: bounded.arrived,
                    coarse,
                    coarse_steps: route.map(|route| route.len()),
                    exact_steps: exact,
                    // No flood: the destination is a building this run laid
                    // itself, and what a refusal means about it is read off the
                    // exact search beside it rather than off a component.
                    walkable_from_origin: None,
                });
            }
        }
        let asked = readings.len();
        let refused = readings
            .iter()
            .filter(|reading| reading.coarse_steps.is_none())
            .count();
        println!(
            "  ring {ring}: starts={} clicks={} pairs={}  bounded_answered={bounded_answered} \
             too_near={too_near} asked_the_graph={asked} refused={refused}",
            starts.len(),
            clicks.len(),
            starts.len() * clicks.len(),
        );
        if readings.is_empty() {
            continue;
        }
        spread(
            "coarse",
            readings.iter().map(|reading| reading.coarse).collect(),
            asked - refused,
            asked,
        );
        detours(&readings);
        for reading in &readings {
            let steps = reading
                .coarse_steps
                .map_or_else(|| "-".to_owned(), |steps| steps.to_string());
            let detour = match (reading.exact_steps, reading.detour()) {
                (Some(exact), Some(detour)) => format!("shortest={exact} detour={detour}%"),
                _ => "no shortest route inside --exact".to_owned(),
            };
            println!(
                "      ({:5}, {:5}) -> ({:5}, {:5}) d={:<4} coarse {:8.3} ms steps={:<5} {detour}",
                reading.from.x,
                reading.from.y,
                reading.x,
                reading.y,
                reading.distance,
                ms(reading.coarse),
                steps,
            );
        }
    }
    Ok(())
}

/// How far past the two endpoints a guessed window reaches, in tiles.
///
/// One navigation region (`navigation.rs`'s private `REGION_SIZE`, 32), because
/// that is the smallest guess with an argument behind it: a corridor's first
/// move is out to a crossing of the region the body stands in, so a window
/// narrower than a region cannot even hold the first hop.
const WINDOW_PAD: u16 = 32;

/// One query, and what its ground costs to carry.
struct Handover {
    from:   Point,
    to:     Point,
    /// The corridor query itself: what the carrying would ride with.
    plan:   Duration,
    /// Cloning the whole live layer — the ground handed over entire.
    whole:  Duration,
    /// Cutting the window around the two endpoints out of it.
    window: Duration,
    /// How many tiles that window had to be asked about.
    looked: u64,
    /// How many tiles of the live layer it kept.
    kept:   usize,
    /// Whether the route the corridor actually returned stayed inside the
    /// window — `None` for a query the corridor refused.
    inside: Option<bool>,
}

/// What one query's ground costs to hand to a thread that is not the one
/// drawing — the measurement `plans/world/pathfinding/PLAN.md`'s P3 waits on.
///
/// # The question, and why it is not "how fast is a thread"
///
/// A plan is ~25 ms on the frame thread and a body takes a step every ~200 ms,
/// so planning is a permanent fraction of the frame while anybody is walking.
/// Moving it off that thread is obvious; what is *not* obvious is what the
/// worker is allowed to read. The search reads two grounds — the **guide**,
/// which is the bare facet and the coarse graph over it and never changes while
/// a body walks, and the **live overlay**, which the client rebuilds whole
/// every time the shard sends it a new picture of the ground.
///
/// So the only half that has to travel is the live one, and the plan named two
/// ways of sending it: the whole thing, or a cut of it around the query. This
/// measures both against the plan they would ride with.
///
/// # What the two numbers mean
///
/// - **whole** is `Overlay::clone` — O(what the shard has placed in view).
/// - **window** is the cut: every tile in the box the two endpoints span,
///   padded by [`WINDOW_PAD`], asked of the overlay and copied when it holds
///   anything — O(the *area* the query might reach), which is a different
///   variable and grows with the square of the distance rather than with the
///   furniture.
///
/// And the cut has a soundness column beside its duration, because a window is
/// a *guess* about where the answer will go: `inside` is whether the route the
/// corridor really returned stayed in the box. A route that leaves it is a
/// route planned over a hole — the live layer simply absent for the tiles it
/// crossed — which is not a slower answer but a wrong one.
///
/// The crowd is not measured. It is a `Vec<Point>` of the mobiles in the view
/// (`clutter::crowd`), six bytes each and tens of them; a clone of it is a
/// memcpy that does not reach a microsecond, and nothing about the shape of
/// this decision turns on it.
fn handover(
    cli: &Cli,
    terrain: &MapTerrain<'_>,
    tiles: &openshard_tiles::TileData,
    graph: &NavigationGraph,
    width: u32,
    height: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let building = Building::lay(cli, tiles)?;
    let live = Footing::new(Some(*terrain), &building.overlay, Doors::AllOpen);
    let nothing_placed = Overlay::default();
    let guide = Footing::new(Some(*terrain), &nothing_placed, Doors::AsTheyStand);
    let clicks = building.clicks(&live);

    for ring in HOUSE_RINGS {
        let starts = building.starts(terrain, ring, width, height);
        let mut readings = Vec::with_capacity(starts.len() * clicks.len());
        for from in &starts {
            for to in &clicks {
                let distance = u32::from(to.x.abs_diff(from.x)).max(u32::from(to.y.abs_diff(from.y)));
                // The same two gates `houses` applies, and for the same reason:
                // a pair the bounded search answers never reaches the graph, and
                // one the client refuses without asking never reaches it either.
                // Neither is a query anybody would hand to a worker.
                if search_path(&live, *from, *to, cli.budget, Weight::PLANNING).arrived {
                    continue;
                }
                if distance <= COARSE_MIN_DISTANCE {
                    continue;
                }
                let mut plan = Duration::MAX;
                let mut route = None;
                for _ in 0..cli.repeat.max(1) {
                    let started = Instant::now();
                    let answer =
                        find_long_path(&guide, &live, graph, *from, *to, cli.budget, Weight::PLANNING);
                    plan = plan.min(started.elapsed());
                    route = Some(answer);
                }
                let route = route.expect("at least one repeat");

                let mut whole = Duration::MAX;
                for _ in 0..cli.repeat.max(1) {
                    let started = Instant::now();
                    let copy = building.overlay.clone();
                    whole = whole.min(started.elapsed());
                    // Kept until the clock has been read, so the optimiser
                    // cannot decide the clone was never needed.
                    drop(copy);
                }

                let box_ = window_of(*from, *to, width, height);
                let mut window = Duration::MAX;
                let mut kept = 0;
                for _ in 0..cli.repeat.max(1) {
                    let started = Instant::now();
                    let cut = cut_window(&building.overlay, box_);
                    window = window.min(started.elapsed());
                    kept = cut.len();
                }

                let inside = route
                    .as_ref()
                    .map(|steps| walked_inside(&live, *from, steps, box_));
                readings.push(Handover {
                    from: *from,
                    to: *to,
                    plan,
                    whole,
                    window,
                    looked: box_.tiles(),
                    kept,
                    inside,
                });
            }
        }
        if readings.is_empty() {
            println!("  ring {ring}: no pair reached the graph");
            continue;
        }
        let escaped = readings
            .iter()
            .filter(|reading| reading.inside == Some(false))
            .count();
        let routed = readings.iter().filter(|r| r.inside.is_some()).count();
        let looked: u64 = readings.iter().map(|reading| reading.looked).sum();
        println!(
            "  ring {ring}: pairs={} window={}x{} mean_tiles_looked_at={} kept={}",
            readings.len(),
            window_of(readings[0].from, readings[0].to, width, height).wide(),
            window_of(readings[0].from, readings[0].to, width, height).high(),
            looked / readings.len() as u64,
            readings[0].kept,
        );
        spread(
            "plan",
            readings.iter().map(|reading| reading.plan).collect(),
            routed,
            readings.len(),
        );
        spread(
            "whole",
            readings.iter().map(|reading| reading.whole).collect(),
            readings.len(),
            readings.len(),
        );
        spread(
            "window",
            readings.iter().map(|reading| reading.window).collect(),
            readings.len(),
            readings.len(),
        );
        // The share of the plan each way of carrying the ground would add, at
        // the medians — the number the decision is actually taken on.
        let share = |pick: fn(&Handover) -> Duration| {
            let mut costs: Vec<Duration> = readings.iter().map(pick).collect();
            let mut plans: Vec<Duration> = readings.iter().map(|reading| reading.plan).collect();
            costs.sort_unstable();
            plans.sort_unstable();
            100.0 * ms(percentile(&costs, 50)) / ms(percentile(&plans, 50))
        };
        println!(
            "    share   whole={:.2}% of the plan, window={:.2}%; routes leaving the window: {escaped}/{routed}",
            share(|reading| reading.whole),
            share(|reading| reading.window),
        );
    }
    Ok(())
}

/// A rectangle of tiles, as the window cut works over one.
#[derive(Clone, Copy)]
struct Window {
    west:  u16,
    north: u16,
    east:  u16,
    south: u16,
}

impl Window {
    const fn wide(self) -> u32 {
        (self.east - self.west) as u32 + 1
    }

    const fn high(self) -> u32 {
        (self.south - self.north) as u32 + 1
    }

    const fn tiles(self) -> u64 {
        self.wide() as u64 * self.high() as u64
    }

    const fn holds(self, point: Point) -> bool {
        point.x >= self.west && point.x <= self.east && point.y >= self.north && point.y <= self.south
    }
}

/// The box a cut would have to guess at for this query: the two endpoints,
/// padded by [`WINDOW_PAD`] and clamped to the facet.
fn window_of(from: Point, to: Point, width: u32, height: u32) -> Window {
    let last_x = (width.saturating_sub(1)).min(u32::from(u16::MAX)) as u16;
    let last_y = (height.saturating_sub(1)).min(u32::from(u16::MAX)) as u16;
    Window {
        west:  from.x.min(to.x).saturating_sub(WINDOW_PAD),
        north: from.y.min(to.y).saturating_sub(WINDOW_PAD),
        east:  from.x.max(to.x).saturating_add(WINDOW_PAD).min(last_x),
        south: from.y.max(to.y).saturating_add(WINDOW_PAD).min(last_y),
    }
}

/// Everything the live layer has inside `window`, as a layer of its own.
///
/// The overlay hands out one tile at a time and nothing else — it is a map from
/// tile to covers with no ordering and no iterator, deliberately, because every
/// caller of it is a step asking about a tile it has in hand. So a cut asks it
/// once per tile of the window, which is what makes this O(area) rather than
/// O(furniture).
fn cut_window(live: &Overlay, window: Window) -> Overlay {
    let mut cut = Overlay::default();
    for y in window.north..=window.south {
        for x in window.west..=window.east {
            let tile = Tile::new(x, y);
            let covers = live.at(tile);
            if !covers.is_empty() {
                cut.set(tile, covers.to_vec());
            }
        }
    }
    cut
}

/// Whether every place the route stands on is inside `window`.
///
/// Walked over the same ground the plan was made against, so what is tested is
/// where the body would really go rather than where the directions look like
/// they lead.
fn walked_inside(
    live: &Footing<'_>,
    from: Point,
    steps: &[openshard_protocol::direction::Direction],
    window: Window,
) -> bool {
    let mut at = from;
    for &direction in steps {
        let Some(next) = openshard_movement::step_allowed(live, at, direction) else {
            // The plan does not walk on this ground any more, which is a
            // different finding and not this one's to report.
            return true;
        };
        at = next;
        if !window.holds(at) {
            return false;
        }
    }
    true
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

    println!(
        "facet {facet} {width}x{height}: {regions} regions, {nodes} nodes, {edges} edges; budget={} repeat={}",
        cli.budget, cli.repeat,
    );

    if cli.houses {
        println!("houses: what the corner rule costs a click on a building");
        houses(&cli, &terrain, &ground.tiles, &graph, width, height)?;
    }
    if cli.handover {
        println!("handover: what one query's ground costs to carry off the frame thread");
        handover(&cli, &terrain, &ground.tiles, &graph, width, height)?;
    }
    if !cli.rings {
        return Ok(());
    }

    let origin = standable(&terrain, cli.x, cli.y).ok_or("nothing stands at the origin")?;
    println!(
        "rings: from ({}, {}, {}) over the bare facet",
        origin.x, origin.y, origin.z,
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
                from: origin,
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
