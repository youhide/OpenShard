//! Component storage.
//!
//! Each component type gets its own [`SparseSet`] column. Sparse sets give O(1)
//! insert/remove/lookup and a dense, cache-friendly array to iterate — which is
//! the shape a UO shard actually wants, since components are added and removed
//! constantly (an item picked up loses its world position, an NPC gains a combat
//! target) rather than being fixed at spawn.

use std::any::Any;
use std::{fmt, iter, slice};

use crate::entity::EntityId;

/// Iterator over `(entity, &component)` for one column.
pub type Iter<'a, T> = iter::Zip<iter::Copied<slice::Iter<'a, EntityId>>, slice::Iter<'a, T>>;

/// Iterator over `(entity, &mut component)` for one column.
pub type IterMut<'a, T> = iter::Zip<iter::Copied<slice::Iter<'a, EntityId>>, slice::IterMut<'a, T>>;

/// Iterator over the entities present in one column.
pub type Entities<'a> = iter::Copied<slice::Iter<'a, EntityId>>;

/// Anything that can be attached to an entity.
///
/// The blanket impl means you never write `impl Component for Foo` — any plain
/// data type that can cross threads is already a component. The `Send + Sync`
/// bound is what lets the simulation shard work across cores.
pub trait Component: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> Component for T {}

/// Type-erased view of a column, so [`crate::Registry`] can hold columns of
/// mixed component types and still clean up after a despawn.
pub(crate) trait Column: Send + Sync {
    fn remove_erased(&mut self, entity: EntityId) -> bool;
    fn clear_erased(&mut self);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Marks an unoccupied slot in [`SparseSet::sparse`].
const EMPTY: u32 = u32::MAX;

/// A column holding one component type, keyed by entity.
///
/// `sparse` maps an entity's slot index to a position in `dense_entities` /
/// `dense_data`, which are kept in lockstep. Iteration walks the dense arrays
/// and touches no empty slots.
pub struct SparseSet<T> {
    /// Entity slot index -> dense position, or [`EMPTY`].
    sparse: Vec<u32>,
    /// The entity owning each dense position, including its generation.
    dense_entities: Vec<EntityId>,
    /// The component values, parallel to `dense_entities`.
    dense_data: Vec<T>,
}

impl<T> Default for SparseSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for SparseSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SparseSet")
            .field("component", &std::any::type_name::<T>())
            .field("len", &self.dense_data.len())
            .finish()
    }
}

impl<T> SparseSet<T> {
    /// An empty column.
    pub const fn new() -> Self {
        Self {
            sparse: Vec::new(),
            dense_entities: Vec::new(),
            dense_data: Vec::new(),
        }
    }

    /// How many entities have this component.
    #[inline]
    pub fn len(&self) -> usize {
        self.dense_data.len()
    }

