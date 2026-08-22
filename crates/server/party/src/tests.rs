//! The rules, over a world with nothing in it but mobiles.
//!
//! A bare [`WorldState`], as in `openshard-guilds`: no map, no terrain, no
//! gateway. Members here carry a `Client` and a session row, because unlike a
//! guild rule almost every party rule *sends a packet*, and a test that could
//! not read the outbox would be asserting half of each one.

use std::collections::{BTreeMap, HashMap};

use openshard_entities::{EntityId, Registry};
use openshard_events::EventBus;
use openshard_gateway::ConnectionId;
use openshard_protocol::access::AccessLevel;
use openshard_protocol::identity::AccountName;
use openshard_protocol::party::CAPACITY;
use openshard_protocol::serial::{Serial, SerialKind};
use openshard_protocol::version::ClientVersion;
use openshard_protocol::world::Facet;
use openshard_state::connection::Connection;
use openshard_state::harvest::Banks;
use openshard_state::rng::Rng;
use openshard_state::sectors::Sectors;
use openshard_state::{
    Client, Dialogue, FacetState, Gameplay, Name, Obstructions, PartyCandidate, PartyMember, QuestDefs,
    Regions, WorldState,
};

use crate::Refusal;

/// One tile of nothing, which is all any rule here needs to stand on.
const SIZE: u32 = 8;

fn world() -> WorldState {
    let mut facets = BTreeMap::new();
    facets.insert(
        Facet(0),
        FacetState {
            map: None,
            coarse: None,
            width: SIZE,
            height: SIZE,
            sectors: Sectors::new(SIZE, SIZE),
            obstructions: Obstructions::default(),
            boats: openshard_state::Boats::default(),
            regions: Regions::new(SIZE, SIZE),
            banks: Banks::default(),
        },
    );
    WorldState {
        registry: Registry::new(),
        bus: EventBus::new(),
        facets,
        default_facet: Facet(0),
        // A shard with no client files: an empty tiledata, not a missing one.
        tiles: openshard_uofiles::tiledata::TileData::empty(),
        multis: openshard_uofiles::multi::Multis::default(),
        players: HashMap::new(),
        connections: HashMap::new(),
        seen: HashMap::new(),
        start: (0, 0),
        rng: Rng::new(1),
        ticks: 0,
        hour: 0,
        worn: Default::default(),
        outbox: Vec::new(),
        open_containers: HashMap::new(),
        trades: Vec::new(),
        quests: QuestDefs::default(),
        dialogue: Dialogue::default(),
        guilds: openshard_state::Guilds::default(),
        alliances: openshard_state::Alliances::default(),
        parties: openshard_state::Parties::default(),
        gameplay: Gameplay::default(),
        save_requested: false,
    }
}

/// A player: a serial, a name, and a connection to send packets down.
fn player(state: &mut WorldState, id: u64, name: &str) -> EntityId {
    let (entity, _) = state
        .registry
        .spawn_with_serial(SerialKind::Mobile)
        .expect("the mobile pool is not exhausted");
    let connection = ConnectionId::from_raw(id);
    state.connections.insert(
        connection,
        Connection::new(
            ClientVersion::new(7, 0, 0, 0),
            AccountName::new("tester"),
            AccessLevel::Player,
        ),
    );
    state.players.insert(connection, entity);
    state.registry.insert(entity, Client { connection });
    state.registry.insert(entity, Name(name.to_owned()));
    entity
}

fn serial(state: &WorldState, entity: EntityId) -> Serial {
    state.registry.serial_of(entity).expect("a mobile serial")
}

/// A leader and one accepted member, the common opening.
fn a_party(state: &mut WorldState) -> (EntityId, EntityId) {
    let leader = player(state, 1, "Leader");
    let member = player(state, 2, "Member");
    crate::invite(state, leader, member).expect("a leader may ask");
    crate::accept(state, member).expect("and they may say yes");
    (leader, member)
}

