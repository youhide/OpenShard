//! UO serials: the identity an entity has *on the wire*.
//!
//! The Ultima Online protocol addresses everything by a 32-bit serial, and the
//! client infers an object's category from the numeric range it falls in.
//! Mobiles and items therefore come from two disjoint pools and cannot be
//! renumbered freely. This is a hard protocol constraint, not a Sphere-ism.

use std::fmt;

/// Lowest valid mobile serial. Zero is reserved.
pub const MOBILE_MIN: u32 = 0x0000_0001;
/// Highest valid mobile serial.
pub const MOBILE_MAX: u32 = 0x3FFF_FFFF;
/// Lowest valid item serial.
pub const ITEM_MIN: u32 = 0x4000_0000;
/// Highest valid item serial.
pub const ITEM_MAX: u32 = 0x7FFF_FFFF;

/// Which pool a [`Serial`] belongs to. The client derives this from the range.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum SerialKind {
    /// A mobile: player, NPC, or pet.
    Mobile,
    /// An item: anything not a mobile, including multis.
    Item,
}

/// A 32-bit object identity as understood by the UO client.
///
/// Construction is checked: a `Serial` always falls inside a valid pool, so
/// [`Serial::kind`] is total and never lies.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Serial(u32);

impl Serial {
    /// Build a serial from a raw wire value.
    ///
    /// Returns `None` for values outside both pools (`0`, and anything at or
    /// above `0x8000_0000`), which the client would not accept.
    #[inline]
    pub const fn new(raw: u32) -> Option<Self> {
        if raw >= MOBILE_MIN && raw <= ITEM_MAX {
            Some(Self(raw))
        } else {
            None
        }
    }

    /// The raw value to put on the wire.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Which pool this serial came from.
    #[inline]
    pub const fn kind(self) -> SerialKind {
        if self.0 <= MOBILE_MAX {
            SerialKind::Mobile
        } else {
            SerialKind::Item
        }
    }

    /// True if this serial addresses a mobile.
    #[inline]
    pub const fn is_mobile(self) -> bool {
        self.0 <= MOBILE_MAX
    }

    /// True if this serial addresses an item.
    #[inline]
    pub const fn is_item(self) -> bool {
        self.0 > MOBILE_MAX
    }
}

impl fmt::Debug for Serial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind() {
            SerialKind::Mobile => "Mobile",
            SerialKind::Item => "Item",
        };
        write!(f, "Serial({kind} 0x{:08X})", self.0)
    }
}

impl fmt::Display for Serial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:08X}", self.0)
    }
}

/// The serial pool for one kind is full.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SerialPoolExhausted(pub SerialKind);

impl fmt::Display for SerialPoolExhausted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {:?} serial pool is exhausted", self.0)
    }
}

impl std::error::Error for SerialPoolExhausted {}

/// Hands out fresh serials from each pool.
///
/// Allocation is a monotonic watermark per pool — freed serials are *not*
/// recycled. Reuse would let a client that is mid-packet-flight act on a
/// serial that now names a different object, which is a whole class of
/// duplication and desync bugs. Both pools are large enough that a shard would
/// have to churn a billion objects to run out.
///
/// When loading a save, call [`SerialAllocator::reserve`] for every serial read
/// back so the watermark never hands out a serial that is already in use.
#[derive(Clone, Debug)]
pub struct SerialAllocator {
    next_mobile: u32,
    next_item: u32,
}

impl Default for SerialAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl SerialAllocator {
    /// A fresh allocator with both pools at their lowest value.
    pub const fn new() -> Self {
        Self {
            next_mobile: MOBILE_MIN,
            next_item: ITEM_MIN,
        }
    }

    /// Take the next serial from `kind`'s pool.
    pub fn alloc(&mut self, kind: SerialKind) -> Result<Serial, SerialPoolExhausted> {
        let (next, max) = match kind {
            SerialKind::Mobile => (&mut self.next_mobile, MOBILE_MAX),
            SerialKind::Item => (&mut self.next_item, ITEM_MAX),
        };
        if *next > max {
            return Err(SerialPoolExhausted(kind));
        }
        let serial = Serial(*next);
        // Saturating so the exhausted pool stays exhausted rather than wrapping
        // back into the other pool's range.
        *next = next.saturating_add(1);
        Ok(serial)
    }

