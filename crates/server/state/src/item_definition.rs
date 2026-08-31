//! The shard's semantic item and material definitions.
//!
//! This is the only place that translates the durable game identities
//! `ItemKindId + MaterialId` into classic client art.  Gameplay asks for a
//! definition by id; old saves and still-unmigrated constructors use
//! [`kind_from_drawn`] only at that compatibility boundary.

use openshard_protocol::item_kind::{
    ItemKindId,
    ItemSelector,
    ItemTag,
    MaterialFamilyId,
    MaterialId,
    MaterialRule,
};
use openshard_protocol::wire::{
    Graphic,
    Hue,
};

use crate::Drawn;

/// Metal grades: iron through valorite.
pub const METAL: MaterialFamilyId = MaterialFamilyId(1);
/// Regular and special woods.
pub const WOOD: MaterialFamilyId = MaterialFamilyId(2);
/// Regular, spined, horned and barbed leather.
pub const LEATHER: MaterialFamilyId = MaterialFamilyId(3);

/// One semantic item definition and its classic presentation base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemDefinition {
    /// Stable identity written to saves and selected by recipes.
    pub id:              ItemKindId,
    /// Developer-readable name until localization is moved behind this table.
    pub name:            &'static str,
    /// The client tile for every presentation variant of this kind.
    pub graphic:         Graphic,
    /// Legacy client art aliases for this kind, such as a flipped tool. The
    /// canonical [`Self::graphic`] is what new semantic construction projects.
    pub legacy_graphics: &'static [Graphic],
    /// The gump that makes this semantic container usable. `None` means this
    /// kind is not a container.
    pub container_gump:  Option<Graphic>,
    /// Which material family may determine its hue, if any.
    pub material_family: Option<MaterialFamilyId>,
    /// Base protection for a piece of armour. This is a fact of its semantic
    /// kind; `None` says the item is not armour.
    pub armor_rating:    Option<u16>,
    /// Closed semantic categories this kind belongs to.
    pub tags:            &'static [ItemTag],
}

/// One material grade and how the classic client represents it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialDefinition {
    /// Stable identity written to saves and selected by recipes.
    pub id:          MaterialId,
    /// The family that may use this material.
    pub family:      MaterialFamilyId,
    /// Developer-readable name until localization is moved behind this table.
    pub name:        &'static str,
    /// The legacy hue projection for this material.
    pub hue:         Hue,
    /// The bonus a material lends a base armour rating. Zero for materials that
    /// do not modify armour; the value belongs to the material, not its hue.
    pub armor_bonus: u16,
}

include!(concat!(env!("OUT_DIR"), "/item_definitions.rs"));

/// Look up an item definition by its semantic id.
#[must_use]
pub fn item_definition(id: ItemKindId) -> Option<&'static ItemDefinition> {
    ITEM_DEFINITIONS.iter().find(|definition| definition.id == id)
}

/// Whether one semantic kind belongs to a closed definition tag.
#[must_use]
pub fn has_tag(id: ItemKindId, tag: ItemTag) -> bool {
    item_definition(id).is_some_and(|definition| definition.tags.contains(&tag))
}

/// Match a resolved item identity against a selector that needs no recipe-line
/// context. `SameAsInput` is deliberately left to the recipe evaluator, which
/// is the only layer that knows which earlier input slot it refers to.
#[must_use]
pub fn selector_matches(kind: ItemKindId, material: Option<MaterialId>, selector: ItemSelector) -> bool {
    match selector {
        ItemSelector::Exact(expected) => kind == expected,
        ItemSelector::KindWithMaterial {
            kind: expected,
            material: rule,
        } if kind == expected => {
            match rule {
                MaterialRule::Any => true,
                MaterialRule::Exact(expected) => material == Some(expected),
                MaterialRule::InFamily(family) => {
                    material_definition_opt(material).is_some_and(|definition| definition.family == family)
                }
                MaterialRule::SameAsInput(_) => false,
            }
        }
        ItemSelector::Tag(tag) => has_tag(kind, tag),
        _ => false,
    }
}

fn material_definition_opt(material: Option<MaterialId>) -> Option<&'static MaterialDefinition> {
    material.and_then(material_definition)
}