/// Every packet this world has queued, drained. The id byte and its `0xBF`
/// subcommand type are what a party test cares about.
fn sent(state: &mut WorldState) -> Vec<Vec<u8>> {
    std::mem::take(&mut state.outbox)
        .into_iter()
        .map(|out| out.packet)
        .collect()
}

/// Which party subcommand types went out, in order. `0xBF` packets only.
fn party_kinds(state: &mut WorldState) -> Vec<u8> {
    sent(state)
        .into_iter()
        .filter(|packet| packet.first() == Some(&0xBF) && packet.len() > 5)
        .filter(|packet| u16::from_be_bytes([packet[3], packet[4]]) == openshard_protocol::party::SUBCOMMAND)
        .map(|packet| packet[5])
        .collect()
}

#[test]
fn asking_somebody_creates_the_party_and_does_not_join_them_to_it() {
    // An invitation is a question. The party exists so the question has
    // something to belong to, and the leader is alone in it until an answer
    // comes back — which is the state `decline` has to clean up.
    let mut state = world();
    let leader = player(&mut state, 1, "Leader");
    let asked = player(&mut state, 2, "Asked");

    crate::invite(&mut state, leader, asked).expect("a first invitation");
    let party = crate::party_of(&state, leader).expect("the leader is in it");
    assert_eq!(party.0, serial(&state, leader), "the leader's serial is the id");
    assert_eq!(crate::roster(&state, party), vec![leader]);
    assert!(state.registry.get::<PartyCandidate>(asked).is_some());
    assert!(crate::party_of(&state, asked).is_none(), "asking joined them");
    assert_eq!(party_kinds(&mut state), vec![0x07], "and they were invited");

    crate::accept(&mut state, asked).expect("yes");
    assert_eq!(crate::roster(&state, party), vec![leader, asked]);
    assert!(state.registry.get::<PartyCandidate>(asked).is_none());
    assert_eq!(
        party_kinds(&mut state),
        vec![0x01, 0x01],
        "and both of them were sent the roster"
    );
}

/// The state ServUO's `OnDecline` exists to clean up. Without it the leader is
/// left in a party of one — and the next invitation would silently reuse it,
/// which is invisible until the cap starts counting a member who is not there.
#[test]
fn a_refusal_closes_a_party_that_has_nobody_left_in_it() {
    let mut state = world();
    let leader = player(&mut state, 1, "Leader");
    let asked = player(&mut state, 2, "Asked");
    crate::invite(&mut state, leader, asked).unwrap();

    crate::decline(&mut state, asked).expect("no");
    assert!(crate::party_of(&state, leader).is_none(), "the party is gone");
    assert!(state.parties.is_empty());
    assert!(state.registry.get::<PartyCandidate>(asked).is_none());
}

/// And the other side of it: a refusal that leaves a real party standing does
/// not close it.
#[test]
fn a_refusal_leaves_a_party_that_still_has_members() {
    let mut state = world();
    let (leader, _) = a_party(&mut state);
    let asked = player(&mut state, 3, "Asked");
    crate::invite(&mut state, leader, asked).unwrap();

    crate::decline(&mut state, asked).unwrap();
    let party = crate::party_of(&state, leader).expect("still a party");
    assert_eq!(crate::roster(&state, party).len(), 2);
}

#[test]
fn the_cap_counts_the_leader_and_every_outstanding_question() {
    // Both halves matter. Counting only members lets a leader hold ten
    // invitations out and gather eleven; not counting the leader lets them
    // gather one too many.
    let mut state = world();
    let leader = player(&mut state, 1, "Leader");
    for id in 0..CAPACITY - 1 {
        let asked = player(&mut state, id as u64 + 2, "Asked");
        crate::invite(&mut state, leader, asked)
            .unwrap_or_else(|refusal| panic!("invitation {id} refused: {refusal:?}"));
    }
    let party = crate::party_of(&state, leader).expect("a party");
    assert_eq!(state.parties.get(party).map(|p| p.taken()), Some(CAPACITY));

    let one_too_many = player(&mut state, 99, "Late");
    assert_eq!(
        crate::invite(&mut state, leader, one_too_many),
        Err(Refusal::PartyIsFull)
    );
}

