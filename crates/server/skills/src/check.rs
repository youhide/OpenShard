//! The skill check and the gain — ServUO's `Scripts/Misc/SkillCheck.cs`.
//!
//! Three questions, in this order, every time a skill is used:
//!
//! 1. **What is the skill worth?** [`skill_value`] — the trained base plus what
//!    the mobile's stats lend it.
//! 2. **Did it work?** [`roll_skill_band`] turns a difficulty band into a chance
//!    and draws against it.
//! 3. **Did it teach anything?** [`gain_chance`] weighs the headroom under this
//!    skill's cap *and* under the total, and a draw against that raises the value
//!    — if the lock allows, if the total leaves room, and if something set to
//!    "down" can give ground when it does not.
//!
//! # Why this replaced the Sphere curve
//!
//! What was here before was Sphere's `Calc_GetSCurve` against a single difficulty
//! number, with a gain chance that fell linearly from 30% at nothing to zero at
//! the cap. It was a stand-in, and it had no idea a total skill cap existed —
//! which is the rule that makes a character a *build* rather than a list of
//! everything. ServUO's model is the classic one: a difficulty *band* (below it
//! you cannot, above it you do not learn), and a gain chance that reads both caps.
//!
//! # Fixed point
//!
//! Skill values are tenths (`755` is 75.5). Chances are per-mille, because the
//! world's [`Rng`](openshard_state::rng::Rng) draws integers and the tick has to
//! replay. ServUO's doubles are carried as integers throughout; where the two
//! could differ, the comment says which way and why.

use openshard_config::CombatEra;
use openshard_entities::EntityId;
use openshard_protocol::skill::SkillLock;
use openshard_state::WorldState;
use openshard_state::components::{
    Client,
    Skills,
    Stats,
};
use openshard_state::skill::Skill;

use crate::{
    SkillChanged,
    SkillValue,
};

/// The lower and upper difficulty edges for a skill check, in tenths.
///
/// Bounds remain signed because some canonical checks start below zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkillBand {
    min: i32,
    max: i32,
}

impl SkillBand {
    /// Build a difficulty band from its inclusive lower and exclusive upper edge.
    pub const fn new(min: i32, max: i32) -> Self {
        Self { min, max }
    }
}
use crate::stats::try_stat_gain;

/// The floor ServUO clamps its gain chance to — 1%, so even a grandmaster with no
/// headroom left has a sliver of a chance rather than a certainty of nothing.
const MIN_GAIN_CHANCE: u32 = 10;

/// A successful skill gain is a small random step, in tenths. Keeping the
/// range narrow makes progress legible while stopping each hit from advancing
/// by the same visible 0.1.
const MIN_GAIN_STEP: u16 = 1;
const MAX_GAIN_STEP: u16 = 3;
/// Below this, every skill use gains experience. In tenths.
const EASY_GAIN_BELOW: u16 = 100;

/// 100.0, in tenths — the number the stat bonus fades against.
const SKILL_100: u16 = 1000;

/// What a skill is actually worth to a mobile: the trained base, plus the share
/// its stats lend — ServUO's `Skill.NonRacialValue`.
///
/// Only a handful of skills lean on stats at all (Parrying, the craft set, Camping,
/// Herding, Magery…); for the rest every scale is zero and this is simply the base.
/// The bonus fades as the base rises (`inv`), and is capped at the row's
/// `stat_total`, so stats above 100 stop helping.
///
/// A **read-site derivation**, like `combat::equipped_weapon` and
/// `items::carried`: nothing is mirrored onto the mobile, so a Strength spell
/// raises a smith's effective skill for as long as it lasts, with no bookkeeping
/// anywhere and nothing to undo when it expires.
#[must_use]
pub fn skill_value(state: &WorldState, entity: EntityId, skill: Skill) -> u16 {
    let base = discorded(
        state,
        entity,
        state.registry.get::<Skills>(entity).map_or(0, |s| s.get(skill)),
    );
    // From AoS the stat influence is gone: ServUO calls `AOS.DisableStatInfluences()`
    // at startup, which zeroes the three scale columns in place. An `if` says the
    // same thing without a mutable table.
    if state.gameplay.combat_era >= CombatEra::new(2) {
        return base;
    }
    let info = skill.info();
    if info.stat_total == 0 {
        return base; // the common case: no stat lends to this skill
    }
    let Some(&Stats {
        strength,
        dexterity,
        intelligence,
    }) = state.registry.get::<Stats>(entity)
    else {
        return base;
    };
    // Scales are hundredths and stats whole points, and the answer wants tenths:
    // 100 strength at a scale of 7.5 is 7.5 skill points, i.e. 75 tenths.
    let offset = (u32::from(strength) * info.str_scale
        + u32::from(dexterity) * info.dex_scale
        + u32::from(intelligence) * info.int_scale)
        / 1000;
    // The ceiling is the row's `stat_total`, in the same tenths.
    let ceiling = info.stat_total / 10;
    // The bonus fades to nothing as the base approaches 100.0. ServUO scales both
    // sides by `inv` and then compares; clamping first and scaling once is the
    // same comparison with one fewer rounding.
    let inv = u32::from(SKILL_100.saturating_sub(base));
    let bonus = offset.min(ceiling) * inv / u32::from(SKILL_100);
    base.saturating_add(u16::try_from(bonus).unwrap_or(u16::MAX))
}

