//! Skills and stats: the check, the gain curve, and the stat foundation.
//!
//! A gameplay system in its own crate, like `chat`. The functions here operate on
//! the shared [`WorldState`]: set a skill or a stat, use a skill against a
//! difficulty band, roll it. A use resolves the check, applies any gain, and emits
//! [`SkillUsed`] — what the use *accomplishes* (the ore, the turned lock) is
//! decided elsewhere, the same decoupling combat's `MobileDied` has.
//!
//! What a skill *is* — its name, its client id, the stats it leans on — is
//! [`openshard_state::skill`], data several crates read. What training one *does*
//! is here.
//!
//! [`roll_skill_band`] and [`roll_skill_chance`] are public on purpose: magic's
//! casting and combat's to-hit train through the very same call a mined ore does,
//! so there is one gain curve in the engine and not three.

mod button;
mod check;
mod handlers;
mod stats;

use std::num::NonZeroU16;

pub use button::{
    DEFAULT_SKILL_DELAY_TICKS,
    SkillRequested,
    set_skill_delay,
    use_skill_button,
};
pub use check::{
    SkillBand,
    gain_chance,
    roll_skill_band,
    roll_skill_chance,
    skill_value,
};
pub use handlers::{
    BANDAGE_GRAPHIC,
    BandageFinished,
    BandageStarted,
    Begged,
    HarvestTarget,
    Harvested,
    InstrumentSpent,
    LOCKPICK_GRAPHIC,
    LockpickBroke,
    MAX_FOLLOWERS,
    Outcome,
    PoisonedSelf,
    Stolen,
    Tamed,
    ToolWorn,
    advance_harvests,
    begin_harvest,
    expire_ghost_contact,
    expire_songs,
    finish_bandages,
    followers_of,
    on_item_target,
    on_second_target,
    on_target,
    play_instrument,
    resolve_harvest_target,
    snooping,
    use_bandage,
    use_lockpick,
    use_tool,
};
use openshard_entities::EntityId;
use openshard_protocol::serial::Serial;
use openshard_state::WorldState;
use openshard_state::components::{
    Hitpoints,
    Mana,
    Skills,
    Stamina,
    Stats,
};
use openshard_state::skill::Skill;
pub use stats::gain_stat;

/// The two ordinary dice used by skill handlers.
const PER_MILLE: NonZeroU16 = NonZeroU16::new(1000).expect("one thousand is nonzero");
const PERCENT: NonZeroU16 = NonZeroU16::new(100).expect("one hundred is nonzero");

/// Draw a value whose type is already narrow enough for signed skill formulae.
///
/// [`openshard_state::Rng::below`] returns a `u32` for general-purpose bounds,
/// but these handlers roll below a `u16` bound. The bound in the type proves
/// both that the draw is well formed and that converting its result cannot
/// fail; callers therefore do not need a made-up value for an impossible error.
fn roll_u16(rng: &mut openshard_state::Rng, bound: NonZeroU16) -> u16 {
    u16::try_from(rng.below(u32::from(bound.get()))).expect("Rng::below returned a value below a u16 bound")
}

/// A trained skill value, represented in tenths (`755` means `75.5`).
///
/// Keeping the fixed-point unit in the type prevents skill events from being
/// accidentally populated with an unrelated `u16` such as a stat or a cap.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SkillValue(u16);

impl SkillValue {
    /// Construct a value from the protocol/gameplay fixed-point representation.
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }

    /// Return the fixed-point value for protocol and scripting boundaries.
    pub const fn raw(self) -> u16 {
        self.0
    }
}

/// A mobile's skill moved, in trained value or in what a window would draw.
///
/// The world reads it to send the owner the single-line `0x3A` that makes an open
/// skill window follow live; nothing else needs it. Three things raise one: the
/// gain path — up from training, or *down* when a skill set to "down" gives
/// ground so another can rise past the total cap — [`set_skill`], and
/// [`apply_stats`].
///
/// # A stat change moves a skill without training it
///
/// The value a window draws is [`skill_value`]: the trained number *plus* what
/// the body's stats lend it before AoS. So changing strength moves every skill
/// strength lends to, without any of them being trained, and a window standing in
/// front of the player would otherwise keep drawing the old numbers forever. Those
/// events carry [`previous`](Self::previous) **equal** to
/// [`value`](Self::value), which is honest — the trained number did not move —
/// and is also what keeps "your skill has increased" quiet for a change that is
/// not a gain.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkillChanged {
    /// The mobile.
    pub entity:   EntityId,
    /// Its wire identity.
    pub serial:   Serial,
    /// Which skill.
    pub skill:    Skill,
    /// Its trained value before this move, in tenths.
    pub previous: SkillValue,
    /// The skill's value now, in tenths.
    pub value:    SkillValue,
}

