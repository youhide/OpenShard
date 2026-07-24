//! The SQLite backend.
//!
//! # Why SQLite is sync behind an async trait
//!
//! [`Store`] is async because one of its backends, PostgreSQL, is a network
//! server whose every call is a round-trip. SQLite is a file on the same disk,
//! and `rusqlite` is a blocking C library. Rather than pretend it is async, each
//! method does its work on [`tokio::task::spawn_blocking`]: the blocking read or
//! write runs on a thread that is allowed to block, and the shard's async runtime
//! is not stalled waiting on a disk. This is the same bargain the whole crate is
//! built on — the save is allowed to be slow, it is not allowed to be in the way.
//!
//! # One connection behind a mutex
//!
//! A `rusqlite::Connection` is `Send` but not `Sync`, and SQLite serialises
//! writers anyway. A single connection behind a [`Mutex`] is honest about that:
//! saves are infrequent and off the tick, so the lock is never contended by
//! anything that matters.

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{params, Connection, OptionalExtension};

use crate::journal::Snapshot;
use crate::record::{
    AccountRecord, CharacterRecord, DecorationRecord, ItemLocation, ItemRecord, MobileRecord,
    SpawnerRecord, SCHEMA_VERSION,
};
use crate::store::{Store, StoreError};

/// The flat form of an [`ItemLocation`] for the `items` table: a kind tag and the
/// union of every variant's parameters, the fields not used by a kind left zero.
struct FlatLocation {
    kind: u8,
    facet: u8,
    x: u16,
    y: u16,
    z: i8,
    parent: u32,
    grid: u8,
    layer: u8,
}

impl ItemLocation {
    fn flatten(self) -> FlatLocation {
        match self {
            ItemLocation::Ground { facet, x, y, z } => FlatLocation {
                kind: 0,
                facet,
                x,
                y,
                z,
                parent: 0,
                grid: 0,
                layer: 0,
            },
            ItemLocation::Contained {
                container,
                x,
                y,
                grid,
            } => FlatLocation {
                kind: 1,
                facet: 0,
                x,
                y,
                z: 0,
                parent: container,
                grid,
                layer: 0,
            },
            ItemLocation::Equipped { mobile, layer } => FlatLocation {
                kind: 2,
                facet: 0,
                x: 0,
                y: 0,
                z: 0,
                parent: mobile,
                grid: 0,
                layer,
            },
        }
    }

    /// Rebuild a location from its flat columns, or `None` if the kind tag is one
    /// no version wrote — a corrupt or future row, dropped rather than guessed.
    fn inflate(f: &FlatLocation) -> Option<Self> {
        match f.kind {
            0 => Some(ItemLocation::Ground {
                facet: f.facet,
                x: f.x,
                y: f.y,
                z: f.z,
            }),
            1 => Some(ItemLocation::Contained {
                container: f.parent,
                x: f.x,
                y: f.y,
                grid: f.grid,
            }),
            2 => Some(ItemLocation::Equipped {
                mobile: f.parent,
                layer: f.layer,
            }),
            _ => None,
        }
    }
}

