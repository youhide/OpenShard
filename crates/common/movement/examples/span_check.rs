//! The step rule over the bake, against the step rule over the map, on a whole
//! facet.
//!
//! ```sh
//! cargo run --release -p openshard-movement --example span_check -- \
//!   --client "/path/to/Ultima Online Classic"
//! ```
//!
//! `docs/map/navigation_spans.md`'s N2 is done when
//! [`Spans::check`](openshard_movement::spans::Spans::check) answers exactly
//! what [`MapTerrain::check`](openshard_movement::MapTerrain::check) answers, and
//! that is two questions rather than one:
//!
//! - **Per-step agreement.** Out of every surface of every column, in all eight
//!   directions, the *landing height* must be the same number — not merely
//!   whether a landing exists. This is the fine-grained oracle and it names the
//!   column that disagrees.
//! - **Whole-facet flood equivalence.** The breadth-first flood
//!   [`coarse_bench`](coarse_bench.rs) uses as its ground truth, run over both
//!   rules from one origin, must reach the identical set of tiles. This is the
//!   coarse oracle and it is the one that would have caught the one-storey
//!   defect: a rule can agree on every step it is asked about and still change
//!   which tiles the world is *made of*, if the steps nobody asked about are the
//!   ones that changed.
//!
//! Both are here rather than in the suite for the reason
//! [`span_index`](span_index.rs) gives: a facet is 29.4 million columns, which is
//! seconds in release and minutes in debug. `spans.rs` carries the per-step
//! oracle over a box of Britain, which is what a machine with an install runs on
//! every `cargo test`.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use openshard_map::grid::Tile;
use openshard_map::map::WorldMap;
use openshard_movement::spans::{SpanIndex, Spans};
use openshard_movement::{MapTerrain, step_from};
use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    #[arg(long, default_value_t = 0)]
    facet: u8,
    /// Where the flood starts — `coarse_bench`'s own origin, so the component
    /// this walks is the one every other measurement in this crate is quoted
    /// against.
    #[arg(long, default_value_t = 1363)]
    x: u16,
    #[arg(long, default_value_t = 1600)]
    y: u16,
    /// Compare every `stride`-th column of the per-step sweep. One is the whole
    /// facet, which is what the node is done against.
    #[arg(long, default_value_t = 1)]
    stride: u16,
    /// How many disagreeing steps to print before going quiet.
    #[arg(long, default_value_t = 10)]
    report: usize,
}

/// The eight tiles one step away, in [`Direction::ALL`]'s own order.
fn neighbour(x: u16, y: u16, direction: Direction) -> Option<(u16, u16)> {
    let (dx, dy) = direction.step();
    let x = u16::try_from(i32::from(x) + dx).ok()?;
    let y = u16::try_from(i32::from(y) + dy).ok()?;
    Some((x, y))
}

/// The whole step rule, with the landing half answered by the *map* — which is
/// [`MapTerrain::check`], and no longer what `step_allowed` asks.
///
/// **Both sides of this oracle are written out here, and since N3 that is
/// forced rather than stylistic.** The flood below used to walk one side
/// through the shipped `step_allowed`; that now reads the bake, so a flood
/// through it would be the bake compared against itself. Writing both out keeps
/// the corner rule identical on both sides and leaves exactly one difference:
/// which `check` answers the landing.
fn map_step(terrain: &MapTerrain<'_>, from: Point, direction: Direction) -> Option<Point> {
    if direction.is_diagonal() {
        let bits = direction.to_bits();
        for flank in [
            Direction::from_bits((bits + 7) % 8),
            Direction::from_bits((bits + 1) % 8),
        ] {
            map_land(terrain, from, flank)?;
        }
    }
    map_land(terrain, from, direction)
}

/// Where one step onto the neighbouring tile lands, over the map — the mirror
/// of [`span_land`], differing in one call.
fn map_land(terrain: &MapTerrain<'_>, from: Point, direction: Direction) -> Option<Point> {
    let to = step_from(from, direction)?;
    let start_z = i32::from(from.z);
    let (_, start_top) = terrain.start_surface(from.x, from.y, start_z);
    let z = i8::try_from(terrain.check(to.x, to.y, start_z, start_top)?).ok()?;
    Some(Point::new(to.x, to.y, z))
}

/// The same rule with the landing half answered by the bake.
fn span_step(
    terrain: &MapTerrain<'_>,
    spans: &Spans<'_>,
    from: Point,
    direction: Direction,
) -> Option<Point> {
    if direction.is_diagonal() {
        let bits = direction.to_bits();
        for flank in [
            Direction::from_bits((bits + 7) % 8),
            Direction::from_bits((bits + 1) % 8),
        ] {
            span_land(terrain, spans, from, flank)?;
        }
    }
    span_land(terrain, spans, from, direction)
}

/// Where one step onto the neighbouring tile lands, over the bake.
///
/// `MapTerrain::can_step` with `check` swapped: the start half is still the
/// map's, because a span carries where a body *stands* and not the crest of the
/// art it stands on, which is what `start_surface` returns. That is N3's
/// problem and it is named in the plan.
fn span_land(
    terrain: &MapTerrain<'_>,
    spans: &Spans<'_>,
    from: Point,
    direction: Direction,
) -> Option<Point> {
    let to = step_from(from, direction)?;
    let start_z = i32::from(from.z);
    let (_, start_top) = terrain.start_surface(from.x, from.y, start_z);
    let z = i8::try_from(spans.check(to.x, to.y, start_z, start_top)?).ok()?;
    Some(Point::new(to.x, to.y, z))
}

