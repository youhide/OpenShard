//! Direction B's real acceptance test: the movement rules over the new source.
//!
//! `openshard-uofiles`' `base_set_import` test asks whether a base set holds the
//! same *world* — the same land cells, the same statics, the same bytes twice.
//! This one asks the question a player would: does the same ground answer the
//! same way. It is a different question because nothing in between is an
//! identity — [`MapTerrain`] reads a tile's flags out of `tiledata.mul`, sorts
//! statics per block, averages four corners for a slope, and walks a Bresenham
//! line for a look. A world that round-trips and a world that *behaves* are two
//! claims, and the second is the one the plan calls B's acceptance test.
//!
//! Skips unless `OPENSHARD_CLIENT` points at a UO client directory, like every
//! other test that needs shipped files. No client files enter this repository.
//!
//! # Why not "run the existing tests over a base set"
//!
//! Because they would then be tests of two things at once, and a failure would
//! not say which. The install-gated terrain tests in `terrain.rs` pin rules —
//! a staircase is climbable, a wall is not walked through. This pins the
//! *source*: whatever those rules answer over the install, they answer the same
//! over the base set, for every question the trait has, at tens of thousands of
//! places. A rule that is wrong is wrong in both columns and this test stays
//! green, which is exactly right — that is `terrain.rs`'s job, not this file's.

use std::path::PathBuf;

use openshard_map::grid::Tile;
use openshard_map::overlay::Doors;
use openshard_movement::spans::SpanIndex;
use openshard_movement::{
    Footing,
    MapTerrain,
};
use openshard_protocol::world::{
    Facet,
    Point,
};

/// The client directory, or `None` to skip.
fn client_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    dir.join("tiledata.mul").exists().then_some(dir)
}

/// How far apart the sampled tiles are.
///
/// Coprime with 8, so consecutive samples land on different cells of a block,
/// and coprime with 64, so they land on different cells of a chunk. A stride
/// that shared a factor with either would walk one lane of the layout and miss
/// whatever the other lanes do.
const STRIDE: usize = 31;

/// What every question was asked about, so a green run cannot be an empty one.
#[derive(Default)]
struct Counts {
    tiles:         u64,
    /// Steps the terrain allowed — a run where everything is refused would
    /// agree perfectly and prove nothing.
    steps_taken:   u64,
    /// Tiles with something standing on them.
    statics:       u64,
    /// Looks the terrain blocked, for the same reason as `steps_taken`.
    sight_blocked: u64,
}