/// The tables, created on open. `IF NOT EXISTS` so opening an existing database
/// is a no-op rather than an error.
const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
    name       TEXT PRIMARY KEY,
    credential TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS characters (
    serial  INTEGER PRIMARY KEY,
    account TEXT NOT NULL,
    name    TEXT NOT NULL,
    body    INTEGER NOT NULL,
    hue     INTEGER NOT NULL,
    facet   INTEGER NOT NULL,
    x       INTEGER NOT NULL,
    y       INTEGER NOT NULL,
    z       INTEGER NOT NULL,
    facing  INTEGER NOT NULL,
    strength     INTEGER NOT NULL,
    dexterity    INTEGER NOT NULL,
    intelligence INTEGER NOT NULL,
    -- The trained skills as a JSON array, like the spawner creature list: a
    -- handful per character, not a table's worth.
    skills  TEXT NOT NULL,
    -- Active effects as a JSON array (poison today, buffs and debuffs later),
    -- so a relog cannot wash a debuff off.
    effects  TEXT NOT NULL,
    -- Whether it logged out dead: a ghost relogs a ghost. 0 for the living.
    dead     INTEGER NOT NULL,
    -- The player's quest log — an opaque JSON blob the pack owns. '' for none.
    quest_blob TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS items (
    serial    INTEGER PRIMARY KEY,
    owner     INTEGER NOT NULL,
    graphic   INTEGER NOT NULL,
    hue       INTEGER NOT NULL,
    amount    INTEGER NOT NULL,
    stackable INTEGER NOT NULL,
    gump      INTEGER,
    -- location: kind 0 ground / 1 contained / 2 equipped, and its parameters.
    loc_kind INTEGER NOT NULL,
    facet    INTEGER NOT NULL,
    x        INTEGER NOT NULL,
    y        INTEGER NOT NULL,
    z        INTEGER NOT NULL,
    parent   INTEGER NOT NULL,
    grid     INTEGER NOT NULL,
    layer    INTEGER NOT NULL,
    price    INTEGER,
    name     TEXT,
    -- a spellbook's learned-spell bitmask, so a bought book still opens after a relog.
    spellbook INTEGER
);
CREATE INDEX IF NOT EXISTS items_owner ON items (owner);
-- NPC mobiles and placed decoration, each a JSON record keyed by serial: a
-- mobile is two dozen fields the simulation refactors freely, and the spawner
-- creature list set the JSON-blob precedent. The schema gate versions them.
CREATE TABLE IF NOT EXISTS mobiles (
    serial INTEGER PRIMARY KEY,
    data   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS decorations (
    serial INTEGER PRIMARY KEY,
    data   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS spawners (
    id            INTEGER PRIMARY KEY,
    facet         INTEGER NOT NULL,
    x             INTEGER NOT NULL,
    y             INTEGER NOT NULL,
    width         INTEGER NOT NULL,
    height        INTEGER NOT NULL,
    max_count     INTEGER NOT NULL,
    respawn_secs  INTEGER NOT NULL,
    remaining_secs INTEGER NOT NULL,
    -- The creature list as a JSON array; a spawner holds a handful, not a table's
    -- worth, so a blob is simpler than a join.
    creatures     TEXT NOT NULL
);";

/// A `Store` kept in a SQLite database.
///
/// One of the two backends behind the [`Store`] trait; PostgreSQL is the other,
/// and which a shard runs is the operator's choice, not a tier — SQLite handles
/// a live shard perfectly well. The character's [`serial`](CharacterRecord::serial)
/// is the primary key, because that is the identity that has to survive a restart.
#[derive(Debug)]
pub struct SqliteStore {
    connection: Arc<Mutex<Connection>>,
}

impl SqliteStore {
    /// Open (or create) a database at `path`.
    ///
    /// Creates the tables if they are new, and refuses a database written by a
    /// build with a different [`SCHEMA_VERSION`] rather than reading it and
    /// silently dropping what it does not understand.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let connection = Connection::open(path).map_err(database)?;
        Self::init(connection)
    }

    /// A throwaway in-memory database, for tests.
    ///
    /// Its contents vanish when it is dropped, so it proves behaviour, not
    /// persistence — the reopen test uses a real file for that.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let connection = Connection::open_in_memory().map_err(database)?;
        Self::init(connection)
    }

    fn init(connection: Connection) -> Result<Self, StoreError> {
        connection.execute_batch(SCHEMA_SQL).map_err(database)?;

        // The schema version is stamped once, on a fresh database, and checked on
        // every open after. A database from the future is refused, not read.
        let found: Option<u32> = connection
            .query_row("SELECT value FROM meta WHERE key = 'schema'", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(database)?;
        match found {
            Some(version) if version != SCHEMA_VERSION => {
                return Err(StoreError::SchemaMismatch {
                    found: version,
                    understood: SCHEMA_VERSION,
                });
            }
            Some(_) => {}
            None => {
                connection
                    .execute(
                        "INSERT INTO meta (key, value) VALUES ('schema', ?1)",
                        params![SCHEMA_VERSION],
                    )
                    .map_err(database)?;
            }
        }

        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }
}

#[async_trait]
impl Store for SqliteStore {
    async fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError> {
        // Refuse before touching the database, exactly as `MemoryStore` does: a
        // snapshot from a future schema must not be half-written.
        if snapshot.schema != SCHEMA_VERSION {
            return Err(StoreError::SchemaMismatch {
                found: snapshot.schema,
                understood: SCHEMA_VERSION,
            });
        }

        let connection = Arc::clone(&self.connection);
        let characters = snapshot.characters.clone();
        let removed = snapshot.removed.clone();
        let inventories = snapshot.inventories.clone();
        let ground = snapshot.ground.clone();
        let spawners = snapshot.spawners.clone();
        let mobiles = snapshot.mobiles.clone();
        let decorations = snapshot.decorations.clone();
        blocking(move || {
            let mut guard = connection
                .lock()
                .expect("the sqlite mutex is never poisoned");
            // One transaction: all of the snapshot or none of it. A half-written
            // world is a world that never existed — see `crate::journal`.
            let transaction = guard.transaction().map_err(database)?;
            for record in &characters {
                let skills = serde_json::to_string(&record.skills)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                let effects = serde_json::to_string(&record.effects)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO characters \
                         (serial, account, name, body, hue, facet, x, y, z, facing, \
                          strength, dexterity, intelligence, skills, effects, dead, quest_blob) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                        params![
                            record.serial,
                            record.account,
                            record.name,
                            record.body,
                            record.hue,
                            record.facet,
                            record.x,
                            record.y,
                            record.z,
                            record.facing,
                            record.strength,
                            record.dexterity,
                            record.intelligence,
                            skills,
                            effects,
                            record.dead,
                            record.quest_blob,
                        ],
                    )
                    .map_err(database)?;
            }
            // The mobiles sweep runs BEFORE the inventories: it clears every item
            // owned by any previously saved mobile (a dead vendor's crate must not
            // linger), and the same snapshot re-writes the live mobiles' inventories
            // right after — the world side always sweeps the two together.
            if let Some(mobiles) = &mobiles {
                transaction
                    .execute(
                        "DELETE FROM items WHERE owner IN (SELECT serial FROM mobiles)",
                        [],
                    )
                    .map_err(database)?;
                transaction
                    .execute("DELETE FROM mobiles", [])
                    .map_err(database)?;
                for mobile in mobiles {
                    let data = serde_json::to_string(mobile)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                    transaction
                        .execute(
                            "INSERT INTO mobiles (serial, data) VALUES (?1, ?2)",
                            params![mobile.serial, data],
                        )
                        .map_err(database)?;
                }
            }
            // Each inventory replaces everything under its owner; a ground sweep
            // replaces every ownerless item. Write one item the same way whichever
            // set it came from.
            let write_item =
                |tx: &rusqlite::Transaction<'_>, item: &ItemRecord| -> rusqlite::Result<()> {
                    let f = item.location.flatten();
                    tx.execute(
                        "INSERT OR REPLACE INTO items \
                     (serial, owner, graphic, hue, amount, stackable, gump, \
                      loc_kind, facet, x, y, z, parent, grid, layer, price, name, spellbook) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                        params![
                            item.serial,
                            item.owner,
                            item.graphic,
                            item.hue,
                            item.amount,
                            item.stackable,
                            item.container_gump,
                            f.kind,
                            f.facet,
                            f.x,
                            f.y,
                            f.z,
                            f.parent,
                            f.grid,
                            f.layer,
                            item.price,
                            item.name,
                            // A u64 mask reinterpreted as i64 (SQLite has no unsigned
                            // 64-bit); read back the same way. The full book is
                            // u64::MAX, which does not fit an i64 unless bit-cast.
                            item.spellbook.map(|mask| mask as i64),
                        ],
                    )?;
                    Ok(())
                };
            for inventory in &inventories {
                transaction
                    .execute(
                        "DELETE FROM items WHERE owner = ?1",
                        params![inventory.owner],
                    )
                    .map_err(database)?;
                for item in &inventory.items {
                    write_item(&transaction, item).map_err(database)?;
                }
            }
            if let Some(ground) = &ground {
                transaction
                    .execute("DELETE FROM items WHERE owner = 0", [])
                    .map_err(database)?;
                for item in ground {
                    write_item(&transaction, item).map_err(database)?;
                }
            }
            for serial in &removed {
                transaction
                    .execute("DELETE FROM characters WHERE serial = ?1", params![serial])
                    .map_err(database)?;
                // A gone character takes its inventory with it.
                transaction
                    .execute("DELETE FROM items WHERE owner = ?1", params![serial])
                    .map_err(database)?;
            }
            // A spawner sweep replaces the whole set.
            if let Some(spawners) = &spawners {
                transaction
                    .execute("DELETE FROM spawners", [])
                    .map_err(database)?;
                for spawner in spawners {
                    let creatures = serde_json::to_string(&spawner.creatures)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                    transaction
                        .execute(
                            "INSERT INTO spawners \
                             (id, facet, x, y, width, height, max_count, \
                              respawn_secs, remaining_secs, creatures) \
                             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                            params![
                                spawner.id,
                                spawner.facet,
                                spawner.x,
                                spawner.y,
                                spawner.width,
                                spawner.height,
                                spawner.max_count,
                                spawner.respawn_secs,
                                spawner.remaining_secs,
                                creatures,
                            ],
                        )
                        .map_err(database)?;
                }
            }
            // A decoration sweep replaces the whole set.
            if let Some(decorations) = &decorations {
                transaction
                    .execute("DELETE FROM decorations", [])
                    .map_err(database)?;
                for decoration in decorations {
                    let data = serde_json::to_string(decoration)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                    transaction
                        .execute(
                            "INSERT INTO decorations (serial, data) VALUES (?1, ?2)",
                            params![decoration.serial, data],
                        )
                        .map_err(database)?;
                }
            }
            transaction.commit().map_err(database)?;
            Ok(())
        })
        .await
    }

    async fn characters(&self) -> Result<Vec<CharacterRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection
                .lock()
                .expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare(
                    "SELECT serial, account, name, body, hue, facet, x, y, z, facing, \
                     strength, dexterity, intelligence, skills, effects, dead, quest_blob \
                     FROM characters",
                )
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    let skills: String = row.get(13)?;
                    let effects: String = row.get(14)?;
                    Ok((
                        CharacterRecord {
                            serial: row.get(0)?,
                            account: row.get(1)?,
                            name: row.get(2)?,
                            body: row.get(3)?,
                            hue: row.get(4)?,
                            facet: row.get(5)?,
                            x: row.get(6)?,
                            y: row.get(7)?,
                            z: row.get(8)?,
                            facing: row.get(9)?,
                            strength: row.get(10)?,
                            dexterity: row.get(11)?,
                            intelligence: row.get(12)?,
                            skills: Vec::new(),
                            effects: Vec::new(),
                            dead: row.get(15)?,
                            quest_blob: row.get(16)?,
                        },
                        skills,
                        effects,
                    ))
                })
                .map_err(database)?;
            let mut characters = Vec::new();
            for row in rows {
                let (mut record, skills, effects) = row.map_err(database)?;
                record.skills = serde_json::from_str(&skills)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                record.effects = serde_json::from_str(&effects)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                characters.push(record);
            }
            Ok(characters)
        })
        .await
    }

    async fn items(&self) -> Result<Vec<ItemRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection
                .lock()
                .expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare(
                    "SELECT serial, owner, graphic, hue, amount, stackable, gump, \
                     loc_kind, facet, x, y, z, parent, grid, layer, price, name, spellbook \
                     FROM items",
                )
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    let flat = FlatLocation {
                        kind: row.get(7)?,
                        facet: row.get(8)?,
                        x: row.get(9)?,
                        y: row.get(10)?,
                        z: row.get(11)?,
                        parent: row.get(12)?,
                        grid: row.get(13)?,
                        layer: row.get(14)?,
                    };
                    Ok((
                        ItemRecord {
                            serial: row.get(0)?,
                            owner: row.get(1)?,
                            graphic: row.get(2)?,
                            hue: row.get(3)?,
                            amount: row.get(4)?,
                            stackable: row.get(5)?,
                            container_gump: row.get(6)?,
                            price: row.get(15)?,
                            name: row.get(16)?,
                            // Bit-cast back from the i64 the mask was stored as.
                            spellbook: row.get::<_, Option<i64>>(17)?.map(|mask| mask as u64),
                            // A placeholder overwritten below; the location cannot be
                            // built inside `query_map`'s closure return type cleanly.
                            location: ItemLocation::Ground {
                                facet: 0,
                                x: 0,
                                y: 0,
                                z: 0,
                            },
                        },
                        flat,
                    ))
                })
                .map_err(database)?;
            let mut items = Vec::new();
            for row in rows {
                let (mut record, flat) = row.map_err(database)?;
                // Drop a row whose kind tag is unknown rather than guess a location.
                if let Some(location) = ItemLocation::inflate(&flat) {
                    record.location = location;
                    items.push(record);
                }
            }
            Ok(items)
        })
        .await
    }

    async fn spawners(&self) -> Result<Vec<SpawnerRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection
                .lock()
                .expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare(
                    "SELECT id, facet, x, y, width, height, max_count, \
                     respawn_secs, remaining_secs, creatures FROM spawners",
                )
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    let creatures: String = row.get(9)?;
                    Ok((
                        SpawnerRecord {
                            id: row.get(0)?,
                            facet: row.get(1)?,
                            x: row.get(2)?,
                            y: row.get(3)?,
                            width: row.get(4)?,
                            height: row.get(5)?,
                            max_count: row.get(6)?,
                            respawn_secs: row.get(7)?,
                            remaining_secs: row.get(8)?,
                            creatures: Vec::new(),
                        },
                        creatures,
                    ))
                })
                .map_err(database)?;
            let mut spawners = Vec::new();
            for row in rows {
                let (mut record, creatures) = row.map_err(database)?;
                record.creatures = serde_json::from_str(&creatures)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                spawners.push(record);
            }
            Ok(spawners)
        })
        .await
    }

    async fn mobiles(&self) -> Result<Vec<MobileRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection
                .lock()
                .expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare("SELECT data FROM mobiles")
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(database)?;
            let mut mobiles = Vec::new();
            for row in rows {
                let data = row.map_err(database)?;
                mobiles.push(
                    serde_json::from_str(&data).map_err(|e| StoreError::Corrupt(e.to_string()))?,
                );
            }
            Ok(mobiles)
        })
        .await
    }

    async fn decorations(&self) -> Result<Vec<DecorationRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection
                .lock()
                .expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare("SELECT data FROM decorations")
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(database)?;
            let mut decorations = Vec::new();
            for row in rows {
                let data = row.map_err(database)?;
                decorations.push(
                    serde_json::from_str(&data).map_err(|e| StoreError::Corrupt(e.to_string()))?,
                );
            }
            Ok(decorations)
        })
        .await
    }

    async fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection
                .lock()
                .expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare("SELECT name, credential FROM accounts")
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(AccountRecord {
                        name: row.get(0)?,
                        credential: row.get(1)?,
                    })
                })
                .map_err(database)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(database)
        })
        .await
    }

    async fn put_account(&self, account: &AccountRecord) -> Result<(), StoreError> {
        let connection = Arc::clone(&self.connection);
        let account = account.clone();
        blocking(move || {
            let guard = connection
                .lock()
                .expect("the sqlite mutex is never poisoned");
            guard
                .execute(
                    "INSERT OR REPLACE INTO accounts (name, credential) VALUES (?1, ?2)",
                    params![account.name, account.credential],
                )
                .map_err(database)?;
            Ok(())
        })
        .await
    }
}

