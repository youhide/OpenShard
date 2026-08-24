//! The PostgreSQL backend.
//!
//! # A second backend, not a better one
//!
//! `PgStore` is one [`Store`](crate::Store) variant, alongside
//! [`SqliteStore`](crate::SqliteStore). Which a shard runs is the operator's
//! choice, not a tier. SQLite keeps a live shard on one disk with no server to
//! run; PostgreSQL puts the same world on a database another machine can reach,
//! shared by more than one process. The simulation makes that closed choice
//! explicitly from configuration.
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
//! costs nothing that matters and keeps the all-or-nothing write the store
//! requires. An async [`Mutex`] rather than a `std` one because the guard is held
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

use openshard_protocol::identity::{AccountName, CharacterName};
use openshard_protocol::serial::Serial;
use tokio::sync::Mutex;
use tokio_postgres::{Client, NoTls, Row};

use crate::journal::Snapshot;
use crate::record::{
    AccountRecord, CharacterRecord, DecorationRecord, GuildRecord, ItemLocation, ItemRecord, MobileRecord,
    RegionRecord, SCHEMA_VERSION, SpawnerRecord, WorldRecord,
};
use crate::store::StoreError;

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
    effects  TEXT NOT NULL,
    dead    BOOLEAN NOT NULL,
    -- Standing: how widely and which way it is known, and how many innocents it
    -- has killed. The last is what makes a repeat killer permanently red.
    fame    INTEGER NOT NULL DEFAULT 0,
    karma   INTEGER NOT NULL DEFAULT 0,
    murders INTEGER NOT NULL DEFAULT 0,
    quests TEXT NOT NULL DEFAULT '[]',
    done_quests TEXT NOT NULL DEFAULT '[]',
    -- Which way the three stats train, and how long since each last rose. JSON,
    -- like the skills beside it: six small numbers only useful together.
    stat_locks TEXT NOT NULL DEFAULT '{}',
    -- Guild membership. Columns rather than a roster table, and no foreign key on
    -- `guilds`: an id naming a guild that is gone reads as no guild, and a
    -- constraint would turn that into a refused write instead.
    guild           INTEGER,
    guild_title     TEXT NOT NULL DEFAULT '',
    -- Where in the guild: 0 Ronin through 4 Leader. See CharacterRecord.
    guild_rank      INTEGER NOT NULL DEFAULT 0,
    guild_candidate INTEGER
);
-- Every guild. Relations and standing offers are JSON, and both are written on
-- both sides, so restoring is idempotent rather than additive.
CREATE TABLE IF NOT EXISTS guilds (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL,
    abbreviation TEXT NOT NULL,
    leader       BIGINT NOT NULL,
    relations    TEXT NOT NULL DEFAULT '[]',
    proposals    TEXT NOT NULL DEFAULT '[]',
    -- Which alliance, by `alliances.id`. No foreign key, for the same reason a
    -- character's guild has none.
    alliance     INTEGER
);
-- Every named alliance. The membership is written here rather than on the
-- guilds, and the guild's own `alliance` column is the back-pointer.
CREATE TABLE IF NOT EXISTS alliances (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL,
    leader  INTEGER NOT NULL,
    members TEXT NOT NULL DEFAULT '[]',
    pending TEXT NOT NULL DEFAULT '[]'
);
-- Every house. The components are absent on purpose: a multi's shape is a pure
-- function of its id and lives in the client's own files, so the footprint is
-- recomputed at boot rather than saved. See the sqlite schema's own note.
-- Every ship on the water. See the sqlite schema for why no component table
-- stands beside it.
CREATE TABLE IF NOT EXISTS boats (
    serial BIGINT PRIMARY KEY,
    multi  INTEGER NOT NULL,
    x      INTEGER NOT NULL,
    y      INTEGER NOT NULL,
    z      SMALLINT NOT NULL,
    facet  SMALLINT NOT NULL,
    owner  BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS houses (
    serial BIGINT PRIMARY KEY,
    multi  INTEGER NOT NULL,
    x      INTEGER NOT NULL,
    y      INTEGER NOT NULL,
    z      SMALLINT NOT NULL,
    facet  SMALLINT NOT NULL,
    owner  BIGINT NOT NULL,
    -- The three access lists, as JSON arrays of serials. See the sqlite schema.
    co_owners TEXT NOT NULL DEFAULT '[]',
    friends   TEXT NOT NULL DEFAULT '[]',
    bans      TEXT NOT NULL DEFAULT '[]',
    -- What this house will hold. See the sqlite schema's own note.
    lockdowns INTEGER NOT NULL DEFAULT 0,
    -- how long it has stood unrefreshed, in ticks. See the sqlite schema.
    age BIGINT NOT NULL DEFAULT 0
);
-- Every component of a designed house. See the sqlite schema's own note.
CREATE TABLE IF NOT EXISTS house_designs (
    house    BIGINT NOT NULL,
    revision INTEGER NOT NULL,
    graphic  INTEGER NOT NULL,
    dx       INTEGER NOT NULL,
    dy       INTEGER NOT NULL,
    dz       INTEGER NOT NULL,
    flags    BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS house_designs_house ON house_designs (house);
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
    spellbook BIGINT,
    -- a corpse's story as JSON (who it was, who killed it, who has read and
    -- rifled it). NULL for every other item.
    corpse TEXT,
    -- the poison on it: level and doses left, NULL for a clean item.
    poison_level INTEGER,
    poison_charges INTEGER,
    -- the trap on it: kind, power and the chest's level. NULL for an untrapped item.
    trap_kind INTEGER,
    trap_power INTEGER,
    trap_level INTEGER,
    -- how many uses are left in a thing that wears out: a tool's swings or an
    -- instrument's tunes. One column for both, as ServUO gives both one
    -- interface; the graphic says which it comes back as.
    uses INTEGER,
    -- what a player made: whether it came out exceptional, and whose name is on
    -- it. NULL for everything nobody crafted. The maker is a name and not a
    -- serial, because the smith logs out and the sword does not.
    exceptional BOOLEAN,
    crafter TEXT,
    -- where a recall rune points. NULL is a blank rune, which is the world's own
    -- representation too — there is no marked flag to keep in step.
    rune_facet INTEGER,
    rune_x INTEGER,
    rune_y INTEGER,
    rune_z INTEGER,
    -- a runebook's whole contents, JSON: its entries are a list, and a list of
    -- sixteen destinations does not become sixteen columns.
    runebook TEXT,
    -- the house this is locked down in, and the access level if it is a secure.
    -- See the sqlite schema.
    lockdown_house BIGINT,
    lockdown_secure SMALLINT
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
-- A facet's named areas. Keyed by (facet, id) because an id is only an index
-- into its own facet's list; the rest is a JSON record, like a decoration.
CREATE TABLE IF NOT EXISTS regions (
    facet INTEGER NOT NULL,
    id    INTEGER NOT NULL,
    data  TEXT NOT NULL,
    PRIMARY KEY (facet, id)
);
-- The world's own scalars: one row, id 0. The clock lives here so a restart does
-- not put the world back at midnight, and the roll generator's state so a restart
-- does not deal the previous run's rolls again. `rng_state` is a u64 written as the
-- BIGINT with the same bits: Postgres has no unsigned integer, so the sign is
-- reinterpreted on the way in and out, never clamped.
-- `guild_high_water` is the highest guild id ever issued, not the maximum of
-- `guilds.id`: a disbanded guild leaves no row, so re-deriving it would re-issue
-- an id every stale member record still names.
CREATE TABLE IF NOT EXISTS world (
    id            INTEGER PRIMARY KEY,
    clock_minutes BIGINT NOT NULL,
    rng_state     BIGINT NOT NULL,
    guild_high_water INTEGER NOT NULL DEFAULT 0,
    alliance_high_water INTEGER NOT NULL DEFAULT 0
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

/// The PostgreSQL variant of [`Store`](crate::Store).
///
/// One of the persistent backends; SQLite is the other, and which a shard runs
/// is the operator's choice, not a tier. The character's
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
        let (client, connection) = tokio_postgres::connect(url, NoTls).await.map_err(database)?;

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

impl PgStore {
    pub(crate) async fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError> {
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
            let skills =
                serde_json::to_string(&record.skills).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let effects =
                serde_json::to_string(&record.effects).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let quests =
                serde_json::to_string(&record.quests).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let done_quests =
                serde_json::to_string(&record.done_quests).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let stat_locks =
                serde_json::to_string(&record.stat_locks).map_err(|e| StoreError::Corrupt(e.to_string()))?;
            transaction
                .execute(
                    // The placeholder list has to match the column list exactly. It
                    // did not: three columns were added over time (fame, karma,
                    // murders, then the two quest ones) and the `VALUES` stopped at
                    // $18 while the bindings went on to twenty-one. PostgreSQL
                    // rejects that outright — "INSERT has more target columns than
                    // expressions" — so every save on a PostgreSQL shard failed at
                    // the first character. SQLite's numbered `?n` params made the
                    // same mistake impossible on the other store, which is why it
                    // went unnoticed.
                    "INSERT INTO characters \
                     (serial, account, name, body, hue, facet, x, y, z, facing, \
                      strength, dexterity, intelligence, skills, effects, dead, fame, karma, murders, \
                       quests, done_quests, stat_locks, guild, guild_title, guild_rank, guild_candidate) \
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, \
                             $17, $18, $19, $20, $21, $22, $23, $24, $25, $26) \
                     ON CONFLICT (serial) DO UPDATE SET \
                     account = EXCLUDED.account, name = EXCLUDED.name, \
                     body = EXCLUDED.body, hue = EXCLUDED.hue, facet = EXCLUDED.facet, \
                     x = EXCLUDED.x, y = EXCLUDED.y, z = EXCLUDED.z, facing = EXCLUDED.facing, \
                     strength = EXCLUDED.strength, dexterity = EXCLUDED.dexterity, \
                     intelligence = EXCLUDED.intelligence, skills = EXCLUDED.skills, \
                     effects = EXCLUDED.effects, dead = EXCLUDED.dead, \
                     fame = EXCLUDED.fame, karma = EXCLUDED.karma, murders = EXCLUDED.murders, \
                     quests = EXCLUDED.quests, done_quests = EXCLUDED.done_quests, \
                     stat_locks = EXCLUDED.stat_locks, guild = EXCLUDED.guild, \
                     guild_title = EXCLUDED.guild_title, guild_rank = EXCLUDED.guild_rank, \
                     guild_candidate = EXCLUDED.guild_candidate",
                    &[
                        &i64::from(record.serial.raw()),
                        &record.account.0,
                        &record.name.0,
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
                        &record.dead,
                        &record.fame,
                        &record.karma,
                        &i32::from(record.murders),
                        &quests,
                        &done_quests,
                        &stat_locks,
                        &record.guild.map(u32::cast_signed),
                        &record.guild_title,
                        &i32::from(record.guild_rank),
                        &record.guild_candidate.map(u32::cast_signed),
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
                let data = serde_json::to_string(mobile).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO mobiles (serial, data) VALUES ($1, $2)",
                        &[&i64::from(mobile.serial.raw()), &data],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        for inventory in &snapshot.inventories {
            transaction
                .execute(
                    "DELETE FROM items WHERE owner = $1",
                    &[&i64::from(inventory.owner.raw())],
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
                .execute("DELETE FROM characters WHERE serial = $1", &[&i64::from(*serial)])
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
                            &i64::from(spawner.id.0),
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
                let data =
                    serde_json::to_string(decoration).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO decorations (serial, data) VALUES ($1, $2)",
                        &[&i64::from(decoration.serial.raw()), &data],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        // A region sweep replaces the whole set.
        if let Some(regions) = &snapshot.regions {
            transaction
                .execute("DELETE FROM regions", &[])
                .await
                .map_err(database)?;
            for region in regions {
                let data = serde_json::to_string(region).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO regions (facet, id, data) VALUES ($1, $2, $3)",
                        &[&i32::from(region.facet), &i32::from(region.id), &data],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        // The guild sweep replaces the whole set: one disbanded since the last
        // save is absent, and the delete is what makes that stick.
        if let Some(guilds) = &snapshot.guilds {
            transaction
                .execute("DELETE FROM guilds", &[])
                .await
                .map_err(database)?;
            for guild in guilds {
                let relations = serde_json::to_string(&guild.relations)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                let proposals = serde_json::to_string(&guild.proposals)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO guilds (id, name, abbreviation, leader, relations, proposals, alliance) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7)",
                        &[
                            &guild.id.cast_signed(),
                            &guild.name,
                            &guild.abbreviation,
                            &i64::from(guild.leader.raw()),
                            &relations,
                            &proposals,
                            &guild.alliance.map(u32::cast_signed),
                        ],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        // Replace-all, like the houses and for a sharper reason: a commit
        // rewrites a house's whole component list, so a merge would leave the
        // previous design's walls standing beside the new ones.
        if let Some(designs) = &snapshot.designs {
            transaction
                .execute("DELETE FROM house_designs", &[])
                .await
                .map_err(database)?;
            for row in designs {
                transaction
                    .execute(
                        "INSERT INTO house_designs (house, revision, graphic, dx, dy, dz, flags) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7)",
                        &[
                            &i64::from(row.house.raw()),
                            &row.revision.cast_signed(),
                            &i32::from(row.graphic),
                            &i32::from(row.dx),
                            &i32::from(row.dy),
                            &i32::from(row.dz),
                            &row.flags.cast_signed(),
                        ],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        if let Some(boats) = &snapshot.boats {
            transaction
                .execute("DELETE FROM boats", &[])
                .await
                .map_err(database)?;
            for boat in boats {
                transaction
                    .execute(
                        "INSERT INTO boats (serial, multi, x, y, z, facet, owner) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7)",
                        &[
                            &i64::from(boat.serial.raw()),
                            &i32::from(boat.multi),
                            &i32::from(boat.x),
                            &i32::from(boat.y),
                            &i16::from(boat.z),
                            &i16::from(boat.facet),
                            &i64::from(boat.owner.raw()),
                        ],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        if let Some(houses) = &snapshot.houses {
            transaction
                .execute("DELETE FROM houses", &[])
                .await
                .map_err(database)?;
            for house in houses {
                transaction
                    .execute(
                        "INSERT INTO houses \
                         (serial, multi, x, y, z, facet, owner, co_owners, friends, bans, \
                          lockdowns, age) \
                         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
                        &[
                            &i64::from(house.serial.raw()),
                            &i32::from(house.multi),
                            &i32::from(house.x),
                            &i32::from(house.y),
                            &i16::from(house.z),
                            &i16::from(house.facet),
                            &i64::from(house.owner.raw()),
                            &serde_json::to_string(&house.co_owners)
                                .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                            &serde_json::to_string(&house.friends)
                                .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                            &serde_json::to_string(&house.bans)
                                .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                            &house.lockdowns.cast_signed(),
                            // Postgres BIGINT is signed; bit-cast, read back the
                            // same way.
                            &house.age.cast_signed(),
                        ],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        if let Some(alliances) = &snapshot.alliances {
            transaction
                .execute("DELETE FROM alliances", &[])
                .await
                .map_err(database)?;
            for alliance in alliances {
                let members = serde_json::to_string(&alliance.members)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                let pending = serde_json::to_string(&alliance.pending)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO alliances (id, name, leader, members, pending) \
                         VALUES ($1, $2, $3, $4, $5)",
                        &[
                            &alliance.id.cast_signed(),
                            &alliance.name,
                            &alliance.leader.cast_signed(),
                            &members,
                            &pending,
                        ],
                    )
                    .await
                    .map_err(database)?;
            }
        }
        if let Some(record) = snapshot.world {
            // `rng_state` goes in as the BIGINT with the same bits: a generator
            // state uses the whole `u64`, Postgres has no unsigned type, and the two
            // casts are exact inverses. A checked conversion would refuse half of
            // every stream's states — a save that starts failing after a few
            // hundred rolls.
            transaction
                .execute(
                    "INSERT INTO world (id, clock_minutes, rng_state, guild_high_water, alliance_high_water) \
                     VALUES (0, $1, $2, $3) \
                     ON CONFLICT (id) DO UPDATE SET clock_minutes = EXCLUDED.clock_minutes, \
                     rng_state = EXCLUDED.rng_state, \
                     guild_high_water = EXCLUDED.guild_high_water, \
                     alliance_high_water = EXCLUDED.alliance_high_water",
                    &[
                        &(record.clock_minutes as i64),
                        &record.rng_state.cast_signed(),
                        &record.guild_high_water.cast_signed(),
                        &record.alliance_high_water.cast_signed(),
                    ],
                )
                .await
                .map_err(database)?;
        }
        transaction.commit().await.map_err(database)?;
        Ok(())
    }

    pub(crate) async fn characters(&self) -> Result<Vec<CharacterRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT serial, account, name, body, hue, facet, x, y, z, facing, \
                 strength, dexterity, intelligence, skills, effects, dead, fame, karma, murders, \
                 quests, done_quests, stat_locks, guild, guild_title, guild_rank, guild_candidate \
                 FROM characters ORDER BY serial",
                &[],
            )
            .await
            .map_err(database)?;
        rows.iter().map(character_from_row).collect()
    }

    pub(crate) async fn items(&self) -> Result<Vec<ItemRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT serial, owner, graphic, hue, amount, stackable, gump, \
                 loc_kind, facet, x, y, z, parent, grid, layer, price, name, spellbook, \
                 corpse, poison_level, poison_charges, trap_kind, trap_power, trap_level, \
                 uses, exceptional, crafter, \
                 rune_facet, rune_x, rune_y, rune_z, runebook, \
                 lockdown_house, lockdown_secure FROM items",
                &[],
            )
            .await
            .map_err(database)?;
        rows.iter().filter_map(item_from_row).collect()
    }

    pub(crate) async fn mobiles(&self) -> Result<Vec<MobileRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query("SELECT data FROM mobiles", &[])
            .await
            .map_err(database)?;
        rows.iter()
            .map(|row| {
                serde_json::from_str(row.get::<_, &str>(0)).map_err(|e| StoreError::Corrupt(e.to_string()))
            })
            .collect()
    }

    pub(crate) async fn decorations(&self) -> Result<Vec<DecorationRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query("SELECT data FROM decorations", &[])
            .await
            .map_err(database)?;
        rows.iter()
            .map(|row| {
                serde_json::from_str(row.get::<_, &str>(0)).map_err(|e| StoreError::Corrupt(e.to_string()))
            })
            .collect()
    }

    pub(crate) async fn regions(&self) -> Result<Vec<RegionRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query("SELECT data FROM regions ORDER BY facet, id", &[])
            .await
            .map_err(database)?;
        rows.iter()
            .map(|row| {
                serde_json::from_str(row.get::<_, &str>(0)).map_err(|e| StoreError::Corrupt(e.to_string()))
            })
            .collect()
    }

    pub(crate) async fn guilds(&self) -> Result<Vec<GuildRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT id, name, abbreviation, leader, relations, proposals, alliance \
                 FROM guilds ORDER BY id",
                &[],
            )
            .await
            .map_err(database)?;
        rows.iter()
            .map(|row| {
                Ok(GuildRecord {
                    id: row.get::<_, i32>(0).cast_unsigned(),
                    name: row.get::<_, String>(1),
                    abbreviation: row.get::<_, String>(2),
                    leader: serial_from(row.get::<_, i64>(3))?,
                    relations: serde_json::from_str(row.get::<_, &str>(4))
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    proposals: serde_json::from_str(row.get::<_, &str>(5))
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    alliance: row.get::<_, Option<i32>>(6).map(i32::cast_unsigned),
                })
            })
            .collect()
    }

    pub(crate) async fn alliances(&self) -> Result<Vec<crate::record::AllianceRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT id, name, leader, members, pending FROM alliances ORDER BY id",
                &[],
            )
            .await
            .map_err(database)?;
        rows.iter()
            .map(|row| {
                Ok(crate::record::AllianceRecord {
                    id: row.get::<_, i32>(0).cast_unsigned(),
                    name: row.get::<_, String>(1),
                    leader: row.get::<_, i32>(2).cast_unsigned(),
                    members: serde_json::from_str(row.get::<_, &str>(3))
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    pending: serde_json::from_str(row.get::<_, &str>(4))
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                })
            })
            .collect()
    }

    pub(crate) async fn designs(&self) -> Result<Vec<crate::record::HouseDesignRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT house, revision, graphic, dx, dy, dz, flags \
                 FROM house_designs ORDER BY house",
                &[],
            )
            .await
            .map_err(database)?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                // A serial this engine did not write drops the row — a component
                // belonging to no house is one nothing could ever draw.
                let house = Serial::new(u32::try_from(row.get::<_, i64>(0)).ok()?)?;
                Some(crate::record::HouseDesignRecord {
                    house,
                    revision: row.get::<_, i32>(1).cast_unsigned(),
                    graphic: u16::try_from(row.get::<_, i32>(2)).ok()?,
                    dx: i16::try_from(row.get::<_, i32>(3)).ok()?,
                    dy: i16::try_from(row.get::<_, i32>(4)).ok()?,
                    dz: i16::try_from(row.get::<_, i32>(5)).ok()?,
                    flags: row.get::<_, i64>(6).cast_unsigned(),
                })
            })
            .collect())
    }

    pub(crate) async fn boats(&self) -> Result<Vec<crate::record::BoatRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT serial, multi, x, y, z, facet, owner FROM boats ORDER BY serial",
                &[],
            )
            .await
            .map_err(database)?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                // A row this engine did not write is a missing ship, not a shard
                // that refuses to boot — the houses reader's reasoning.
                let serial = Serial::new(u32::try_from(row.get::<_, i64>(0)).ok()?)?;
                let owner = Serial::new(u32::try_from(row.get::<_, i64>(6)).ok()?)?;
                Some(crate::record::BoatRecord {
                    serial,
                    multi: u16::try_from(row.get::<_, i32>(1)).ok()?,
                    x: u16::try_from(row.get::<_, i32>(2)).ok()?,
                    y: u16::try_from(row.get::<_, i32>(3)).ok()?,
                    z: i8::try_from(row.get::<_, i16>(4)).ok()?,
                    facet: u8::try_from(row.get::<_, i16>(5)).ok()?,
                    owner,
                })
            })
            .collect())
    }

    pub(crate) async fn houses(&self) -> Result<Vec<crate::record::HouseRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT serial, multi, x, y, z, facet, owner, co_owners, friends, bans, \
                 lockdowns, age FROM houses ORDER BY serial",
                &[],
            )
            .await
            .map_err(database)?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                // A row this engine did not write is skipped rather than refused,
                // the sqlite reader's reasoning: a corrupt house is a missing
                // house, and a shard that will not boot over one bad row is worse.
                let serial = Serial::new(row.get::<_, i32>(0).cast_unsigned())?;
                let owner = Serial::new(row.get::<_, i32>(6).cast_unsigned())?;
                Some(crate::record::HouseRecord {
                    serial,
                    multi: row.get::<_, i32>(1).cast_unsigned() as u16,
                    x: row.get::<_, i32>(2).cast_unsigned() as u16,
                    y: row.get::<_, i32>(3).cast_unsigned() as u16,
                    z: row.get::<_, i16>(4) as i8,
                    facet: row.get::<_, i16>(5) as u8,
                    owner,
                    // A list that will not parse reads as empty. A house whose
                    // friends are unreadable is a house nobody but the owner can
                    // enter, which is recoverable; refusing the whole read is a
                    // shard that will not boot.
                    co_owners: serde_json::from_str(row.get::<_, &str>(7)).unwrap_or_default(),
                    friends: serde_json::from_str(row.get::<_, &str>(8)).unwrap_or_default(),
                    bans: serde_json::from_str(row.get::<_, &str>(9)).unwrap_or_default(),
                    lockdowns: row.get::<_, i32>(10).cast_unsigned(),
                    age: row.get::<_, i64>(11).cast_unsigned(),
                })
            })
            .collect())
    }

    pub(crate) async fn world(&self) -> Result<Option<WorldRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT clock_minutes, rng_state, guild_high_water, alliance_high_water FROM world WHERE id = 0",
                &[],
            )
            .await
            .map_err(database)?;
        // No row at all is a world nobody has saved yet, which is not a row of
        // zeroes: see `Store::world`.
        Ok(rows.first().map(|row| WorldRecord {
            clock_minutes: row.get::<_, i64>(0).max(0) as u64,
            // Unsigned again, bit for bit — see the write in `save`.
            rng_state: row.get::<_, i64>(1).cast_unsigned(),
            guild_high_water: row.get::<_, i32>(2).cast_unsigned(),
            alliance_high_water: row.get::<_, i32>(3).cast_unsigned(),
        }))
    }

    pub(crate) async fn spawners(&self) -> Result<Vec<SpawnerRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query(
                "SELECT id, facet, x, y, width, height, max_count, \
                 respawn_secs, remaining_secs, creatures FROM spawners ORDER BY id",
                &[],
            )
            .await
            .map_err(database)?;
        rows.iter().map(spawner_from_row).collect()
    }

    pub(crate) async fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError> {
        let client = self.client.lock().await;
        let rows = client
            .query("SELECT name, credential FROM accounts", &[])
            .await
            .map_err(database)?;
        Ok(rows
            .iter()
            .map(|row| AccountRecord {
                name: AccountName(row.get(0)),
                credential: row.get(1),
            })
            .collect())
    }

    pub(crate) async fn put_account(&self, account: &AccountRecord) -> Result<(), StoreError> {
        let client = self.client.lock().await;
        client
            .execute(
                "INSERT INTO accounts (name, credential) VALUES ($1, $2) \
                 ON CONFLICT (name) DO UPDATE SET credential = EXCLUDED.credential",
                &[&account.name.0, &account.credential],
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
        serial: u32::try_from(row.get::<_, i64>(0))
            .ok()
            .and_then(Serial::new)
            .ok_or_else(|| corrupt("serial"))?,
        account: AccountName(row.get(1)),
        name: CharacterName(row.get(2)),
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
        dead: row.get::<_, bool>(15),
        fame: row.get::<_, i32>(16),
        karma: row.get::<_, i32>(17),
        murders: u16::try_from(row.get::<_, i32>(18)).unwrap_or(0),
        quests: serde_json::from_str(row.get::<_, &str>(19))
            .map_err(|e| StoreError::Corrupt(e.to_string()))?,
        done_quests: serde_json::from_str(row.get::<_, &str>(20))
            .map_err(|e| StoreError::Corrupt(e.to_string()))?,
        stat_locks: serde_json::from_str(row.get::<_, &str>(21))
            .map_err(|e| StoreError::Corrupt(e.to_string()))?,
        guild: row.get::<_, Option<i32>>(22).map(i32::cast_unsigned),
        guild_title: row.get::<_, String>(23),
        guild_rank: u8::try_from(row.get::<_, i32>(24)).unwrap_or(0),
        guild_candidate: row.get::<_, Option<i32>>(25).map(i32::cast_unsigned),
    })
}

