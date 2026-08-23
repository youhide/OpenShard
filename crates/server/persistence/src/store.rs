//! The database, as a concrete choice.
//!
//! A shard has one of three known stores: memory, SQLite, or PostgreSQL.  That
//! choice is represented by [`Store`], rather than erased behind a trait object:
//! callers can see the closed set of backends they are choosing between.
//!
//! # Errors are for the caller to decide about
//!
//! A store says what went wrong. It does not decide whether that is fatal —
//! that is the shard's call, and the answer is usually "log it, put the
//! entities back, try again next save". A store that panicked on a full disk
//! would take the world down over something that fixes itself.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use openshard_protocol::identity::AccountName;
use openshard_protocol::serial::Serial;
#[cfg(test)]
use openshard_protocol::world::{Aggression, DamageType};

use crate::journal::Snapshot;
use crate::pg::PgStore;
use crate::record::{
    AccountRecord, CharacterRecord, DecorationRecord, GuildRecord, ItemRecord, MobileRecord, RegionRecord,
    SCHEMA_VERSION, SpawnerRecord, WorldRecord,
};
use crate::sqlite::SqliteStore;

/// What a store could not do.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The save is from a version this build does not understand.
    ///
    /// Refused rather than read: opening a newer save with older code means
    /// silently dropping every field it does not recognise, and then writing
    /// the loss back on the next save. A shard that will not start is a bad
    /// morning. A shard that quietly deletes a column is a bad year.
    #[error("save is schema v{found}, this build understands v{understood}")]
    SchemaMismatch {
        /// What the data claims to be.
        found: u32,
        /// What this build can read.
        understood: u32,
    },
    /// The database said no.
    #[error("database: {0}")]
    Database(String),
    /// The data on disk is not what it claims to be.
    #[error("corrupt: {0}")]
    Corrupt(String),
}

/// Operations every built-in persistence backend supports.
///
/// This is an implementation detail: the shard deals in the concrete [`Store`]
/// enum, while the three backend implementations share these operations here.
#[async_trait]
pub(crate) trait Backend: Send + Sync {
    /// Write a snapshot.
    ///
    /// # Must be atomic
    ///
    /// All of it or none of it. The snapshot is a consistent picture of one
    /// tick, and half of it is a world that never existed — see the `journal`
    /// module. A backend that cannot do a transaction is not a
    /// backend that can implement this.
    async fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError>;

    /// Every character, **in ascending serial order**.
    ///
    /// The order is part of the contract here and nowhere else in this trait,
    /// because this is the only read whose order a player sees. The caller enrols
    /// them in the order given, that list is what `0xA9` draws, and `0x83` picks a
    /// character by its position in it — so the sequence this returns is the slot
    /// order, and a boot that returns it differently has shuffled somebody's
    /// character screen.
    ///
    /// Serial ascending is creation order: the allocator hands them out upwards
    /// and a restored character keeps the one it was created with. It is also the
    /// only key every backend has, which is the other half of why it is the rule —
    /// the three implementations below disagreed by default. SQLite's `serial` is
    /// `INTEGER PRIMARY KEY`, so a bare select is already rowid order and looked
    /// stable; Postgres returns heap order, where an `UPDATE` moves a row to the
    /// end, so one logout reordered the list on the next boot; and
    /// [`MemoryStore`] returned `HashMap` iteration order, which is a fresh
    /// shuffle every process. All three now say `ORDER BY serial` or its
    /// equivalent.
    async fn characters(&self) -> Result<Vec<CharacterRecord>, StoreError>;

    /// Every saved item: characters' carried inventories and loose ground clutter.
    /// The caller reserves their serials, restores ground items now, and hands each
    /// character its own when it logs in.
    async fn items(&self) -> Result<Vec<ItemRecord>, StoreError>;

    /// Every saved spawn region, with the respawn timer it was saved with. The
    /// caller re-creates them at boot so populated areas stay populated across a
    /// restart, and a rare spawn keeps its remaining wait.
    async fn spawners(&self) -> Result<Vec<SpawnerRecord>, StoreError>;

