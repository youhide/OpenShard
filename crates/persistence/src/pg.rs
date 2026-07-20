//! The PostgreSQL backend.
//!
//! # A second backend, not a better one
//!
//! `PgStore` sits behind the same [`Store`] trait as
//! [`SqliteStore`](crate::SqliteStore), and which a shard runs is the operator's
//! choice, not a tier. SQLite keeps a live shard on one disk with no server to
//! run; PostgreSQL puts the same world on a database another machine can reach,
//! shared by more than one process. The simulation cannot tell them apart — the
//! trait is the seam that makes the choice a config line rather than a rewrite.
//!
//! # Async all the way down, so no `spawn_blocking`
//!
//! Where the SQLite backend wraps a blocking C library in
//! [`tokio::task::spawn_blocking`], `tokio-postgres` is native async: every call
//! is already a network round-trip that yields rather than blocks. What this file
//! adds is the one piece the driver leaves to its caller — the *connection
//! future*, which drives the actual socket and which nothing works without —
//! spawned onto the runtime so the client it is paired with can make progress.
//!
//! # One connection behind an async mutex
//!
//! The same shape as SQLite's, for the same reasons. A transaction borrows the
//! client mutably, so the client cannot simply be shared by `&`; and saves are
//! infrequent and off the tick, so serialising them through a single connection
//! costs nothing that matters and keeps the all-or-nothing write the trait
//! demands. An async [`Mutex`] rather than a `std` one because the guard is held
//! across `.await` — the whole point is that holding it never blocks the runtime.
//!
//! # No TLS yet
//!
//! Connections are made with [`NoTls`]. That is enough for a database on the same
//! host or a trusted network, which is where a first backend earns its keep;
//! wiring an encryptor in is a later, additive change and does not touch the
//! shape here. The connection string is never logged, because it can carry a
//! password.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio_postgres::{Client, NoTls, Row};

use crate::journal::Snapshot;
use crate::record::{
    AccountRecord, CharacterRecord, DecorationRecord, ItemLocation, ItemRecord, MobileRecord,
    SpawnerRecord, SCHEMA_VERSION,
};
use crate::store::{Store, StoreError};

