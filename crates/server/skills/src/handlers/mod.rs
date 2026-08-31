//! What each usable skill actually does.
//!
//! One module per family, and one dispatch — [`start`] for the button, [`on_target`]
//! for the cursor's answer. The split is ServUO's `SkillUseCallback` and the
//! `Target` it puts up: pressing the button rarely *does* anything by itself, it
//! asks a question, and the answer arrives a packet later.
//!
//! A skill missing from [`ASKS`] is one whose core behaviour is not built yet:
//! it still passes every gate, still announces
//! [`SkillRequested`](crate::SkillRequested), and a pack can give it meaning. That
//! is the difference between "the core has no opinion" and "the client cannot do
//! this", which is cliloc 500014 and is decided a step earlier.

mod animal;
mod appraise;
mod bandage;
mod bard;
mod forensics;
mod harvest;
mod lore;
mod mind;
mod poison;
mod social;
mod stealth;
mod taming;

pub use bandage::{
    BANDAGE_GRAPHIC,
    BandageFinished,
    BandageStarted,
    LOCKPICK_GRAPHIC,
    LockpickBroke,
    finish_bandages,
    use_bandage,
    use_lockpick,
};
pub use bard::{
    InstrumentSpent,
    expire_songs,
    play_instrument,
};
pub use harvest::{
    HarvestTarget,
    Harvested,
    ToolWorn,
    advance_harvests,
    begin_harvest,
    resolve_harvest_target,
    use_tool,
};
use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::localized::begging;
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::target::{
    TargetCursor,
    TargetKind,
};
use openshard_protocol::wire::{
    ClilocId,
    CursorId,
};
use openshard_state::components::{
    Client,
    HearsGhosts,
    Position,
};
use openshard_state::{
    Skill,
    TargetPurpose,
    WorldState,
    in_range,
};
pub use poison::PoisonedSelf;
pub use social::Begged;
pub use stealth::{
    Stolen,
    snooping,
};
pub use taming::{
    MAX_FOLLOWERS,
    Tamed,
    followers_of,
};

/// A skill that answers a question about something: the line it asks with, and how
/// far the cursor reaches.
///
/// Both numbers are ServUO's, per skill, out of each handler's `OnUse` and the
/// `Target(range, …)` it constructs — they are *not* one shared number. Arms Lore
/// wants the thing in your hands (2), Anatomy across a room (8), Forensic
/// Evaluation as far as you can see a body (10).
struct Ask {
    /// The cliloc the client is prompted with — "Whom shall I examine?".
    prompt: ClilocId,
    /// How far the answer may be, in tiles. Re-checked server-side when it lands.
    range:  u32,
}

/// The prompt and reach of every skill that raises an object cursor.
///
/// A skill absent from this table has no core cursor behaviour yet.
#[rustfmt::skip]
const ASKS: &[(Skill, Ask)] = &[
    (Skill::Anatomy,   Ask { prompt: ClilocId(500_321), range: 8 }),  // Whom shall I examine?
    (Skill::EvalInt,   Ask { prompt: ClilocId(500_906), range: 8 }),  // What do you wish to evaluate?
    (Skill::ArmsLore,  Ask { prompt: ClilocId(500_349), range: 2 }),  // What item do you wish to get information about?
    (Skill::ItemId,    Ask { prompt: ClilocId(500_343), range: 8 }),  // What do you wish to appraise and identify?
    (Skill::Forensics, Ask { prompt: ClilocId(501_000), range: 10 }), // Show me the crime.
    (Skill::TasteId,   Ask { prompt: ClilocId(502_807), range: 2 }),  // What would you like to taste?
    (Skill::Poisoning, Ask { prompt: ClilocId(502_137), range: 2 }),  // Select the poison you wish to use
    (Skill::Begging,   Ask { prompt: begging::PROMPT, range: 2 }),
    (Skill::RemoveTrap, Ask { prompt: ClilocId(502_368), range: 2 }), // Which trap will you attempt to disarm?
    (Skill::Stealing,  Ask { prompt: ClilocId(502_698), range: stealth::STEAL_RANGE }), // What do you wish to steal?
    (Skill::AnimalTaming, Ask { prompt: ClilocId(502_789), range: taming::TAME_RANGE }), // Tame which animal?
    (Skill::AnimalLore, Ask { prompt: ClilocId(500_328), range: 8 }), // What animal should I look at?
];

/// The ask for a skill, if the core raises a cursor for it.
fn ask_for(skill: Skill) -> Option<&'static Ask> {
    ASKS.iter()
        .find(|(candidate, _)| *candidate == skill)
        .map(|(_, ask)| ask)
}

