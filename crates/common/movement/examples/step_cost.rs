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
//!
//! # Raise `--repeat` on a busy machine, and say what you raised it to
//!
//! Every row is the *least* of `--repeat` passes, which is the right estimator
//! under load — the fastest pass is the least disturbed one — but only once
//! there are enough passes for one of them to run clean. The default five is
//! enough on a quiet machine and is not enough on a loaded one: at load average
//! 33 on 24 cores it moved rows by 30% run to run and produced a stable-looking
//! reading that `--repeat 25` does not reproduce, which is how
//! `navigation_spans.md`'s *baked adjacency* entry briefly recorded two tiers as
//! costing the same when one of them is 23 ns cheaper. **Take three runs and
//! quote the least**, and put the repeat count next to any number kept.

use std::hint::black_box;
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
use openshard_movement::spans::Spans;
use openshard_movement::{
    Footing,
    MapTerrain,
    SearchExit,
    Weight,
    bake,
    search_path,
    step_allowed,
    steps_out_of,
};
use openshard_protocol::direction::Direction;
use openshard_protocol::world::{
    Facet,
    Point,
};

#[derive(Debug, Parser)]
struct Cli {
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    #[arg(long, default_value_t = 1500)]
    x:      u16,
    #[arg(long, default_value_t = 1900)]
    y:      u16,
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
    let baking = Instant::now();
    let ground = bake::open_facet(&cli.client, bake::WorldSource::Install, Facet(0))?;
    let map = ground.world.snapshot.map();
    let index = &ground.spans;
    println!(
        "read and baked {} spans in {:.2}s, {} B resident",
        index.span_count(),
        baking.elapsed().as_secs_f64(),
        index.resident_bytes(),
    );
    let terrain = ground.terrain();
    // The map and nothing over it: this probe is about what the *ground* costs
    // to ask, so the live world is empty on purpose.
    let nothing_placed = Overlay::default();
    let footing = Footing::new(Some(terrain), &nothing_placed, Doors::AsTheyStand);

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

