//! Asking somebody along, answering, leaving, and the loot flag.

use openshard_entities::EntityId;
use openshard_protocol::party::{
    CAPACITY,
    PartyInvitation,
};
use openshard_protocol::server_packet::ServerPacket;
use openshard_state::{
    PartyCandidate,
    PartyId,
    PartyMember,
    WorldState,
};

use crate::{
    Refusal,
    announce,
    may_lead,
    tell_removal,
    tell_roster,
};

/// Ask `candidate` into the party `inviter` leads, opening one if they lead
/// none.
///
/// # A party of one
///
/// Asking is what *creates* a party: before this there is no group, and the
/// invitation has to belong to something. So a leader who has asked one person
/// and been ignored is leading a party containing only themselves — which is
/// ServUO's own state, and is why [`decline`] has to close it again rather than
/// leaving somebody grouped with nobody.
pub fn invite(state: &mut WorldState, inviter: EntityId, candidate: EntityId) -> Result<(), Refusal> {
    if candidate == inviter {
        return Err(Refusal::Yourself);
    }
    // Leading none is not a refusal — it is the ordinary way to start one. Only
    // being in *somebody else's* party is.
    if let Some(party) = state.party_of(inviter) {
        let leader = party.leader;
        if state.registry.serial_of(inviter) != Some(leader) {
            return Err(Refusal::NotTheLeader);
        }
    }
    if state.registry.serial_of(candidate).is_none() {
        return Err(Refusal::NotAMobile);
    }
    if state.party_of(candidate).is_some() || state.registry.get::<PartyCandidate>(candidate).is_some() {
        return Err(Refusal::TheyAreInAParty);
    }

    let leader_serial = state.registry.serial_of(inviter).ok_or(Refusal::NotAMobile)?;
    let candidate_serial = state.registry.serial_of(candidate).ok_or(Refusal::NotAMobile)?;
    let party = state.parties.open(leader_serial);
    // The membership component for a leader who did not have one: `open` put
    // them in the roster, and the reverse index has to agree.
    if state.registry.get::<PartyMember>(inviter).is_none() {
        state.registry.insert(
            inviter,
            PartyMember {
                party,
                can_loot: false,
            },
        );
    }
    // Counted *after* the party exists, so a leader's own place is in the
    // number — ServUO measures `Members.Count + Candidates.Count` against the
    // cap, and a leader who is not counted could gather eleven.
    if state.parties.get(party).is_some_and(|p| p.taken() >= CAPACITY) {
        return Err(Refusal::PartyIsFull);
    }
    if let Some(entry) = state.parties.get_mut(party) {
        entry.candidates.push(candidate_serial);
    }
    state.registry.insert(candidate, PartyCandidate { party });
    let packet = ServerPacket::PartyInvitation(PartyInvitation {
        leader: leader_serial,
    });
    state.send_to(candidate, &packet);
    Ok(())
}

/// Answer an invitation with yes.
pub fn accept(state: &mut WorldState, candidate: EntityId) -> Result<PartyId, Refusal> {
    let party = state
        .registry
        .get::<PartyCandidate>(candidate)
        .map(|invitation| invitation.party)
        .ok_or(Refusal::NotInvited)?;
    // The invitation outlived the party — the leader logged out, or disbanded.
    // Clear it rather than joining a group that is not there.
    if state.parties.get(party).is_none() {
        state.registry.remove::<PartyCandidate>(candidate);
        return Err(Refusal::NotInvited);
    }
    if state.party_of(candidate).is_some() {
        state.registry.remove::<PartyCandidate>(candidate);
        return Err(Refusal::TheyAreInAParty);
    }
    let serial = state.registry.serial_of(candidate).ok_or(Refusal::NotAMobile)?;

    state.registry.remove::<PartyCandidate>(candidate);
    if let Some(entry) = state.parties.get_mut(party) {
        entry.candidates.retain(|asked| *asked != serial);
        entry.members.push(serial);
    }
    state.registry.insert(
        candidate,
        PartyMember {
            party,
            can_loot: false,
        },
    );
    let name = name_of(state, candidate);
    announce(state, party, &format!("{name} has joined the party."));
    tell_roster(state, party);
    Ok(party)
}

/// Answer an invitation with no.
///
/// Closes a party the refusal has left with nobody in it but its leader and no
/// other question outstanding — ServUO's `OnDecline` does exactly this, and the
/// alternative is a player quietly in a "party" alone, whose next invitation
/// would silently reuse it.
pub fn decline(state: &mut WorldState, candidate: EntityId) -> Result<(), Refusal> {
    let party = state
        .registry
        .get::<PartyCandidate>(candidate)
        .map(|invitation| invitation.party)
        .ok_or(Refusal::NotInvited)?;
    let serial = state.registry.serial_of(candidate).ok_or(Refusal::NotAMobile)?;
    state.registry.remove::<PartyCandidate>(candidate);
    if let Some(entry) = state.parties.get_mut(party) {
        entry.candidates.retain(|asked| *asked != serial);
    }
    let name = name_of(state, candidate);
    announce(state, party, &format!("{name} does not wish to join the party."));

    let stranded = state
        .parties
        .get(party)
        .is_some_and(|entry| entry.candidates.is_empty() && entry.members.len() <= 1);
    if stranded {
        close(state, party);
    }
    Ok(())
}