/// The tables, created on connect. `IF NOT EXISTS` so connecting to a database
/// that already has them is a no-op rather than an error.
///
/// PostgreSQL has no unsigned integers: a `serial` is a `u32`, stored as
/// `BIGINT` so its full range fits with room to spare, and the small fields go in
/// `INTEGER`. The conversion back is checked — see [`character_from_row`] — so a
/// value the column should never hold surfaces as [`StoreError::Corrupt`] rather
/// than a silently wrong character.
const SCHEMA_SQL: &str = "\
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value BIGINT NOT NULL);
CREATE TABLE IF NOT EXISTS accounts (
    name       TEXT PRIMARY KEY,
    credential TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS characters (
    serial  BIGINT PRIMARY KEY,
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
    skills  TEXT NOT NULL,
    effects  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS items (
    serial    BIGINT PRIMARY KEY,
    owner     BIGINT NOT NULL,
    graphic   INTEGER NOT NULL,
    hue       INTEGER NOT NULL,
    amount    INTEGER NOT NULL,
    stackable BOOLEAN NOT NULL,
    gump      INTEGER,
    loc_kind INTEGER NOT NULL,
    facet    INTEGER NOT NULL,
    x        INTEGER NOT NULL,
    y        INTEGER NOT NULL,
    z        INTEGER NOT NULL,
    parent   BIGINT NOT NULL,
    grid     INTEGER NOT NULL,
    layer    INTEGER NOT NULL,
    price    BIGINT,
    name     TEXT,
    -- a spellbook's learned-spell bitmask, so a bought book still opens after a relog.
    spellbook BIGINT
);
CREATE INDEX IF NOT EXISTS items_owner ON items (owner);
CREATE TABLE IF NOT EXISTS mobiles (
    serial BIGINT PRIMARY KEY,
    data   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS decorations (
    serial BIGINT PRIMARY KEY,
    data   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS spawners (
    id             BIGINT PRIMARY KEY,
    facet          INTEGER NOT NULL,
    x              INTEGER NOT NULL,
    y              INTEGER NOT NULL,
    width          INTEGER NOT NULL,
    height         INTEGER NOT NULL,
    max_count      INTEGER NOT NULL,
    respawn_secs   BIGINT NOT NULL,
    remaining_secs BIGINT NOT NULL,
    creatures      TEXT NOT NULL
);";

/// A `Store` kept in a PostgreSQL database.
///
/// One of the two backends behind the [`Store`] trait; SQLite is the other, and
/// which a shard runs is the operator's choice, not a tier. The character's
/// [`serial`](CharacterRecord::serial) is the primary key, because that is the
/// identity that has to survive a restart.
pub struct PgStore {
    /// One connection, behind an async mutex — see the module docs for why a
    /// single serialised connection rather than a pool.
    client: Arc<Mutex<Client>>,
}

impl fmt::Debug for PgStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The client holds a live socket and a connection string that can carry a
        // password; neither belongs in a debug line.
        formatter.debug_struct("PgStore").finish_non_exhaustive()
    }
}

impl PgStore {
    /// Connect to the database named by a `postgres://` URL and make sure the
    /// tables and schema stamp are in place.
    ///
    /// Refuses a database written by a build with a different [`SCHEMA_VERSION`]
    /// rather than reading it and silently dropping what it does not understand —
    /// the same refusal the SQLite backend makes.
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let (client, connection) = tokio_postgres::connect(url, NoTls)
            .await
            .map_err(database)?;

        // The connection future is the half of the driver that owns the socket:
        // until something polls it, the client's calls never leave the process.
        // It ends when the client is dropped or the server hangs up — and when
        // the server hangs up, every pending and future client call already
        // returns its own error, which is where the shard reacts. So there is
        // nothing to do here but let it finish.
        tokio::spawn(async move {
            let _ = connection.await;
        });

        Self::init(client).await
    }

    async fn init(client: Client) -> Result<Self, StoreError> {
        client.batch_execute(SCHEMA_SQL).await.map_err(database)?;

        // The schema version is stamped once, on a fresh database, and checked on
        // every connect after. A database from the future is refused, not read.
        let found: Option<i64> = client
            .query_opt("SELECT value FROM meta WHERE key = 'schema'", &[])
            .await
            .map_err(database)?
            .map(|row| row.get(0));
        match found {
            Some(version) if version != i64::from(SCHEMA_VERSION) => {
                return Err(StoreError::SchemaMismatch {
                    found: u32::try_from(version).unwrap_or(u32::MAX),
                    understood: SCHEMA_VERSION,
                });
            }
            Some(_) => {}
            None => {
                client
                    .execute(
                        "INSERT INTO meta (key, value) VALUES ('schema', $1)",
                        &[&i64::from(SCHEMA_VERSION)],
                    )
                    .await
                    .map_err(database)?;
            }
        }

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
        })
    }
}

