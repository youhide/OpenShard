//! Semantic item and material identities.
//!
//! [`crate::wire::Graphic`] and [`crate::wire::Hue`] are client presentation
//! values.  These ids name the game definitions that presentation is projected
//! from; they are deliberately not aliases for either wire number.

/// A stable, shard-defined kind of item: a longsword, ingot, feather or key.
///
/// Values are allocated in the item-definition data and remain reserved when a
/// definition is retired.  They are never derived from legacy art or a table
/// position, so a save, script and future client view keep the same meaning
/// when presentation changes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ItemKindId(pub u32);

impl ItemKindId {
    /// The first valid definition id. Zero is reserved as an invalid/sentinel
    /// value at foreign-data boundaries and is never emitted by definitions.
    pub const FIRST: u32 = 1;

    /// Make an id supplied by validated definition data.
    #[must_use]
    pub const fn new(value: u32) -> Option<Self> {
        if value >= Self::FIRST {
            Some(Self(value))
        } else {
            None
        }
    }
}

/// A stable, shard-defined material grade: iron, valorite, oak or barbed
/// leather.  Absence of this value means that an item has no material axis.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct MaterialId(pub u16);

impl MaterialId {
    /// The first valid material id; zero is not a material.
    pub const FIRST: u16 = 1;

    /// Make an id supplied by validated material-definition data.
    #[must_use]
    pub const fn new(value: u16) -> Option<Self> {
        if value >= Self::FIRST {
            Some(Self(value))
        } else {
            None
        }
    }
}

/// A family of interchangeable material grades, for example metal or wood.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct MaterialFamilyId(pub u8);

impl MaterialFamilyId {
    /// Make a non-zero family id supplied by validated definition data.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value != 0 { Some(Self(value)) } else { None }
    }
}

/// A closed gameplay category used by recipe selectors and definition data.
///
/// This is intentionally not a stringly-typed tag bag: adding a new category
/// requires choosing its meaning at the protocol/script boundary.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum ItemTag {
    Ingot,
    Ore,
    Log,
    Weapon,
    Armor,
    Tool,
    Instrument,
    Container,
    Spellbook,
    Runebook,
}

/// A material constraint in a typed recipe input.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum MaterialRule {
    /// The item may carry any material, including no material where the recipe
    /// definition permits it.
    Any,
    /// The input must carry precisely this material grade.
    Exact(MaterialId),
    /// The input must carry the material selected from another input line.
    SameAsInput(u8),
    /// The input must carry some grade in this material family.
    InFamily(MaterialFamilyId),
}

/// A semantic recipe input selector.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum ItemSelector {
    /// One exact kind, regardless of material.
    Exact(ItemKindId),
    /// One kind constrained by a material rule.
    KindWithMaterial {
        kind: ItemKindId,
        material: MaterialRule,
    },
    /// Any kind carrying this closed semantic category.
    Tag(ItemTag),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_ids_reject_the_reserved_zero() {
        assert_eq!(ItemKindId::new(0), None);
        assert_eq!(MaterialId::new(0), None);
        assert_eq!(MaterialFamilyId::new(0), None);
        assert_eq!(ItemKindId::new(1), Some(ItemKindId(1)));
        assert_eq!(MaterialId::new(1), Some(MaterialId(1)));
    }

    #[test]
    fn a_selector_names_domain_identity_not_presentation() {
        let selector = ItemSelector::KindWithMaterial {
            kind: ItemKindId(1),
            material: MaterialRule::Exact(MaterialId(9)),
        };
        assert!(matches!(selector, ItemSelector::KindWithMaterial { .. }));
    }
}
