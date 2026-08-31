//! The world's monotonically increasing simulation clock.
//!
//! A `WorldTick` names an instant in this world's lifetime.  It is deliberately
//! distinct from a `u64` duration: adding a duration produces another instant,
//! while subtracting two instants produces the elapsed number of ticks.

use std::fmt;
use std::ops::{
    Add,
    AddAssign,
    Sub,
};

/// An absolute instant on the deterministic world clock.
///
/// The counter is process-local: persistence stores relative time where a
/// timer has to survive a restart, and uses [`Self::raw`] only at wire/save
/// boundaries that genuinely contain the clock value.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WorldTick(u64);

impl WorldTick {
    /// The first instant of a newly booted world.
    pub const ZERO: Self = Self(0);
    /// A sentinel later than every real tick.
    pub const MAX: Self = Self(u64::MAX);

    /// Rebuild an instant read from a persistence boundary.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// The primitive representation for a persistence or protocol boundary.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Move forward by a duration, clamping at the final representable instant.
    #[must_use]
    pub const fn saturating_add(self, ticks: u64) -> Self {
        Self(self.0.saturating_add(ticks))
    }

    /// The elapsed duration since `earlier`, or zero when it lies in the future.
    #[must_use]
    pub const fn saturating_sub(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }

    /// Whether this instant falls on a duration cadence.
    #[must_use]
    pub const fn is_multiple_of(self, cadence: u64) -> bool {
        self.0.is_multiple_of(cadence)
    }
}

impl Add<u64> for WorldTick {
    type Output = Self;

    fn add(self, ticks: u64) -> Self {
        Self(self.0 + ticks)
    }
}

impl AddAssign<u64> for WorldTick {
    fn add_assign(&mut self, ticks: u64) {
        self.0 += ticks;
    }
}

impl Sub for WorldTick {
    type Output = u64;

    fn sub(self, other: Self) -> u64 {
        self.0 - other.0
    }
}

impl fmt::Display for WorldTick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