/// A mobile used a skill: the check resolved, and any gain is already applied.
///
/// What the *use* accomplishes is not decided here — whether the ore comes out of
/// the rock, whether the lockpick turns — only whether the roll passed and where
/// the skill stands now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SkillUsed {
    /// The mobile.
    pub entity:  EntityId,
    /// Its wire identity.
    pub serial:  Serial,
    /// Which skill.
    pub skill:   Skill,
    /// Whether the check succeeded.
    pub success: bool,
    /// The skill's value now, in tenths, after any gain.
    pub value:   SkillValue,
}

/// Set a mobile's stats by serial, and re-cap its pools to match.
pub fn set_stats(state: &mut WorldState, serial: Serial, stats: Stats) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    apply_stats(state, entity, stats);
}

/// Set a mobile's stats and re-cap its hit points, mana and stamina.
///
/// The one door stats change through, so the three derived pools can never drift
/// from them — a stat gain and a `Command::SetStats` both land here. It is also
/// the door the *skills* a stat lends to are announced from: see
/// [`SkillChanged`]'s own docs for why a stat change is a skill change.
pub fn apply_stats(state: &mut WorldState, entity: EntityId, stats: Stats) {
    // Every skill's drawn value, taken before the stats move. Fifty-eight reads
    // of a table, on a path walked when an operator types `.set` or a stat gains
    // — the alternative is deciding from the scale columns which skills *could*
    // have moved, which is the same table read plus a rule to get wrong.
    let before = drawn_values(state, entity);
    state.registry.insert(entity, stats);
    announce_drawn_moves(state, entity, &before);
    // Strength caps hit points, intelligence mana, dexterity stamina; a lowered
    // cap drags the current value down with it, a raised one leaves room to heal
    // into.
    if let Some(&Hitpoints { current, .. }) = state.registry.get::<Hitpoints>(entity) {
        state.registry.insert(
            entity,
            Hitpoints {
                current: current.min(stats.strength),
                max:     stats.strength,
            },
        );
    }
    if let Some(&Mana { current, .. }) = state.registry.get::<Mana>(entity) {
        state.registry.insert(
            entity,
            Mana {
                current: current.min(stats.intelligence),
                max:     stats.intelligence,
            },
        );
    }
    if let Some(&Stamina { current, .. }) = state.registry.get::<Stamina>(entity) {
        state.registry.insert(
            entity,
            Stamina {
                current: current.min(stats.dexterity),
                max:     stats.dexterity,
            },
        );
    }
}

/// Set a mobile's skill value, in tenths, clamped to that skill's cap.
///
/// `skill` crossed the command queue unchecked (N3's "the queue is a
/// delivery, not a checkpoint"); this is the seam that owns the skill list,
/// so this is where an id past the table is refused — the same shape
/// `set_skill_lock` (`world/src/tick/skills_wire.rs`) already uses.
pub fn set_skill(state: &mut WorldState, serial: Serial, skill: u8, value: u16) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    let Some(skill) = Skill::from_id(skill) else {
        return;
    };
    let mut skills = state.registry.get::<Skills>(entity).cloned().unwrap_or_default();
    let cap = skills.cap(skill).min(state.gameplay.skill_cap);
    let previous = skills.get(skill);
    let raised = value.min(cap);
    skills.set(skill, raised);
    state.registry.insert(entity, skills);
    if raised != previous {
        state.bus.send(SkillChanged {
            entity,
            serial,
            skill,
            previous: SkillValue::new(previous),
            value: SkillValue::new(raised),
        });
    }
}

