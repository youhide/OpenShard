//! The seven trades: their recipe lists, and the headers that name them.
//!
//! Each `Def*.cs` in ServUO is a recipe table plus a short header of overrides —
//! the main skill, the chance floor, the exceptional curve, the sound, and what
//! `CanCraft` demands. Both halves are data here: the tables are the trades'
//! `data/*.json` and the headers are [`data/craft_systems.json`], and `build.rs`
//! turns the lot into the `const`s below before this crate compiles.
//!
//! The headers used to be hand-written Rust, which meant the build script could
//! see a recipe's group index but not how many groups its trade had, and a
//! recipe's skill lines but not which skill its system rolled. Both invariants
//! then lived in assertions in this file, green until somebody ran the tests
//! against a row that was already committed. They are checked in `build.rs` now,
//! and a bad row is a build failure naming it — see S2 in
//! `docs/server/evidence/2026-07-31-invariants-nothing-enforces.md`.
//!
//! [`data/craft_systems.json`]: ../../data/craft_systems.json

pub mod alchemy;
pub mod blacksmithy;
pub mod carpentry;
pub mod cooking;
pub mod fletching;
pub mod tailoring;
pub mod tinkering;

use openshard_protocol::wire::SoundId;
use openshard_state::{
    Skill,
    TICKS_PER_SECOND,
};

use crate::system::{
    CraftSystemDef,
    Eca,
    Needs,
    SystemId,
    Text,
};

include!(concat!(env!("OUT_DIR"), "/systems.rs"));

/// One system by id.
#[must_use]
pub fn system(id: SystemId) -> Option<&'static CraftSystemDef> {
    SYSTEMS.get(id.index())
}

#[cfg(test)]
mod tests {
    use openshard_protocol::item_kind::{
        ItemKindId,
        ItemSelector,
        MaterialRule,
    };
    use openshard_protocol::wire::{
        Graphic,
        Hue,
    };
    use openshard_state::item_definition::{
        LEATHER,
        METAL,
        WOOD,
    };
    use openshard_state::{
        AddonKind,
        item_definition as find_item_definition,
    };

    use super::*;

    #[test]
    fn every_addon_deed_keeps_its_installation_kind() {
        let addons: Vec<_> = carpentry::RECIPES
            .iter()
            .filter_map(|recipe| recipe.addon)
            .collect();
        assert_eq!(
            addons,
            vec![
                AddonKind::ElvenSpinningWheelEast,
                AddonKind::ElvenSpinningWheelSouth,
                AddonKind::SpinningWheelEast,
                AddonKind::SpinningWheelSouth,
                AddonKind::LoomEast,
                AddonKind::LoomSouth,
                AddonKind::StoneOvenEast,
                AddonKind::StoneOvenSouth,
                AddonKind::ElvenOvenSouth,
                AddonKind::ElvenOvenEast,
            ]
        );
    }

    /// An addon deed has exactly **one** row in its trade's table.
    ///
    /// Every addon deed draws the same generic scroll (`0x14F0`), so a second row
    /// with the same display name is invisible in the gump *except* as a
    /// duplicate line — and the untyped one of the pair crafts a scroll that
    /// installs nothing. That is not hypothetical: giving the two elven ovens
    /// their `kind` and `addon` added new rows beside the old ones instead of
    /// changing them, and both facings sat in the carpentry window twice, one
    /// working and one inert, until this test was written.
    #[test]
    fn no_addon_deed_is_offered_twice() {
        for system in SYSTEMS {
            for recipe in system.recipes {
                if recipe.addon.is_none() {
                    continue;
                }
                let same_name = system
                    .recipes
                    .iter()
                    .filter(|other| other.name == recipe.name)
                    .count();
                assert_eq!(
                    same_name, 1,
                    "{:?} names {same_name} rows in {:?}",
                    recipe.name, system.skill
                );
            }
        }
    }

    /// An addon recipe's output *is* the deed that installs it: a row naming one
    /// kind and installing another would craft a scroll that opens somebody
    /// else's oven, and both halves are hand-written data.
    #[test]
    fn an_addon_recipe_outputs_its_own_addon_s_deed() {
        for system in SYSTEMS {
            for recipe in system.recipes {
                let Some(addon) = recipe.addon else {
                    continue;
                };
                assert_eq!(
                    recipe.kind,
                    Some(addon.deed_kind()),
                    "{addon:?} is crafted as {:?}",
                    recipe.kind
                );
            }
        }
    }

    #[test]
    fn every_trade_has_exactly_one_system() {
        // `tool_system` finds a system by its main skill, so two systems sharing
        // one would make a tool open whichever came first — silently, and only
        // for one of the two trades.
        for (i, def) in SYSTEMS.iter().enumerate() {
            for other in &SYSTEMS[i + 1..] {
                assert_ne!(def.skill, other.skill, "{:?} appears twice", def.skill);
            }
        }
    }

