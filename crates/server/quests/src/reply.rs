//! Reading the quest window's answer.
//!
//! A port of `MondainQuestGump.OnResponse` and `MondainResignGump.OnResponse`.
//! The button ids mean what they mean because the layout said so, and the *page*
//! they were clicked on comes from what the server remembered opening — never
//! from the client, which is free to send any number it likes.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::gump::{ButtonId, GumpAnswer, GumpId, GumpResponse, RawGumpId};
use openshard_state::components::QuestLog;
use openshard_state::{QuestGumpContext, QuestSection, WorldState};

use crate::gump::{
    self, QUEST_GUMP, QUEST_RESIGN_GUMP, RESIGN_OK, RESIGN_SWITCH_YES, RESIGN_SWITCHES, button,
};
use crate::{offer, turnin};

/// The two windows the quest system draws, and the only ids it answers for.
const QUEST_GUMPS: [GumpId; 2] = [QUEST_GUMP, QUEST_RESIGN_GUMP];

/// Whether a gump id belongs to the quest system.
#[must_use]
pub fn owns(gump_id: RawGumpId) -> bool {
    gump_id.validate(&QUEST_GUMPS).is_some()
}

/// Act on a quest dialog's reply.
///
/// Returns whether the reply was one of ours, so the router can fall through to
/// the pack for anything else.
pub fn handle(state: &mut WorldState, connection: ConnectionId, response: &GumpResponse) -> bool {
    let Some(gump) = response.gump_id.validate(&QUEST_GUMPS) else {
        return false;
    };
    let Some(&player) = state.players.get(&connection) else {
        return true;
    };
    if gump == QUEST_RESIGN_GUMP {
        resign_reply(state, player, response);
        return true;
    }
    // The context is the server's memory of what it drew. A reply with none is a
    // reply to a window this side never opened — a stale click, or a crafted
    // packet — and does nothing.
    let Some(context) = state.row_of_mut(player).and_then(|row| row.quest_gump.take()) else {
        return true;
    };
    quest_reply(state, player, &context, response.button.interpret());
    true
}

/// The main window's buttons.
fn quest_reply(state: &mut WorldState, player: EntityId, context: &QuestGumpContext, answer: GumpAnswer) {
    // The `X` this layout draws carries the close box's own id, so dismissing
    // the window and pressing it are the same answer by construction — see
    // `button::CLOSE`.
    let GumpAnswer::Pressed(pressed) = answer else {
        return;
    };
    match pressed {
        button::CLOSE => {}
        button::CLOSE_QUEST => {
            // Back to the log.
            gump::show(
                state,
                player,
                QuestGumpContext {
                    quest: None,
                    section: QuestSection::Main,
                    offer: false,
                    completed: false,
                    giver: None,
                },
            );
        }
        button::ACCEPT_QUEST if context.offer => {
            if let Some(key) = &context.quest {
                offer::accept(state, player, key, context.giver);
            }
        }
        button::REFUSE_QUEST if context.offer => {
            if let Some(key) = &context.quest {
                offer::refuse(state, player, key);
            }
        }
        button::RESIGN_QUEST if !context.offer => {
            if let Some(key) = &context.quest {
                gump::show_resign(state, player, key);
            }
            // Remembered so the confirmation's reply knows which quest it is
            // about: the resign dialog carries no quest of its own.
            if let Some(row) = state.row_of_mut(player) {
                row.quest_gump = Some(context.clone());
            }
        }
        button::COMPLETE if !context.offer => {
            hand_in(state, player, context);
        }
        button::ACCEPT_REWARD if context.completed => {
            if let Some(key) = &context.quest {
                turnin::complete(state, player, key);
            }
        }
        button::PREVIOUS_PAGE | button::NEXT_PAGE => {
            let section = page(context.section, pressed == button::NEXT_PAGE);
            gump::show(
                state,
                player,
                QuestGumpContext {
                    section,
                    ..context.clone()
                },
            );
        }
        row => open_row(state, player, row),
    }
}

/// Hand a finished quest in. With rewards to show, the rewards page comes first
/// and pays on its own button; with none, it is paid outright — ServUO's
/// `Buttons.Complete`.
fn hand_in(state: &mut WorldState, player: EntityId, context: &QuestGumpContext) {
    let Some(key) = &context.quest else {
        return;
    };
    if !turnin::is_complete(state, player, key) {
        state.system_message(player, "You do not have everything you need!");
        return;
    }
    let has_rewards = state
        .quests
        .get(key)
        .is_some_and(|quest| !quest.rewards.is_empty());
    if has_rewards {
        gump::show(
            state,
            player,
            QuestGumpContext {
                section: QuestSection::Rewards,
                completed: true,
                ..context.clone()
            },
        );
    } else {
        turnin::complete(state, player, key);
    }
}

/// A row in the quest log: open that quest's detail page.
fn open_row(state: &mut WorldState, player: EntityId, pressed: ButtonId) {
    let Some(index) = gump::row_of(pressed) else {
        return;
    };
    let Some(entry) = state
        .registry
        .get::<QuestLog>(player)
        .and_then(|log| log.active.get(index.position()))
    else {
        return;
    };
    let (key, giver) = (entry.key.clone(), entry.giver);
    gump::show(
        state,
        player,
        gump::log_context(&key, QuestSection::Description, giver),
    );
}

/// The next or previous page of a quest, stopping at the ends.
const fn page(from: QuestSection, forward: bool) -> QuestSection {
    match (from, forward) {
        (QuestSection::Description, true) => QuestSection::Objectives,
        (QuestSection::Objectives, true) => QuestSection::Rewards,
        (QuestSection::Objectives, false) => QuestSection::Description,
        (QuestSection::Rewards, false) => QuestSection::Objectives,
        (section, _) => section,
    }
}

/// The resign confirmation: only the "yes" radio actually gives the quest up.
fn resign_reply(state: &mut WorldState, player: EntityId, response: &GumpResponse) {
    let context = state.row_of_mut(player).and_then(|row| row.quest_gump.take());
    let Some(context) = context else {
        return;
    };
    if response.button.interpret() != GumpAnswer::Pressed(RESIGN_OK) {
        return; // dismissed
    }
    // The choice rides in the switch, so it is checked like one: an id past the
    // pair this dialog drew is not "no", it is nothing at all.
    let yes = response
        .switches
        .iter()
        .filter_map(|switch| switch.validate(RESIGN_SWITCHES).ok())
        .any(|switch| switch == RESIGN_SWITCH_YES);
    if yes {
        if let Some(key) = &context.quest {
            offer::resign(state, player, key);
        }
    } else {
        // Kept: back to the quest's page, where it was.
        gump::show(state, player, context);
    }
}
