//! Routes a person actually asked for, on the facet the client shipped.
//!
//! `walk_scenes.rs` walks the step rule over scenes this repository builds, and
//! `path.rs`'s own tests plan routes over an overlay a test wrote. Neither can
//! answer the question a player asks — *why did my click not walk me down from
//! this floor* — because the geometry that refuses it is Britannia's own: a
//! building whose upper storey is statics in `statics0.mul`, reachable only by
//! whatever stair its author drew.
//!
//! What this asks is the client's own question, in the client's own two steps:
//! the bounded A\* at [`PLAN_BUDGET`] first, and the coarse graph after it when
//! the destination is further than [`COARSE_MIN_DISTANCE`]. A test that asked
//! only the first would report a refusal the shipped client never makes, and one
//! that asked with an unbounded budget would report an arrival it never makes
//! either.
//!
//! Gated on `OPENSHARD_CLIENT` and `#[ignore]`d, like every test here that reads
//! a couple of gigabytes of client files:
//!
//! ```sh
//! OPENSHARD_CLIENT=… cargo test --release -p openshard-movement \
//!   --test real_routes -- --nocapture --ignored
//! ```

use std::path::PathBuf;

use openshard_map::grid::Tile;
use openshard_map::overlay::{Doors, Overlay};
use openshard_map::snapshot::MapSnapshot;
use openshard_movement::bake::{FacetWorld, WorldSource};
use openshard_movement::spans::SpanIndex;
use openshard_movement::{
    COARSE_MIN_DISTANCE, Footing, MapTerrain, NavigationGraph, Weight, destination_place, find_long_path,
    find_path, search_path, step_allowed,
};
use openshard_protocol::world::{Facet, Point};

/// The upper storey a person was standing on when a click did nothing:
/// `(1340, 1676)` at z 52, over land at 30.
const UPSTAIRS: Point = Point::new(1340, 1676, 52);
/// The other end of the same report — a second place on that storey, sixteen
/// tiles away and also at 52.
const ALONG: Point = Point::new(1356, 1669, 52);
/// The height of the street under that building.
const STREET_Z: i8 = 30;
/// What a client's click-to-walk plan may spend, from `client/app`'s `steer.rs`.
///
/// Copied rather than imported: `openshard-movement` is below the client and
/// must not depend on it. A number that drifts apart from that one makes this
/// test report about a client nobody runs, which is what the assertion below
/// names it for.
const PLAN_BUDGET: usize = 700;

fn client_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    dir.join("tiledata.mul").exists().then_some(dir)
}

/// What an install owns, so a test can hold it and hand out views of it.
struct Install {
    snapshot: MapSnapshot,
    tiles: openshard_tiles::TileData,
    spans: SpanIndex,
    /// The baked coarse graph, when one is beside the install and current.
    ///
    /// `None` is a real state and not a failure — a client with no bake plans
    /// long routes with nothing but the bounded search, and says so on stderr —
    /// so the survey reports which of the two it measured rather than skipping.
    coarse: Option<NavigationGraph>,
}

impl Install {
    fn terrain(&self) -> MapTerrain<'_> {
        MapTerrain::new(self.snapshot.map(), &self.tiles, &self.spans)
    }
}

fn real_install() -> Option<Install> {
    let dir = client_dir()?;
    let facet = Facet(0);
    // The world a *shard* is running, when one is named: `world.base_sets` in
    // `openshard.toml` replaces the install's map and statics, and the artifacts
    // move with it — beside the base set rather than beside the install. A
    // survey that read the install's world and the install's graph would report
    // on a world nobody is playing, and it is exactly the pair whose staleness
    // is being asked about.
    let base_set = std::env::var_os("OPENSHARD_BASE_SET").map(PathBuf::from);
    let source = match &base_set {
        Some(path) => WorldSource::BaseSet(path),
        None => WorldSource::Install,
    };
    let world = FacetWorld::read(&dir, source, facet).expect("the facet should load");
    let tiles =
        openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata should load");
    let spans = SpanIndex::build(world.snapshot.map(), &tiles);
    let navigation = world.navigation_path(&dir);
    let coarse = world
        .stamp(&dir, facet)
        .and_then(|stamp| openshard_movement::bake::load(&navigation, &stamp))
        .map_err(|error| eprintln!("no coarse graph: {error}"))
        .ok();
    Some(Install {
        snapshot: world.snapshot,
        tiles,
        spans,
        coarse,
    })
}