/// Look up a material definition by its semantic id.
#[must_use]
pub fn material_definition(id: MaterialId) -> Option<&'static MaterialDefinition> {
    MATERIAL_DEFINITIONS.iter().find(|definition| definition.id == id)
}

/// Resolve a legacy hue where a caller has not yet been migrated to a material
/// component. This remains a compatibility adapter; new gameplay uses
/// [`material_definition`] with a [`MaterialId`] directly.
#[must_use]
pub fn material_from_legacy_hue(hue: Hue) -> Option<MaterialId> {
    MATERIAL_DEFINITIONS
        .iter()
        .find(|definition| definition.hue == hue)
        .map(|definition| definition.id)
}

/// Resolve a legacy hue within one declared material family.
///
/// Classic `Hue::NONE` is legitimately shared by iron, regular wood and
/// regular leather, so new semantic adapters must use this family-qualified
/// form rather than guessing the first global hue match.
#[must_use]
pub fn material_from_legacy_hue_in_family(family: MaterialFamilyId, hue: Hue) -> Option<MaterialId> {
    MATERIAL_DEFINITIONS
        .iter()
        .find(|definition| definition.family == family && definition.hue == hue)
        .map(|definition| definition.id)
}

/// Project one semantic item identity to the classic item drawing.
///
/// A material is valid only when its family is the one the item definition
/// declares.  A material-less item uses its base `Hue::NONE`; callers cannot
/// invent an arbitrary hue for a semantic item.
#[must_use]
pub fn presentation_of(kind: ItemKindId, material: Option<MaterialId>) -> Option<Drawn> {
    let item = item_definition(kind)?;
    let hue = match (item.material_family, material) {
        (None, None) => Hue::NONE,
        (Some(family), Some(material)) if material_definition(material)?.family == family => {
            material_definition(material)?.hue
        }
        _ => return None,
    };
    Some(Drawn {
        id: item.graphic,
        hue,
    })
}