#[test]
fn only_the_leader_kicks_and_anybody_may_leave() {
    // One packet on the wire — `0x02` naming a serial — and two meanings. The
    // third case is the one worth refusing: a member naming somebody else.
    let mut state = world();
    let (leader, member) = a_party(&mut state);
    let third = player(&mut state, 3, "Third");
    crate::invite(&mut state, leader, third).unwrap();
    crate::accept(&mut state, third).unwrap();

    assert_eq!(
        crate::remove(&mut state, member, third),
        Err(Refusal::NotTheLeader),
        "a member may not turn out another"
    );
    assert_eq!(crate::remove(&mut state, member, member), Ok(()), "but may leave");
    assert!(crate::party_of(&state, member).is_none());
    assert_eq!(crate::remove(&mut state, leader, third), Ok(()));
}

/// Where a party differs from a guild most sharply. A guild outlives its
/// founder because it is a thing in the world; a party is only the people in it,
/// and ServUO promotes nobody.
#[test]
fn the_leader_leaving_takes_the_party_with_them() {
    let mut state = world();
    let (leader, member) = a_party(&mut state);
    let party = crate::party_of(&state, leader).expect("a party");

    crate::remove(&mut state, leader, leader).expect("a leader may leave");
    assert!(state.parties.get(party).is_none());
    assert!(crate::party_of(&state, member).is_none(), "and so does everyone");
    assert!(state.registry.get::<PartyMember>(member).is_none());
    // The empty list, which is how each client is told it is in no party.
    assert!(
        party_kinds(&mut state).contains(&0x02),
        "each member is sent a removal"
    );
}

#[test]
fn a_party_of_two_closes_when_one_of_them_goes() {
    // Not a special case in the wire, but it is one in the rules: a party of one
    // is a party with nobody to talk to.
    let mut state = world();
    let (leader, member) = a_party(&mut state);
    crate::remove(&mut state, member, member).unwrap();
    assert!(crate::party_of(&state, leader).is_none());
    assert!(state.parties.is_empty());
}

#[test]
fn a_line_reaches_the_whole_party_and_a_private_one_reaches_one_of_them() {
    let mut state = world();
    let (leader, member) = a_party(&mut state);
    let _ = sent(&mut state);

    crate::say_to_party(&mut state, leader, "regroup").expect("a member may speak");
    assert_eq!(party_kinds(&mut state), vec![0x04, 0x04], "both heard it");

    crate::say_privately(&mut state, leader, member, "you first").unwrap();
    assert_eq!(party_kinds(&mut state), vec![0x03], "and only one heard this");
}

#[test]
fn a_private_line_may_not_name_somebody_outside_the_party() {
    // The serial comes off the wire, and a client is free to name anybody on the
    // shard. Without this, party chat is a private-message channel to strangers.
    let mut state = world();
    let (leader, _) = a_party(&mut state);
    let stranger = player(&mut state, 9, "Stranger");
    assert_eq!(
        crate::say_privately(&mut state, leader, stranger, "psst"),
        Err(Refusal::NotYourMember)
    );
}

#[test]
fn speaking_with_no_party_is_refused_rather_than_broadcast() {
    let mut state = world();
    let alone = player(&mut state, 1, "Alone");
    assert_eq!(
        crate::say_to_party(&mut state, alone, "anyone?"),
        Err(Refusal::NotInAParty)
    );
}

#[test]
fn the_loot_flag_is_off_until_a_player_says_otherwise() {
    let mut state = world();
    let (_, member) = a_party(&mut state);
    assert!(!state.party_may_loot(member), "off by default");

    crate::set_can_loot(&mut state, member, true).unwrap();
    assert!(state.party_may_loot(member));
    crate::set_can_loot(&mut state, member, false).unwrap();
    assert!(!state.party_may_loot(member));
}

