//! What changed, and the one moment it is allowed to be read.
//!
//! # The rule this file exists to enforce
//!
//! The database is never touched inside a tick. Not "rarely" — never. A tick
//! that waits on a disk is a tick that took however long the disk took, and at
//! 20Hz there is 50ms of budget for the entire world.
//!
//! But the *data* can only be read honestly from inside a tick, because that is
//! the only moment nothing is half-applied. So the two halves are split:
//!
//! ```text
//!   inside the tick            outside the tick
//!   ──────────────────         ─────────────────
//!   journal.drain(..)   ───>   store.save(snapshot).await
//!   a memcpy of what                the slow part
//!   changed, taken at
//!   one instant
//! ```
//!
//! [`Journal::drain`] is synchronous and takes a closure over the world's own
//! data, so it can only be called from where that data lives. [`Snapshot`] is
//! plain owned values, so it can go anywhere. The boundary is the type
//! signature, not a comment asking nicely.
//!
//! # Why the snapshot is taken all at once
//!
//! This is the part that looks like an optimisation and is not.
//!
//! If a character is snapshotted at tick 100 and the item in its pack at tick
//! 140, the pair that reaches disk is a world that never existed. That is not a
//! theoretical concern in UO — it is the shape of every duplication bug the game
//! has ever had. Player drags an item out of a container; the save catches the
//! container still holding it *and* the ground already having it; the shard
//! restarts and there are two.
//!
//! One `drain`, one instant, one transaction. The write is allowed to be slow.
//! It is not allowed to be a different world halfway through.
//!
//! # Both of the other emulators stop the world instead
//!
//! This is the alternative, and it is worth knowing what it costs, because two
//! independent projects arrived at it and neither is run by fools.
//!
//! Sphere walks the whole world to save it. ServUO does the same and pauses the
//! network while it does — `NetState.Pause()`, a broadcast to every player
//! reading "The world is saving, please wait", and a global `Saving` flag that
//! the rest of the engine has to know about. Anything that spawns or deletes
//! during the window goes into a "safety queue" and is written to
//! `world-save-errors.log` with the advice that "the offending scripts be
//! corrected".
//!
//! Read that log message again. It is not a bug in anyone's scripts. It is the
//! save design leaking into every other file in the engine: because the save is
//! not atomic with respect to the world, the world is required to hold still,
//! and the parts that will not hold still become the script author's problem.
//!
//! The split here is what buys that back. The world never holds still, no system
//! needs to know a save is happening, and there is no flag to check because
//! there is no window to be inside of. What it costs is this file — the price is
//! paid once, here, instead of being spread across everything that ever mutates
//! anything.

use std::collections::HashSet;

use openshard_entities::{EntityId, Serial};

use crate::record::{CharacterRecord, Inventory, ItemRecord, SpawnerRecord, SCHEMA_VERSION};

/// A consistent picture of everything that changed, taken at one tick.
///
/// Owned values only: this crosses out of the simulation and must not borrow
/// anything the next tick is about to write to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Snapshot {
    /// The tick it was taken at. For logging, and for spotting a store that has
    /// fallen behind.
    pub tick: u64,
    /// The shape the records are in. Travels with the data, so a store can
    /// refuse a save it does not understand rather than write it anyway.
    pub schema: u32,
    /// Characters that changed.
    pub characters: Vec<CharacterRecord>,
    /// Serials that are gone and must be deleted.
    pub removed: Vec<u32>,
    /// The full carried inventory of each character in this snapshot: the store
    /// replaces everything under each `owner` rather than diffing item by item.
    pub inventories: Vec<Inventory>,
    /// Every loose item on the ground, when this snapshot swept it. `Some(_)`
    /// replaces the whole ground; `None` leaves the stored ground untouched (this
    /// snapshot only carried character changes).
    pub ground: Option<Vec<ItemRecord>>,
    /// Every spawn region and its respawn timer, when this snapshot swept them.
    /// `Some(_)` replaces the whole set; `None` leaves the stored spawners be.
    pub spawners: Option<Vec<SpawnerRecord>>,
}

impl Snapshot {
    /// Whether this snapshot would write nothing.
    pub fn is_empty(&self) -> bool {
        self.characters.is_empty()
            && self.removed.is_empty()
            && self.inventories.is_empty()
            && self.ground.is_none()
            && self.spawners.is_none()
    }

    /// How many rows this would touch.
    pub fn len(&self) -> usize {
        self.characters.len()
            + self.removed.len()
            + self
                .inventories
                .iter()
                .map(|inv| inv.items.len())
                .sum::<usize>()
            + self.ground.as_ref().map_or(0, Vec::len)
            + self.spawners.as_ref().map_or(0, Vec::len)
    }
}

