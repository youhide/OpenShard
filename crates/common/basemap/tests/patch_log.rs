//! A world is a base set plus its log, and every way that pair fails.
//!
//! The base set half is `base_set.rs`. This one is about what
//! [`openshard_basemap::load`] adds: a facet resolved to the revision its last
//! patch produced, and a refusal for every way a log can fail to be this
//! world's.

use std::path::{Path, PathBuf};

use openshard_basemap::{BaseError, Loaded, load, patches, write};
use openshard_map::grid::BlockExtent;
use openshard_map::map::{LandCell, LandTile, Map, StaticItem};
use openshard_map::patch::{Patch, PatchAuthor, PatchError, PatchOp, PatchTime, StaticId};
use openshard_map::snapshot::{MapRevision, MapSnapshot};
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::Facet;

const FACET: Facet = Facet(0);
const BLOCKS: u32 = 9;

/// The ground the fixture starts as, so a test can say what a patch replaced.
const GROUND: LandCell = LandCell {
    tile: LandTile(3),
    z: 0,
};

fn rock(x: u16, y: u16) -> StaticItem {
    StaticItem {
        tile: Graphic(0x1234),
        x,
        y,
        z: 7,
        hue: Hue::NONE,
    }
}

/// A base set of flat ground at `temp_dir`, and the log path beside it.
///
/// Nine blocks square, for `base_set.rs`'s reason: three of the four chunks are
/// edge chunks, so nothing here can assume a whole one.
fn world(tag: &str) -> (PathBuf, PathBuf) {
    let base_set =
        std::env::temp_dir().join(format!("openshard-patchlog-{tag}-{}.osbase", std::process::id()));
    let log = patches::log_path(&base_set);
    std::fs::remove_file(&base_set).ok();
    std::fs::remove_file(&log).ok();

    let map = Map::from_blocks(
        BlockExtent {
            wide: BLOCKS,
            down: BLOCKS,
        },
        |_, _| GROUND,
    );
    write(&base_set, &MapSnapshot::new(FACET, map)).expect("a writable temp dir");
    (base_set, log)
}

fn clean(base_set: &Path, log: &Path) {
    std::fs::remove_file(base_set).ok();
    std::fs::remove_file(log).ok();
}

fn patch(parent: MapRevision, ops: Vec<PatchOp>) -> Patch {
    Patch::new(
        FACET,
        parent,
        PatchAuthor("a test".into()),
        PatchTime(1_755_000_000),
        ops,
    )
}

/// The whole of direction C's first half: a change is committed, the shard is
/// restarted, and the world it comes up on is the changed one.
#[test]
fn a_world_is_its_base_set_plus_its_log() {
    let (base_set, log) = world("resolves");
    let hill = LandCell {
        tile: LandTile(9),
        z: 40,
    };

    // Two patches, each against the revision the one before it produced.
    let first = patch(
        MapRevision::INITIAL,
        vec![PatchOp::SetLand {
            x: 10,
            y: 10,
            was: GROUND,
            now: hill,
        }],
    );
    let second = patch(first.revision(), vec![PatchOp::AddStatic { item: rock(10, 10) }]);
    for committed in [&first, &second] {
        patches::append(&log, FACET, MapRevision::INITIAL, committed).expect("a writable temp dir");
    }

    let Loaded {
        snapshot,
        base,
        log: found,
        patches: applied,
    } = load(&base_set).expect("a base set and its log");

    assert_eq!(applied, 2);
    // The revision the log lies over, which is what an appender has to name.
    assert_eq!(base, MapRevision::INITIAL);
    assert_eq!(found.as_deref(), Some(log.as_path()));
    // Revision 1 is the base; two patches make it 3.
    assert_eq!(snapshot.revision(), MapRevision::decoded(3));
    assert_eq!(snapshot.map().land(10, 10), Some(hill));
    assert_eq!(
        snapshot.map().statics_at(10, 10).copied().collect::<Vec<_>>(),
        vec![rock(10, 10)]
    );
    // And nothing else moved.
    assert_eq!(snapshot.map().land(11, 10), Some(GROUND));
    assert_eq!(snapshot.map().static_count(), 1);
    clean(&base_set, &log);
}

