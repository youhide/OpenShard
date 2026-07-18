//! Skills and stats: usage checks, the gain curve, and the stat foundation.
//!
//! A gameplay system in its own crate, like [`chat`](openshard_chat). The
//! functions here operate on the shared [`WorldState`]: set a skill or a stat,
//! use a skill against a difficulty, roll it. A use resolves the check, applies
//! any gain, and emits [`SkillUsed`] — what the use *accomplishes* (the ore, the
//! turned lock) is a script's to decide, the same decoupling combat's
//! `MobileDied` has.
//!
//! [`roll_skill`] is public on purpose: magic's casting rolls the same way a
//! mined ore does, so the caster trains Magery through this one function.

mod curves;

pub use curves::SKILL_CAP;

use openshard_entities::{EntityId, Serial};
use openshard_state::components::{Hitpoints, Mana, Skills, Stats};
use openshard_state::WorldState;

/// A mobile used a skill: the check resolved, and any gain is already applied.
///
/// What the *use* accomplishes is not decided here — whether the ore comes out
/// of the rock, whether the lockpick turns — only whether the roll passed and
/// where the skill stands now. A script reads this and grants the reward, the
/// same decoupling combat's `MobileDied` has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkillUsed {
    /// The mobile.
    pub entity: EntityId,
    /// Its wire identity.
    pub serial: Serial,
    /// Which skill, by id.
    pub skill: u8,
    /// Whether the check succeeded.
    pub success: bool,
    /// The skill's value now, in tenths, after any gain.
    pub value: u16,
}

/// Set a mobile's stats, and re-cap its hit points and mana to match.
pub fn set_stats(
    state: &mut WorldState,
    serial: u32,
    strength: u16,
    dexterity: u16,
    intelligence: u16,
) {
    let Some(entity) = Serial::new(serial).and_then(|s| state.registry.entity_of(s)) else {
        return;
    };
    state.registry.insert(
        entity,
        Stats {
            strength,
            dexterity,
            intelligence,
        },
    );
    // Strength caps hit points, intelligence mana; a lowered cap drags the
    // current value down with it, a raised one leaves room to heal into.
    if let Some(&Hitpoints { current, .. }) = state.registry.get::<Hitpoints>(entity) {
        state.registry.insert(
            entity,
            Hitpoints {
                current: current.min(strength),
                max: strength,
            },
        );
    }
    if let Some(&Mana { current, .. }) = state.registry.get::<Mana>(entity) {
        state.registry.insert(
            entity,
            Mana {
                current: current.min(intelligence),
                max: intelligence,
            },
        );
    }
}

/// Set a mobile's skill value, in tenths.
pub fn set_skill(state: &mut WorldState, serial: u32, skill: u8, value: u16) {
    let Some(serial) = Serial::new(serial) else {
        return;
    };
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    let mut skills = state
        .registry
        .get::<Skills>(entity)
        .cloned()
        .unwrap_or_default();
    skills.set(skill, value.min(state.gameplay.skill_cap));
    state.registry.insert(entity, skills);
}

/// Use a skill against a difficulty: roll it, teach from it, announce it.
pub fn use_skill(state: &mut WorldState, serial: u32, skill: u8, difficulty: u16) {
    let Some(serial) = Serial::new(serial) else {
        return;
    };
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    let success = roll_skill(state, entity, skill, difficulty);
    let value = state
        .registry
        .get::<Skills>(entity)
        .map_or(0, |s| s.get(skill));
    state.bus.send(SkillUsed {
        entity,
        serial,
        skill,
        success,
        value,
    });
}

/// Roll a skill against a difficulty and teach from the attempt: returns whether
/// it passed, and bumps the value on a gain. The shared heart of [`use_skill`]
/// and magic's casting, so a mined ore and a cast spell train the same way.
///
/// The success draw comes before the gain draw, always, so the sequence is fixed
/// and the whole thing replays.
pub fn roll_skill(state: &mut WorldState, entity: EntityId, skill: u8, difficulty: u16) -> bool {
    let value = state
        .registry
        .get::<Skills>(entity)
        .map_or(0, |s| s.get(skill));
    let success = curves::success_chance(value, difficulty) >= state.rng.below(1000);
    if value < state.gameplay.skill_cap && state.rng.below(1000) < curves::gain_chance(value) {
        let mut skills = state
            .registry
            .get::<Skills>(entity)
            .cloned()
            .unwrap_or_default();
        skills.set(skill, value + 1);
        state.registry.insert(entity, skills);
    }
    success
}
