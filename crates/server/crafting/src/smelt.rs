//! Ore into ingots.
//!
//! ServUO's `BaseOre.OnDoubleClick`, and the step without which the whole of
//! Blacksmithy is unreachable from Mining: a miner is paid in **ore**, and every
//! smithing recipe eats **ingots**. Nothing else in the engine turns one into the
//! other.
//!
//! **One deliberate difference.** ServUO raises a target cursor and asks the
//! player to click the forge, for two reasons — to pick which of several forges,
//! and to allow clicking another pile of ore to combine the two. Neither applies
//! here: [`environment::around`](crate::environment::around) already answers "is
//! there a forge within reach" as one predicate, and identical piles merge on
//! their own through `items`' stacking. So a double-click at a forge smelts, and
//! a double-click anywhere else says so.

use openshard_entities::EntityId;
use openshard_protocol::wire::{ClilocId, Graphic};
use openshard_skills::{roll_skill_band, skill_value};
use openshard_state::components::{Drawn, Stackable};
use openshard_state::harvest::{ORE_GRAPHIC, ORES};
use openshard_state::{Skill, WorldState};

use crate::environment;
use crate::system::Needs;

/// The art an ingot takes — ServUO's `BaseIngot`, one graphic for all nine
/// metals, told apart by hue exactly as the ore is.
pub const INGOT_GRAPHIC: Graphic = Graphic(0x1BF2);

/// How many ingots one unit of the large ore pile yields. ServUO's
/// `ingotAmount = toConsume * 2` for art `0x19B9`, which is the only ore art this
/// engine produces.
const INGOTS_PER_ORE: u32 = 2;

/// The most ore one smelt will take, ServUO's own cap.
const MAX_PER_SMELT: u16 = 30_000;

/// "You must be near a forge to smelt ore." — ServUO says this by simply not
/// accepting the target; a shard that answers nothing at all reads as broken, so
/// the nearest stock line is used.
const NO_FORGE: ClilocId = ClilocId(1_044_267);
/// "You have no idea how to smelt this strange ore!"
const TOO_STRANGE: ClilocId = ClilocId(501_986);
/// "There is not enough metal-bearing ore in this pile to make an ingot."
const NOT_ENOUGH: ClilocId = ClilocId(501_987);
/// "You smelt the ore removing the impurities and put the metal in your backpack."
const SMELTED: ClilocId = ClilocId(501_988);
/// "You burn away the impurities but are left with less useable metal."
const BURNED: ClilocId = ClilocId(501_990);

/// How hard each metal is to smelt, in tenths — ServUO's `difficulty` switch,
/// indexed like [`ORES`].
///
/// Not the same numbers as the mining band: finding valorite and *purifying* it
/// are different problems, and ServUO gives them different curves. The band is
/// twenty-five points either side of the difficulty.
const DIFFICULTY: [i32; 9] = [500, 650, 700, 750, 800, 850, 900, 950, 990];

/// The band either side of a metal's difficulty, in tenths.
const BAND: i32 = 250;

/// Smelt a pile of ore, or say why not. Returns whether the item was ore at all.
pub fn smelt(state: &mut WorldState, smelter: EntityId, ore: EntityId) -> bool {
    let Some(graphic) = state.registry.get::<Drawn>(ore).copied() else {
        return false;
    };
    if graphic.id != ORE_GRAPHIC {
        return false;
    }
    let Some(metal) = ORES.iter().position(|row| row.hue == graphic.hue) else {
        // A pile at a hue no metal claims is somebody's decoration, not ore.
        return false;
    };
    let needs = Needs {
        forge: true,
        ..Needs::none()
    };
    if !environment::around(state, smelter).satisfy(needs) {
        state.localized_message(smelter, NO_FORGE, "");
        return true;
    }

    let difficulty = DIFFICULTY[metal];
    // The flat gate, and it is not the same question as the roll: a metal beyond
    // you is not a hard smelt, it is one you have never been taught.
    if difficulty > DIFFICULTY[0] && difficulty > i32::from(skill_value(state, smelter, Skill::Mining)) {
        state.localized_message(smelter, TOO_STRANGE, "");
        return true;
    }

    let held = openshard_items::amount_of(state, ore);
    if held == 0 {
        state.localized_message(smelter, NOT_ENOUGH, "");
        return true;
    }
    let taking = held.min(MAX_PER_SMELT);

    if !roll_skill_band(
        state,
        smelter,
        Skill::Mining,
        openshard_skills::SkillBand::new(difficulty - BAND, difficulty + BAND),
    ) {
        // A botched smelt burns half the pile away. ServUO swaps to a smaller ore
        // art when a single unit is left; this engine has one ore art (see
        // `harvest`'s note on `RandomSize`), so a last unit is simply gone.
        let left = held / 2;
        if let Some(serial) = state.registry.serial_of(ore) {
            openshard_items::consume(state, serial, held - left);
        }
        state.localized_message(smelter, BURNED, "");
        return true;
    }

    let Some(serial) = state.registry.serial_of(ore) else {
        return true;
    };
    let Some(pack) = state
        .registry
        .serial_of(smelter)
        .and_then(|mobile| openshard_items::backpack_of(state, mobile))
    else {
        return true;
    };
    openshard_items::consume(state, serial, taking);
    let ingots = u32::from(taking) * INGOTS_PER_ORE;
    let made = openshard_items::give(state, pack, INGOT_GRAPHIC, graphic.hue, ingots);
    if let Some(made) = made.last {
        // Ingots stack, and `give` only marks what it *creates* — a merge onto an
        // existing pile leaves the marker where it was.
        state.registry.insert(made, Stackable);
    }
    if made.is_complete() {
        state.localized_message(smelter, SMELTED, "");
    } else {
        state.system_message(
            smelter,
            &format!(
                "Only {} of {ingots} ingots could be placed in your pack.",
                made.given
            ),
        );
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_metal_has_a_difficulty() {
        assert_eq!(DIFFICULTY.len(), ORES.len());
    }

    #[test]
    fn the_difficulties_climb_with_the_metal() {
        // A softer metal that were harder to smelt would let a miner make
        // valorite ingots before iron ones.
        for pair in DIFFICULTY.windows(2) {
            assert!(pair[1] > pair[0]);
        }
    }

    #[test]
    fn iron_is_the_one_metal_anybody_can_smelt() {
        // ServUO's gate is `difficulty > 50.0 && …`, so iron alone falls through
        // it — which is what lets a character with no Mining at all turn the ore
        // they bought into something a smith can use.
        assert_eq!(DIFFICULTY[0], 500);
        assert!(DIFFICULTY[1..].iter().all(|d| *d > 500));
    }
}