/// Take Discordance's cut off a skill, if a bard has put this mobile out of tune.
///
/// Applied here, at the *one* question every system asks about how good somebody
/// is, so a discorded creature hits worse, resists worse and casts worse without
/// combat, magic or the AI knowing what a lute is. The penalty is a percentage and
/// the value is in tenths, so the arithmetic is exact.
fn discorded(state: &WorldState, entity: EntityId, base: u16) -> u16 {
    let Some(song) = state
        .registry
        .get::<openshard_state::components::Discorded>(entity)
    else {
        return base;
    };
    base.saturating_sub(base * song.penalty.min(100) / 100)
}

/// Roll a skill against a difficulty band, and teach from the attempt.
///
/// ServUO's `CheckSkill(skill, minSkill, maxSkill)`: under `min_skill` the task is
/// beyond the mobile and fails without a draw; at or above `max_skill` it is no
/// challenge and succeeds without one. Between them the chance is where the skill
/// sits in the band, and *that* is what both the success draw and the gain chance
/// read — so a task at the edge of your ability teaches most.
///
/// Both bounds are in tenths, like the skill, and signed: several of ServUO's
/// bands start below zero (Stealth in light armour, Cartography's first level), so
/// that everyone succeeds and everyone still learns.
pub fn roll_skill_band(state: &mut WorldState, entity: EntityId, skill: Skill, band: SkillBand) -> bool {
    let value = i32::from(skill_value(state, entity, skill));
    if value < band.min {
        return false; // too difficult
    }
    if value >= band.max {
        return true; // no challenge
    }
    // Reaching here proves `band.min <= value < band.max`, hence the divisor is
    // positive even if a caller supplied a degenerate band. Do the subtraction
    // wide: `SkillBand` accepts signed `i32` edges, whose full span exceeds an
    // `i32` even though any canonical skill band is much narrower.
    check(state, entity, skill, chance_in_band(value, band))
}

/// Return the per-mille chance for a value strictly inside a difficulty band.
///
/// The caller establishes `band.min <= value < band.max`, so the denominator is
/// strictly positive.
fn chance_in_band(value: i32, band: SkillBand) -> u32 {
    let chance =
        (i64::from(value) - i64::from(band.min)) * 1000 / (i64::from(band.max) - i64::from(band.min));
    chance.clamp(0, 1000) as u32
}

/// Roll a skill against a chance already worked out (per-mille), and teach from
/// the attempt — ServUO's `CheckSkill(skill, chance)`.
///
/// For a caller that computes its own odds rather than naming a band: combat's
/// to-hit is a formula over two mobiles' weapon skills, not a difficulty, but a
/// swing still trains the skill the same way.
pub fn roll_skill_chance(state: &mut WorldState, entity: EntityId, skill: Skill, chance: u32) -> bool {
    check(state, entity, skill, chance.min(1000))
}

/// The heart: draw for success, then draw for a gain — in that fixed order, so
/// the sequence replays.
fn check(state: &mut WorldState, entity: EntityId, skill: Skill, chance: u32) -> bool {
    if state.gameplay.total_skill_cap == 0 {
        return false; // ServUO's `Skills.Cap == 0` guard
    }
    let success = state.rng.below(1000) <= chance;
    let gain = gain_chance(state, entity, skill, chance, success);
    // A skill under 10.0 always takes something from the attempt — the first
    // few points come for the asking, which keeps a new character moving.
    let base = state.registry.get::<Skills>(entity).map_or(0, |s| s.get(skill));
    if base < EASY_GAIN_BELOW || state.rng.below(1000) <= gain {
        gain_skill(state, entity, skill);
    }
    success
}

/// How likely this attempt is to teach something, in per-mille — ServUO's
/// `GetGainChance`.
///
/// Two halves averaged: how much room is left under the *total* cap, and how much
/// under this skill's own. Then the difficulty is folded in — a success at the
/// limit of your ability teaches half as much again, and (pre-AoS) even a failure
/// at one still teaches a fifth — and the whole is halved once more and scaled by
/// the skill's `gain_factor`.
#[must_use]
pub fn gain_chance(state: &WorldState, entity: EntityId, skill: Skill, chance: u32, success: bool) -> u32 {
    let skills = state.registry.get::<Skills>(entity);
    let total_cap = state.gameplay.total_skill_cap.max(1);
    let total = skills.map_or(0, Skills::total).min(total_cap);
    let cap = skills.map_or(state.gameplay.skill_cap, |s| s.cap(skill)).max(1);
    let base = skills.map_or(0, |s| s.get(skill)).min(cap);

    // Headroom under the two caps, each as a per-mille fraction, averaged.
    let mut gain =
        ((total_cap - total) * 1000 / total_cap + u32::from(cap - base) * 1000 / u32::from(cap)) / 2;
    // What the attempt itself was worth. From AoS a failure teaches nothing;
    // before it, a failure at something hard still does.
    let weight = if success {
        500
    } else if state.gameplay.combat_era >= CombatEra::new(2) {
        0
    } else {
        200
    };
    gain += (1000 - chance.min(1000)) * weight / 1000;
    gain /= 2;
    gain = gain * skill.info().gain_factor / 1000;
    gain.clamp(MIN_GAIN_CHANCE, 1000)
}