    /// Every saved NPC mobile — townsfolk, vendors, creatures. The caller
    /// re-creates them at boot exactly as they stood, the Sphere/ServUO whole-world
    /// model: a killed creature is simply not in the save, and stays gone.
    async fn mobiles(&self) -> Result<Vec<MobileRecord>, StoreError>;

    /// Every saved decoration — the placed statics, doors and town containers.
    /// The caller re-lays them at boot, door state and all.
    async fn decorations(&self) -> Result<Vec<DecorationRecord>, StoreError>;

    /// Every facet's named regions, for the boot load.
    ///
    /// # Errors
    /// If the store cannot be read.
    async fn regions(&self) -> Result<Vec<RegionRecord>, StoreError>;

    /// Every guild on the shard, for the boot load.
    ///
    /// Only the guilds. Who is *in* one comes back with the characters, because
    /// membership is a character's field — so a roster is the sum of who names
    /// the guild, and there is no second list to fall out of step with it.
    ///
    /// # Errors
    /// If the store cannot be read.
    async fn guilds(&self) -> Result<Vec<GuildRecord>, StoreError>;

    /// Every named alliance, in id order.
    async fn alliances(&self) -> Result<Vec<crate::record::AllianceRecord>, StoreError>;

    /// Every house on the shard, in serial order.
    ///
    /// # Errors
    /// If the store cannot be read.
    async fn houses(&self) -> Result<Vec<crate::record::HouseRecord>, StoreError>;

    /// Every designed house's components, in serial order.
    ///
    /// A second read rather than a join into [`houses`](Self::houses), and that
    /// is the cost `HouseDesignRecord` names: a design is a few hundred rows and
    /// a house record is one, so they do not travel together. On the
    /// overwhelmingly common shard this answers an empty vector.
    ///
    /// # Errors
    /// If the store cannot be read.
    async fn designs(&self) -> Result<Vec<crate::record::HouseDesignRecord>, StoreError>;

    /// Every ship on the water, as last saved. The boat index is rebuilt from
    /// these at boot, the way a house's footprint is.
    ///
    /// # Errors
    /// If the store cannot be read.
    async fn boats(&self) -> Result<Vec<crate::record::BoatRecord>, StoreError>;

    /// The world's own scalars, as last saved — the clock and where the roll
    /// generator got to. One read for the one row, so a scalar added to
    /// [`WorldRecord`] does not add a method here and a query to every backend.
    ///
    /// `None` means this store has never held the row: a world nobody has saved
    /// yet. That is not the same as a row of zeroes, and the difference matters for
    /// the generator — zero is a seed like any other, so a caller handed
    /// `WorldRecord::default()` would overwrite whatever the config asked for with
    /// it. Absence leaves the fresh world's own seed alone.
    ///
    /// # Errors
    /// If the store cannot be read.
    async fn world(&self) -> Result<Option<WorldRecord>, StoreError>;

    /// Every account.
    async fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError>;

    /// Add or update an account.
    async fn put_account(&self, account: &AccountRecord) -> Result<(), StoreError>;
}

/// Where the world is kept.
///
/// The set is deliberately closed. Adding a persistence backend is an explicit
/// change to this enum, rather than an implicit new implementation hidden from
/// the shard behind dynamic dispatch.
#[derive(Debug)]
pub enum Store {
    Memory(MemoryStore),
    Sqlite(SqliteStore),
    Postgres(PgStore),
}

macro_rules! delegate {
    ($store:expr, $method:ident($($argument:expr),* $(,)?)) => {
        match $store {
            Self::Memory(store) => store.$method($($argument),*).await,
            Self::Sqlite(store) => store.$method($($argument),*).await,
            Self::Postgres(store) => store.$method($($argument),*).await,
        }
    };
}

impl Store {
    pub fn memory() -> Self {
        Self::Memory(MemoryStore::new())
    }

    pub fn sqlite(store: SqliteStore) -> Self {
        Self::Sqlite(store)
    }

    pub fn postgres(store: PgStore) -> Self {
        Self::Postgres(store)
    }

