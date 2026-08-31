//! Parties: asking somebody along, and everything that follows.
//!
//! # What is here, and what is [`openshard_state::party`]
//!
//! The substrate below holds what a party *is* — who is in it, in what order,
//! and who has been asked. This crate holds the rules: the capacity, who may
//! kick whom, what a decline does to a party of one, and where a line of chat
//! goes. The same split [`openshard_guilds`] has over
//! [`Guilds`](openshard_state::Guilds).
//!
//! # A party has a leader and nothing else
//!
//! There are no ranks here and there is no equivalent of [`may_lead`]'s ladder,
//! because ServUO's party has exactly two rules about authority: the leader
//! adds and kicks, and anybody may remove **themselves**. That second one is
//! the same packet as the first (`0x02` naming a serial), which is the whole of
//! why [`remove`] takes both an actor and a target rather than being two
//! functions.
//!
//! # The leader never changes, so the leader is the key
//!
//! A leader who leaves disbands the party rather than handing it on — ServUO's
//! `Party.Remove` is explicit — so a party's identity is its leader's serial for
//! the party's whole life. See [`PartyId`].
//!
//! # An invitation is a consent, and a refusal can end a party
//!
//! Same shape as a guild's: [`invite`] leaves a
//! [`PartyCandidate`](openshard_state::PartyCandidate) and the player answers
//! it. What is different is the *decline*: a leader who asked one person, and
//! was refused, is left leading a party of one, and ServUO closes it rather than
//! leaving them in a group by themselves. [`decline`] does the same.
//!
//! [`may_lead`]: openshard_guilds::may_lead
//! [`openshard_guilds`]: https://docs.rs/openshard-guilds

mod chat;
mod membership;
#[cfg(test)]
mod tests;

pub use chat::{
    say_privately,
    say_to_party,
};
pub use membership::{
    accept,
    decline,
    disband,
    invite,
    on_logout,
    remove,
    set_can_loot,
};
use openshard_entities::EntityId;
use openshard_protocol::party::{
    PartyMemberList,
    PartyRemoveMember,
};
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_state::{
    PartyId,
    WorldState,
};

/// Why a party operation was refused.
///
/// [`Refusal`](openshard_guilds::Refusal)'s counterpart, and deliberately its
/// own type rather than a shared one: the two systems have one word in common
/// ("you are not in a party" / "you are not in a guild") and nothing else, and a
/// merged enum would be a list of variants most callers can never produce.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// The actor is in no party.
    NotInAParty,
    /// The actor is in one but does not lead it.
    NotTheLeader,
    /// The party is full — members and outstanding invitations together.
    PartyIsFull,
    /// The mobile named is not one a party can hold.
    NotAMobile,
    /// The mobile named is already in a party.
    TheyAreInAParty,
    /// The mobile named is not in the actor's party.
    NotYourMember,
    /// The operation names the actor, and it is not one that may.
    Yourself,
    /// There is no invitation to answer.
    NotInvited,
}

impl Refusal {
    /// What to tell the player.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NotInAParty => "You are not in a party.",
            Self::NotTheLeader => "You do not lead your party.",
            Self::PartyIsFull => "Your party is full.",
            Self::NotAMobile => "That cannot join a party.",
            Self::TheyAreInAParty => "They are already in a party.",
            Self::NotYourMember => "They are not in your party.",
            Self::Yourself => "You cannot do that to yourself.",
            Self::NotInvited => "Nobody has invited you to a party.",
        }
    }
}

/// The party `actor` is in, or why they are in none.
#[must_use]
pub fn party_of(state: &WorldState, actor: EntityId) -> Option<PartyId> {
    state.party_of(actor).map(|party| PartyId(party.leader))
}

/// The party `actor` leads, or why they do not lead one.
///
/// [`may_lead`](openshard_guilds::may_lead)'s counterpart, and the only
/// authority check a party has.
pub fn may_lead(state: &WorldState, actor: EntityId) -> Result<PartyId, Refusal> {
    let party = state.party_of(actor).ok_or(Refusal::NotInAParty)?;
    let serial = state.registry.serial_of(actor).ok_or(Refusal::NotAMobile)?;
    if party.leader == serial {
        Ok(PartyId(party.leader))
    } else {
        Err(Refusal::NotTheLeader)
    }
}

/// Everyone in `party` who is still resolvable to an entity, in wire order.
///
/// A serial that names nobody is skipped rather than being an error: a member
/// who logged out between the change and the announcement is a race the shard
/// wins by saying nothing to them.
#[must_use]
pub fn roster(state: &WorldState, party: PartyId) -> Vec<EntityId> {
    state
        .parties
        .get(party)
        .map(|party| {
            party
                .members
                .iter()
                .filter_map(|serial| state.registry.entity_of(*serial))
                .collect()
        })
        .unwrap_or_default()
}

/// Send one packet to every member of `party`.
///
/// The router the whole system is built on, and the reason party had to land
/// before guild chat: "a line goes to a set of people who are not the ones
/// standing nearby" is one mechanism, and building it twice would be building it
/// twice.
pub fn tell_party(state: &mut WorldState, party: PartyId, packet: &ServerPacket) {
    for member in roster(state, party) {
        state.send_to(member, packet);
    }
}

/// Say something to every member of `party`, as a system line.
pub fn announce(state: &mut WorldState, party: PartyId, text: &str) {
    for member in roster(state, party) {
        state.system_message(member, text);
    }
}

/// Send everybody the party's roster, as it now stands.
///
/// There is no add-one-member packet — a change re-sends the whole list, which
/// is ServUO's own approach and is what keeps every client's roster identical
/// rather than accumulated from deltas it may have missed.
pub fn tell_roster(state: &mut WorldState, party: PartyId) {
    let members = members_of(state, party);
    let packet = ServerPacket::PartyMemberList(PartyMemberList { members });
    tell_party(state, party, &packet);
}

/// Tell everyone left that `removed` has gone, and tell `removed` they are in
/// no party.
///
/// Two packets and one shape: the "you are in no party" packet *is* a removal
/// with an empty list — see
/// [`PartyRemoveMember`](openshard_protocol::party::PartyRemoveMember).
pub fn tell_removal(state: &mut WorldState, party: PartyId, removed: Serial) {
    if let Some(entity) = state.registry.entity_of(removed) {
        let empty = ServerPacket::PartyRemoveMember(PartyRemoveMember {
            removed,
            members: Vec::new(),
        });
        state.send_to(entity, &empty);
    }
    let members = members_of(state, party);
    if members.is_empty() {
        return;
    }
    let packet = ServerPacket::PartyRemoveMember(PartyRemoveMember { removed, members });
    tell_party(state, party, &packet);
}

/// The serials in a party, in order. Empty for a party that is gone.
fn members_of(state: &WorldState, party: PartyId) -> Vec<Serial> {
    state
        .parties
        .get(party)
        .map(|party| party.members.clone())
        .unwrap_or_default()
}
