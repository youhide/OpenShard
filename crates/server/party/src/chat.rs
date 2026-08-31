//! Party chat: a line to everybody, or a line to one of them.
//!
//! The first tenant of the "send to a set" router in
//! [`tell_party`](crate::tell_party), and the reason party had to be built
//! before guild chat rather than beside it.
//!
//! # Not speech
//!
//! A party line does not go out as `0x1C` or `0xAE`, is not heard by anybody
//! standing nearby, and carries no position — it is its own packet
//! ([`PartyTextMessage`]) and the client draws it in its own colour. So none of
//! the speech machinery applies: no distance, no whisper radius, no line over a
//! head. What it shares with speech is only that a player typed it.

use openshard_entities::EntityId;
use openshard_protocol::party::{
    MESSAGE_LIMIT,
    PartyTextMessage,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_state::WorldState;

use crate::{
    Refusal,
    tell_party,
};

/// Say something to the whole party.
///
/// # A line too long is dropped, not clipped
///
/// ServUO returns without a word for anything over
/// [`MESSAGE_LIMIT`], and this does the same. Clipping would put half a sentence
/// on nine other people's screens and attribute it to somebody who did not write
/// it, which is worse than saying nothing. An empty line — or one that is empty
/// once trimmed — goes the same way.
pub fn say_to_party(state: &mut WorldState, speaker: EntityId, text: &str) -> Result<(), Refusal> {
    let Some(text) = acceptable(text) else {
        return Ok(());
    };
    let party = crate::party_of(state, speaker).ok_or(Refusal::NotInAParty)?;
    let from = state.registry.serial_of(speaker).ok_or(Refusal::NotAMobile)?;
    let packet = ServerPacket::PartyTextMessage(PartyTextMessage {
        to_all: true,
        from,
        text,
    });
    tell_party(state, party, &packet);
    Ok(())
}

/// Say something to one member of the party.
///
/// The recipient has to be in the *speaker's* party, checked here rather than
/// trusted: the serial comes off the wire, and a client is free to name anybody
/// on the shard.
pub fn say_privately(
    state: &mut WorldState,
    speaker: EntityId,
    listener: EntityId,
    text: &str,
) -> Result<(), Refusal> {
    let Some(text) = acceptable(text) else {
        return Ok(());
    };
    let party = crate::party_of(state, speaker).ok_or(Refusal::NotInAParty)?;
    let listener_serial = state.registry.serial_of(listener).ok_or(Refusal::NotAMobile)?;
    if !state
        .parties
        .get(party)
        .is_some_and(|entry| entry.contains(listener_serial))
    {
        return Err(Refusal::NotYourMember);
    }
    let from = state.registry.serial_of(speaker).ok_or(Refusal::NotAMobile)?;
    let packet = ServerPacket::PartyTextMessage(PartyTextMessage {
        to_all: false,
        from,
        text,
    });
    state.send_to(listener, &packet);
    Ok(())
}

/// The line to send, or `None` for one that should simply not be sent.
///
/// Trimmed first and measured after, which is ServUO's order
/// (`text.Length > 128 || (text = text.Trim()).Length == 0` measures the raw
/// length and then trims) — near enough that the only disagreement is a line of
/// 130 characters that is 120 after trimming, which the reference drops and this
/// sends. Taking the trimmed length is the kinder of the two and is what a
/// player would expect from a stray space.
fn acceptable(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MESSAGE_LIMIT {
        return None;
    }
    Some(text.to_owned())
}

#[cfg(test)]
mod tests {
    use openshard_protocol::party::MESSAGE_LIMIT;

    use super::acceptable;

    #[test]
    fn a_line_is_trimmed_and_an_empty_one_is_not_sent() {
        assert_eq!(acceptable("  hello  ").as_deref(), Some("hello"));
        assert_eq!(acceptable("   "), None);
        assert_eq!(acceptable(""), None);
    }

    #[test]
    fn a_line_past_the_limit_is_dropped_rather_than_cut() {
        // Cut, it would attribute half a sentence to somebody who wrote a whole
        // one. Counted in characters, not bytes: a client sends UTF-16 and may
        // send anything.
        let long: String = std::iter::repeat_n('é', MESSAGE_LIMIT + 1).collect();
        assert_eq!(acceptable(&long), None);
        let just_fits: String = std::iter::repeat_n('é', MESSAGE_LIMIT).collect();
        assert_eq!(acceptable(&just_fits).as_deref(), Some(just_fits.as_str()));
    }
}