    pub async fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError> {
        delegate!(self, save(snapshot))
    }

    pub async fn characters(&self) -> Result<Vec<CharacterRecord>, StoreError> {
        delegate!(self, characters())
    }

    pub async fn items(&self) -> Result<Vec<ItemRecord>, StoreError> {
        delegate!(self, items())
    }

    pub async fn spawners(&self) -> Result<Vec<SpawnerRecord>, StoreError> {
        delegate!(self, spawners())
    }

    pub async fn mobiles(&self) -> Result<Vec<MobileRecord>, StoreError> {
        delegate!(self, mobiles())
    }

    pub async fn decorations(&self) -> Result<Vec<DecorationRecord>, StoreError> {
        delegate!(self, decorations())
    }

    pub async fn regions(&self) -> Result<Vec<RegionRecord>, StoreError> {
        delegate!(self, regions())
    }

    pub async fn guilds(&self) -> Result<Vec<GuildRecord>, StoreError> {
        delegate!(self, guilds())
    }

    pub async fn alliances(&self) -> Result<Vec<crate::record::AllianceRecord>, StoreError> {
        delegate!(self, alliances())
    }

    pub async fn houses(&self) -> Result<Vec<crate::record::HouseRecord>, StoreError> {
        delegate!(self, houses())
    }

    pub async fn designs(&self) -> Result<Vec<crate::record::HouseDesignRecord>, StoreError> {
        delegate!(self, designs())
    }

    pub async fn boats(&self) -> Result<Vec<crate::record::BoatRecord>, StoreError> {
        delegate!(self, boats())
    }

    pub async fn world(&self) -> Result<Option<WorldRecord>, StoreError> {
        delegate!(self, world())
    }

    pub async fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError> {
        delegate!(self, accounts())
    }

    pub async fn put_account(&self, account: &AccountRecord) -> Result<(), StoreError> {
        delegate!(self, put_account(account))
    }
}

/// A store that keeps everything in memory and never fails.
///
/// For tests, and for a shard started with no database at all — which is a real
/// mode, not a broken one: the shard already runs without a map, and running
/// without persistence is the same bargain. Nothing is saved and nothing is
/// pretended to be.
#[derive(Debug, Default)]
pub struct MemoryStore {
    /// Keyed by serial, which is the identity that outlives a restart.
    characters: Mutex<HashMap<Serial, CharacterRecord>>,
    /// Items keyed by serial: inventory (owner is a character) and ground
    /// (`owner` is `None`).
    items: Mutex<HashMap<Serial, ItemRecord>>,
    /// Spawn regions keyed by id.
    spawners: Mutex<HashMap<u32, SpawnerRecord>>,
    /// NPC mobiles keyed by serial.
    mobiles: Mutex<HashMap<Serial, MobileRecord>>,
    /// Placed decorations keyed by serial.
    decorations: Mutex<HashMap<Serial, DecorationRecord>>,
    /// Named regions, keyed by `(facet, id)`.
    regions: Mutex<HashMap<(u8, u16), RegionRecord>>,
    /// Guilds, keyed by id.
    guilds: Mutex<HashMap<u32, GuildRecord>>,
    alliances: Mutex<HashMap<u32, crate::record::AllianceRecord>>,
    houses: Mutex<HashMap<u32, crate::record::HouseRecord>>,
    designs: Mutex<Vec<crate::record::HouseDesignRecord>>,
    boats: Mutex<HashMap<u32, crate::record::BoatRecord>>,
    /// The world's own scalars: the clock, and where the rolls got to. `None` until
    /// a snapshot carries them.
    world: Mutex<Option<WorldRecord>>,
    accounts: Mutex<HashMap<AccountName, AccountRecord>>,
    /// How many saves have landed. What a test asserts on.
    saves: Mutex<u64>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many saves have landed.
    pub fn save_count(&self) -> u64 {
        *self.saves.lock().expect("the mutex is never poisoned")
    }

    /// How many characters it holds.
    pub fn character_count(&self) -> usize {
        self.characters.lock().expect("the mutex is never poisoned").len()
    }
}

