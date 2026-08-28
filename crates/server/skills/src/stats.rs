//! Stat gain: the strength, dexterity and intelligence a skill's use nudges up.
//!
//! Ported from ServUO's `SkillCheck.Gain` tail and `IncreaseStat`. Every gain
//! passes through [`try_stat_gain`], which the skill gain calls once it has
//! actually raised something — so stats follow skills, and a locked or capped
//! skill trains neither.
//!
//! Two mechanics, and which one runs is the era:
//!
//! - **Before ML** each stat rolls its own weight from the skill's row
//!   (`str_gain`/`dex_gain`/`int_gain` over ServUO's `33.3`), strength first,
//!   then dexterity, then intelligence, and the first to pass wins.
//! - **From ML** (`combat_era` 4) one flat chance decides whether *any* stat
//!   gains, and then the skill's `primary` stat takes it three times in four.
//!
//! Three things gate a rise, all of them ServUO's: the stat's own arrow must
//! point up, its per-stat cap must have room, and — at the **total** cap — some
//! other stat set to "down" must be willing to give up a point. That last is what
//! makes the classic 225 a budget rather than a wall.

use openshard_config::CombatEra;
use openshard_entities::EntityId;
use openshard_state::components::{LastStatGain, StatLock, StatLocks, Stats};
use openshard_state::skill::Skill;
use openshard_state::{StatCode, WorldState, WorldTick};

use crate::apply_stats;

/// The floor a stat can be lowered to when another one takes its point —
/// ServUO's `RawStr > 10`. Nothing is stripped past this.
const MIN_STAT: u16 = 10;

/// ServUO's divisor on a skill row's stat-gain weight: `StrGain / 33.3`. Carried
/// as its per-mille reciprocal so the chance stays integer — a weight of `2.0`
/// (thousandths `2000`) becomes `2000 * 10 / 333` ≈ 60‰, which is ServUO's 6%.
const GAIN_DIVISOR_MILLI: u32 = 333;

/// Try to nudge a stat after `skill` trained. Called by the skill gain, once.
pub(crate) fn try_stat_gain(state: &mut WorldState, entity: EntityId, skill: Skill) {
    let info = skill.info();
    let locks = state
        .registry
        .get::<StatLocks>(entity)
        .copied()
        .unwrap_or_default();

    if state.gameplay.combat_era >= CombatEra::new(4) {
        // ML: one flat chance, then the skill's own primary/secondary stat.
        if state.rng.below(1000) >= state.gameplay.stat_gain_chance {
            return;
        }
        let primary = info.primary;
        let secondary = info.secondary;
        let primary_up = lock_of(locks, primary) == StatLock::Up;
        let secondary_up = lock_of(locks, secondary) == StatLock::Up;
        let stat = if primary_up && secondary_up {
            // One time in four the secondary takes it instead.
            if state.rng.below(4) == 0 {
                Some(secondary)
            } else {
                Some(primary)
            }
        } else if primary_up {
            Some(primary)
        } else if secondary_up {
            Some(secondary)
        } else {
            None
        };
        if let Some(stat) = stat {
            gain_stat(state, entity, stat);
        }
        return;
    }

    // Pre-ML: each stat rolls the weight the skill's row gives it, in order, and
    // the first to pass takes the gain. `else if` and not three draws — a use
    // never raises two stats at once.
    for (stat, weight) in [
        (StatCode::Str, info.str_gain),
        (StatCode::Dex, info.dex_gain),
        (StatCode::Int, info.int_gain),
    ] {
        if lock_of(locks, stat) != StatLock::Up {
            continue;
        }
        let chance = weight * 10 / GAIN_DIVISOR_MILLI;
        if chance > state.rng.below(1000) {
            gain_stat(state, entity, stat);
            return;
        }
    }
}

/// Which arrow governs a stat.
const fn lock_of(locks: StatLocks, stat: StatCode) -> StatLock {
    match stat {
        StatCode::Str => locks.strength,
        StatCode::Dex => locks.dexterity,
        StatCode::Int => locks.intelligence,
    }
}