/// Run a skill's core behaviour. Returns whether the core knew what to do with it.
///
/// Called after the gates and after the event, so a pack that wants a skill to
/// mean something else has already seen it and the core is the fallback, not the
/// competition.
pub(crate) fn start(state: &mut WorldState, actor: EntityId, skill: Skill) -> bool {
    // A skill that asks a question puts up its cursor and waits; the answer arrives
    // in [`on_target`] a packet later.
    if let Some(ask) = ask_for(skill) {
        // Remove Trap is the one that refuses *before* the cursor: ServUO checks
        // the two skills it leans on in `OnUse` and never raises a target for
        // someone who could not disarm anything anyway.
        if skill == Skill::RemoveTrap && !social::may_remove_traps(state, actor) {
            return true;
        }
        return raise_cursor(state, actor, skill, ask.prompt);
    }
    // And a skill a mobile turns on itself resolves here and now.
    match skill {
        Skill::Meditation => {
            mind::meditation(state, actor);
            true
        }
        Skill::SpiritSpeak => {
            mind::spirit_speak(state, actor);
            true
        }
        Skill::Hiding => {
            stealth::hiding(state, actor);
            true
        }
        Skill::Stealth => {
            stealth::stealth(state, actor);
            true
        }
        Skill::DetectHidden => {
            stealth::detect_hidden(state, actor);
            true
        }
        // The bard skills raise their own cursors, because the reach is the bard's
        // (`8 + value/15`) rather than a constant, and each wants an instrument in
        // the pack before it asks anything.
        Skill::Peacemaking | Skill::Provocation | Skill::Discordance => bard::start(state, actor, skill),
        _ => false,
    }
}

/// Let a Spirit Speak contact with the netherworld lapse, telling whoever held it.
///
/// The counterpart of every other expiry the tick runs (`magic::expire_frozen`,
/// `expire_buffs`), on the tick counter for the same reason: a contact measured
/// against a wall clock would not replay.
pub fn expire_ghost_contact(state: &mut WorldState) {
    let now = state.ticks;
    let lapsed: Vec<EntityId> = state
        .registry
        .query::<HearsGhosts>()
        .filter(|(_, contact)| now >= contact.until)
        .map(|(entity, _)| entity)
        .collect();
    for entity in lapsed {
        state.registry.remove::<HearsGhosts>(entity);
        mind::contact_faded(state, entity);
    }
}

/// A skill's cursor came back with something. Runs the skill against it.
///
/// Returns a theft to carry out, if the skill was Stealing and it resolved: moving
/// an item into a pack is `items`' door and turning a thief criminal is `combat`'s,
/// so this decides and the tick applies — the split `ai::think_one` uses.
pub fn on_target(state: &mut WorldState, actor: EntityId, skill: Skill, target: Option<Serial>) -> Outcome {
    // The bard skills judge their own reach, which widens with the skill, so they
    // are not in the fixed table.
    if matches!(
        skill,
        Skill::Peacemaking | Skill::Provocation | Skill::Discordance
    ) {
        let Some(target) = target.and_then(|s| state.registry.entity_of(s)) else {
            return Outcome::default();
        };
        if !within(state, actor, target, bard::bard_range(state, actor, skill)) {
            return Outcome::default();
        }
        match skill {
            Skill::Peacemaking => bard::peacemaking(state, actor, target),
            Skill::Provocation => bard::provoke_first(state, actor, target),
            _ => bard::discordance(state, actor, target),
        }
        return Outcome::default();
    }
    let Some(ask) = ask_for(skill) else {
        return Outcome::default();
    };
    let Some(target) = target.and_then(|s| state.registry.entity_of(s)) else {
        return Outcome::default();
    };
    // Checked server-side even though the cursor was raised with a range: the range
    // on a `0x6C` is the client's own courtesy, and a client is never the judge of
    // reach — the same rule `ITEM_REACH` holds for a lift.
    if !within(state, actor, target, ask.range) {
        return Outcome::default();
    }
    match skill {
        Skill::Stealing => {
            return Outcome {
                stolen: stealth::stealing(state, actor, target),
                ..Outcome::default()
            };
        }
        Skill::AnimalTaming => {
            return Outcome {
                tamed: taming::taming(state, actor, target),
                ..Outcome::default()
            };
        }
        _ => {}
    }
    match skill {
        Skill::Anatomy => lore::anatomy(state, actor, target),
        Skill::EvalInt => lore::eval_int(state, actor, target),
        Skill::ArmsLore => appraise::arms_lore(state, actor, target),
        Skill::ItemId => appraise::item_id(state, actor, target),
        Skill::Forensics => forensics::forensics(state, actor, target),
        Skill::AnimalLore => animal::animal_lore(state, actor, target),
        Skill::TasteId => poison::taste_id(state, actor, target),
        // Poisoning is the one skill that asks twice: this was the potion, and the
        // next cursor asks what to put it on.
        Skill::Poisoning => poison::chose_potion(state, actor, target),
        Skill::Begging => social::begging(state, actor, target),
        Skill::RemoveTrap => social::remove_trap(state, actor, target),
        _ => {}
    }
    Outcome::default()
}

