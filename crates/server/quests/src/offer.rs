//! Offering a quest, and what happens to the answer.
//!
//! ServUO's `MondainQuester.OnTalk`, in order: finish a delivery, then show a
//! quest already in progress, then check the cap, then offer a new one. The order
//! matters — a giver you already have a quest from must talk about *that* quest
//! rather than offering another.

use openshard_entities::EntityId;
use openshard_protocol::serial::Serial;
use openshard_state::components::{
    DoneQuest,
    QuestGiver,
    QuestLog,
    QuestState,
};
use openshard_state::quest::{
    ObjectiveKind,
    QuestKey,
};
use openshard_state::{
    QuestGumpContext,
    QuestSection,
    TICKS_PER_SECOND,
    WorldState,
    WorldTick,
};

use crate::events::{
    QuestAccepted,
    QuestRefused,
    QuestResigned,
};
use crate::gump::{
    self,
    sound,
};
use crate::turnin;

/// How many quests one character may have at once. ServUO's `QuestLimitReached`.
pub const QUEST_LIMIT: usize = 10;

/// A player double-clicked or spoke to a mobile: if it gives quests, say
/// something about them.
///
/// Returns whether the mobile was a quest giver at all, so a caller can fall
/// through to whatever else a click on it means.
pub fn talk_to(state: &mut WorldState, player: EntityId, giver: EntityId) -> bool {
    // A delivery first, before anything asks whether this mobile gives quests —
    // the destination of a delivery is usually somebody else's giver, or a plain
    // vendor that gives none. ServUO's `OnTalk` opens the same way.
    let delivered = crate::progress::deliver_to(state, player, giver);

    // Note there is no escortable branch here. An escort is a *quest* — ServUO
    // starts the follow in `BaseQuest.OnAccept`, never on the click — so an
    // escortable reaches this the ordinary way, as the giver of a quest with an
    // escort objective. Starting it on the double-click instead made an NPC
    // wander off after whoever glanced at it, with no offer, no log entry and no
    // way to say no.
    let Some(keys) = state.registry.get::<QuestGiver>(giver).map(|q| q.keys.clone()) else {
        return delivered;
    };
    let giver_serial = state.registry.serial_of(giver);

    // A quest from this giver that is already in progress talks about itself:
    // finished ones offer the turn-in, unfinished ones get the giver's nudge.
    // ServUO's `QuestHelper.InProgress`.
    for key in &keys {
        let held = state
            .registry
            .get::<QuestLog>(player)
            .and_then(|log| log.active_quest(key))
            .is_some();
        if !held {
            continue;
        }
        let section = if turnin::is_complete(state, player, key) {
            QuestSection::Complete
        } else if failed(state, player, key) {
            QuestSection::Failed
        } else {
            QuestSection::InProgress
        };
        gump::show(state, player, gump::log_context(key, section, giver_serial));
        return true;
    }

    // Otherwise the first quest it may still offer.
    let Some(key) = keys
        .iter()
        .find(|key| can_offer(state, player, key) && crate::log::offerable(state, key, giver_serial))
    else {
        return true; // a giver with nothing left to give is still a giver
    };
    if quest_count(state, player) >= QUEST_LIMIT {
        state.system_message(player, "You may not take on any more quests.");
        return true;
    }
    offer(state, player, key, giver_serial);
    true
}

/// Put a quest in front of a player.
pub fn offer(state: &mut WorldState, player: EntityId, key: &QuestKey, giver: Option<Serial>) {
    if state.quests.get(key).is_none() {
        return;
    }
    gump::show(
        state,
        player,
        QuestGumpContext {
            quest: Some(key.clone()),
            section: QuestSection::Description,
            offer: true,
            completed: false,
            giver,
        },
    );
}

/// Whether a player may be offered a quest: they are not already on it, and any
/// cooldown from finishing it before has run out.
///
/// ServUO's `QuestHelper.CanOffer`, minus the chain rules (quest chains are
/// deferred) and the objective-collision check.
#[must_use]
pub fn can_offer(state: &WorldState, player: EntityId, key: &QuestKey) -> bool {
    if state.quests.get(key).is_none() {
        return false;
    }
    let Some(log) = state.registry.get::<QuestLog>(player) else {
        return true; // no log at all: nothing has been taken or finished
    };
    if log.active_quest(key).is_some() {
        return false;
    }
    log.done
        .iter()
        .find(|done| &done.key == key)
        .is_none_or(|done| state.ticks >= done.restart_at)
}

