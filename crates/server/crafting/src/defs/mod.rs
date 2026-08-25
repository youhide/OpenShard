//! The five trades: their recipe lists, and the headers that name them.
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
//! and a bad row is a build failure naming it — see `docs/unenforced.md` S2.
//!
//! [`data/craft_systems.json`]: ../../data/craft_systems.json

pub mod alchemy;
pub mod blacksmithy;
pub mod carpentry;
pub mod tailoring;
pub mod tinkering;

use openshard_protocol::wire::SoundId;
use openshard_state::{Skill, TICKS_PER_SECOND};

use crate::system::{CraftSystemDef, Eca, Needs, SystemId, Text};

include!(concat!(env!("OUT_DIR"), "/systems.rs"));

/// One system by id.
#[must_use]
pub fn system(id: SystemId) -> Option<&'static CraftSystemDef> {
    SYSTEMS.get(id.index())
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_protocol::wire::{Graphic, Hue};

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
        // The vendors already stock all five trades' tools. A graphic in the tool
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
}
