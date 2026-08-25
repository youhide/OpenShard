//! What one `.setland` costs, at each end, on a facet the size of Felucca.
//!
//! `to_the_client.md`'s E4 backlog says it in as many words — *"a publish costs
//! the window a whole facet's rebuild, and nobody has measured it"* — and an
//! unmeasured cost is a cost nobody can rank against the work that would remove
//! it. This is the measurement, split into the pieces a publish is made of,
//! because they do not have the same fix:
//!
//! - **`MapSnapshot::publish`** — the shard's own change. It touches the tiles
//!   the ops name and nothing else, which is `plan.md`'s "a publish never
//!   rebuilds a facet" taken literally. The floor.
//! - **`SpanIndex::build`** — the rebake, paid at *both* ends:
//!   [`Ground::publish`] on the shard and [`Ground::take_chunks`] in the window.
//!   The whole facet, because the bake has no seam smaller than one.
//! - **`chunk::apply`** — the window's own, which the shard never pays. A
//!   block's statics are one run in a facet-wide vector, so a chunk whose item
//!   count moved moves every static after it: one memmove of the tail, made once
//!   for the whole set. It used to rebuild the facet instead — 15.3 ms here, and
//!   a second 150 MiB facet resident while it ran — on the argument that the
//!   tail copy was unavoidable anyway. It is; what the rebuild *added* to it was
//!   the 117 MiB of land no splice touches and a re-sort of all 458,752 blocks
//!   rather than the sixty-four that arrived.
//!
//!   The line below is the cheap half of that, and deliberately: a `.setland`
//!   moves no statics, so the chunk it sends holds exactly as many as the one it
//!   replaces and the tail does not move at all. A publish that *added* an item
//!   is 3.9–5.6 ms — see `WorldMap::replace_blocks`, which has both numbers and
//!   why the second one is mostly neither the span nor the tail.
//! - **`Chunk::of`** — cutting the square out on the shard, for scale.
//!
//! # Why this is a test and not a bench
//!
//! `cargo bench` builds under `profile.bench`, which is `release` — and the
//! number a person is complaining about is the one a **`cargo run`** produces. A
//! bench could only ever report the column nobody plays in.
//!
//! That distinction is the whole reason this file exists rather than a note in a
//! document. `[profile.dev.package."*"]` reaches *dependencies* and not workspace
//! members, so `openshard-map`, `openshard-movement` and `openshard-tiles` — the
//! three crates a publish is made of — were compiled at `opt-level = 0` in every
//! debug build, and one `.setland` was **2.5 seconds** of stall split between the
//! shard's tick and the window's event-loop thread. The root `Cargo.toml` now
//! names them, with these numbers in the comment that does it. Run this under
//! both profiles when they are doubted.
//!
//! ```sh
//! OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
//!     cargo test -p openshard-movement --test publish_cost -- --ignored --nocapture
//! OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
//!     cargo test --release -p openshard-movement --test publish_cost -- --ignored --nocapture
//! ```
//!
//! `#[ignore]`d because it reads a 100 MB base set and rebuilds a facet several
//! times: it is a measurement somebody asks for, not an oracle. It asserts
//! nothing about the numbers, because a threshold here would be a test of the
//! machine it ran on.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use openshard_map::chunk::{Chunk, ChunkCoord};
use openshard_map::map::LandCell;
use openshard_map::patch::{Patch, PatchAuthor, PatchOp, PatchTime};
use openshard_movement::ground::Ground;
use openshard_movement::spans::SpanIndex;
use openshard_tiles::{LandTileId, TileData};

/// The base set to measure over: `OPENSHARD_BASE_SET`, or the one the repo's own
/// `openshard.toml` names, which is where `openshard-map-import` puts it.
fn base_set() -> Option<PathBuf> {
    let path = match std::env::var_os("OPENSHARD_BASE_SET") {
        Some(named) => PathBuf::from(named),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../felucca.osbase"),
    };
    path.exists().then_some(path)
}

/// `tiledata.mul`, which is not what a base set replaces — the bake reads a
/// tile's flags out of it, so there is no measuring the bake without one.
fn tile_data() -> Option<TileData> {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).ok()
}

/// How many times a repeatable measurement is taken. The fastest is reported:
/// it is the one least polluted by whatever else this machine was doing.
const RUNS: u32 = 3;

/// Time `body` [`RUNS`] times and hand back the fastest.
///
/// The value is dropped after the clock is read, so a destructor walking a
/// facet-sized vector is not counted as the work that built it.
fn best_of<T>(mut body: impl FnMut() -> T) -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..RUNS {
        let start = Instant::now();
        let value = body();
        let took = start.elapsed();
        drop(value);
        best = best.min(took);
    }
    best
}

/// Milliseconds to one decimal, which is the resolution a person watching a
/// frame can tell apart.
fn ms(took: Duration) -> String {
    format!("{:>8.1} ms", took.as_secs_f64() * 1000.0)
}