/// The client's own plan, in the client's own order: the bounded search, and
/// the coarse graph after it for anything further than a few tiles.
///
/// `steer.rs`'s `Readings::path`, written out — with one footing rather than
/// two, because nothing is placed on the ground here and a live reading and a
/// guide reading of a bare facet are the same reading.
fn client_plan(
    footing: &Footing<'_>,
    coarse: Option<&NavigationGraph>,
    from: Point,
    to: Point,
) -> Option<Vec<openshard_protocol::direction::Direction>> {
    if let Some(local) = find_path(footing, from, to, PLAN_BUDGET, Weight::PLANNING) {
        return Some(local);
    }
    let distance = u32::from(from.x.abs_diff(to.x)).max(u32::from(from.y.abs_diff(to.y)));
    if distance <= COARSE_MIN_DISTANCE {
        return None;
    }
    coarse.and_then(|graph| find_long_path(footing, footing, graph, from, to, PLAN_BUDGET, Weight::PLANNING))
}

/// One search, reported the way a bug report needs it: whether it arrived, what
/// it cost, and — walked by the shipped step rule — where the route it planned
/// actually ends.
fn report(label: &str, footing: &Footing<'_>, from: Point, to: Point, budget: usize) {
    let goal = destination_place(footing, from, to);
    let search = search_path(footing, from, to, budget, Weight::PLANNING);
    let mut at = from;
    for &direction in &search.route {
        let Some(next) = step_allowed(footing, at, direction) else {
            println!("  {label}: the search planned a step the rule refuses at {at:?}");
            break;
        };
        at = next;
    }
    println!(
        "  {label:<28} budget={budget:<6} arrived={:<5} exit={:?} nodes={} steps={} ends at ({}, {}, {}) goal=({}, {}, {})",
        search.arrived,
        search.exit,
        search.explored,
        search.route.len(),
        at.x,
        at.y,
        at.z,
        goal.x,
        goal.y,
        goal.z,
    );
}

/// Every place to stand in a square around a point, printed as a picture.
///
/// The highest surface alone is what tells a raised floor from the ground under
/// it, which is all this has to show: where the building is.
fn picture(terrain: &MapTerrain<'_>, centre: Point, radius: u16) {
    for y in (centre.y - radius)..=(centre.y + radius) {
        let mut row = String::new();
        for x in (centre.x - radius)..=(centre.x + radius) {
            let highest = terrain.surfaces(x, y).into_iter().max();
            row.push(match (x, y) {
                _ if (x, y) == (centre.x, centre.y) => '@',
                _ if (x, y) == (ALONG.x, ALONG.y) => 'B',
                _ => match highest {
                    None => '#',
                    Some(z) if z >= 45 => 'X',
                    Some(z) if z >= 35 => '+',
                    Some(z) if z >= 25 => '.',
                    Some(_) => ',',
                },
            });
        }
        println!("  {row}");
    }
}

