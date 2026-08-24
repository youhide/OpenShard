//! Finishing a quest: is it done, take what it asked for, pay what it promised.

use openshard_entities::EntityId;
use openshard_state::WorldState;
use openshard_state::components::QuestLog;
use openshard_state::quest::{ObjectiveKind, QuestKey, RewardKind};

use crate::events::QuestCompleted;
use crate::gump::{self, sound};
use crate::offer;
use openshard_protocol::wire::{Graphic, Hue};

/// Whether every objective a quest needs has been met.
///
/// `all_objectives` decides whether that means all of them or any one of them —
/// ServUO's `BaseQuest.Completed`.
#[must_use]
pub fn is_complete(state: &WorldState, player: EntityId, key: &QuestKey) -> bool {
    let Some(quest) = state.quests.get(key) else {
        return false;
    };
    let Some(entry) = state
        .registry
        .get::<QuestLog>(player)
        .and_then(|log| log.active_quest(key))
    else {
        return false;
    };
    if entry.failed {
        return false;
    }
    let mut met = quest
        .objectives
        .iter()
        .enumerate()
        .map(|(index, objective)| entry.progress.get(index).copied().unwrap_or(0) >= objective.count);
    if quest.all_objectives {
        met.all(|done| done)
    } else {
        met.any(|done| done)
    }
}

/// Hand a finished quest in: take the items it asked for, pay the rewards, and
/// move it from the log to the done list.
///
/// The item take is **all-or-nothing across every objective**: everything is
/// checked before anything is removed, so a player one item short of the second
/// objective does not lose what they brought for the first. That is the bug the
/// pack's version had — it took each objective independently — and it is worth
/// the extra pass, because the failure is invisible to the player.
pub fn complete(state: &mut WorldState, player: EntityId, key: &QuestKey) -> bool {
    if !is_complete(state, player, key) {
        state.system_message(player, "You do not have everything you need!");
        return false;
    }
    let Some(quest) = state.quests.get(key).cloned() else {
        return false;
    };
    let Some(player_serial) = state.registry.serial_of(player) else {
        return false;
    };

    // What has to be handed over, gathered first — and only for the objectives
    // that were actually met. An `all_objectives: false` quest completes on one
    // of its list, and demanding the goods for the others would make a quest the
    // player has finished impossible to hand in.
    let progress = state
        .registry
        .get::<QuestLog>(player)
        .and_then(|log| log.active_quest(key))
        .map(|entry| entry.progress.clone())
        .unwrap_or_default();
    let mut wanted: Vec<(Graphic, u16)> = Vec::new();
    for (index, objective) in quest.objectives.iter().enumerate() {
        if progress.get(index).copied().unwrap_or(0) < objective.count {
            continue;
        }
        match objective.kind {
            ObjectiveKind::Obtain { graphic } | ObjectiveKind::Deliver { graphic, .. } => {
                wanted.push((graphic, objective.count));
            }
            ObjectiveKind::Slay { .. } | ObjectiveKind::Escort { .. } => {}
        }
    }
    // Checked in full before a single item is removed.
    let short = wanted.iter().any(|&(graphic, count)| {
        openshard_items::carried_amount(state, player_serial, graphic) < u32::from(count)
    });
    if short {
        state.system_message(player, "You do not have everything you need!");
        return false;
    }
    for &(graphic, count) in &wanted {
        openshard_items::take_from_backpack(state, player_serial, graphic, count);
    }

    // Pay. Rewards that will not fit are still paid — `give_to_backpack` merges
    // or places, and a player with no backpack simply gets nothing, which cannot
    // happen to a real character.
    for reward in &quest.rewards {
        match reward.kind {
            RewardKind::Gold(amount) => {
                openshard_items::give(
                    state,
                    backpack_or_return(state, player_serial),
                    openshard_items::GOLD_GRAPHIC,
                    Hue(0),
                    amount,
                );
            }
            RewardKind::Item {
                graphic,
                hue,
                amount,
                stackable,
            } => {
                openshard_items::give_to_backpack(state, player_serial, graphic, hue, amount, stackable);
            }
        }
    }

    // And move it out of the log.
    let giver = state
        .registry
        .get::<QuestLog>(player)
        .and_then(|log| log.active_quest(key))
        .and_then(|entry| entry.giver);
    if let Some(mut log) = state.registry.get::<QuestLog>(player).cloned() {
        log.active.retain(|quest| &quest.key != key);
        offer::remember_done(state, &mut log, key);
        state.registry.insert(player, log);
    }

    gump::play(state, player, sound::COMPLETE);
    state.system_message(player, "You have completed a quest!");
    state.bus.send(QuestCompleted {
        player: player_serial,
        key: key.clone(),
        giver,
    });
    true
}

/// The player's backpack, or their own serial if they somehow have none — the
/// gold payout needs a container serial and `items::give` ignores one that names
/// no container, so nothing is created out of thin air either way.
fn backpack_or_return(
    state: &WorldState,
    player: openshard_protocol::serial::Serial,
) -> openshard_protocol::serial::Serial {
    openshard_items::backpack_of(state, player).unwrap_or(player)
}
