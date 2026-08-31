//! Whether it works, and whether it comes out well.
//!
//! ServUO's `CraftItem.GetSuccessChance` / `GetExceptionalChance` / `CheckSkills`,
//! in per-mille. Three things here are worth reading closely rather than
//! skimming, because each is a place a plausible simplification is wrong.
//!
//! **Failing the band and failing the roll are different refusals.** A crafter
//! below a recipe's `min` (less its offset) on *any* required skill has
//! `all_skills` false, which is chance zero — but it is answered with a different
//! line and it costs no materials, where a failed roll costs them. Folding the two
//! together would quietly eat the ingots of every player who clicked a recipe they
//! were not yet good enough for.
//!
//! **The exceptional draw is independent of the success draw**, and it is made
//! first. Two draws, always both spent, in a fixed order — otherwise the sequence
//! after a craft depends on whether the craft worked, and the tick stops
//! replaying.
//!
//! **The chance can be negative**, and is not clamped up. A recipe with a
//! `min_skill_offset` lets a crafter attempt something below the band's floor;
//! ServUO leaves that chance below zero so the roll can never pass, and the offset
//! is a licence to *try*, not a discount on the odds.

use openshard_entities::EntityId;
use openshard_skills::{
    roll_skill_band,
    skill_value,
};
use openshard_state::WorldState;

use crate::recipe::Recipe;
use crate::system::{
    CraftSystemDef,
    Eca,
};

/// The odds of one recipe for one crafter, in per-mille.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chance {
    /// The chance the craft succeeds at all.
    pub success:     u32,
    /// The chance it comes out exceptional. Read only on the success branch, but
    /// rolled either way.
    pub exceptional: u32,
    /// Whether every required skill clears its floor. False means the crafter
    /// cannot attempt this at all, which is a refusal and not a failure.
    pub all_skills:  bool,
}

/// What one attempt actually did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Roll {
    /// Whether it worked.
    pub success:     bool,
    /// Whether it came out exceptional. Meaningless unless `success`.
    pub exceptional: bool,
    /// Whether the crafter was allowed to try — see [`Chance::all_skills`].
    pub all_skills:  bool,
}

/// The odds, without touching the world — what the gump's detail page prints.
///
/// Deliberately separate from [`roll`]: ServUO's `GetSuccessChance` trains the
/// skills it reads when `gainSkills` is set, and a gump that is *drawn* must not
/// train anything. Splitting the two is how a player refreshing a window cannot
/// grind a skill by looking at it.
#[must_use]
pub fn chance(state: &WorldState, crafter: EntityId, system: &CraftSystemDef, recipe: &Recipe) -> Chance {
    let mut all_skills = true;
    let mut main = None;
    for want in recipe.skills {
        let floor = want.min - recipe.min_skill_offset;
        let value = i32::from(skill_value(state, crafter, want.skill));
        if value < floor {
            all_skills = false;
        }
        if want.skill == system.skill {
            main = Some((floor, want.max, value));
        }
    }
    let success = match (all_skills, main) {
        (true, Some((floor, max, value))) => interpolate(system.chance_at_min, floor, max, value),
        // No main-skill line at all is a malformed recipe rather than a hard
        // craft; refusing it is the safe reading.
        _ => 0,
    };
    Chance {
        success,
        exceptional: exceptional_chance(state, crafter, system, recipe, success),
        all_skills,
    }
}

/// Roll the attempt: train every required skill, then draw for quality and for
/// success.
///
/// The passive per-skill training is what makes a smith's Mining creep up while
/// they hammer — ServUO does it inside `GetSuccessChance`, once, for every listed
/// skill including the main one, and skips it entirely for a `use_all_res` recipe
/// (which trains once per item made instead, at the end).
pub(crate) fn roll(
    state: &mut WorldState,
    crafter: EntityId,
    system: &CraftSystemDef,
    recipe: &Recipe,
) -> Roll {
    if !recipe.use_all_res {
        for want in recipe.skills {
            roll_skill_band(
                state,
                crafter,
                want.skill,
                openshard_skills::SkillBand::new(want.min - recipe.min_skill_offset, want.max),
            );
        }
    }
    let odds = chance(state, crafter, system, recipe);
    // Both draws are always spent, and in this order, so what follows a craft does
    // not depend on how the craft went.
    let exceptional = draw(state, odds.exceptional);
    let success = draw(state, odds.success);
    Roll {
        success,
        exceptional,
        all_skills: odds.all_skills,
    }
}

/// Train every required skill once per item made — ServUO's `MultipleSkillCheck`,
/// the `use_all_res` path's substitute for the passive check.
pub(crate) fn train_per_item(state: &mut WorldState, crafter: EntityId, recipe: &Recipe, made: u16) {
    for _ in 0..made {
        for want in recipe.skills {
            roll_skill_band(
                state,
                crafter,
                want.skill,
                openshard_skills::SkillBand::new(want.min - recipe.min_skill_offset, want.max),
            );
        }
    }
}

/// One draw against a per-mille chance, off the world's seeded generator.
fn draw(state: &mut WorldState, chance: u32) -> bool {
    chance > 0 && state.rng.below(1000) < chance
}