/// What a resolved skill wants the tick to do, where the doing belongs to another
/// crate. Empty for the great majority, which finish where they stand.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Outcome {
    /// A theft to carry out: `items` moves it, `combat` flags the thief.
    pub stolen: Option<Stolen>,
    /// A taming: `npc` owns what a creature is, so it makes the pet.
    pub tamed:  Option<Tamed>,
}

/// A skill's *second* cursor came back. Only Poisoning asks twice.
///
/// The reach is the same as the first ask's, and re-checked here for the same
/// reason: a `0x6C` range is the client's courtesy, never the judge.
pub fn on_second_target(
    state: &mut WorldState,
    actor: EntityId,
    skill: Skill,
    first: EntityId,
    target: Option<Serial>,
) {
    if skill == Skill::Provocation {
        let range = bard::bard_range(state, actor, skill);
        if let Some(victim) = target.and_then(|s| state.registry.entity_of(s)) {
            if within(state, actor, victim, range) {
                bard::provoke_second(state, actor, first, victim);
            }
        }
        return;
    }
    let Some(ask) = ask_for(skill) else {
        return;
    };
    let Some(target) = target.and_then(|s| state.registry.entity_of(s)) else {
        return;
    };
    if !within(state, actor, target, ask.range) {
        return;
    }
    if skill == Skill::Poisoning {
        poison::apply_to(state, actor, first, target);
    }
}

/// The second cursor of an *item*-started skill — a bandage or a lockpick, whose
/// first "answer" is the item itself.
///
/// Returned rather than applied for the usual reason: spending a bandage is
/// `items`' door, and so is snapping a pick.
pub fn on_item_target(
    state: &mut WorldState,
    actor: EntityId,
    skill: Skill,
    item: EntityId,
    target: Option<Serial>,
) -> (Option<BandageStarted>, Option<LockpickBroke>) {
    let Some(target) = target.and_then(|s| state.registry.entity_of(s)) else {
        return (None, None);
    };
    if !within(state, actor, target, bandage::HEAL_RANGE) {
        return (None, None);
    }
    match skill {
        Skill::Healing => (bandage::begin_heal(state, actor, item, target), None),
        Skill::Lockpicking => (None, bandage::pick_lock(state, actor, item, target)),
        _ => (None, None),
    }
}

/// A mobile's connection and serial, for the handlers that raise their own
/// cursor.
pub(super) fn client_of(state: &WorldState, mobile: EntityId) -> Option<(ConnectionId, Serial)> {
    let client = state.registry.get::<Client>(mobile)?;
    let serial = state.registry.serial_of(mobile)?;
    Some((client.connection, serial))
}

pub(super) fn send_object_cursor(state: &mut WorldState, connection: ConnectionId, cursor_id: CursorId) {
    state.send_packet(
        connection,
        &ServerPacket::TargetCursor(TargetCursor {
            cursor_id,
            kind: TargetKind::Object,
        }),
    );
}

/// Whether two things are on the same facet and within `range` tiles.
pub(super) fn within(state: &WorldState, a: EntityId, b: EntityId, range: u32) -> bool {
    let (Some(&Position(at)), Some(&Position(other))) = (
        state.registry.get::<Position>(a),
        state.registry.get::<Position>(b),
    ) else {
        return false;
    };
    state.facet_of(a) == state.facet_of(b) && in_range(at, other, range)
}

/// Put up a cursor that must pick an object, remembering which skill asked, and
/// prompt the asker with the skill's own line. Returns whether a cursor went up —
/// a creature has none, and a skill that cannot ask has not started.
pub(super) fn raise_cursor(state: &mut WorldState, actor: EntityId, skill: Skill, prompt: ClilocId) -> bool {
    let Some(&Client { connection, .. }) = state.registry.get::<Client>(actor) else {
        return false; // a creature has no cursor to raise
    };
    let Some(serial) = state.registry.serial_of(actor) else {
        return false;
    };
    state.raise_target(actor, TargetPurpose::Skill { skill });
    state.localized_message(actor, prompt, "");
    send_object_cursor(state, connection, CursorId(serial.raw()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ask_is_a_skill_the_window_can_press() {
        // A cursor for a skill the button refuses would never be raised: the gate
        // runs first. So each row here must be a skill ServUO marks usable.
        for (skill, _) in ASKS {
            let info = skill.info();
            assert!(info.usable, "{} is not usable from the window", info.name);
        }
    }

    #[test]
    fn no_skill_asks_twice() {
        for (i, (skill, _)) in ASKS.iter().enumerate() {
            for (other, _) in &ASKS[i + 1..] {
                assert_ne!(skill.id(), other.id(), "{skill:?} appears twice");
            }
        }
    }
}