#[async_trait]
impl Backend for MemoryStore {
    async fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError> {
        if snapshot.schema != SCHEMA_VERSION {
            return Err(StoreError::SchemaMismatch {
                found: snapshot.schema,
                understood: SCHEMA_VERSION,
            });
        }
        let mut characters = self.characters.lock().expect("the mutex is never poisoned");
        let mut items = self.items.lock().expect("the mutex is never poisoned");
        for record in &snapshot.characters {
            characters.insert(record.serial, record.clone());
        }
        // Each inventory replaces everything under its owner: drop the old set,
        // then write the new one.
        for inventory in &snapshot.inventories {
            items.retain(|_, item| item.owner != Some(inventory.owner));
            for item in &inventory.items {
                items.insert(item.serial, item.clone());
            }
        }
        // A ground sweep replaces every ownerless item at once.
        if let Some(ground) = &snapshot.ground {
            items.retain(|_, item| item.owner.is_some());
            for item in ground {
                items.insert(item.serial, item.clone());
            }
        }
        for serial in &snapshot.removed {
            // `snapshot.removed` carries the bare wire value (see
            // `Journal::forget_serial`) rather than a `Serial`: the character is
            // already gone by the time this runs, so there is nothing left to
            // build one from except the number itself. It was a valid `Serial`
            // when the row was written, so it stays one now.
            let serial = Serial::new(*serial).expect("a removed serial was valid when saved");
            characters.remove(&serial);
            // A gone character takes its inventory with it.
            items.retain(|_, item| item.owner != Some(serial));
        }
        // A mobile sweep replaces the whole set — and a mobile no longer in it
        // (killed since the last save) takes its worn gear and stock with it, or
        // dead vendors would leave orphaned crates in the items table forever.
        if let Some(records) = &snapshot.mobiles {
            let mut mobiles = self.mobiles.lock().expect("the mutex is never poisoned");
            let fresh: std::collections::HashSet<Serial> = records.iter().map(|m| m.serial).collect();
            let gone: Vec<Serial> = mobiles
                .keys()
                .filter(|serial| !fresh.contains(serial))
                .copied()
                .collect();
            for serial in gone {
                items.retain(|_, item| item.owner != Some(serial));
            }
            mobiles.clear();
            for record in records {
                mobiles.insert(record.serial, record.clone());
            }
        }
        drop(items);
        drop(characters);
        // A spawner sweep replaces the whole set at once.
        if let Some(records) = &snapshot.spawners {
            let mut spawners = self.spawners.lock().expect("the mutex is never poisoned");
            spawners.clear();
            for record in records {
                spawners.insert(record.id, record.clone());
            }
        }
        // A decoration sweep likewise.
        if let Some(records) = &snapshot.decorations {
            let mut decorations = self.decorations.lock().expect("the mutex is never poisoned");
            decorations.clear();
            for record in records {
                decorations.insert(record.serial, record.clone());
            }
        }
        // And the guilds, replace-all: one disbanded since the last save is
        // absent, and the clear is what makes that stick.
        if let Some(records) = &snapshot.guilds {
            let mut guilds = self.guilds.lock().expect("the mutex is never poisoned");
            guilds.clear();
            for record in records {
                guilds.insert(record.id, record.clone());
            }
        }
        // And the alliances, replace-all for the guilds' reason.
        if let Some(records) = &snapshot.alliances {
            let mut alliances = self.alliances.lock().expect("the mutex is never poisoned");
            alliances.clear();
            for record in records {
                alliances.insert(record.id, record.clone());
            }
        }
        // A design is replace-all like the houses, and for a sharper reason: a
        // commit rewrites a house's whole component list, so a merge would leave
        // the walls of the design before it standing beside the new ones.
        if let Some(records) = &snapshot.designs {
            let mut designs = self.designs.lock().expect("the mutex is never poisoned");
            designs.clear();
            designs.extend(records.iter().copied());
        }
        // The ships, on the houses' terms: a scuttling is an absence.
        if let Some(records) = &snapshot.boats {
            let mut boats = self.boats.lock().expect("the mutex is never poisoned");
            boats.clear();
            for record in records {
                boats.insert(record.serial.raw(), *record);
            }
        }
        // And the houses, on the same terms: a demolition is an absence.
        if let Some(records) = &snapshot.houses {
            let mut houses = self.houses.lock().expect("the mutex is never poisoned");
            houses.clear();
            for record in records {
                houses.insert(record.serial.raw(), record.clone());
            }
        }
        // The regions sweep replaces the whole map of the world at once.
        if let Some(records) = &snapshot.regions {
            let mut regions = self.regions.lock().expect("the mutex is never poisoned");
            regions.clear();
            for record in records {
                regions.insert((record.facet, record.id), record.clone());
            }
        }
        // The world's own row, replaced whole when a snapshot carries it.
        if let Some(record) = snapshot.world {
            *self.world.lock().expect("the mutex is never poisoned") = Some(record);
        }
        *self.saves.lock().expect("the mutex is never poisoned") += 1;
        Ok(())
    }