/// The flag is about the *party*, so it means nothing once there is none —
/// otherwise a player who allowed looting and then left would leave a corpse
/// anybody's old party could open.
#[test]
fn the_loot_flag_lapses_with_the_party() {
    let mut state = world();
    let (leader, member) = a_party(&mut state);
    crate::set_can_loot(&mut state, member, true).unwrap();
    crate::remove(&mut state, leader, leader).unwrap();
    assert!(!state.party_may_loot(member));
}

#[test]
fn nobody_is_in_two_parties() {
    let mut state = world();
    let (_, member) = a_party(&mut state);
    let other_leader = player(&mut state, 3, "Other");
    assert_eq!(
        crate::invite(&mut state, other_leader, member),
        Err(Refusal::TheyAreInAParty)
    );
    // And an outstanding question counts, so two leaders cannot both be waiting
    // on the same answer.
    let asked = player(&mut state, 4, "Asked");
    crate::invite(&mut state, other_leader, asked).unwrap();
    let third = player(&mut state, 5, "Third");
    assert_eq!(
        crate::invite(&mut state, third, asked),
        Err(Refusal::TheyAreInAParty)
    );
}

#[test]
fn a_member_may_not_ask_anybody_along() {
    let mut state = world();
    let (_, member) = a_party(&mut state);
    let stranger = player(&mut state, 9, "Stranger");
    assert_eq!(
        crate::invite(&mut state, member, stranger),
        Err(Refusal::NotTheLeader)
    );
}

#[test]
fn an_invitation_that_outlived_its_party_is_cleared_rather_than_honoured() {
    let mut state = world();
    let (leader, _) = a_party(&mut state);
    let asked = player(&mut state, 3, "Asked");
    crate::invite(&mut state, leader, asked).unwrap();

    crate::disband(&mut state, leader).unwrap();
    assert_eq!(crate::accept(&mut state, asked), Err(Refusal::NotInvited));
    assert!(state.registry.get::<PartyCandidate>(asked).is_none());
}

/// The leak this engine has and the reference does not. ServUO's logged-out
/// player stays in the world and stays in the party; ours despawns, so a party
/// left alone would hold a serial naming nobody — counted against the cap and
/// drawn on everybody else's roster as somebody they cannot see.
#[test]
fn logging_out_leaves_the_party() {
    let mut state = world();
    let (leader, member) = a_party(&mut state);
    let third = player(&mut state, 3, "Third");
    crate::invite(&mut state, leader, third).unwrap();
    crate::accept(&mut state, third).unwrap();
    let party = crate::party_of(&state, leader).expect("a party");

    crate::on_logout(&mut state, member);
    assert_eq!(
        state.parties.get(party).map(|p| p.members.len()),
        Some(2),
        "the serial is out of the roster, not merely unresolvable"
    );
    assert!(crate::party_of(&state, member).is_none());
}

#[test]
fn logging_out_while_asked_gives_the_place_back() {
    // The other half: an invitation nobody will answer would hold a place
    // against the cap for as long as the party lived.
    let mut state = world();
    let (leader, _) = a_party(&mut state);
    let asked = player(&mut state, 3, "Asked");
    crate::invite(&mut state, leader, asked).unwrap();
    let party = crate::party_of(&state, leader).expect("a party");
    assert_eq!(state.parties.get(party).map(|p| p.taken()), Some(3));

    crate::on_logout(&mut state, asked);
    assert_eq!(state.parties.get(party).map(|p| p.taken()), Some(2));
}

#[test]
fn the_leader_logging_out_takes_the_party_with_them() {
    let mut state = world();
    let (leader, member) = a_party(&mut state);
    crate::on_logout(&mut state, leader);
    assert!(state.parties.is_empty());
    assert!(crate::party_of(&state, member).is_none());
}

/// Almost everyone, every logout. It must cost nothing and say nothing.
#[test]
fn logging_out_with_no_party_is_silent() {
    let mut state = world();
    let alone = player(&mut state, 1, "Alone");
    crate::on_logout(&mut state, alone);
    assert!(state.parties.is_empty());
    assert!(sent(&mut state).is_empty());
}