/// Take the quest: it goes in the log with every objective at zero.
pub fn accept(state: &mut WorldState, player: EntityId, key: &QuestKey, giver: Option<Serial>) {
    let Some(quest) = state.quests.get(key) else {
        return;
    };
    // Objective clocks are ticks from now, not the definition's seconds — a timed
    // quest is timed from the moment it is taken.
    let seconds_left: Vec<u32> = quest.objectives.iter().map(|o| o.seconds).collect();
    let progress = vec![0u16; quest.objectives.len()];
    let wants_escort = quest
        .objectives
        .iter()
        .any(|objective| matches!(objective.kind, ObjectiveKind::Escort { .. }));

    let mut log = state
        .registry
        .get::<QuestLog>(player)
        .cloned()
        .unwrap_or_default();
    if log.active_quest(key).is_some() {
        return; // already taken; a double-click on Accept is not two quests
    }
    log.active.push(QuestState {
        key: key.clone(),
        progress,
        seconds_left,
        failed: false,
        giver,
    });
    state.registry.insert(player, log);

    // If it asks for an escort, the giver starts following now — ServUO's
    // `BaseQuest.OnAccept`, which is the moment the player agreed to lead it.
    if wants_escort {
        if let Some(giver) = giver {
            if let Some(destination) = crate::log::begin_escort(state, player, giver) {
                // Said out loud by the traveller, not shown to the player as a system
                // line — ServUO's `BaseEscortable` uses `Say` here, so whoever else is
                // standing around learns an escort has just set out.
                let speaker = state.registry.entity_of(giver);
                crate::progress::escortable_says(
                    state,
                    speaker,
                    &format!("Lead on! Payment will be made when we arrive in {destination}."),
                );
            }
        }
    }

    if let Some(serial) = state.registry.serial_of(player) {
        state.bus.send(QuestAccepted {
            player: serial,
            key:    key.clone(),
        });
    }
    gump::play(state, player, sound::ACCEPT);
    state.system_message(player, "You have accepted the quest.");
}

/// Turn the offer down. Nothing is started, and the giver says its piece.
pub fn refuse(state: &mut WorldState, player: EntityId, key: &QuestKey) {
    if let Some(serial) = state.registry.serial_of(player) {
        state.bus.send(QuestRefused {
            player: serial,
            key:    key.clone(),
        });
    }
    gump::show(
        state,
        player,
        QuestGumpContext {
            quest:     Some(key.clone()),
            section:   QuestSection::Refuse,
            offer:     true,
            completed: false,
            giver:     None,
        },
    );
}

/// Give up a quest already taken. Its cooldown starts as if it had been finished,
/// so resigning is not a way to re-roll a once-only quest.
pub fn resign(state: &mut WorldState, player: EntityId, key: &QuestKey) {
    let Some(mut log) = state.registry.get::<QuestLog>(player).cloned() else {
        return;
    };
    let Some(index) = log.active.iter().position(|quest| &quest.key == key) else {
        return;
    };
    let giver = log.active[index].giver;
    log.active.remove(index);
    remember_done(state, &mut log, key);
    state.registry.insert(player, log);
    // Anyone this quest had following stops following. Without this a resigned
    // escort trails the player for ever, off any quest log.
    if let Some(giver) = giver {
        crate::log::release_escort(state, giver);
    }

    if let Some(serial) = state.registry.serial_of(player) {
        state.bus.send(QuestResigned {
            player: serial,
            key:    key.clone(),
        });
    }
    gump::play(state, player, sound::RESIGN);
    state.system_message(player, "You have resigned the quest.");
}

/// Record that a quest is finished with (turned in or given up), with the wait
/// before it may be taken again.
pub(crate) fn remember_done(
    state: &WorldState,
    log: &mut openshard_state::components::QuestLog,
    key: &QuestKey,
) {
    let Some(quest) = state.quests.get(key) else {
        return;
    };
    let restart_at = if quest.done_once {
        WorldTick::MAX
    } else {
        state.ticks + u64::from(quest.restart_delay_secs) * TICKS_PER_SECOND
    };
    if let Some(done) = log.done.iter_mut().find(|done| &done.key == key) {
        done.restart_at = restart_at;
    } else {
        log.done.push(DoneQuest {
            key: key.clone(),
            restart_at,
        });
    }
}

/// Whether a player's copy of a quest has failed its timer.
#[must_use]
pub fn failed(state: &WorldState, player: EntityId, key: &QuestKey) -> bool {
    state
        .registry
        .get::<QuestLog>(player)
        .and_then(|log| log.active_quest(key))
        .is_some_and(|entry| entry.failed)
}

/// How many quests a player has in progress.
fn quest_count(state: &WorldState, player: EntityId) -> usize {
    state
        .registry
        .get::<QuestLog>(player)
        .map_or(0, |log| log.active.len())
}