    async fn characters(&self) -> Result<Vec<CharacterRecord>, StoreError> {
        // Sorted, not `values()`: the trait promises ascending serial, and a
        // `HashMap` walks its own way round every process. This is the backend a
        // shard with no database runs on and the one every test uses, so an
        // unsorted read here is a slot order that is different each boot and a
        // test that is green on the run that wrote it.
        let mut characters = self
            .characters
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        characters.sort_by_key(|record| record.serial);
        Ok(characters)
    }

    async fn items(&self) -> Result<Vec<ItemRecord>, StoreError> {
        Ok(self
            .items
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect())
    }

    async fn spawners(&self) -> Result<Vec<SpawnerRecord>, StoreError> {
        Ok(self
            .spawners
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect())
    }

    async fn mobiles(&self) -> Result<Vec<MobileRecord>, StoreError> {
        Ok(self
            .mobiles
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect())
    }

    async fn decorations(&self) -> Result<Vec<DecorationRecord>, StoreError> {
        Ok(self
            .decorations
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect())
    }

    async fn guilds(&self) -> Result<Vec<GuildRecord>, StoreError> {
        let mut guilds: Vec<_> = self
            .guilds
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect();
        guilds.sort_by_key(|guild| guild.id);
        Ok(guilds)
    }

    async fn alliances(&self) -> Result<Vec<crate::record::AllianceRecord>, StoreError> {
        let mut alliances: Vec<_> = self
            .alliances
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect();
        alliances.sort_by_key(|alliance| alliance.id);
        Ok(alliances)
    }

    async fn designs(&self) -> Result<Vec<crate::record::HouseDesignRecord>, StoreError> {
        let mut designs = self.designs.lock().expect("the mutex is never poisoned").clone();
        designs.sort_by_key(|row| row.house.raw());
        Ok(designs)
    }

    async fn boats(&self) -> Result<Vec<crate::record::BoatRecord>, StoreError> {
        let mut boats: Vec<_> = self
            .boats
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .copied()
            .collect();
        boats.sort_by_key(|boat| boat.serial.raw());
        Ok(boats)
    }

    async fn houses(&self) -> Result<Vec<crate::record::HouseRecord>, StoreError> {
        let mut houses: Vec<_> = self
            .houses
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect();
        houses.sort_by_key(|house| house.serial.raw());
        Ok(houses)
    }

    async fn regions(&self) -> Result<Vec<RegionRecord>, StoreError> {
        Ok(self
            .regions
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect())
    }

    async fn world(&self) -> Result<Option<WorldRecord>, StoreError> {
        Ok(*self.world.lock().expect("the mutex is never poisoned"))
    }

    async fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError> {
        Ok(self
            .accounts
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect())
    }