/// Take a member out of a party.
///
/// Two callers in one, which is the wire's own shape: `0x02` names a serial, and
/// the leader naming somebody else is a kick while anybody naming themselves is
/// leaving. A member naming a third party is refused.
///
/// The **leader** leaving takes the party with it. ServUO promotes nobody, and
/// this is where a party differs from a guild most sharply: a guild outlives its
/// founder because it is a thing in the world, and a party is only the people in
/// it.
pub fn remove(state: &mut WorldState, actor: EntityId, target: EntityId) -> Result<(), Refusal> {
    let party = state
        .party_of(actor)
        .map(|party| PartyId(party.leader))
        .ok_or(Refusal::NotInAParty)?;
    let actor_serial = state.registry.serial_of(actor).ok_or(Refusal::NotAMobile)?;
    let target_serial = state.registry.serial_of(target).ok_or(Refusal::NotAMobile)?;
    let leads = party.0 == actor_serial;
    if !leads && actor != target {
        return Err(Refusal::NotTheLeader);
    }
    if !state
        .parties
        .get(party)
        .is_some_and(|entry| entry.contains(target_serial))
    {
        return Err(Refusal::NotYourMember);
    }

    if target_serial == party.0 {
        return disband(state, target);
    }

    if let Some(entry) = state.parties.get_mut(party) {
        entry.members.retain(|member| *member != target_serial);
    }
    state.registry.remove::<PartyMember>(target);
    state.system_message(target, "You have been removed from the party.");
    tell_removal(state, party, target_serial);

    // Down to the leader alone: there is no party left to be in, so it closes
    // rather than leaving somebody grouped with nobody. Same rule as `decline`'s.
    if state.parties.get(party).is_some_and(|e| e.members.len() <= 1) {
        announce(state, party, "The last person has left the party.");
        close(state, party);
    } else {
        tell_roster(state, party);
    }
    Ok(())
}

/// End the party `leader` leads.
pub fn disband(state: &mut WorldState, leader: EntityId) -> Result<(), Refusal> {
    let party = may_lead(state, leader)?;
    announce(state, party, "Your party has disbanded.");
    close(state, party);
    Ok(())
}

/// Take the party out and unpick everything that named it.
///
/// Every member and every candidate loses their component here rather than
/// discovering it later. Unlike a guild's disband — which leaves a component
/// naming a dead id, because a member might be offline — a party's members are
/// online by construction, so there is nobody this cannot reach.
fn close(state: &mut WorldState, party: PartyId) {
    let Some(entry) = state.parties.close(party) else {
        return;
    };
    for serial in entry.members {
        let Some(member) = state.registry.entity_of(serial) else {
            continue;
        };
        state.registry.remove::<PartyMember>(member);
        // The empty list, which is how a client is told it is in no party — see
        // `PartyRemoveMember`. Sent per member and naming them, because that is
        // the packet's shape.
        let packet = ServerPacket::PartyRemoveMember(openshard_protocol::party::PartyRemoveMember {
            removed: serial,
            members: Vec::new(),
        });
        state.send_to(member, &packet);
    }
    for serial in entry.candidates {
        let Some(asked) = state.registry.entity_of(serial) else {
            continue;
        };
        state.registry.remove::<PartyCandidate>(asked);
        state.system_message(asked, "That party is no longer forming.");
    }
}

/// Take a player out of whatever party they were in or had been asked to,
/// because they are leaving the world.
///
/// # Why this is needed here and is not needed in the reference
///
/// ServUO's logged-out `PlayerMobile` stays in the world and stays in its party;
/// there is a body to come back to. This engine despawns the entity at
/// disconnect, so a party left alone would hold a serial that names nobody —
/// counting against the capacity, listed on everyone else's roster as a member
/// they cannot see, and keeping a party alive after the last person in it has
/// gone. That is the leak this closes.
///
/// It follows from what a party *is*, which is the same reason none of this is
/// saved: a group of people playing together right now.
///
/// Silent for a player in no party and holding no invitation, which is almost
/// everyone.
pub fn on_logout(state: &mut WorldState, player: EntityId) {
    if state.registry.get::<PartyCandidate>(player).is_some() {
        let _ = decline(state, player);
    }
    if state.party_of(player).is_some() {
        let _ = remove(state, player, player);
    }
}

/// Say whether the party may take from this member's corpse.
pub fn set_can_loot(state: &mut WorldState, member: EntityId, can_loot: bool) -> Result<(), Refusal> {
    if state.party_of(member).is_none() {
        return Err(Refusal::NotInAParty);
    }
    if let Some(entry) = state.registry.get_mut::<PartyMember>(member) {
        entry.can_loot = can_loot;
    }
    state.system_message(
        member,
        match can_loot {
            true => "You have chosen to allow your party to loot your corpse.",
            false => "You have chosen to prevent your party from looting your corpse.",
        },
    );
    Ok(())
}

/// A mobile's name, or a word that reads as one in a sentence.
pub(crate) fn name_of(state: &WorldState, entity: EntityId) -> String {
    state
        .registry
        .get::<openshard_state::Name>(entity)
        .map_or_else(|| "Someone".to_owned(), |name| name.0.clone())
}
