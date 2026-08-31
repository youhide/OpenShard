//! N8's oracle on a real facet: **a facet patched into shape answers what the
//! same facet baked whole answers.**
//!
//! `docs/map/navigation_spans.md`'s N8 names this as the one of its three "done
//! when"s that is not a number, and says what it is for: *"what it catches is a
//! column reading its neighbour's run — the failure this layer's packing has
//! always been one mistake away from, and which the bake's own comment says
//! would be silent."*
//!
//! The unit tests beside [`SpanIndex`] ask the same question over scenes, where
//! every column is a fixture somebody wrote. This one asks it over Britannia,
//! where the populations the layout was chosen off actually live: twelve-span
//! columns, blocks that are 82% empty tables, and the seams where a chunk's
//! edge is also a block's.
//!
//! ```sh
//! OPENSHARD_CLIENT="/path/to/Ultima Online Classic" \
//!     cargo test -p openshard-movement --test publish_locality -- --ignored --nocapture
//! ```
//!
//! `#[ignore]`d for the reason `publish_cost` is: it reads a 100 MB base set and
//! bakes a facet.

use std::path::PathBuf;

use openshard_map::map::{
    LandCell,
    StaticItem,
    WorldMap,
};
use openshard_map::patch::{
    Patch,
    PatchAuthor,
    PatchOp,
    PatchTime,
};
use openshard_movement::ground::Ground;
use openshard_movement::spans::{
    Span,
    SpanIndex,
    Spans,
};
use openshard_protocol::world::Facet;
use openshard_tiles::TileData;

/// The base set to patch: `OPENSHARD_BASE_SET`, or the one the repo's own
/// `openshard.toml` names.
fn base_set() -> Option<PathBuf> {
    let path = match std::env::var_os("OPENSHARD_BASE_SET") {
        Some(named) => PathBuf::from(named),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../felucca.osbase"),
    };
    path.exists().then_some(path)
}

/// `tiledata.mul`, which a base set does not replace — the bake reads a tile's
/// flags out of it.
fn tile_data() -> Option<TileData> {
    let dir = PathBuf::from(std::env::var_os("OPENSHARD_CLIENT")?);
    openshard_uofiles::tiledata::load_tiles(dir.join("tiledata.mul")).ok()
}

/// Where the sample edits land, and every one of them is a position rather than
/// a place: inside a chunk, on its eastern edge, on its southern edge, and on
/// the corner where both meet.
///
/// The three edges are the whole point. A column's own height is the average of
/// the four cells meeting at its north-west corner, so an edit at `x % 64 == 0`
/// is read by a column in the chunk to the west — and a rebake that took only
/// the edited chunk's blocks would leave that column answering for the world as
/// it was. Around Britain, so the neighbourhoods swept below hold buildings
/// rather than ocean.
const EDITS: [(u16, u16); 4] = [(1363, 1600), (1408, 1600), (1363, 1664), (1408, 1664)];

/// How far around each edit every column is compared, in tiles.
///
/// Wider than the block the edit is in and wider than the chunk, so the sweep
/// covers both the blocks that were rebuilt and the ones that deliberately were
/// not.
const AROUND: i32 = 96;

/// How far apart the facet-wide samples are. Coprime with 8 and with 64, so
/// consecutive samples land on different cells of a block *and* of a chunk.
const STRIDE: usize = 31;

/// Raise one cell, published the way a `.setland` is.
fn raise(ground: &mut Ground, tiles: &TileData, x: u16, y: u16) {
    let base = ground.snapshot().expect("the facet it was built with");
    let was = base.map().land(x, y).expect("the sample tile is on this facet");
    let op = PatchOp::set_land(
        base.map(),
        x,
        y,
        LandCell {
            tile: was.tile,
            z:    was.z.saturating_add(40),
        },
    )
    .expect("a tile on this facet");
    publish(ground, tiles, vec![op]);
}