/// A world nobody has edited is its base set, and its log is `None` rather than
/// an empty one — which matters because a file is an input to a bake's stamp
/// and an absent file is not.
#[test]
fn a_world_with_no_log_is_its_base_set() {
    let (base_set, log) = world("no-log");
    let loaded = load(&base_set).expect("a base set");
    assert_eq!(loaded.patches, 0);
    assert_eq!(loaded.log, None);
    assert_eq!(loaded.snapshot.revision(), MapRevision::INITIAL);
    clean(&base_set, &log);
}

/// A log is committed to in order, and a chain that does not hold is refused at
/// the record that broke it — not applied last-write-wins.
#[test]
fn a_patch_that_does_not_follow_the_one_before_it_is_refused() {
    let (base_set, log) = world("out-of-order");
    let step = |z| {
        patch(
            MapRevision::INITIAL,
            vec![PatchOp::SetLand {
                x: 1,
                y: 1,
                was: GROUND,
                now: LandCell { tile: GROUND.tile, z },
            }],
        )
    };
    // Both against revision 1, so the second one cannot follow the first.
    patches::append(&log, FACET, MapRevision::INITIAL, &step(5)).expect("a writable temp dir");
    patches::append(&log, FACET, MapRevision::INITIAL, &step(7)).expect("a writable temp dir");

    assert!(matches!(
        load(&base_set),
        Err(BaseError::NotApplied {
            at: 1,
            source: PatchError::Conflict { .. },
            ..
        })
    ));
    clean(&base_set, &log);
}

/// The `was` fields earning their bytes: a log whose header passes and whose
/// ops describe some other world is refused by the first op.
#[test]
fn a_patch_against_a_world_that_is_not_there_is_refused() {
    let (base_set, log) = world("elsewhere");
    let elsewhere = LandCell {
        tile: LandTile(200),
        z: -12,
    };
    let stray = patch(
        MapRevision::INITIAL,
        vec![PatchOp::SetLand {
            x: 4,
            y: 4,
            was: elsewhere,
            now: GROUND,
        }],
    );
    patches::append(&log, FACET, MapRevision::INITIAL, &stray).expect("a writable temp dir");

    assert!(matches!(
        load(&base_set),
        Err(BaseError::NotApplied {
            at: 0,
            source: PatchError::LandNotAsRecorded { .. },
            ..
        })
    ));
    clean(&base_set, &log);
}

/// An ordinal is only an identity against a stated revision, and this is the
/// shape of edit that proves it survives a round trip through the log: three
/// identical rocks, and the middle one taken away by a later patch.
#[test]
fn an_ordinal_survives_the_log() {
    let (base_set, log) = world("ordinal");
    let first = patch(
        MapRevision::INITIAL,
        vec![
            PatchOp::AddStatic { item: rock(2, 2) },
            PatchOp::AddStatic { item: rock(2, 2) },
            PatchOp::AddStatic { item: rock(2, 2) },
        ],
    );
    let second = patch(
        first.revision(),
        vec![PatchOp::RemoveStatic {
            which: StaticId(1),
            was: rock(2, 2),
        }],
    );
    for committed in [&first, &second] {
        patches::append(&log, FACET, MapRevision::INITIAL, committed).expect("a writable temp dir");
    }

    let loaded = load(&base_set).expect("a base set and its log");
    assert_eq!(loaded.snapshot.map().statics_at(2, 2).count(), 2);
    clean(&base_set, &log);
}