/// A survey of the two places a person reported, and of the routes between them
/// and the street below.
///
/// Not an assertion about the numbers: what it prints is the evidence the
/// assertions are written from, and a facet's own geometry is not something a
/// test should pin to a count.
#[test]
#[ignore = "reads a client install and surveys one building — see the doc comment"]
fn a_second_storey_route_survey() {
    let Some(install) = real_install() else {
        eprintln!("OPENSHARD_CLIENT is unset — nothing to survey");
        return;
    };
    let terrain = install.terrain();
    let nothing_placed = Overlay::default();
    let footing = Footing::new(Some(terrain), &nothing_placed, Doors::AsTheyStand);
    println!(
        "coarse graph: {}",
        match &install.coarse {
            Some(graph) => {
                let (regions, nodes, edges) = graph.counts();
                format!("{regions} regions, {nodes} nodes, {edges} edges")
            }
            None => "none — long routes are the bounded search alone".to_owned(),
        },
    );

    for at in [UPSTAIRS, ALONG] {
        println!(
            "({}, {}) stands: {:?} land={:?} ceiling={:?}",
            at.x,
            at.y,
            terrain.surfaces(at.x, at.y),
            terrain.ground_z(Tile::new(at.x, at.y)),
            terrain.ceiling(at.x, at.y),
        );
    }

    println!(
        "the building around ({}, {}), 'X' is a storey:",
        UPSTAIRS.x, UPSTAIRS.y
    );
    picture(&terrain, UPSTAIRS, 12);

    let ground = Point::new(UPSTAIRS.x, UPSTAIRS.y, STREET_Z);
    println!("routes:");
    for budget in [PLAN_BUDGET, 2_000, 20_000] {
        report("upstairs -> along", &footing, UPSTAIRS, ALONG, budget);
        report("upstairs -> its own ground", &footing, UPSTAIRS, ground, budget);
        report("ground -> upstairs", &footing, ground, UPSTAIRS, budget);
    }

    // Every place on the street around the building, asked for from the storey,
    // the way the client asks: bounded search, then the coarse graph.
    //
    // One route is an anecdote. What a person clicking about upstairs meets is a
    // *neighbourhood* of destinations, and the question is how much of it the
    // shipped plan can answer — so the second pass differs from the first only
    // in the cap, and a destination it reaches that the first does not is the
    // budget and nothing else.
    let (mut planned, mut budget_bound, mut nowhere) = (0u32, 0u32, 0u32);
    let (mut coarse_saved, mut worst) = (0u32, 0usize);
    for y in (UPSTAIRS.y - 20)..=(UPSTAIRS.y + 20) {
        for x in (UPSTAIRS.x - 20)..=(UPSTAIRS.x + 20) {
            // The street, named the way a click on it names it: the ground's own
            // height, which `destination_place` resolves onto whatever surface
            // that column really has.
            let to = Point::new(x, y, STREET_Z);
            let local = find_path(&footing, UPSTAIRS, to, PLAN_BUDGET, Weight::PLANNING).is_some();
            let plan = client_plan(&footing, install.coarse.as_ref(), UPSTAIRS, to).is_some();
            if plan && !local {
                coarse_saved += 1;
            }
            if plan {
                planned += 1;
                continue;
            }
            let generous = search_path(&footing, UPSTAIRS, to, 50_000, Weight::PLANNING);
            if generous.arrived {
                budget_bound += 1;
                worst = worst.max(generous.explored);
            } else {
                nowhere += 1;
            }
        }
    }
    println!(
        "street around the storey: planned = {planned} (of which {coarse_saved} only because of the \
         coarse graph), refused = {}, of which {budget_bound} are reachable at 50000 nodes and \
         {nowhere} are not; worst budget-bound search cost {worst} nodes",
        budget_bound + nowhere,
    );

    // What a click across water costs, which is the archipelago's own question.
    //
    // A facet is islands, so the coarse graph is a **forest** and not a tree:
    // Britain and Moonglow are two components of it with no edge between them,
    // and nothing in the graph says so. `abstract_path` is an A* whose heuristic
    // points at a goal it can never reach, so it settles every node of the
    // component it started in before it can answer — and, unlike the joins and
    // the refinement, that walk is not charged to `LONG_PATH_EFFORT`. The
    // refusal is correct; what it costs is what this measures, and what a stored
    // component label would turn into a comparison of two integers.
    println!("clicks across water, and one over land for scale:");
    for (label, to) in [
        ("Moonglow, an island", Point::new(4467, 1283, 0)),
        ("Skara Brae, an island", Point::new(600, 2100, 0)),
        ("Magincia, an island", Point::new(3714, 2220, 0)),
        ("Trinsic, the same landmass", Point::new(1828, 2745, 0)),
    ] {
        let started = std::time::Instant::now();
        let plan = client_plan(&footing, install.coarse.as_ref(), UPSTAIRS, to);
        println!(
            "  {label:<26} ({:4}, {:4}) planned={:<5} steps={:<6} {:8.3} ms",
            to.x,
            to.y,
            plan.is_some(),
            plan.as_ref().map_or(0, Vec::len),
            started.elapsed().as_secs_f64() * 1_000.0,
        );
    }

    // The claim this file is for: a body on that storey has a way down, and the
    // shipped client plans it. The route is walked by the step rule rather than
    // trusted, because a plan the rule refuses is worse than no plan.
    let down =
        client_plan(&footing, install.coarse.as_ref(), UPSTAIRS, ground).expect("the storey has a way down");
    let mut at = UPSTAIRS;
    for &direction in &down {
        at = step_allowed(&footing, at, direction).expect("the plan named a step the rule refuses");
    }
    assert_eq!(
        at,
        Point::new(ground.x, ground.y, STREET_Z),
        "the way down does not end on the street it was planned to",
    );
}