    #[test]
    fn every_tool_on_the_shelf_opens_a_system() {
        // The vendors already stock every trade's tools. A graphic in the tool
        // table with no system behind it is a tool that answers a double-click
        // with nothing at all, which is what every one of these was before this
        // slice.
        for graphic in 0..=u16::MAX {
            let Some(tool) = openshard_state::craft::craft_tool(Graphic(graphic)) else {
                continue;
            };
            assert!(
                SYSTEMS.iter().any(|def| def.skill == tool.skill),
                "{graphic:#06X} names {:?}, which no system practises",
                tool.skill
            );
        }
    }

    #[test]
    fn every_registered_craft_tool_kind_opens_a_system() {
        // The semantic path is deliberately stricter than the legacy graphic
        // test above: a registry definition must not inherit a craft UI merely
        // because its presentation happens to resemble an old tool.
        for definition in openshard_state::item_definition::ITEM_DEFINITIONS {
            if openshard_state::craft::craft_tool_for_kind(definition.id).is_some() {
                assert!(
                    crate::tool_system_for_kind(definition.id).is_some(),
                    "registered craft tool {} ({}) opens no craft system",
                    definition.name,
                    definition.id.0
                );
            }
        }
    }

    // `every_recipe_names_a_group_that_exists` and
    // `every_recipe_leads_with_its_systems_own_skill` were here. They are
    // `build.rs::check` now — the same two assertions, a build earlier, and
    // deliberately not kept in both places: a check that lives twice drifts, and
    // the copy that is wrong is the one nobody is reading.

    #[test]
    fn the_material_axis_substitutes_into_a_line_that_wants_it() {
        // The axis is a hue swap onto one resource line. A system with an axis
        // whose recipes never name its graphic would offer a material picker that
        // changed nothing.
        for def in SYSTEMS {
            let Some(axis) = def.sub_res else { continue };
            assert!(!axis.entries.is_empty());
            assert_eq!(axis.entries[0].hue, Hue(0), "{:?}'s plain grade", def.skill);
            for entry in axis.entries {
                assert_eq!(
                    openshard_state::presentation_of(axis.item_kind, Some(entry.material)),
                    Some(openshard_state::Drawn {
                        id:  axis.graphic,
                        hue: entry.hue,
                    }),
                    "{:?}'s axis must declare the exact kind/material it renders",
                    def.skill,
                );
                assert_eq!(
                    openshard_state::material_definition(entry.material).map(|material| material.hue),
                    Some(entry.hue),
                    "{:?}'s material row {:?} has a mismatched display hue",
                    def.skill,
                    entry.material
                );
            }
            let uses_axis = def
                .recipes
                .iter()
                .filter(|recipe| recipe.resources.iter().any(|res| res.from_axis))
                .count();
            assert!(uses_axis > 0, "{:?} has an axis nothing reads", def.skill);
            for recipe in def.recipes {
                for res in recipe.resources {
                    assert_eq!(
                        res.from_axis,
                        res.graphic == axis.graphic,
                        "{:?} recipe {:#06X}",
                        def.skill,
                        recipe.graphic.0
                    );
                }
            }
        }
    }

    #[test]
    fn the_metals_are_the_same_hues_the_ground_yields() {
        // Mining pays in ore hues and a smith spends ingot hues; if the two
        // tables ever disagree, valorite ore smelts into an ingot no recipe can
        // find. Both come from ServUO's one `CraftResourceInfo` table, and this
        // is what says so.
        let axis = blacksmithy::SUB_RES;
        let ores = openshard_state::harvest::ORES;
        assert_eq!(axis.entries.len(), ores.len());
        for (entry, ore) in axis.entries.iter().zip(ores) {
            assert_eq!(entry.hue, ore.hue);
        }
    }

    #[test]
    fn migrated_smithing_rows_name_their_inputs_and_outputs_semantically() {
        let smithing = system(SystemId::new(0)).expect("blacksmithy");
        for kind in [ItemKindId(4), ItemKindId(5)] {
            let recipe = smithing
                .recipes
                .iter()
                .find(|recipe| recipe.kind == Some(kind))
                .expect("registered smithing recipe");
            assert_eq!(
                recipe.output_material,
                crate::recipe::OutputMaterial::InheritInput(0)
            );
            assert!(matches!(
                recipe.resources[0].selector,
                Some(ItemSelector::KindWithMaterial {
                    kind:     ItemKindId(1),
                    material: MaterialRule::Any,
                })
            ));
        }
    }

