//! Where a facet comes from, and what follows from the answer.
//!
//! [`base_set_terrain`](../base_set_terrain.rs) already pins that the two
//! sources hold the same *world*: whatever the movement rules answer over the
//! install, they answer over a base set written from it, at tens of thousands of
//! places. This file pins the half above that — that the shard's boot, the two
//! bakes and the client resolve the *source* identically, because they now go
//! through one function to do it.
//!
//! It needs no client install: a base set is a world of ours, and one four
//! blocks square is written into the temp directory here.

use std::path::{Path, PathBuf};

use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, WorldMap};
use openshard_map::patch::{Patch, PatchAuthor, PatchOp, PatchTime};
use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_movement::bake::{FacetWorld, SourceError, WorldSource};
use openshard_protocol::world::Facet;
use openshard_tiles::LandTileId;

const FACET: Facet = Facet(0);
const BLOCKS: u32 = 4;

/// Flat ground, so a patch that raises one tile is visible against it.
const GROUND: LandCell = LandCell {
    tile: LandTileId(3),
    z: 0,
};

/// A base set in the temp directory, and the log path beside it.
///
/// The tag keeps two tests in one binary off each other's files and the pid
/// keeps two runs off each other's, which is what every other fixture over a
/// base set in this workspace does.
fn base_set(tag: &str, facet: Facet) -> (PathBuf, PathBuf) {
    let path = std::env::temp_dir().join(format!("openshard-source-{tag}-{}.osbase", std::process::id()));
    let log = openshard_basemap::patches::log_path(&path);
    std::fs::remove_file(&path).ok();
    std::fs::remove_file(&log).ok();

    let map = WorldMap::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        |_, _| GROUND,
    );
    openshard_basemap::write(
        &path,
        &MapSnapshot::new(facet, map),
        openshard_basemap::Identity::Mint,
    )
    .expect("a writable temp dir");
    (path, log)
}

fn clean(base_set: &Path, log: &Path) {
    std::fs::remove_file(base_set).ok();
    std::fs::remove_file(log).ok();
}

/// The ordinary case: a base set resolves to itself, and everything derived
/// from it is told to live beside it.
#[test]
fn a_base_set_world_carries_where_it_came_from_and_where_its_artifacts_go() {
    let (path, log) = base_set("plain", FACET);
    let install = std::env::temp_dir();

    let world = FacetWorld::read(&install, WorldSource::BaseSet(&path), FACET).expect("the base set");

    assert_eq!(world.snapshot.facet(), FACET);
    assert_eq!(world.snapshot.revision(), MapRevision::INITIAL);
    assert_eq!(world.base, Some(MapRevision::INITIAL));
    assert_eq!(world.patches, 0);
    assert_eq!(world.base_set.as_deref(), Some(path.as_path()));
    assert_eq!(
        world.log, None,
        "a world nobody has edited has no log, and an absent file is not an empty one"
    );
    // Beside the base set, not in the install: an artifact of this world left in
    // the install directory would be found by a reader of the install and
    // refused for reasons it cannot see.
    assert_eq!(world.artifacts(&install), openshard_movement::bake::beside(&path));

    clean(&path, &log);
}

/// The install is the other arm, and it is a source rather than the absence of
/// one — so it answers with no base set, no log and no base revision.
#[test]
fn an_install_world_has_no_base_set_and_keeps_its_artifacts_in_the_install() {
    let install = std::env::temp_dir().join(format!("openshard-source-empty-{}", std::process::id()));
    std::fs::create_dir_all(&install).expect("a writable temp dir");

    // No map files in there, so this is the failure path — which is the only
    // thing about the install arm a test without a client install can reach,
    // and it is worth reaching: it pins that a missing facet is *this* error
    // rather than a panic.
    let error = FacetWorld::read(&install, WorldSource::Install, FACET)
        .expect_err("an empty directory is not a client install");
    assert!(
        matches!(&error, SourceError::Install { path, .. } if path == &install),
        "expected the install arm to name the directory it failed on, got {error}"
    );

    std::fs::remove_dir_all(&install).ok();
}

/// A file that is a facet, and not the facet it was named for.
///
/// The trap this closes is a config that loads Tokuno as Felucca: every
/// coordinate plausible, every place wrong, and nothing anywhere saying so.
#[test]
fn a_base_set_of_another_facet_is_refused_rather_than_loaded() {
    let (path, log) = base_set("wrong-facet", Facet(3));
    let install = std::env::temp_dir();

    let error = FacetWorld::read(&install, WorldSource::BaseSet(&path), FACET)
        .expect_err("facet 3's world is not facet 0's");
    assert!(
        matches!(
            &error,
            SourceError::WrongFacet { wanted, found, .. } if *wanted == FACET && *found == Facet(3)
        ),
        "expected both facet numbers in the refusal, got {error}"
    );

    clean(&path, &log);
}

/// The world is the base set **plus its log**, and the resolution says so.
///
/// This is what makes one function rather than three worth having: a caller
/// that read the base alone would hold a world the shard is not running, and it
/// would hold it silently.
#[test]
fn a_committed_patch_is_part_of_the_world_the_source_resolves_to() {
    let (path, log) = base_set("patched", FACET);
    let install = std::env::temp_dir();

    let before = FacetWorld::read(&install, WorldSource::BaseSet(&path), FACET).expect("the base set");
    let raised = LandCell {
        tile: GROUND.tile,
        z: 40,
    };
    let op = PatchOp::set_land(before.snapshot.map(), 5, 6, raised).expect("a tile of this world");
    let patch = Patch::new(
        FACET,
        before.snapshot.revision(),
        PatchAuthor("a test".to_owned()),
        PatchTime(0),
        vec![op],
    );
    openshard_basemap::patches::append(&log, FACET, MapRevision::INITIAL, &patch)
        .expect("a writable temp dir");

    let after = FacetWorld::read(&install, WorldSource::BaseSet(&path), FACET).expect("the base set");
    assert_eq!(after.patches, 1);
    assert_eq!(
        after.snapshot.revision(),
        MapRevision::INITIAL.after(),
        "one patch moves the world one revision"
    );
    assert_eq!(
        after.base,
        Some(MapRevision::INITIAL),
        "the base set's own revision is the log's header and does not move with it"
    );
    assert_eq!(
        after.log.as_deref(),
        Some(log.as_path()),
        "the log is an input to every stamp over this world"
    );
    assert_eq!(
        after.snapshot.map().land(5, 6),
        Some(raised),
        "the ground the patch raised"
    );

    clean(&path, &log);
}
