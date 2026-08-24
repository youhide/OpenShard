//! What the radar's coarse floor costs, level by level, and what the frame that
//! builds one chunk of it pays.
//!
//! ```text
//! OPENSHARD_CLIENT=/path/to/client \
//!   cargo run --release -p openshard-client-render --example radar_floor_cost
//! ```
//!
//! [`radar::build_chunk`] rasters a chunk's whole tile span out of the map and
//! then reduces it, at any level. A level-`n` chunk covers `4^n · 4096` tiles,
//! so the top of Britannia's ladder is a single chunk covering 8192² of them —
//! one scratch buffer, one walk of more than the whole facet, inside one
//! `App::draw_from`. The producer's budget cannot refuse it either:
//! `take_for_producer_by_cost` always takes at least one key, whatever it costs.
//!
//! That is why `SWEEP_LOD` is a ceiling and not only a floor, and this is the
//! reading the ceiling was chosen from. It prints both paths:
//!
//! - **direct** — walk the map once per level, which is what the sweep used to
//!   ask for and what nothing asks for now;
//! - **climbed** — walk it at `SWEEP_LOD` alone and let
//!   [`radar::build_ready_ancestors`] reduce everything above it.
//!
//! The climb's chunk counts are part of the reading rather than a formality: a
//! level whose chunk count is *odd* asks the level above it for a child past the
//! facet's edge, and Britannia goes odd at level four. Before
//! [`radar::build_lod_parent`] treated an off-facet child as a quadrant of
//! unmapped ground, this table read 6 of 8 chunks at level five, 1 of 2 at six
//! and 0 of 1 at seven.
//!
//! The last table is the one that makes the two paths one product rather than
//! two pictures that resemble each other: every coarse chunk of the shipped
//! facet, climbed, against the same chunk built directly.
//!
//! Sample counts are printed with the timings for the reason `coarse_bench`
//! prints `repeat=`: a default is a claim about the machine the reading was
//! taken on.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use openshard_client_render::radar::{
    self, BASE_CHUNK_TILES, RadarCache, RadarChunkCoord, RadarExtent, RadarLod, RadarRegion, SWEEP_LOD,
};
use openshard_protocol::world::Facet;
use openshard_uofiles::radarcol::RadarColors;

