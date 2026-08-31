//! **The shard reader, gated.**
//!
//! `examples/shard/mod.rs` is what stops a scene tool from answering "there is
//! no cabinet there" about a cabinet a player can see (`docs/parity.md`, the
//! shard-furniture section). It reads two tables of a database this crate may
//! not depend on the writer of, so what can go wrong is a *silent* wrong answer:
//! a column that has been renamed, a window off by a tile, a `loc_kind` filter
//! that lets a barrel's contents onto the street. Every one of those returns
//! rows, or returns none, and neither looks like a failure.
//!
//! So the reader is asked, on a database written here row by row, about rows
//! that are meant to come back and rows that are meant not to — and the ones
//! that are meant not to are the point. A test that only checked the cabinet
//! arrives would pass on a `read` with no `WHERE` clause at all.
//!
//! No GPU and no client files: this is SQL and arithmetic.

// The tool's own module, reached the way `tests/traced.rs` reaches
// `examples/oracle/mod.rs` — one reader, not a second copy of it in `tests/`.
// This gate exercises the window reader; the same module's house reader is
// exercised by the example binary that also includes it.
#[allow(dead_code)]
#[path = "../examples/shard/mod.rs"]
mod shard;

use std::path::{Path, PathBuf};

use shard::{Placed, Source, Window};

/// The two tables as `crates/server/persistence/src/sqlite.rs` declares them,
/// cut down to the columns this reader names.
///
/// **Cut down deliberately, and it does not weaken the gate.** The reader
/// selects columns by name, so a database with fewer of them answers exactly as
/// the shard's own does for those columns — while a rename on the server's side
/// makes this fixture and the real database fail together, which is the whole
/// property being claimed. Copying the full `SCHEMA` in would only add columns
/// nothing here reads.
const SCHEMA: &str = "
CREATE TABLE items (
    serial   INTEGER PRIMARY KEY,
    graphic  INTEGER NOT NULL,
    hue      INTEGER NOT NULL,
    loc_kind INTEGER NOT NULL,
    facet    INTEGER NOT NULL,
    x        INTEGER NOT NULL,
    y        INTEGER NOT NULL,
    z        INTEGER NOT NULL
);
CREATE TABLE decorations (
    serial INTEGER PRIMARY KEY,
    data   TEXT NOT NULL
);";

/// A path under `temp_dir` nothing else in this run will pick, removed first so
/// a previous run's file cannot answer for this one.
fn scratch(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("openshard-shard-{tag}-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    path
}

/// One `items` row of the fixture.
struct ItemRow {
    serial: i64,
    graphic: u16,
    hue: u16,
    loc_kind: u8,
    facet: u8,
    x: u16,
    y: u16,
    z: i8,
}

/// One `decorations` row: the same without `loc_kind`, which a decoration has
/// no column for — it is always standing where it stands.
type DecorationRow = (i64, u16, u16, u8, u16, u16, i8);

/// A database holding exactly the rows given — the decorations written as the
/// JSON blob the server writes.
fn write(path: &Path, items: &[ItemRow], decorations: &[DecorationRow]) {
    let connection = rusqlite::Connection::open(path).expect("a scratch database");
    connection.execute_batch(SCHEMA).expect("the two tables");
    for item in items {
        connection
            .execute(
                "INSERT INTO items (serial, graphic, hue, loc_kind, facet, x, y, z) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    item.serial,
                    item.graphic,
                    item.hue,
                    item.loc_kind,
                    item.facet,
                    item.x,
                    item.y,
                    item.z,
                ],
            )
            .expect("an item row");
    }
    for &(serial, graphic, hue, facet, x, y, z) in decorations {
        // The real record has `door`, `container_gump` and `key_value` beside
        // these; they are left out for the same reason the schema above is cut
        // down — `json_extract` reads keys by name, and a key nothing selects
        // cannot change an answer.
        let data = format!(
            r#"{{"serial":{serial},"graphic":{graphic},"hue":{hue},"facet":{facet},"x":{x},"y":{y},"z":{z}}}"#
        );
        connection
            .execute(
                "INSERT INTO decorations (serial, data) VALUES (?1, ?2)",
                rusqlite::params![serial, data],
            )
            .expect("a decoration row");
    }
}

/// Britain's own window, the one the cabinet was found in.
const WINDOW: Window = Window {
    facet: 0,
    min_x: 1500,
    max_x: 1508,
    min_y: 1651,
    max_y: 1659,
};