/// `chance_at_min` at the floor, certainty at the top, straight line between —
/// ServUO's one formula, in per-mille.
fn interpolate(chance_at_min: u32, floor: i32, max: i32, value: i32) -> u32 {
    if value >= max {
        return 1000;
    }
    if max <= floor {
        // A degenerate band would divide by zero. Everything at or above the top
        // has already returned, so what is left is below it: refuse.
        return 0;
    }
    let at_min = i64::from(chance_at_min);
    let span = i64::from(max - floor);
    let over = i64::from(value - floor);
    let chance = at_min + over * (1000 - at_min) / span;
    // Clamped at the bottom only. A negative chance is a real state (see the
    // module note) and reads the same as zero here, because nothing below one is
    // ever drawn.
    u32::try_from(chance.clamp(0, 1000)).expect("a clamped per-mille chance fits u32")
}

/// The exceptional chance: the success chance less the system's offset.
fn exceptional_chance(
    state: &WorldState,
    crafter: EntityId,
    system: &CraftSystemDef,
    recipe: &Recipe,
    success: u32,
) -> u32 {
    if recipe.never_exceptional {
        return 0;
    }
    if recipe.always_exceptional {
        return 1000;
    }
    let main = skill_value(state, crafter, system.skill);
    exceptional_curve(system.eca, success, i32::from(main))
}

/// The exceptional chance for a given success chance — ServUO's `CraftECA`
/// switch, in per-mille, with the main skill in tenths.
fn exceptional_curve(eca: Eca, success: u32, main: i32) -> u32 {
    let chance = i64::from(success);
    let chance = match eca {
        Eca::ChanceMinusSixty => chance - 600,
        Eca::FiftyPercentChanceMinusTenPercent => chance / 2 - 100,
        Eca::ChanceMinusSixtyToFourtyFive => {
            // ServUO's `0.60 - (skill - 95.0) * 0.03`, clamped to [0.45, 0.60].
            // In tenths and per-mille that is three points of offset per tenth of
            // skill past 95.0 — so a grandmaster smith is fifteen points better
            // at masterpieces than one who has only just qualified.
            let offset = (600 - (i64::from(main) - 950) * 3).clamp(450, 600);
            chance - offset
        }
    };
    u32::try_from(chance.clamp(0, 1000)).expect("a clamped per-mille chance fits u32")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_runs_from_the_systems_floor_to_certainty() {
        // A smith's `chance_at_min` is zero: a recipe you have only just qualified
        // for never works on the first try, and the odds climb straight to
        // certainty at the top of the band.
        assert_eq!(interpolate(0, 0, 1000, 0), 0);
        assert_eq!(interpolate(0, 0, 1000, 500), 500);
        assert_eq!(interpolate(0, 0, 1000, 1000), 1000);
        // A tailor's is five hundred, which lifts the whole line rather than
        // shifting it: half at the floor, three quarters at the middle.
        assert_eq!(interpolate(500, 0, 1000, 0), 500);
        assert_eq!(interpolate(500, 0, 1000, 500), 750);
        assert_eq!(interpolate(500, 0, 1000, 1000), 1000);
    }

    #[test]
    fn a_skill_under_the_floor_reads_as_no_chance_rather_than_a_small_one() {
        // The negative-chance case the module note is about: a recipe's
        // `min_skill_offset` licenses the attempt, it does not improve the odds.
        assert_eq!(interpolate(0, 550, 1000, 500), 0);
    }

    #[test]
    fn a_band_with_no_width_never_divides_by_zero() {
        assert_eq!(interpolate(0, 500, 500, 500), 1000);
        assert_eq!(interpolate(0, 500, 500, 499), 0);
    }

    #[test]
    fn the_smiths_exceptional_offset_narrows_past_ninety_five() {
        // ServUO's sliding `CraftECA`: sixty points off at 95.0, forty-five at
        // 100.0, and clamped to that range at both ends.
        let eca = Eca::ChanceMinusSixtyToFourtyFive;
        assert_eq!(exceptional_curve(eca, 1000, 950), 400);
        assert_eq!(exceptional_curve(eca, 1000, 1000), 550);
        assert_eq!(exceptional_curve(eca, 1000, 700), 400, "clamped at sixty");
        assert_eq!(exceptional_curve(eca, 1000, 1200), 550, "clamped at forty-five");
    }

    #[test]
    fn the_other_two_curves_are_their_arithmetic() {
        assert_eq!(exceptional_curve(Eca::ChanceMinusSixty, 1000, 0), 400);
        assert_eq!(exceptional_curve(Eca::ChanceMinusSixty, 500, 0), 0);
        let half = Eca::FiftyPercentChanceMinusTenPercent;
        assert_eq!(exceptional_curve(half, 1000, 0), 400);
        assert_eq!(exceptional_curve(half, 200, 0), 0);
    }

    #[test]
    fn a_hopeless_craft_can_never_be_a_masterpiece() {
        // Every curve is a subtraction from the success chance, so a crafter who
        // can barely make a thing cannot make a fine one — which is why the
        // exceptional roll is not an independent difficulty.
        for eca in [
            Eca::ChanceMinusSixty,
            Eca::FiftyPercentChanceMinusTenPercent,
            Eca::ChanceMinusSixtyToFourtyFive,
        ] {
            assert_eq!(exceptional_curve(eca, 0, 1000), 0);
        }
    }
}
