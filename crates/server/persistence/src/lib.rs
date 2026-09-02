//! Keeping the world: the save queue, the records, and the backends.
//!
//! # The one rule
//!
//! The database is never touched inside a tick.
//!
//! Everything here is shaped around that. The world is in memory and stays
//! there; persistence is something that happens *next to* the simulation, on a
//! task the tick never waits for. A shard whose disk is slow is a shard that
//! saves late — never a shard that lags.
//!
//! # The shape
//!
//! ```text
//!   inside the tick                   outside the tick
//!   ────────────────                  ────────────────
//!   Journal::touch(entity)      the world changed
//!   Journal::drain(tick, ..)  ───>  Snapshot  ───>  Store::save(..).await
//!        a memcpy                 owned values        the slow part
//! ```
//!
//! - [`Journal`] tracks what changed and hands it over exactly once.
//! - [`Snapshot`] is that handover: owned, consistent, taken at one tick.
//! - [`Store`] makes the database choice explicit. [`MemoryStore`],
//!   [`SqliteStore`], and [`PgStore`] are its three backends.
//! - [`record`] is what the shapes look like on disk, which is deliberately
//!   *not* what the components look like in memory.
//!
//! # What is recorded
//!
//! The whole world, not a character and its pack: every online character in full,
//! every live mobile, ground clutter, decoration, spawn regions with their
//! remaining timers, named regions, guilds, houses, boats and the world clock.
//! The model is `docs/server/design_persistence.md`; what each schema version
//! added is `docs/server/evidence/2026-08-24-the-persistence-phase.md`.

mod journal;
mod pg;
pub mod record;
mod sqlite;
mod store;

pub use journal::{
    Journal,
    Snapshot,
};
pub use pg::PgStore;
pub use record::{
    AccountRecord,
    AllianceRecord,
    CharacterRecord,
    CorpseData,
    CorpseEquipmentData,
    CreatureData,
    DecorationRecord,
    DoneQuestRecord,
    DoorState,
    EFFECT_POISON,
    EffectRecord,
    GuildRecord,
    GuildStanding,
    Inventory,
    ItemAffixRecord,
    ItemLocation,
    ItemRecord,
    MobileRecord,
    PetData,
    QuestRecord,
    RegionRecord,
    RestockLineRecord,
    RestockRecord,
    RunebookData,
    RunebookEntryData,
    SCHEMA_VERSION,
    SkillRecord,
    SpawnerRecord,
    StatLockRecord,
    WorldRecord,
};
pub use sqlite::SqliteStore;
pub use store::{
    MemoryStore,
    Store,
    StoreError,
};

/// Decode one nullable JSON field whose absence is meaningful item state.
///
/// Malformed JSON is not absence: treating it that way would let the next save
/// persist the invented empty state over the only copy of the original value.
fn item_json<T: serde::de::DeserializeOwned>(
    json: Option<String>,
    field: &'static str,
) -> Result<Option<T>, StoreError> {
    json.map(|json| {
        serde_json::from_str(&json)
            .map_err(|error| StoreError::Corrupt(format!("invalid item {field}: {error}")))
    })
    .transpose()
}