    // The two tiers, kept apart, because they are answered by different code at
    // different costs and *any* per-span structure can only ever address one of
    // them. `navigation_spans.md`'s *baked adjacency* is the case in point: a
    // neighbour mask hangs on a span, and 92% of the facet's columns have no
    // span to hang one on. What that entry is worth is this split times the gap
    // between the two rows below, so the split is measured rather than assumed.
    let (stored, bare): (Vec<Point>, Vec<Point>) = standing.iter().partition(|p| index.stores(map, p.x, p.y));
    println!(
        "  of which {} stand on a stored column ({:.1}%) and {} on bare land ({:.1}%)",
        stored.len(),
        100.0 * stored.len() as f64 / n as f64,
        bare.len(),
        100.0 * bare.len() as f64 / n as f64,
    );
    // How many of a node's eight neighbours the rule accepts. A *rejection*
    // mask — one bit per direction and no landing height — is the cheap half of
    // baked adjacency: it cannot answer where a step lands, but it can say which
    // neighbours are not worth reading at all, and what that is worth is exactly
    // the refused share of the eight scattered column reads the row below
    // measures. Reported per tier, since only one of them could carry a mask.
    for (label, tier) in [("a stored column", &stored), ("bare land", &bare)] {
        if tier.is_empty() {
            continue;
        }
        let allowed: usize = tier
            .iter()
            .map(|&p| steps_out_of(&footing, p).iter().flatten().count())
            .sum();
        println!(
            "  from {label}: {:.2} of 8 neighbours allowed, {:.0}% refused",
            allowed as f64 / tier.len() as f64,
            100.0 - 100.0 * allowed as f64 / (8.0 * tier.len() as f64),
        );
    }

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
    // The *start* half of a step, asked once per node expansion where the
    // landing half is asked eight or sixteen times. `navigation_spans.md`'s N3
    // decides whether it is worth baking, and this row is that decision.
    measure(cli.repeat, n, "terrain.start_surface (the start)", || {
        standing
            .iter()
            .map(|p| terrain.start_surface(p.x, p.y, i32::from(p.z)).1 as u64)
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
    // What `search` does per popped tile since `navigation_spans.md`'s N3: one
    // call, which resolves the tile being stepped off once and answers each
    // neighbour once. Every row below is the same eight answers by another
    // route, and they all carry the same checksum.
    measure(cli.repeat, n, "steps_out_of (a whole node)", || {
        standing
            .iter()
            .map(|&p| {
                steps_out_of(black_box(&footing), black_box(p))
                    .into_iter()
                    .flatten()
                    .map(|p| u64::from(p.x))
                    .sum::<u64>()
            })
            .sum()
    });
    // The same expansion, over each tier alone. The gap between these two rows
    // is the *whole* of what a per-span neighbour mask could ever recover, and
    // only over the tier that has spans: a bare column's expansion is already
    // four corner reads and a compare, and giving it a mask would mean a dense
    // array over all 29.4 M columns — which is the same 29.4 MB the plan's
    // *dense `average_land_z`* entry priced and declined.
    if !stored.is_empty() {
        measure(cli.repeat, stored.len(), "  ... from a stored column", || {
            stored
                .iter()
                .map(|&p| {
                    steps_out_of(black_box(&footing), black_box(p))
                        .into_iter()
                        .flatten()
                        .map(|p| u64::from(p.x))
                        .sum::<u64>()
                })
                .sum()
        });
    }
    if !bare.is_empty() {
        measure(cli.repeat, bare.len(), "  ... from bare land", || {
            bare.iter()
                .map(|&p| {
                    steps_out_of(black_box(&footing), black_box(p))
                        .into_iter()
                        .flatten()
                        .map(|p| u64::from(p.x))
                        .sum::<u64>()
                })
                .sum()
        });
    }
    // The same eight answers asked one direction at a time, which is what a
    // search did before N3 — and what a caller that wants exactly one direction
    // still pays, since `step_allowed` is now one slot of the row above. Eight
    // of those is eight expansions, so this row is the *price of asking singly*
    // rather than a slower rule.
    measure(cli.repeat, n, "8 x step_allowed (singly)", || {
        standing
            .iter()
            .map(|&p| {
                Direction::ALL
                    .into_iter()
                    .filter_map(|d| step_allowed(black_box(&footing), black_box(p), d))
                    .map(|p| u64::from(p.x))
                    .sum::<u64>()
            })
            .sum()
    });
    // The landing half derived from the column's statics, which is what
    // `MapTerrain::check` still does — the rule the bake is proved equal to, and
    // the number the bake is worth measuring against.
    measure(cli.repeat, n, "the same, landings over the map", || {
        standing.iter().map(|&p| hoisted_expand(&terrain, p)).sum()
    });
    // The floor a per-span neighbour structure could ever reach: the same
    // expansion with the landing half *free*. Nothing is looked up at all, so
    // what is left on the clock is what a baked mask cannot remove — the tile
    // being stepped off, resolved once, and the flank rule over eight slots.
    //
    // Its checksum differs from every row above on purpose: with every landing
    // accepted, every direction is allowed, and that is what makes it a floor
    // rather than an answer.
    measure(cli.repeat, n, "the floor: landings free", || {
        standing
            .iter()
            .map(|&p| {
                expand(
                    p,
                    |to, start_z, _| {
                        let _ = black_box(to);
                        Some(start_z)
                    },
                    terrain.start_surface(p.x, p.y, i32::from(p.z)).1,
                )
            })
            .sum()
    });
    // And the same again with the *landing* half answered off the span bake —
    // `navigation_spans.md`'s N2. The start half is still the map's: a span
    // carries where a body stands and not the crest of the art under it, which
    // is what `start_surface` returns, and N3 measured that half at a seventh of
    // an expansion and left it there.
    let spans = Spans::new(map, index);
    measure(cli.repeat, n, "the same, landings off the bake", || {
        standing.iter().map(|&p| span_expand(&terrain, &spans, p)).sum()
    });
    // The same eight lookups aimed at *one* column instead of eight. Identical
    // work per call — the same `check`, the same tier, the same arithmetic — and
    // the only thing taken away is that each neighbour resolves its own block:
    // `extent().index_of`, `blocks`, `tables`, the occupancy word and the prefix
    // sum, walked eight times for tiles that share a block whenever the node is
    // not on a block edge. The gap between this row and the one above is what a
    // hoist could recover, and it costs no bytes to recover.
    //
    // A different checksum, necessarily: eight answers about one column are one
    // answer eight times. This prices the *addressing*, not the rule.
    measure(cli.repeat, n, "the same, all eight on one column", || {
        standing
            .iter()
            .map(|&p| {
                expand(
                    p,
                    |to, start_z, start_top| {
                        let _ = black_box(to);
                        spans.check(p.x, p.y, start_z, start_top)
                    },
                    terrain.start_surface(p.x, p.y, i32::from(p.z)).1,
                )
            })
            .sum()
    });
    println!("what the search itself costs, with terrain taken away:");
    // Open ground with no map at all and nothing on it: the floor under every
    // measurement above. A search here pops its whole budget — the goal is two
    // thousand tiles away and the budget is hundreds — while a step is a hash
    // miss and nothing else, so what is left on the clock is the *search*: a
    // binary heap, two `FxHashMap`s and a closed set. Terrain work is what a
    // span grid shrinks; this is what it shrinks it **towards**, and a ratio
    // quoted without it is a ratio that ignores its own limit.
    let nothing = Overlay::default();
    let plain = Footing::new(None, &nothing, Doors::AsTheyStand);
    let from = Point::new(2000, 2000, 0);
    let to = Point::new(4000, 2000, 0);
    for budget in [400_usize, 600] {
        let mut fastest = Duration::MAX;
        let mut explored = 0;
        for _ in 0..cli.repeat.max(1) {
            let started = Instant::now();
            let search = search_path(
                black_box(&plain),
                black_box(from),
                black_box(to),
                budget,
                Weight::EXACT,
            );
            fastest = fastest.min(started.elapsed());
            explored = search.explored;
            assert!(
                matches!(search.exit, SearchExit::Budget),
                "the goal must stay out of reach of the budget"
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
    expand(
        from,
        |to, start_z, start_top| terrain.check(to.x, to.y, start_z, start_top),
        terrain.start_surface(from.x, from.y, i32::from(from.z)).1,
    )
}

/// The same expansion with the landing half read off the span bake.
///
/// Identical in shape to [`hoisted_expand`] on purpose: the two differ in one
/// call and nothing else, so the difference between their rows is the rule and
/// not the harness.
fn span_expand(terrain: &MapTerrain<'_>, spans: &Spans<'_>, from: Point) -> u64 {
    expand(
        from,
        |to, start_z, start_top| spans.check(to.x, to.y, start_z, start_top),
        terrain.start_surface(from.x, from.y, i32::from(from.z)).1,
    )
}

/// One node expansion: eight landings, then the diagonals' flank rule over
/// them.
fn expand(from: Point, mut landing_at: impl FnMut(Point, i32, i32) -> Option<i32>, start_top: i32) -> u64 {
    let start_z = i32::from(from.z);
    // One landing answer per neighbour, in `Direction::ALL` order.
    let mut landing = [None; 8];
    for direction in Direction::ALL {
        let Some(to) = openshard_movement::step_from(from, direction) else {
            continue;
        };
        landing[direction.to_bits() as usize] = landing_at(to, start_z, start_top)
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