/// What has changed since the last save.
///
/// # Why a dirty set and not a full sweep
///
/// Sphere saves the world by walking all of it, and that is what a world save
/// costs there: the whole shard pauses proportionally to how much is in it. It
/// works, and it is also the thing every operator learns to schedule around.
///
/// Marking what changed makes the common save proportional to *activity* rather
/// than to size. A shard with fifty thousand items and four players awake saves
/// four things.
///
/// The cost is that a missed [`Journal::touch`] is a silent, delayed data loss:
/// the change is in memory, looks right for hours, and is simply not there after
/// a restart. That is a genuinely worse failure than a slow save, which is why
/// [`Journal::touch_all`] exists — a full sweep is always correct, and a shard
/// that suspects it is dropping writes can fall back to one.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Journal {
    dirty: HashSet<EntityId>,
    kept: Vec<CharacterRecord>,
    kept_inventories: Vec<Inventory>,
    removed: HashSet<u32>,
}

impl Journal {
    /// An empty journal.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark an entity as needing saving.
    ///
    /// Idempotent, and cheap enough to call on every change: the alternative —
    /// calling it only where it "obviously matters" — is how a field goes
    /// unsaved for a month.
    pub fn touch(&mut self, entity: EntityId) {
        self.dirty.insert(entity);
    }

    /// Mark everything in an iterator as needing saving.
    ///
    /// The escape hatch: a full sweep is always correct, whatever the dirty
    /// tracking has missed.
    pub fn touch_all(&mut self, entities: impl IntoIterator<Item = EntityId>) {
        self.dirty.extend(entities);
    }

    /// Record something now, because it will not be readable later.
    ///
    /// # Why this exists and `touch` is not enough
    ///
    /// [`touch`](Self::touch) is a promise to read the entity at save time. That
    /// promise cannot be kept for a character logging out: by the time the next
    /// save comes round the entity is despawned and its components are gone, so
    /// the read returns nothing and the last thing the player did — the whole
    /// session — is never written.
    ///
    /// Logout is exactly when a save matters most, so the record is taken at the
    /// one moment it can be: before the despawn.
    pub fn keep(&mut self, record: CharacterRecord) {
        self.kept.push(record);
    }

    /// Record a logging-out character's whole inventory, for the same reason as
    /// [`keep`](Self::keep): in a moment the entity is despawned and its worn and
    /// contained items are unreadable, so they are walked now, before the despawn.
    /// An online character's inventory is walked live at snapshot time instead.
    pub fn keep_inventory(&mut self, inventory: Inventory) {
        self.kept_inventories.push(inventory);
    }

    /// Mark a serial as deleted.
    ///
    /// Takes the serial and not just the entity because by the time anything
    /// asks, the entity is gone and there is nothing left to look the serial up
    /// on. This is the same reason `World::disconnect` reads the serial before
    /// it despawns.
    pub fn forget(&mut self, entity: EntityId, serial: Serial) {
        // The delete wins over any pending write. Otherwise a character deleted
        // in the same save window as it moved gets written back by its own
        // gravestone.
        self.dirty.remove(&entity);
        self.removed.insert(serial.raw());
    }

    /// Whether a save would write nothing.
    pub fn is_empty(&self) -> bool {
        self.dirty.is_empty()
            && self.kept.is_empty()
            && self.kept_inventories.is_empty()
            && self.removed.is_empty()
    }

    /// How many rows are waiting to be written.
    pub fn len(&self) -> usize {
        self.dirty.len() + self.kept.len() + self.removed.len()
    }

    /// Whether an entity is waiting to be saved.
    pub fn is_dirty(&self, entity: EntityId) -> bool {
        self.dirty.contains(&entity)
    }

