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

use std::collections::HashMap;
use std::path::PathBuf;

use openshard_map::grid::Tile;
use openshard_map::overlay::{
    Doors,
    Overlay,
};
use openshard_movement::bake::{
    OpenFacet,
    WorldSource,
    open_facet,
};
use openshard_movement::{
    COARSE_MIN_DISTANCE,
    Footing,
    MapTerrain,
    NavigationGraph,
    Weight,
    destination_place,
    find_long_path,
    find_path,
    search_path,
    step_allowed,
};
use openshard_protocol::world::{
    Facet,
    Point,
};

/// The upper storey a person was standing on when a click did nothing:
/// `(1340, 1676)` at z 52, over land at 30.
const UPSTAIRS: Point = Point::new(1340, 1676, 52);
/// The other end of the same report — a second place on that storey, sixteen
/// tiles away and also at 52.
const ALONG: Point = Point::new(1356, 1669, 52);
/// The height of the street under that building.
const STREET_Z: i8 = 30;
/// Where the route journal's first recorded session was walked: the tile the
/// body stood on, twenty-five tiles south of the castle roof it was clicked at,
/// when its route began to oscillate — see `docs/world/README.md`'s finding 23.
const SOUTH: Point = Point::new(1345, 1918, 0);
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

/// The facet, and the coarse graph beside it.
///
/// The graph is `None` on a real state and not a failure — a client with no bake
/// plans long routes with nothing but the bounded search, and says so on
/// stderr — so the survey reports which of the two it measured rather than
/// skipping.
fn real_facet() -> Option<(OpenFacet, Option<NavigationGraph>)> {
    let dir = client_dir()?;
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
    let facet = open_facet(&dir, source, Facet(0)).expect("the facet should load");
    let coarse = facet
        .coarse()
        .map_err(|error| eprintln!("no coarse graph: {error}"))
        .ok();
    Some((facet, coarse))
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
                _ => {
                    match highest {
                        None => '#',
                        Some(z) if z >= 45 => 'X',
                        Some(z) if z >= 35 => '+',
                        Some(z) if z >= 25 => '.',
                        Some(_) => ',',
                    }
                }
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
    let Some((facet, coarse)) = real_facet() else {
        eprintln!("OPENSHARD_CLIENT is unset — nothing to survey");
        return;
    };
    let terrain = facet.terrain();
    let nothing_placed = Overlay::default();
    let footing = Footing::new(Some(terrain), &nothing_placed, Doors::AsTheyStand);
    println!(
        "coarse graph: {}",
        match &coarse {
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
            let plan = client_plan(&footing, coarse.as_ref(), UPSTAIRS, to).is_some();
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
        let plan = client_plan(&footing, coarse.as_ref(), UPSTAIRS, to);
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
    let down = client_plan(&footing, coarse.as_ref(), UPSTAIRS, ground).expect("the storey has a way down");
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

/// Every place a route stands on, walked by the shipped step rule.
///
/// The directions are trusted for nothing: a plan is a claim about the ground
/// and this is the ground answering it, which is the same walk `report` makes
/// and the same one `steer.rs` makes of its own plan before drawing it.
fn walked(
    footing: &Footing<'_>,
    from: Point,
    route: &[openshard_protocol::direction::Direction],
) -> Vec<Point> {
    let mut at = from;
    let mut places = vec![at];
    for &direction in route {
        at = step_allowed(footing, at, direction).expect("the plan named a step the rule refuses");
        places.push(at);
    }
    places
}

/// The first place a route stands on twice, and how many steps that costs.
fn loop_in(places: &[Point]) -> Option<(Point, usize, usize)> {
    let mut seen = HashMap::new();
    for (index, &place) in places.iter().enumerate() {
        if let Some(first) = seen.insert(place, index) {
            return Some((place, first, index));
        }
    }
    None
}

/// A long route may not visit one place twice.
///
/// The report is `docs/world/README.md`'s finding 23: a click twenty-five tiles
/// away planned a route that began by stepping **off** the tile it then walked
/// straight back onto, and the plan made from that neighbouring tile began by
/// stepping back again. The body walked between the two until the window closed,
/// and the stall patience never saw it — `STUCK_STEPS` compares the body's
/// position, and the body was moving.
///
/// The assertion is not about which way a route goes, which is the facet's own
/// business. It is that a walk which stands somewhere twice has a shorter walk
/// inside it: whatever the corridor decided, the steps between the two visits
/// are steps that lead nowhere, and two neighbouring starts each stepping onto
/// the other is exactly what that looks like from a body's point of view.
///
/// The destinations are far enough to be answered by the coarse graph rather
/// than the bounded search ([`COARSE_MIN_DISTANCE`]), because refinement is what
/// this is about, and the starts are a square of tiles rather than one, because
/// the report is about *neighbouring* starts disagreeing.
#[test]
#[ignore = "reads a client install and plans a few hundred long routes — see the doc comment"]
fn a_long_route_never_visits_a_place_twice() {
    let Some((facet, coarse)) = real_facet() else {
        eprintln!("OPENSHARD_CLIENT is unset — nothing to walk");
        return;
    };
    let Some(graph) = coarse else {
        eprintln!("no coarse graph — a long route cannot be planned at all");
        return;
    };
    let terrain = facet.terrain();
    let nothing_placed = Overlay::default();
    let footing = Footing::new(Some(terrain), &nothing_placed, Doors::AsTheyStand);

    // Where the journal's session was walked, and the storey this file already
    // knows about: two neighbourhoods of the same landmass rather than one, so a
    // loop that is a property of one building is told from a property of the
    // refinement.
    let (mut planned, mut looped, mut worst) = (0u32, 0u32, 0usize);
    let mut first_report = None;
    for centre in [SOUTH, UPSTAIRS] {
        for to in [
            Point::new(1828, 2745, 0),
            Point::new(1345, 1830, 0),
            Point::new(1420, 1960, 0),
            Point::new(1290, 1750, 0),
        ] {
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let x = centre.x.saturating_add_signed(dx as i16);
                    let y = centre.y.saturating_add_signed(dy as i16);
                    // A start nothing stands on plans nothing, and a square of
                    // tiles around a body is bound to hold a few.
                    let Some(from) = terrain.can_step(Point::new(x, y, centre.z), Point::new(x, y, centre.z))
                    else {
                        continue;
                    };
                    let Some(route) = find_long_path(
                        &footing,
                        &footing,
                        &graph,
                        from,
                        to,
                        PLAN_BUDGET,
                        Weight::PLANNING,
                    ) else {
                        continue;
                    };
                    planned += 1;
                    let places = walked(&footing, from, &route);
                    let Some((place, first, again)) = loop_in(&places) else {
                        continue;
                    };
                    looped += 1;
                    worst = worst.max(again - first);
                    first_report.get_or_insert((from, to, place, first, again, places.clone()));
                }
            }
        }
    }
    println!(
        "long routes planned: {planned}, of which {looped} stand somewhere twice (worst loop {worst} steps)"
    );
    if let Some((from, to, place, first, again, places)) = first_report {
        println!(
            "  ({}, {}, {}) -> ({}, {}, {}): at ({}, {}, {}) as step {first} and step {again}; first eight {:?}",
            from.x,
            from.y,
            from.z,
            to.x,
            to.y,
            to.z,
            place.x,
            place.y,
            place.z,
            &places[..places.len().min(8)],
        );
    }
    assert_eq!(
        looped, 0,
        "a long route walks a loop that leads nowhere — see the printed route",
    );
}

/// The castle the journal's session was walked at, as the shard holds it: a
/// custom design of 2196 components, its origin at `(1333, 1882, 0)` on Felucca.
///
/// A file and not a constant, because it is a *building somebody built* — the
/// `house_designs` rows of the shard this repository is developed against,
/// exported once. What it buys is the one scene the report cannot be reproduced
/// without: a roof at z 88 reachable only by the stairs its owner drew, which no
/// arrangement of a floor or two stands in for.
const CASTLE_DESIGN: &str = include_str!("data/castle-1333-1882.csv");
const CASTLE_AT: Point = Point::new(1333, 1882, 0);
/// The click that started the recorded session: a tile of that castle's roof.
const CASTLE_ROOF: Point = Point::new(1345, 1894, 88);

/// The castle laid into an overlay the way a client lays a house it is shown.
///
/// `client/app`'s `clutter::fill`, in the one form this crate can reach: each
/// drawn component becomes the covers its art carries, based at the height the
/// component stands at, and every component of one tile goes in together.
fn place_castle(overlay: &mut Overlay, tiles: &openshard_tiles::TileData, origin: Point) {
    let mut covers: HashMap<Tile, Vec<openshard_map::overlay::Cover>> = HashMap::new();
    for line in CASTLE_DESIGN.lines().filter(|line| !line.is_empty()) {
        let mut fields = line.split(',').map(|field| field.trim());
        let mut next = || fields.next().expect("a design row has five fields");
        let dx: i32 = next().parse().expect("dx");
        let dy: i32 = next().parse().expect("dy");
        let dz: i32 = next().parse().expect("dz");
        let graphic: u16 = next().parse().expect("graphic");
        let flags: u64 = next().parse().expect("flags");
        // The same skip the client makes: a component the house does not draw is
        // not in anybody's way either. See `multi::Component::drawn`.
        if flags == 0 {
            continue;
        }
        let Ok(x) = u16::try_from(i32::from(origin.x) + dx) else {
            continue;
        };
        let Ok(y) = u16::try_from(i32::from(origin.y) + dy) else {
            continue;
        };
        let Ok(z) = i8::try_from(i32::from(origin.z) + dz) else {
            continue;
        };
        let tile = tiles.static_tile(graphic);
        let laid = openshard_map::overlay::Cover::of_static(tile).based_at(z);
        // A leaf is marked as one, because a client plans its own route through
        // a shut door it is going to open — `Doors::AllOpen`, and the whole
        // reason the session's plans reached a roof behind one. The tiledata
        // flag rather than `client/render`'s open/shut table: which of a pair a
        // graphic is does not matter to a step that opens either.
        let laid = match tile.flags.has(openshard_tiles::TileFlags::DOOR) {
            true => laid.as_door(),
            false => laid,
        };
        covers.entry(Tile::new(x, y)).or_default().extend(laid);
    }
    for (tile, covers) in covers {
        overlay.set(tile, covers);
    }
}

/// The report itself: the castle, the click on its roof, and the two
/// neighbouring tiles whose plans each began by stepping onto the other.
///
/// `docs/world/README.md`'s finding 23, reproduced. The session's journal has
/// the plan from `(1345, 1918)` starting south west and the plan from
/// `(1344, 1919)` starting north east, for 126 plans on one click, and the stall
/// patience never ended the order because the body was moving the whole time.
#[test]
#[ignore = "reads a client install and lays a castle over it — see the doc comment"]
fn a_route_onto_a_castle_roof_never_visits_a_place_twice() {
    let Some((facet, coarse)) = real_facet() else {
        eprintln!("OPENSHARD_CLIENT is unset — nothing to walk");
        return;
    };
    let Some(graph) = coarse else {
        eprintln!("no coarse graph — a long route cannot be planned at all");
        return;
    };
    let terrain = facet.terrain();
    let mut overlay = Overlay::default();
    place_castle(&mut overlay, &facet.tiles, CASTLE_AT);
    // The reading the session's own steps were decided by: a living player with
    // the auto-door setting on walks its route through a shut leaf, because
    // `walk` sends the use before the step. See `client/app`'s `walking_doors`.
    let live = Footing::new(Some(terrain), &overlay, Doors::AllOpen);
    let nothing_placed = Overlay::default();
    let guide = Footing::new(Some(terrain), &nothing_placed, Doors::AsTheyStand);
    // The scene is the reported one only while the roof is really up there: a
    // castle laid at the wrong origin resolves the click to the street, and the
    // whole test would then pass about nothing.
    assert_eq!(
        destination_place(&live, SOUTH, CASTLE_ROOF),
        CASTLE_ROOF,
        "the castle was not laid where the click landed on it",
    );

    // What the corridor is measured against: the same walk asked of an exact
    // search with a budget nobody plays at. A hierarchy is allowed to be longer
    // than the exact answer — that is what buys the speed — and how much longer
    // is the number this prints rather than pins.
    let started = std::time::Instant::now();
    let exact = search_path(&live, SOUTH, CASTLE_ROOF, 200_000, Weight::PLANNING);
    let exact_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let started = std::time::Instant::now();
    let corridor = find_long_path(
        &guide,
        &live,
        &graph,
        SOUTH,
        CASTLE_ROOF,
        PLAN_BUDGET,
        Weight::PLANNING,
    );
    let corridor_ms = started.elapsed().as_secs_f64() * 1_000.0;
    println!(
        "from ({}, {}): exact arrived={} nodes={} steps={} in {exact_ms:.1} ms; corridor steps={} in \
         {corridor_ms:.1} ms",
        SOUTH.x,
        SOUTH.y,
        exact.arrived,
        exact.explored,
        exact.route.len(),
        corridor.as_ref().map_or(0, Vec::len),
    );

    let (mut planned, mut looped) = (0u32, 0u32);
    let mut reports = Vec::new();
    // Where each start's route goes first, which is the whole of what a walking
    // body ever acts on: it takes one step and plans again.
    let mut first_step: HashMap<Point, Point> = HashMap::new();
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let x = SOUTH.x.saturating_add_signed(dx as i16);
            let y = SOUTH.y.saturating_add_signed(dy as i16);
            let Some(from) = terrain.can_step(Point::new(x, y, SOUTH.z), Point::new(x, y, SOUTH.z)) else {
                println!("  ({x}, {y}): nothing stands here");
                continue;
            };
            let (route, exit) = openshard_movement::search_long_path(
                &guide,
                &live,
                &graph,
                from,
                CASTLE_ROOF,
                PLAN_BUDGET,
                Weight::PLANNING,
            );
            let Some(route) = route else {
                println!("  ({x}, {y}): no route onto the roof — {exit:?}");
                continue;
            };
            planned += 1;
            let places = walked(&live, from, &route);
            println!(
                "  from ({}, {}, {}): {} steps, first four {:?}",
                from.x,
                from.y,
                from.z,
                route.len(),
                &places[..places.len().min(5)],
            );
            if let Some(&next) = places.get(1) {
                first_step.insert(from, next);
            }
            if let Some((place, first, again)) = loop_in(&places) {
                looped += 1;
                reports.push(format!(
                    "from ({}, {}, {}): at ({}, {}, {}) as step {first} and step {again}",
                    from.x, from.y, from.z, place.x, place.y, place.z,
                ));
            }
        }
    }
    println!("routes onto the roof: {planned}, of which {looped} stand somewhere twice");
    for report in &reports {
        println!("  {report}");
    }
    assert_eq!(looped, 0, "a route onto the roof walks a loop that leads nowhere",);

    // And the report as a body meets it: an order is walked one step at a time,
    // replanning from wherever that step landed. Two starts whose plans each
    // begin by stepping onto the other is a walk that never ends and never
    // stalls — the patience cannot see it, because the body is moving.
    let mut swapped = Vec::new();
    for (&from, &next) in &first_step {
        if first_step.get(&next) == Some(&from) {
            swapped.push(format!(
                "({}, {}, {}) steps onto ({}, {}, {}) and back",
                from.x, from.y, from.z, next.x, next.y, next.z,
            ));
        }
    }
    for report in &swapped {
        println!("  {report}");
    }
    assert!(
        swapped.is_empty(),
        "two neighbouring starts plan onto each other, which is a walk with no end",
    );
}
