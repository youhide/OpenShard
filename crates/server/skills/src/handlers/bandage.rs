//! Healing and Veterinary — the skills that come through an item rather than the
//! window, and Lockpicking beside them.
//!
//! ServUO's `Bandage.cs` and `Lockpick.cs`. None of the three has a usable button:
//! the action that uses them is a double-click on the bandage, the lockpick, the
//! thing in your hand. They enter through the same `ItemUsed` seam a pack's potion
//! does, which is why they are here rather than in the button dispatch.
//!
//! A bandage is the one skill in the engine whose **duration is the mechanic**: it
//! takes seconds, the patient can be hit meanwhile, and a fresh bandage restarts
//! it. So unlike Poisoning — whose two-second beat is flavour and is resolved at
//! once — this really does keep a `Bandaging` component and finish on the tick
//! counter.

use openshard_entities::EntityId;
use openshard_protocol::wire::{ClilocId, CursorId, Graphic};
use openshard_state::components::{Bandaging, Ghost, Hitpoints, Lock, Poisoned, Stats};
use openshard_state::{Skill, TICKS_PER_SECOND, TargetPurpose, WorldState};

use crate::check::roll_skill_band;

/// The clean bandage a healer carries — ServUO's `Bandage`, item `0x0E21`.
pub const BANDAGE_GRAPHIC: Graphic = Graphic(0x0E21);
/// A lockpick — ServUO's `Lockpick`, item `0x14FC`.
pub const LOCKPICK_GRAPHIC: Graphic = Graphic(0x14FC);

const HEAL_WHOM: &str = "Who will you use the bandage on?";
const NOT_DAMAGED: &str = "That being is not damaged!";
const BEGIN_BANDAGES: &str = "You begin applying the bandages.";
const FINISH_BANDAGES: &str = "You finish applying the bandages.";
const BARELY_HELP: &str = "You apply the bandages, but they barely help.";
const CURED: &str = "You have cured the target of all poisons!";
const RESURRECTED: &str = "You are able to resurrect your patient.";
const NOT_RESURRECTED: &str = "You are unable to resurrect your patient.";
const CANNOT_HEAL: &str = "You cannot heal that.";

/// "That did not work." — a lockpick that broke, or a lock that held.
const PICK_FAILED: ClilocId = ClilocId(502_075);
/// "You broke the lockpick."
const PICK_BROKE: ClilocId = ClilocId(502_074);
/// "The lock quickly yields to your skill."
const LOCK_YIELDS: ClilocId = ClilocId(502_076);
/// "You do not have the skill to pick that lock."
const LOCK_TOO_HARD: ClilocId = ClilocId(502_072);
/// "That does not appear to be locked."
const NOT_LOCKED: ClilocId = ClilocId(502_069);
/// "What do you want to pick?"
const PICK_WHAT: ClilocId = ClilocId(502_068);

/// The Healing a cure needs before it is even attempted, in tenths.
const CURE_NEEDS: u16 = 600;
/// The Healing a resurrection needs, in tenths.
const RESURRECT_NEEDS: u16 = 800;
/// How far a healer can reach — ServUO's `Target(1, …)`.
pub(super) const HEAL_RANGE: u32 = 1;

/// Start a bandage: raise the cursor that asks who it is for.
///
/// Returns whether the item was one this handles at all, so the caller knows
/// whether the pack should still hear about the click.
pub fn use_bandage(state: &mut WorldState, healer: EntityId, bandage: EntityId) -> bool {
    let Some((connection, serial)) = super::client_of(state, healer) else {
        return true;
    };
    state.raise_target(
        healer,
        TargetPurpose::SkillSecond {
            skill: Skill::Healing,
            first: bandage,
        },
    );
    state.system_message(healer, HEAL_WHOM);
    super::send_object_cursor(state, connection, CursorId(serial.raw()));
    true
}