    /// Ensure `serial` will never be handed out by [`SerialAllocator::alloc`].
    ///
    /// Call this for every serial loaded from persistence.
    pub fn reserve(&mut self, serial: Serial) {
        let raw = serial.raw();
        let next = match serial.kind() {
            SerialKind::Mobile => &mut self.next_mobile,
            SerialKind::Item => &mut self.next_item,
        };
        if raw >= *next {
            *next = raw.saturating_add(1);
        }
    }

    /// The serial `alloc` would return next, or `None` if the pool is exhausted.
    pub const fn peek(&self, kind: SerialKind) -> Option<Serial> {
        let (next, max) = match kind {
            SerialKind::Mobile => (self.next_mobile, MOBILE_MAX),
            SerialKind::Item => (self.next_item, ITEM_MAX),
        };
        if next > max {
            // Must not fall through to `Serial::new`: an exhausted mobile pool
            // sits at ITEM_MIN, which is a perfectly valid *item* serial.
            None
        } else {
            Serial::new(next)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_decides_kind() {
        assert_eq!(Serial::new(MOBILE_MIN).unwrap().kind(), SerialKind::Mobile);
        assert_eq!(Serial::new(MOBILE_MAX).unwrap().kind(), SerialKind::Mobile);
        assert_eq!(Serial::new(ITEM_MIN).unwrap().kind(), SerialKind::Item);
        assert_eq!(Serial::new(ITEM_MAX).unwrap().kind(), SerialKind::Item);
    }

    #[test]
    fn rejects_out_of_range() {
        assert_eq!(Serial::new(0), None);
        assert_eq!(Serial::new(ITEM_MAX + 1), None);
        assert_eq!(Serial::new(u32::MAX), None, "0xFFFFFFFF means 'nothing'");
    }

    #[test]
    fn pools_are_independent_and_monotonic() {
        let mut alloc = SerialAllocator::new();
        let m1 = alloc.alloc(SerialKind::Mobile).unwrap();
        let i1 = alloc.alloc(SerialKind::Item).unwrap();
        let m2 = alloc.alloc(SerialKind::Mobile).unwrap();

        assert_eq!(m1.raw(), MOBILE_MIN);
        assert_eq!(m2.raw(), MOBILE_MIN + 1);
        assert_eq!(i1.raw(), ITEM_MIN);
        assert!(m1.is_mobile() && m2.is_mobile() && i1.is_item());
    }

    #[test]
    fn reserve_advances_watermark_past_loaded_serials() {
        let mut alloc = SerialAllocator::new();
        let loaded = Serial::new(0x0000_1000).unwrap();
        alloc.reserve(loaded);
        assert_eq!(alloc.alloc(SerialKind::Mobile).unwrap().raw(), 0x0000_1001);
        // Reserving something older must not rewind the watermark.
        alloc.reserve(Serial::new(0x0000_0005).unwrap());
        assert_eq!(alloc.alloc(SerialKind::Mobile).unwrap().raw(), 0x0000_1002);
    }

    #[test]
    fn reserve_only_touches_its_own_pool() {
        let mut alloc = SerialAllocator::new();
        alloc.reserve(Serial::new(ITEM_MAX).unwrap());
        assert_eq!(alloc.alloc(SerialKind::Mobile).unwrap().raw(), MOBILE_MIN);
        assert_eq!(
            alloc.alloc(SerialKind::Item),
            Err(SerialPoolExhausted(SerialKind::Item))
        );
    }

    #[test]
    fn exhausted_pool_never_bleeds_into_the_other() {
        let mut alloc = SerialAllocator::new();
        alloc.reserve(Serial::new(MOBILE_MAX).unwrap());
        assert_eq!(
            alloc.alloc(SerialKind::Mobile),
            Err(SerialPoolExhausted(SerialKind::Mobile))
        );
        // Still exhausted on a second attempt; must not wrap into item range.
        assert_eq!(
            alloc.alloc(SerialKind::Mobile),
            Err(SerialPoolExhausted(SerialKind::Mobile))
        );
        assert_eq!(alloc.alloc(SerialKind::Item).unwrap().raw(), ITEM_MIN);
    }
}
