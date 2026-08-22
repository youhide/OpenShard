//! The rules, over a world with nothing in it but mobiles.
//!
//! A bare [`WorldState`]: no map, no terrain, no gateway. That is enough for
//! every rule here, because none of them look at the ground — and it keeps the
//! assertions about guilds rather than about a world that had to be stood up
//! first. The window's tests add a session row and a `Client`, which is all a
//! gump needs; where those packets end up is the tick's business.

use std::collections::{BTreeMap, HashMap};

use openshard_entities::{EntityId, Registry};
use openshard_events::EventBus;
use openshard_gateway::ConnectionId;
use openshard_protocol::access::AccessLevel;
use openshard_protocol::gump::{ButtonId, GumpResponse, RawButtonId, RawGumpId, RawGumpKey};
use openshard_protocol::identity::AccountName;
use openshard_protocol::serial::SerialKind;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::world::Facet;
use openshard_state::connection::Connection;
use openshard_state::harvest::Banks;
use openshard_state::rng::Rng;
use openshard_state::sectors::Sectors;
use openshard_state::{
    Client, Dialogue, FacetState, Gameplay, GuildCandidate, GuildGumpContext, GuildId, GuildMember,
    GuildPage, Obstructions, QuestDefs, Rank, Regions, TargetPurpose, WorldState,
};

use crate::{Outcome, Refusal, may_lead, roster};

/// One tile of nothing, which is all any rule here needs to stand on.
const SIZE: u32 = 8;