    #[test]
    fn every_typed_recipe_output_has_its_registered_base_projection() {
        for system in SYSTEMS {
            for recipe in system.recipes {
                let Some(kind) = recipe.kind else {
                    continue;
                };
                let definition = find_item_definition(kind).expect("typed recipe kind is registered");
                let material = match recipe.output_material {
                    crate::recipe::OutputMaterial::None => None,
                    crate::recipe::OutputMaterial::Fixed(material) => Some(material),
                    crate::recipe::OutputMaterial::InheritInput(_) => {
                        match definition.material_family {
                            Some(METAL) => Some(openshard_protocol::item_kind::MaterialId(1)),
                            Some(WOOD) => Some(openshard_protocol::item_kind::MaterialId(20)),
                            Some(LEATHER) => Some(openshard_protocol::item_kind::MaterialId(40)),
                            Some(family) => panic!("unmapped material family {}", family.0),
                            None => panic!("typed material output has no material family"),
                        }
                    }
                    crate::recipe::OutputMaterial::Legacy => {
                        panic!("typed recipe has legacy material policy")
                    }
                };
                assert_eq!(
                    openshard_state::presentation_of(kind, material),
                    Some(openshard_state::Drawn {
                        id:  recipe.graphic,
                        hue: recipe.hue,
                    }),
                    "{:?} recipe {:#06X}",
                    system.skill,
                    recipe.graphic.0
                );
            }
        }
    }

    #[test]
    fn a_tinkered_pickaxe_keeps_the_ingots_semantic_material() {
        let tinkering = system(SystemId::new(3)).expect("tinkering");
        let recipe = tinkering
            .recipes
            .iter()
            .find(|recipe| recipe.kind == Some(ItemKindId(9)))
            .expect("registered pickaxe recipe");
        assert_eq!(
            recipe.output_material,
            crate::recipe::OutputMaterial::InheritInput(0)
        );
        assert!(matches!(
            recipe.resources[0].selector,
            Some(ItemSelector::KindWithMaterial {
                kind:     ItemKindId(1),
                material: MaterialRule::Any,
            })
        ));
    }

    #[test]
    fn grapes_of_wrath_and_the_enchanted_apple_keep_cookings_raised_chance_floor() {
        // ServUO's `DefCooking.GetChanceAtMin` special-cases these two at 50%
        // although the trade itself starts every other recipe at 0%.
        let cooking = system(SystemId::new(6)).expect("cooking");
        for graphic in [0x2FD7, 0x2FD8] {
            let recipe = cooking
                .recipes
                .iter()
                .find(|recipe| recipe.graphic.0 == graphic)
                .unwrap_or_else(|| panic!("{graphic:#06X} recipe"));
            assert_eq!(recipe.min_chance, Some(500), "{graphic:#06X}");
        }
    }

    #[test]
    fn a_tinkered_pair_of_tongs_keeps_the_ingots_semantic_material() {
        let tinkering = system(SystemId::new(3)).expect("tinkering");
        let recipe = tinkering
            .recipes
            .iter()
            .find(|recipe| recipe.kind == Some(ItemKindId(10)))
            .expect("registered tongs recipe");
        assert_eq!(
            recipe.output_material,
            crate::recipe::OutputMaterial::InheritInput(0)
        );
        assert!(matches!(
            recipe.resources[0].selector,
            Some(ItemSelector::KindWithMaterial {
                kind:     ItemKindId(1),
                material: MaterialRule::Any,
            })
        ));
    }

    #[test]
    fn every_registered_craft_tool_recipe_has_a_typed_output() {
        for system in SYSTEMS {
            for recipe in system.recipes {
                let Some((kind, _)) = openshard_state::kind_from_drawn(openshard_state::Drawn {
                    id:  recipe.graphic,
                    hue: recipe.hue,
                }) else {
                    continue;
                };
                if openshard_state::craft::craft_tool_for_kind(kind).is_some() {
                    assert_eq!(
                        recipe.kind,
                        Some(kind),
                        "{:?} recipe {:#06X} creates registered craft tool {} without its ItemKindId",
                        system.skill,
                        recipe.graphic.0,
                        kind.0
                    );
                }
            }
        }
    }

    #[test]
    fn every_craftable_ranged_weapon_has_combat_rules() {
        // DefBowFletching also contains expansion bows and fukiya darts, but a
        // recipe must not ship before its output can actually attack. Group two
        // is the weapon page; the material and ammunition pages are not weapons.
        for recipe in fletching::RECIPES.iter().filter(|recipe| recipe.group == 2) {
            assert!(
                openshard_state::weapon::weapon_data(recipe.graphic).is_some(),
                "fletching recipe {:#06X} has no combat row",
                recipe.graphic.0
            );
        }
    }
}