/// Interpret a legacy graphic/hue pair through the audited registry.
///
/// This is intentionally not a fallback that manufactures ids for unknown art:
/// a pair not named in the registry stays an explicit legacy item until its
/// migration row is added.
#[must_use]
pub fn kind_from_drawn(drawn: Drawn) -> Option<(ItemKindId, Option<MaterialId>)> {
    let item = ITEM_DEFINITIONS.iter().find(|definition| {
        definition.graphic == drawn.id || definition.legacy_graphics.contains(&drawn.id)
    })?;
    let material = match item.material_family {
        None if drawn.hue == Hue::NONE => None,
        Some(family) => {
            Some(
                MATERIAL_DEFINITIONS
                    .iter()
                    .find(|definition| definition.family == family && definition.hue == drawn.hue)?
                    .id,
            )
        }
        None => return None,
    };
    Some((item.id, material))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_projection_round_trips_without_a_hue_identity_lookup() {
        let longsword = ItemKindId(4);
        let valorite = MaterialId(9);
        let drawn = presentation_of(longsword, Some(valorite)).expect("valorite longsword");
        assert_eq!(
            drawn,
            Drawn {
                id:  Graphic(0x0F61),
                hue: Hue(0x08AB),
            }
        );
        assert_eq!(kind_from_drawn(drawn), Some((longsword, Some(valorite))));
    }

    #[test]
    fn incompatible_material_is_not_a_valid_presentation() {
        assert_eq!(presentation_of(ItemKindId(4), Some(MaterialId(20))), None);
        assert_eq!(presentation_of(ItemKindId(5), None), None);
    }

    #[test]
    fn a_shared_plain_hue_is_resolved_inside_its_material_family() {
        assert_eq!(
            material_from_legacy_hue_in_family(METAL, Hue::NONE),
            Some(MaterialId(1))
        );
        assert_eq!(
            material_from_legacy_hue_in_family(WOOD, Hue::NONE),
            Some(MaterialId(20))
        );
        assert_eq!(
            material_from_legacy_hue_in_family(LEATHER, Hue::NONE),
            Some(MaterialId(40))
        );
    }

    #[test]
    fn a_flipped_tool_art_is_an_alias_for_the_same_semantic_kind() {
        assert_eq!(
            kind_from_drawn(Drawn {
                id:  Graphic(0x0E85),
                hue: Hue(0x08AB),
            }),
            Some((ItemKindId(9), Some(MaterialId(9))))
        );
        assert_eq!(
            presentation_of(ItemKindId(9), Some(MaterialId(9))).map(|drawn| drawn.id),
            Some(Graphic(0x0E86)),
            "new typed construction uses the kind's canonical art"
        );
    }

    #[test]
    fn metal_tools_keep_their_material_as_part_of_their_identity() {
        assert_eq!(
            kind_from_drawn(Drawn {
                id:  Graphic(0x0FBB),
                hue: Hue(0x08AB),
            }),
            Some((ItemKindId(10), Some(MaterialId(9))))
        );
        assert_eq!(
            presentation_of(ItemKindId(10), Some(MaterialId(9))),
            Some(Drawn {
                id:  Graphic(0x0FBB),
                hue: Hue(0x08AB),
            })
        );
    }

    #[test]
    fn every_material_bearing_kind_recognizes_its_plain_f1_presentation() {
        for definition in ITEM_DEFINITIONS {
            let Some(family) = definition.material_family else {
                continue;
            };
            let material = match family {
                METAL => MaterialId(1),
                WOOD => MaterialId(20),
                LEATHER => MaterialId(40),
                _ => panic!("unmapped material family {}", family.0),
            };
            assert_eq!(
                kind_from_drawn(Drawn {
                    id:  definition.graphic,
                    hue: Hue::NONE,
                }),
                Some((definition.id, Some(material))),
                "{}",
                definition.name
            );
        }
    }

    #[test]
    fn every_valid_kind_material_projection_round_trips() {
        for definition in ITEM_DEFINITIONS {
            let Some(family) = definition.material_family else {
                continue;
            };
            for material in MATERIAL_DEFINITIONS
                .iter()
                .filter(|material| material.family == family)
            {
                let drawn = presentation_of(definition.id, Some(material.id))
                    .expect("definition and matching material family project");
                assert_eq!(
                    kind_from_drawn(drawn),
                    Some((definition.id, Some(material.id))),
                    "{} + {}",
                    definition.name,
                    material.name
                );
            }
        }
    }

    #[test]
    fn tags_belong_to_the_definition_not_the_client_art() {
        assert!(has_tag(ItemKindId(1), ItemTag::Ingot));
        assert!(has_tag(ItemKindId(4), ItemTag::Weapon));
        assert!(has_tag(ItemKindId(9), ItemTag::Tool));
        assert!(!has_tag(ItemKindId(4), ItemTag::Armor));
    }

    #[test]
    fn resolved_selectors_use_kind_material_and_tags() {
        assert!(selector_matches(
            ItemKindId(1),
            Some(MaterialId(9)),
            ItemSelector::KindWithMaterial {
                kind:     ItemKindId(1),
                material: MaterialRule::InFamily(METAL),
            }
        ));
        assert!(selector_matches(
            ItemKindId(1),
            Some(MaterialId(1)),
            ItemSelector::Tag(ItemTag::Ingot)
        ));
        assert!(!selector_matches(
            ItemKindId(1),
            Some(MaterialId(1)),
            ItemSelector::KindWithMaterial {
                kind:     ItemKindId(1),
                material: MaterialRule::Exact(MaterialId(9)),
            }
        ));
    }

    #[test]
    fn craft_axis_resources_have_typed_material_identity() {
        assert_eq!(
            kind_from_drawn(Drawn {
                id:  Graphic(0x1BD7),
                hue: Hue(0x07DA),
            }),
            Some((ItemKindId(36), Some(MaterialId(21))))
        );
        assert_eq!(
            kind_from_drawn(Drawn {
                id:  Graphic(0x1081),
                hue: Hue(0x0845),
            }),
            Some((ItemKindId(37), Some(MaterialId(42))))
        );
    }

    #[test]
    fn common_craft_ingredients_are_exact_kinds() {
        assert_eq!(
            kind_from_drawn(Drawn {
                id:  Graphic(0x1766),
                hue: Hue::NONE,
            }),
            Some((ItemKindId(38), None))
        );
        assert_eq!(
            kind_from_drawn(Drawn {
                id:  Graphic(0x0F0E),
                hue: Hue::NONE,
            }),
            Some((ItemKindId(39), None))
        );
    }
}