/// Turn a `rusqlite` error into the trait's error. The database says what went
/// wrong; whether that is fatal is the shard's call, not this crate's.
fn database(error: rusqlite::Error) -> StoreError {
    StoreError::Database(error.to_string())
}

/// Run blocking database work off the async runtime.
///
/// A panic in the closure comes back as a [`StoreError::Database`] rather than
/// taking the runtime down: a corrupt row should fail one save, not the shard.
async fn blocking<F, T>(work: F) -> Result<T, StoreError>
where
    F: FnOnce() -> Result<T, StoreError> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(join) => Err(StoreError::Database(format!(
            "the sqlite task did not finish: {join}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn character(serial: u32, x: u16) -> CharacterRecord {
        CharacterRecord {
            serial,
            account: "admin".into(),
            name: "Alpha".into(),
            body: 0x0190,
            hue: 0,
            facet: 0,
            x,
            y: 1600,
            z: 30,
            facing: 0,
            strength: 100,
            dexterity: 100,
            intelligence: 100,
            skills: Vec::new(),
            effects: Vec::new(),
            dead: false,
            quest_blob: String::new(),
        }
    }

    fn snapshot(characters: Vec<CharacterRecord>, removed: Vec<u32>) -> Snapshot {
        Snapshot {
            tick: 1,
            schema: SCHEMA_VERSION,
            characters,
            removed,
            inventories: vec![],
            ground: None,
            spawners: None,
            mobiles: None,
            decorations: None,
        }
    }

    fn contained(serial: u32, owner: u32, container: u32) -> ItemRecord {
        ItemRecord {
            serial,
            owner,
            graphic: 0x0EED,
            hue: 0,
            amount: 1,
            stackable: false,
            container_gump: None,
            price: None,
            name: None,
            spellbook: None,
            location: ItemLocation::Contained {
                container,
                x: 0,
                y: 0,
                grid: 0,
            },
        }
    }

    /// A distinct temp path per test, cleaned up front so a leftover from a
    /// crashed run does not poison the next one.
    fn temp_db(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("openshard-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[tokio::test]
    async fn a_saved_character_reads_back() {
        let store = SqliteStore::open_in_memory().expect("open");
        store
            .save(&snapshot(vec![character(1, 100)], vec![]))
            .await
            .expect("save");
        let characters = store.characters().await.expect("read");
        assert_eq!(characters.len(), 1);
        assert_eq!(characters[0].serial, 1);
        assert_eq!(characters[0].x, 100);
    }

    #[tokio::test]
    async fn saving_the_same_serial_twice_updates_rather_than_duplicates() {
        // The primary key is the serial, so a second save of the same character
        // is an update, not a second row — the same guarantee `MemoryStore` gives.
        let store = SqliteStore::open_in_memory().expect("open");
        store
            .save(&snapshot(vec![character(1, 100)], vec![]))
            .await
            .expect("save");
        store
            .save(&snapshot(vec![character(1, 200)], vec![]))
            .await
            .expect("save");
        let characters = store.characters().await.expect("read");
        assert_eq!(characters.len(), 1);
        assert_eq!(characters[0].x, 200);
    }

    #[tokio::test]
    async fn a_removal_takes_the_character_out() {
        let store = SqliteStore::open_in_memory().expect("open");
        store
            .save(&snapshot(vec![character(1, 100)], vec![]))
            .await
            .expect("save");
        store.save(&snapshot(vec![], vec![1])).await.expect("save");
        assert!(store.characters().await.expect("read").is_empty());
    }

    #[tokio::test]
    async fn a_negative_height_survives_the_database() {
        // z is i8 and SQLite stores it as a signed integer. The mistake would be
        // reading it back as u8, turning a basement at z=-40 into z=216.
        let store = SqliteStore::open_in_memory().expect("open");
        let mut record = character(1, 100);
        record.z = -40;
        store
            .save(&snapshot(vec![record], vec![]))
            .await
            .expect("save");
        assert_eq!(store.characters().await.expect("read")[0].z, -40);
    }

    #[tokio::test]
    async fn accounts_round_trip() {
        let store = SqliteStore::open_in_memory().expect("open");
        store
            .put_account(&AccountRecord {
                name: "admin".into(),
                credential: "secret".into(),
            })
            .await
            .expect("put");
        let accounts = store.accounts().await.expect("read");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "admin");
        assert_eq!(accounts[0].credential, "secret");
    }

    #[tokio::test]
    async fn a_save_from_the_future_is_refused_and_not_written() {
        let store = SqliteStore::open_in_memory().expect("open");
        store
            .save(&snapshot(vec![character(1, 100)], vec![]))
            .await
            .expect("save");
        let future = Snapshot {
            tick: 2,
            schema: SCHEMA_VERSION + 1,
            characters: vec![character(1, 999)],
            removed: vec![],
            inventories: vec![],
            ground: None,
            spawners: None,
            mobiles: None,
            decorations: None,
        };
        let error = store.save(&future).await.expect_err("must refuse");
        assert!(matches!(error, StoreError::SchemaMismatch { .. }));
        assert_eq!(
            store.characters().await.expect("read")[0].x,
            100,
            "the refused save must not have landed"
        );
    }

    #[tokio::test]
    async fn mobiles_and_decorations_replace_and_reopen() {
        // The two whole-world tables: a sweep replaces the set, a dead mobile's
        // items go with it, and everything survives a reopen from the file.
        use crate::record::{DecorationRecord, DoorState, Inventory, MobileRecord};
        fn mobile(serial: u32, hits: u16) -> MobileRecord {
            MobileRecord {
                serial,
                body: 0x00C8,
                hue: 0,
                facet: 0,
                x: 1400,
                y: 1600,
                z: 0,
                facing: 0,
                name: Some("Mirabel".into()),
                hits_current: hits,
                hits_max: 30,
                notoriety: 3,
                damage: 3,
                resistance: 0,
                swing: 0,
                sight: 8,
                aggression: 0,
                beat: 0,
                ranged: 0,
                ranged_kind: 0,
                wander: true,
                banker: false,
                vendor: true,
                npc_home: Some((1400, 1600, 0)),
                npc_wander: 2,
                spawned_by: None,
                effects: Vec::new(),
                skills: Vec::new(),
            }
        }
        let decoration = DecorationRecord {
            serial: 0x4000_0100,
            graphic: 0x0675,
            hue: 0,
            facet: 0,
            x: 1401,
            y: 1600,
            z: 0,
            door: Some(DoorState {
                closed_graphic: 0x0675,
                open_graphic: 0x0676,
                offset_x: -1,
                offset_y: 1,
                is_open: true,
            }),
            container_gump: None,
        };
        let path = temp_db("world-tables");
        {
            let store = SqliteStore::open(&path).expect("open");
            let mut first = snapshot(vec![], vec![]);
            first.inventories = vec![Inventory {
                owner: 2,
                items: vec![contained(0x4000_0001, 2, 2)],
            }];
            first.mobiles = Some(vec![mobile(2, 30), mobile(3, 30)]);
            first.decorations = Some(vec![decoration.clone()]);
            store.save(&first).await.expect("save");
            // Mobile 2 dies; the next sweep carries only the wounded survivor.
            let mut second = snapshot(vec![], vec![]);
            second.mobiles = Some(vec![mobile(3, 7)]);
            store.save(&second).await.expect("save");
        }
        {
            let store = SqliteStore::open(&path).expect("reopen");
            let mobiles = store.mobiles().await.expect("read");
            assert_eq!(mobiles.len(), 1, "the dead mobile is gone");
            assert_eq!(mobiles[0].serial, 3);
            assert_eq!(mobiles[0].hits_current, 7, "wounds survived the reopen");
            assert_eq!(mobiles[0].npc_home, Some((1400, 1600, 0)));
            assert!(
                store.items().await.expect("read").is_empty(),
                "the dead mobile's items went with it"
            );
            let decorations = store.decorations().await.expect("read");
            assert_eq!(decorations.len(), 1);
            assert_eq!(decorations[0], decoration, "door state and all");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn a_priced_named_item_survives_a_reopen() {
        // Vendor stock: the price and label columns round-trip, or a restored
        // shop sells nameless wares for a coin.
        let path = temp_db("priced-item");
        {
            let store = SqliteStore::open(&path).expect("open");
            let mut item = contained(0x4000_0001, 1, 1);
            item.price = Some(4);
            item.name = Some("black pearl".into());
            let mut snap = snapshot(vec![character(1, 100)], vec![]);
            snap.inventories = vec![crate::record::Inventory {
                owner: 1,
                items: vec![item],
            }];
            store.save(&snap).await.expect("save");
        }
        {
            let store = SqliteStore::open(&path).expect("reopen");
            let items = store.items().await.expect("read");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].price, Some(4));
            assert_eq!(items[0].name.as_deref(), Some("black pearl"));
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn a_spellbook_mask_survives_a_reopen() {
        // The learned-spell bitmask round-trips through the i64 column even with
        // the top bit set (u64::MAX, the full book) — a signed widen would lose
        // it, so it is stored and read as a bit-cast. Without it a restored book
        // has no spells and refuses to open.
        let path = temp_db("spellbook-item");
        let full = u64::MAX;
        {
            let store = SqliteStore::open(&path).expect("open");
            let mut item = contained(0x4000_0001, 1, 1);
            item.spellbook = Some(full);
            let mut snap = snapshot(vec![character(1, 100)], vec![]);
            snap.inventories = vec![crate::record::Inventory {
                owner: 1,
                items: vec![item],
            }];
            store.save(&snap).await.expect("save");
        }
        {
            let store = SqliteStore::open(&path).expect("reopen");
            let items = store.items().await.expect("read");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].spellbook, Some(full));
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn it_persists_across_a_reopen() {
        // The whole point of the crate: write to a real file, close it, open a
        // fresh store on the same file, and find the world still there.
        let path = temp_db("reopen");
        {
            let store = SqliteStore::open(&path).expect("open");
            store
                .save(&snapshot(vec![character(7, 4242)], vec![]))
                .await
                .expect("save");
            store
                .put_account(&AccountRecord {
                    name: "admin".into(),
                    credential: "x".into(),
                })
                .await
                .expect("put");
        }
        {
            let store = SqliteStore::open(&path).expect("reopen");
            let characters = store.characters().await.expect("read");
            assert_eq!(characters.len(), 1);
            assert_eq!(characters[0].serial, 7);
            assert_eq!(characters[0].x, 4242, "position survived the restart");
            assert_eq!(store.accounts().await.expect("read").len(), 1);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn opening_a_database_from_the_future_is_refused() {
        // Older code opening a newer save must refuse, not read it and write the
        // loss back on the next save.
        let path = temp_db("future");
        {
            let connection = Connection::open(&path).expect("raw open");
            connection
                .execute_batch(
                    "CREATE TABLE meta (key TEXT PRIMARY KEY, value INTEGER NOT NULL);\
                     INSERT INTO meta (key, value) VALUES ('schema', 999);",
                )
                .expect("stamp a future schema");
        }
        let error = SqliteStore::open(&path).expect_err("must refuse");
        assert!(matches!(
            error,
            StoreError::SchemaMismatch { found: 999, .. }
        ));
        let _ = std::fs::remove_file(&path);
    }
}
