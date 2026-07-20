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
//! - [`Store`] is a database, and [`MemoryStore`] is the one that cannot fail.
//!   [`SqliteStore`] and [`PgStore`] are the two real backends; which a shard
//!   runs is the operator's choice, and neither is a tier.
//! - [`record`] is what the shapes look like on disk, which is deliberately
//!   *not* what the components look like in memory.
//!
//! # What is not here yet
//!
//! Items: the journal takes entities, and a character is the only thing it knows
//! how to record so far. See `docs/roadmap.md`.

mod journal;
mod pg;
pub mod record;
mod sqlite;
mod store;

pub use journal::{Journal, Snapshot};
pub use pg::PgStore;
pub use record::{
    AccountRecord, CharacterRecord, CreatureData, DecorationRecord, DoorState, EffectRecord,
    Inventory, ItemLocation, ItemRecord, MobileRecord, SkillRecord, SpawnerRecord, EFFECT_POISON,
    SCHEMA_VERSION,
};
pub use sqlite::SqliteStore;
pub use store::{MemoryStore, Store, StoreError};
