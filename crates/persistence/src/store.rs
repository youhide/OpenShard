//! The database, as an interface.
//!
//! # Why a trait and not just SQLite
//!
//! A shard runs on SQLite or on PostgreSQL, whichever the operator prefers —
//! neither is "the real one", and SQLite is a fine choice for a live shard. But
//! the reason for the trait is narrower than "swappable backends": it is that the
//! *tests* need a store that cannot fail, and every backend can fail.
//! [`MemoryStore`] is what lets a test assert what the world hands to persistence
//! without a database anywhere near it.
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

use crate::journal::Snapshot;
use crate::record::{
    AccountRecord, CharacterRecord, DecorationRecord, ItemRecord, MobileRecord, SpawnerRecord,
    SCHEMA_VERSION,
};

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

/// Somewhere the world can be kept.
///
/// `Send + Sync` and by-reference: a store is shared, and the drain task holds
/// it for the life of the shard.
#[async_trait]
pub trait Store: Send + Sync {
    /// Write a snapshot.
    ///
    /// # Must be atomic
    ///
    /// All of it or none of it. The snapshot is a consistent picture of one
    /// tick, and half of it is a world that never existed — see the `journal`
    /// module. A backend that cannot do a transaction is not a
    /// backend that can implement this.
    async fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError>;

    /// Every character.
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

    /// Every account.
    async fn accounts(&self) -> Result<Vec<AccountRecord>, StoreError>;

    /// Add or update an account.
    async fn put_account(&self, account: &AccountRecord) -> Result<(), StoreError>;
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
    characters: Mutex<HashMap<u32, CharacterRecord>>,
    /// Items keyed by serial: inventory (owner is a character) and ground (owner 0).
    items: Mutex<HashMap<u32, ItemRecord>>,
    /// Spawn regions keyed by id.
    spawners: Mutex<HashMap<u32, SpawnerRecord>>,
    /// NPC mobiles keyed by serial.
    mobiles: Mutex<HashMap<u32, MobileRecord>>,
    /// Placed decorations keyed by serial.
    decorations: Mutex<HashMap<u32, DecorationRecord>>,
    accounts: Mutex<HashMap<String, AccountRecord>>,
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
        self.characters
            .lock()
            .expect("the mutex is never poisoned")
            .len()
    }
}

#[async_trait]
impl Store for MemoryStore {
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
            items.retain(|_, item| item.owner != inventory.owner);
            for item in &inventory.items {
                items.insert(item.serial, item.clone());
            }
        }
        // A ground sweep replaces every ownerless item at once.
        if let Some(ground) = &snapshot.ground {
            items.retain(|_, item| item.owner != 0);
            for item in ground {
                items.insert(item.serial, item.clone());
            }
        }
        for serial in &snapshot.removed {
            characters.remove(serial);
            // A gone character takes its inventory with it.
            items.retain(|_, item| item.owner != *serial);
        }
        // A mobile sweep replaces the whole set — and a mobile no longer in it
        // (killed since the last save) takes its worn gear and stock with it, or
        // dead vendors would leave orphaned crates in the items table forever.
        if let Some(records) = &snapshot.mobiles {
            let mut mobiles = self.mobiles.lock().expect("the mutex is never poisoned");
            let fresh: std::collections::HashSet<u32> = records.iter().map(|m| m.serial).collect();
            let gone: Vec<u32> = mobiles
                .keys()
                .filter(|serial| !fresh.contains(serial))
                .copied()
                .collect();
            for serial in gone {
                items.retain(|_, item| item.owner != serial);
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
            let mut decorations = self
                .decorations
                .lock()
                .expect("the mutex is never poisoned");
            decorations.clear();
            for record in records {
                decorations.insert(record.serial, record.clone());
            }
        }
        *self.saves.lock().expect("the mutex is never poisoned") += 1;
        Ok(())
    }

    async fn characters(&self) -> Result<Vec<CharacterRecord>, StoreError> {
        Ok(self
            .characters
            .lock()
            .expect("the mutex is never poisoned")
            .values()
            .cloned()
            .collect())
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
            inventories: vec![],
            ground: None,
            spawners: None,
            mobiles: None,
            decorations: None,
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
        };
        let error = store.save(&future).await.expect_err("must refuse");
        assert!(matches!(error, StoreError::SchemaMismatch { .. }));

        let characters = store.characters().await.expect("load");
        assert_eq!(
            characters[0].x, 100,
            "the refused save must not have landed"
        );
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
            location: crate::record::ItemLocation::Contained {
                container,
                x: 0,
                y: 0,
                grid: 0,
            },
        }
    }

    fn ground(serial: u32) -> ItemRecord {
        ItemRecord {
            serial,
            owner: 0,
            graphic: 0x1BFB,
            hue: 0,
            amount: 1,
            stackable: false,
            container_gump: None,
            price: None,
            name: None,
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
                    owner: 1,
                    items: vec![contained(0x4000_0001, 1, 1), contained(0x4000_0002, 1, 1)],
                }],
                ground: None,
                spawners: None,
                mobiles: None,
                decorations: None,
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
                    owner: 1,
                    items: vec![contained(0x4000_0001, 1, 1)],
                }],
                ground: None,
                spawners: None,
                mobiles: None,
                decorations: None,
            })
            .await
            .expect("save");

        let items = store.items().await.expect("load");
        assert_eq!(items.len(), 1, "the owner's items are replaced, not merged");
    }

    fn mobile(serial: u32, hits: u16) -> crate::record::MobileRecord {
        crate::record::MobileRecord {
            serial,
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
            vendor: false,
            npc_home: None,
            npc_wander: 0,
            spawned_by: Some(1),
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
                    owner: 2,
                    items: vec![contained(0x4000_0001, 2, 2)],
                }],
                ground: None,
                spawners: None,
                mobiles: Some(vec![mobile(2, 30), mobile(3, 30)]),
                decorations: None,
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
            })
            .await
            .expect("save");

        let mobiles = store.mobiles().await.expect("load");
        assert_eq!(mobiles.len(), 1, "the dead mobile is gone");
        assert_eq!(mobiles[0].serial, 3);
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
                    owner: 1,
                    items: vec![contained(0x4000_0001, 1, 1)],
                }],
                ground: Some(vec![ground(0x4000_0010)]),
                spawners: None,
                mobiles: None,
                decorations: None,
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
            })
            .await
            .expect("save");
        let items = store.items().await.expect("load");
        assert_eq!(items.len(), 2, "one inventory item, one fresh ground item");

        // Deleting the character deletes its inventory but not the ground item.
        store.save(&snapshot(vec![], vec![1])).await.expect("save");
        let items = store.items().await.expect("load");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].owner, 0, "only the ground item survives");
    }
}
