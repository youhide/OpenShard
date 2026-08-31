//! The lore skills that read a living body: Anatomy and Evaluating Intelligence.
//!
//! Each raises a cursor, and each answers with a **cliloc over the thing looked
//! at**, seen by the asker alone — ServUO's `PrivateOverheadMessage`, so a busy
//! street does not read everybody's checks. The skills that read an *object* are
//! [`super::appraise`]; the one that reads a crime is [`super::forensics`].
//!
//! They share one shape, and it is worth naming because the rest of the skill
//! handlers follow it: a margin of error that narrows as the skill rises, a
//! `roll_skill_band` that both decides and trains, and a cliloc chosen by
//! *arithmetic on a base number* — `1038045 + strength*11 + dexterity` is one
//! cliloc per combination, and the client has all 121 of them. Getting the base
//! or the stride wrong shows a plausible sentence about the wrong thing, which is
//! why the numbers are quoted from ServUO beside each one.

use openshard_entities::EntityId;
use openshard_protocol::wire::ClilocId;
use openshard_state::components::{
    Body,
    BodyType,
    Mana,
    Stamina,
    Stats,
};
use openshard_state::{
    Skill,
    WorldState,
};

use crate::check::roll_skill_band;

/// "That looks [strong] and [dexterous]." — the base of an eleven-by-eleven block
/// of clilocs, strength striding by eleven and dexterity by one.
const ANATOMY_RESULT: u32 = 1_038_045;
/// "That being is at [n] percent endurance." — eleven in a row.
const ANATOMY_STAMINA: u32 = 1_038_303;
/// "You can not quite get a sense of their physical characteristics."
const ANATOMY_FAILED: ClilocId = ClilocId(1_042_666);
/// "You know yourself quite well enough already."
const ANATOMY_SELF: ClilocId = ClilocId(500_324);
/// "Only living things have anatomies!"
const ANATOMY_NOT_ALIVE: ClilocId = ClilocId(500_323);
/// The skill at which Anatomy starts reporting stamina too, in tenths.
const ANATOMY_STAMINA_AT: u16 = 650;

/// What a stat reads as on a mobile that was never given a sheet.
///
/// Every ServUO `Mobile` has all three; here they are a component, and a bare
/// spawned creature may have none. The status bar already answers this question
/// the same way (`World::status_of`), and two different fallbacks for the same
/// missing number would be worse than one that is merely a convention.
const STAT_WITHOUT_A_SHEET: u16 = 100;

/// "He/She/It looks [of average intellect]." — thirty-three in a row, in three
/// blocks of eleven: male, female, and everything that is not human.
const EVAL_INT_RESULT: u32 = 1_038_169;
/// "That being is at [n] percent mental strength."
const EVAL_INT_MANA: u32 = 1_038_202;
/// "You cannot judge his/her/its mental abilities." — three in a row.
const EVAL_INT_FAILED: u32 = 1_038_166;
/// "Hmm, that person looks really silly."
const EVAL_INT_SELF: ClilocId = ClilocId(500_910);
/// "It looks smarter than a rock, but dumber than a piece of wood."
const EVAL_INT_ITEM: ClilocId = ClilocId(500_908);
/// The skill at which Eval Int starts reporting mana too, in tenths.
const EVAL_INT_MANA_AT: u16 = 760;

/// A margin of error, in whole stat points, that narrows as the skill rises —
/// ServUO's `Math.Max(0, ceiling - value / divisor)`. A novice's guess is wrong by
/// up to twenty-five; a grandmaster's is exact.
fn margin(value: u16, ceiling: i32, divisor: i32) -> i32 {
    (ceiling - i32::from(value) / (divisor * 10)).max(0)
}

/// A stat fuzzed by the margin, then reduced to the 0..=10 index a cliloc block
/// is addressed by. `rng` is the world's, so the guess replays.
fn fuzzed_index(state: &mut WorldState, value: i32, margin: i32) -> u32 {
    let spread = margin * 2 + 1;
    let noise = state.rng.below(u32::try_from(spread).unwrap_or(1)) as i32 - margin;
    u32::try_from((value + noise) / 10).unwrap_or(0).min(10)
}

/// A pool as a percentage of its maximum, the form both stamina and mana lines
/// take. A mobile with no pool reads as full.
fn pool_percent(current: u16, max: u16) -> i32 {
    i32::from(current) * 100 / i32::from(max.max(1))
}