/// Felucca out of the install and Felucca out of a base set answer every
/// terrain question identically.
#[test]
fn a_base_set_walks_and_sees_exactly_as_the_install_does() {
    let Some(dir) = client_dir() else {
        return;
    };
    let facet = Facet(0);
    let installed = openshard_uofiles::map::load_facet(&dir, facet).expect("a readable facet 0");

    let path = std::env::temp_dir().join(format!(
        "openshard-terrain-{}-{}.osbase",
        facet.0,
        std::process::id()
    ));
    openshard_basemap::write(&path, &installed, openshard_basemap::Identity::Mint)
        .expect("a writable temp dir");
    let restored = openshard_basemap::read(&path).expect("the base set we just wrote");
    std::fs::remove_file(&path).ok();

    // One tile table for both: `tiledata.mul` is not what a base set replaces,
    // and giving the two sides different tables would make this a test of the
    // table instead of a test of the map.
    let tiles =
        openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).expect("tiledata should load");
    // A bake each, over each side's own map: a shared one would be a third
    // party both terrains agreed with rather than a projection of the map each
    // is actually reading.
    let was_spans = SpanIndex::build(installed.map(), &tiles);
    let is_spans = SpanIndex::build(restored.map(), &tiles);
    let was: MapTerrain<'_> = MapTerrain::new(installed.map(), &tiles, &was_spans);
    let is: MapTerrain<'_> = MapTerrain::new(restored.map(), &tiles, &is_spans);

    let (width, height) = (restored.map().width() as u16, restored.map().height() as u16);
    assert_eq!(
        (installed.map().width() as u16, installed.map().height() as u16),
        (width, height),
        "the base set came back a different size"
    );

    // A look is asked of the whole footing rather than of the map alone — the
    // rule lives in `sight::trace`, which walks the map and the live world over
    // one ray. Nothing is live here, so what this compares is still the two
    // maps and only the two maps.
    let nothing_placed = openshard_map::overlay::Overlay::default();
    let was_footing = Footing::new(Some(was), &nothing_placed, Doors::AsTheyStand);
    let is_footing = Footing::new(Some(is), &nothing_placed, Doors::AsTheyStand);

    let mut counts = Counts::default();
    // Reused rather than allocated per tile: `statics_at` appends.
    let (mut left, mut right) = (Vec::new(), Vec::new());
    for y in (0..height).step_by(STRIDE) {
        for x in (0..width).step_by(STRIDE) {
            let tile = Tile::new(x, y);
            counts.tiles += 1;

            // What the ground *is* — the question harvesting asks. A pickaxe
            // works a mountain face and a shovel works sand, and the two are
            // told apart by this and by nothing else.
            assert_eq!(
                was.land_tile(tile),
                is.land_tile(tile),
                "the land tile at ({x}, {y})"
            );
            assert_eq!(
                was.ground_z(tile),
                is.ground_z(tile),
                "the ground height at ({x}, {y})"
            );
            assert_eq!(
                was.land_is_water(tile),
                is.land_is_water(tile),
                "whether ({x}, {y}) is water"
            );

            left.clear();
            right.clear();
            was.statics_at(tile, &mut left);
            is.statics_at(tile, &mut right);
            assert_eq!(left, right, "the statics at ({x}, {y})");
            if !left.is_empty() {
                counts.statics += 1;
            }
            // Order, not just membership: the draw order and `statics::pick`
            // both take the last, so two sets that agree as sets and disagree
            // as sequences are two different worlds on the screen.
            assert!(
                left.iter().eq(right.iter()),
                "the statics at ({x}, {y}) came back in another order"
            );

            let Some(ground) = is.ground_z(tile) else {
                continue;
            };
            let z = i32::from(ground);
            assert_eq!(
                was.stand_z(tile, z),
                is.stand_z(tile, z),
                "standing on ({x}, {y})"
            );
            assert_eq!(
                was.spawn_z(tile, z),
                is.spawn_z(tile, z),
                "spawning on ({x}, {y})"
            );
            assert_eq!(
                was.can_fit(tile, z, 16),
                is.can_fit(tile, z, 16),
                "fitting on ({x}, {y})"
            );

            // The step, in all eight directions. `can_step` is the whole of
            // movement: it reads the surface underfoot, every static on the
            // tile stepped to, and the flags of each of them.
            let from = Point { x, y, z: ground };
            for (dx, dy) in [
                (1, 0),
                (1, 1),
                (0, 1),
                (-1, 1),
                (-1, 0),
                (-1, -1),
                (0, -1),
                (1, -1),
            ] {
                let (Some(nx), Some(ny)) = (
                    x.checked_add_signed(dx).filter(|&n| n < width),
                    y.checked_add_signed(dy).filter(|&n| n < height),
                ) else {
                    continue;
                };
                let to = Point {
                    x: nx,
                    y: ny,
                    z: ground,
                };
                let (a, b) = (was.can_step(from, to), is.can_step(from, to));
                assert_eq!(a, b, "the step from ({x}, {y}) to ({nx}, {ny})");
                if a.is_some() {
                    counts.steps_taken += 1;
                }
            }

            // And a look, far enough to cross tiles the sample itself skipped:
            // `sight_clear` walks every tile of the line, so this is the one
            // question here that reads the ground *between* two samples.
            let (Some(tx), Some(ty)) = (
                x.checked_add(12).filter(|&n| n < width),
                y.checked_add(12).filter(|&n| n < height),
            ) else {
                continue;
            };
            let to = Point {
                x: tx,
                y: ty,
                z: ground,
            };
            let (a, b) = (
                openshard_movement::sight_clear(&was_footing, from, to),
                openshard_movement::sight_clear(&is_footing, from, to),
            );
            assert_eq!(a, b, "the look from ({x}, {y}) to ({tx}, {ty})");
            if !a {
                counts.sight_blocked += 1;
            }
        }
    }

    // A run that asked nothing would pass every assertion above. The numbers
    // are floors under what a real Felucca gives — on the shipped one this is
    // 30,856 tiles, 61,264 steps and a quarter of the eight directions being
    // walkable, which is what a facet that is mostly ocean looks like with
    // swimming off.
    eprintln!(
        "sampled {} tiles: {} steps allowed, {} tiles with statics, {} looks blocked",
        counts.tiles, counts.steps_taken, counts.statics, counts.sight_blocked
    );
    assert!(counts.tiles > 30_000, "only {} tiles sampled", counts.tiles);
    assert!(
        counts.steps_taken > 50_000,
        "only {} steps were allowed; the sample is not walking anywhere",
        counts.steps_taken
    );
    assert!(
        counts.statics > 1_000,
        "only {} sampled tiles had statics on them",
        counts.statics
    );
    assert!(
        counts.sight_blocked > 100,
        "only {} looks were blocked; the sample never saw a wall or a hill",
        counts.sight_blocked
    );
}