#[async_trait]
impl Store for PgStore {
    async fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError> {
        // Refuse before touching the database, exactly as the other backends do:
        // a snapshot from a future schema must not be half-written.
        if snapshot.schema != SCHEMA_VERSION {
            return Err(StoreError::SchemaMismatch {
                found: snapshot.schema,
                understood: SCHEMA_VERSION,
            });
        }

        let mut client = self.client.lock().await;
        // One transaction: all of the snapshot or none of it. A half-written
        // world is a world that never existed — see `crate::journal`.
        let transaction = client.transaction().await.map_err(database)?;
        for record in &snapshot.characters {
            let skills = serde_json::to_string(&record.skills)
                .map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let effects = serde_json::to_string(&record.effects)
                .map_err(|e| StoreError::Corrupt(e.to_string()))?;
            transaction
                .execute(
                    "INSERT INTO characters \
                     (serial, account, name, body, hue, facet, x, y, z, facing, \
                      strength, dexterity, intelligence, skills, effects) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15) \
                     ON CONFLICT (serial) DO UPDATE SET \
                     account = EXCLUDED.account, name = EXCLUDED.name, \
                     body = EXCLUDED.body, hue = EXCLUDED.hue, facet = EXCLUDED.facet, \
                     x = EXCLUDED.x, y = EXCLUDED.y, z = EXCLUDED.z, facing = EXCLUDED.facing, \
                     strength = EXCLUDED.strength, dexterity = EXCLUDED.dexterity, \
                     intelligence = EXCLUDED.intelligence, skills = EXCLUDED.skills, \
                     effects = EXCLUDED.effects",
                    &[
                        &i64::from(record.serial),
                        &record.account,
                        &record.name,
                        &i32::from(record.body),
                        &i32::from(record.hue),
                        &i32::from(record.facet),
                        &i32::from(record.x),
                        &i32::from(record.y),
                        &i32::from(record.z),
                        &i32::from(record.facing),
                        &i32::from(record.strength),
                        &i32::from(record.dexterity),
                        &i32::from(record.intelligence),
                        &skills,
                        &effects,
                    ],
                )
                .await
                .map_err(database)?;
        }
        // The mobiles sweep runs BEFORE the inventories: it clears every item
        // owned by any previously saved mobile (a dead vendor's crate must not
        // linger), and the same snapshot re-writes the live mobiles' inventories
        // right after — the world side always sweeps the two together.
        if let Some(mobiles) = &snapshot.mobiles {
            transaction
                .execute(
                    "DELETE FROM items WHERE owner IN (SELECT serial FROM mobiles)",
                    &[],
                )
                .await
                .map_err(database)?;
            transaction
                .execute("DELETE FROM mobiles", &[])
                .await
                .map_err(database)?;
            for mobile in mobiles {
                let data = serde_json::to_string(mobile)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO mobiles (serial, data) VALUES ($1, $2)",
                        &[&i64::from(mobile.serial), &data],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        for inventory in &snapshot.inventories {
            transaction
                .execute(
                    "DELETE FROM items WHERE owner = $1",
                    &[&i64::from(inventory.owner)],
                )
                .await
                .map_err(database)?;
            for item in &inventory.items {
                insert_item(&transaction, item).await?;
            }
        }
        if let Some(ground) = &snapshot.ground {
            transaction
                .execute("DELETE FROM items WHERE owner = 0", &[])
                .await
                .map_err(database)?;
            for item in ground {
                insert_item(&transaction, item).await?;
            }
        }
        for serial in &snapshot.removed {
            transaction
                .execute(
                    "DELETE FROM characters WHERE serial = $1",
                    &[&i64::from(*serial)],
                )
                .await
                .map_err(database)?;
            // A gone character takes its inventory with it.
            transaction
                .execute("DELETE FROM items WHERE owner = $1", &[&i64::from(*serial)])
                .await
                .map_err(database)?;
        }
        if let Some(spawners) = &snapshot.spawners {
            transaction
                .execute("DELETE FROM spawners", &[])
                .await
                .map_err(database)?;
            for spawner in spawners {
                let creatures = serde_json::to_string(&spawner.creatures)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO spawners \
                         (id, facet, x, y, width, height, max_count, \
                          respawn_secs, remaining_secs, creatures) \
                         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
                        &[
                            &i64::from(spawner.id),
                            &i32::from(spawner.facet),
                            &i32::from(spawner.x),
                            &i32::from(spawner.y),
                            &i32::from(spawner.width),
                            &i32::from(spawner.height),
                            &i32::from(spawner.max_count),
                            &(spawner.respawn_secs as i64),
                            &(spawner.remaining_secs as i64),
                            &creatures,
                        ],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        // A decoration sweep replaces the whole set.
        if let Some(decorations) = &snapshot.decorations {
            transaction
                .execute("DELETE FROM decorations", &[])
                .await
                .map_err(database)?;
            for decoration in decorations {
                let data = serde_json::to_string(decoration)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO decorations (serial, data) VALUES ($1, $2)",
                        &[&i64::from(decoration.serial), &data],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        transaction.commit().await.map_err(database)?;
        Ok(())
    }

    async fn characters(&self) -> Result<Vec<CharacterRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT serial, account, name, body, hue, facet, x, y, z, facing, \
                 strength, dexterity, intelligence, skills, effects FROM characters",
                &[],
            )
            .await
            .map_err(database)?;
        rows.iter().map(character_from_row).collect()
    }

    async fn items(&self) -> Result<Vec<ItemRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT serial, owner, graphic, hue, amount, stackable, gump, \
                 loc_kind, facet, x, y, z, parent, grid, layer, price, name, spellbook \
                 FROM items",
                &[],
            )
            .await
            .map_err(database)?;
        rows.iter().filter_map(item_from_row).collect()
    }

    async fn mobiles(&self) -> Result<Vec<MobileRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query("SELECT data FROM mobiles", &[])
            .await
            .map_err(database)?;
        rows.iter()
            .map(|row| {
                serde_json::from_str(row.get::<_, &str>(0))
                    .map_err(|e| StoreError::Corrupt(e.to_string()))
            })
            .collect()
    }

    async fn decorations(&self) -> Result<Vec<DecorationRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query("SELECT data FROM decorations", &[])
            .await
            .map_err(database)?;
        rows.iter()
            .map(|row| {
                serde_json::from_str(row.get::<_, &str>(0))
                    .map_err(|e| StoreError::Corrupt(e.to_string()))
            })
            .collect()
    }

