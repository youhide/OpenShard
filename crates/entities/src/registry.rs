//! The entity registry: the single owner of every entity and component.

use std::any::TypeId;
use std::collections::HashMap;
use std::{fmt, iter, option};

use crate::component::{split_two, Column, Component, Iter, IterMut, SparseSet};
use crate::entity::{EntityAllocator, EntityId};
use crate::serial::{Serial, SerialAllocator, SerialKind, SerialPoolExhausted};

/// Iterator returned by [`Registry::query`].
///
/// Spelled out rather than `impl Iterator` so callers can name it in struct
/// fields and trait signatures.
pub type Query<'a, T> = iter::Flatten<option::IntoIter<Iter<'a, T>>>;

/// Iterator returned by [`Registry::query_mut`].
pub type QueryMut<'a, T> = iter::Flatten<option::IntoIter<IterMut<'a, T>>>;

/// Binding a serial failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BindSerialError {
    /// The entity is dead or was never spawned here.
    NoSuchEntity(EntityId),
    /// The serial is already bound to a different entity.
    SerialTaken {
        /// The serial that is already in use.
        serial: Serial,
        /// Who currently holds it.
        holder: EntityId,
    },
    /// The entity already has a different serial. Serials are not reassignable.
    AlreadyBound {
        /// The entity in question.
        entity: EntityId,
        /// The serial it already holds.
        existing: Serial,
    },
}

impl fmt::Display for BindSerialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchEntity(e) => write!(f, "{e:?} is not alive"),
            Self::SerialTaken { serial, holder } => {
                write!(f, "{serial} is already bound to {holder:?}")
            }
            Self::AlreadyBound { entity, existing } => {
                write!(f, "{entity:?} is already bound to {existing}")
            }
        }
    }
}

impl std::error::Error for BindSerialError {}

/// Spawning an entity with a fresh serial failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpawnError(pub SerialPoolExhausted);

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot spawn: {}", self.0)
    }
}

impl std::error::Error for SpawnError {}

/// Owns every entity, every component, and the serial index.
///
/// There is no global state: a `Registry` is a plain value that the world server
/// holds and passes to systems. Tests spin up as many as they like.
///
/// # Serials
/// The wire identity lives here rather than in a separate index because keeping
/// the two in sync manually is a bug factory — [`Registry::despawn`] releasing
/// the serial mapping is the whole point.
#[derive(Default)]
pub struct Registry {
    entities: EntityAllocator,
    /// Columns, one per component type. Indexed by `column_index`; never
    /// reordered or removed, so indices stay stable for the lifetime of the
    /// registry.
    columns: Vec<Box<dyn Column>>,
    column_index: HashMap<TypeId, usize>,
    serial_to_entity: HashMap<Serial, EntityId>,
    entity_to_serial: HashMap<EntityId, Serial>,
    serials: SerialAllocator,
}

