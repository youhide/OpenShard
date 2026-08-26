//! **What the server put in the world, which no client file knows about.**
//!
//! A scene tool that reads `map0LegacyMUL.uop` and `statics.mul` sees the
//! shard's *art* and none of its *furniture*. `docs/parity.md`'s first backlog
//! entry is what that costs: a person asked about a cabinet they could see at
//! Britain's `(1504, 1655)`, and `tile_probe`, `onsite.rs`, `geometry_census.rs`
//! and `isolated_scene.rs` all agreed there was no cabinet there — because the
//! cabinet is two `decorations` rows (`0x0A97`/`0x0A98` at `(1505, 1656, 27)`
//! and `(1506, 1656, 27)`) and not a static. Half a session went into explaining
//! the nearest map static instead, which is a different graphic with a different
//! box.
//!
//! So "it does not reproduce in the tool" was a **false negative for everything
//! the server placed**, and nothing said so. `OPENSHARD_SCENE_EXTRA` closed it
//! by hand once the two rows had been transcribed, and that is exactly the
//! problem: a hand-transcribed input is not a parity input. This module is the
//! reader that makes it one.
//!
//! # Not a library, and not the server's own store
//!
//! A module the scene tools share (`mod shard;`), for
//! `examples/oracle/mod.rs`'s reason: the alternative is a second copy of the
//! same two queries. It cannot be a library here either, and this time the rule
//! is the workspace's own — `openshard-persistence` is a **server** crate, and
//! `crates/client/*` may not depend on it (`docs/architecture.md`). So the two
//! tables are read by SQL written out here rather than through that crate's
//! `Store` trait.
//!
//! That is a duplication of the *schema*, and it is bounded on purpose: seven
//! column names and six JSON keys, every one of them named in
//! `crates/server/persistence/src/sqlite.rs`'s own `SCHEMA` and in
//! `record.rs`'s `ItemRecord`/`DecorationRecord`. A rename there fails here
//! loudly (SQLite has no such column) rather than quietly returning nothing —
//! which is the failure mode this whole module exists to remove.
//!
//! # Read-only, on a database a shard may have open
//!
//! Opened `SQLITE_OPEN_READ_ONLY`, so this can neither create the file it was
//! pointed at nor write a byte of one that exists. Both matter: a mistyped path
//! that *created* an empty database would report "the server placed nothing
//! here", which is the false negative again, and a live shard is holding the
//! real one open. SQLite lets any number of readers in beside a writer, so
//! nothing here has to wait for a save to finish.

use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;

/// Which table a placed thing was read out of.
///
/// Kept because the two are different questions to a person looking at a
/// frame — a decoration is what a pack laid over the map at Populate time and
/// stays put, while a ground item is something a player or an NPC dropped this
/// session — and because a count of each is what the tool prints beside its
/// picture. The renderer draws them identically; nothing downstream branches on
/// this.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// An `items` row with `loc_kind = 0`: lying on the ground.
    Ground,
    /// A `decorations` row: the statics, doors and town containers a pack lays
    /// over the map's art.
    Decoration,
}

/// One thing the server has placed, as the database has it.
///
/// Deliberately *not* a `GroundItem`: this module has no business knowing what
/// the renderer's list looks like, and the caller is the one that translates a
/// real coordinate onto whatever anchor its scene is built on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Placed {
    /// Which table it came from.
    pub source: Source,
    /// Where it stands, in the map's own coordinates.
    pub x: u16,
    /// Where it stands, in the map's own coordinates.
    pub y: u16,
    /// How high. Signed: UO has basements.
    pub z: i8,
    /// The graphic as it stands now — a door's *current* leaf, since that is
    /// what the record keeps and what the client would be looking at.
    pub graphic: u16,
    /// Its hue, `0` for none.
    pub hue: u16,
}

/// One classic house placed by the shard.
///
/// A house is not an `items` row: its `multi` has to be expanded through the
/// client's `multi.mul` before a frame can draw its floors and walls. Keeping
/// this narrow record separate makes that omission explicit at the diagnostic
/// boundary instead of silently producing a picture without the building.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct House {
    /// The house's persistent serial, used only to select the one to inspect.
    pub serial: u32,
    /// The client's multi id (not the `0x4000`-offset graphic).
    pub multi: u16,
    /// The multi origin in world coordinates.
    pub at: openshard_protocol::world::Point,
}