    async fn put_account(&self, account: &AccountRecord) -> Result<(), StoreError> {
        self.accounts
            .lock()
            .expect("the mutex is never poisoned")
            .insert(account.name.clone(), account.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::StatLockRecord;
    use openshard_protocol::identity::CharacterName;
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

    #[tokio::test]
    async fn saving_the_same_serial_twice_updates_rather_than_duplicates() {
        // A save is an upsert keyed by serial. Getting this wrong gives you two
        // rows for one character and a load that picks whichever came back
        // first — which is the same character in two places, one of them stale.
        let store = MemoryStore::new();
        store
            .save(&snapshot(vec![character(1, 100)], vec![]))
            .await
            .expect("save");
        store
            .save(&snapshot(vec![character(1, 200)], vec![]))
            .await
            .expect("save");

        let characters = store.characters().await.expect("load");
        assert_eq!(characters.len(), 1);
        assert_eq!(characters[0].x, 200);
    }

    #[tokio::test]
    async fn characters_come_back_in_slot_order() {
        // The one read in this trait whose order a player sees: it is the account's
        // character list, and `0x83` picks by position in it. Saved out of order on
        // purpose, and enough of them that a `HashMap` walk agreeing by luck is one
        // arrangement in 8!.
        let store = MemoryStore::new();
        for serial in [6u32, 2, 8, 1, 5, 3, 7, 4] {
            store
                .save(&snapshot(vec![character(serial, 100)], vec![]))
                .await
                .expect("save");
        }

        let serials = store
            .characters()
            .await
            .expect("load")
            .iter()
            .map(|record| record.serial.raw())
            .collect::<Vec<_>>();
        assert_eq!(serials, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[tokio::test]
    async fn a_removal_takes_the_character_out() {
        let store = MemoryStore::new();
        store
            .save(&snapshot(vec![character(1, 100)], vec![]))
            .await
            .expect("save");
        store.save(&snapshot(vec![], vec![1])).await.expect("save");
        assert_eq!(store.character_count(), 0);
    }

    #[tokio::test]
    async fn a_save_from_the_future_is_refused_and_not_written() {
        // The point of refusing: the data must still be there afterwards,
        // untouched. A store that rejects the schema and writes anyway has
        // gained nothing.
        let store = MemoryStore::new();
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

        let characters = store.characters().await.expect("load");
        assert_eq!(characters[0].x, 100, "the refused save must not have landed");
    }

    fn contained(serial: u32, owner: u32, container: u32) -> ItemRecord {
        ItemRecord {
            serial: Serial::new(serial).expect("a valid test serial"),
            owner: Some(Serial::new(owner).expect("a valid test serial")),
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
            location: crate::record::ItemLocation::Contained {
                container: Serial::new(container).expect("a valid test serial"),
                x: 0,
                y: 0,
                grid: 0,
            },
        }
    }

    fn ground(serial: u32) -> ItemRecord {
        ItemRecord {
            serial: Serial::new(serial).expect("a valid test serial"),
            owner: None,
            graphic: 0x1BFB,
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
            location: crate::record::ItemLocation::Ground {
                facet: 0,
                x: 1400,
                y: 1600,
                z: 0,
            },
        }
    }

    #[tokio::test]
    async fn an_inventory_save_replaces_the_owners_items() {
        // A character reorganises: the store holds what the last save said, not a
        // union of every save. Two items, then one, leaves one — not three.
        let store = MemoryStore::new();
        store
            .save(&Snapshot {
                tick: 1,
                schema: SCHEMA_VERSION,
                characters: vec![character(1, 100)],
                removed: vec![],
                inventories: vec![crate::record::Inventory {
                    owner: Serial::new(1).expect("a valid test serial"),
                    items: vec![contained(0x4000_0001, 1, 1), contained(0x4000_0002, 1, 1)],
                }],
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
            })
            .await
            .expect("save");
        store
            .save(&Snapshot {
                tick: 2,
                schema: SCHEMA_VERSION,
                characters: vec![character(1, 100)],
                removed: vec![],
                inventories: vec![crate::record::Inventory {
                    owner: Serial::new(1).expect("a valid test serial"),
                    items: vec![contained(0x4000_0001, 1, 1)],
                }],
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
            })
            .await
            .expect("save");

        let items = store.items().await.expect("load");
        assert_eq!(items.len(), 1, "the owner's items are replaced, not merged");
    }

    fn mobile(serial: u32, hits: u16) -> crate::record::MobileRecord {
        crate::record::MobileRecord {
            serial: Serial::new(serial).expect("a valid test serial"),
            body: 0x00C8,
            hue: 0,
            facet: 0,
            x: 1400,
            y: 1600,
            z: 0,
            facing: 0,
            name: None,
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
            vendor: false,
            healer: false,
            title: None,
            npc_home: None,
            night_home: None,
            pet: None,
            restock: None,
            npc_wander: 0,
            spawned_by: Some(1),
            effects: Vec::new(),
            skills: Vec::new(),
            quest_giver: Vec::new(),
            escort_destination: None,
        }
    }

    #[tokio::test]
    async fn a_mobile_sweep_replaces_the_set_and_a_dead_mobiles_items_go_with_it() {
        // The whole-world model: the store holds what the last sweep said. A
        // mobile absent from the new sweep was killed — it must vanish, and so
        // must its worn gear, or dead vendors leave orphaned crates forever.
        let store = MemoryStore::new();
        store
            .save(&Snapshot {
                tick: 1,
                schema: SCHEMA_VERSION,
                characters: vec![],
                removed: vec![],
                inventories: vec![crate::record::Inventory {
                    owner: Serial::new(2).expect("a valid test serial"),
                    items: vec![contained(0x4000_0001, 2, 2)],
                }],
                ground: None,
                spawners: None,
                mobiles: Some(vec![mobile(2, 30), mobile(3, 30)]),
                decorations: None,
                regions: None,
                guilds: None,
                alliances: None,
                houses: None,
                designs: None,
                boats: None,
                world: None,
            })
            .await
            .expect("save");
        // The next sweep: mobile 2 died (and its inventory was not re-swept),
        // mobile 3 lives on wounded.
        store
            .save(&Snapshot {
                tick: 2,
                schema: SCHEMA_VERSION,
                characters: vec![],
                removed: vec![],
                inventories: vec![],
                ground: None,
                spawners: None,
                mobiles: Some(vec![mobile(3, 7)]),
                decorations: None,
                regions: None,
                guilds: None,
                alliances: None,
                houses: None,
                designs: None,
                boats: None,
                world: None,
            })
            .await
            .expect("save");

        let mobiles = store.mobiles().await.expect("load");
        assert_eq!(mobiles.len(), 1, "the dead mobile is gone");
        assert_eq!(mobiles[0].serial, Serial::new(3).expect("a valid test serial"));
        assert_eq!(mobiles[0].hits_current, 7, "the survivor keeps its wounds");
        assert!(
            store.items().await.expect("load").is_empty(),
            "the dead mobile's items went with it"
        );
    }

    #[tokio::test]
    async fn a_ground_sweep_replaces_only_ground_and_removing_a_character_takes_its_items() {
        let store = MemoryStore::new();
        store
            .save(&Snapshot {
                tick: 1,
                schema: SCHEMA_VERSION,
                characters: vec![character(1, 100)],
                removed: vec![],
                inventories: vec![crate::record::Inventory {
                    owner: Serial::new(1).expect("a valid test serial"),
                    items: vec![contained(0x4000_0001, 1, 1)],
                }],
                ground: Some(vec![ground(0x4000_0010)]),
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
            })
            .await
            .expect("save");
        // A later ground sweep leaves the inventory alone.
        store
            .save(&Snapshot {
                tick: 2,
                schema: SCHEMA_VERSION,
                characters: vec![],
                removed: vec![],
                inventories: vec![],
                ground: Some(vec![ground(0x4000_0011)]),
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
            })
            .await
            .expect("save");
        let items = store.items().await.expect("load");
        assert_eq!(items.len(), 2, "one inventory item, one fresh ground item");

        // Deleting the character deletes its inventory but not the ground item.
        store.save(&snapshot(vec![], vec![1])).await.expect("save");
        let items = store.items().await.expect("load");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].owner, None, "only the ground item survives");
    }
}