fn world() -> WorldState {
    let mut facets = BTreeMap::new();
    facets.insert(
        Facet(0),
        FacetState {
            terrain: None,
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
        tiles: None,
        multis: None,
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

/// A mobile with a serial, which is all a guild records about a member.
fn mobile(state: &mut WorldState) -> EntityId {
    let (entity, _) = state
        .registry
        .spawn_with_serial(SerialKind::Mobile)
        .expect("the mobile pool is not exhausted");
    entity
}

/// Found a guild with one member, the common opening.
fn a_guild(state: &mut WorldState) -> (EntityId, GuildId) {
    let leader = mobile(state);
    let guild = crate::found(state, leader, "The Silver Serpent", "OSS").expect("a first guild");
    (leader, guild)
}

/// A mobile with a client behind it, which is what a window needs: a `Client`
/// component, a session row to hang the context on, and a `players` entry so a
/// reply can find who sent it.
fn player(state: &mut WorldState, id: u64) -> (EntityId, ConnectionId) {
    let entity = mobile(state);
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
    (entity, connection)
}

/// The `0xB1` a client would send back: our gump, one button, and whatever was
/// typed into the fields.
fn reply(button: ButtonId, fields: &[(u16, &str)]) -> GumpResponse {
    GumpResponse {
        serial: RawGumpKey(0),
        gump_id: RawGumpId(crate::GUILD_GUMP.0),
        button: RawButtonId(button.0),
        switches: Vec::new(),
        text_entries: fields.iter().map(|&(id, text)| (id, text.to_owned())).collect(),
    }
}

/// What the window last drew for this player.
fn context(state: &WorldState, player: EntityId) -> Option<GuildGumpContext> {
    state.row_of(player).and_then(|row| row.guild_gump.clone())
}

#[test]
fn founding_a_guild_makes_the_founder_its_leader() {
    let mut state = world();
    let (leader, guild) = a_guild(&mut state);
    assert_eq!(may_lead(&state, leader), Ok(guild));
    assert_eq!(state.guild_of(leader).map(|g| g.id), Some(guild));
    assert_eq!(roster(&state, guild), vec![leader]);
}

#[test]
fn a_name_or_an_abbreviation_belongs_to_one_guild() {
    // The abbreviation is drawn in brackets beside a name. Two guilds sharing one
    // would make the bracket a lie, and there is nothing on screen to tell them
    // apart by.
    let mut state = world();
    a_guild(&mut state);
    let second = mobile(&mut state);
    assert_eq!(
        crate::found(&mut state, second, "the silver serpent", "TBR"),
        Err(Refusal::NameTaken),
        "case is not what makes two guilds different"
    );
    assert_eq!(
        crate::found(&mut state, second, "The Black Rose", "oss"),
        Err(Refusal::AbbreviationTaken)
    );
    assert_eq!(
        crate::found(&mut state, second, "   ", "TBR"),
        Err(Refusal::NoName)
    );
    // And the founder of one may not found another.
    let leader = state.guilds.iter().next().expect("the first guild").leader;
    let founder = state.registry.entity_of(leader).expect("its leader");
    assert_eq!(
        crate::found(&mut state, founder, "The Black Rose", "TBR"),
        Err(Refusal::AlreadyInAGuild)
    );
}

#[test]
fn a_guild_may_not_conscript() {
    // An invitation is a question, not a membership. The difference matters
    // because the answer is the player's, and a guild that could add people
    // without asking could turn a stranger orange to their own friends.
    let mut state = world();
    let (leader, guild) = a_guild(&mut state);
    let recruit = mobile(&mut state);

    crate::invite(&mut state, leader, recruit).expect("a leader may ask");
    assert!(state.guild_of(recruit).is_none(), "asking joined them");
    assert!(state.registry.has::<GuildCandidate>(recruit));

    assert_eq!(crate::accept_invitation(&mut state, recruit), Ok(guild));
    assert_eq!(state.guild_of(recruit).map(|g| g.id), Some(guild));
    assert!(
        !state.registry.has::<GuildCandidate>(recruit),
        "the invitation outlived the answer"
    );
}

#[test]
fn a_newcomer_asks_nobody_and_dismisses_nobody() {
    let mut state = world();
    let (leader, _) = a_guild(&mut state);
    let member = mobile(&mut state);
    crate::invite(&mut state, leader, member).unwrap();
    crate::accept_invitation(&mut state, member).unwrap();
    assert_eq!(
        crate::rank_of(&state, member),
        Some(Rank::Ronin),
        "and a newcomer is a Ronin, not a Member"
    );

    let stranger = mobile(&mut state);
    // `NotYourPlaceTo` and not `NotTheLeader`, which is what this asserted while
    // a guild was a leader and everybody else: the refusal is about the rank
    // now, and an Emissary would pass both of these.
    assert_eq!(
        crate::invite(&mut state, member, stranger),
        Err(Refusal::NotYourPlaceTo)
    );
    assert_eq!(
        crate::dismiss(&mut state, member, leader),
        Err(Refusal::NotYourPlaceTo)
    );
    assert_eq!(
        crate::invite(&mut state, stranger, member),
        Err(Refusal::NotInAGuild)
    );
}

#[test]
fn an_invitation_does_not_outlive_the_guild() {
    // Disbanding does not walk the roster of people it merely *asked* — they are
    // not on it. So the stale invitation is caught when it is answered, and
    // cleared rather than left to be answered again.
    let mut state = world();
    let (leader, _) = a_guild(&mut state);
    let recruit = mobile(&mut state);
    crate::invite(&mut state, leader, recruit).unwrap();
    crate::disband(&mut state, leader).unwrap();

    assert_eq!(
        crate::accept_invitation(&mut state, recruit),
        Err(Refusal::NoSuchGuild)
    );
    assert!(
        !state.registry.has::<GuildCandidate>(recruit),
        "a question nobody is left to have asked"
    );
}

#[test]
fn a_leader_may_not_walk_out_on_a_guild_that_still_has_members() {
    // The guild would be left naming a leader who is not in it, and nothing could
    // appoint another.
    let mut state = world();
    let (leader, guild) = a_guild(&mut state);
    let member = mobile(&mut state);
    crate::invite(&mut state, leader, member).unwrap();
    crate::accept_invitation(&mut state, member).unwrap();

    assert_eq!(
        crate::leave(&mut state, leader),
        Err(Refusal::PassLeadershipFirst)
    );
    crate::pass_leadership(&mut state, leader, member).expect("handing it over");
    assert_eq!(may_lead(&state, member), Ok(guild));
    crate::leave(&mut state, leader).expect("no longer the leader");
    assert_eq!(roster(&state, guild), vec![member]);
}

#[test]
fn the_last_member_leaving_disbands_rather_than_orphans() {
    let mut state = world();
    let (leader, guild) = a_guild(&mut state);
    crate::leave(&mut state, leader).expect("the last one out");
    assert!(state.guilds.get(guild).is_none(), "a guild with nobody in it");
    assert!(state.guild_of(leader).is_none());
}

#[test]
fn a_title_is_clipped_and_may_be_taken_back() {
    let mut state = world();
    let (leader, _) = a_guild(&mut state);
    let long = "Grand Warlord of the Eastern Reaches";
    crate::set_title(&mut state, leader, leader, long).unwrap();
    let title = |state: &WorldState| {
        state
            .registry
            .get::<GuildMember>(leader)
            .map(|m| m.title.clone())
            .unwrap_or_default()
    };
    assert_eq!(title(&state), "Grand Warlord of the", "clipped to 20");
    assert_eq!(title(&state).chars().count(), crate::TITLE_LIMIT);

    // Clearing it is a thing a leader is allowed to say, not an error — refusing
    // it would leave no way to undo a title at all.
    crate::set_title(&mut state, leader, leader, "  ").unwrap();
    assert_eq!(title(&state), "");
}

#[test]
fn the_label_is_what_a_click_draws() {
    let mut state = world();
    let (leader, _) = a_guild(&mut state);
    assert_eq!(state.guild_label(leader).as_deref(), Some("[OSS]"));
    crate::set_title(&mut state, leader, leader, "Warlord").unwrap();
    assert_eq!(state.guild_label(leader).as_deref(), Some("[Warlord, OSS]"));
    assert_eq!(
        state.guild_title_of(leader),
        Some(("Warlord".to_owned(), "The Silver Serpent".to_owned()))
    );

    // And nothing at all for a mobile in no guild, which is almost every one.
    let stranger = mobile(&mut state);
    assert_eq!(state.guild_label(stranger), None);
}

#[test]
fn a_war_takes_two_declarations() {
    let mut state = world();
    let (ours, one) = a_guild(&mut state);
    let theirs = mobile(&mut state);
    let two = crate::found(&mut state, theirs, "The Black Rose", "TBR").unwrap();

    assert_eq!(crate::declare_war(&mut state, ours, two), Ok(Outcome::Offered));
    assert!(
        !state.guilds.get(one).unwrap().at_war_with(two),
        "one guild's word made a war"
    );
    assert_eq!(crate::declare_war(&mut state, theirs, one), Ok(Outcome::Declared));
    assert!(state.guilds.get(one).unwrap().at_war_with(two));

    // Peace is one guild's decision, not a second handshake: the alternative is a
    // guild that cannot stop being attacked because its attacker will not agree.
    crate::make_peace(&mut state, ours, two).expect("ending it");
    assert!(!state.guilds.get(one).unwrap().at_war_with(two));
    assert!(!state.guilds.get(two).unwrap().at_war_with(one));
}

#[test]
fn a_guild_declares_on_someone_else() {
    let mut state = world();
    let (ours, one) = a_guild(&mut state);
    assert_eq!(
        crate::declare_war(&mut state, ours, one),
        Err(Refusal::NoSuchGuild),
        "at war with itself"
    );
    assert_eq!(
        crate::make_peace(&mut state, ours, GuildId(999)),
        Err(Refusal::NoSuchGuild)
    );
}

#[test]
fn disbanding_clears_the_membership_it_leaves_behind() {
    // `guild_of` already reads a membership naming a dead guild as none, which is
    // what protects an offline member. This is the other half: the component goes
    // too, so nothing is left to be restored into a guild that is gone.
    let mut state = world();
    let (leader, guild) = a_guild(&mut state);
    let member = mobile(&mut state);
    crate::invite(&mut state, leader, member).unwrap();
    crate::accept_invitation(&mut state, member).unwrap();

    crate::disband(&mut state, leader).expect("the leader's to disband");
    assert!(state.guilds.get(guild).is_none());
    assert!(!state.registry.has::<GuildMember>(member));
    assert!(!state.registry.has::<GuildMember>(leader));
    assert_eq!(roster(&state, guild), Vec::<EntityId>::new());
}

#[test]
fn the_window_a_player_with_no_guild_gets_is_the_founding_form() {
    // And it is the only page they get: a stale button from a window drawn before
    // they left lands here rather than on an empty roster.
    let mut state = world();
    let (player, connection) = player(&mut state, 1);
    crate::open(&mut state, connection);
    let drawn = context(&state, player).expect("a window");
    assert_eq!(drawn.page, GuildPage::Main);

    crate::gump::show(&mut state, player, GuildPage::Diplomacy);
    assert_eq!(context(&state, player).expect("a window").page, GuildPage::Main);
}

#[test]
fn the_founding_form_founds_what_was_typed_into_it() {
    let mut state = world();
    let (player, connection) = player(&mut state, 1);
    crate::open(&mut state, connection);

    let typed = reply(
        crate::gump::button::FOUND,
        &[
            (crate::gump::FIELD_NAME as u16, "The Silver Serpent"),
            (crate::gump::FIELD_ABBREVIATION as u16, "OSS"),
        ],
    );
    assert!(crate::handle(&mut state, connection, &typed));
    let guild = state.guild_of(player).expect("a guild");
    assert_eq!(guild.name, "The Silver Serpent");
    assert_eq!(guild.abbreviation, "OSS");
}

#[test]
fn a_reply_to_a_window_this_side_never_opened_does_nothing() {
    // The gump id is not a secret and the button is whatever the client says. The
    // context is what makes the difference, and it is taken rather than read — so
    // the same reply twice is one action.
    let mut state = world();
    let (player, connection) = player(&mut state, 1);
    let typed = reply(
        crate::gump::button::FOUND,
        &[
            (crate::gump::FIELD_NAME as u16, "The Silver Serpent"),
            (crate::gump::FIELD_ABBREVIATION as u16, "OSS"),
        ],
    );
    assert!(
        crate::handle(&mut state, connection, &typed),
        "the reply is still ours to have refused"
    );
    assert!(state.guild_of(player).is_none(), "a guild from no window");

    crate::open(&mut state, connection);
    crate::handle(&mut state, connection, &typed);
    assert!(state.guild_of(player).is_some());
    // The second press finds no context. Without that, a name already taken would
    // be the only thing stopping it.
    crate::handle(&mut state, connection, &typed);
    assert_eq!(state.guilds.len(), 1);
}

#[test]
fn a_diplomacy_row_declares_on_the_guild_that_row_drew() {
    let mut state = world();
    let (leader, connection) = player(&mut state, 1);
    let own = crate::found(&mut state, leader, "The Silver Serpent", "OSS").unwrap();
    let other = mobile(&mut state);
    let theirs = crate::found(&mut state, other, "The Black Rose", "TBR").unwrap();

    crate::gump::show(&mut state, leader, GuildPage::Diplomacy);
    let drawn = context(&state, leader).expect("a window");
    assert_eq!(drawn.guilds, vec![theirs], "the page listed its own guild");

    let war = reply(
        crate::gump::row_button(crate::gump::DIPLOMACY_BASE, crate::gump::DIPLOMACY_ACTIONS, 0, 0),
        &[],
    );
    crate::handle(&mut state, connection, &war);
    assert!(
        state.guilds.get(own).unwrap().has_declared_on(theirs),
        "row zero declared on nobody"
    );
}

#[test]
fn a_row_the_window_never_drew_names_nobody() {
    let mut state = world();
    let (leader, connection) = player(&mut state, 1);
    crate::found(&mut state, leader, "The Silver Serpent", "OSS").unwrap();
    let other = mobile(&mut state);
    crate::found(&mut state, other, "The Black Rose", "TBR").unwrap();

    crate::gump::show(&mut state, leader, GuildPage::Diplomacy);
    // One guild was listed; row four is a number the client made up.
    let forged = reply(
        crate::gump::row_button(crate::gump::DIPLOMACY_BASE, crate::gump::DIPLOMACY_ACTIONS, 4, 0),
        &[],
    );
    crate::handle(&mut state, connection, &forged);
    assert!(
        state.guilds.iter().all(|guild| guild.war_offers.is_empty()),
        "a forged row declared a war"
    );
}

#[test]
fn a_roster_row_sets_the_title_typed_beside_it() {
    let mut state = world();
    let (leader, connection) = player(&mut state, 1);
    crate::found(&mut state, leader, "The Silver Serpent", "OSS").unwrap();
    let member = mobile(&mut state);
    crate::invite(&mut state, leader, member).unwrap();
    crate::accept_invitation(&mut state, member).unwrap();

    crate::gump::show(&mut state, leader, GuildPage::Roster);
    let drawn = context(&state, leader).expect("a window");
    let row = drawn
        .members
        .iter()
        .position(|&serial| Some(serial) == state.registry.serial_of(member))
        .expect("the member was drawn");

    let set = reply(
        crate::gump::row_button(crate::gump::ROSTER_BASE, crate::gump::ROSTER_ACTIONS, row, 0),
        &[(row as u16, "Warlord")],
    );
    crate::handle(&mut state, connection, &set);
    assert_eq!(
        state
            .registry
            .get::<GuildMember>(member)
            .map(|m| m.title.as_str()),
        Some("Warlord")
    );
}

#[test]
fn the_invite_button_raises_a_cursor_only_for_a_leader() {
    let mut state = world();
    let (leader, connection) = player(&mut state, 1);
    crate::found(&mut state, leader, "The Silver Serpent", "OSS").unwrap();
    crate::gump::show(&mut state, leader, GuildPage::Main);
    crate::handle(&mut state, connection, &reply(crate::gump::button::INVITE, &[]));
    assert_eq!(state.take_target(leader), Some(TargetPurpose::GuildInvite));

    // A plain member pressing the same button — the window can outlive the rank
    // that drew it, and hiding a button hides it on one screen only.
    let (member, member_connection) = player(&mut state, 2);
    crate::invite(&mut state, leader, member).unwrap();
    crate::accept_invitation(&mut state, member).unwrap();
    crate::gump::show(&mut state, member, GuildPage::Main);
    crate::handle(
        &mut state,
        member_connection,
        &reply(crate::gump::button::INVITE, &[]),
    );
    assert_eq!(state.take_target(member), None, "a member raised a cursor");
}

/// A guild with a leader and a member at `rank`, for the rank rules below.
fn a_guild_with(state: &mut WorldState, rank: Rank) -> (EntityId, EntityId) {
    let (leader, _) = a_guild(state);
    let member = mobile(state);
    crate::invite(state, leader, member).unwrap();
    crate::accept_invitation(state, member).unwrap();
    if let Some(entry) = state.registry.get_mut::<GuildMember>(member) {
        entry.rank = rank;
    }
    (leader, member)
}

#[test]
fn the_founder_leads_and_a_new_leader_trades_places_with_the_old() {
    let mut state = world();
    let (leader, member) = a_guild_with(&mut state, Rank::Member);
    assert_eq!(crate::rank_of(&state, leader), Some(Rank::Leader));

    crate::pass_leadership(&mut state, leader, member).expect("a leader may hand it over");
    assert_eq!(crate::rank_of(&state, member), Some(Rank::Leader));
    assert_eq!(
        crate::rank_of(&state, leader),
        Some(Rank::Member),
        "and the old leader steps down to Member, not out of the guild"
    );
    assert!(may_lead(&state, member).is_ok());
    assert_eq!(may_lead(&state, leader), Err(Refusal::NotTheLeader));
}

/// ServUO's promotion condition, which is two rungs and not one. The Emissary
/// case is the whole point: promoting into the rank directly below your own
/// would let you make somebody a Warlord, who may declare wars you may not.
#[test]
fn a_promotion_stops_two_rungs_below_the_promoter() {
    let mut state = world();
    let (_, emissary) = a_guild_with(&mut state, Rank::Emissary);
    let recruit = mobile(&mut state);
    crate::invite(&mut state, emissary, recruit).expect("an Emissary recruits");
    crate::accept_invitation(&mut state, recruit).unwrap();

    assert_eq!(crate::promote(&mut state, emissary, recruit), Ok(Rank::Member));
    assert_eq!(
        crate::promote(&mut state, emissary, recruit),
        Err(Refusal::TheyOutrankYou),
        "an Emissary may not make an Emissary, nor a Warlord"
    );
}

#[test]
fn only_the_leader_promotes_into_the_rank_below_their_own() {
    let mut state = world();
    let (leader, member) = a_guild_with(&mut state, Rank::Member);
    assert_eq!(crate::promote(&mut state, leader, member), Ok(Rank::Emissary));
    assert_eq!(crate::promote(&mut state, leader, member), Ok(Rank::Warlord));
    // And no further: reaching Leader is `pass_leadership`, which is a trade
    // rather than a promotion — see that function.
    assert_eq!(
        crate::promote(&mut state, leader, member),
        Err(Refusal::NoFurtherRank)
    );
}

#[test]
fn a_ronin_is_the_floor_and_is_turned_out_rather_than_demoted() {
    let mut state = world();
    let (leader, member) = a_guild_with(&mut state, Rank::Member);
    assert_eq!(crate::demote(&mut state, leader, member), Ok(Rank::Ronin));
    assert_eq!(
        crate::demote(&mut state, leader, member),
        Err(Refusal::NoFurtherRank)
    );
    assert_eq!(crate::dismiss(&mut state, leader, member), Ok(()));
}

/// The `REMOVE_LOWEST_RANK` arm. An ordinary member may get rid of a newcomer
/// and nobody else — which is the only thing their rank lets them do to another
/// player, and the reason the flag exists apart from `REMOVE_PLAYERS`.
#[test]
fn an_ordinary_member_may_turn_out_a_ronin_and_no_one_else() {
    let mut state = world();
    let (leader, member) = a_guild_with(&mut state, Rank::Member);
    let ronin = mobile(&mut state);
    crate::invite(&mut state, leader, ronin).unwrap();
    crate::accept_invitation(&mut state, ronin).unwrap();

    let other = mobile(&mut state);
    crate::invite(&mut state, leader, other).unwrap();
    crate::accept_invitation(&mut state, other).unwrap();
    crate::promote(&mut state, leader, other).expect("to Member");

    assert_eq!(crate::dismiss(&mut state, member, ronin), Ok(()));
    assert_eq!(
        crate::dismiss(&mut state, member, other),
        Err(Refusal::NotYourPlaceTo),
        "a Member holds no REMOVE_PLAYERS, so an equal is out of reach"
    );
}

/// The trap this whole file's rank order is written around: the Warlord is the
/// higher rank and may do less. Asserted end to end rather than only on the flag
/// table, because a check written as a rank comparison would pass that table's
/// test and fail here.
#[test]
fn a_warlord_declares_wars_and_an_emissary_recruits_and_neither_does_the_other() {
    let mut state = world();
    let (leader, warlord) = a_guild_with(&mut state, Rank::Warlord);
    let emissary = mobile(&mut state);
    crate::invite(&mut state, leader, emissary).unwrap();
    crate::accept_invitation(&mut state, emissary).unwrap();
    if let Some(entry) = state.registry.get_mut::<GuildMember>(emissary) {
        entry.rank = Rank::Emissary;
    }

    let rival_leader = mobile(&mut state);
    let rival = crate::found(&mut state, rival_leader, "The Black Rose", "TBR").unwrap();

    assert_eq!(
        crate::declare_war(&mut state, warlord, rival),
        Ok(Outcome::Offered)
    );
    assert_eq!(
        crate::declare_war(&mut state, emissary, rival),
        Err(Refusal::NotYourPlaceTo),
        "an Emissary may not declare a war"
    );

    let stranger = mobile(&mut state);
    assert_eq!(crate::invite(&mut state, emissary, stranger), Ok(()));
    let another = mobile(&mut state);
    assert_eq!(
        crate::invite(&mut state, warlord, another),
        Err(Refusal::NotYourPlaceTo),
        "and a Warlord may not recruit"
    );
}

/// An alliance is the Leader's alone, and a war is not — so the two now ask for
/// two different flags, which is what splitting `propose` in half was for.
#[test]
fn an_alliance_is_out_of_a_warlords_reach() {
    let mut state = world();
    let (leader, warlord) = a_guild_with(&mut state, Rank::Warlord);
    let rival_leader = mobile(&mut state);
    let rival = crate::found(&mut state, rival_leader, "The Black Rose", "TBR").unwrap();

    assert_eq!(
        crate::invite_to_alliance(&mut state, warlord, rival, "The Compact"),
        Err(Refusal::NotYourPlaceTo)
    );
    // The war half of the same window is the Warlord's, and both ends of it: the
    // rank that declares is the rank that may stop.
    crate::declare_war(&mut state, warlord, rival).unwrap();
    assert_eq!(crate::make_peace(&mut state, warlord, rival), Ok(()));

    // And leaving is the Leader's, the same flag the invitation wanted.
    crate::invite_to_alliance(&mut state, leader, rival, "The Compact").unwrap();
    assert_eq!(
        crate::leave_alliance(&mut state, warlord),
        Err(Refusal::NotYourPlaceTo),
        "a Warlord may not take the guild out of an alliance either"
    );
}

/// Retitling yourself is its own arm of the rule. Without it an Emissary could
/// name every Ronin and never their own title, because they do not outrank
/// themselves.
#[test]
fn a_title_reaches_yourself_and_everybody_you_outrank() {
    let mut state = world();
    let (leader, emissary) = a_guild_with(&mut state, Rank::Emissary);
    let ronin = mobile(&mut state);
    crate::invite(&mut state, leader, ronin).unwrap();
    crate::accept_invitation(&mut state, ronin).unwrap();

    assert_eq!(crate::set_title(&mut state, emissary, ronin, "Recruit"), Ok(()));
    assert_eq!(
        crate::set_title(&mut state, emissary, emissary, "Master of Arms"),
        Ok(())
    );
    assert_eq!(
        crate::set_title(&mut state, emissary, leader, "Nobody"),
        Err(Refusal::TheyOutrankYou)
    );
    // And the title is not the rank: the Emissary now wears a title that says
    // something else entirely, and still holds an Emissary's permissions.
    assert_eq!(crate::rank_of(&state, emissary), Some(Rank::Emissary));
    assert_eq!(
        state
            .registry
            .get::<GuildMember>(emissary)
            .map(|m| m.title.as_str()),
        Some("Master of Arms")
    );
}

/// A guild line goes to the roster and to nobody else. The interesting half is
/// the "nobody else": ordinary speech picks listeners by distance, and these
/// three are standing on top of each other with no position component at all —
/// so a line that fell through to the broadcast would reach the stranger.
#[test]
fn a_guild_line_reaches_the_roster_and_stops_there() {
    let mut state = world();
    let (leader, _) = a_guild_with(&mut state, Rank::Member);
    let members: Vec<_> = crate::roster(&state, state.guild_of(leader).unwrap().id);
    // Both members need a client, or the outbox cannot show who heard it.
    for (id, &member) in members.iter().enumerate() {
        let connection = ConnectionId::from_raw(100 + id as u64);
        state.connections.insert(
            connection,
            Connection::new(
                ClientVersion::new(7, 0, 0, 0),
                AccountName::new("tester"),
                AccessLevel::Player,
            ),
        );
        state.players.insert(connection, member);
        state.registry.insert(member, Client { connection });
    }
    let (stranger, _) = player(&mut state, 200);
    assert!(state.guild_of(stranger).is_none());
    state.outbox.clear();

    crate::say_to_guild(
        &mut state,
        leader,
        openshard_protocol::wire::Hue(0x3B2),
        openshard_protocol::speech::Font(3),
        "regroup",
    )
    .expect("a member may speak to their guild");

    let heard: Vec<_> = std::mem::take(&mut state.outbox)
        .into_iter()
        .filter(|out| out.packet.first() == Some(&0xAE))
        .map(|out| out.connection)
        .collect();
    assert_eq!(heard.len(), 2, "both guild members, and not the stranger");
}

#[test]
fn speaking_to_a_guild_you_are_not_in_is_refused() {
    let mut state = world();
    let (alone, _) = player(&mut state, 1);
    assert_eq!(
        crate::say_to_guild(
            &mut state,
            alone,
            openshard_protocol::wire::Hue(0x3B2),
            openshard_protocol::speech::Font(3),
            "anyone?"
        ),
        Err(Refusal::NotInAGuild)
    );
}

/// The line reaches the alliance, which is now one set rather than one per
/// speaker. Until named alliances landed this reached "every guild yours has
/// allied with", so two guilds allied to the same third heard each other while
/// being strangers — see `chat.rs`'s own note.
#[test]
fn an_alliance_line_reaches_every_ally_and_is_refused_without_one() {
    let mut state = world();
    // A founder with a client, unlike `a_guild`'s: the speaker's own guild has
    // to be able to hear the line, and that is half of what this asserts.
    let (leader, _) = player(&mut state, 1);
    crate::found(&mut state, leader, "The Silver Serpent", "OSS").unwrap();
    assert_eq!(
        crate::say_to_alliance(
            &mut state,
            leader,
            openshard_protocol::wire::Hue(0x3B2),
            openshard_protocol::speech::Font(3),
            "hello"
        ),
        Err(Refusal::NoAllies),
        "an unallied guild has no alliance to speak to"
    );

    // Two more guilds, both asked into the one alliance and both answering.
    for (name, abbreviation, id) in [("The Black Rose", "TBR", 50u64), ("The Grey Owl", "TGO", 51)] {
        let (their_leader, _) = player(&mut state, id);
        let theirs = crate::found(&mut state, their_leader, name, abbreviation).unwrap();
        crate::invite_to_alliance(&mut state, leader, theirs, "The Northern Compact").unwrap();
        crate::join_alliance(&mut state, their_leader).unwrap();
    }
    state.outbox.clear();

    crate::say_to_alliance(
        &mut state,
        leader,
        openshard_protocol::wire::Hue(0x3B2),
        openshard_protocol::speech::Font(3),
        "to arms",
    )
    .expect("an allied guild may speak to its alliance");

    let heard = std::mem::take(&mut state.outbox)
        .into_iter()
        .filter(|out| out.packet.first() == Some(&0xAE))
        .count();
    assert_eq!(heard, 3, "the speaker's own guild and both allies");
}

/// An invitation, and the guild that was asked answering it.
///
/// Founding is one call and not two: a guild in no alliance that asks somebody
/// in makes one, and the partner starts pending — which is what stops an
/// alliance being a thing you can be put into by somebody else naming it.
#[test]
fn an_alliance_is_founded_by_asking_and_joined_by_answering() {
    let mut state = world();
    let (leader, own) = a_guild(&mut state);
    let their_leader = mobile(&mut state);
    let theirs = crate::found(&mut state, their_leader, "The Black Rose", "TBR").unwrap();

    assert_eq!(
        crate::join_alliance(&mut state, their_leader),
        Err(Refusal::NotAsked),
        "a guild nobody asked joined one"
    );

    let alliance = crate::invite_to_alliance(&mut state, leader, theirs, "The Northern Compact")
        .expect("a first alliance");
    assert_eq!(state.guilds.get(own).unwrap().alliance, Some(alliance));
    assert_eq!(
        state.guilds.get(theirs).unwrap().alliance,
        None,
        "being asked joined them"
    );
    assert!(!state.allied(own, theirs), "and made them allies");

    crate::join_alliance(&mut state, their_leader).expect("answering yes");
    assert_eq!(state.guilds.get(theirs).unwrap().alliance, Some(alliance));
    assert!(state.allied(own, theirs));
    assert!(state.allied(theirs, own), "and the other way, which is the point");
}

/// The third guild is in it with the second, which is what a *named* alliance
/// buys and the old pairwise relation did not: B and C never declared anything
/// about each other.
#[test]
fn a_third_guild_joins_the_alliance_rather_than_the_guild_that_asked() {
    let mut state = world();
    let (leader, _) = a_guild(&mut state);
    let mut joined = Vec::new();
    for (name, abbreviation) in [("The Black Rose", "TBR"), ("The Grey Owl", "TGO")] {
        let their_leader = mobile(&mut state);
        let theirs = crate::found(&mut state, their_leader, name, abbreviation).unwrap();
        crate::invite_to_alliance(&mut state, leader, theirs, "The Northern Compact").unwrap();
        crate::join_alliance(&mut state, their_leader).unwrap();
        joined.push(theirs);
    }
    assert!(
        state.allied(joined[0], joined[1]),
        "two guilds in one alliance are strangers to each other"
    );
}

/// The name is the alliance's own, and reading it from every invitation would
/// let any leader rename one by asking somebody in.
#[test]
fn extending_an_alliance_does_not_rename_it() {
    let mut state = world();
    let (leader, own) = a_guild(&mut state);
    let (second, third) = {
        let one = mobile(&mut state);
        let two = mobile(&mut state);
        (
            (
                one,
                crate::found(&mut state, one, "The Black Rose", "TBR").unwrap(),
            ),
            (two, crate::found(&mut state, two, "The Grey Owl", "TGO").unwrap()),
        )
    };
    let alliance = crate::invite_to_alliance(&mut state, leader, second.1, "The Northern Compact").unwrap();
    crate::join_alliance(&mut state, second.0).unwrap();

    let again = crate::invite_to_alliance(&mut state, leader, third.1, "Something Else").unwrap();
    assert_eq!(again, alliance, "a second alliance was founded");
    assert_eq!(
        state.alliances.get(alliance).unwrap().name,
        "The Northern Compact"
    );
    assert_eq!(state.guilds.get(own).unwrap().alliance, Some(alliance));
}

/// A name is claimed once, exactly as a guild's is.
#[test]
fn two_alliances_may_not_share_a_name() {
    let mut state = world();
    let (leader, _) = a_guild(&mut state);
    let one = mobile(&mut state);
    let partner = crate::found(&mut state, one, "The Black Rose", "TBR").unwrap();
    crate::invite_to_alliance(&mut state, leader, partner, "The Northern Compact").unwrap();

    let two = mobile(&mut state);
    let other = crate::found(&mut state, two, "The Grey Owl", "TGO").unwrap();
    let three = mobile(&mut state);
    let another = crate::found(&mut state, three, "The Ash", "ASH").unwrap();
    assert_eq!(
        crate::invite_to_alliance(&mut state, two, another, "the northern compact"),
        Err(Refusal::NameTaken)
    );
    assert_eq!(
        crate::invite_to_alliance(&mut state, two, another, ""),
        Err(Refusal::NoName)
    );
    assert_eq!(state.guilds.get(other).unwrap().alliance, None);
}

/// Green and orange cannot both be true, so the two are refused in both orders.
#[test]
fn a_war_and_an_alliance_refuse_each_other() {
    let mut state = world();
    let (leader, own) = a_guild(&mut state);
    let their_leader = mobile(&mut state);
    let theirs = crate::found(&mut state, their_leader, "The Black Rose", "TBR").unwrap();

    crate::declare_war(&mut state, leader, theirs).unwrap();
    crate::declare_war(&mut state, their_leader, own).unwrap();
    assert_eq!(
        crate::invite_to_alliance(&mut state, leader, theirs, "The Northern Compact"),
        Err(Refusal::AtWarWithThem)
    );

    // The other order: allied first, and then the war is what is refused.
    crate::make_peace(&mut state, leader, theirs).unwrap();
    crate::invite_to_alliance(&mut state, leader, theirs, "The Northern Compact").unwrap();
    crate::join_alliance(&mut state, their_leader).unwrap();
    assert_eq!(
        crate::declare_war(&mut state, leader, theirs),
        Err(Refusal::AlliedWithThem)
    );
}

/// And the third way in: joining an alliance that holds a guild you are at war
/// with. The invitation checked the inviter, which is not the same set.
#[test]
fn joining_is_refused_while_at_war_with_anybody_inside() {
    let mut state = world();
    let (leader, own) = a_guild(&mut state);
    let second = mobile(&mut state);
    let partner = crate::found(&mut state, second, "The Black Rose", "TBR").unwrap();
    crate::invite_to_alliance(&mut state, leader, partner, "The Northern Compact").unwrap();
    crate::join_alliance(&mut state, second).unwrap();

    // A third at war with the *partner*, not with the guild that asks it in.
    let third = mobile(&mut state);
    let newcomer = crate::found(&mut state, third, "The Grey Owl", "TGO").unwrap();
    crate::declare_war(&mut state, third, partner).unwrap();
    crate::declare_war(&mut state, second, newcomer).unwrap();
    crate::invite_to_alliance(&mut state, leader, newcomer, "ignored").unwrap();
    assert_eq!(
        crate::join_alliance(&mut state, third),
        Err(Refusal::AtWarWithThem)
    );
    assert_eq!(state.guilds.get(newcomer).unwrap().alliance, None);
    assert!(!state.allied(own, newcomer));
}

/// Leaving, declining, and the disband that follows the second-to-last guild
/// going — one button, and all three ends of it.
#[test]
fn leaving_an_alliance_of_two_takes_the_alliance_with_it() {
    let mut state = world();
    let (leader, own) = a_guild(&mut state);
    assert_eq!(crate::leave_alliance(&mut state, leader), Err(Refusal::NotAllied));

    let their_leader = mobile(&mut state);
    let theirs = crate::found(&mut state, their_leader, "The Black Rose", "TBR").unwrap();
    let alliance = crate::invite_to_alliance(&mut state, leader, theirs, "The Northern Compact").unwrap();

    // Declining is the same call, and it leaves the alliance standing with one
    // member — which the *inviter* leaving is then what disbands.
    crate::leave_alliance(&mut state, their_leader).expect("declining");
    assert!(state.alliances.get(alliance).is_some(), "a decline dissolved it");
    assert!(
        state.alliances.get(alliance).unwrap().pending.is_empty(),
        "and left the question standing"
    );

    crate::join_alliance(&mut state, their_leader).expect_err("no longer asked");
    crate::leave_alliance(&mut state, leader).expect("the founder leaving");
    assert!(
        state.alliances.get(alliance).is_none(),
        "an alliance of none stood"
    );
    assert_eq!(state.guilds.get(own).unwrap().alliance, None);
    assert_eq!(
        state.guilds.get(theirs).unwrap().alliance,
        None,
        "a guild was left naming an alliance that is gone"
    );
}

/// Three in, one out: the alliance stands, and the leaver is on its own.
#[test]
fn leaving_an_alliance_of_three_leaves_it_standing() {
    let mut state = world();
    let (leader, own) = a_guild(&mut state);
    let mut joined = Vec::new();
    for (name, abbreviation) in [("The Black Rose", "TBR"), ("The Grey Owl", "TGO")] {
        let their_leader = mobile(&mut state);
        let theirs = crate::found(&mut state, their_leader, name, abbreviation).unwrap();
        crate::invite_to_alliance(&mut state, leader, theirs, "The Northern Compact").unwrap();
        crate::join_alliance(&mut state, their_leader).unwrap();
        joined.push((their_leader, theirs));
    }
    let alliance = state.guilds.get(own).unwrap().alliance.expect("an alliance");

    crate::leave_alliance(&mut state, leader).expect("the founder leaving");
    let standing = state.alliances.get(alliance).expect("two members is an alliance");
    assert_ne!(standing.leader, own, "it is still led by the guild that left");
    assert!(standing.contains(standing.leader), "and led from outside itself");
    assert!(
        state.allied(joined[0].1, joined[1].1),
        "the two left in it drifted apart"
    );
    assert!(!state.allied(own, joined[0].1));
}
