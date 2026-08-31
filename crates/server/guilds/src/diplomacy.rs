//! War, and the alliance that used to be mistaken for it.
//!
//! # Two shapes, and they were one function too long
//!
//! A **war** is two declarations: A declares on B and nothing changes, B
//! declares on A and both are at war. Peace is one guild's decision, because the
//! alternative is a guild that cannot stop being attacked by one that will not
//! agree to stop.
//!
//! An **alliance** is not that. It is a named group a guild is invited *into* by
//! a guild already in it, and answered by that guild's own leader — the same
//! shape a player's guild membership has, one level up. Until 2026-08-15 both
//! went through one `propose`, on the argument that they were the same
//! handshake, and the cost was that an alliance was pairwise: A allied with B
//! and with C left B and C strangers, and "who is in my alliance" had no answer.
//! See [`Alliance`](openshard_state::Alliance).

use openshard_entities::EntityId;
use openshard_state::{
    AllianceId,
    GuildId,
    Removal,
    WorldState,
};

use crate::{
    RankFlags,
    Refusal,
    announce,
    may,
    recolour_guild,
};

/// The longest an alliance may call itself. ServUO's own prompt takes the same
/// as a guild name.
pub const ALLIANCE_NAME_LIMIT: usize = 40;

/// What a declaration did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The other guild has not said the same yet. Nothing has changed but the
    /// list they see.
    Offered,
    /// They had already said it, so it is now true of both.
    Declared,
}

/// Declare war on another guild.
///
/// Nothing happens on the strength of one guild's word: this leaves a
/// declaration, and the war exists when `other` declares back.
pub fn declare_war(state: &mut WorldState, actor: EntityId, other: GuildId) -> Result<Outcome, Refusal> {
    let (own, _) = may(state, actor, RankFlags::CONTROL_WAR_STATUS)?;
    if own == other || state.guilds.get(other).is_none() {
        return Err(Refusal::NoSuchGuild);
    }
    // Not somebody you are allied with. ServUO has no such check because its
    // alliance gump simply does not offer the button; here the reply path can be
    // reached with a stale window, and a war inside an alliance would make two
    // guilds read green and orange to each other depending on which question was
    // asked first.
    if state.allied(own, other) {
        return Err(Refusal::AlliedWithThem);
    }

    let (ours, theirs) = names(state, own, other);
    if !state.guilds.declare_war(own, other) {
        // Told to both sides. A declaration nobody is told about is one the other
        // guild cannot answer, and answering is the whole mechanism.
        announce(state, own, &format!("You have declared war on {theirs}."));
        announce(state, other, &format!("{ours} has declared war on you."));
        return Ok(Outcome::Offered);
    }

    let text = format!("{ours} and {theirs} are at war.");
    announce(state, own, &text);
    announce(state, other, &text);
    // Both rosters: the colour moved on every screen where a member of one can
    // see a member of the other, and that is two directions.
    recolour_guild(state, own);
    recolour_guild(state, other);
    Ok(Outcome::Declared)
}

/// End a war, both ways — and take back a declaration nobody answered, which is
/// the same button on the same row.
///
/// One guild's decision, not a second handshake. See the module docs.
pub fn make_peace(state: &mut WorldState, actor: EntityId, other: GuildId) -> Result<(), Refusal> {
    let (own, _) = may(state, actor, RankFlags::CONTROL_WAR_STATUS)?;
    if own == other || state.guilds.get(other).is_none() {
        return Err(Refusal::NoSuchGuild);
    }
    let (ours, theirs) = names(state, own, other);
    state.guilds.undeclare(own, other);
    let text = format!("{ours} and {theirs} are at peace.");
    announce(state, own, &text);
    announce(state, other, &text);
    recolour_guild(state, own);
    recolour_guild(state, other);
    Ok(())
}

/// Ask `other` into an alliance, founding one under `name` if this guild is in
/// none.
///
/// # The name is only read the first time
///
/// A guild already in an alliance is *extending* it, and the alliance's name is
/// its own — passing a different one does not rename it. That is deliberate: an
/// invitation that could rename the body sending it would let any member rename
/// an alliance by inviting somebody.
pub fn invite_to_alliance(
    state: &mut WorldState,
    actor: EntityId,
    other: GuildId,
    name: &str,
) -> Result<AllianceId, Refusal> {
    let (own, _) = may(state, actor, RankFlags::ALLIANCE_CONTROL)?;
    if own == other || state.guilds.get(other).is_none() {
        return Err(Refusal::NoSuchGuild);
    }
    if state.guilds.get(other).and_then(|guild| guild.alliance).is_some() {
        return Err(Refusal::TheyAreAllied);
    }
    if state
        .guilds
        .get(own)
        .is_some_and(|guild| guild.at_war_with(other))
    {
        return Err(Refusal::AtWarWithThem);
    }

    let alliance = match state.guilds.get(own).and_then(|guild| guild.alliance) {
        Some(alliance) => {
            state.alliances.ask(alliance, other);
            alliance
        }
        None => {
            let name = crate::membership::clip(name, ALLIANCE_NAME_LIMIT);
            if name.is_empty() {
                return Err(Refusal::NoName);
            }
            if state.alliances.by_name(&name).is_some() {
                return Err(Refusal::NameTaken);
            }
            let alliance = state.alliances.found(name, own, other);
            if let Some(guild) = state.guilds.get_mut(own) {
                guild.alliance = Some(alliance);
            }
            alliance
        }
    };
    let (ours, theirs) = names(state, own, other);
    let alliance_name = alliance_name(state, alliance);
    announce(
        state,
        own,
        &format!("{theirs} has been asked into {alliance_name}."),
    );
    announce(
        state,
        other,
        &format!("{ours} has asked your guild into {alliance_name}."),
    );
    Ok(alliance)
}