/// The window to read: one facet, and the rectangle a scene's radius covers.
///
/// The same rectangle the map's statics are pulled from, so that a tool asking
/// for one tile does not silently acquire a barrel from the next street. Both
/// bounds inclusive, as the tile loops that produce them are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Window {
    /// Which facet, matching the one the tool loaded its map from.
    pub facet: u8,
    /// Westmost tile, inclusive.
    pub min_x: u16,
    /// Eastmost tile, inclusive.
    pub max_x: u16,
    /// Northmost tile, inclusive.
    pub min_y: u16,
    /// Southmost tile, inclusive.
    pub max_y: u16,
}

/// The database `config` names, as a path that can be opened.
///
/// Panics rather than returning an `Option`, and the messages are the point:
/// every way this can fail is a way a tool would otherwise draw a frame missing
/// everything the server placed and say nothing. Each one names the knob that
/// turns the reader off, because "I know, and I want the map's art alone" is a
/// legitimate answer — it is just not one a tool may assume on a caller's
/// behalf.
///
/// A relative `database` is resolved against the **config file's own
/// directory**, not the process's. A shard is run from the directory its
/// `openshard.toml` sits in (`cargo run -p openshard-playground` from the
/// workspace root), so the two agree there; resolving against the tool's own
/// working directory would instead make the answer depend on where a person
/// happened to type `cargo run`.
pub fn database_in(config: &Path) -> PathBuf {
    let loaded = openshard_config::Config::load(config).unwrap_or_else(|error| {
        panic!("reading {}: {error} — point OPENSHARD_SCENE_CONFIG at a shard's own openshard.toml, or set OPENSHARD_SCENE_SHARD=0", config.display())
    });
    let database = loaded.persistence.database;
    assert!(
        !database.is_empty(),
        "{} keeps the world in memory (`database` is empty), so nothing the server placed can be \
         read back: point OPENSHARD_SCENE_CONFIG at a shard that persists, or set \
         OPENSHARD_SCENE_SHARD=0 to draw the map's art alone",
        config.display(),
    );
    assert!(
        !database.starts_with("postgres://") && !database.starts_with("postgresql://"),
        "{} keeps the world in PostgreSQL ({database:?}), which this reader does not speak — set \
         OPENSHARD_SCENE_SHARD_DB to a SQLite file, or OPENSHARD_SCENE_SHARD=0 to draw the map's \
         art alone",
        config.display(),
    );
    let path = PathBuf::from(&database);
    match path.is_absolute() {
        true => path,
        false => config.parent().unwrap_or_else(|| Path::new(".")).join(path),
    }
}

/// Everything the server has placed inside `window`, both tables, unsorted.
///
/// Unsorted deliberately: the caller appends these to a list the frame's own
/// assembly sorts by [`depth::Order`](openshard_client_render::depth::Order),
/// and a `ORDER BY` here would be a second opinion about drawing order that
/// nothing reads.
pub fn read(database: &Path, window: Window) -> Vec<Placed> {
    assert!(
        database.exists(),
        "{} does not exist: a shard that has never saved has no database, and this reader will \
         not create one — set OPENSHARD_SCENE_SHARD_DB, or OPENSHARD_SCENE_SHARD=0 to draw the \
         map's art alone",
        database.display(),
    );
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|error| panic!("opening {}: {error}", database.display()));

    let mut placed = Vec::new();
    // The ground items: `loc_kind = 0` is the ground, `1` is inside a container
    // and `2` is worn — see `crates/server/persistence/src/sqlite.rs`'s SCHEMA.
    // A barrel's contents are not on any street, so the kind is a filter and not
    // something to resolve.
    query_into(
        &connection,
        "SELECT graphic, hue, x, y, z FROM items \
         WHERE loc_kind = 0 AND facet = ?1 AND x BETWEEN ?2 AND ?3 AND y BETWEEN ?4 AND ?5",
        window,
        Source::Ground,
        &mut placed,
    );
    // The decorations, whose record is one JSON blob — so the window is applied
    // by `json_extract` rather than by columns. Slower than an index would be
    // (every row is parsed), and it is a tool reading a few tens of thousands of
    // rows once, which is milliseconds.
    query_into(
        &connection,
        "SELECT json_extract(data, '$.graphic'), json_extract(data, '$.hue'), \
                json_extract(data, '$.x'), json_extract(data, '$.y'), json_extract(data, '$.z') \
         FROM decorations \
         WHERE json_extract(data, '$.facet') = ?1 \
           AND json_extract(data, '$.x') BETWEEN ?2 AND ?3 \
           AND json_extract(data, '$.y') BETWEEN ?4 AND ?5",
        window,
        Source::Decoration,
        &mut placed,
    );
    placed
}

