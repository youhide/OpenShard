//! How many standable surfaces a facet's columns actually hold.
//!
//! ```sh
//! cargo run --release -p openshard-movement --example span_census -- \
//!   --client "/path/to/Ultima Online Classic"
//! ```
//!
//! The census `docs/map/navigation_spans.md`'s storage decision is taken from.
//! A span grid's whole cost is *how many spans there are* and *how they are
//! addressed*, and both are properties of the map rather than of the design —
//! so they are counted rather than assumed. The distribution matters more than
//! the total: if nearly every column holds one surface, a layout that pays per
//! column is paying for the wrong thing.
//!
//! Water is counted separately because it is a surface only a swimmer stands
//! on, and the plan keeps one artifact for both by flagging the span rather
//! than baking two grids.

use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use openshard_map::map::WorldMap;
use openshard_movement::surfaces::stand_surfaces;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    #[arg(long, default_value_t = 0)]
    facet: u8,
    /// Longest span list to report individually; everything above is one bucket.
    #[arg(long, default_value_t = 16)]
    buckets: usize,
}

/// Which half of the column a surface came from.
///
/// The split the storage decision turns on. A land surface is one
/// `average_land_z` — four corner reads, about 12 ns, no statics walked and no
/// `tiledata` consulted — so storing it buys nothing a lookup does not already
/// give. A static surface is the expensive half and the only half a bake can
/// save.
#[derive(Default)]
struct Split {
    land: u64,
    statics: u64,
    /// Columns holding at least one static surface.
    columns: u64,
    /// Columns holding a static surface *and* a land one under it.
    mixed: u64,
    /// Columns with no statics at all — nothing to stand on above the ground,
    /// and nothing to bump into. The only population a step rule can answer
    /// from the land grid alone.
    bare: u64,
    /// Blocks whose whole 8x8 holds no statics.
    bare_blocks: u64,
}

/// The block size the map itself is stored in, which a span grid mirrors.
const BLOCK: u32 = 8;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let tiles = openshard_uofiles::tiledata::load_tiles(cli.client.join("tiledata.mul"))?;
    let map: WorldMap = openshard_uofiles::map::read_facet(&cli.client, cli.facet)?;
    let (width, height) = (map.width(), map.height());
    let columns = u64::from(width) * u64::from(height);

    let started = Instant::now();
    let mut histogram = vec![0_u64; cli.buckets + 2];
    let mut spans = 0_u64;
    let mut water_spans = 0_u64;
    let mut longest = (0_usize, 0_u16, 0_u16);
    let blocks_across = width.div_ceil(BLOCK) as usize;
    let mut block_has_span = vec![false; blocks_across * height.div_ceil(BLOCK) as usize];
    let mut surfaces = Vec::new();
    let mut split = Split::default();
    let mut static_block = vec![false; blocks_across * height.div_ceil(BLOCK) as usize];

    for y in 0..height as u16 {
        for x in 0..width as u16 {
            // Walking twice is what separates the two populations: the walker's
            // surfaces, and the ones only a swimmer stands on.
            surfaces.clear();
            surfaces.extend(stand_surfaces(&map, &tiles, x, y, false));
            let walkable = surfaces.len();
            let swimming = stand_surfaces(&map, &tiles, x, y, true).len();
            water_spans += (swimming - walkable) as u64;
            spans += walkable as u64;
            histogram[walkable.min(cli.buckets + 1)] += 1;
            if walkable > longest.0 {
                longest = (walkable, x, y);
            }
            let block = (usize::from(y) / BLOCK as usize) * blocks_across + usize::from(x) / BLOCK as usize;
            if walkable > 0 {
                block_has_span[block] = true;
            }
            // `stand_surfaces` puts the land surface first when there is one,
            // so the rest are the statics' — the same order its own doc pins.
            let land_surface = usize::from(map.land(x, y).is_some_and(|land| {
                let flags = tiles.land(land.tile.0).flags;
                !flags.is_water() && !flags.is_blocking()
            }));
            let bare = map.statics_at(x, y).next().is_none();
            split.bare += u64::from(bare);
            let from_statics = walkable - land_surface;
            split.land += land_surface as u64;
            split.statics += from_statics as u64;
            if from_statics > 0 {
                split.columns += 1;
                split.mixed += land_surface as u64;
                static_block[block] = true;
            }
        }
    }
    let elapsed = started.elapsed();

    let blocks = block_has_span.len();
    let occupied = block_has_span.iter().filter(|&&flag| flag).count();
    println!(
        "facet {} {width}x{height}: {columns} columns, {blocks} blocks of {BLOCK}x{BLOCK}, counted in {:.1}s",
        cli.facet,
        elapsed.as_secs_f64(),
    );
    println!(
        "  walker spans      {spans} ({:.2} per column, {:.1}% of columns hold at least one)",
        spans as f64 / columns as f64,
        100.0 * (columns - histogram[0]) as f64 / columns as f64,
    );
    println!(
        "  swimmer-only      {water_spans} more ({:.1}% on top of the walker's)",
        100.0 * water_spans as f64 / spans.max(1) as f64,
    );
    println!(
        "  blocks with any   {occupied} of {blocks} ({:.1}%)",
        100.0 * occupied as f64 / blocks as f64,
    );
    println!(
        "  longest column    {} spans at ({}, {})",
        longest.0, longest.1, longest.2
    );
    let static_blocks = static_block.iter().filter(|&&flag| flag).count();
    println!(
        "  of the walker spans: {} are the land surface, {} come from statics ({:.1}%)",
        split.land,
        split.statics,
        100.0 * split.statics as f64 / spans.max(1) as f64,
    );
    println!(
        "  columns with any static surface: {} ({:.2}% of all columns); {} of them also have ground",
        split.columns,
        100.0 * split.columns as f64 / columns as f64,
        split.mixed,
    );
    println!(
        "  blocks with any static surface:  {static_blocks} of {blocks} ({:.1}%)",
        100.0 * static_blocks as f64 / blocks as f64,
    );
    for by in 0..height.div_ceil(BLOCK) as u16 {
        for bx in 0..blocks_across as u16 {
            let bare = (0..BLOCK as u16).all(|dy| {
                (0..BLOCK as u16).all(|dx| {
                    map.statics_at(bx * BLOCK as u16 + dx, by * BLOCK as u16 + dy)
                        .next()
                        .is_none()
                })
            });
            split.bare_blocks += u64::from(bare);
        }
    }
    println!(
        "  columns with NO statics at all:  {} ({:.1}%); whole blocks with none: {} ({:.1}%)",
        split.bare,
        100.0 * split.bare as f64 / columns as f64,
        split.bare_blocks,
        100.0 * split.bare_blocks as f64 / blocks as f64,
    );

    println!("  spans per column:");
    let mut cumulative = 0_u64;
    for (count, &columns_with) in histogram.iter().enumerate() {
        if columns_with == 0 {
            continue;
        }
        cumulative += columns_with;
        let label = match count > cli.buckets {
            true => format!(">{}", cli.buckets),
            false => count.to_string(),
        };
        println!(
            "    {label:>4}  {columns_with:>12}  {:6.2}%  (cumulative {:6.3}%)",
            100.0 * columns_with as f64 / columns as f64,
            100.0 * cumulative as f64 / columns as f64,
        );
    }
    Ok(())
}