/// A guild's leader serial, out of the BIGINT it was written as.
fn serial_from(raw: i64) -> Result<Serial, StoreError> {
    u32::try_from(raw)
        .ok()
        .and_then(Serial::new)
        .ok_or_else(|| corrupt("guild leader"))
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
                i64::from(container.raw()),
                i32::from(grid),
                0,
            ),
            ItemLocation::Equipped { mobile, layer } => {
                (2, 0, 0, 0, 0, i64::from(mobile.raw()), 0, i32::from(layer))
            }
        };
    transaction
        .execute(
            "INSERT INTO items \
             (serial, owner, graphic, hue, amount, stackable, gump, \
              loc_kind, facet, x, y, z, parent, grid, layer, price, name, spellbook, \
              corpse, poison_level, poison_charges, trap_kind, trap_power, trap_level, uses, \
              exceptional, crafter, rune_facet, rune_x, rune_y, rune_z, runebook, \
              lockdown_house, lockdown_secure) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21, \
                     $22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33,$34) \
             ON CONFLICT (serial) DO UPDATE SET \
             owner = EXCLUDED.owner, graphic = EXCLUDED.graphic, hue = EXCLUDED.hue, \
             amount = EXCLUDED.amount, stackable = EXCLUDED.stackable, gump = EXCLUDED.gump, \
             loc_kind = EXCLUDED.loc_kind, \
             facet = EXCLUDED.facet, x = EXCLUDED.x, y = EXCLUDED.y, z = EXCLUDED.z, \
             parent = EXCLUDED.parent, grid = EXCLUDED.grid, layer = EXCLUDED.layer, \
             price = EXCLUDED.price, name = EXCLUDED.name, spellbook = EXCLUDED.spellbook, \
             corpse = EXCLUDED.corpse, poison_level = EXCLUDED.poison_level, \
             poison_charges = EXCLUDED.poison_charges, trap_kind = EXCLUDED.trap_kind, \
             trap_power = EXCLUDED.trap_power, trap_level = EXCLUDED.trap_level, \
             uses = EXCLUDED.uses, exceptional = EXCLUDED.exceptional, \
             crafter = EXCLUDED.crafter, rune_facet = EXCLUDED.rune_facet, \
             rune_x = EXCLUDED.rune_x, rune_y = EXCLUDED.rune_y, rune_z = EXCLUDED.rune_z, \
             runebook = EXCLUDED.runebook, lockdown_house = EXCLUDED.lockdown_house, \
             lockdown_secure = EXCLUDED.lockdown_secure",
            &[
                &i64::from(item.serial.raw()),
                // `owner` is `NOT NULL BIGINT` with `0` the sentinel for "no owner" —
                // a ground item — the same convention the `DELETE FROM items WHERE
                // owner = 0` sweeps above rely on. Only the Rust-side type changed,
                // from a bare `u32` to a checked, absent `Serial`.
                &i64::from(item.owner.map_or(0, |serial| serial.raw())),
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
                // Four fields only useful together, so JSON, like the skills on a
                // character.
                &item
                    .corpse
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                &item.poison.map(|(level, _)| i32::from(level)),
                &item.poison.map(|(_, charges)| i32::from(charges)),
                &item.trap.map(|trap| i32::from(trap.kind)),
                &item.trap.map(|trap| i32::from(trap.power)),
                &item.trap.map(|trap| i32::from(trap.level)),
                &item.uses.map(i32::from),
                &item.crafted.as_ref().map(|(fine, _)| *fine),
                &item.crafted.as_ref().and_then(|(_, maker)| maker.clone()),
                &item.rune.map(|(facet, _, _, _)| i32::from(facet)),
                &item.rune.map(|(_, x, _, _)| i32::from(x)),
                &item.rune.map(|(_, _, y, _)| i32::from(y)),
                &item.rune.map(|(_, _, _, z)| i32::from(z)),
                &item
                    .runebook
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                &item.locked_down.map(|pinned| i64::from(pinned.house.raw())),
                &item.locked_down.and_then(|pinned| pinned.secure).map(i16::from),
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
        // `parent` is `0` for a `Ground` item (the column is unused there, like
        // `facet`/`x`/`y`/`z` for the other two kinds) and a real serial for the
        // other two, so it is only turned into a checked `Serial` inside the
        // branches that use it as one.
        let parent = u32::try_from(row.get::<_, i64>(12)).map_err(|_| corrupt("parent"))?;
        let grid = u8::try_from(row.get::<_, i32>(13)).map_err(|_| corrupt("grid"))?;
        let layer = u8::try_from(row.get::<_, i32>(14)).map_err(|_| corrupt("layer"))?;
        let location = match kind {
            0 => ItemLocation::Ground { facet, x, y, z },
            1 => ItemLocation::Contained {
                container: Serial::new(parent).ok_or_else(|| corrupt("parent"))?,
                x,
                y,
                grid,
            },
            2 => ItemLocation::Equipped {
                mobile: Serial::new(parent).ok_or_else(|| corrupt("parent"))?,
                layer,
            },
            _ => return Ok(None),
        };
        // `owner` is `NOT NULL BIGINT`, `0` the sentinel for "no owner" (a ground
        // item) — see the write side in `insert_item`.
        let owner_raw = u32::try_from(row.get::<_, i64>(1)).map_err(|_| corrupt("owner"))?;
        let owner = if owner_raw == 0 {
            None
        } else {
            Some(Serial::new(owner_raw).ok_or_else(|| corrupt("owner"))?)
        };
        Ok(Some(ItemRecord {
            serial: u32::try_from(row.get::<_, i64>(0))
                .ok()
                .and_then(Serial::new)
                .ok_or_else(|| corrupt("serial"))?,
            owner,
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
            corpse: row
                .get::<_, Option<String>>(18)
                .and_then(|json| serde_json::from_str(&json).ok()),
            poison: row
                .get::<_, Option<i32>>(19)
                .zip(row.get::<_, Option<i32>>(20))
                .map(|(level, charges)| {
                    (
                        u8::try_from(level).unwrap_or(0),
                        u16::try_from(charges).unwrap_or(0),
                    )
                }),
            trap: match (
                row.get::<_, Option<i32>>(21),
                row.get::<_, Option<i32>>(22),
                row.get::<_, Option<i32>>(23),
            ) {
                (Some(kind), Some(power), Some(level)) => Some(crate::record::TrapRecord {
                    kind: u8::try_from(kind).unwrap_or(0),
                    power: u16::try_from(power).unwrap_or(0),
                    level: u8::try_from(level).unwrap_or(0),
                }),
                _ => None,
            },
            uses: row
                .get::<_, Option<i32>>(24)
                .map(|uses| u16::try_from(uses).map_err(|_| corrupt("uses")))
                .transpose()?,
            crafted: row
                .get::<_, Option<bool>>(25)
                .map(|fine| (fine, row.get::<_, Option<String>>(26))),
            // All four or none: a rune half-read is a rune pointing somewhere
            // nobody marked.
            rune: match (
                row.get::<_, Option<i32>>(27),
                row.get::<_, Option<i32>>(28),
                row.get::<_, Option<i32>>(29),
                row.get::<_, Option<i32>>(30),
            ) {
                (Some(facet), Some(x), Some(y), Some(z)) => Some((
                    u8::try_from(facet).map_err(|_| corrupt("rune_facet"))?,
                    u16::try_from(x).map_err(|_| corrupt("rune_x"))?,
                    u16::try_from(y).map_err(|_| corrupt("rune_y"))?,
                    i8::try_from(z).map_err(|_| corrupt("rune_z"))?,
                )),
                _ => None,
            },
            runebook: row
                .get::<_, Option<String>>(31)
                .and_then(|json| serde_json::from_str(&json).ok()),
            // A house serial that will not parse drops the whole pin — the
            // sqlite reader's reasoning: an item claiming to be locked down in
            // nothing is one nobody could ever release.
            locked_down: row
                .get::<_, Option<i64>>(32)
                .and_then(|raw| u32::try_from(raw).ok())
                .and_then(Serial::new)
                .map(|house| crate::record::LockdownData {
                    house,
                    secure: row.get::<_, Option<i16>>(33).and_then(|n| u8::try_from(n).ok()),
                }),
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
        id: openshard_state::SpawnerId(u32::try_from(row.get::<_, i64>(0)).map_err(|_| corrupt("id"))?),
        facet: u8::try_from(row.get::<_, i32>(1)).map_err(|_| corrupt("facet"))?,
        x: u16::try_from(row.get::<_, i32>(2)).map_err(|_| corrupt("x"))?,
        y: u16::try_from(row.get::<_, i32>(3)).map_err(|_| corrupt("y"))?,
        width: u16::try_from(row.get::<_, i32>(4)).map_err(|_| corrupt("width"))?,
        height: u16::try_from(row.get::<_, i32>(5)).map_err(|_| corrupt("height"))?,
        max_count: u16::try_from(row.get::<_, i32>(6)).map_err(|_| corrupt("max_count"))?,
        respawn_secs: u64::try_from(row.get::<_, i64>(7)).map_err(|_| corrupt("respawn_secs"))?,
        remaining_secs: u64::try_from(row.get::<_, i64>(8)).map_err(|_| corrupt("remaining_secs"))?,
        creatures: serde_json::from_str(&creatures).map_err(|e| StoreError::Corrupt(e.to_string()))?,
    })
}

/// A column held a value outside the range of the record field it maps to.
fn corrupt(field: &str) -> StoreError {
    StoreError::Corrupt(format!(
        "the {field} column holds a value outside the range of its record field"
    ))
}

/// Turn a `tokio_postgres` error into the store's error. The database says what
/// went wrong; whether that is fatal is the shard's call, not this crate's.
fn database(error: tokio_postgres::Error) -> StoreError {
    StoreError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::StatLockRecord;

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
            serial: Serial::new(serial).expect("a valid test serial"),
            account: AccountName::new("admin"),
            name: CharacterName::new("Alpha"),
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
            fame: 0,
            karma: 0,
            murders: 0,
            quests: Vec::new(),
            done_quests: Vec::new(),
            guild: None,
            guild_title: String::new(),
            guild_rank: 0,
            guild_candidate: None,
            stat_locks: StatLockRecord::default(),
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
            regions: None,
            guilds: None,
            alliances: None,
            houses: None,
            designs: None,
            boats: None,
            world: None,
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
                 DROP TABLE IF EXISTS world; \
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
        assert_eq!(characters[0].serial, Serial::new(1).unwrap());
        assert_eq!(characters[0].x, 100);
    }

    #[tokio::test]
    async fn a_logout_does_not_move_a_character_down_the_list() {
        // The backend the slot-order rule was found on. A bare `SELECT` here is
        // heap order, and an `UPDATE` in PostgreSQL writes a new tuple at the end
        // of the heap — so saving one character, which is what a logout does,
        // moved it to the bottom of its own account's list on the next boot. The
        // second save below is that logout, and it is what makes this test able
        // to fail: without it the rows come back in insertion order and an
        // unordered read looks correct.
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
        for serial in [1u32, 2, 3] {
            store
                .save(&snapshot(vec![character(serial, 100)], vec![]))
                .await
                .expect("save");
        }
        store
            .save(&snapshot(vec![character(1, 200)], vec![]))
            .await
            .expect("the first character logs out");

        let serials = store
            .characters()
            .await
            .expect("read")
            .iter()
            .map(|record| record.serial.raw())
            .collect::<Vec<_>>();
        assert_eq!(serials, [1, 2, 3]);
    }

    #[tokio::test]
    async fn the_world_row_round_trips_high_bit_and_all() {
        // The SQLite twin of this test explains the trap: the generator's state uses
        // every bit of a `u64`, PostgreSQL's widest integer is a signed BIGINT, and
        // the fix is to reinterpret the sign rather than convert it. Both backends
        // get the same gate because both columns are signed for the same reason and
        // either one could be the odd one out.
        let _guard = LOCK.lock().await;
        let Some(store) = fresh().await else {
            return;
        };
        assert_eq!(store.world().await.expect("read"), None, "nothing has saved yet");

        let record = WorldRecord {
            clock_minutes: 13 * 60,
            rng_state: 0xFEDC_BA98_7654_3210,
            guild_high_water: 0,
            alliance_high_water: 0,
        };
        assert!(record.rng_state > i64::MAX.cast_unsigned(), "the high bit is set");
        store
            .save(&Snapshot {
                world: Some(record),
                ..snapshot(vec![], vec![])
            })
            .await
            .expect("save");
        assert_eq!(store.world().await.expect("read"), Some(record));

        // And the upsert replaces the row rather than adding a second one.
        let later = WorldRecord {
            clock_minutes: 14 * 60,
            rng_state: 7,
            guild_high_water: 0,
            alliance_high_water: 0,
        };
        store
            .save(&Snapshot {
                world: Some(later),
                ..snapshot(vec![], vec![])
            })
            .await
            .expect("save");
        assert_eq!(store.world().await.expect("read"), Some(later));
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
        store.save(&snapshot(vec![record], vec![])).await.expect("save");
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
            Serial::new(0x7FFF_FFFF).unwrap()
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
                name: AccountName::new("admin"),
                credential: "secret".into(),
            })
            .await
            .expect("put");
        // And an upsert on the same name updates rather than duplicating.
        store
            .put_account(&AccountRecord {
                name: AccountName::new("admin"),
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
            regions: None,
            guilds: None,
            alliances: None,
            houses: None,
            designs: None,
            boats: None,
            world: None,
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
                name: AccountName::new("admin"),
                credential: "x".into(),
            })
            .await
            .expect("put");
        drop(store);

        let url = std::env::var("OPENSHARD_POSTGRES").expect("still set");
        let reopened = PgStore::connect(&url).await.expect("reconnect");
        let characters = reopened.characters().await.expect("read");
        assert_eq!(characters.len(), 1);
        assert_eq!(characters[0].serial, Serial::new(7).unwrap());
        assert_eq!(characters[0].x, 4242, "position survived the reconnect");
        assert_eq!(reopened.accounts().await.expect("read").len(), 1);
    }

    #[tokio::test]
    async fn connecting_to_a_database_from_the_future_is_refused() {
        // Older code connecting to a newer save must refuse, not read it and write
        // the loss back on the next save.
        let _guard = LOCK.lock().await;
        let Some(url) = std::env::var("OPENSHARD_POSTGRES").ok() else {
            return;
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
        assert!(matches!(error, StoreError::SchemaMismatch { found: 999, .. }));
    }
}