#[test]
fn reads_both_tables_and_keeps_out_what_the_window_does_not_hold() {
    let path = scratch("window");
    write(
        &path,
        &[
            // Inside: a dropped item, which is what `loc_kind = 0` means.
            ItemRow {
                serial: 1,
                graphic: 0x0EED,
                hue: 0,
                loc_kind: 0,
                facet: 0,
                x: 1504,
                y: 1655,
                z: 27,
            },
            // Inside the rectangle and inside a *container* — a barrel's
            // contents are not on the street, and a reader that dropped the
            // `loc_kind` filter would draw them there.
            ItemRow {
                serial: 2,
                graphic: 0x0EED,
                hue: 0,
                loc_kind: 1,
                facet: 0,
                x: 1504,
                y: 1655,
                z: 27,
            },
            // Worn, likewise: on a body, not on the ground.
            ItemRow {
                serial: 3,
                graphic: 0x1F03,
                hue: 0,
                loc_kind: 2,
                facet: 0,
                x: 1504,
                y: 1655,
                z: 27,
            },
            // Another facet, at the same coordinates. Trammel is Britain's twin
            // tile for tile, so a reader that forgot the facet would find a
            // plausible thing at a plausible place.
            ItemRow {
                serial: 4,
                graphic: 0x0EED,
                hue: 0,
                loc_kind: 0,
                facet: 1,
                x: 1504,
                y: 1655,
                z: 27,
            },
            // One tile east of the window's edge.
            ItemRow {
                serial: 5,
                graphic: 0x0EED,
                hue: 0,
                loc_kind: 0,
                facet: 0,
                x: 1509,
                y: 1655,
                z: 27,
            },
            // One tile north of it.
            ItemRow {
                serial: 6,
                graphic: 0x0EED,
                hue: 0,
                loc_kind: 0,
                facet: 0,
                x: 1504,
                y: 1650,
                z: 27,
            },
        ],
        &[
            // The cabinet the whole entry is about, both halves.
            (10, 0x0A97, 0, 0, 1505, 1656, 27),
            (11, 0x0A98, 0, 0, 1506, 1656, 27),
            // On the window's own corner: inclusive, both bounds.
            (12, 0x0B1D, 33, 0, 1508, 1659, 0),
            // A facet away, and a tile away.
            (13, 0x0A97, 0, 3, 1505, 1656, 27),
            (14, 0x0A97, 0, 0, 1499, 1656, 27),
        ],
    );

    let mut read = shard::read(&path, WINDOW);
    read.sort_by_key(|one| (one.x, one.y, one.graphic));
    assert_eq!(
        read,
        vec![
            Placed {
                source: Source::Ground,
                x: 1504,
                y: 1655,
                z: 27,
                graphic: 0x0EED,
                hue: 0
            },
            Placed {
                source: Source::Decoration,
                x: 1505,
                y: 1656,
                z: 27,
                graphic: 0x0A97,
                hue: 0
            },
            Placed {
                source: Source::Decoration,
                x: 1506,
                y: 1656,
                z: 27,
                graphic: 0x0A98,
                hue: 0
            },
            Placed {
                source: Source::Decoration,
                x: 1508,
                y: 1659,
                z: 0,
                graphic: 0x0B1D,
                hue: 33
            },
        ],
        "the ground item, both halves of the cabinet, and the corner — and nothing contained, \
         worn, off-facet or off-window",
    );
    std::fs::remove_file(&path).expect("cleaning up");
}

#[test]
fn a_basement_comes_back_below_the_ground() {
    // `z` is the one column whose SQLite type is wider than its meaning on both
    // sides of zero, and `as i8` on a negative `i64` is the kind of quiet wrong
    // answer that draws a cellar on a roof.
    let path = scratch("basement");
    write(
        &path,
        &[ItemRow {
            serial: 1,
            graphic: 0x0EED,
            hue: 0,
            loc_kind: 0,
            facet: 0,
            x: 1504,
            y: 1655,
            z: -60,
        }],
        &[(2, 0x0A97, 0, 0, 1505, 1656, -128)],
    );
    let mut read = shard::read(&path, WINDOW);
    read.sort_by_key(|one| one.z);
    assert_eq!(read.iter().map(|one| one.z).collect::<Vec<_>>(), vec![-128, -60]);
    std::fs::remove_file(&path).expect("cleaning up");
}

#[test]
#[should_panic(expected = "does not exist")]
fn refuses_a_database_that_is_not_there_rather_than_creating_an_empty_one() {
    // The failure this reader exists to remove, in its purest form: a path with
    // nothing at it must not read back as "the server placed nothing here".
    let path = scratch("missing");
    shard::read(&path, WINDOW);
}

#[test]
fn creates_nothing_when_it_refuses() {
    // The other half of the test above: refusing is worth nothing if the
    // refusal itself left a database behind, because the *next* run would find
    // one and answer "the server placed nothing here" with no panic at all.
    // What holds today is the `exists` check, which runs before any open;
    // `SQLITE_OPEN_READ_ONLY` is the second line, and it is what would still
    // hold if that check were ever dropped.
    let path = scratch("untouched");
    let refused = std::panic::catch_unwind(|| shard::read(&path, WINDOW));
    assert!(refused.is_err(), "a missing database is a panic");
    assert!(!path.exists(), "and the reader left no database behind it");
}

/// A config naming a database beside itself, which is where a shard's own
/// `openshard.toml` names one.
#[test]
fn a_relative_database_is_resolved_against_the_config_and_not_the_process() {
    let dir = std::env::temp_dir().join(format!("openshard-shard-config-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let config = dir.join("openshard.toml");
    std::fs::write(
        &config,
        "[server]\nname = \"t\"\nlisten = \"0.0.0.0:2593\"\nadvertise = \"127.0.0.1:2593\"\n\n\
         [persistence]\ndatabase = \"world.db\"\n",
    )
    .expect("a scratch config");

    assert_eq!(
        shard::database_in(&config),
        dir.join("world.db"),
        "beside the config that named it — resolving against the process's own directory would \
         make the answer depend on where somebody typed `cargo run`",
    );
    std::fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
#[should_panic(expected = "keeps the world in memory")]
fn an_in_memory_shard_is_refused_rather_than_read_as_empty() {
    // A shard configured with no database has a world and no way to show it,
    // which is the false negative wearing a different hat.
    let dir = std::env::temp_dir().join(format!("openshard-shard-memory-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let config = dir.join("openshard.toml");
    std::fs::write(
        &config,
        "[server]\nname = \"t\"\nlisten = \"0.0.0.0:2593\"\nadvertise = \"127.0.0.1:2593\"\n",
    )
    .expect("a scratch config");
    shard::database_in(&config);
}