    /// True if no entity has this component.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.dense_data.is_empty()
    }

    /// The dense position of `entity`, or `None` if it has no component here.
    ///
    /// This is where staleness is caught: the sparse slot may still point at a
    /// live dense entry, but if the generation differs the handle refers to a
    /// dead entity that happened to occupy the same slot.
    #[inline]
    fn dense_index(&self, entity: EntityId) -> Option<usize> {
        let pos = *self.sparse.get(entity.index() as usize)?;
        if pos == EMPTY {
            return None;
        }
        let pos = pos as usize;
        if self.dense_entities[pos] == entity {
            Some(pos)
        } else {
            None
        }
    }

    /// Whether `entity` has this component.
    #[inline]
    pub fn contains(&self, entity: EntityId) -> bool {
        self.dense_index(entity).is_some()
    }

    /// Borrow `entity`'s component.
    #[inline]
    pub fn get(&self, entity: EntityId) -> Option<&T> {
        self.dense_index(entity).map(|i| &self.dense_data[i])
    }

    /// Mutably borrow `entity`'s component.
    #[inline]
    pub fn get_mut(&mut self, entity: EntityId) -> Option<&mut T> {
        self.dense_index(entity).map(|i| &mut self.dense_data[i])
    }

    /// Attach `value` to `entity`, returning the previous value if it had one.
    pub fn insert(&mut self, entity: EntityId, value: T) -> Option<T> {
        let slot = entity.index() as usize;
        if slot >= self.sparse.len() {
            self.sparse.resize(slot + 1, EMPTY);
        }

        let existing = self.sparse[slot];
        if existing != EMPTY {
            let pos = existing as usize;
            if self.dense_entities[pos] == entity {
                return Some(std::mem::replace(&mut self.dense_data[pos], value));
            }
            // The slot still points at a dead entity of an earlier generation
            // whose component was never cleaned up. Take over its dense
            // position instead of pushing, which would orphan the old entry:
            // unreachable through `sparse`, yet still visible to `iter`.
            self.dense_entities[pos] = entity;
            self.dense_data[pos] = value;
            return None;
        }

        let pos = self.dense_data.len();
        // `u32::MAX` is EMPTY, so it is not a usable dense position.
        assert!(
            pos < EMPTY as usize,
            "a component column cannot hold more than u32::MAX - 1 entries"
        );
        self.sparse[slot] = pos as u32;
        self.dense_entities.push(entity);
        self.dense_data.push(value);
        None
    }

    /// Detach `entity`'s component and return it.
    pub fn remove(&mut self, entity: EntityId) -> Option<T> {
        let pos = self.dense_index(entity)?;
        let slot = entity.index() as usize;
        self.sparse[slot] = EMPTY;

        // swap_remove keeps the dense arrays packed; the element that moved into
        // `pos` needs its sparse pointer repaired.
        self.dense_entities.swap_remove(pos);
        let value = self.dense_data.swap_remove(pos);
        if pos < self.dense_entities.len() {
            let moved = self.dense_entities[pos];
            self.sparse[moved.index() as usize] = pos as u32;
        }
        Some(value)
    }

    /// Drop every component in this column.
    pub fn clear(&mut self) {
        self.sparse.clear();
        self.dense_entities.clear();
        self.dense_data.clear();
    }

    /// Every `(entity, &component)` pair, in dense order.
    pub fn iter(&self) -> Iter<'_, T> {
        self.dense_entities
            .iter()
            .copied()
            .zip(self.dense_data.iter())
    }

    /// Every `(entity, &mut component)` pair, in dense order.
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        // Splitting the borrow across two fields is what lets the entity keys
        // stay readable while the values are handed out mutably.
        self.dense_entities
            .iter()
            .copied()
            .zip(self.dense_data.iter_mut())
    }

    /// The entities in this column, in dense order.
    pub fn entities(&self) -> Entities<'_> {
        self.dense_entities.iter().copied()
    }
}

impl<T: Component> Column for SparseSet<T> {
    fn remove_erased(&mut self, entity: EntityId) -> bool {
        self.remove(entity).is_some()
    }