/// Put a second copy of something already standing in this chunk one tile east
/// of the edit, which is the op that moves a *block's* run rather than a cell.
///
/// Nothing is added where the chunk holds no statics at all — an item of a
/// graphic the table has never heard of would be a static with no height, which
/// is a weaker edit than the one this is here to make.
fn duplicate_a_static(ground: &mut Ground, tiles: &TileData, x: u16, y: u16) {
    let base = ground.snapshot().expect("the facet it was built with");
    let Some(&standing) = base
        .map()
        .statics_in_block(u32::from(x) / 8, u32::from(y) / 8)
        .first()
    else {
        return;
    };
    let op = PatchOp::add_static(
        base.map(),
        StaticItem {
            x,
            y,
            z: standing.z.saturating_add(20),
            ..standing
        },
    )
    .expect("a tile on this facet");
    publish(ground, tiles, vec![op]);
}

fn publish(ground: &mut Ground, tiles: &TileData, ops: Vec<PatchOp>) {
    let parent = ground.snapshot().expect("the facet it was built with").revision();
    let patch = Patch::new(
        Facet(0),
        parent,
        PatchAuthor("the locality oracle".to_owned()),
        PatchTime(0),
        ops,
    );
    ground.publish(&patch, tiles).expect("the sample patch applies");
}

/// Every surface the bake holds for one column, asked as a swimmer so that
/// nothing is filtered out of the comparison before it is made.
fn column(spans: &Spans<'_>, x: u16, y: u16) -> Vec<Span> {
    spans.surfaces(x, y).collect()
}

#[test]
#[ignore = "reads a 100 MB base set and bakes a facet; needs OPENSHARD_CLIENT"]
fn a_patched_facet_answers_like_one_baked_whole() {
    let (Some(path), Some(tiles)) = (base_set(), tile_data()) else {
        eprintln!(
            "skipping: needs a base set (OPENSHARD_BASE_SET, or felucca.osbase beside \
             openshard.toml) and OPENSHARD_CLIENT for tiledata.mul"
        );
        return;
    };
    let snapshot = openshard_basemap::load(&path)
        .expect("a readable base set")
        .snapshot;
    let mut ground = Ground::new(Some(snapshot), &tiles);

    for (x, y) in EDITS {
        raise(&mut ground, &tiles, x, y);
        duplicate_a_static(&mut ground, &tiles, x + 1, y);
    }

    // The facet as it now stands, and the bake it would have if nothing had
    // ever been published into it. Everything below is one against the other.
    let map: &WorldMap = ground.snapshot().expect("the facet it was built with").map();
    let whole = SpanIndex::build(map, &tiles);
    let patched = ground
        .terrain(&tiles)
        .expect("the facet it was built with")
        .spans()
        .swimming(true);
    let baked = Spans::new(map, &whole).swimming(true);

    let mut compared = 0_u64;
    let mut stored = 0_u64;
    // Hands back whether the column it just checked was a *stored* one, which is
    // the tier this node moves — the counting is the caller's so the closure
    // borrows nothing it would have to give back.
    let compare = |x: u16, y: u16| -> bool {
        let held = column(&patched, x, y);
        assert_eq!(
            held,
            column(&baked, x, y),
            "({x}, {y}) reads differently after a partial rebake than it does baked whole"
        );
        whole.stores(map, x, y)
    };

    // Every column around every edit, which is where a seam would be.
    for (at_x, at_y) in EDITS {
        for dy in -AROUND..=AROUND {
            for dx in -AROUND..=AROUND {
                let (Ok(x), Ok(y)) = (
                    u16::try_from(i32::from(at_x) + dx),
                    u16::try_from(i32::from(at_y) + dy),
                ) else {
                    continue;
                };
                if map.contains(x, y) {
                    stored += u64::from(compare(x, y));
                    compared += 1;
                }
            }
        }
    }
    let near = compared;
    let near_stored = stored;

    // And a sparse sweep of the whole facet, which is where a *repointing* that
    // disturbed somebody else's run would be — the failure that has nothing to
    // do with the edit's own neighbourhood.
    for y in (0..map.height() as usize).step_by(STRIDE) {
        for x in (0..map.width() as usize).step_by(STRIDE) {
            stored += u64::from(compare(x as u16, y as u16));
            compared += 1;
        }
    }

    println!(
        "{near} columns around {} edits ({near_stored} of them stored), \
         {} across the facet ({} stored)",
        EDITS.len(),
        compared - near,
        stored - near_stored
    );
    // A green run over nothing but bare ocean would prove nothing: the tier this
    // node moves is the stored one.
    assert!(
        stored > 10_000,
        "only {stored} of the columns compared were stored"
    );
}