/// How many chunks of one level are timed individually. A level with fewer
/// chunks than this is timed whole.
const DEFAULT_SAMPLES: usize = 3;

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn main() {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT").expect("OPENSHARD_CLIENT"));
    let samples: usize = std::env::args().nth(1).map_or(DEFAULT_SAMPLES, |value| {
        value.parse().expect("samples is a count")
    });
    assert!(samples > 0, "a sample count is positive");

    let map = openshard_uofiles::map::read_facet(&dir, 0).expect("facet 0");
    let colors = RadarColors::load(dir.join("radarcol.mul")).expect("radarcol.mul");
    let facet = Facet(0);
    // The radar addresses a facet in `u16`, which is the map format's own
    // coordinate; a facet outside it has no radar image at all.
    let (width, height) = (
        u16::try_from(map.width()).expect("a facet the radar can address"),
        u16::try_from(map.height()).expect("a facet the radar can address"),
    );
    let extent = RadarExtent::new(width, height).expect("a facet with an extent");
    let whole = RadarRegion::new(facet, radar::RadarTile::new(0, 0), extent);
    let top = radar::max_lod(extent);
    println!(
        "facet 0 {}x{}: SWEEP_LOD={} max_lod={} samples={samples}",
        map.width(),
        map.height(),
        SWEEP_LOD.value(),
        top.value(),
    );

    println!();
    println!("direct — what the shipped sweep builds, one key at a time");
    println!("  lod  chunks  tiles/chunk    scratch    per chunk     level total");
    let mut direct_total = Duration::ZERO;
    for level in SWEEP_LOD.value()..=top.value() {
        let lod = RadarLod::new(level);
        let coords: Vec<RadarChunkCoord> = radar::region_chunks(whole, lod).collect();
        let side = u32::from(BASE_CHUNK_TILES) << u32::from(level);
        let tiles = u64::from(side) * u64::from(side);
        // `build_chunk`'s own scratch, plus the `best_z` column `fill` keeps
        // beside it: two bytes and one byte a tile.
        let scratch = tiles * 3;
        let cache = RadarCache::default();
        let timed: Vec<Duration> = coords
            .iter()
            .take(samples)
            .map(|chunk| {
                let key = cache.key(facet, lod, *chunk);
                let started = Instant::now();
                let built = radar::build_chunk(&map, &colors, key);
                let elapsed = started.elapsed();
                assert!(built.is_some(), "a key inside the facet builds");
                elapsed
            })
            .collect();
        let per_chunk = median(timed);
        let level_total = per_chunk * u32::try_from(coords.len()).expect("a level fits a u32");
        direct_total += level_total;
        println!(
            "  {level:>3}  {:>6}  {tiles:>11}  {:>7} KiB  {per_chunk:>11.2?}  {level_total:>11.2?}",
            coords.len(),
            scratch / 1024,
        );
    }
    println!("  whole floor, direct: {direct_total:.2?} of synchronous frame time");

    println!();
    println!("climbed — the floor at SWEEP_LOD, and the ladder reduced from it");
    let mut cache = RadarCache::default();
    let floor: Vec<RadarChunkCoord> = radar::region_chunks(whole, SWEEP_LOD).collect();
    let started = Instant::now();
    let mut climbed = 0_usize;
    for chunk in &floor {
        let key = cache.key(facet, SWEEP_LOD, *chunk);
        let built = radar::build_chunk(&map, &colors, key).expect("a floor key builds");
        assert!(
            cache.publish(built),
            "the floor is published at the current revision"
        );
        climbed += radar::build_ready_ancestors(&mut cache, key, extent);
    }
    let floor_elapsed = started.elapsed();
    println!(
        "  {} chunks at lod {} in {floor_elapsed:.2?}, and {climbed} ancestors reduced from them",
        floor.len(),
        SWEEP_LOD.value(),
    );
    println!("  lod  chunks  present after the climb");
    for level in SWEEP_LOD.value()..=top.value() {
        let lod = RadarLod::new(level);
        let coords: Vec<RadarChunkCoord> = radar::region_chunks(whole, lod).collect();
        let present = coords
            .iter()
            .filter(|chunk| cache.get(cache.key(facet, lod, **chunk)).is_some())
            .count();
        let verdict = if present == coords.len() {
            "complete"
        } else {
            "INCOMPLETE"
        };
        println!("  {level:>3}  {:>6}  {present:>6}  {verdict}", coords.len());
    }
    println!(
        "  whole floor, climbed: {floor_elapsed:.2?}, retained {} chunks",
        cache.retained_len(),
    );

    // The two paths are one product or they are two pictures that resemble each
    // other, and the second is a thing a person notices as the map changing
    // slightly when a level lands. R2 asserted this on a fixture for `n` up to
    // three; here it is every coarse chunk of the shipped facet, including the
    // ones whose eastern children are ground the facet does not have.
    println!();
    println!("identity — every climbed chunk against the same chunk built directly");
    for level in SWEEP_LOD.value().saturating_add(1)..=top.value() {
        let lod = RadarLod::new(level);
        let coords: Vec<RadarChunkCoord> = radar::region_chunks(whole, lod).collect();
        let mut agree = 0_usize;
        let mut differ = Vec::new();
        for chunk in &coords {
            let key = cache.key(facet, lod, *chunk);
            let direct = radar::build_chunk(&map, &colors, key).expect("a coarse key is addressable");
            match cache.get(key) {
                Some(climbed) if climbed.pixels() == direct.pixels() => agree += 1,
                _ => differ.push(*chunk),
            }
        }
        let verdict = if differ.is_empty() {
            String::from("identical")
        } else {
            format!("DIFFER at {differ:?}")
        };
        println!("  {level:>3}  {agree:>6}/{:<6}  {verdict}", coords.len());
    }
}