/// The bandage's cursor came back: begin the work, or refuse.
pub(super) fn begin_heal(
    state: &mut WorldState,
    healer: EntityId,
    bandage: EntityId,
    patient: EntityId,
) -> Option<BandageStarted> {
    if !state.registry.has::<openshard_state::components::Body>(patient) {
        state.system_message(healer, CANNOT_HEAL);
        return None;
    }
    let dead = state.registry.has::<Ghost>(patient);
    let hurt = state
        .registry
        .get::<Hitpoints>(patient)
        .is_some_and(|hits| hits.current < hits.max);
    let poisoned = state.registry.has::<Poisoned>(patient);
    if !dead && !hurt && !poisoned {
        state.system_message(healer, NOT_DAMAGED);
        return None;
    }
    // Pre-AoS timing, off the healer's dexterity: patching yourself is slow, and a
    // resurrection is five seconds longer than a wound.
    let dex = state
        .registry
        .get::<Stats>(healer)
        .map_or(100, |stats| stats.dexterity);
    let seconds = if healer == patient {
        // `9.4 + 0.6 * ((120 - dex) / 10)`, in tenths of a second so the fraction
        // is exact.
        94 + 6 * (120i32 - i32::from(dex)).max(0) / 10
    } else {
        let base = if dex >= 100 {
            30
        } else if dex >= 40 {
            40
        } else {
            50
        };
        base + if dead { 50 } else { 0 }
    };
    let ticks = u64::try_from(seconds).unwrap_or(50) * TICKS_PER_SECOND / 10;
    state.registry.insert(
        healer,
        Bandaging {
            patient,
            done_at: state.ticks + ticks,
        },
    );
    state.system_message(healer, BEGIN_BANDAGES);
    // The bandage is spent at the *start*, as ServUO consumes it when the work
    // begins — walking away does not get it back.
    Some(BandageStarted { bandage })
}

/// A bandage was used up starting a heal. `items` removes it; this crate only says
/// so, the split every other intent here uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BandageStarted {
    /// The bandage that was spent.
    pub bandage: EntityId,
}

/// What a finished bandage did, for the tick to apply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BandageFinished {
    /// Who was patched up.
    pub patient: EntityId,
    /// Hit points to give back. Zero when the attempt only cured or resurrected.
    pub healed: u16,
    /// Whether the poison was drawn out.
    pub cured: bool,
    /// Whether the patient was brought back.
    pub resurrected: bool,
}

