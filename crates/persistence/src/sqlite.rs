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
use crate::record::{AccountRecord, CharacterRecord, SCHEMA_VERSION};
use crate::store::{Store, StoreError};

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
    facing  INTEGER NOT NULL
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
        blocking(move || {
            let mut guard = connection
                .lock()
                .expect("the sqlite mutex is never poisoned");
            // One transaction: all of the snapshot or none of it. A half-written
            // world is a world that never existed — see `crate::journal`.
            let transaction = guard.transaction().map_err(database)?;
            for record in &characters {
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO characters \
                         (serial, account, name, body, hue, facet, x, y, z, facing) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
                        ],
                    )
                    .map_err(database)?;
            }
            for serial in &removed {
                transaction
                    .execute("DELETE FROM characters WHERE serial = ?1", params![serial])
                    .map_err(database)?;
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
                    "SELECT serial, account, name, body, hue, facet, x, y, z, facing \
                     FROM characters",
                )
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(CharacterRecord {
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
                    })
                })
                .map_err(database)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(database)
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
        }
    }

    fn snapshot(characters: Vec<CharacterRecord>, removed: Vec<u32>) -> Snapshot {
        Snapshot {
            tick: 1,
            schema: SCHEMA_VERSION,
            characters,
            removed,
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
