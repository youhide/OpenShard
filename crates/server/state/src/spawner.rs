//! Identity of a spawn region within one world's spawner list.

use serde::{Deserialize, Serialize};

/// A spawn region's slot in the world's spawner list.
///
/// The slot is assigned by the world when the region is registered. It is
/// intentionally distinct from every other numeric identifier: [`SpawnedBy`]
/// tags on mobiles and persisted spawner records use this namespace, whose
/// first valid value is zero.
///
/// [`SpawnedBy`]: crate::components::SpawnedBy
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpawnerId(pub u32);

impl SpawnerId {
    /// The placeholder used before a world assigns the spawner's actual slot.
    pub const PLACEHOLDER: Self = Self(0);

    /// Convert this identifier into the index it names.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[cfg(test)]
mod tests {
    use super::SpawnerId;

    #[test]
    fn it_keeps_the_existing_integer_save_format() {
        assert_eq!(serde_json::to_string(&SpawnerId(42)).unwrap(), "42");
        assert_eq!(serde_json::from_str::<SpawnerId>("42").unwrap(), SpawnerId(42));
    }
}