/// Answer an alliance invitation with yes.
///
/// The alliance is not named by the caller: a guild is asked into exactly one at
/// a time, and which one is the shard's record rather than the client's claim.
pub fn join_alliance(state: &mut WorldState, actor: EntityId) -> Result<AllianceId, Refusal> {
    let (own, _) = may(state, actor, RankFlags::ALLIANCE_CONTROL)?;
    if state.guilds.get(own).and_then(|guild| guild.alliance).is_some() {
        return Err(Refusal::AlreadyAllied);
    }
    let Some(alliance) = pending_for(state, own) else {
        return Err(Refusal::NotAsked);
    };
    // A war with anybody already inside it. Joining would make two guilds read
    // green and orange to each other at once, and the notoriety answer would
    // depend on which question was asked first.
    let members: Vec<GuildId> = state
        .alliances
        .get(alliance)
        .map(|entry| entry.members.iter().copied().collect())
        .unwrap_or_default();
    let at_war = state
        .guilds
        .get(own)
        .is_some_and(|guild| members.iter().any(|member| guild.at_war_with(*member)));
    if at_war {
        return Err(Refusal::AtWarWithThem);
    }

    if !state.alliances.accept(alliance, own) {
        return Err(Refusal::NotAsked);
    }
    if let Some(guild) = state.guilds.get_mut(own) {
        guild.alliance = Some(alliance);
    }
    let ours = names(state, own, own).0;
    let alliance_name = alliance_name(state, alliance);
    tell_alliance(state, alliance, &format!("{ours} has joined {alliance_name}."));
    recolour_alliance(state, alliance);
    Ok(alliance)
}

/// Leave the alliance, or decline the invitation to one.
///
/// One function, because the packet is one button and the two differ only in
/// whether the guild had answered yet. An alliance left with fewer than two
/// members goes with it — see [`Alliances::remove`](openshard_state::Alliances::remove).
pub fn leave_alliance(state: &mut WorldState, actor: EntityId) -> Result<(), Refusal> {
    let (own, _) = may(state, actor, RankFlags::ALLIANCE_CONTROL)?;
    let alliance = state
        .guilds
        .get(own)
        .and_then(|guild| guild.alliance)
        .or_else(|| pending_for(state, own))
        .ok_or(Refusal::NotAllied)?;

    let ours = names(state, own, own).0;
    let alliance_name = alliance_name(state, alliance);
    let removal = state.alliances.remove(alliance, own);
    if let Some(guild) = state.guilds.get_mut(own) {
        guild.alliance = None;
    }
    match removal {
        Removal::Disbanded(gone) => {
            // Everybody it held, members and pending both: each has a link to
            // unhook, and the alliance is no longer there to be asked.
            for guild in gone.members.iter().chain(gone.pending.iter()) {
                if let Some(entry) = state.guilds.get_mut(*guild) {
                    entry.alliance = None;
                }
                announce(state, *guild, &format!("{alliance_name} has dissolved."));
                recolour_guild(state, *guild);
            }
        }
        Removal::Stood => {
            tell_alliance(state, alliance, &format!("{ours} has left {alliance_name}."));
            recolour_alliance(state, alliance);
        }
        Removal::Gone => {}
    }
    recolour_guild(state, own);
    Ok(())
}

/// Every guild in an alliance, members only.
#[must_use]
pub fn alliance_members(state: &WorldState, alliance: AllianceId) -> Vec<GuildId> {
    state
        .alliances
        .get(alliance)
        .map(|entry| entry.members.iter().copied().collect())
        .unwrap_or_default()
}

/// Which alliance has asked `guild` in, if one has.
///
/// A scan, and deliberately: there are a handful of alliances on a shard and
/// this is asked when a leader presses a button, never on a hot path.
fn pending_for(state: &WorldState, guild: GuildId) -> Option<AllianceId> {
    state
        .alliances
        .iter()
        .find(|alliance| alliance.pending.contains(&guild))
        .map(|alliance| alliance.id)
}

/// Say something to every guild in an alliance.
fn tell_alliance(state: &mut WorldState, alliance: AllianceId, text: &str) {
    for guild in alliance_members(state, alliance) {
        announce(state, guild, text);
    }
}

/// Re-announce every member of every guild in an alliance.
///
/// The colour moved for all of them at once: joining turns a whole roster green
/// to another whole roster, in both directions.
fn recolour_alliance(state: &mut WorldState, alliance: AllianceId) {
    for guild in alliance_members(state, alliance) {
        recolour_guild(state, guild);
    }
}

/// What an alliance calls itself, or a word that reads as one in a sentence.
fn alliance_name(state: &WorldState, alliance: AllianceId) -> String {
    state
        .alliances
        .get(alliance)
        .map_or_else(|| "the alliance".to_owned(), |entry| entry.name.clone())
}

/// Both guilds' names, for a message that has to read as one sentence.
///
/// Taken before the change and returned owned, because every caller goes on to
/// borrow the world mutably and the names live in it.
fn names(state: &WorldState, own: GuildId, other: GuildId) -> (String, String) {
    let name = |id| {
        state
            .guilds
            .get(id)
            .map_or_else(String::new, |guild| guild.name.clone())
    };
    (name(own), name(other))
}