impl fmt::Debug for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Registry")
            .field("entities", &self.entities.len())
            .field("component_types", &self.columns.len())
            .field("serials_bound", &self.serial_to_entity.len())
            .finish()
    }
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    // -- entity lifecycle -------------------------------------------------

    /// Create an entity with no components and no serial.
    ///
    /// Use this for entities the client never sees. Anything a client can
    /// address needs [`Registry::spawn_with_serial`].
    pub fn spawn(&mut self) -> EntityId {
        self.entities.alloc()
    }

    /// Create an entity and bind it to a fresh serial from `kind`'s pool.
    pub fn spawn_with_serial(
        &mut self,
        kind: SerialKind,
    ) -> Result<(EntityId, Serial), SpawnError> {
        // Allocate the serial first: if the pool is exhausted we must not leave
        // a dangling entity behind.
        let serial = self.serials.alloc(kind).map_err(SpawnError)?;
        let entity = self.entities.alloc();
        self.serial_to_entity.insert(serial, entity);
        self.entity_to_serial.insert(entity, serial);
        Ok((entity, serial))
    }

    /// Destroy `entity`, dropping every component and releasing its serial.
    ///
    /// Returns `false` if it was already dead. Safe to call with a stale handle.
    pub fn despawn(&mut self, entity: EntityId) -> bool {
        if !self.entities.contains(entity) {
            return false;
        }
        for column in &mut self.columns {
            column.remove_erased(entity);
        }
        if let Some(serial) = self.entity_to_serial.remove(&entity) {
            self.serial_to_entity.remove(&serial);
        }
        self.entities.free(entity)
    }

    /// Whether `entity` is alive.
    #[inline]
    pub fn contains(&self, entity: EntityId) -> bool {
        self.entities.contains(entity)
    }

    /// How many entities are alive.
    #[inline]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Whether the registry holds no live entities.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entities.len() == 0
    }

    /// Destroy everything. Serial watermarks are *not* rewound.
    pub fn clear(&mut self) {
        for column in &mut self.columns {
            column.clear_erased();
        }
        self.entities.clear();
        self.serial_to_entity.clear();
        self.entity_to_serial.clear();
    }

    // -- components -------------------------------------------------------

    /// Attach `value` to `entity`, returning the previous value if any.
    ///
    /// Returns `None` and stores nothing if the entity is dead — a stale handle
    /// must never be able to resurrect an entity by writing to it.
    pub fn insert<T: Component>(&mut self, entity: EntityId, value: T) -> Option<T> {
        if !self.entities.contains(entity) {
            return None;
        }
        self.column_or_insert::<T>().insert(entity, value)
    }

    /// Detach `entity`'s `T` and return it.
    pub fn remove<T: Component>(&mut self, entity: EntityId) -> Option<T> {
        if !self.entities.contains(entity) {
            return None;
        }
        self.column_mut::<T>()?.remove(entity)
    }

    /// Borrow `entity`'s `T`.
    #[inline]
    pub fn get<T: Component>(&self, entity: EntityId) -> Option<&T> {
        self.column::<T>()?.get(entity)
    }

    /// Mutably borrow `entity`'s `T`.
    #[inline]
    pub fn get_mut<T: Component>(&mut self, entity: EntityId) -> Option<&mut T> {
        self.column_mut::<T>()?.get_mut(entity)
    }

    /// Whether `entity` has a `T`.
    #[inline]
    pub fn has<T: Component>(&self, entity: EntityId) -> bool {
        self.column::<T>().is_some_and(|c| c.contains(entity))
    }

    /// How many entities have a `T`.
    pub fn count<T: Component>(&self) -> usize {
        self.column::<T>().map_or(0, SparseSet::len)
    }

    // -- serials ----------------------------------------------------------

    /// Bind an existing serial to an existing entity.
    ///
    /// This is the path used when loading a save, where serials are dictated by
    /// the data rather than allocated. It also reserves the serial so the
    /// allocator will never hand it out again.
    pub fn bind_serial(&mut self, entity: EntityId, serial: Serial) -> Result<(), BindSerialError> {
        if !self.entities.contains(entity) {
            return Err(BindSerialError::NoSuchEntity(entity));
        }
        match self.serial_to_entity.get(&serial) {
            Some(&holder) if holder == entity => return Ok(()),
            Some(&holder) => return Err(BindSerialError::SerialTaken { serial, holder }),
            None => {}
        }
        if let Some(&existing) = self.entity_to_serial.get(&entity) {
            return Err(BindSerialError::AlreadyBound { entity, existing });
        }
        self.serials.reserve(serial);
        self.serial_to_entity.insert(serial, entity);
        self.entity_to_serial.insert(entity, serial);
        Ok(())
    }

    /// Reserve a serial that belongs to something not currently spawned.
    ///
    /// The counterpart to [`bind_serial`](Self::bind_serial) for a load that does
    /// not spawn everything it reads: a logged-out character lives in the database
    /// rather than as an entity, but its serial was on the wire in every packet it
    /// was ever in and must not be handed to someone else before it is next
    /// played. This bumps the allocator past it so a fresh spawn never collides,
    /// with no entity to hang it on. Binding it to an entity later still succeeds.
    pub fn reserve_serial(&mut self, serial: Serial) {
        self.serials.reserve(serial);
    }

    /// The wire serial of `entity`, if it has one.
    #[inline]
    pub fn serial_of(&self, entity: EntityId) -> Option<Serial> {
        self.entity_to_serial.get(&entity).copied()
    }

    /// Resolve a serial off the wire to a live entity.
    ///
    /// This is the hot path for nearly every incoming packet.
    #[inline]
    pub fn entity_of(&self, serial: Serial) -> Option<EntityId> {
        self.serial_to_entity.get(&serial).copied()
    }

    /// Borrow the serial allocator, e.g. to inspect watermarks before a save.
    #[inline]
    pub const fn serial_allocator(&self) -> &SerialAllocator {
        &self.serials
    }

    // -- queries ----------------------------------------------------------

    /// Every `(entity, &T)`.
    ///
    /// Yields nothing if no entity has ever had a `T`; an unknown component is
    /// an empty query, not an error.
    pub fn query<T: Component>(&self) -> Query<'_, T> {
        self.column::<T>()
            .map(SparseSet::iter)
            .into_iter()
            .flatten()
    }

    /// Every `(entity, &mut T)`.
    pub fn query_mut<T: Component>(&mut self) -> QueryMut<'_, T> {
        self.column_mut::<T>()
            .map(SparseSet::iter_mut)
            .into_iter()
            .flatten()
    }

    /// Every entity that has both `A` and `B`, with both borrowed immutably.
    ///
    /// Unlike [`Registry::for_each2_mut`] this needs no column splitting, since
    /// overlapping shared borrows are fine — `A` and `B` may even be the same
    /// type.
    pub fn query2<A: Component, B: Component>(
        &self,
    ) -> impl Iterator<Item = (EntityId, &A, &B)> + '_ {
        let b = self.column::<B>();
        self.query::<A>()
            .filter_map(move |(entity, a)| Some((entity, a, b?.get(entity)?)))
    }

    /// Run `f` over every entity that has both `A` and `B`, with `A` mutable.
    ///
    /// This is a closure rather than an iterator because handing out `&mut A`
    /// and `&B` from two columns of the same registry is exactly the borrow the
    /// compiler cannot verify across a return. Keeping the split internal means
    /// the two columns can be proven disjoint, and so no unsafe code.
    ///
    /// # Panics
    /// If `A` and `B` are the same type, which would alias one column.
    pub fn for_each2_mut<A, B, F>(&mut self, mut f: F)
    where
        A: Component,
        B: Component,
        F: FnMut(EntityId, &mut A, &B),
    {
        let type_a = TypeId::of::<A>();
        let type_b = TypeId::of::<B>();
        assert_ne!(
            type_a, type_b,
            "for_each2_mut needs two distinct component types"
        );

        let (Some(&index_a), Some(&index_b)) = (
            self.column_index.get(&type_a),
            self.column_index.get(&type_b),
        ) else {
            // One of the components has never been inserted, so no entity can
            // have both.
            return;
        };

        let (column_a, column_b) = split_two(&mut self.columns, index_a, index_b);
        let set_a = column_a
            .as_any_mut()
            .downcast_mut::<SparseSet<A>>()
            .expect("column registered under a mismatched TypeId");
        let set_b = column_b
            .as_any()
            .downcast_ref::<SparseSet<B>>()
            .expect("column registered under a mismatched TypeId");

        // Iterate the mutable side and probe the other, so cost scales with the
        // smaller concern rather than the whole world.
        for (entity, a) in set_a.iter_mut() {
            if let Some(b) = set_b.get(entity) {
                f(entity, a, b);
            }
        }
    }

    // -- column plumbing --------------------------------------------------

    fn column<T: Component>(&self) -> Option<&SparseSet<T>> {
        let index = *self.column_index.get(&TypeId::of::<T>())?;
        self.columns[index].as_any().downcast_ref::<SparseSet<T>>()
    }

    fn column_mut<T: Component>(&mut self) -> Option<&mut SparseSet<T>> {
        let index = *self.column_index.get(&TypeId::of::<T>())?;
        self.columns[index]
            .as_any_mut()
            .downcast_mut::<SparseSet<T>>()
    }

    fn column_or_insert<T: Component>(&mut self) -> &mut SparseSet<T> {
        let type_id = TypeId::of::<T>();
        let index = match self.column_index.get(&type_id) {
            Some(&index) => index,
            None => {
                let index = self.columns.len();
                self.columns.push(Box::new(SparseSet::<T>::new()));
                self.column_index.insert(type_id, index);
                index
            }
        };
        self.columns[index]
            .as_any_mut()
            .downcast_mut::<SparseSet<T>>()
            .expect("column registered under a mismatched TypeId")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(PartialEq, Debug)]
    struct Position {
        x: i32,
        y: i32,
    }

    #[derive(PartialEq, Debug)]
    struct Health(u32);

    #[derive(PartialEq, Debug)]
    struct Poisoned;

    #[test]
    fn spawn_insert_get() {
        let mut reg = Registry::new();
        let e = reg.spawn();
        assert!(reg.contains(e));
        assert_eq!(reg.len(), 1);

        assert_eq!(reg.insert(e, Position { x: 1, y: 2 }), None);
        assert_eq!(reg.insert(e, Health(100)), None);
        assert_eq!(reg.get::<Position>(e), Some(&Position { x: 1, y: 2 }));
        assert_eq!(reg.get::<Health>(e), Some(&Health(100)));
        assert!(reg.has::<Health>(e));
        assert!(!reg.has::<Poisoned>(e));

        reg.get_mut::<Health>(e).unwrap().0 = 50;
        assert_eq!(reg.get::<Health>(e), Some(&Health(50)));

        assert_eq!(reg.remove::<Health>(e), Some(Health(50)));
        assert!(!reg.has::<Health>(e));
    }

    #[test]
    fn despawn_clears_every_column() {
        let mut reg = Registry::new();
        let e = reg.spawn();
        reg.insert(e, Position { x: 1, y: 2 });
        reg.insert(e, Health(100));
        reg.insert(e, Poisoned);

        assert!(reg.despawn(e));
        assert!(!reg.contains(e));
        assert_eq!(reg.len(), 0);
        assert_eq!(reg.count::<Position>(), 0);
        assert_eq!(reg.count::<Health>(), 0);
        assert_eq!(reg.count::<Poisoned>(), 0);
        assert!(!reg.despawn(e), "double despawn is a no-op");
    }

    #[test]
    fn recycled_entity_starts_clean() {
        let mut reg = Registry::new();
        let old = reg.spawn();
        reg.insert(old, Health(100));
        reg.despawn(old);

        let fresh = reg.spawn();
        assert_eq!(fresh.index(), old.index(), "precondition: slot recycled");
        assert_eq!(reg.get::<Health>(fresh), None, "no inherited components");
        assert_eq!(reg.get::<Health>(old), None, "stale handle sees nothing");
    }

    #[test]
    fn stale_handles_cannot_write() {
        let mut reg = Registry::new();
        let dead = reg.spawn();
        reg.despawn(dead);

        assert_eq!(reg.insert(dead, Health(1)), None);
        assert_eq!(
            reg.count::<Health>(),
            0,
            "writing through a stale handle must not resurrect it"
        );
        assert_eq!(reg.remove::<Health>(dead), None);
    }

    #[test]
    fn spawn_with_serial_indexes_both_ways() {
        let mut reg = Registry::new();
        let (mobile, ms) = reg.spawn_with_serial(SerialKind::Mobile).unwrap();
        let (item, is) = reg.spawn_with_serial(SerialKind::Item).unwrap();

        assert!(ms.is_mobile());
        assert!(is.is_item());
        assert_eq!(reg.serial_of(mobile), Some(ms));
        assert_eq!(reg.entity_of(ms), Some(mobile));
        assert_eq!(reg.entity_of(is), Some(item));
        assert_ne!(ms, is);
    }

    #[test]
    fn despawn_releases_the_serial_mapping() {
        let mut reg = Registry::new();
        let (e, s) = reg.spawn_with_serial(SerialKind::Mobile).unwrap();
        reg.despawn(e);

        assert_eq!(reg.entity_of(s), None, "a dead serial resolves to nothing");
        assert_eq!(reg.serial_of(e), None);
    }

    #[test]
    fn serials_are_not_reused_after_despawn() {
        // A client packet in flight may still name the old serial; handing it to
        // a new object would let the client act on the wrong thing.
        let mut reg = Registry::new();
        let (e, first) = reg.spawn_with_serial(SerialKind::Item).unwrap();
        reg.despawn(e);
        let (_, second) = reg.spawn_with_serial(SerialKind::Item).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn bind_serial_reserves_against_the_allocator() {
        // The load path: serials come from the save, and freshly allocated ones
        // must not collide with them.
        let mut reg = Registry::new();
        let e = reg.spawn();
        let loaded = Serial::new(0x4000_0500).unwrap();
        assert_eq!(reg.bind_serial(e, loaded), Ok(()));
        assert_eq!(reg.entity_of(loaded), Some(e));

        let (_, fresh) = reg.spawn_with_serial(SerialKind::Item).unwrap();
        assert_eq!(fresh.raw(), 0x4000_0501);
    }

    #[test]
    fn reserve_serial_keeps_a_loaded_serial_out_of_the_allocator() {
        // The load-on-play path: a character read from the database is not
        // spawned until it is played, but its serial must be off-limits from
        // boot so a newly created character never takes it.
        let mut reg = Registry::new();
        let loaded = Serial::new(0x0000_0005).unwrap();
        reg.reserve_serial(loaded);

        let (_, fresh) = reg.spawn_with_serial(SerialKind::Mobile).unwrap();
        assert!(
            fresh.raw() > loaded.raw(),
            "a fresh spawn skips the reserved serial"
        );

        // And the reserved serial can still be bound when the character is played.
        let entity = reg.spawn();
        assert_eq!(reg.bind_serial(entity, loaded), Ok(()));
    }

    #[test]
    fn bind_serial_rejects_conflicts() {
        let mut reg = Registry::new();
        let a = reg.spawn();
        let b = reg.spawn();
        let s = Serial::new(0x1234).unwrap();
        let other = Serial::new(0x5678).unwrap();

        reg.bind_serial(a, s).unwrap();
        assert_eq!(
            reg.bind_serial(b, s),
            Err(BindSerialError::SerialTaken {
                serial: s,
                holder: a
            })
        );
        assert_eq!(
            reg.bind_serial(a, other),
            Err(BindSerialError::AlreadyBound {
                entity: a,
                existing: s
            })
        );
        assert_eq!(
            reg.bind_serial(a, s),
            Ok(()),
            "rebinding the same pair is idempotent"
        );

        let dead = reg.spawn();
        reg.despawn(dead);
        assert_eq!(
            reg.bind_serial(dead, other),
            Err(BindSerialError::NoSuchEntity(dead))
        );
    }

    #[test]
    fn query_visits_only_matching_entities() {
        let mut reg = Registry::new();
        let a = reg.spawn();
        let b = reg.spawn();
        let c = reg.spawn();
        reg.insert(a, Health(10));
        reg.insert(b, Health(20));
        reg.insert(c, Position { x: 0, y: 0 });

        let mut healths: Vec<u32> = reg.query::<Health>().map(|(_, h)| h.0).collect();
        healths.sort_unstable();
        assert_eq!(healths, vec![10, 20]);

        for (_, h) in reg.query_mut::<Health>() {
            h.0 += 1;
        }
        let mut healths: Vec<u32> = reg.query::<Health>().map(|(_, h)| h.0).collect();
        healths.sort_unstable();
        assert_eq!(healths, vec![11, 21]);
    }

    #[test]
    fn query_on_an_unknown_component_is_empty_not_a_panic() {
        let reg = Registry::new();
        assert_eq!(reg.query::<Health>().count(), 0);
        assert_eq!(reg.count::<Health>(), 0);
    }

    #[test]
    fn query2_intersects() {
        let mut reg = Registry::new();
        let both = reg.spawn();
        let only_health = reg.spawn();
        let only_pos = reg.spawn();
        reg.insert(both, Health(10));
        reg.insert(both, Position { x: 5, y: 5 });
        reg.insert(only_health, Health(20));
        reg.insert(only_pos, Position { x: 1, y: 1 });

        let hits: Vec<_> = reg.query2::<Health, Position>().map(|(e, ..)| e).collect();
        assert_eq!(hits, vec![both]);
    }

    #[test]
    fn for_each2_mut_writes_only_the_intersection() {
        let mut reg = Registry::new();
        let both = reg.spawn();
        let only_health = reg.spawn();
        reg.insert(both, Health(10));
        reg.insert(both, Position { x: 3, y: 0 });
        reg.insert(only_health, Health(10));

        let mut visited = 0;
        reg.for_each2_mut::<Health, Position, _>(|_, health, pos| {
            visited += 1;
            health.0 += pos.x as u32;
        });

        assert_eq!(visited, 1);
        assert_eq!(reg.get::<Health>(both), Some(&Health(13)));
        assert_eq!(reg.get::<Health>(only_health), Some(&Health(10)));
    }

    #[test]
    fn for_each2_mut_is_silent_when_a_column_is_missing() {
        let mut reg = Registry::new();
        let e = reg.spawn();
        reg.insert(e, Health(10));

        let mut visited = 0;
        reg.for_each2_mut::<Health, Position, _>(|_, _, _| visited += 1);
        assert_eq!(visited, 0);
    }

    #[test]
    #[should_panic(expected = "distinct component types")]
    fn for_each2_mut_rejects_aliasing_the_same_column() {
        let mut reg = Registry::new();
        let e = reg.spawn();
        reg.insert(e, Health(10));
        reg.for_each2_mut::<Health, Health, _>(|_, _, _| {});
    }

    #[test]
    fn registries_are_independent() {
        // No global state: two registries hand out the same ids and never see
        // each other's data.
        let mut a = Registry::new();
        let mut b = Registry::new();
        let ea = a.spawn();
        let eb = b.spawn();
        assert_eq!(ea, eb, "ids are registry-local, so they collide by design");

        a.insert(ea, Health(1));
        assert_eq!(a.get::<Health>(ea), Some(&Health(1)));
        assert_eq!(b.get::<Health>(eb), None);
    }
}
