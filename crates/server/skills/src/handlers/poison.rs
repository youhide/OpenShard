//! Poisoning, and the skill that finds out: Taste Identification.
//!
//! ServUO's `Poisoning` and `TasteID`. The pair belongs in one file because they
//! are two ends of the same fact — a [`PoisonCharges`] on an item — one skill
//! putting it there and the other reading it off.
//!
//! Poisoning is the engine's only **two-cursor** skill: it asks for the potion,
//! then for the blade. The first answer rides on
//! [`TargetPurpose::SkillSecond`](openshard_state::TargetPurpose::SkillSecond) and is
//! re-checked when the second lands, so a potion drunk or dropped while the second
//! cursor was up poisons nothing.

use openshard_entities::EntityId;
use openshard_protocol::serial::Serial;
use openshard_protocol::wire::{ClilocId, CursorId, Hue, SoundId};
use openshard_protocol::world::PoisonLevel;
use openshard_state::components::{
    Amount, Drawn, EMPTY_BOTTLE_GRAPHIC, ItemKind, POISON_POTION_GRAPHIC, PoisonCharges,
};
use openshard_state::weapon::{WeaponKind, weapon_data, weapon_data_for_kind};
use openshard_state::{Skill, TargetPurpose, WorldState};

use crate::check::roll_skill_band;

/// "To what do you wish to apply the poison?" — the prompt for the second cursor.
const APPLY_TO_WHAT: ClilocId = ClilocId(502_142);
/// "That is not a poison potion."
const NOT_A_POTION: ClilocId = ClilocId(502_139);
/// "You cannot poison that! You can only poison bladed or piercing weapons, food
/// or drink." — the pre-AoS refusal.
const CANNOT_POISON: ClilocId = ClilocId(502_145);
/// "You apply the poison."
const APPLIED: ClilocId = ClilocId(1_010_517);
/// "You fail to apply a sufficient dose of poison on the blade" — the slashing form.
const FAILED_ON_BLADE: ClilocId = ClilocId(1_010_516);
/// "You fail to apply a sufficient dose of poison" — everything else.
const FAILED: ClilocId = ClilocId(1_010_518);
/// "You make a grave mistake while applying the poison."
const GRAVE_MISTAKE: ClilocId = ClilocId(502_148);
/// The sound of a bottle being uncorked over a blade — ServUO's `PlaySound(0x4F)`.
const APPLY_SOUND: SoundId = SoundId(0x004F);
/// What poisoning yourself costs in karma on success — ServUO's `AwardKarma(-20)`.
/// Coating a blade is not a nice thing to do.
const KARMA_COST: i32 = -20;
/// The skill below which a fumble can poison the poisoner, in tenths.
const FUMBLE_BELOW: u16 = 800;
/// One chance in twenty, ServUO's `Utility.Random(20) == 0`.
const FUMBLE_ONE_IN: u32 = 20;

/// "It appears to have poison smeared on it."
const TASTES_POISONED: ClilocId = ClilocId(1_038_284);
/// "You detect nothing unusual about this substance."
const TASTES_CLEAN: ClilocId = ClilocId(1_010_600);
/// "You cannot discern anything about this substance." — the failed roll.
const CANNOT_DISCERN: ClilocId = ClilocId(502_823);
/// "You feel that such an action would be inappropriate." — tasting a person.
const INAPPROPRIATE: ClilocId = ClilocId(502_816);

/// The Poisoning band each poison strength is applied against, in tenths — the
/// potion classes' `MinPoisoningSkill`/`MaxPoisoningSkill`, indexed by level.
///
/// Lethal is ServUO's fifth level and has no potion class of its own in the classic
/// set; it takes deadly's band, which is the honest reading of a table that stops
/// at four entries.
const POISON_BANDS: [(i32, i32); 5] = [(0, 600), (300, 700), (600, 1000), (800, 1000), (800, 1000)];

/// How many doses coating a blade leaves — ServUO's `18 - level * 2`.
#[must_use]
pub(super) fn charges_for(level: PoisonLevel) -> u16 {
    18u16.saturating_sub(u16::from(level.get()) * 2)
}

/// The first cursor's answer: the potion. Puts up the second cursor if it is one.
pub(super) fn chose_potion(state: &mut WorldState, actor: EntityId, potion: EntityId) {
    if !is_poison_potion(state, potion) {
        state.localized_message(actor, NOT_A_POTION, "");
        return;
    }
    let Some(&openshard_state::components::Client { connection, .. }) =
        state.registry.get::<openshard_state::components::Client>(actor)
    else {
        return;
    };
    let Some(serial) = state.registry.serial_of(actor) else {
        return;
    };
    state.raise_target(
        actor,
        TargetPurpose::SkillSecond {
            skill: Skill::Poisoning,
            first: potion,
        },
    );
    state.localized_message(actor, APPLY_TO_WHAT, "");
    super::send_object_cursor(state, connection, CursorId(serial.raw()));
}

