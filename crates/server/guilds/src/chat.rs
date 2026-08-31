//! Guild chat: a line to the guild, and a line to everybody it is allied with.
//!
//! # It is speech, and it is not spoken
//!
//! A guild line arrives as ordinary `0xAD` speech with the mode byte set to
//! [`TalkMode::Guild`], and goes back out as an ordinary `0xAE` with the same
//! mode — so the client draws it as a line from a named speaker, in the guild
//! colour, and *not* over anybody's head. Nothing about earshot applies: the
//! listeners are the roster, and `World::say` branches here before it measures a
//! distance. See [`speech_range`](openshard_chat::speech_range), which answers
//! zero for both modes so that a routing failure is silence rather than a
//! private line shouted down the street.
//!
//! # Alliance chat reaches the alliance
//!
//! It did not always. Until named alliances landed, being allied was a pairwise
//! `Relation::Ally` and this function reached "every guild yours has declared an
//! alliance with" — which meant two guilds allied to the same third could hear
//! each other through it while being strangers to one another, and the set a
//! line reached depended on who was speaking. An alliance is a named group now,
//! so the set is the same for every member, which is what a channel is.

use openshard_entities::EntityId;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::{
    Font,
    TalkMode,
    UnicodeMessage,
};
use openshard_protocol::wire::Hue;
use openshard_state::{
    GuildId,
    WorldState,
};

use crate::{
    Refusal,
    roster,
};

/// The language tag every line this engine sends carries — `openshard-chat`'s
/// own, repeated rather than shared because that crate does not depend on this
/// one and a guild line is not routed through it.
const LANGUAGE: &str = "ENU";

/// Say something to every member of your guild who is online.
pub fn say_to_guild(
    state: &mut WorldState,
    speaker: EntityId,
    hue: Hue,
    font: Font,
    text: &str,
) -> Result<(), Refusal> {
    let guild = state.guild_of(speaker).ok_or(Refusal::NotInAGuild)?.id;
    let packet = line(state, speaker, TalkMode::Guild, hue, font, text)?;
    tell_guild(state, guild, &packet);
    Ok(())
}

/// Say something to every guild in your alliance, your own included.
pub fn say_to_alliance(
    state: &mut WorldState,
    speaker: EntityId,
    hue: Hue,
    font: Font,
    text: &str,
) -> Result<(), Refusal> {
    let own = state.guild_of(speaker).ok_or(Refusal::NotInAGuild)?;
    let alliance = own.alliance.ok_or(Refusal::NoAllies)?;
    // The alliance's own members, which include the speaker's guild — so there
    // is no "and also my own" to remember, and a line cannot reach the allies
    // while the speaker's guildmates watch them go quiet.
    let members: Vec<GuildId> = crate::alliance_members(state, alliance);
    if members.is_empty() {
        return Err(Refusal::NoAllies);
    }
    let packet = line(state, speaker, TalkMode::Alliance, hue, font, text)?;
    for guild in members {
        tell_guild(state, guild, &packet);
    }
    Ok(())
}

/// Send one packet to every member of `guild` who is online.
///
/// [`tell_party`](openshard_party::tell_party)'s counterpart, and the second
/// tenant of the same idea: a line goes to a set of people picked by membership
/// rather than by where they are standing.
pub fn tell_guild(state: &mut WorldState, guild: GuildId, packet: &ServerPacket) {
    for member in roster(state, guild) {
        state.send_to(member, packet);
    }
}

/// Build the `0xAE` a guild line goes out as.
///
/// The speaker's own serial, body and name ride along, exactly as they do for
/// ordinary speech — which is what lets a client draw "Lord British: regroup"
/// rather than an anonymous system line.
fn line(
    state: &WorldState,
    speaker: EntityId,
    mode: TalkMode,
    hue: Hue,
    font: Font,
    text: &str,
) -> Result<ServerPacket, Refusal> {
    let serial = state.registry.serial_of(speaker).ok_or(Refusal::NotAMobile)?;
    Ok(ServerPacket::UnicodeMessage(UnicodeMessage {
        serial: Some(serial),
        graphic: state.registry.get::<openshard_state::Body>(speaker).map(|b| b.id),
        mode,
        hue,
        font,
        language: LANGUAGE.to_owned(),
        name: state
            .registry
            .get::<openshard_state::Name>(speaker)
            .map_or_else(String::new, |name| name.0.clone()),
        text: text.to_owned(),
    }))
}
