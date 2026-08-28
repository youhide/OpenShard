//! Reading the guild window's answer.
//!
//! Every branch resolves the reply against [`GuildGumpContext`] — what this side
//! remembers drawing — before it does anything. A button naming row nine of a
//! four-row list, or a member who logged out while the window was open, resolves
//! to nothing.
//!
//! The authority is checked *here* as well as when the page was drawn: the gump
//! id is not a secret, the window can outlive a leadership change, and a page
//! that only hid a button hid it on one client's screen.

use openshard_entities::EntityId;
use openshard_gateway::ConnectionId;
use openshard_protocol::gump::{GumpAnswer, GumpResponse};
use openshard_state::{GuildGumpContext, GuildPage, TargetPurpose, WorldState};

use crate::gump::{
    self, DIPLOMACY_BUTTONS, FIELD_ABBREVIATION, FIELD_NAME, GUILD_GUMP, ROSTER_BUTTONS, RowAction, button,
};
use crate::{RankFlags, Refusal};

/// The paperdoll's Guild button: open the window.
pub fn open(state: &mut WorldState, connection: ConnectionId) {
    if let Some(&player) = state.players.get(&connection) {
        gump::show(state, player, GuildPage::Main);
    }
}

/// Act on a guild window reply.
///
/// Returns whether the reply was one of ours, so the router can fall through.
pub fn handle(state: &mut WorldState, connection: ConnectionId, response: &GumpResponse) -> bool {
    if response.gump_id.validate(&[GUILD_GUMP]).is_none() {
        return false;
    }
    let Some(&player) = state.players.get(&connection) else {
        return true;
    };
    // Taken, not read: the window is gone the moment it is answered, and every
    // branch that draws another draws it fresh. A second reply to the same window
    // finds no context and does nothing, which is what makes a double-click on a
    // button one dismissal rather than two.
    let Some(context) = state.row_of_mut(player).and_then(|row| row.guild_gump.take()) else {
        return true;
    };
    let GumpAnswer::Pressed(pressed) = response.button.interpret() else {
        return true; // the close box
    };

    match pressed {
        button::CLOSE => {}
        button::MAIN => gump::show(state, player, GuildPage::Main),
        button::ROSTER => gump::show(state, player, GuildPage::Roster),
        button::DIPLOMACY => gump::show(state, player, GuildPage::Diplomacy),
        button::FOUND => {
            let name = field(response, FIELD_NAME);
            let abbreviation = field(response, FIELD_ABBREVIATION);
            match crate::found(state, player, &name, &abbreviation) {
                Ok(_) => state.system_message(player, "Your guild is founded."),
                Err(refusal) => refuse(state, player, refusal),
            }
            gump::show(state, player, GuildPage::Main);
        }
        button::ACCEPT => {
            match crate::accept_invitation(state, player) {
                Ok(_) => state.system_message(player, "You have joined the guild."),
                Err(refusal) => refuse(state, player, refusal),
            }
            gump::show(state, player, GuildPage::Main);
        }
        button::DECLINE => {
            crate::decline_invitation(state, player);
            gump::show(state, player, GuildPage::Main);
        }
        button::LEAVE => {
            match crate::leave(state, player) {
                Ok(()) => state.system_message(player, "You have left your guild."),
                Err(refusal) => refuse(state, player, refusal),
            }
            gump::show(state, player, GuildPage::Main);
        }
        button::DISBAND => {
            if let Err(refusal) = crate::disband(state, player) {
                refuse(state, player, refusal);
            }
            gump::show(state, player, GuildPage::Main);
        }
        button::LEAVE_ALLIANCE => {
            if let Err(refusal) = crate::leave_alliance(state, player) {
                refuse(state, player, refusal);
            }
            gump::show(state, player, GuildPage::Diplomacy);
        }
        button::JOIN_ALLIANCE => {
            match crate::join_alliance(state, player) {
                Ok(_) => state.system_message(player, "Your guild has joined the alliance."),
                Err(refusal) => refuse(state, player, refusal),
            }
            gump::show(state, player, GuildPage::Diplomacy);
        }
        button::INVITE => {
            // Checked before the cursor goes up, so a plain member never gets one
            // — and checked again when it comes down, because the guild can
            // change while it is up.
            if let Err(refusal) = crate::may(state, player, RankFlags::CAN_INVITE) {
                refuse(state, player, refusal);
            } else {
                state.raise_target(player, TargetPurpose::GuildInvite);
                state.system_message(player, "Whom shall we ask to join?");
            }
        }
        _ => other_button(state, player, &context, response, pressed),
    }
    true
}

/// The rows: a member's, or another guild's.
fn other_button(
    state: &mut WorldState,
    player: EntityId,
    context: &GuildGumpContext,
    response: &GumpResponse,
    pressed: openshard_protocol::gump::ButtonId,
) {
    if let Some((row, action)) = ROSTER_BUTTONS.decode(pressed, context.members.len()) {
        roster_row(state, player, context, response, row, action);
        gump::show(state, player, GuildPage::Roster);
        return;
    }
    if let Some((row, action)) = DIPLOMACY_BUTTONS.decode(pressed, context.guilds.len()) {
        let other = context.guilds[row];
        let outcome = match action {
            gump::DIPLOMACY_WAR => crate::declare_war(state, player, other).map(|_| ()),
            gump::DIPLOMACY_PEACE => crate::make_peace(state, player, other),
            // The alliance's name comes off the field on the same page, and is
            // read only when this guild is in none.
            gump::DIPLOMACY_ALLY => {
                crate::invite_to_alliance(state, player, other, &field(response, gump::FIELD_ALLIANCE))
                    .map(|_| ())
            }
            // If a future layout widens its stride without teaching the reply
            // the new action, an invented button remains a no-op.
            _ => return,
        };
        if let Err(refusal) = outcome {
            refuse(state, player, refusal);
        }
        gump::show(state, player, GuildPage::Diplomacy);
    }
}

/// One member row: set the title typed beside it, turn them out, or hand them the
/// guild.
fn roster_row(
    state: &mut WorldState,
    player: EntityId,
    context: &GuildGumpContext,
    response: &GumpResponse,
    row: usize,
    action: RowAction,
) {
    // Through the serial the window drew, so a row naming someone who logged out
    // resolves to nobody rather than to whoever inherited the entity slot.
    let Some(member) = state.registry.entity_of(context.members[row]) else {
        return;
    };
    let outcome = match action {
        // The field beside the row carries the row's own index as its id, so the
        // title comes back with the click that asked for it — one packet, and no
        // "which row was I editing" to remember.
        gump::ROSTER_TITLE => crate::set_title(state, player, member, &field(response, row as u32)),
        gump::ROSTER_PROMOTE => crate::promote(state, player, member).map(|_| ()),
        gump::ROSTER_DEMOTE => crate::demote(state, player, member).map(|_| ()),
        gump::ROSTER_DISMISS => crate::dismiss(state, player, member),
        gump::ROSTER_LEAD => crate::pass_leadership(state, player, member),
        // A future stride can add an action without silently turning it into
        // leadership transfer in an older reply path.
        _ => return,
    };
    if let Err(refusal) = outcome {
        refuse(state, player, refusal);
    }
}

/// The text a field came back with, or empty.
fn field(response: &GumpResponse, id: u32) -> String {
    response
        .text_entries
        .iter()
        .find(|(field, _)| u32::from(*field) == id)
        .map_or_else(String::new, |(_, text)| text.clone())
}

/// Say why, in one place.
fn refuse(state: &mut WorldState, player: EntityId, refusal: Refusal) {
    state.system_message(player, refusal.message());
}