/// Raise a skill — ServUO's `Gain`, minus the branches for systems this engine
/// does not have (Siege, quest bonuses, alacrity scrolls, pet masteries).
///
/// Three gates, and each is a rule players feel:
///
/// - the arrow must point **up**, and the skill must be under its own cap;
/// - under 10.0 a gain is worth one to four tenths, not one;
/// - the **total** cap must have room — and a skill set to "down" gives up what
///   this one takes so that it can.
///
/// The total cap binds players only. A creature's sheet is a spawn's data, not a
/// build, and ServUO exempts it the same way (`if (!from.Player || …)`).
fn gain_skill(state: &mut WorldState, entity: EntityId, skill: Skill) {
    let Some(skills) = state.registry.get::<Skills>(entity).cloned() else {
        // No sheet at all: nothing to train. `set_skill` gives a mobile one.
        return;
    };
    let base = skills.get(skill);
    let cap = skills.cap(skill);
    if base >= cap || skills.lock(skill) != SkillLock::Up {
        // Locked, set to fall, or already at its ceiling. Deliberately no stat
        // gain either — ServUO reads the same lock for both.
        return;
    }
    let gain_steps = std::num::NonZeroU16::new(MAX_GAIN_STEP - MIN_GAIN_STEP + 1)
        .expect("the maximum gain step is at least the minimum");
    let to_gain = MIN_GAIN_STEP + crate::roll_u16(&mut state.rng, gain_steps);

    let player = state.registry.has::<Client>(entity);
    let mut skills = skills;
    let mut lowered = None;
    if player {
        lowered = reduce_a_down_skill(state, &mut skills, skill, to_gain);
    }
    let fits = !player || skills.total() + u32::from(to_gain) <= state.gameplay.total_skill_cap;
    let raised = fits.then(|| {
        let raised = base.saturating_add(to_gain).min(cap);
        skills.set(skill, raised);
        raised
    });
    // Written back whichever way it went: a "down" skill gives ground even on the
    // gain that then does not fit.
    state.registry.insert(entity, skills);

    // Both halves of the move are announced, so an open window follows a skill
    // that falls as well as one that rises.
    if let Some(serial) = state.registry.serial_of(entity) {
        for (moved, previous, value) in lowered
            .into_iter()
            .map(|(moved, value)| (moved, value.saturating_add(to_gain), value))
            .chain(raised.map(|value| (skill, base, value)))
        {
            state.bus.send(SkillChanged {
                entity,
                serial,
                skill: moved,
                previous: SkillValue::new(previous),
                value: SkillValue::new(value),
            });
        }
    }

    if raised.is_some() {
        try_stat_gain(state, entity, skill);
    }
}

/// Take `to_gain` tenths off some skill the player set to "down", to make room
/// under the total cap — ServUO's `CheckReduceSkill`. Returns what it lowered.
///
/// The chance is how full the sheet is: a character nowhere near the total cap
/// almost never sheds anything, one at the cap sheds on almost every gain.
///
/// **This is where the arithmetic was audited.** ServUO writes
/// `skills.Total / skills.Cap >= Utility.RandomDouble()`, and both sides are `int`
/// in C#, so the division truncates to 0 for every character under the cap and to
/// 1 exactly at it — the branch is effectively "never, then always". The
/// surrounding code makes the intent plain (it is a fullness *fraction*, sitting
/// next to a `RandomDouble`), so the intent is what is implemented here: the real
/// ratio, in per-mille, drawn against the world's generator. Copying the
/// truncation would be copying a bug, which is worse than not copying it.
fn reduce_a_down_skill(
    state: &mut WorldState,
    skills: &mut Skills,
    gaining: Skill,
    to_gain: u16,
) -> Option<(Skill, u16)> {
    let total_cap = state.gameplay.total_skill_cap.max(1);
    let fullness = (skills.total() * 1000 / total_cap).min(1000);
    if state.rng.below(1000) >= fullness {
        return None;
    }
    let victim = skills.ids().find(|&skill| {
        skill != gaining && skills.lock(skill) == SkillLock::Down && skills.get(skill) >= to_gain
    })?;
    let lowered = skills.get(victim) - to_gain;
    skills.set(victim, lowered);
    Some((victim, lowered))
}

#[cfg(test)]
mod tests {
    use super::{
        SkillBand,
        chance_in_band,
    };

    #[test]
    fn chance_in_band_accepts_the_full_i32_span() {
        assert_eq!(chance_in_band(0, SkillBand::new(i32::MIN, i32::MAX)), 500);
    }
}