/// The tile the sample edit lands on — `openshard.toml`'s own start, which is
/// somewhere inside the facet rather than at its origin. A cost that only shows
/// on block zero is not a cost.
const AT: (u16, u16) = (1363, 1600);

#[test]
#[ignore = "reads a 100 MB base set and rebuilds a facet; a measurement, not an oracle"]
fn what_one_setland_costs_at_each_end() {
    let (Some(path), Some(tiles)) = (base_set(), tile_data()) else {
        eprintln!(
            "skipping: needs a base set (OPENSHARD_BASE_SET, or felucca.osbase beside \
             openshard.toml) and OPENSHARD_CLIENT for tiledata.mul"
        );
        return;
    };

    let read = Instant::now();
    let mut snapshot = openshard_basemap::load(&path)
        .expect("a readable base set")
        .snapshot;
    let read = read.elapsed();

    let facet = snapshot.facet();
    let extent = snapshot.map().extent();
    let (x, y) = AT;
    println!(
        "facet {} at revision {}: {}x{} blocks, {} tiles, {} statics — read in {}",
        facet.0,
        snapshot.revision().get(),
        extent.wide,
        extent.down,
        u64::from(snapshot.map().width()) * u64::from(snapshot.map().height()),
        snapshot.map().static_count(),
        ms(read).trim()
    );

    // ---- the shard's half -------------------------------------------------

    let was = snapshot
        .map()
        .land(x, y)
        .expect("the sample tile is on this facet");
    let raise = |parent, was: LandCell, now: LandCell| {
        Patch::new(
            facet,
            parent,
            PatchAuthor("measurement".to_owned()),
            PatchTime(0),
            vec![PatchOp::SetLand { x, y, was, now }],
        )
    };
    let raised = LandCell {
        tile: LandTileId(3),
        z: was.z.saturating_add(40),
    };

    // Not repeatable — the world it publishes into is the world it moved — so
    // this one is a single clock rather than a best-of.
    let patch = raise(snapshot.revision(), was, raised);
    let start = Instant::now();
    snapshot.publish(&patch).expect("the sample patch applies");
    let publish = start.elapsed();

    // What a facet-wide bake costs, which is what both ends paid for one tile
    // until `navigation_spans.md`'s N8. Kept as the column the two numbers below
    // are read against: it is the same work, over 7,168 chunks instead of one.
    let bake = best_of(|| SpanIndex::build(snapshot.map(), &tiles));

    // The square the shard cuts and sends, out of the world after the edit.
    let at = ChunkCoord::containing(x, y);
    let cut = best_of(|| Chunk::of(&snapshot, at).expect("the sample chunk is on this facet"));
    let chunks = vec![Chunk::of(&snapshot, at).expect("the sample chunk is on this facet")];

    // Repeatable because the square being written in is the square that is
    // already there: the chunk was cut out of this very world, so every round
    // moves the same blocks the same amount and leaves the same facet behind.
    let written = best_of(|| snapshot.take_chunks(&chunks).expect("one chunk of this facet"));

    // `Ground::new` bakes, and that bake is outside the clock: what is being
    // measured is what a *publish* costs an end that is already running.
    let mut ground = Ground::new(Some(snapshot), &tiles);

    // The window's half: a chunk arrives, and the bake follows it.
    let take = best_of(|| {
        ground
            .take_chunks(&chunks, &tiles)
            .expect("one chunk of this facet")
    });

    // The shard's half, through the door a `.setland` actually goes through.
    // The patch is rebuilt each round against the world the round before left,
    // so every round moves the same one tile of the same one chunk — and the
    // world it publishes into is a world one revision further along, which is
    // exactly what a second `.setland` is.
    let mut z = raised.z;
    let published = best_of(|| {
        let base = ground.snapshot().expect("the facet it was built with");
        let was = base.map().land(x, y).expect("the sample tile is on this facet");
        z = z.wrapping_add(1);
        let patch = raise(base.revision(), was, LandCell { tile: was.tile, z });
        ground.publish(&patch, &tiles).expect("the sample patch applies")
    });

    println!("\nthe shard, on the tick the operator typed into:");
    println!("  MapSnapshot::publish — the tiles the op names {}", ms(publish));
    println!(
        "  SpanIndex::build     — the whole facet        {}   (what it used to pay)",
        ms(bake)
    );
    println!(
        "  = Ground::publish    — publish and rebake     {}",
        ms(published)
    );

    println!("\nthe wire:");
    println!("  Chunk::of            — one square, cut        {}", ms(cut));

    println!("\nthe window, on the frame the publish landed — the event-loop thread:");
    println!("  chunk::apply         — the squares written in {}", ms(written));
    println!(
        "  SpanIndex::build     — the facet rebaked      {}   (what it used to pay)",
        ms(bake)
    );
    println!("  = Ground::take_chunks                         {}", ms(take));
    println!("\nthe hitch a person sees is the last line.");
}