    async fn spawners(&self) -> Result<Vec<SpawnerRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT id, facet, x, y, width, height, max_count, \
                 respawn_secs, remaining_secs, creatures FROM spawners",
                &[],
            )
            .await
            .map_err(database)?;
        rows.iter().map(spawner_from_row).collect()
    }

    async fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query("SELECT name, credential FROM accounts", &[])
            .await
            .map_err(database)?;
        Ok(rows
            .iter()
            .map(|row| AccountRecord {
                name: row.get(0),
                credential: row.get(1),
            })
            .collect())
    }

    async fn put_account(&self, account: &AccountRecord) -> Result<(), StoreError> {
        let client = self.client.lock().await;
        client
            .execute(
                "INSERT INTO accounts (name, credential) VALUES ($1, $2) \
                 ON CONFLICT (name) DO UPDATE SET credential = EXCLUDED.credential",
                &[&account.name, &account.credential],
            )
            .await
            .map_err(database)?;
        Ok(())
    }
}

/// Rebuild a [`CharacterRecord`] from a row, checking every narrowing.
///
/// The columns are `BIGINT`/`INTEGER` because PostgreSQL has no unsigned or
/// one-byte integers, so each field is wider on disk than in the record. A value
/// that does not fit the record's type — a `z` above 127, a `serial` past
/// `u32::MAX` — means the row was written by something other than this code, so
/// it is [`StoreError::Corrupt`], not a silently truncated character standing in
/// the wrong place.
fn character_from_row(row: &Row) -> Result<CharacterRecord, StoreError> {
    Ok(CharacterRecord {
        serial: u32::try_from(row.get::<_, i64>(0)).map_err(|_| corrupt("serial"))?,
        account: row.get(1),
        name: row.get(2),
        body: u16::try_from(row.get::<_, i32>(3)).map_err(|_| corrupt("body"))?,
        hue: u16::try_from(row.get::<_, i32>(4)).map_err(|_| corrupt("hue"))?,
        facet: u8::try_from(row.get::<_, i32>(5)).map_err(|_| corrupt("facet"))?,
        x: u16::try_from(row.get::<_, i32>(6)).map_err(|_| corrupt("x"))?,
        y: u16::try_from(row.get::<_, i32>(7)).map_err(|_| corrupt("y"))?,
        z: i8::try_from(row.get::<_, i32>(8)).map_err(|_| corrupt("z"))?,
        facing: u8::try_from(row.get::<_, i32>(9)).map_err(|_| corrupt("facing"))?,
        strength: u16::try_from(row.get::<_, i32>(10)).map_err(|_| corrupt("strength"))?,
        dexterity: u16::try_from(row.get::<_, i32>(11)).map_err(|_| corrupt("dexterity"))?,
        intelligence: u16::try_from(row.get::<_, i32>(12)).map_err(|_| corrupt("intelligence"))?,
        skills: serde_json::from_str(row.get::<_, &str>(13))
            .map_err(|e| StoreError::Corrupt(e.to_string()))?,
        effects: serde_json::from_str(row.get::<_, &str>(14))
            .map_err(|e| StoreError::Corrupt(e.to_string()))?,
    })
}

