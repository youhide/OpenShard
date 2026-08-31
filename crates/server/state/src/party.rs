//! Parties: who is grouped with whom, and who has been asked.
//!
//! Shared substrate, not rules — the same split [`Guilds`](crate::Guilds) has.
//! The *system* (asking, accepting, kicking, the chat) is `openshard-party`
//! above this.
//!
//! # Why it is here and not only in the party crate
//!
//! Less forcefully than for guilds, and worth being honest about: no packet the
//! wire path builds reads a party today, because party membership does not move
//! a notoriety byte. What does read it from below is the corpse — whether the
//! party may loot one is a fact about the dead player that `openshard-items`
//! asks — and putting the membership where only one crate above could see it
//! would mean the looting rule had to be passed down instead of looked up.
//!
//! # A party does not survive a restart
//!
//! Nothing here is saved, and that is the reference's own behaviour rather than
//! an omission: ServUO's `Party` has no serialization at all, and a shard that
//! comes back up has no parties in it. It is also the honest reading — a party
//! is a group of people who are playing together *right now*, and restoring one
//! whose members are all offline would put five people in a room that is empty.

use std::collections::BTreeMap;

use openshard_protocol::serial::Serial;

/// A party's identity: **its leader's serial**.
///
/// Not a counter, because a party's leader never changes. ServUO's `Party.Remove`
/// disbands outright when the leader is the one leaving, rather than promoting
/// anybody — so the leader is fixed for the party's whole life, which makes it a
/// key rather than merely a field. A newtype anyway, so that a function taking
/// one cannot be handed any other serial by accident.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct PartyId(pub Serial);

/// One party.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Party {
    /// Who leads it, which is also its id.
    pub leader:     Serial,
    /// Everyone in it, **in order**, the leader first.
    ///
    /// A `Vec` and not a set: the order is on the wire. `PartyMemberList` sends
    /// the roster as a sequence and the client draws its party window in that
    /// sequence, so a membership that reshuffled itself between packets would
    /// make the rows jump.
    ///
    /// This is the only truth about who is in the party.
    /// [`PartyMember`](crate::components::PartyMember) is the reverse index —
    /// "which party is *this* mobile in" — and the crate above is what keeps the
    /// two in step, exactly as it does for a guild's roster.
    pub members:    Vec<Serial>,
    /// Who has been asked and has not answered.
    ///
    /// On the party as well as on the candidate, unlike a guild's invitation
    /// which lives on the invitee alone. The reason is the capacity rule: ServUO
    /// counts `Members.Count + Candidates.Count` against
    /// [`CAPACITY`](openshard_protocol::party::CAPACITY), so the party has to be
    /// able to count the questions it has out.
    pub candidates: Vec<Serial>,
}

impl Party {
    /// Whether `serial` is in it. Candidates are not members.
    #[must_use]
    pub fn contains(&self, serial: Serial) -> bool {
        self.members.contains(&serial)
    }

    /// How many places are taken — members and outstanding invitations both,
    /// which is what the reference measures against the cap.
    #[must_use]
    pub fn taken(&self) -> usize {
        self.members.len() + self.candidates.len()
    }
}

/// Every party on the shard, by its leader.
#[derive(Clone, Default, Debug)]
pub struct Parties {
    parties: BTreeMap<PartyId, Party>,
}

impl Parties {
    /// The party `id` names, if there is one.
    #[must_use]
    pub fn get(&self, id: PartyId) -> Option<&Party> {
        self.parties.get(&id)
    }

    /// The same, to change.
    pub fn get_mut(&mut self, id: PartyId) -> Option<&mut Party> {
        self.parties.get_mut(&id)
    }

    /// Every party, in leader-serial order.
    pub fn iter(&self) -> impl Iterator<Item = &Party> {
        self.parties.values()
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parties.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parties.is_empty()
    }

    /// Start a party with `leader` alone in it, or hand back the one they
    /// already lead.
    ///
    /// A party of one is a real state and not a placeholder: it is what exists
    /// between "I asked somebody" and their answer, and it is what the capacity
    /// check counts against. It goes away on its own when the invitation is
    /// declined — see `openshard_party::decline`.
    pub fn open(&mut self, leader: Serial) -> PartyId {
        let id = PartyId(leader);
        self.parties.entry(id).or_insert_with(|| {
            Party {
                leader,
                members: vec![leader],
                candidates: Vec::new(),
            }
        });
        id
    }

    /// Take a party out entirely. The callers are responsible for the
    /// components that named it — see `openshard_party::disband`.
    pub fn close(&mut self, id: PartyId) -> Option<Party> {
        self.parties.remove(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial(raw: u32) -> Serial {
        Serial::new(raw).expect("a mobile serial")
    }

    #[test]
    fn opening_a_party_twice_is_the_same_party() {
        // The leader's serial *is* the key, so this cannot mint a second one —
        // which matters because "add a member" is reached by a leader who may or
        // may not already have a party, and the caller should not have to know
        // which.
        let mut parties = Parties::default();
        let first = parties.open(serial(0x2A));
        let again = parties.open(serial(0x2A));
        assert_eq!(first, again);
        assert_eq!(parties.len(), 1);
        assert_eq!(parties.get(first).map(|p| p.members.len()), Some(1));
    }

    #[test]
    fn a_candidate_takes_a_place_before_they_answer() {
        // The capacity rule counts questions as well as people. Without it a
        // leader could ask twenty and let the first ten through.
        let mut parties = Parties::default();
        let id = parties.open(serial(0x2A));
        let party = parties.get_mut(id).expect("just opened");
        party.candidates.push(serial(0x2B));
        assert_eq!(party.taken(), 2);
        assert!(!party.contains(serial(0x2B)), "asked is not joined");
    }
}