/// Every tile the origin can walk to, flooded once over the whole facet.
///
/// `coarse_bench`'s `land_component`, with the step rule handed in. Keyed by
/// `(x, y)` and not by the whole point, for that function's own reason: a tile
/// first reached at one height is not explored again at another, which
/// under-counts a gallery over a street and never over-counts. Both rules are
/// flooded the same way, so the under-count is common to them and a difference
/// is a difference in the rule.
fn flood(
    width: u32,
    height: u32,
    origin: Point,
    mut step: impl FnMut(Point, Direction) -> Option<Point>,
) -> Vec<bool> {
    let mut reached = vec![false; (width as usize) * (height as usize)];
    let index = |x: u16, y: u16| (y as usize) * (width as usize) + (x as usize);
    let mut queue = VecDeque::new();
    reached[index(origin.x, origin.y)] = true;
    queue.push_back(origin);
    while let Some(at) = queue.pop_front() {
        for direction in Direction::ALL {
            let Some(next) = step(at, direction) else {
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let tiles = openshard_uofiles::tiledata::load_tiles(cli.client.join("tiledata.mul"))?;
    let map: WorldMap = openshard_uofiles::map::read_facet(&cli.client, cli.facet)?;
    let (width, height) = (map.width(), map.height());

    let started = Instant::now();
    let index = SpanIndex::build(&map, &tiles);
    // The terrain borrows the bake, because a step reads it — and this example
    // is what proves the two rules agree, so it reaches past `can_step` to
    // `MapTerrain::check` on one side and `Spans::check` on the other.
    let terrain = MapTerrain::new(&map, &tiles, &index);
    println!(
        "facet {} {width}x{height}: baked in {:.2}s, {} spans, {} B resident",
        cli.facet,
        started.elapsed().as_secs_f64(),
        index.span_count(),
        index.resident_bytes(),
    );

    // The fine oracle. Every surface of every column is a place a body could be
    // standing, and `start_surface` turns it into the pair `check` reaches its
    // source through — so this is a node expansion out of every standable place
    // on the facet, for both abilities.
    let sweeping = Instant::now();
    let mut compared = 0_u64;
    let mut landed = 0_u64;
    let mut disagreements = 0_u64;
    for swimming in [false, true] {
        let terrain = terrain.swimming(swimming);
        let spans = Spans::new(&map, &index).swimming(swimming);
        for y in (0..height as u16).step_by(usize::from(cli.stride.max(1))) {
            for x in (0..width as u16).step_by(usize::from(cli.stride.max(1))) {
                for start in spans.surfaces(x, y) {
                    let start_z = i32::from(start.stand_z);
                    let (_, start_top) = terrain.start_surface(x, y, start_z);
                    for direction in Direction::ALL {
                        let Some((to_x, to_y)) = neighbour(x, y, direction) else {
                            continue;
                        };
                        let baked = spans.check(to_x, to_y, start_z, start_top);
                        let walked = terrain.check(to_x, to_y, start_z, start_top);
                        compared += 1;
                        landed += u64::from(baked.is_some());
                        if baked != walked {
                            disagreements += 1;
                            if disagreements <= cli.report as u64 {
                                println!(
                                    "  ({to_x}, {to_y}) from ({x}, {y}) z={start_z} \
                                     top={start_top} swimming={swimming}: \
                                     map {walked:?}, bake {baked:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "  {compared} steps compared in {:.1}s ({landed} landed somewhere): \
         {disagreements} disagreements",
        sweeping.elapsed().as_secs_f64(),
    );

    // The coarse oracle. One origin, two floods, one bitmap each.
    let origin = terrain
        .ground_z(Tile::new(cli.x, cli.y))
        .map(|z| Point::new(cli.x, cli.y, z))
        .and_then(|point| terrain.can_step(point, point))
        .ok_or_else(|| format!("nothing stands at ({}, {})", cli.x, cli.y))?;
    let flooding = Instant::now();
    let by_map = flood(width, height, origin, |at, direction| {
        map_step(&terrain, at, direction)
    });
    let map_time = flooding.elapsed();
    let spans = Spans::new(&map, &index);
    let flooding = Instant::now();
    let by_spans = flood(width, height, origin, |at, direction| {
        span_step(&terrain, &spans, at, direction)
    });
    let span_time = flooding.elapsed();
    let map_count = by_map.iter().filter(|reached| **reached).count();
    let span_count = by_spans.iter().filter(|reached| **reached).count();
    println!(
        "  flood from ({}, {}, {}): map {map_count} tiles in {:.1}s, \
         bake {span_count} tiles in {:.1}s",
        origin.x,
        origin.y,
        origin.z,
        map_time.as_secs_f64(),
        span_time.as_secs_f64(),
    );
    let mut differed = 0_u64;
    for (slot, (one, other)) in by_map.iter().zip(by_spans.iter()).enumerate() {
        if one == other {
            continue;
        }
        differed += 1;
        if differed <= cli.report as u64 {
            let (x, y) = ((slot % width as usize) as u16, (slot / width as usize) as u16);
            println!("  ({x}, {y}): map reached {one}, bake reached {other}");
        }
    }
    println!("  {differed} tiles reached by one flood and not the other");

    if disagreements > 0 || differed > 0 {
        return Err(
            format!("{disagreements} steps and {differed} tiles where the bake is not the map").into(),
        );
    }
    Ok(())
}