/// Write one item, flattening its location into the union of columns. Shared by
/// the inventory and ground writes in `save`.
async fn insert_item(
    transaction: &tokio_postgres::Transaction<'_>,
    item: &ItemRecord,
) -> Result<(), StoreError> {
    // (kind, facet, x, y, z, parent, grid, layer) — the fields a kind does not use
    // are zero, the same flat form the SQLite backend writes.
    let (kind, facet, x, y, z, parent, grid, layer): (i32, i32, i32, i32, i32, i64, i32, i32) =
        match item.location {
            ItemLocation::Ground { facet, x, y, z } => (
                0,
                i32::from(facet),
                i32::from(x),
                i32::from(y),
                i32::from(z),
                0,
                0,
                0,
            ),
            ItemLocation::Contained {
                container,
                x,
                y,
                grid,
            } => (
                1,
                0,
                i32::from(x),
                i32::from(y),
                0,
                i64::from(container),
                i32::from(grid),
                0,
            ),
            ItemLocation::Equipped { mobile, layer } => {
                (2, 0, 0, 0, 0, i64::from(mobile), 0, i32::from(layer))
            }
        };
    transaction
        .execute(
            "INSERT INTO items \
             (serial, owner, graphic, hue, amount, stackable, gump, \
              loc_kind, facet, x, y, z, parent, grid, layer, price, name, spellbook) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18) \
             ON CONFLICT (serial) DO UPDATE SET \
             owner = EXCLUDED.owner, graphic = EXCLUDED.graphic, hue = EXCLUDED.hue, \
             amount = EXCLUDED.amount, stackable = EXCLUDED.stackable, gump = EXCLUDED.gump, \
             loc_kind = EXCLUDED.loc_kind, \
             facet = EXCLUDED.facet, x = EXCLUDED.x, y = EXCLUDED.y, z = EXCLUDED.z, \
             parent = EXCLUDED.parent, grid = EXCLUDED.grid, layer = EXCLUDED.layer, \
             price = EXCLUDED.price, name = EXCLUDED.name, spellbook = EXCLUDED.spellbook",
            &[
                &i64::from(item.serial),
                &i64::from(item.owner),
                &i32::from(item.graphic),
                &i32::from(item.hue),
                &i32::from(item.amount),
                &item.stackable,
                &item.container_gump.map(i32::from),
                &kind,
                &facet,
                &x,
                &y,
                &z,
                &parent,
                &grid,
                &layer,
                &item.price.map(i64::from),
                &item.name,
                // A u64 mask reinterpreted as i64 (Postgres BIGINT is signed);
                // the full book is u64::MAX, so it must be bit-cast, not widened.
                &item.spellbook.map(|mask| mask as i64),
            ],
        )
        .await
        .map_err(database)?;
    Ok(())
}

