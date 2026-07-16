//! Entity identity: generational indices.

use std::fmt;
use std::num::NonZeroU32;

/// A handle to an entity.
///
/// An `EntityId` is a generational index: the `index` is a slot that gets
/// recycled, and the `generation` distinguishes the current occupant of that
/// slot from every previous one. Holding a stale `EntityId` is safe — every
/// lookup validates the generation and returns `None` for dead entities.
///
/// This is an *internal* handle. It is never sent to a client; see
/// [`crate::Serial`] for the identity that appears on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId {
    index: u32,
    generation: NonZeroU32,
}

impl EntityId {
    pub(crate) const fn new(index: u32, generation: NonZeroU32) -> Self {
        Self { index, generation }
    }

    /// The recyclable slot this entity occupies.
    #[inline]
    pub const fn index(self) -> u32 {
        self.index
    }

    /// How many times this slot has been reused. Always non-zero.
    #[inline]
    pub const fn generation(self) -> u32 {
        self.generation.get()
    }

    /// Pack into a single `u64`, for storage in a save file or a script handle.
    #[inline]
    pub const fn to_bits(self) -> u64 {
        ((self.generation.get() as u64) << 32) | (self.index as u64)
    }

    /// Unpack a value produced by [`EntityId::to_bits`].
    ///
    /// Returns `None` if the generation is zero, which no live entity ever has.
    #[inline]
    pub const fn from_bits(bits: u64) -> Option<Self> {
        match NonZeroU32::new((bits >> 32) as u32) {
            Some(generation) => Some(Self {
                index: bits as u32,
                generation,
            }),
            None => None,
        }
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityId({}v{})", self.index, self.generation)
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

/// Hands out [`EntityId`]s and tracks which are alive.
#[derive(Debug, Default)]
pub(crate) struct EntityAllocator {
    /// Current generation of each slot. Parallel to `alive`.
    generations: Vec<NonZeroU32>,
    /// Whether each slot is currently occupied.
    alive: Vec<bool>,
    /// Slots that can be recycled.
    free: Vec<u32>,
    live_count: usize,
}

/// The first generation handed out for a slot.
const FIRST_GENERATION: NonZeroU32 = NonZeroU32::MIN;

impl EntityAllocator {
    pub(crate) fn alloc(&mut self) -> EntityId {
        self.live_count += 1;
        match self.free.pop() {
            Some(index) => {
                let slot = index as usize;
                self.alive[slot] = true;
                EntityId::new(index, self.generations[slot])
            }
            None => {
                let index = u32::try_from(self.generations.len())
                    .expect("OpenShard supports at most u32::MAX entity slots");
                self.generations.push(FIRST_GENERATION);
                self.alive.push(true);
                EntityId::new(index, FIRST_GENERATION)
            }
        }
    }

    /// Returns `true` if the entity was alive and is now freed.
    pub(crate) fn free(&mut self, entity: EntityId) -> bool {
        if !self.contains(entity) {
            return false;
        }
        let slot = entity.index() as usize;
        self.alive[slot] = false;
        self.live_count -= 1;
        // Bump the generation so the stale handle can never match again.
        // Skipping zero keeps the NonZeroU32 niche and the `from_bits` contract.
        let next = self.generations[slot].get().wrapping_add(1);
        self.generations[slot] = NonZeroU32::new(next).unwrap_or(FIRST_GENERATION);
        self.free.push(entity.index());
        true
    }

    pub(crate) fn contains(&self, entity: EntityId) -> bool {
        let slot = entity.index() as usize;
        match self.alive.get(slot) {
            Some(true) => self.generations[slot] == entity.generation,
            _ => false,
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.live_count
    }

    /// Free every live slot at once.
    ///
    /// Generations are deliberately *not* reset. Wiping them would make the next
    /// `alloc` hand back an `EntityId` bit-identical to one issued before the
    /// clear, and every stale handle to it would spring back to life pointing at
    /// a different entity — the exact failure generational indices exist to
    /// prevent.
    pub(crate) fn clear(&mut self) {
        self.free.clear();
        for slot in 0..self.generations.len() {
            if self.alive[slot] {
                let next = self.generations[slot].get().wrapping_add(1);
                self.generations[slot] = NonZeroU32::new(next).unwrap_or(FIRST_GENERATION);
                self.alive[slot] = false;
            }
            self.free.push(slot as u32);
        }
        self.live_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_distinct_ids() {
        let mut alloc = EntityAllocator::default();
        let a = alloc.alloc();
        let b = alloc.alloc();
        assert_ne!(a, b);
        assert_eq!(alloc.len(), 2);
        assert!(alloc.contains(a));
        assert!(alloc.contains(b));
    }

    #[test]
    fn recycles_slot_but_bumps_generation() {
        let mut alloc = EntityAllocator::default();
        let a = alloc.alloc();
        assert!(alloc.free(a));
        assert!(!alloc.contains(a));
        assert_eq!(alloc.len(), 0);

        let b = alloc.alloc();
        assert_eq!(a.index(), b.index(), "slot should be recycled");
        assert_ne!(a.generation(), b.generation(), "generation must advance");
        assert!(alloc.contains(b));
        assert!(!alloc.contains(a), "stale handle must stay dead");
    }

    #[test]
    fn double_free_is_a_no_op() {
        let mut alloc = EntityAllocator::default();
        let a = alloc.alloc();
        assert!(alloc.free(a));
        assert!(!alloc.free(a));
        assert_eq!(alloc.len(), 0);
    }

    #[test]
    fn clear_does_not_resurrect_stale_handles() {
        let mut alloc = EntityAllocator::default();
        let a = alloc.alloc();
        let b = alloc.alloc();

        alloc.clear();
        assert_eq!(alloc.len(), 0);
        assert!(!alloc.contains(a));
        assert!(!alloc.contains(b));

        // Every slot is reusable, but no handle from before the clear may match
        // whatever now occupies its slot.
        let fresh_one = alloc.alloc();
        let fresh_two = alloc.alloc();
        for stale in [a, b] {
            assert!(!alloc.contains(stale), "{stale:?} came back to life");
            assert_ne!(stale, fresh_one);
            assert_ne!(stale, fresh_two);
        }
        assert_eq!(alloc.len(), 2);
    }

    #[test]
    fn clear_reuses_every_slot_exactly_once() {
        let mut alloc = EntityAllocator::default();
        let originals: Vec<_> = (0..3).map(|_| alloc.alloc()).collect();
        alloc.free(originals[1]); // one slot already free before the clear
        alloc.clear();

        let mut slots: Vec<u32> = (0..3).map(|_| alloc.alloc().index()).collect();
        slots.sort_unstable();
        assert_eq!(slots, vec![0, 1, 2], "no slot leaked or duplicated");
    }

    #[test]
    fn bits_round_trip() {
        let mut alloc = EntityAllocator::default();
        let a = alloc.alloc();
        assert_eq!(EntityId::from_bits(a.to_bits()), Some(a));
        assert_eq!(EntityId::from_bits(0), None);
    }
}
