//! Which files an interiors flood says it was built from.
//!
//! The trap this closes is silent by construction. A facet read from a base set
//! is not derived from `map0LegacyMUL.uop` and `statics0.mul` any more — but
//! those files are still sitting in the install with their old length and their
//! old mtime, so a stamp that named them would **pass**, and hand the client a
//! flood of a world it has never seen. The navigation artifact's own
//! `stamp_of_base_set` exists for exactly this, and this file is the second
//! artifact keyed to a world saying the same thing.
//!
//! Every assertion here is about *inputs*, and a stamp records a name, a length
//! and an mtime — so the way to ask whether a file is an input is to change it
//! and see whether the stamp moves. No install and no bake: the files below are
//! written by hand, and the base set is a world of ours four tiles square.

use std::path::{Path, PathBuf};

use openshard_client_artscan::interiors;
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, WorldMap};
use openshard_map::patch::{Patch, PatchAuthor, PatchOp, PatchTime};
use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_movement::bake::{FacetWorld, WorldSource};
use openshard_protocol::world::Facet;
use openshard_tiles::LandTileId;

const FACET: Facet = Facet(0);

/// The two files a base set does **not** replace, plus the install's map — which
/// it does.
///
/// The map file is here so that it can be *changed* below: the whole question is
/// whether a base-set stamp notices, and it can only fail to notice a file that
/// exists.
fn install(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openshard-interior-stamp-{tag}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a writable temp dir");
    for name in ["map0LegacyMUL.uop", "tiledata.mul", "openshard-art.table"] {
        std::fs::write(dir.join(name), b"one").expect("a writable temp dir");
    }
    dir
}

/// A base set of flat ground inside `dir`, and the log path beside it.
fn base_set(dir: &Path) -> (PathBuf, PathBuf) {
    let path = dir.join("world.osbase");
    let log = openshard_basemap::patches::log_path(&path);
    let map = WorldMap::from_blocks(BlockExtent { wide: 2, down: 2 }, |_, _| LandCell {
        tile: LandTileId(3),
        z: 0,
    });
    openshard_basemap::write(
        &path,
        &MapSnapshot::new(FACET, map),
        openshard_basemap::Identity::Mint,
    )
    .expect("a writable temp dir");
    (path, log)
}

/// The stamp an interiors flood over the base set in `dir` would record.
fn stamp(dir: &Path, base_set: &Path) -> interiors::Stamp {
    let world = FacetWorld::read(dir, WorldSource::BaseSet(base_set), FACET).expect("the base set");
    interiors::stamp_of(dir, &world, FACET).expect("every input exists")
}

/// The install's map is not an input of a flood over a base set, and that has to
/// be true of a file that is *there* — an absent one would prove nothing.
#[test]
fn a_base_set_flood_does_not_notice_the_installs_map_changing() {
    let dir = install("map");
    let (path, _log) = base_set(&dir);

    let before = stamp(&dir, &path);
    std::fs::write(dir.join("map0LegacyMUL.uop"), b"a completely different facet")
        .expect("a writable temp dir");
    let after = stamp(&dir, &path);

    assert_eq!(
        before, after,
        "the install's map moved and a base-set flood's stamp moved with it, so it is stamping \
         files this world is not derived from"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// And the two files it *is* still derived from move it.
///
/// `tiledata.mul` because what a tile means is the table's — a flood built under
/// one table is not valid under another — and the wall catalogue because what a
/// *wall* is was measured off the art.
#[test]
fn the_tile_table_and_the_wall_catalogue_are_still_inputs() {
    let dir = install("shared");
    let (path, _log) = base_set(&dir);

    let before = stamp(&dir, &path);
    std::fs::write(dir.join("tiledata.mul"), b"another table entirely").expect("a writable temp dir");
    let table_moved = stamp(&dir, &path);
    assert_ne!(
        before, table_moved,
        "a flood built under one tile table must not validate under another"
    );

    std::fs::write(dir.join("openshard-art.table"), b"another measurement").expect("a writable temp dir");
    let catalogue_moved = stamp(&dir, &path);
    assert_ne!(
        table_moved, catalogue_moved,
        "a flood built from one wall catalogue must not validate against another"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A world of ours is the base set **plus its log**, so a flood run before an
/// edit was committed is stale — and a stamp naming only the base set would say
/// it was fine.
#[test]
fn a_committed_patch_moves_the_stamp() {
    let dir = install("patched");
    let (path, log) = base_set(&dir);

    let before = stamp(&dir, &path);
    let world = FacetWorld::read(&dir, WorldSource::BaseSet(&path), FACET).expect("the base set");
    let raised = LandCell {
        tile: LandTileId(3),
        z: 40,
    };
    let op = PatchOp::set_land(world.snapshot.map(), 1, 1, raised).expect("a tile of this world");
    let patch = Patch::new(
        FACET,
        world.snapshot.revision(),
        PatchAuthor("a test".to_owned()),
        PatchTime(0),
        vec![op],
    );
    openshard_basemap::patches::append(&log, FACET, MapRevision::INITIAL, &patch)
        .expect("a writable temp dir");

    assert_ne!(
        before,
        stamp(&dir, &path),
        "committing a patch left the stamp unchanged, so a flood over the world before the edit \
         would still be accepted"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The same world, stamped twice, is the same stamp — otherwise every
/// `assert_ne!` above would pass for a stamp that is never equal to itself.
#[test]
fn one_world_stamps_the_same_way_twice() {
    let dir = install("stable");
    let (path, _log) = base_set(&dir);

    assert_eq!(stamp(&dir, &path), stamp(&dir, &path));

    std::fs::remove_dir_all(&dir).ok();
}