/// Rebuild an [`ItemRecord`] from a row, or drop it (`None`) if its location kind
/// is one no version wrote. Every narrowing is checked, like [`character_from_row`].
fn item_from_row(row: &Row) -> Option<Result<ItemRecord, StoreError>> {
    fn build(row: &Row) -> Result<Option<ItemRecord>, StoreError> {
        let kind: i32 = row.get(7);
        let facet = u8::try_from(row.get::<_, i32>(8)).map_err(|_| corrupt("facet"))?;
        let x = u16::try_from(row.get::<_, i32>(9)).map_err(|_| corrupt("x"))?;
        let y = u16::try_from(row.get::<_, i32>(10)).map_err(|_| corrupt("y"))?;
        let z = i8::try_from(row.get::<_, i32>(11)).map_err(|_| corrupt("z"))?;
        let parent = u32::try_from(row.get::<_, i64>(12)).map_err(|_| corrupt("parent"))?;
        let grid = u8::try_from(row.get::<_, i32>(13)).map_err(|_| corrupt("grid"))?;
        let layer = u8::try_from(row.get::<_, i32>(14)).map_err(|_| corrupt("layer"))?;
        let location = match kind {
            0 => ItemLocation::Ground { facet, x, y, z },
            1 => ItemLocation::Contained {
                container: parent,
                x,
                y,
                grid,
            },
            2 => ItemLocation::Equipped {
                mobile: parent,
                layer,
            },
            _ => return Ok(None),
        };
        Ok(Some(ItemRecord {
            serial: u32::try_from(row.get::<_, i64>(0)).map_err(|_| corrupt("serial"))?,
            owner: u32::try_from(row.get::<_, i64>(1)).map_err(|_| corrupt("owner"))?,
            graphic: u16::try_from(row.get::<_, i32>(2)).map_err(|_| corrupt("graphic"))?,
            hue: u16::try_from(row.get::<_, i32>(3)).map_err(|_| corrupt("hue"))?,
            amount: u16::try_from(row.get::<_, i32>(4)).map_err(|_| corrupt("amount"))?,
            stackable: row.get(5),
            container_gump: row
                .get::<_, Option<i32>>(6)
                .map(|g| u16::try_from(g).map_err(|_| corrupt("gump")))
                .transpose()?,
            price: row
                .get::<_, Option<i64>>(15)
                .map(|p| u32::try_from(p).map_err(|_| corrupt("price")))
                .transpose()?,
            name: row.get(16),
            // Bit-cast back from the i64 the mask was stored as.
            spellbook: row.get::<_, Option<i64>>(17).map(|mask| mask as u64),
            location,
        }))
    }
    build(row).transpose()
}

/// Rebuild a [`SpawnerRecord`] from a row, checking every narrowing and parsing
/// the creature list back from its JSON column.
fn spawner_from_row(row: &Row) -> Result<SpawnerRecord, StoreError> {
    let creatures: String = row.get(9);
    Ok(SpawnerRecord {
        id: u32::try_from(row.get::<_, i64>(0)).map_err(|_| corrupt("id"))?,
        facet: u8::try_from(row.get::<_, i32>(1)).map_err(|_| corrupt("facet"))?,
        x: u16::try_from(row.get::<_, i32>(2)).map_err(|_| corrupt("x"))?,
        y: u16::try_from(row.get::<_, i32>(3)).map_err(|_| corrupt("y"))?,
        width: u16::try_from(row.get::<_, i32>(4)).map_err(|_| corrupt("width"))?,
        height: u16::try_from(row.get::<_, i32>(5)).map_err(|_| corrupt("height"))?,
        max_count: u16::try_from(row.get::<_, i32>(6)).map_err(|_| corrupt("max_count"))?,
        respawn_secs: u64::try_from(row.get::<_, i64>(7)).map_err(|_| corrupt("respawn_secs"))?,
        remaining_secs: u64::try_from(row.get::<_, i64>(8))
            .map_err(|_| corrupt("remaining_secs"))?,
        creatures: serde_json::from_str(&creatures)
            .map_err(|e| StoreError::Corrupt(e.to_string()))?,
    })
}

/// A column held a value outside the range of the record field it maps to.
fn corrupt(field: &str) -> StoreError {
    StoreError::Corrupt(format!(
        "the {field} column holds a value outside the range of its record field"
    ))
}

