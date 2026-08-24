//! Handing out serials.
//!
//! The [`Serial`] type itself is a wire concept and lives in
//! `openshard_protocol`; what belongs here is the server-side policy of *who
//! gets which number*, which the client never sees and the protocol does not
//! constrain beyond the pool boundaries.

use std::fmt;

use openshard_protocol::serial::{ITEM_MAX, ITEM_MIN, MOBILE_MAX, MOBILE_MIN, Serial, SerialKind};

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
    pub const fn alloc(&mut self, kind: SerialKind) -> Result<Serial, SerialPoolExhausted> {
        let (next, max) = match kind {
            SerialKind::Mobile => (&mut self.next_mobile, MOBILE_MAX),
            SerialKind::Item => (&mut self.next_item, ITEM_MAX),
        };
        if *next > max {
            return Err(SerialPoolExhausted(kind));
        }
        // The watermark is inside the pool, so this is a serial by construction.
        let serial = Serial::new(*next).unwrap();
        // Saturating so the exhausted pool stays exhausted rather than wrapping
        // back into the other pool's range.
        *next = next.saturating_add(1);
        Ok(serial)
    }

    /// Ensure `serial` will never be handed out by [`SerialAllocator::alloc`].
    ///
    /// Call this for every serial loaded from persistence.
    pub const fn reserve(&mut self, serial: Serial) {
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