/// The second cursor's answer: what to smear it on.
pub(super) fn apply_to(state: &mut WorldState, actor: EntityId, potion: EntityId, target: EntityId) {
    // The potion may have gone while the cursor was up — drunk, dropped, sold.
    let Some(&PoisonCharges { level, .. }) = state.registry.get::<PoisonCharges>(potion) else {
        return;
    };
    if !is_poison_potion(state, potion) {
        return;
    }
    if !can_be_poisoned(state, target) {
        state.localized_message(actor, CANNOT_POISON, "");
        return;
    }
    // The bottle is spent whether the application takes or not: ServUO consumes it
    // before the roll and hands back the empty.
    spend_potion(state, potion);
    state.play_sound(actor, APPLY_SOUND);

    let skill = Skill::Poisoning;
    let (min, max) = POISON_BANDS[usize::from(level.get())];
    if roll_skill_band(state, actor, skill, crate::SkillBand::new(min, max)) {
        state.registry.insert(
            target,
            PoisonCharges {
                level,
                charges: charges_for(level),
            },
        );
        state.localized_message(actor, APPLIED, "");
        // Coating a blade is not a nice thing to do, and the karma table says so.
        openshard_state::title::award_karma(state, actor, KARMA_COST);
        return;
    }
    // A fumble under grandmaster can poison the poisoner — one chance in twenty,
    // rolled on the world's own generator so a botch replays.
    let trained = state
        .registry
        .get::<openshard_state::Skills>(actor)
        .map_or(0, |s| s.get(skill));
    if trained < FUMBLE_BELOW && state.rng.below(FUMBLE_ONE_IN) == 0 {
        state.localized_message(actor, GRAVE_MISTAKE, "");
        if let Some(serial) = state.registry.serial_of(actor) {
            state.bus.send(PoisonedSelf {
                entity: actor,
                serial,
                level,
            });
        }
        return;
    }
    // Two failure lines: a blade gets its own.
    let slashing = weapon_kind_of(state, target) == Some(WeaponKind::Slashing);
    state.localized_message(actor, if slashing { FAILED_ON_BLADE } else { FAILED }, "");
}

/// A poisoner poisoned themselves. Emitted rather than applied, because applying
/// poison is `combat`'s door and this crate sits below it — the tick reads this and
/// calls it, the same decide-then-apply split `ai::think_one` uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PoisonedSelf {
    /// Who fumbled.
    pub entity: EntityId,
    /// Their wire identity.
    pub serial: Serial,
    /// The strength of the poison they were handling.
    pub level: PoisonLevel,
}

/// Taste Identification: whether a thing has poison on it.
pub(super) fn taste_id(state: &mut WorldState, actor: EntityId, target: EntityId) {
    // Tasting a person is not a thing one does.
    if state.registry.has::<openshard_state::components::Body>(target) {
        state.localized_message(actor, INAPPROPRIATE, "");
        return;
    }
    let skill = Skill::TasteId;
    if !roll_skill_band(state, actor, skill, crate::SkillBand::new(0, 1000)) {
        state.private_overhead_cliloc(actor, target, CANNOT_DISCERN, "");
        return;
    }
    let poisoned = state.registry.has::<PoisonCharges>(target);
    let line = if poisoned { TASTES_POISONED } else { TASTES_CLEAN };
    state.private_overhead_cliloc(actor, target, line, "");
}

/// Whether an item is a bottle of poison — the shared potion graphic *and* a dose
/// on it. ServUO asks `targeted is BasePoisonPotion`, which is one class per
/// strength; here all four are the same bottle and the strength is on the item, so
/// a bottle with no poison in it is not a poison potion.
fn is_poison_potion(state: &WorldState, item: EntityId) -> bool {
    state
        .registry
        .get::<Drawn>(item)
        .is_some_and(|graphic| graphic.id == POISON_POTION_GRAPHIC)
        && state.registry.has::<PoisonCharges>(item)
}

/// Whether poison will stay on the target — pre-AoS, a one-handed bladed or
/// piercing weapon.
///
/// ServUO also allows food and drink, which this engine has no component for yet;
/// a poisoned meal is a slice of its own and its absence is why the refusal line
/// here is the weapon one.
fn can_be_poisoned(state: &WorldState, target: EntityId) -> bool {
    matches!(
        weapon_kind_of(state, target),
        Some(WeaponKind::Slashing | WeaponKind::Piercing)
    )
}

/// The weapon kind of an item, if the core table knows it as a weapon at all.
fn weapon_kind_of(state: &WorldState, item: EntityId) -> Option<WeaponKind> {
    match state.registry.get::<ItemKind>(item) {
        Some(kind) => weapon_data_for_kind(kind.0),
        None => state
            .registry
            .get::<Drawn>(item)
            .and_then(|graphic| weapon_data(graphic.id)),
    }
    .map(|weapon| weapon.kind)
}

/// Take one dose out of the potion and leave the empty bottle behind.
///
/// A stack of potions loses one from the stack; a single bottle becomes an empty one
/// in place, which is ServUO's `Consume` plus the `Bottle` it hands back, done
/// without needing to reach into the backpack. The client redraws it the next time
/// the container is opened — the same limitation every live container update has.
fn spend_potion(state: &mut WorldState, potion: EntityId) {
    let amount = state.registry.get::<Amount>(potion).map_or(1, |a| a.0);
    if amount > 1 {
        state.registry.insert(potion, Amount(amount - 1));
        return;
    }
    // The last one: the bottle stays where it was (in the pack, or on the ground)
    // and is simply empty now — no poison, no label, no dose.
    state.registry.remove::<PoisonCharges>(potion);
    state.registry.insert(
        potion,
        Drawn {
            id: EMPTY_BOTTLE_GRAPHIC,
            hue: Hue(0),
        },
    );
}