    fn clear_erased(&mut self) {
        self.clear();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Mutably borrow two distinct slots of a slice at once.
///
/// The registry needs `&mut` to one column and `&` to another to run a join.
/// Splitting the backing slice is how that is expressed without unsafe code —
/// the borrow checker can see the two halves are disjoint.
///
/// # Panics
/// If `i == j`.
pub(crate) fn split_two<T>(slice: &mut [T], i: usize, j: usize) -> (&mut T, &mut T) {
    assert_ne!(i, j, "split_two requires two distinct indices");
    if i < j {
        let (left, right) = slice.split_at_mut(j);
        (&mut left[i], &mut right[0])
    } else {
        let (left, right) = slice.split_at_mut(i);
        (&mut right[0], &mut left[j])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityAllocator;

    #[test]
    fn insert_get_remove() {
        let mut alloc = EntityAllocator::default();
        let e = alloc.alloc();
        let mut set: SparseSet<u32> = SparseSet::new();

        assert!(set.is_empty());
        assert_eq!(set.insert(e, 7), None);
        assert_eq!(set.get(e), Some(&7));
        assert_eq!(set.len(), 1);

        assert_eq!(set.insert(e, 9), Some(7), "insert replaces and returns old");
        assert_eq!(set.len(), 1, "replacing must not grow the column");

        *set.get_mut(e).unwrap() += 1;
        assert_eq!(set.get(e), Some(&10));

        assert_eq!(set.remove(e), Some(10));
        assert_eq!(set.get(e), None);
        assert!(set.is_empty());
        assert_eq!(set.remove(e), None);
    }

    #[test]
    fn swap_remove_repairs_the_moved_entry() {
        let mut alloc = EntityAllocator::default();
        let a = alloc.alloc();
        let b = alloc.alloc();
        let c = alloc.alloc();
        let mut set: SparseSet<&str> = SparseSet::new();
        set.insert(a, "a");
        set.insert(b, "b");
        set.insert(c, "c");

        // Removing the first entry swaps `c` into position 0.
        assert_eq!(set.remove(a), Some("a"));
        assert_eq!(set.get(b), Some(&"b"));
        assert_eq!(set.get(c), Some(&"c"), "moved entry must still be findable");
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn stale_handle_does_not_read_a_recycled_slot() {
        let mut alloc = EntityAllocator::default();
        let old = alloc.alloc();
        let mut set: SparseSet<u32> = SparseSet::new();
        set.insert(old, 1);
        set.remove(old);
        alloc.free(old);

        let fresh = alloc.alloc();
        assert_eq!(fresh.index(), old.index(), "precondition: slot recycled");
        set.insert(fresh, 2);

        assert_eq!(set.get(fresh), Some(&2));
        assert_eq!(set.get(old), None, "stale generation must not alias");
        assert_eq!(set.remove(old), None);
        assert_eq!(
            set.get(fresh),
            Some(&2),
            "failed stale remove must not disturb the live entry"
        );
    }

    #[test]
    fn a_recycled_slot_inherits_nothing() {
        // Here the sparse slot still points at a *live* dense entry, so only the
        // generation check can tell the two entities apart.
        let mut alloc = EntityAllocator::default();
        let old = alloc.alloc();
        let mut set: SparseSet<u32> = SparseSet::new();
        set.insert(old, 1);
        alloc.free(old);
        let fresh = alloc.alloc();
        assert_eq!(fresh.index(), old.index(), "precondition: slot recycled");

        // A bare column does not observe frees — clearing components on despawn
        // is `Registry`'s job, so `old`'s data is legitimately still here.
        assert_eq!(set.get(old), Some(&1));
        // What must never happen is the new occupant seeing it.
        assert_eq!(
            set.get(fresh),
            None,
            "the new entity inherits no components"
        );
    }

    #[test]
    fn insert_over_a_stale_entry_reuses_its_slot() {
        let mut alloc = EntityAllocator::default();
        let old = alloc.alloc();
        let other = alloc.alloc();
        let mut set: SparseSet<u32> = SparseSet::new();
        set.insert(old, 1);
        set.insert(other, 2);
        alloc.free(old);
        let fresh = alloc.alloc();

        // No prior remove: `sparse[0]` still points at `old`'s dense entry.
        assert_eq!(
            set.insert(fresh, 3),
            None,
            "previous value belonged to another entity"
        );
        assert_eq!(set.len(), 2, "the dead entry is taken over, not orphaned");
        assert_eq!(set.get(fresh), Some(&3));
        assert_eq!(set.get(other), Some(&2), "the untouched entry survives");

        let entities: Vec<_> = set.entities().collect();
        assert!(
            !entities.contains(&old),
            "no dead entity may survive in the dense array"
        );
    }

    #[test]
    fn iteration_visits_every_entry() {
        let mut alloc = EntityAllocator::default();
        let ids: Vec<_> = (0..5).map(|_| alloc.alloc()).collect();
        let mut set: SparseSet<u32> = SparseSet::new();
        for (i, id) in ids.iter().enumerate() {
            set.insert(*id, i as u32);
        }

        let mut seen: Vec<u32> = set.iter().map(|(_, v)| *v).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);

        for (_, v) in set.iter_mut() {
            *v *= 10;
        }
        let mut seen: Vec<u32> = set.iter().map(|(_, v)| *v).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 10, 20, 30, 40]);
    }

    #[test]
    fn sparse_grows_for_high_slot_indices() {
        let mut alloc = EntityAllocator::default();
        let ids: Vec<_> = (0..100).map(|_| alloc.alloc()).collect();
        let mut set: SparseSet<u32> = SparseSet::new();
        // Insert only the last one; sparse must grow to cover slot 99.
        set.insert(ids[99], 42);
        assert_eq!(set.get(ids[99]), Some(&42));
        assert_eq!(set.get(ids[0]), None);
    }

    #[test]
    fn split_two_yields_disjoint_borrows() {
        let mut v = vec![1, 2, 3];
        let (a, b) = split_two(&mut v, 0, 2);
        *a += 10;
        *b += 10;
        assert_eq!(v, vec![11, 2, 13]);

        let (a, b) = split_two(&mut v, 2, 0);
        assert_eq!((*a, *b), (13, 11), "reversed indices keep argument order");
    }

    #[test]
    #[should_panic(expected = "distinct indices")]
    fn split_two_rejects_aliasing() {
        let mut v = vec![1, 2, 3];
        let _ = split_two(&mut v, 1, 1);
    }
}