/// What a window would draw for every skill this mobile has, by id.
///
/// [`skill_value`], not the trained number: the point of taking it is to notice a
/// move that never touched the sheet.
fn drawn_values(state: &WorldState, entity: EntityId) -> [u16; openshard_state::skill::SKILL_COUNT] {
    let mut values = [0u16; openshard_state::skill::SKILL_COUNT];
    for (id, slot) in values.iter_mut().enumerate() {
        // `SKILL_COUNT` is the length of the table, so every id in it is a skill.
        let skill = Skill::from_id(id as u8).expect("an id under SKILL_COUNT is a skill");
        *slot = skill_value(state, entity, skill);
    }
    values
}

/// Announce every skill whose drawn value differs from what `before` recorded.
///
/// The trained number is what rides on the event, and it is the same on both
/// sides — see [`SkillChanged`].
fn announce_drawn_moves(
    state: &mut WorldState,
    entity: EntityId,
    before: &[u16; openshard_state::skill::SKILL_COUNT],
) {
    let Some(serial) = state.registry.serial_of(entity) else {
        return;
    };
    let after = drawn_values(state, entity);
    for (id, (was, is)) in before.iter().zip(after.iter()).enumerate() {
        if was == is {
            continue;
        }
        let skill = Skill::from_id(id as u8).expect("an id under SKILL_COUNT is a skill");
        let trained = SkillValue::new(state.registry.get::<Skills>(entity).map_or(0, |s| s.get(skill)));
        state.bus.send(SkillChanged {
            entity,
            serial,
            skill,
            previous: trained,
            value: trained,
        });
    }
}

/// Set the ceiling on one of a mobile's skills, in tenths, dragging the value
/// down under it if it now sits above. Same unchecked-queue seam as `set_skill`.
pub fn set_skill_cap(state: &mut WorldState, serial: Serial, skill: u8, cap: u16) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    let Some(skill) = Skill::from_id(skill) else {
        return;
    };
    let mut skills = state.registry.get::<Skills>(entity).cloned().unwrap_or_default();
    skills.set_cap(skill, cap);
    let value = skills.get(skill);
    if value > cap {
        skills.set(skill, cap);
    }
    state.registry.insert(entity, skills);
}

/// Use a skill against a difficulty band: roll it, teach from it, announce it.
///
/// The band is ServUO's — under `min_skill` the attempt is beyond the mobile,
/// at `max_skill` it is no challenge — and both are in tenths. Same
/// unchecked-queue seam as `set_skill`.
pub fn use_skill(state: &mut WorldState, serial: Serial, skill: u8, min_skill: i32, max_skill: i32) {
    let Some(entity) = state.registry.entity_of(serial) else {
        return;
    };
    let Some(skill) = Skill::from_id(skill) else {
        return;
    };
    let success = roll_skill_band(state, entity, skill, SkillBand::new(min_skill, max_skill));
    let value = state.registry.get::<Skills>(entity).map_or(0, |s| s.get(skill));
    state.bus.send(SkillUsed {
        entity,
        serial,
        skill,
        success,
        value: SkillValue::new(value),
    });
}

#[cfg(test)]
mod tests {
    use super::{
        PER_MILLE,
        PERCENT,
        SkillValue,
        apply_stats,
        roll_u16,
        set_stats,
    };

    #[test]
    fn stat_mutation_api_carries_stats_as_one_value() {
        let _: fn(
            &mut openshard_state::WorldState,
            openshard_protocol::serial::Serial,
            openshard_state::components::Stats,
        ) = set_stats;
        let _: fn(
            &mut openshard_state::WorldState,
            openshard_entities::EntityId,
            openshard_state::components::Stats,
        ) = apply_stats;
    }

    #[test]
    fn skill_value_keeps_fixed_point_tenths_explicit() {
        let value = SkillValue::new(755);

        assert_eq!(value.raw(), 755);
        assert!(value > SkillValue::new(754));
    }

    #[test]
    fn narrow_roll_stays_below_its_typed_bound() {
        let mut rng = openshard_state::Rng::new(7);
        for bound in [PERCENT, PER_MILLE, std::num::NonZeroU16::MAX] {
            for _ in 0..1000 {
                assert!(roll_u16(&mut rng, bound) < bound.get());
            }
        }
        assert_eq!(roll_u16(&mut rng, std::num::NonZeroU16::MIN), 0);
    }
}