/// Turn a `tokio_postgres` error into the trait's error. The database says what
/// went wrong; whether that is fatal is the shard's call, not this crate's.
fn database(error: tokio_postgres::Error) -> StoreError {
    StoreError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests need a real PostgreSQL. They read a connection URL from
    // `OPENSHARD_POSTGRES` and skip when it is unset, the same bargain the
    // client-file tests strike with `OPENSHARD_CLIENT`: a checkout with no
    // database configured stays green, and the coverage is there for anyone who
    // points the variable at a server.
    //
    // They share one database's tables, so a single async lock serialises them
    // and each one drops the tables first — no ordering between tests, no
    // leftovers from a crashed run.
    static LOCK: Mutex<()> = Mutex::const_new(());

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

    /// Connect if a database is configured, dropping the tables so the test
    /// starts from nothing. `None` means "no `OPENSHARD_POSTGRES`; skip".
    async fn fresh() -> Option<PgStore> {
        let url = std::env::var("OPENSHARD_POSTGRES").ok()?;
        let (client, connection) = tokio_postgres::connect(&url, NoTls)
            .await
            .expect("connect to the test database");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
            .batch_execute(
                "DROP TABLE IF EXISTS characters; \
                 DROP TABLE IF EXISTS accounts; \
                 DROP TABLE IF EXISTS meta;",
            )
            .await
            .expect("reset the test database");
        drop(client);
        Some(PgStore::connect(&url).await.expect("open the store"))
    }

    #[tokio::test]
    async fn a_saved_character_reads_back() {
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
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
        // is an upsert, not a second row — the same guarantee the other backends
        // give. `ON CONFLICT DO UPDATE` is where PostgreSQL spells that.
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
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
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
        store
            .save(&snapshot(vec![character(1, 100)], vec![]))
            .await
            .expect("save");
        store.save(&snapshot(vec![], vec![1])).await.expect("save");
        assert!(store.characters().await.expect("read").is_empty());
    }

    #[tokio::test]
    async fn a_negative_height_survives_the_database() {
        // z is i8 and the column is a signed INTEGER. The mistake would be reading
        // it back as u8, turning a basement at z=-40 into z=216.
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
        let mut record = character(1, 100);
        record.z = -40;
        store
            .save(&snapshot(vec![record], vec![]))
            .await
            .expect("save");
        assert_eq!(store.characters().await.expect("read")[0].z, -40);
    }

    #[tokio::test]
    async fn a_full_range_serial_survives_the_database() {
        // The widest serial an item can carry is 0x7FFF_FFFF. Stored as BIGINT and
        // read back through a checked narrowing, it must come out unchanged rather
        // than tripping the corruption guard.
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
        store
            .save(&snapshot(vec![character(0x7FFF_FFFF, 100)], vec![]))
            .await
            .expect("save");
        assert_eq!(
            store.characters().await.expect("read")[0].serial,
            0x7FFF_FFFF
        );
    }

    #[tokio::test]
    async fn accounts_round_trip() {
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
        store
            .put_account(&AccountRecord {
                name: "admin".into(),
                credential: "secret".into(),
            })
            .await
            .expect("put");
        // And an upsert on the same name updates rather than duplicating.
        store
            .put_account(&AccountRecord {
                name: "admin".into(),
                credential: "changed".into(),
            })
            .await
            .expect("put again");
        let accounts = store.accounts().await.expect("read");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "admin");
        assert_eq!(accounts[0].credential, "changed");
    }

    #[tokio::test]
    async fn a_save_from_the_future_is_refused_and_not_written() {
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
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
    async fn it_persists_across_a_reconnect() {
        // The whole point of the crate: write, drop the store, connect a fresh one
        // to the same database, and find the world still there.
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
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
        drop(store);

        let url = std::env::var("OPENSHARD_POSTGRES").expect("still set");
        let reopened = PgStore::connect(&url).await.expect("reconnect");
        let characters = reopened.characters().await.expect("read");
        assert_eq!(characters.len(), 1);
        assert_eq!(characters[0].serial, 7);
        assert_eq!(characters[0].x, 4242, "position survived the reconnect");
        assert_eq!(reopened.accounts().await.expect("read").len(), 1);
    }

    #[tokio::test]
    async fn connecting_to_a_database_from_the_future_is_refused() {
        // Older code connecting to a newer save must refuse, not read it and write
        // the loss back on the next save.
        let _guard = LOCK.lock().await;
        let url = match std::env::var("OPENSHARD_POSTGRES").ok() {
            Some(url) => url,
            None => return,
        };
        let (client, connection) = tokio_postgres::connect(&url, NoTls).await.expect("connect");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
            .batch_execute(
                "DROP TABLE IF EXISTS characters; \
                 DROP TABLE IF EXISTS accounts; \
                 DROP TABLE IF EXISTS meta; \
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value BIGINT NOT NULL); \
                 INSERT INTO meta (key, value) VALUES ('schema', 999);",
            )
            .await
            .expect("stamp a future schema");
        drop(client);

        let error = PgStore::connect(&url).await.expect_err("must refuse");
        assert!(matches!(
            error,
            StoreError::SchemaMismatch { found: 999, .. }
        ));
    }
}