/// The header's own job: a log of another facet, refused by a byte rather than
/// by a tile that happens to disagree.
#[test]
fn a_log_of_another_facet_is_refused() {
    let (base_set, log) = world("wrong-facet");
    let stray = Patch::new(
        Facet(3),
        MapRevision::INITIAL,
        PatchAuthor("a test".into()),
        PatchTime(0),
        Vec::new(),
    );
    patches::append(&log, Facet(3), MapRevision::INITIAL, &stray).expect("a writable temp dir");

    assert!(matches!(
        load(&base_set),
        Err(BaseError::Log {
            source: patches::LogError::WrongFacet { .. }
        })
    ));
    // And a patch cannot be committed to a log of a world it is not part of.
    assert!(matches!(
        patches::append(
            &log,
            FACET,
            MapRevision::INITIAL,
            &patch(MapRevision::INITIAL, Vec::new())
        ),
        Err(patches::LogError::WrongFacet { .. })
    ));
    clean(&base_set, &log);
}

/// A log written over a revision this base set is not at. The base set here is
/// revision 1, and the log says it lies over 50.
#[test]
fn a_log_written_over_another_revision_is_refused() {
    let (base_set, log) = world("wrong-base");
    let over = MapRevision::decoded(50);
    patches::append(&log, FACET, over, &patch(over, Vec::new())).expect("a writable temp dir");

    assert!(matches!(
        load(&base_set),
        Err(BaseError::Log {
            source: patches::LogError::WrongBase { .. }
        })
    ));
    clean(&base_set, &log);
}

/// A crash between a record's length and its payload leaves a file that is
/// refused by name, not a world quietly missing its last edit.
#[test]
fn a_torn_record_is_refused_rather_than_trimmed() {
    let (base_set, log) = world("torn");
    patches::append(
        &log,
        FACET,
        MapRevision::INITIAL,
        &patch(
            MapRevision::INITIAL,
            vec![PatchOp::AddStatic { item: rock(3, 3) }],
        ),
    )
    .expect("a writable temp dir");

    let bytes = std::fs::read(&log).unwrap();
    std::fs::write(&log, &bytes[..bytes.len() - 3]).unwrap();
    assert!(matches!(
        load(&base_set),
        Err(BaseError::Log {
            source: patches::LogError::Truncated { at: 0, .. }
        })
    ));
    clean(&base_set, &log);
}

/// What the checksum is for: a record whose bytes are not the bytes it was
/// written as, caught before a decoder is handed them.
#[test]
fn a_flipped_bit_is_caught_by_the_checksum() {
    let (base_set, log) = world("checksum");
    patches::append(
        &log,
        FACET,
        MapRevision::INITIAL,
        &patch(
            MapRevision::INITIAL,
            vec![PatchOp::AddStatic { item: rock(3, 3) }],
        ),
    )
    .expect("a writable temp dir");

    let mut bytes = std::fs::read(&log).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&log, &bytes).unwrap();
    assert!(matches!(
        load(&base_set),
        Err(BaseError::Log {
            source: patches::LogError::Corrupt { at: 0, .. }
        })
    ));
    clean(&base_set, &log);
}

#[test]
fn a_file_that_is_not_a_log_is_refused() {
    let (base_set, log) = world("not-a-log");
    std::fs::write(&log, b"OSPX and then some").unwrap();
    assert!(matches!(
        load(&base_set),
        Err(BaseError::Log {
            source: patches::LogError::NotALog { .. }
        })
    ));

    std::fs::write(&log, b"OSP").unwrap();
    assert!(matches!(
        load(&base_set),
        Err(BaseError::Log {
            source: patches::LogError::NotALog { .. }
        })
    ));
    clean(&base_set, &log);
}

/// The log lives beside the base set, at the same name with its own extension.
#[test]
fn a_log_is_named_for_the_base_set_it_lies_over() {
    assert_eq!(
        patches::log_path(Path::new("/shard/felucca.osbase")),
        PathBuf::from("/shard/felucca.ospatch")
    );
    // A base set with no extension still gets one.
    assert_eq!(
        patches::log_path(Path::new("/shard/felucca")),
        PathBuf::from("/shard/felucca.ospatch")
    );
}