/// One classic house, selected by its serial.
///
/// This is deliberately not a window query: a diagnostic caller names the
/// building it wants, which also avoids guessing whether a large multi whose
/// origin is just outside a scene reaches into it. Designed houses have no
/// classic multi and are therefore rejected loudly rather than drawn as an
/// empty lot.
pub fn house(database: &Path, serial: u32) -> Option<House> {
    assert!(
        database.exists(),
        "{} does not exist: point OPENSHARD_SCENE_SHARD_DB at a saved shard",
        database.display(),
    );
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|error| panic!("opening {}: {error}", database.display()));
    connection
        .query_row(
            "SELECT multi, x, y, z FROM houses WHERE serial = ?1",
            [i64::from(serial)],
            |row| {
                let multi: i64 = row.get(0)?;
                let x: i64 = row.get(1)?;
                let y: i64 = row.get(2)?;
                let z: i64 = row.get(3)?;
                Ok((multi, x, y, z))
            },
        )
        .optional()
        .unwrap_or_else(|error| panic!("reading house {serial}: {error}"))
        .map(|(multi, x, y, z)| House {
            serial,
            multi: u16::try_from(multi).unwrap_or_else(|_| panic!("house {serial} multi: {multi}")),
            at: openshard_protocol::world::Point::new(
                u16::try_from(x).unwrap_or_else(|_| panic!("house {serial} x: {x}")),
                u16::try_from(y).unwrap_or_else(|_| panic!("house {serial} y: {y}")),
                i8::try_from(z).unwrap_or_else(|_| panic!("house {serial} z: {z}")),
            ),
        })
}

/// One query of the five columns every row here comes back as, appended to
/// `out`.
///
/// The two statements differ in their `FROM` and in nothing else, so the
/// decoding is written once: five `i64`s out of SQLite's one integer type,
/// each narrowed by a `try_from` that names the column it refused. A `z` of 200
/// is a corrupt row and not a basement, and the loud way to learn that is
/// better than an `as i8` quietly drawing it at `-56`.
fn query_into(
    connection: &rusqlite::Connection,
    sql: &str,
    window: Window,
    source: Source,
    out: &mut Vec<Placed>,
) {
    let mut statement = connection
        .prepare(sql)
        .unwrap_or_else(|error| panic!("preparing {sql:?}: {error}"));
    let rows = statement
        .query_map(
            rusqlite::params![
                i64::from(window.facet),
                i64::from(window.min_x),
                i64::from(window.max_x),
                i64::from(window.min_y),
                i64::from(window.max_y),
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .unwrap_or_else(|error| panic!("running {sql:?}: {error}"));
    for row in rows {
        let (graphic, hue, x, y, z) = row.unwrap_or_else(|error| panic!("reading a row: {error}"));
        out.push(Placed {
            source,
            x: u16::try_from(x).unwrap_or_else(|_| panic!("x out of a facet: {x}")),
            y: u16::try_from(y).unwrap_or_else(|_| panic!("y out of a facet: {y}")),
            z: i8::try_from(z).unwrap_or_else(|_| panic!("z out of the world: {z}")),
            graphic: u16::try_from(graphic).unwrap_or_else(|_| panic!("graphic: {graphic}")),
            hue: u16::try_from(hue).unwrap_or_else(|_| panic!("hue: {hue}")),
        });
    }
}