/// Raise one stat by a point, if its cooldown has passed and the caps allow —
/// ServUO's `GainStat` into `IncreaseStat`.
pub fn gain_stat(state: &mut WorldState, entity: EntityId, stat: StatCode) {
    if !claim_stat_timer(state, entity, stat) {
        return;
    }
    let Some(&stats) = state.registry.get::<Stats>(entity) else {
        return;
    };
    let locks = state
        .registry
        .get::<StatLocks>(entity)
        .copied()
        .unwrap_or_default();
    let individual = state.gameplay.stat_cap_individual;
    if value_of(stats, stat) >= individual {
        return; // this stat is already at its own ceiling
    }

    let total = u32::from(stats.strength) + u32::from(stats.dexterity) + u32::from(stats.intelligence);
    let at_total_cap = total >= u32::from(state.gameplay.stat_cap);
    let mut next = stats;
    if at_total_cap {
        // Somebody has to give up a point. Prefer the *lower* of the two others,
        // so a build sheds from where it can least afford to keep — ServUO's
        // `RawDex < RawInt || !CanLower(Int)`.
        let (first, second) = others(stat);
        let can = |s: StatCode| lock_of(locks, s) == StatLock::Down && value_of(stats, s) > MIN_STAT;
        let donor = if can(first) && (value_of(stats, first) < value_of(stats, second) || !can(second)) {
            Some(first)
        } else if can(second) {
            Some(second)
        } else {
            None
        };
        let Some(donor) = donor else {
            return; // at the cap with nothing set to give ground: no gain
        };
        set_value(&mut next, donor, value_of(stats, donor) - 1);
    }
    set_value(&mut next, stat, value_of(stats, stat) + 1);
    apply_stats(state, entity, next);
}

/// Whether this stat's cooldown has passed, stamping it if so — ServUO's
/// `CheckStatTimer`. A tick count, so it replays.
fn claim_stat_timer(state: &mut WorldState, entity: EntityId, stat: StatCode) -> bool {
    let now = state.ticks;
    let delay = state.gameplay.stat_gain_ticks;
    let mut last = state
        .registry
        .get::<LastStatGain>(entity)
        .copied()
        .unwrap_or_default();
    let previous = match stat {
        StatCode::Str => last.strength,
        StatCode::Dex => last.dexterity,
        StatCode::Int => last.intelligence,
    };
    // `previous` is zero for a mobile that has never gained, which is "long ago"
    // on a fresh world and stays true after a restore: the stamp is a tick count
    // and the counter only ever climbs.
    if previous != WorldTick::ZERO && now < previous.saturating_add(delay) {
        return false;
    }
    match stat {
        StatCode::Str => last.strength = now.max(WorldTick::from_raw(1)),
        StatCode::Dex => last.dexterity = now.max(WorldTick::from_raw(1)),
        StatCode::Int => last.intelligence = now.max(WorldTick::from_raw(1)),
    }
    state.registry.insert(entity, last);
    true
}

/// The two stats that are not `stat`, in strength/dexterity/intelligence order.
const fn others(stat: StatCode) -> (StatCode, StatCode) {
    match stat {
        StatCode::Str => (StatCode::Dex, StatCode::Int),
        StatCode::Dex => (StatCode::Str, StatCode::Int),
        StatCode::Int => (StatCode::Str, StatCode::Dex),
    }
}

/// One stat's value.
const fn value_of(stats: Stats, stat: StatCode) -> u16 {
    match stat {
        StatCode::Str => stats.strength,
        StatCode::Dex => stats.dexterity,
        StatCode::Int => stats.intelligence,
    }
}

/// Write one stat's value.
const fn set_value(stats: &mut Stats, stat: StatCode, value: u16) {
    match stat {
        StatCode::Str => stats.strength = value,
        StatCode::Dex => stats.dexterity = value,
        StatCode::Int => stats.intelligence = value,
    }
}
