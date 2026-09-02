//! What the span bake costs, and the whole-facet proof that it says the same
//! thing the walk does.
//!
//! ```sh
//! cargo run --release -p openshard-movement --example span_index -- \
//!   --client "/path/to/Ultima Online Classic"
//! ```
//!
//! `docs/world/evidence/2026-08-25-the-span-layer.md`'s N1 is done when
//! `Spans::surfaces(x, y)` returns exactly what
//! [`stand_surfaces`](openshard_movement::surfaces::stand_surfaces) returns for
//! **every** column of facet 0 and for both abilities, and when the built size
//! and build time are measured rather than estimated. Both are this example: a
//! sample would not do, because the whole point of the three tiers is that
//! different columns are answered by different code, and a sample that missed
//! the 0.16% of columns holding three or more surfaces would have proved the
//! easy two thirds.
//!
//! It lives beside [`span_census`](span_census.rs) rather than in the test
//! suite for the reason that census does: 29.4 million columns walked twice by
//! two different implementations is seconds in release and minutes in debug, and
//! `cargo test` is neither. The suite carries the same comparison over a box of
//! Britain — see `spans.rs`.

use std::path::PathBuf;
use std::time::Instant;

use clap::Parser;
use openshard_map::map::WorldMap;
use openshard_movement::spans::{
    SpanIndex,
    Spans,
};
use openshard_movement::surfaces::stand_surfaces;

#[derive(Debug, Parser)]
struct Cli {
    #[arg(short, long, env = "OPENSHARD_CLIENT", value_name = "DIR")]
    client: PathBuf,
    #[arg(long, default_value_t = 0)]
    facet:  u8,
    /// How many disagreeing columns to print before going quiet.
    #[arg(long, default_value_t = 10)]
    report: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let tiles = openshard_uofiles::tiledata::load_tiles(cli.client.join("tiledata.mul"))?;
    let map: WorldMap = openshard_uofiles::map::read_facet(&cli.client, cli.facet)?;
    let (width, height) = (map.width(), map.height());
    let columns = u64::from(width) * u64::from(height);

    let started = Instant::now();
    let index = SpanIndex::build(&map, &tiles);
    let built = started.elapsed();
    println!(
        "facet {} {width}x{height}: {columns} columns baked in {:.2}s",
        cli.facet,
        built.as_secs_f64(),
    );
    let bytes = index.resident_bytes();
    println!(
        "  {} spans over {} columns of {} blocks with statics",
        index.span_count(),
        index.column_count(),
        index.table_count(),
    );
    // What the occupancy mask is worth, printed rather than argued: a dense
    // table addresses every cell of every block that holds a static, and this
    // is how many of those cells actually own a run.
    println!(
        "  {:.1}% of the cells those blocks address own a run",
        100.0 * index.column_count() as f64 / (index.table_count() * 64) as f64,
    );
    println!(
        "  resident {bytes} B ({:.1} MiB), {:.2} bytes per column of the facet",
        bytes as f64 / (1024.0 * 1024.0),
        bytes as f64 / columns as f64,
    );

    // The oracle. Sorted rather than compared in place: the bake stores a column
    // highest-first because that is the order the step rule wants to walk it in,
    // and `stand_surfaces` is in the map file's own order, which is not an order
    // at all — see its own doc comment.
    let checking = Instant::now();
    let mut disagreements = 0_u64;
    let mut derived = Vec::new();
    let mut baked = Vec::new();
    for y in 0..height as u16 {
        for x in 0..width as u16 {
            for swimming in [false, true] {
                let spans = Spans::new(&map, &index).swimming(swimming);
                derived.clear();
                derived.extend(stand_surfaces(&map, &tiles, x, y, swimming));
                derived.sort_unstable();
                baked.clear();
                baked.extend(spans.surfaces(x, y).map(|span| i32::from(span.stand_z)));
                baked.sort_unstable();
                if derived != baked {
                    disagreements += 1;
                    if disagreements <= cli.report as u64 {
                        println!("  ({x}, {y}) swimming={swimming}: walk {derived:?}, bake {baked:?}");
                    }
                }
            }
        }
    }
    println!(
        "  {} columns × 2 abilities compared in {:.1}s: {disagreements} disagreements",
        columns,
        checking.elapsed().as_secs_f64(),
    );
    if disagreements > 0 {
        return Err(format!("{disagreements} columns where the bake is not the walk").into());
    }
    Ok(())
}
