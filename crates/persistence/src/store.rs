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
use crate::record::{AccountRecord, CharacterRecord, SCHEMA_VERSION};

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
    /// tick, and half of it is a world that never existed — see
    /// [`crate::journal`]. A backend that cannot do a transaction is not a
    /// backend that can implement this.
    async fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError>;

    /// Every character.
    async fn characters(&self) -> Result<Vec<CharacterRecord>, StoreError>;

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
        for record in &snapshot.characters {
            characters.insert(record.serial, record.clone());
        }
        for serial in &snapshot.removed {
            characters.remove(serial);
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
        };
        let error = store.save(&future).await.expect_err("must refuse");
        assert!(matches!(error, StoreError::SchemaMismatch { .. }));

        let characters = store.characters().await.expect("load");
        assert_eq!(
            characters[0].x, 100,
            "the refused save must not have landed"
        );
    }
}