/// Finish every bandage whose time is up, and say what each did.
///
/// Runs on the tick counter like every other timer in the engine. The *effects* —
/// hit points, a cure, a resurrection — are returned rather than applied, because
/// each of those is another crate's one door.
pub fn finish_bandages(state: &mut WorldState) -> Vec<BandageFinished> {
    let now = state.ticks;
    let due: Vec<(EntityId, EntityId)> = state
        .registry
        .query::<Bandaging>()
        .filter(|(_, work)| now >= work.done_at)
        .map(|(healer, work)| (healer, work.patient))
        .collect();
    let mut finished = Vec::new();
    for (healer, patient) in due {
        state.registry.remove::<Bandaging>(healer);
        // The patient may have died, walked off or been healed meanwhile.
        if state.registry.serial_of(patient).is_none() {
            continue;
        }
        if !super::within(state, healer, patient, HEAL_RANGE) {
            continue;
        }
        // Veterinary for a creature, Healing for a person — ServUO's
        // `GetPrimarySkill`, which is the whole difference between the two skills.
        let skill = if state.registry.has::<openshard_state::components::Client>(patient) {
            Skill::Healing
        } else {
            Skill::Veterinary
        };
        let healing = crate::skill_value(state, healer, skill);
        let anatomy = crate::skill_value(state, healer, Skill::Anatomy);

        if state.registry.has::<Ghost>(patient) {
            // A resurrection: 80.0 in both, then `(healing - 68) / 50`.
            let able = healing >= RESURRECT_NEEDS && anatomy >= RESURRECT_NEEDS;
            let chance = (i32::from(healing) - 680) * 1000 / 500;
            if able && chance > i32::try_from(state.rng.below(1000)).unwrap_or(0) {
                state.system_message(healer, RESURRECTED);
                finished.push(BandageFinished {
                    patient,
                    healed: 0,
                    cured: false,
                    resurrected: true,
                });
            } else {
                state.system_message(healer, NOT_RESURRECTED);
            }
            continue;
        }

        if let Some(&Poisoned { level, .. }) = state.registry.get::<Poisoned>(patient) {
            // A cure: 60.0 in both, then `(healing - 30)/50 - level/10`.
            let able = healing >= CURE_NEEDS && anatomy >= CURE_NEEDS;
            let chance = (i32::from(healing) - 300) * 1000 / 500 - i32::from(level.get()) * 100;
            if able && chance > i32::try_from(state.rng.below(1000)).unwrap_or(0) {
                state.system_message(healer, CURED);
                finished.push(BandageFinished {
                    patient,
                    healed: 0,
                    cured: true,
                    resurrected: false,
                });
                continue;
            }
            state.system_message(healer, BARELY_HELP);
            continue;
        }

        // An ordinary wound. `(healing + 10) / 100` to work at all, and the amount
        // is `anatomy/5 + healing/5 + 3` to `anatomy/5 + healing/2 + 10` — pre-AoS,
        // so Anatomy is worth as much as Healing to a field surgeon.
        let chance = (i32::from(healing) + 100) / 10;
        let took = roll_skill_band(state, healer, skill, crate::SkillBand::new(0, 1000))
            && chance > i32::try_from(state.rng.below(1000)).unwrap_or(0);
        if !took {
            state.system_message(healer, BARELY_HELP);
            continue;
        }
        let min = anatomy / 50 + healing / 50 + 3;
        let max = anatomy / 50 + healing / 20 + 10;
        let span = u32::from(max.saturating_sub(min)) + 1;
        let healed = min + u16::try_from(state.rng.below(span)).unwrap_or(0);
        state.system_message(healer, FINISH_BANDAGES);
        finished.push(BandageFinished {
            patient,
            healed,
            cured: false,
            resurrected: false,
        });
    }
    finished
}

/// Start a lockpick: raise the cursor that asks which lock.
pub fn use_lockpick(state: &mut WorldState, picker: EntityId, pick: EntityId) -> bool {
    let Some((connection, serial)) = super::client_of(state, picker) else {
        return true;
    };
    state.raise_target(
        picker,
        TargetPurpose::SkillSecond {
            skill: Skill::Lockpicking,
            first: pick,
        },
    );
    state.localized_message(picker, PICK_WHAT, "");
    super::send_object_cursor(state, connection, CursorId(serial.raw()));
    true
}

/// The lockpick's cursor came back: turn the lock, or break the pick.
pub(super) fn pick_lock(
    state: &mut WorldState,
    picker: EntityId,
    pick: EntityId,
    target: EntityId,
) -> Option<LockpickBroke> {
    let Some(&Lock {
        required_skill,
        max_skill,
        ..
    }) = state.registry.get::<Lock>(target)
    else {
        state.localized_message(picker, NOT_LOCKED, "");
        return None;
    };
    let skill = Skill::Lockpicking;
    // Below the lock's own requirement nothing happens but a broken pick — ServUO
    // refuses outright rather than letting a novice grind a vault open.
    if crate::skill_value(state, picker, skill) < required_skill {
        state.localized_message(picker, LOCK_TOO_HARD, "");
        return None;
    }
    if roll_skill_band(
        state,
        picker,
        skill,
        crate::SkillBand::new(i32::from(required_skill), i32::from(max_skill)),
    ) {
        state.registry.remove::<Lock>(target);
        state.localized_message(picker, LOCK_YIELDS, "");
        return None;
    }
    state.localized_message(picker, PICK_FAILED, "");
    state.localized_message(picker, PICK_BROKE, "");
    Some(LockpickBroke { pick })
}

/// A lockpick snapped. Removing it is `items`' door.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LockpickBroke {
    /// The pick that broke.
    pub pick: EntityId,
}