/// Anatomy: strength, dexterity, and — past 65.0 — how winded the target is.
pub(super) fn anatomy(state: &mut WorldState, actor: EntityId, target: EntityId) {
    if actor == target {
        state.localized_message(actor, ANATOMY_SELF, "");
        return;
    }
    // A body is what makes something a mobile here — an item carries a graphic
    // instead. ServUO asks `targeted is Mobile`, which is the same question.
    let Some(stats) = mobile_stats(state, target) else {
        state.localized_message(actor, ANATOMY_NOT_ALIVE, "");
        return;
    };
    let skill = Skill::Anatomy;
    let margin = margin(crate::skill_value(state, actor, skill), 25, 4);
    let strength = fuzzed_index(state, i32::from(stats.strength), margin);
    let dexterity = fuzzed_index(state, i32::from(stats.dexterity), margin);
    let stamina = state
        .registry
        .get::<Stamina>(target)
        .map_or(100, |s| pool_percent(s.current, s.max));
    let stamina = fuzzed_index(state, stamina, margin);

    if roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000)) {
        state.private_overhead_cliloc(
            actor,
            target,
            ClilocId(ANATOMY_RESULT + strength * 11 + dexterity),
            "",
        );
        // The endurance line is a second sentence, and only a trained eye sees it.
        // ServUO reads the *base* here, not the effective value: a strong smith
        // does not learn to read a stranger's breathing.
        if trained(state, actor, skill) >= ANATOMY_STAMINA_AT {
            state.private_overhead_cliloc(actor, target, ClilocId(ANATOMY_STAMINA + stamina), "");
        }
    } else {
        state.private_overhead_cliloc(actor, target, ANATOMY_FAILED, "");
    }
}

/// Evaluating Intelligence: wits, and — past 76.0 — how much mana is left.
pub(super) fn eval_int(state: &mut WorldState, actor: EntityId, target: EntityId) {
    if actor == target {
        state.localized_message(actor, EVAL_INT_SELF, "");
        return;
    }
    let Some(stats) = mobile_stats(state, target) else {
        state.localized_message(actor, EVAL_INT_ITEM, "");
        return;
    };
    let skill = Skill::EvalInt;
    let margin = margin(crate::skill_value(state, actor, skill), 20, 5);
    let intelligence = fuzzed_index(state, i32::from(stats.intelligence), margin);
    let mana = state
        .registry
        .get::<Mana>(target)
        .map_or(100, |m| pool_percent(m.current, m.max));
    let mana = fuzzed_index(state, mana, margin);
    // Which block of eleven the sentence comes from: he, she, or it.
    let body = state.registry.get::<Body>(target).map_or(22, |body| {
        match openshard_state::components::body_type(body.id) {
            BodyType::Human => u32::from(openshard_state::components::body_is_female(body.id)) * 11,
            _ => 22,
        }
    });

    if roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1200)) {
        state.private_overhead_cliloc(actor, target, ClilocId(EVAL_INT_RESULT + intelligence + body), "");
        if trained(state, actor, skill) >= EVAL_INT_MANA_AT {
            state.private_overhead_cliloc(actor, target, ClilocId(EVAL_INT_MANA + mana), "");
        }
    } else {
        // Three failure lines, one per pronoun — the block index over eleven.
        state.private_overhead_cliloc(actor, target, ClilocId(EVAL_INT_FAILED + body / 11), "");
    }
}

/// The stats of `entity` if it is a mobile at all, `None` if it is an item.
fn mobile_stats(state: &WorldState, entity: EntityId) -> Option<Stats> {
    state.registry.get::<Body>(entity)?;
    Some(state.registry.get::<Stats>(entity).copied().unwrap_or(Stats {
        strength:     STAT_WITHOUT_A_SHEET,
        dexterity:    STAT_WITHOUT_A_SHEET,
        intelligence: STAT_WITHOUT_A_SHEET,
    }))
}

/// What a mobile has actually *trained*, in tenths — the base, with no help from
/// its stats. The threshold at which a lore skill starts telling you more reads
/// this rather than the effective value, as ServUO's do.
fn trained(state: &WorldState, entity: EntityId, skill: Skill) -> u16 {
    state
        .registry
        .get::<openshard_state::Skills>(entity)
        .map_or(0, |s| s.get(skill))
}
