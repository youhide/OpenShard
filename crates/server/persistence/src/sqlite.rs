//! The SQLite backend.
//!
//! # Why SQLite is sync behind an async interface
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
use openshard_protocol::identity::{AccountName, CharacterName};
use openshard_protocol::serial::Serial;
#[cfg(test)]
use openshard_protocol::world::{Aggression, DamageType};
use rusqlite::{Connection, OptionalExtension, params};

use crate::journal::Snapshot;
use crate::record::{
    AccountRecord, CharacterRecord, DecorationRecord, GuildRecord, ItemLocation, ItemRecord, MobileRecord,
    RegionRecord, SCHEMA_VERSION, SpawnerRecord, StatLockRecord, WorldRecord,
};
use crate::store::{Backend, StoreError};

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
                parent: container.raw(),
                grid,
                layer: 0,
            },
            ItemLocation::Equipped { mobile, layer } => FlatLocation {
                kind: 2,
                facet: 0,
                x: 0,
                y: 0,
                z: 0,
                parent: mobile.raw(),
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
            // `f.parent` came off the same `NOT NULL` column `insert_item`/`write_item`
            // writes a real serial into for these two kinds, so a value that does not
            // parse as one is as corrupt as an unknown `kind` tag — dropped the same
            // way, via the `?` on this function's own `Option` return.
            1 => Some(ItemLocation::Contained {
                container: Serial::new(f.parent)?,
                x: f.x,
                y: f.y,
                grid: f.grid,
            }),
            2 => Some(ItemLocation::Equipped {
                mobile: Serial::new(f.parent)?,
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
    -- Standing: how widely and which way it is known, and how many innocents it
    -- has killed. The last is what makes a repeat killer permanently red, and it
    -- used to live only in memory.
    fame     INTEGER NOT NULL DEFAULT 0,
    karma    INTEGER NOT NULL DEFAULT 0,
    murders  INTEGER NOT NULL DEFAULT 0,
    -- The player's quest log — an opaque JSON blob the quest system owns. '' for none.
    quests TEXT NOT NULL DEFAULT '[]',
    done_quests TEXT NOT NULL DEFAULT '[]',
    -- Which way the three stats train, and how long since each last rose. JSON,
    -- like the skills beside it: six small numbers that are only useful together.
    stat_locks TEXT NOT NULL DEFAULT '{}',
    -- Guild membership. Columns rather than a roster table, because the question
    -- asked is which guild this character is in, and a roster is the rare
    -- direction. NULL for the unguilded, which is most of them. Deliberately no
    -- foreign key on `guilds`: an id naming a guild that is gone reads as no
    -- guild, and a constraint would turn that into a refused write instead.
    guild         INTEGER,
    guild_title   TEXT NOT NULL DEFAULT '',
    -- Where in the guild: 0 Ronin through 4 Leader. See CharacterRecord.
    guild_rank    INTEGER NOT NULL DEFAULT 0,
    guild_candidate INTEGER
);
-- Every guild on the shard. The relations and the standing offers are JSON, like
-- a spawner's creature list: a handful per guild, not a table's worth. Both are
-- written on both sides, so restoring is idempotent rather than additive.
CREATE TABLE IF NOT EXISTS guilds (
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL,
    abbreviation TEXT NOT NULL,
    leader       INTEGER NOT NULL,
    relations    TEXT NOT NULL DEFAULT '[]',
    proposals    TEXT NOT NULL DEFAULT '[]',
    -- Which alliance, by `alliances.id`. No foreign key, for the same reason a
    -- character's guild has none: an id naming an alliance that is gone reads as
    -- no alliance, and a constraint would turn that into a refused write.
    alliance     INTEGER
);
-- Every named alliance. The membership is written here rather than on the
-- guilds — one list to keep in step instead of N — and the guild's own
-- `alliance` column is the back-pointer for the lookup.
CREATE TABLE IF NOT EXISTS alliances (
    id      INTEGER PRIMARY KEY,
    name    TEXT NOT NULL,
    leader  INTEGER NOT NULL,
    members TEXT NOT NULL DEFAULT '[]',
    pending TEXT NOT NULL DEFAULT '[]'
);
-- Every house. The *components* are deliberately absent: a multi's shape is a
-- pure function of its id and lives in the client's own files, so saving it
-- would be saving a copy of a file every client already has — one that goes
-- stale the day the operator updates their install. The footprint is recomputed
-- at boot from the id and the position.
CREATE TABLE IF NOT EXISTS houses (
    serial INTEGER PRIMARY KEY,
    multi  INTEGER NOT NULL,
    x      INTEGER NOT NULL,
    y      INTEGER NOT NULL,
    z      INTEGER NOT NULL,
    facet  INTEGER NOT NULL,
    owner  INTEGER NOT NULL,
    -- The three access lists, as JSON arrays of serials. A house's own data and
    -- not a join table, for the guild relations' reason: they are read whole,
    -- every time, by the one house that owns them.
    co_owners TEXT NOT NULL DEFAULT '[]',
    friends   TEXT NOT NULL DEFAULT '[]',
    bans      TEXT NOT NULL DEFAULT '[]',
    -- What this house will hold. A number rather than a recomputation, because it
    -- is the footprint times a shard's own tuning constant: see
    -- `housing::storage`.
    lockdowns INTEGER NOT NULL DEFAULT 0,
    -- how long it has stood unrefreshed, in ticks. Elapsed rather than a
    -- deadline: the tick counter is not saved. See `housing::decay`.
    age INTEGER NOT NULL DEFAULT 0
);
-- Every component of a house whose shape nobody shipped. The one place
-- components are saved, and the table above says why they are not saved there:
-- a classic multi's shape is a pure function of its id, and a design is the
-- original with nothing to go stale against. A classic house writes no rows.
-- Every ship on the water. No component table beside it, unlike the houses: a
-- boat's shape is a pure function of its multi id with no designed case at all,
-- so it is exactly what that rule was written for. The hull-or-deck split is
-- recomputed at boot from the same multi table the mooring read.
CREATE TABLE IF NOT EXISTS boats (
    serial INTEGER PRIMARY KEY,
    multi  INTEGER NOT NULL,
    x      INTEGER NOT NULL,
    y      INTEGER NOT NULL,
    z      INTEGER NOT NULL,
    facet  INTEGER NOT NULL,
    owner  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS house_designs (
    house    INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    graphic  INTEGER NOT NULL,
    dx       INTEGER NOT NULL,
    dy       INTEGER NOT NULL,
    dz       INTEGER NOT NULL,
    flags    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS house_designs_house ON house_designs (house);
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
    spellbook INTEGER,
    -- a corpse's story as JSON (who it was, who killed it, who has read and
    -- rifled it), like the skills on a character: four fields only useful
    -- together, and only on corpses. NULL for every other item.
    corpse TEXT,
    -- the poison on it: level and doses left, NULL for a clean item. Two small
    -- numbers that are meaningless apart, so one column.
    poison_level INTEGER,
    poison_charges INTEGER,
    -- the trap on it: kind, power and the chest's level. NULL for an untrapped
    -- item, which is nearly all of them.
    trap_kind INTEGER,
    trap_power INTEGER,
    trap_level INTEGER,
    -- how many uses are left in a thing that wears out: a tool's swings or an
    -- instrument's tunes. One column for both, as ServUO gives both one
    -- interface; the graphic says which it comes back as.
    uses INTEGER,
    -- what a player made: whether it came out exceptional, and whose name is on
    -- it. NULL for everything nobody crafted, which is nearly every item on a
    -- shard. The maker is a name and not a serial, because the smith logs out and
    -- the sword does not.
    exceptional INTEGER,
    crafter TEXT,
    -- where a recall rune points. NULL is a blank rune, which is the world's own
    -- representation too — there is no marked flag to keep in step with a
    -- destination that would mean nothing when it is false.
    rune_facet INTEGER,
    rune_x INTEGER,
    rune_y INTEGER,
    rune_z INTEGER,
    -- a runebook's whole contents. JSON because its entries are a list, and a
    -- list of sixteen destinations does not become sixteen columns.
    runebook TEXT,
    -- the house this is locked down in, and the access level if it is a secure.
    -- Two columns rather than JSON: neither is a list, and the house half is the
    -- one a demolition would want to sweep by.
    lockdown_house INTEGER,
    lockdown_secure INTEGER
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
-- does not deal the previous run's rolls again. `rng_state` is a u64 written as
-- the signed word with the same bits: SQLite has one integer type and it is
-- signed, so the sign is reinterpreted on the way in and out, never clamped.
-- `guild_high_water` is the highest guild id ever issued, and is *not* the
-- maximum of `guilds.id`: a disbanded guild leaves no row, so re-deriving it
-- would re-issue an id every stale member record still names.
CREATE TABLE IF NOT EXISTS world (
    id            INTEGER PRIMARY KEY,
    clock_minutes INTEGER NOT NULL,
    rng_state     INTEGER NOT NULL,
    guild_high_water INTEGER NOT NULL DEFAULT 0,
    alliance_high_water INTEGER NOT NULL DEFAULT 0
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

/// The SQLite variant of [`Store`](crate::Store).
///
/// One of the persistent backends; PostgreSQL is the other, and which a shard
/// runs is the operator's choice, not a tier — SQLite handles a live shard
/// perfectly well. The character's [`serial`](CharacterRecord::serial) is the
/// primary key, because that is the identity that has to survive a restart.
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
impl Backend for SqliteStore {
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
        let regions = snapshot.regions.clone();
        let guilds = snapshot.guilds.clone();
        let alliances = snapshot.alliances.clone();
        let houses = snapshot.houses.clone();
        let designs = snapshot.designs.clone();
        let boats = snapshot.boats.clone();
        let world = snapshot.world;
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
                let quests = serde_json::to_string(&record.quests)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                let done_quests = serde_json::to_string(&record.done_quests)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                let stat_locks = serde_json::to_string(&record.stat_locks)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                transaction
                    .execute(
                        "INSERT OR REPLACE INTO characters \
                         (serial, account, name, body, hue, facet, x, y, z, facing, \
                          strength, dexterity, intelligence, skills, effects, dead, fame, karma, murders, \
                           quests, done_quests, stat_locks, guild, guild_title, guild_rank, guild_candidate) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
                                 ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)",
                        params![
                            record.serial.raw(),
                            record.account.0,
                            record.name.0,
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
                            record.fame,
                            record.karma,
                            record.murders,
                            quests,
                            done_quests,
                            stat_locks,
                            record.guild,
                            record.guild_title,
                            record.guild_rank,
                            record.guild_candidate,
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
                            params![mobile.serial.raw(), data],
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
                      loc_kind, facet, x, y, z, parent, grid, layer, price, name, spellbook, \
                      corpse, poison_level, poison_charges, trap_kind, trap_power, \
                      trap_level, uses, exceptional, crafter, \
                      rune_facet, rune_x, rune_y, rune_z, runebook, \
                      lockdown_house, lockdown_secure) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,\
                             ?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34)",
                        params![
                            item.serial.raw(),
                            // `owner` is `NOT NULL`, `0` the sentinel for "no owner" (a
                            // ground item) — the same convention the `DELETE FROM items
                            // WHERE owner = 0` sweep above relies on. Only the Rust-side
                            // type changed, from a bare `u32` to a checked, absent
                            // `Serial`.
                            item.owner.map_or(0, |serial| serial.raw()),
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
                            // Four fields only useful together, so JSON, like the
                            // skills on a character.
                            item.corpse
                                .as_ref()
                                .map(serde_json::to_string)
                                .transpose()
                                .unwrap_or_default(),
                            item.poison.map(|(level, _)| level),
                            item.poison.map(|(_, charges)| charges),
                            item.trap.map(|trap| trap.kind),
                            item.trap.map(|trap| trap.power),
                            item.trap.map(|trap| trap.level),
                            item.uses,
                            item.crafted.as_ref().map(|(fine, _)| *fine),
                            item.crafted
                                .as_ref()
                                .and_then(|(_, maker)| maker.as_deref()),
                            item.rune.map(|(facet, _, _, _)| facet),
                            item.rune.map(|(_, x, _, _)| x),
                            item.rune.map(|(_, _, y, _)| y),
                            item.rune.map(|(_, _, _, z)| z),
                            item.runebook
                                .as_ref()
                                .map(serde_json::to_string)
                                .transpose()
                                .unwrap_or_default(),
                            item.locked_down.map(|pinned| pinned.house.raw()),
                            item.locked_down.and_then(|pinned| pinned.secure),
                        ],
                    )?;
                    Ok(())
                };
            for inventory in &inventories {
                transaction
                    .execute(
                        "DELETE FROM items WHERE owner = ?1",
                        params![inventory.owner.raw()],
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
                            params![decoration.serial.raw(), data],
                        )
                        .map_err(database)?;
                }
            }
            // A region sweep replaces the whole set.
            if let Some(regions) = &regions {
                transaction
                    .execute("DELETE FROM regions", [])
                    .map_err(database)?;
                for region in regions {
                    let data = serde_json::to_string(region)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                    transaction
                        .execute(
                            "INSERT INTO regions (facet, id, data) VALUES (?1, ?2, ?3)",
                            params![region.facet, region.id, data],
                        )
                        .map_err(database)?;
                }
            }
            // And a guild sweep, likewise: a guild disbanded since the last save
            // is absent here, and the delete is what makes that stick.
            if let Some(guilds) = &guilds {
                transaction.execute("DELETE FROM guilds", []).map_err(database)?;
                for guild in guilds {
                    let relations = serde_json::to_string(&guild.relations)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                    let proposals = serde_json::to_string(&guild.proposals)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                    transaction
                        .execute(
                            "INSERT INTO guilds (id, name, abbreviation, leader, relations, proposals, alliance) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            params![
                                guild.id,
                                guild.name,
                                guild.abbreviation,
                                guild.leader.raw(),
                                relations,
                                proposals,
                                guild.alliance
                            ],
                        )
                        .map_err(database)?;
                }
            }
            // The alliance sweep, for the guild sweep's reason: one dissolved
            // since the last save is absent here, and the delete is what makes
            // that stick.
            if let Some(alliances) = &alliances {
                transaction.execute("DELETE FROM alliances", []).map_err(database)?;
                for alliance in alliances {
                    let members = serde_json::to_string(&alliance.members)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                    let pending = serde_json::to_string(&alliance.pending)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?;
                    transaction
                        .execute(
                            "INSERT INTO alliances (id, name, leader, members, pending) \
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![alliance.id, alliance.name, alliance.leader, members, pending],
                        )
                        .map_err(database)?;
                }
            }
            // The designs, replace-all: a commit rewrites a house's whole
            // component list, so a merge would leave the previous design's walls
            // standing beside the new ones.
            if let Some(designs) = &designs {
                transaction
                    .execute("DELETE FROM house_designs", [])
                    .map_err(database)?;
                for row in designs {
                    transaction
                        .execute(
                            "INSERT INTO house_designs (house, revision, graphic, dx, dy, dz, flags) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            params![
                                row.house.raw(),
                                row.revision,
                                row.graphic,
                                row.dx,
                                row.dy,
                                row.dz,
                                // SQLite has no unsigned 64-bit; bit-cast, and
                                // read back the same way.
                                row.flags.cast_signed(),
                            ],
                        )
                        .map_err(database)?;
                }
            }
            // The ships, replace-all like the houses: a scuttling is an absence.
            if let Some(boats) = &boats {
                transaction.execute("DELETE FROM boats", []).map_err(database)?;
                for boat in boats {
                    transaction
                        .execute(
                            "INSERT INTO boats (serial, multi, x, y, z, facet, owner) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                            params![
                                boat.serial.raw(),
                                boat.multi,
                                boat.x,
                                boat.y,
                                boat.z,
                                boat.facet,
                                boat.owner.raw(),
                            ],
                        )
                        .map_err(database)?;
                }
            }
            // And the houses, on the same terms: a demolition is an absence.
            if let Some(houses) = &houses {
                transaction.execute("DELETE FROM houses", []).map_err(database)?;
                for house in houses {
                    transaction
                        .execute(
                            "INSERT INTO houses \
                             (serial, multi, x, y, z, facet, owner, co_owners, friends, bans, \
                              lockdowns, age) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                            params![
                                house.serial.raw(),
                                house.multi,
                                house.x,
                                house.y,
                                house.z,
                                house.facet,
                                house.owner.raw(),
                                serde_json::to_string(&house.co_owners)
                                    .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                                serde_json::to_string(&house.friends)
                                    .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                                serde_json::to_string(&house.bans)
                                    .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                                house.lockdowns,
                                // SQLite has no unsigned 64-bit; bit-cast, and
                                // read back the same way.
                                house.age.cast_signed(),
                            ],
                        )
                        .map_err(database)?;
                }
            }
            if let Some(record) = world {
                // `rng_state` goes in as the signed word with the same bits. A
                // generator state uses the whole `u64`, the column is signed, and
                // the two casts are exact inverses — a `try_into` here would refuse
                // half of every stream's states, which is a save that starts
                // failing after a few hundred rolls.
                transaction
                    .execute(
                        "INSERT INTO world (id, clock_minutes, rng_state, guild_high_water, alliance_high_water) VALUES (0, ?1, ?2, ?3, ?4) \
                         ON CONFLICT(id) DO UPDATE SET clock_minutes = excluded.clock_minutes, \
                         rng_state = excluded.rng_state, \
                         guild_high_water = excluded.guild_high_water, \
                         alliance_high_water = excluded.alliance_high_water",
                        params![
                            record.clock_minutes,
                            record.rng_state.cast_signed(),
                            record.guild_high_water,
                            record.alliance_high_water
                        ],
                    )
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
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare(
                    "SELECT serial, account, name, body, hue, facet, x, y, z, facing, \
                     strength, dexterity, intelligence, skills, effects, dead, fame, karma, \
                     murders, quests, done_quests, stat_locks, guild, guild_title, guild_rank, guild_candidate \
                     FROM characters ORDER BY serial",
                )
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    let skills: String = row.get(13)?;
                    let effects: String = row.get(14)?;
                    let quests: String = row.get(19)?;
                    let done_quests: String = row.get(20)?;
                    let stat_locks: String = row.get(21)?;
                    Ok((
                        CharacterRecord {
                            serial: get_serial(row, 0)?,
                            account: AccountName(row.get(1)?),
                            name: CharacterName(row.get(2)?),
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
                            fame: row.get(16)?,
                            karma: row.get(17)?,
                            murders: row.get(18)?,
                            quests: Vec::new(),
                            done_quests: Vec::new(),
                            stat_locks: StatLockRecord::default(),
                            guild: row.get(22)?,
                            guild_title: row.get(23)?,
                            guild_rank: row.get(24)?,
                            guild_candidate: row.get(25)?,
                        },
                        skills,
                        effects,
                        quests,
                        done_quests,
                        stat_locks,
                    ))
                })
                .map_err(database)?;
            let mut characters = Vec::new();
            for row in rows {
                let (mut record, skills, effects, quests, done_quests, stat_locks) = row.map_err(database)?;
                record.skills =
                    serde_json::from_str(&skills).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                record.effects =
                    serde_json::from_str(&effects).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                record.quests =
                    serde_json::from_str(&quests).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                record.done_quests =
                    serde_json::from_str(&done_quests).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                record.stat_locks =
                    serde_json::from_str(&stat_locks).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                characters.push(record);
            }
            Ok(characters)
        })
        .await
    }

    async fn items(&self) -> Result<Vec<ItemRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare(
                    "SELECT serial, owner, graphic, hue, amount, stackable, gump, \
                     loc_kind, facet, x, y, z, parent, grid, layer, price, name, spellbook, \
                     corpse, poison_level, poison_charges, trap_kind, trap_power, trap_level, \
                     uses, exceptional, crafter, \
                     rune_facet, rune_x, rune_y, rune_z, runebook, \
                     lockdown_house, lockdown_secure FROM items",
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
                            serial: get_serial(row, 0)?,
                            owner: get_optional_serial(row, 1)?,
                            graphic: row.get(2)?,
                            hue: row.get(3)?,
                            amount: row.get(4)?,
                            stackable: row.get(5)?,
                            container_gump: row.get(6)?,
                            price: row.get(15)?,
                            name: row.get(16)?,
                            // Bit-cast back from the i64 the mask was stored as.
                            spellbook: row.get::<_, Option<i64>>(17)?.map(|mask| mask as u64),
                            corpse: row
                                .get::<_, Option<String>>(18)?
                                .and_then(|json| serde_json::from_str(&json).ok()),
                            poison: row.get::<_, Option<u8>>(19)?.zip(row.get::<_, Option<u16>>(20)?),
                            trap: match (
                                row.get::<_, Option<u8>>(21)?,
                                row.get::<_, Option<u16>>(22)?,
                                row.get::<_, Option<u8>>(23)?,
                            ) {
                                (Some(kind), Some(power), Some(level)) => {
                                    Some(crate::record::TrapRecord { kind, power, level })
                                }
                                _ => None,
                            },
                            uses: row.get(24)?,
                            crafted: row
                                .get::<_, Option<bool>>(25)?
                                .map(|fine| (fine, row.get::<_, Option<String>>(26).ok().flatten())),
                            // All four or none: a rune half-read is a rune that
                            // points somewhere nobody marked.
                            rune: match (
                                row.get::<_, Option<u8>>(27)?,
                                row.get::<_, Option<u16>>(28)?,
                                row.get::<_, Option<u16>>(29)?,
                                row.get::<_, Option<i8>>(30)?,
                            ) {
                                (Some(facet), Some(x), Some(y), Some(z)) => Some((facet, x, y, z)),
                                _ => None,
                            },
                            runebook: row
                                .get::<_, Option<String>>(31)?
                                .and_then(|json| serde_json::from_str(&json).ok()),
                            // A house serial that will not parse drops the whole
                            // pin: an item claiming to be locked down in nothing
                            // is one nobody could ever release.
                            locked_down: row.get::<_, Option<u32>>(32)?.and_then(Serial::new).map(|house| {
                                crate::record::LockdownData {
                                    house,
                                    secure: row.get::<_, Option<u8>>(33).ok().flatten(),
                                }
                            }),
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
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare(
                    "SELECT id, facet, x, y, width, height, max_count, \
                     respawn_secs, remaining_secs, creatures FROM spawners ORDER BY id",
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
                record.creatures =
                    serde_json::from_str(&creatures).map_err(|e| StoreError::Corrupt(e.to_string()))?;
                spawners.push(record);
            }
            Ok(spawners)
        })
        .await
    }

    async fn mobiles(&self) -> Result<Vec<MobileRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard.prepare("SELECT data FROM mobiles").map_err(database)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(database)?;
            let mut mobiles = Vec::new();
            for row in rows {
                let data = row.map_err(database)?;
                mobiles.push(serde_json::from_str(&data).map_err(|e| StoreError::Corrupt(e.to_string()))?);
            }
            Ok(mobiles)
        })
        .await
    }

    async fn decorations(&self) -> Result<Vec<DecorationRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard.prepare("SELECT data FROM decorations").map_err(database)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(database)?;
            let mut decorations = Vec::new();
            for row in rows {
                let data = row.map_err(database)?;
                decorations
                    .push(serde_json::from_str(&data).map_err(|e| StoreError::Corrupt(e.to_string()))?);
            }
            Ok(decorations)
        })
        .await
    }

    async fn regions(&self) -> Result<Vec<RegionRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare("SELECT data FROM regions ORDER BY facet, id")
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(database)?;
            let mut regions = Vec::new();
            for row in rows {
                let data = row.map_err(database)?;
                regions.push(serde_json::from_str(&data).map_err(|e| StoreError::Corrupt(e.to_string()))?);
            }
            Ok(regions)
        })
        .await
    }

    async fn alliances(&self) -> Result<Vec<crate::record::AllianceRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare("SELECT id, name, leader, members, pending FROM alliances ORDER BY id")
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })
                .map_err(database)?;
            let mut alliances = Vec::new();
            for row in rows {
                let (id, name, leader, members, pending) = row.map_err(database)?;
                alliances.push(crate::record::AllianceRecord {
                    id,
                    name,
                    leader,
                    members: serde_json::from_str(&members)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    pending: serde_json::from_str(&pending)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                });
            }
            Ok(alliances)
        })
        .await
    }

    async fn designs(&self) -> Result<Vec<crate::record::HouseDesignRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare(
                    "SELECT house, revision, graphic, dx, dy, dz, flags \
                     FROM house_designs ORDER BY house",
                )
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, u16>(2)?,
                        row.get::<_, i16>(3)?,
                        row.get::<_, i16>(4)?,
                        row.get::<_, i16>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                })
                .map_err(database)?;
            let mut out = Vec::new();
            for row in rows {
                let (house, revision, graphic, dx, dy, dz, flags) = row.map_err(database)?;
                // A serial this engine did not write drops the row, the houses
                // reader's reasoning: a component belonging to no house is one
                // nothing could ever draw.
                let Some(house) = Serial::new(house) else {
                    continue;
                };
                out.push(crate::record::HouseDesignRecord {
                    house,
                    revision,
                    graphic,
                    dx,
                    dy,
                    dz,
                    flags: flags.cast_unsigned(),
                });
            }
            Ok(out)
        })
        .await
    }

    async fn boats(&self) -> Result<Vec<crate::record::BoatRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare("SELECT serial, multi, x, y, z, facet, owner FROM boats ORDER BY serial")
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, u16>(1)?,
                        row.get::<_, u16>(2)?,
                        row.get::<_, u16>(3)?,
                        row.get::<_, i8>(4)?,
                        row.get::<_, u8>(5)?,
                        row.get::<_, u32>(6)?,
                    ))
                })
                .map_err(database)?;
            let mut boats = Vec::new();
            for row in rows {
                let (serial, multi, x, y, z, facet, owner) = row.map_err(database)?;
                // A row this engine did not write is a missing ship, not a shard
                // that refuses to boot — the houses reader's reasoning.
                let (Some(serial), Some(owner)) = (Serial::new(serial), Serial::new(owner)) else {
                    continue;
                };
                boats.push(crate::record::BoatRecord {
                    serial,
                    multi,
                    x,
                    y,
                    z,
                    facet,
                    owner,
                });
            }
            Ok(boats)
        })
        .await
    }

    async fn houses(&self) -> Result<Vec<crate::record::HouseRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare(
                    "SELECT serial, multi, x, y, z, facet, owner, co_owners, friends, bans, \
                     lockdowns, age FROM houses ORDER BY serial",
                )
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, u16>(1)?,
                        row.get::<_, u16>(2)?,
                        row.get::<_, u16>(3)?,
                        row.get::<_, i8>(4)?,
                        row.get::<_, u8>(5)?,
                        row.get::<_, u32>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, u32>(10)?,
                        row.get::<_, i64>(11)?,
                    ))
                })
                .map_err(database)?;
            let mut houses = Vec::new();
            for row in rows {
                let (serial, multi, x, y, z, facet, owner, co_owners, friends, bans, lockdowns, age) =
                    row.map_err(database)?;
                // A row whose serial or owner will not parse is one this engine
                // did not write. Skipped rather than refused: a corrupt house is
                // a missing house, and refusing the read would be a shard that
                // will not boot over one bad row.
                let (Some(serial), Some(owner)) = (Serial::new(serial), Serial::new(owner)) else {
                    continue;
                };
                houses.push(crate::record::HouseRecord {
                    serial,
                    multi,
                    x,
                    y,
                    z,
                    facet,
                    owner,
                    co_owners: serde_json::from_str(&co_owners)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    friends: serde_json::from_str(&friends)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    bans: serde_json::from_str(&bans).map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    lockdowns,
                    age: age.cast_unsigned(),
                });
            }
            Ok(houses)
        })
        .await
    }

    async fn guilds(&self) -> Result<Vec<GuildRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare(
                    "SELECT id, name, abbreviation, leader, relations, proposals, alliance \
                     FROM guilds ORDER BY id",
                )
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        get_serial(row, 3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<u32>>(6)?,
                    ))
                })
                .map_err(database)?;
            let mut guilds = Vec::new();
            for row in rows {
                let (id, name, abbreviation, leader, relations, proposals, alliance) =
                    row.map_err(database)?;
                guilds.push(GuildRecord {
                    id,
                    name,
                    abbreviation,
                    leader,
                    relations: serde_json::from_str(&relations)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    proposals: serde_json::from_str(&proposals)
                        .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                    alliance,
                });
            }
            Ok(guilds)
        })
        .await
    }

    async fn world(&self) -> Result<Option<WorldRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let row: Option<(i64, i64, u32, u32)> = guard
                .query_row(
                    "SELECT clock_minutes, rng_state, guild_high_water, alliance_high_water FROM world WHERE id = 0",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, u32>(2)?,
                            row.get::<_, u32>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(database)?;
            // No row at all is a world nobody has saved yet, which is not a row of
            // zeroes: see `Store::world`.
            Ok(
                row.map(|(clock_minutes, rng_state, guild_high_water, alliance_high_water)| WorldRecord {
                    clock_minutes: clock_minutes.max(0) as u64,
                    // Unsigned again, bit for bit — see the write in `save`.
                    rng_state: rng_state.cast_unsigned(),
                    guild_high_water,
                    alliance_high_water,
                }),
            )
        })
        .await
    }

    async fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError> {
        let connection = Arc::clone(&self.connection);
        blocking(move || {
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            let mut statement = guard
                .prepare("SELECT name, credential FROM accounts")
                .map_err(database)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(AccountRecord {
                        name: AccountName(row.get(0)?),
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
            let guard = connection.lock().expect("the sqlite mutex is never poisoned");
            guard
                .execute(
                    "INSERT OR REPLACE INTO accounts (name, credential) VALUES (?1, ?2)",
                    params![account.name.0, account.credential],
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

/// Read column `idx` as a checked [`Serial`].
///
/// `Serial` has no `FromSql` impl of its own — a serial is a `u32` on disk, and
/// the checked conversion happens here rather than by giving the newtype a wire
/// format it does not otherwise need. A value that does not fit fails the same
/// way `rusqlite`'s own narrowing conversions do (`z` as `i8`, `body` as `u16`):
/// [`rusqlite::Error::IntegralValueOutOfRange`], routed through [`database`] like
/// every other read failure in this file.
fn get_serial(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Serial> {
    let raw: u32 = row.get(idx)?;
    Serial::new(raw).ok_or(rusqlite::Error::IntegralValueOutOfRange(idx, i64::from(raw)))
}

/// Read column `idx` as a checked `Option<Serial>`, where `0` is the on-disk
/// sentinel for "none" — the same convention [`ItemRecord::owner`] writes on
/// the way in (see the `save` method) and the `owner = 0` sweep for ground
/// items relies on.
fn get_optional_serial(row: &rusqlite::Row<'_>, idx: usize) -> rusqlite::Result<Option<Serial>> {
    let raw: u32 = row.get(idx)?;
    if raw == 0 {
        Ok(None)
    } else {
        Serial::new(raw)
            .map(Some)
            .ok_or(rusqlite::Error::IntegralValueOutOfRange(idx, i64::from(raw)))
    }
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
    use openshard_protocol::world::Sight;

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

    fn contained(serial: u32, owner: u32, container: u32) -> ItemRecord {
        ItemRecord {
            serial: Serial::new(serial).expect("a valid test serial"),
            // `0` is the on-disk sentinel for "no owner" — a ground item — the
            // same convention `get_optional_serial` reads back.
            owner: if owner == 0 {
                None
            } else {
                Some(Serial::new(owner).expect("a valid test serial"))
            },
            graphic: 0x0EED,
            hue: 0,
            amount: 1,
            stackable: false,
            container_gump: None,
            price: None,
            name: None,
            spellbook: None,
            corpse: None,
            poison: None,
            trap: None,
            uses: None,
            crafted: None,
            rune: None,
            runebook: None,
            locked_down: None,
            location: ItemLocation::Contained {
                container: Serial::new(container).expect("a valid test serial"),
                x: 0,
                y: 0,
                grid: 0,
            },
        }
    }

    /// The same item lying on the ground rather than in a pack — the *other*
    /// restore path, which no test that only fills a backpack exercises.
    ///
    /// Calls `contained` with a placeholder (nonzero, since `0` is not a valid
    /// `Serial`) owner and container — both immediately overwritten below, the
    /// owner by this function's caller's ground sweep semantics and the location
    /// by the struct-update — so the placeholder's value never surfaces.
    fn ground(serial: u32, x: u16, y: u16) -> ItemRecord {
        ItemRecord {
            location: ItemLocation::Ground { facet: 0, x, y, z: 0 },
            ..contained(serial, 0, 1)
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
        assert_eq!(characters[0].serial, Serial::new(1).unwrap());
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
    async fn characters_come_back_in_slot_order() {
        // Green before the `ORDER BY` that this pins, and said so rather than
        // presented as evidence: `serial INTEGER PRIMARY KEY` is the rowid alias,
        // so a bare select here was already ascending and this backend is the
        // reason the rule looked held. What it protects is the schema — a serial
        // that stops being the rowid, or a plan that scans an index instead —
        // where the drift would otherwise be silent and only on somebody's shard.
        let store = SqliteStore::open_in_memory().expect("open");
        for serial in [3u32, 1, 2] {
            store
                .save(&snapshot(vec![character(serial, 100)], vec![]))
                .await
                .expect("save");
        }
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
        store.save(&snapshot(vec![record], vec![])).await.expect("save");
        assert_eq!(store.characters().await.expect("read")[0].z, -40);
    }

    #[tokio::test]
    async fn the_world_row_is_absent_until_a_snapshot_carries_it() {
        // Absent, not a row of zeroes. A caller that got `WorldRecord::default()`
        // out of an untouched store would seed the world with zero and quietly
        // discard whatever `world.seed` asked for.
        let store = SqliteStore::open_in_memory().expect("open");
        assert_eq!(store.world().await.expect("read"), None);
        store
            .save(&snapshot(vec![character(1, 100)], vec![]))
            .await
            .expect("save");
        assert_eq!(
            store.world().await.expect("read"),
            None,
            "a snapshot that did not sweep the world's scalars must not invent them"
        );
    }

    #[tokio::test]
    async fn a_high_bit_rng_state_survives_the_database() {
        // The generator's state is a u64 and SQLite has one integer type, signed.
        // Half of every stream's states have the high bit set, so the mistake here
        // — a checked conversion, or a `max(0)` like the clock's — is a shard whose
        // saves start failing, or whose roll stream silently resets, after a few
        // hundred rolls.
        let store = SqliteStore::open_in_memory().expect("open");
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
    }

    #[tokio::test]
    async fn a_guild_and_its_members_survive_a_reopen() {
        use crate::record::{GuildRecord, GuildStanding};

        // A guild sweep replaces the set, so a guild disbanded since the last save
        // is absent from the next one — and the delete is what makes that stick.
        // The membership does not ride here at all: it is a character column, and
        // the two are written on their own schedules.
        let path = temp_db("guilds");
        let guild = |id: u32, name: &str, at_war_with: Option<u32>| GuildRecord {
            id,
            name: name.to_owned(),
            abbreviation: name[..3].to_owned(),
            leader: Serial::new(0x0000_0001).expect("a leader serial"),
            relations: at_war_with
                .map(|other| vec![GuildStanding { other, at_war: true }])
                .unwrap_or_default(),
            proposals: vec![GuildStanding {
                other: 99,
                at_war: false,
            }],
            alliance: None,
        };

        let mut member = character(1, 100);
        member.guild = Some(1);
        member.guild_title = "Warlord".to_owned();
        member.guild_candidate = Some(2);

        {
            let store = SqliteStore::open(&path).expect("open");
            store
                .save(&Snapshot {
                    guilds: Some(vec![
                        guild(1, "Silverfoot", Some(2)),
                        guild(2, "Blackrose", Some(1)),
                    ]),
                    world: Some(WorldRecord {
                        clock_minutes: 0,
                        rng_state: 0,
                        // Higher than any guild in the table, which is the whole
                        // reason it is saved rather than derived: guild 3 was
                        // founded and disbanded, and leaves no row.
                        guild_high_water: 3,
                        alliance_high_water: 0,
                    }),
                    ..snapshot(vec![member.clone()], vec![])
                })
                .await
                .expect("save");
        }

        let store = SqliteStore::open(&path).expect("reopen");
        let guilds = store.guilds().await.expect("read");
        assert_eq!(guilds.len(), 2);
        assert_eq!(guilds[0].name, "Silverfoot");
        assert_eq!(guilds[0].abbreviation, "Sil");
        assert_eq!(
            guilds[0].relations,
            vec![GuildStanding {
                other: 2,
                at_war: true
            }],
            "the war did not survive the door"
        );
        assert_eq!(
            guilds[0].proposals,
            vec![GuildStanding {
                other: 99,
                at_war: false
            }],
            "a standing offer is half a war, and losing it undoes one"
        );
        assert_eq!(
            store.world().await.expect("read").map(|w| w.guild_high_water),
            Some(3),
            "the id counter is not the maximum id in the table"
        );

        let characters = store.characters().await.expect("read");
        let saved = characters.first().expect("the member");
        assert_eq!(saved.guild, Some(1));
        assert_eq!(saved.guild_title, "Warlord");
        assert_eq!(
            saved.guild_candidate,
            Some(2),
            "an invitation left for an offline player is the one that must survive"
        );
    }

    /// The membership is written on the alliance, not spread across the guilds.
    ///
    /// A sweep, for the guild sweep's reason: an alliance dissolved since the
    /// last save is absent from the next one, and the delete is what makes that
    /// stick. The pending guild rides in its own column because a save of "the
    /// members" would silently drop every standing invitation.
    #[tokio::test]
    async fn an_alliance_and_its_membership_survive_a_reopen() {
        use crate::record::AllianceRecord;

        let path = temp_db("alliances");
        {
            let store = SqliteStore::open(&path).expect("open");
            store
                .save(&Snapshot {
                    alliances: Some(vec![
                        AllianceRecord {
                            id: 1,
                            name: "The Northern Compact".to_owned(),
                            leader: 1,
                            members: vec![1, 2],
                            pending: vec![3],
                        },
                        AllianceRecord {
                            id: 2,
                            name: "The Ash Pact".to_owned(),
                            leader: 4,
                            members: vec![4, 5],
                            pending: vec![],
                        },
                    ]),
                    world: Some(WorldRecord {
                        clock_minutes: 0,
                        rng_state: 0,
                        guild_high_water: 0,
                        // Higher than either row, which is the whole reason it is
                        // saved rather than derived from the table.
                        alliance_high_water: 7,
                    }),
                    ..snapshot(vec![], vec![])
                })
                .await
                .expect("save");
            // And the second save is a sweep: the Ash Pact disbanded.
            store
                .save(&Snapshot {
                    tick: 2,
                    alliances: Some(vec![AllianceRecord {
                        id: 1,
                        name: "The Northern Compact".to_owned(),
                        leader: 2,
                        members: vec![1, 2],
                        pending: vec![3],
                    }]),
                    ..snapshot(vec![], vec![])
                })
                .await
                .expect("save");
        }

        let store = SqliteStore::open(&path).expect("reopen");
        let alliances = store.alliances().await.expect("read");
        assert_eq!(
            alliances.len(),
            1,
            "the disbanded one is absent, which is the delete"
        );
        assert_eq!(alliances[0].name, "The Northern Compact");
        assert_eq!(
            alliances[0].leader, 2,
            "the leader the second save named was not written"
        );
        assert_eq!(alliances[0].members, vec![1, 2]);
        assert_eq!(
            alliances[0].pending,
            vec![3],
            "a standing question is not a membership, and losing it drops it"
        );
        assert_eq!(
            store.world().await.expect("read").map(|w| w.alliance_high_water),
            Some(7),
            "the id counter is not the maximum id in the table"
        );
    }

    #[tokio::test]
    async fn accounts_round_trip() {
        let store = SqliteStore::open_in_memory().expect("open");
        store
            .put_account(&AccountRecord {
                name: AccountName::new("admin"),
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
    async fn mobiles_and_decorations_replace_and_reopen() {
        // The two whole-world tables: a sweep replaces the set, a dead mobile's
        // items go with it, and everything survives a reopen from the file.
        use crate::record::{DecorationRecord, DoorState, Inventory, MobileRecord};
        fn mobile(serial: u32, hits: u16) -> MobileRecord {
            MobileRecord {
                serial: Serial::new(serial).expect("a valid test serial"),
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
                notoriety: openshard_protocol::mobile::Notoriety::Neutral,
                damage: 3,
                resistance: openshard_protocol::world::PhysicalResistance::new(0),
                swing: 0,
                sight: Sight(8),
                aggression: Aggression::from_bits(0),
                beat: 0,
                ranged: None,
                ranged_kind: DamageType::Physical,
                wander: true,
                banker: false,
                vendor: true,
                healer: false,
                title: Some("the vendor".into()),
                npc_home: Some((1400, 1600, 0)),
                npc_wander: 2,
                night_home: None,
                pet: None,
                restock: None,
                spawned_by: None,
                effects: Vec::new(),
                skills: Vec::new(),
                quest_giver: Vec::new(),
                escort_destination: None,
            }
        }
        let decoration = DecorationRecord {
            key_value: 0,
            serial: Serial::new(0x4000_0100).unwrap(),
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
                owner: Serial::new(2).unwrap(),
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
            assert_eq!(mobiles[0].serial, Serial::new(3).unwrap());
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
                owner: Serial::new(1).unwrap(),
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
                owner: Serial::new(1).unwrap(),
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
    async fn a_marked_rune_and_a_filled_runebook_survive_a_reopen() {
        // A rune is nothing *but* where it points, so an unsaved one comes back a
        // blank and the walk that marked it was for nothing. Both are checked on
        // the ground and in a container, because those are two different restore
        // paths in the world and only one of them being right is the failure that
        // shows up a week later as "my bank runes work and my pack runes don't".
        let path = temp_db("rune-and-book");
        let book = crate::record::RunebookData {
            entries: vec![
                crate::record::RunebookEntryData {
                    facet: 0,
                    x: 1336,
                    y: 1997,
                    z: 5,
                    description: "Britain".into(),
                },
                crate::record::RunebookEntryData {
                    facet: 1,
                    x: 2701,
                    y: 692,
                    z: 5,
                    description: "Minoc".into(),
                },
            ],
            charges: 3,
            max_charges: 10,
            default_entry: Some(1),
        };
        {
            let store = SqliteStore::open(&path).expect("open");
            let mut carried = contained(0x4000_0001, 1, 1);
            carried.rune = Some((0, 1495, 1629, -20));
            let mut dropped = ground(0x4000_0002, 100, 100);
            dropped.runebook = Some(book.clone());
            let mut snap = snapshot(vec![character(1, 100)], vec![]);
            snap.inventories = vec![crate::record::Inventory {
                owner: Serial::new(1).unwrap(),
                items: vec![carried],
            }];
            snap.ground = Some(vec![dropped]);
            store.save(&snap).await.expect("save");
        }
        {
            let store = SqliteStore::open(&path).expect("reopen");
            let items = store.items().await.expect("read");
            let carried = items
                .iter()
                .find(|item| item.serial == Serial::new(0x4000_0001).unwrap())
                .expect("the carried rune");
            assert_eq!(
                carried.rune,
                Some((0, 1495, 1629, -20)),
                "a negative z is a dungeon floor, and has to survive signed"
            );
            let dropped = items
                .iter()
                .find(|item| item.serial == Serial::new(0x4000_0002).unwrap())
                .expect("the dropped book");
            assert_eq!(dropped.runebook, Some(book));
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn an_unmarked_rune_stays_unmarked() {
        // The absence is the answer: no destination means a blank rune, and a
        // column read as a zeroed tuple would silently point every blank rune in
        // the world at the top-left corner of Felucca.
        let path = temp_db("blank-rune");
        {
            let store = SqliteStore::open(&path).expect("open");
            let mut snap = snapshot(vec![character(1, 100)], vec![]);
            snap.inventories = vec![crate::record::Inventory {
                owner: Serial::new(1).unwrap(),
                items: vec![contained(0x4000_0001, 1, 1)],
            }];
            store.save(&snap).await.expect("save");
        }
        {
            let store = SqliteStore::open(&path).expect("reopen");
            let items = store.items().await.expect("read");
            assert_eq!(items[0].rune, None);
            assert_eq!(items[0].runebook, None);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn a_corpses_story_survives_a_reopen() {
        // A corpse lies for seven minutes; a shard can restart inside that. Who it
        // was, who killed it, who has read it and who has rifled it all have to
        // come back, or the investigation a player was halfway through resets to a
        // body nobody has touched.
        let path = temp_db("corpse-story");
        let story = crate::record::CorpseData {
            owner: "a lich".into(),
            killer: Some("Rowena".into()),
            examined_by: Some("Mordred".into()),
            looters: vec!["Vesper".into(), "Rowena".into()],
            // And which way it lies: the picture's other half, saved here
            // because the item row's `amount` already carries the body.
            facing: 6,
        };
        {
            let store = SqliteStore::open(&path).expect("open");
            let mut item = contained(0x4000_0001, 1, 1);
            item.corpse = Some(story.clone());
            let mut snap = snapshot(vec![character(1, 100)], vec![]);
            snap.inventories = vec![crate::record::Inventory {
                owner: Serial::new(1).unwrap(),
                items: vec![item],
            }];
            store.save(&snap).await.expect("save");
        }
        {
            let store = SqliteStore::open(&path).expect("reopen");
            let items = store.items().await.expect("read");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].corpse.as_ref(), Some(&story));
        }
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn the_poison_on_an_item_survives_a_reopen() {
        // All four poison potions are the same graphic, so an unsaved bottle comes
        // back as an empty one and a blade a player spent a potion coating comes
        // back clean — the `spellbook` lesson, one schema bump later.
        let path = temp_db("item-poison");
        {
            let store = SqliteStore::open(&path).expect("open");
            let mut item = contained(0x4000_0001, 1, 1);
            item.poison = Some((3, 12));
            let mut snap = snapshot(vec![character(1, 100)], vec![]);
            snap.inventories = vec![crate::record::Inventory {
                owner: Serial::new(1).unwrap(),
                items: vec![item],
            }];
            store.save(&snap).await.expect("save");
        }
        {
            let store = SqliteStore::open(&path).expect("reopen");
            let items = store.items().await.expect("read");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].poison, Some((3, 12)));
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
                    name: AccountName::new("admin"),
                    credential: "x".into(),
                })
                .await
                .expect("put");
        }
        {
            let store = SqliteStore::open(&path).expect("reopen");
            let characters = store.characters().await.expect("read");
            assert_eq!(characters.len(), 1);
            assert_eq!(characters[0].serial, Serial::new(7).unwrap());
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
        assert!(matches!(error, StoreError::SchemaMismatch { found: 999, .. }));
        let _ = std::fs::remove_file(&path);
    }
}