    /// Take everything that changed, as of `tick`.
    ///
    /// `record` turns an entity into what should be written; returning `None`
    /// drops it, which is the honest answer for an entity that is not a
    /// character or was despawned without being forgotten.
    ///
    /// Returns `None` when there is nothing to write, so a quiet shard queues
    /// nothing rather than a stream of empty transactions.
    ///
    /// # This clears the journal
    ///
    /// The caller now owns the only copy of these changes. If the write fails
    /// they are not coming back from here: the recovery is to mark the world
    /// dirty again with [`touch_all`](Self::touch_all) and let the next save
    /// read it fresh. Re-writing this snapshot would write where everyone
    /// *was*, and the world has kept moving.
    pub fn drain<F>(&mut self, tick: u64, mut record: F) -> Option<Snapshot>
    where
        F: FnMut(EntityId) -> Option<CharacterRecord>,
    {
        if self.is_empty() {
            return None;
        }
        // Kept records first, then read the dirty entities. Order matters if a
        // serial is in both: a character that logged out and whose entity was
        // somehow still readable would otherwise be written from the live read,
        // and the live read is the one that is about to be wrong.
        let mut characters = std::mem::take(&mut self.kept);
        characters.extend(self.dirty.drain().filter_map(&mut record));
        let removed = self.removed.drain().collect();
        // Logged-out characters' inventories, captured before their despawn. Online
        // characters' inventories are added live by the caller after the drain —
        // `ground` likewise — because both need to read the still-live world.
        let inventories = std::mem::take(&mut self.kept_inventories);
        Some(Snapshot {
            tick,
            schema: SCHEMA_VERSION,
            characters,
            removed,
            inventories,
            ground: None,
            spawners: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_entities::{Registry, SerialKind};

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

    #[test]
    fn a_quiet_world_saves_nothing() {
        // The reason `drain` returns Option. A shard where nobody is doing
        // anything must not queue a transaction every tick just to say so.
        let mut journal = Journal::new();
        assert!(journal.drain(1, |_| Some(character(1, 0))).is_none());
    }

    #[test]
    fn touching_twice_saves_once() {
        // A character that moved thirty times this second is one row.
        let mut registry = Registry::new();
        let entity = registry.spawn();
        let mut journal = Journal::new();
        for _ in 0..30 {
            journal.touch(entity);
        }
        let snapshot = journal.drain(1, |_| Some(character(1, 5))).expect("dirty");
        assert_eq!(snapshot.characters.len(), 1);
    }

    #[test]
    fn draining_clears_the_journal() {
        let mut registry = Registry::new();
        let entity = registry.spawn();
        let mut journal = Journal::new();
        journal.touch(entity);
        journal.drain(1, |_| Some(character(1, 5))).expect("dirty");
        assert!(journal.is_empty(), "a drained journal must be empty");
        assert!(journal.drain(2, |_| Some(character(1, 5))).is_none());
    }

    #[test]
    fn a_deleted_character_is_not_also_written_back() {
        // The gravestone bug. A character that moved and was then deleted in the
        // same save window must not be resurrected by its own pending write.
        let mut registry = Registry::new();
        let (entity, serial) = registry
            .spawn_with_serial(SerialKind::Mobile)
            .expect("a serial");

        let mut journal = Journal::new();
        journal.touch(entity);
        journal.forget(entity, serial);

        let snapshot = journal
            .drain(1, |_| panic!("a forgotten entity must not be recorded"))
            .expect("a deletion is a change");
        assert!(snapshot.characters.is_empty());
        assert_eq!(snapshot.removed, vec![serial.raw()]);
    }

    #[test]
    fn a_deletion_alone_is_worth_saving() {
        // The opposite mistake: treating "no dirty entities" as "nothing to do"
        // leaves the deleted character on disk forever.
        let mut registry = Registry::new();
        let (entity, serial) = registry
            .spawn_with_serial(SerialKind::Mobile)
            .expect("a serial");

        let mut journal = Journal::new();
        journal.forget(entity, serial);
        let snapshot = journal.drain(1, |_| None).expect("a deletion is a change");
        assert_eq!(snapshot.len(), 1);
        assert!(!snapshot.is_empty());
    }

    #[test]
    fn an_entity_with_nothing_to_record_is_dropped_quietly() {
        // `record` returning None is normal, not an error: the journal tracks
        // entities and not everything is a character.
        let mut registry = Registry::new();
        let a = registry.spawn();
        let b = registry.spawn();
        let mut journal = Journal::new();
        journal.touch(a);
        journal.touch(b);
        let snapshot = journal
            .drain(1, |entity| (entity == a).then(|| character(1, 5)))
            .expect("dirty");
        assert_eq!(snapshot.characters.len(), 1);
    }

    #[test]
    fn a_snapshot_carries_the_schema_it_was_written_against() {
        // So a store can refuse rather than guess.
        let mut registry = Registry::new();
        let entity = registry.spawn();
        let mut journal = Journal::new();
        journal.touch(entity);
        let snapshot = journal.drain(7, |_| Some(character(1, 5))).expect("dirty");
        assert_eq!(snapshot.schema, SCHEMA_VERSION);
        assert_eq!(snapshot.tick, 7);
    }

    #[test]
    fn a_full_sweep_saves_everything_whatever_the_tracking_missed() {
        // The escape hatch has to actually work: this is what a shard falls back
        // to when it suspects a `touch` is missing somewhere.
        let mut registry = Registry::new();
        let entities: Vec<_> = (0..10).map(|_| registry.spawn()).collect();
        let mut journal = Journal::new();
        journal.touch_all(entities.iter().copied());
        assert_eq!(journal.len(), 10);
        let snapshot = journal.drain(1, |_| Some(character(1, 5))).expect("dirty");
        assert_eq!(snapshot.characters.len(), 10);
    }
}
