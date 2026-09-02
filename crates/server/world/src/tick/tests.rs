use openshard_chat::MobileSpoke;
use openshard_combat::{
    MobileDamaged,
    MobileDied,
    WRESTLING_SPEED,
    swing_ticks,
};
use openshard_config::CombatEra;
use openshard_events::Cursor;
use openshard_magic::SpellCast;
use openshard_movement::WALK_INTERVAL;
use openshard_movement::scene::Scene;
use openshard_protocol::casting::SpellId;
use openshard_protocol::containers::GridSlot;
use openshard_protocol::feedback::{
    ActionStage,
    CombatActionBalked,
    CombatActionEnded,
    CombatActionOutcome,
    CombatActionPhase,
    CombatActionStage,
    InterruptReason,
};
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::items::DropDestination;
use openshard_protocol::mobile::Remove;
use openshard_protocol::packet::encode_packet;
use openshard_protocol::serial::RawSerial;
use openshard_protocol::skill::SkillLock;
use openshard_protocol::wire::{
    Graphic,
    Hue,
    RawSkillId,
    SoundId,
};
use openshard_protocol::world::{
    Aggression,
    RangedRange,
    RawStepSequence,
    TurnRequest,
};
use openshard_skills::SkillUsed;
use openshard_state::action_rules::{
    ActionEffect,
    ActionRules,
    ConditionEffects,
    ConditionSet,
};
use openshard_state::components::{
    ActionKind,
    Amount,
    Banker,
    CombatAction,
    Contained,
    Container,
    Corpse,
    CorpseBody,
    CriminalUntil,
    Decays,
    Drawn,
    Equipped,
    ItemAffix,
    MurderDecay,
    Murders,
    Phase,
    Riding,
    Route,
    RouteRefused,
    Skills,
    Stackable,
    SwingSpeed,
    Watch,
    WrestlingCombo,
    WrestlingOpener,
    WrestlingStride,
};
use openshard_state::sectors::distance;
use openshard_state::{
    SettledItemLocation,
    Skill,
    StatLock,
};

use super::*;

pub(super) const START: Tile = Tile::new(1363, 1600);

/// A generous upper bound on ticks-per-beat, so a test loop that waits "a few
/// beats" survives any cadence the defaults settle on.
///
/// It has to cover the *spread* as well as the interval. A beat is armed as
/// `interval + rng.below(beat_jitter(interval))` (`npc::next_beat`) —
/// deliberately, so a crowd does not act in unison — and a loop sized to the bare
/// interval turns every "wait one beat" here into a coin flip on the seed. The
/// widest beat these tests wait on is the idle amble, twice the default creature
/// step.
///
/// Derived from that step rather than written out: as the bare `16` it used to
/// be, it was two ambles at the 50ms tick and one at the 25ms one, which turned
/// every "wait a beat" loop below into a coin flip the day the tick changed.
pub(super) const AI_THINK_TICKS: u64 = {
    let amble = 2 * Gameplay::ticks_from_ms(400);
    amble + amble / openshard_npc::BEAT_JITTER_FRACTION
};

/// Ticks a bare-handed, default-dexterity mobile waits between swings under
/// the default rules — the pace the combat tests reckon against. `dex 100`,
/// wrestling, era 1, scale 10000: twenty ticks (one second).
const WRESTLING_SWING_TICKS: u64 = swing_ticks(100, WRESTLING_SPEED, 1, 10000);

#[test]
fn bare_hands_take_one_second_at_default_dexterity() {
    assert_eq!(WRESTLING_SWING_TICKS, TICKS_PER_SECOND);
}

#[test]
fn command_work_over_the_tick_budget_stays_fifo_for_the_next_tick() {
    let now = Instant::now();
    let mut world = world();
    let missing = Serial::new(1).unwrap();
    for value in 0..300u16 {
        world.queue(Command::SetSkill {
            serial: missing,
            skill: 0,
            value,
        });
    }
    world.tick(now);
    assert_eq!(world.queued(), 300 - super::MAX_COMMAND_WORK_PER_TICK);
    world.tick(now + TICK_INTERVAL);
    assert_eq!(world.queued(), 0);
}

#[test]
fn catalogue_opens_are_coalesced_and_separately_budgeted() {
    let now = Instant::now();
    let mut world = world();
    let repeated = ConnectionId::from_raw(1);
    for _ in 0..10 {
        world.queue(Command::OpenCraftCatalogue { connection: repeated });
    }
    for id in 2..=40 {
        world.queue(Command::OpenCraftCatalogue {
            connection: ConnectionId::from_raw(id),
        });
    }
    world.tick(now);
    assert_eq!(world.queued(), 40 - super::MAX_CATALOGUE_OPENS_PER_TICK);
    world.tick(now + TICK_INTERVAL);
    assert_eq!(world.queued(), 0);
}

pub(super) fn world() -> World {
    World::new(START)
}

/// Regression for the kiting race: the client decided this input was a turn,
/// then combat turned the authoritative body before the request arrived. A
/// legacy `0x02` would now be reinterpreted as a step; the typed request must
/// still leave the position untouched.
#[test]
fn a_turn_request_cannot_become_a_step_after_combat_already_turned_the_body() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let before = world.state.registry.get::<Position>(entity).unwrap().0;
    let east = Facing::walking(Direction::East);
    let Movement(mut walker) = *world.state.registry.get::<Movement>(entity).unwrap();
    walker.facing = east;
    world.state.registry.insert(entity, Movement(walker));
    world.state.registry.insert(entity, Heading(east));
    let _ = world.drain_outbound().count();

    world.queue(Command::Turn {
        connection,
        request: TurnRequest {
            facing:   east,
            sequence: RawStepSequence(0),
        },
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        before,
        "an already-satisfied turn is acknowledged, never re-derived as a step"
    );
    assert!(
        world
            .drain_outbound()
            .any(|out| out.packet.first() == Some(&0x22)),
        "the turn remains in the ordered movement acknowledgement stream"
    );
    assert!(
        !world.state.registry.has::<LastStep>(entity),
        "a turn does not leave combat's movement marker behind"
    );
}

/// The flags a fixture wall carries: impassable, and a wall for the sight line.
pub(super) const WALL_FLAGS: u64 = openshard_tiles::TileFlags::WALL | openshard_tiles::TileFlags::BLOCK;

/// A tiledata a fixture can hand a house or a ship: whichever graphics the test
/// names, with the flags and heights it means them to have.
///
/// **A real table, not a double.** These used to be answered by a hand-written
/// `Terrain` — `item_blocks` returning `graphic == WALL` — which meant the test
/// agreed with itself about a question the shard asks `tiledata.mul`. One `.mul`
/// row written here is the same indirection the real file has, so a fixture
/// cannot test a shortcut the shard does not take.
pub(super) fn tiles_with(entries: &[(u16, u64, u8)]) -> openshard_tiles::TileData {
    let mut tiles = openshard_tiles::TileData::empty();
    for &(graphic, flags, height) in entries {
        tiles.set_static_tile(
            graphic,
            openshard_tiles::StaticTile {
                flags: openshard_tiles::TileFlags::new(flags),
                height,
                ..openshard_tiles::StaticTile::default()
            },
        );
    }
    tiles
}

/// A multi table holding one shape under one id — what a fixture house or ship
/// is made of, read the way the shard reads it.
pub(super) fn multis_with(
    id: u16,
    components: Vec<openshard_uofiles::multi::Component>,
) -> openshard_uofiles::multi::Multis {
    openshard_uofiles::multi::Multis::of([openshard_uofiles::multi::Multi::new(id, components)])
}

/// The long-distance guide shares a facet's static-terrain lifetime, and it is
/// *handed in* rather than built: sampling a whole facet costs minutes, so the
/// shard bakes the graph to a file and loads it (`movement::bake`), which is
/// what `with_facet`'s third argument is for. `with_terrain` is the same door
/// with no graph, and a facet loaded through it has none — that is a fact worth
/// pinning, because "the router is absent" and "the router is missing" look
/// identical from a caller and only one of them is a bug.
///
/// No AI consumes it yet. This proves the capability is reachable rather than a
/// client-only cache.
#[test]
fn a_facet_keeps_the_coarse_router_it_was_given_and_no_other() {
    use openshard_map::grid::BlockExtent;
    use openshard_map::map::{
        LandCell,
        WorldMap,
    };
    use openshard_map::snapshot::MapSnapshot;
    use openshard_movement::{
        MapTerrain,
        NavigationGraph,
    };
    use openshard_protocol::world::Facet;
    use openshard_tiles::{
        LandTileId,
        TileData,
    };

    let flat = || {
        WorldMap::from_blocks(BlockExtent { wide: 1, down: 1 }, |_, _| {
            LandCell {
                tile: LandTileId(0),
                z:    0,
            }
        })
    };
    let snapshot = || MapSnapshot::new(Facet(0), flat());

    // A map alone: no graph was baked, so the facet reports none.
    let unbaked = World::new(START).with_map(snapshot());
    assert_eq!(
        unbaked.state.facet_state(Facet(0)).coarse_router().map(|_| ()),
        None,
        "a facet loaded without a baked graph must not appear to have one"
    );

    // The same map with its graph: kept, and over the map's own extent.
    // Nothing live over it, because a baked graph is the *static* connectivity
    // of a facet.
    let nothing_placed = openshard_map::overlay::Overlay::default();
    let nothing_over = |map, tiles, spans| {
        openshard_movement::Footing::new(
            Some(MapTerrain::new(map, tiles, spans)),
            &nothing_placed,
            Doors::AsTheyStand,
        )
    };
    let empty = TileData::empty();
    let map = flat();
    let spans = openshard_movement::spans::SpanIndex::build(&map, &empty);
    let baked =
        NavigationGraph::build(&nothing_over(&map, &empty, &spans), 8, 8).expect("an 8x8 facet has a graph");
    let loaded = World::new(START).with_facet(
        Facet(0),
        snapshot(),
        Some(baked),
        FacetRules::classic(Facet(0)),
        None,
    );
    assert_eq!(
        loaded
            .state
            .facet_state(Facet(0))
            .coarse_router()
            .map(NavigationGraph::dimensions),
        Some((8, 8))
    );
}

/// The first id of the band [`connection`] mints from.
///
/// Deliberately far above every connection id written by hand in these tests —
/// the largest is `1000`, the loner in `interest_tests` — so a minted id can
/// never *be* one a test also wrote as a literal. That is the property, not the
/// number: a test that says `enter_as(.., ConnectionId::from_raw(2), ..)` beside
/// an `enter` must get two connections, and it would silently get one if the
/// counter ever wandered into the small numbers.
const MINTED_CONNECTIONS: u64 = 1 << 20;

thread_local! {
    /// How many ids [`connection`] has minted on this thread.
    ///
    /// Thread-local rather than a process-wide atomic because libtest gives each
    /// test its own thread in a parallel run, so the sequence a test sees is its
    /// own and does not depend on what else happened to be running. Nothing may
    /// depend on the *values*, though: with `--test-threads=1` libtest runs on
    /// the main thread and the counter is shared by every test in the binary.
    /// Uniqueness is what is promised here, and uniqueness holds either way
    /// because the counter only ever goes up.
    static MINTED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A connection id nobody else has.
///
/// This used to hand back `ConnectionId::from_raw(1)` every time, which made
/// `enter(&mut world, now)` twice one connection entering twice rather than two
/// players — `WorldState::players` is keyed by connection, so the second `Enter`
/// quietly replaced the first, and the assertions about "the other player" ran
/// against a scene with one player in it and passed.
///
/// A test that needs a *known* id — one it will name again in a packet, or match
/// against a literal — says `enter_as` and keeps its own.
pub(super) fn connection() -> ConnectionId {
    MINTED.with(|minted| {
        let next = minted.get() + 1;
        minted.set(next);
        ConnectionId::from_raw(MINTED_CONNECTIONS + next)
    })
}

/// Whether nobody at all has anything on their cursor.
///
/// The cursor is a field on each connection's row rather than a map of its own,
/// so "the world holds nothing" is a question about every row and not about one
/// map being empty. Worth keeping in that wider form: a drag that bounced onto
/// the *wrong* connection would still leave the right one clear.
pub(super) fn nothing_is_held(world: &World) -> bool {
    world.state.connections.values().all(|row| row.held.is_none())
}

/// Put "admin"/"Lord British" on file as if a previous run had saved it there,
/// so the next `Character::Saved` entry restores it.
///
/// This is the boot path — [`World::restore_characters`] is what the shard calls
/// with the store's rows — and it is how a test builds a *stored* character now
/// that the roster is the world's: the row goes in, and `Enter` names it. There
/// is no longer a way to hand `enter` a character it has nothing on file for,
/// which is the point of S4 and also the reason this helper exists.
///
/// Everything but the serial, the spot and the look is what
/// `CharacterSheet::starting()` describes, so a character restored through here
/// is the same one the hand-built `StoredCharacter` fixtures used to be.
/// Hands back what `World::restore_characters` does, because a test that goes on
/// to restore items needs it — that is the whole point of the token, and a test
/// is not exempt from the order it states.
pub(super) fn on_file(
    world: &mut World,
    serial: Serial,
    position: Point,
    appearance: Appearance,
) -> RestoredCharacters {
    world.restore_characters(vec![CharacterRecord {
        serial,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Lord British".to_owned()),
        body: appearance.body.0,
        hue: appearance.hue.0,
        facet: 0,
        x: position.x,
        y: position.y,
        z: position.z,
        facing: Facing::walking(Direction::South).to_bits(),
        strength: DEFAULT_HITPOINTS,
        dexterity: DEFAULT_DEXTERITY,
        intelligence: DEFAULT_MANA,
        skills: Vec::new(),
        stat_locks: openshard_persistence::StatLockRecord::default(),
        effects: Vec::new(),
        dead: false,
        fame: 0,
        karma: 0,
        murders: 0,
        quests: Vec::new(),
        done_quests: Vec::new(),
        guild: None,
        guild_title: String::new(),
        guild_rank: 0,
        guild_candidate: None,
    }])
}

/// Walk the boot restore up to the mobiles with nothing in it, for a test whose
/// subject is the mobiles alone.
///
/// [`World::restore_mobiles`] takes the [`RestoredItems`] that only
/// [`World::restore_items`] can hand back, which in turn takes what only
/// [`World::restore_characters`] can — the ordering rule, as a signature. A test
/// restoring mobiles out of a snapshot with no characters and no items in it
/// still has to walk that, and this is the walk, written once rather than as two
/// lines and a comment in eight places.
///
/// Deliberately not a constructor for the token: a way to build one without
/// running the restore is the order back as a convention, inside the very crate
/// that defines it.
pub(super) fn nothing_restored_first(world: &mut World) -> RestoredItems {
    let characters = world.restore_characters(Vec::new());
    world.restore_items(Vec::new(), &characters)
}

/// Hand a connection over to the world as "admin", the way the shard loop does
/// when the login conversation finishes. What the character screen's own packets
/// need: the account, the authority and the client version all live on the
/// connection's row after this, and nothing has picked a character.
pub(super) fn authenticate(world: &mut World, connection: ConnectionId, now: Instant) {
    world.queue(Command::Authenticated {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        access: AccessLevel::Player,
    });
    world.tick(now);
}

/// Delete a character off "admin"'s list from a second connection sitting on the
/// character screen — which is the only way a real `0x83` ever arrives, since the
/// connection playing a character is not the one deleting it.
pub(super) fn delete_slot(world: &mut World, slot: u32, now: Instant) -> ConnectionId {
    // Minted rather than a literal: the caller is already playing on a
    // connection of its own, and a helper that picked a fixed number would be
    // one collision away from deleting from the connection it means to test.
    let screen = connection();
    authenticate(world, screen, now);
    world.queue(Command::DeleteCharacter {
        connection: screen,
        slot:       openshard_protocol::wire::RawCharacterSlot(slot),
    });
    world.tick(now);
    screen
}

pub(super) fn enter(world: &mut World, now: Instant) -> ConnectionId {
    enter_as(world, connection(), now)
}

pub(super) fn enter_as(world: &mut World, connection: ConnectionId, now: Instant) -> ConnectionId {
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Lord British".to_owned()),
        access: AccessLevel::Player,
        character: Character::fresh(Facet(0)),
    }));
    world.tick(now);
    connection
}

/// Enter as a game master — the authority the `.`-command tests need.
pub(super) fn enter_gm(world: &mut World, now: Instant) -> ConnectionId {
    let connection = connection();
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Lord British".to_owned()),
        access: AccessLevel::GameMaster,
        character: Character::fresh(Facet(0)),
    }));
    world.tick(now);
    connection
}

/// Every packet the last tick produced for one connection, leaving the rest of
/// the queue where it was.
///
/// The partition is the point. This used to drain the whole outbound queue and
/// filter it, so two calls in a row were not two questions: the first emptied
/// the world and the second answered about nothing — silently, and with an
/// assertion that passed. In a test with one connection the two behaviours are
/// indistinguishable, which is exactly why it survived in dozens of them; the
/// first test with two connections is where it turns a green run into a proof
/// of nothing.
pub(super) fn packets_for(world: &mut World, connection: ConnectionId) -> Vec<Vec<u8>> {
    let mut asked_about = Vec::new();
    let mut everyone_else = Vec::new();
    for out in world.drain_outbound() {
        if out.connection == connection {
            asked_about.push(out.packet);
        } else {
            everyone_else.push(out);
        }
    }
    // Several tests assert on the order a connection was sent things in, so the
    // packets left behind keep theirs: the drain emptied the queue and nothing
    // can have queued anything since, so writing them back is the identity on
    // the connections this call did not ask about.
    world.state.outbox = everyone_else;
    asked_about
}

/// Put an entity somewhere directly, as if it had walked there.
pub(super) fn teleport(world: &mut World, connection: ConnectionId, point: Point) {
    let entity = world.state.players[&connection];
    world.state.registry.insert(entity, Position(point));
    if let Some(Movement(mut walker)) = world.state.registry.get::<Movement>(entity).copied() {
        walker.position = point;
        world.state.registry.insert(entity, Movement(walker));
    }
    let facet = world.state.facet_of(entity);
    world.state.place_mobile(facet, entity, point);
    world.state.refresh_around(entity);
}

pub(super) fn walk(sequence: u8, direction: Direction) -> WalkRequest {
    use openshard_protocol::world::RawFastwalkKey;

    WalkRequest {
        facing:       Facing::walking(direction),
        sequence:     RawStepSequence(sequence),
        fastwalk_key: RawFastwalkKey(0),
    }
}

/// The same step, taken at a run — the bit the condition rules of
/// `docs/combat_actions.md`'s D4 key on.
pub(super) fn run(sequence: u8, direction: Direction) -> WalkRequest {
    use openshard_protocol::world::RawFastwalkKey;

    WalkRequest {
        facing:       Facing::running(direction),
        sequence:     RawStepSequence(sequence),
        fastwalk_key: RawFastwalkKey(0),
    }
}

/// The serial the world gave the character a connection is driving.
pub(super) fn serial_of(world: &World, connection: ConnectionId) -> Serial {
    let entity = world.state.players[&connection];
    world.state.registry.serial_of(entity).unwrap()
}

#[test]
fn entering_twice_through_the_helper_is_two_players_and_not_one() {
    // What `connection()` minting a fresh id each call is for. It used to hand
    // back `1` every time, so this test's two `enter`s were one connection
    // entering twice: `players` is keyed by connection, the second `Enter`
    // replaced the first, and a scene meant to hold two players held one — with
    // every assertion about "the other player" passing against it.
    //
    // Both halves are asserted. The ids being different is the mechanism; the
    // world holding two players is the consequence, and it is the consequence
    // the tests that reach for this helper actually rely on.
    let now = Instant::now();
    let mut world = world();
    let first = enter(&mut world, now);
    let second = enter(&mut world, now);

    assert_ne!(first, second, "two calls, two connections");
    assert_eq!(world.player_count(), 2, "and two players in the world, not one");
    assert_ne!(
        world.state.players[&first], world.state.players[&second],
        "each driving a character of its own"
    );
}

#[test]
fn a_minted_connection_is_never_one_a_test_wrote_by_hand() {
    // The band of `MINTED_CONNECTIONS`, pinned. Tests routinely say
    // `enter(&mut world, now)` and then `enter_as(.., ConnectionId::from_raw(2),
    // ..)` for the second player; if a minted id ever landed on a small literal
    // the two would be one connection again, and this time invisibly, because
    // the helper would look like it was doing its job.
    //
    // `1000` is the largest hand-written id in this crate's tests — the loner in
    // `interest_tests`. The assertion is the gap, not the constant.
    for _ in 0..8 {
        assert!(
            connection().get() > 1000,
            "a minted id must sit above every id these tests write as a literal"
        );
    }
}

#[test]
fn asking_what_one_connection_was_sent_leaves_the_other_its_own() {
    // `packets_for` is a question, and two of them in a row are two answers.
    // The trap this closes is the version that drained the whole queue and
    // filtered: the first call emptied the world and the second answered
    // "nothing", which reads exactly like a connection the world never spoke
    // to — silently, and with a passing assertion.
    //
    // Both halves are asserted, because the negative one alone would be green
    // in a world where nothing reaches anybody.
    let now = Instant::now();
    let mut world = world();
    // Named rather than minted: this test is about two *particular* connections
    // and reads better naming them, and the ids being distinct is asserted below
    // either way.
    let alice = enter_as(&mut world, ConnectionId::from_raw(1), now);
    let bob = enter_as(&mut world, ConnectionId::from_raw(2), now);
    assert_ne!(alice, bob, "two connections, not one asked about twice");

    let to_alice = packets_for(&mut world, alice);
    assert!(!to_alice.is_empty(), "entering the world says something to alice");

    let to_bob = packets_for(&mut world, bob);
    assert!(
        !to_bob.is_empty(),
        "and asking about alice did not take bob's answer"
    );

    assert!(
        packets_for(&mut world, alice).is_empty(),
        "what was answered for is gone: the queue is emptied of what it hands back"
    );
}

/// **A decreed step turns and moves in the same beat**, where a client's walk
/// turns first and moves on the next request.
///
/// This used to be one rule for both, on the argument that the clients watching
/// cannot tell who ordered the step — which is true about the *picture* and says
/// nothing about the *cost*. Turn-as-step exists so that a client sending a
/// direction over a lossy sequence and a server answering it stay in step; there
/// is no request, no acknowledgement and no sequence behind a decree, so the
/// mobile was paying for a protocol it was not speaking. What it cost was one
/// beat out of every direction change, and that beat is what stopped a kiting
/// archer from ever opening a gap once its shot began turning it to face its
/// mark. The reference moves a creature the same way — `BaseAI.DoMove` sets the
/// direction and moves in one call, which is why a monster walks a diagonal
/// instead of pirouetting on to it.
#[test]
fn a_server_step_turns_and_moves_in_one_beat() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let _ = packets_for(&mut world, connection);

    let facing0 = world.state.registry.get::<Heading>(entity).unwrap().0.direction;
    let dir = if facing0 == Direction::North {
        Direction::South
    } else {
        Direction::North
    };
    let from = world.state.registry.get::<Position>(entity).unwrap().0;

    let mut moved: Cursor<MobileMoved> = world.bus().cursor();
    let mut turned: Cursor<MobileTurned> = world.bus().cursor();

    world.queue(Command::Step {
        serial,
        direction: dir.to_bits(),
    });
    world.tick(now);
    assert_eq!(
        world.bus().read(&mut turned).count(),
        1,
        "the body did turn — this is a step in a direction it was not facing"
    );
    let moves: Vec<MobileMoved> = world.bus().read(&mut moved).copied().collect();
    assert_eq!(moves.len(), 1, "and it moved on the same beat");
    assert_eq!(moves[0].from, from);
    assert_eq!(moves[0].to, openshard_movement::step_from(from, dir).unwrap());
    assert_eq!(moves[0].facing.direction, dir, "facing the way it went");
    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        openshard_movement::step_from(from, dir).unwrap(),
    );
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|packet| packet.first() == Some(&0x20)),
        "a server-decreed player step synchronizes the owning client"
    );
}

/// The half of the old rule that survives: a decree that turns into a wall is
/// still a turn, and every screen is owed it.
///
/// The turn is written before the landing is tested, so the refusal path is the
/// one place that has to broadcast it — a body facing a way no packet mentioned
/// is the desync this whole area keeps producing.
#[test]
fn a_decreed_step_into_a_wall_still_announces_the_turn() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let from = world.state.registry.get::<Position>(entity).unwrap().0;
    // A body of our own in the way: the crowd rule refuses a step on to an
    // occupied tile, and it needs no terrain fixture to set up.
    let facing0 = world.state.registry.get::<Heading>(entity).unwrap().0.direction;
    let dir = if facing0 == Direction::North {
        Direction::South
    } else {
        Direction::North
    };
    let blocked = openshard_movement::step_from(from, dir).unwrap();
    let _blocker = spawn_mobile_at(&mut world, blocked, 50, now);
    // And a step of stamina spent, or the shove would carry the body through: a
    // decreed step is held to the walked one's shove rule, and a rested mobile
    // is allowed to push. What is under test is the refusal, not the shove.
    tire(&mut world, entity);
    let _ = packets_for(&mut world, connection);

    let mut moved: Cursor<MobileMoved> = world.bus().cursor();
    let mut turned: Cursor<MobileTurned> = world.bus().cursor();
    world.queue(Command::Step {
        serial,
        direction: dir.to_bits(),
    });
    world.tick(now);
    assert_eq!(world.bus().read(&mut turned).count(), 1, "it turned");
    assert_eq!(world.bus().read(&mut moved).count(), 0, "and went nowhere");
    assert_eq!(
        world.state.registry.get::<Heading>(entity).unwrap().0.direction,
        dir,
        "the facing is the new one"
    );
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|packet| packet.first() == Some(&0x20)),
        "and the owning client was told, or it draws a body facing the old way"
    );
}

/// Take `entity` one point off a full stamina pool, which is the whole of the
/// difference between shoving and being stopped.
///
/// One point, and not "empty": ServUO's test is `Stam == StamMax`, so the
/// interesting case is the player who is *almost* rested. A test that drained
/// the pool would pass against a `>= 10` rule this one refuses.
fn tire(world: &mut World, entity: EntityId) {
    let &openshard_state::components::Stamina { current, max } = world
        .state
        .registry
        .get::<openshard_state::components::Stamina>(entity)
        .expect("a player carries a stamina pool");
    assert_eq!(current, max, "a player enters the world rested");
    world.state.registry.insert(
        entity,
        openshard_state::components::Stamina {
            current: current - 1,
            max,
        },
    );
}

/// **A rested player shoves past a body rather than stopping at it** —
/// `Mobile.CheckShove`, and the rule this engine used to contradict the stock
/// client about on every facet.
///
/// The three halves of the claim are asserted together because separating them
/// would let a shove that costs nothing, or one that says nothing, pass: the
/// step goes through, ten stamina are gone, and the mover is told which of the
/// four lines applies.
#[test]
fn a_rested_player_shoves_past_a_body() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let from = world.state.registry.get::<Position>(entity).unwrap().0;
    // The player begins facing south, so this is a real step rather than a turn.
    let onto = Point::new(from.x, from.y + 1, from.z);
    spawn_mobile_at(&mut world, onto, 50, now);
    let _ = world.drain_outbound().count();

    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        onto,
        "a rested player walks through the body rather than into it"
    );
    let stamina = world
        .state
        .registry
        .get::<openshard_state::components::Stamina>(entity)
        .copied()
        .expect("a player carries a stamina pool");
    assert_eq!(
        stamina.max - stamina.current,
        openshard_state::runtime::SHOVE_STAMINA,
        "and arrives ten stamina poorer"
    );
    assert!(
        world
            .drain_outbound()
            .any(|out| out.packet.first() == Some(&0xC1)),
        "the mover is told they shoved somebody"
    );
}

/// The other side of the same rule, and the only branch that stops anybody: one
/// point below full is refused, exactly as it was before the shove existed.
#[test]
fn a_tired_player_is_stopped_by_a_body() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let from = world.state.registry.get::<Position>(entity).unwrap().0;
    spawn_mobile_at(&mut world, Point::new(from.x, from.y + 1, from.z), 50, now);
    tire(&mut world, entity);
    let _ = world.drain_outbound().count();

    let mut refused: Cursor<StepRefused> = world.bus().cursor();
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        from,
        "the occupied tile is not entered"
    );
    assert!(
        world
            .drain_outbound()
            .any(|out| out.packet.first() == Some(&0x21)),
        "the client receives the ordinary walk rejection"
    );
    assert!(
        world.bus().read(&mut refused).any(|event| event.entity == entity),
        "systems are told that the step was blocked"
    );
}

/// A wall is still a wall, and it is what keeps the shove from being "a refused
/// step is retried without its crowd".
///
/// The control the pair above cannot be: a rested player is exactly the case
/// that shoves, so if the ground were re-asked without its bodies this one would
/// walk into a wall for ten stamina.
#[test]
fn a_rested_player_does_not_shove_past_a_wall() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let from = world.state.registry.get::<Position>(entity).unwrap().0;
    // A closed door is the live world's own way of being in the way, and it is
    // in the overlay rather than in the crowd.
    let (_, _) = place_door(&mut world, Point::new(from.x, from.y + 1, from.z), now);
    let _ = world.drain_outbound().count();

    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        from,
        "the ground refused, and ground does not move for ten stamina"
    );
    let stamina = world
        .state
        .registry
        .get::<openshard_state::components::Stamina>(entity)
        .copied()
        .expect("a player carries a stamina pool");
    assert_eq!(
        stamina.current, stamina.max,
        "and nothing was charged for the refusal"
    );
}

/// The decreed step is held to the same rule, which is what keeps the shove from
/// being a property of the `0x02` path rather than of the engine.
#[test]
fn a_server_step_shoves_and_a_tired_one_does_not() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let from = world.state.registry.get::<Position>(entity).unwrap().0;
    let onto = Point::new(from.x, from.y + 1, from.z);
    spawn_mobile_at(&mut world, onto, 50, now);
    tire(&mut world, entity);

    let mut refused: Cursor<StepRefused> = world.bus().cursor();
    world.queue(Command::Step {
        serial,
        direction: Direction::South.to_bits(),
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        from,
        "a tired server-directed mobile also stays off the occupied tile"
    );
    assert!(
        world.bus().read(&mut refused).any(|event| event.entity == entity),
        "the blocked server step is observable"
    );

    // Rested, the same decree goes through.
    let &openshard_state::components::Stamina { max, .. } = world
        .state
        .registry
        .get::<openshard_state::components::Stamina>(entity)
        .expect("a player carries a stamina pool");
    world
        .state
        .registry
        .insert(entity, openshard_state::components::Stamina { current: max, max });
    world.queue(Command::Step {
        serial,
        direction: Direction::South.to_bits(),
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        onto,
        "and a rested one shoves through"
    );
}

/// **A facet with free movement has no crowd at all**, so nobody is in anybody's
/// way and nothing is charged for walking past them.
///
/// The first branch of `CheckShove`, asked one layer earlier than ServUO asks
/// it — in `crowd_near` — which is what makes it true of a *route* as well as of
/// a step. Facet 1 is Trammel's number, and the client decides the same question
/// the same way from the number alone.
#[test]
fn a_facet_with_free_movement_has_nobody_in_the_way() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    add_empty_facet(&mut world, Facet(1));
    assert!(
        world.state.facet_state(Facet(1)).rules().free_movement,
        "facet 1 runs Trammel rules"
    );

    let at = Point::new(START.x, START.y, 0);
    world.state.move_to(entity, Facet(1), at);
    let onto = Point::new(at.x, at.y + 1, at.z);
    let other = spawn_mobile_at(&mut world, onto, 50, now);
    let other = world.state.registry.entity_of(other).expect("a spawned mobile");
    world.state.move_to(other, Facet(1), onto);
    let _ = world.drain_outbound().count();

    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        onto,
        "the body was never in the way here"
    );
    let stamina = world
        .state
        .registry
        .get::<openshard_state::components::Stamina>(entity)
        .copied()
        .expect("a player carries a stamina pool");
    assert_eq!(
        stamina.current, stamina.max,
        "and walking past somebody cost nothing, because there was no shove"
    );
}

#[test]
fn a_server_step_does_not_cut_a_corner() {
    // A diagonal may not clip the corner where two blockers meet, and that half
    // of the step rule lives in `steps_out_of` rather than in one landing. The
    // decree used to ask `can_step`, which answers for the destination tile
    // alone — so `find_path` refused to *plan* a corner cut and the shard then
    // walked one on the next order. See `docs/world/evidence/2026-08-25-the-span-layer.md`'s N3.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let from = world.state.registry.get::<Position>(entity).unwrap().0;
    // One crate due east. The south-east diagonal clips its corner, while the
    // tile that step lands on and the other flank are both wide open.
    let crate_entity = world.state.registry.spawn();
    world.state.facet_state_mut(Facet(0)).block(
        from.x + 1,
        from.y,
        crate_entity,
        openshard_map::overlay::Cover::blocking(0, openshard_state::DOOR_HEIGHT),
    );

    let mut refused: Cursor<StepRefused> = world.bus().cursor();
    // Twice: the first may only turn to face south-east, the second steps.
    for _ in 0..2 {
        world.queue(Command::Step {
            serial,
            direction: Direction::SouthEast.to_bits(),
        });
        world.tick(now);
    }
    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        from,
        "the corner cut is refused, however open its destination is"
    );
    assert!(
        world.bus().read(&mut refused).any(|event| event.entity == entity),
        "and the refusal is observable"
    );

    // The control, and it is why the destination was never the reason: take the
    // flank away and the identical order is an ordinary step.
    world
        .state
        .facet_state_mut(Facet(0))
        .unblock(from.x + 1, from.y, crate_entity);
    world.queue(Command::Step {
        serial,
        direction: Direction::SouthEast.to_bits(),
    });
    world.tick(now);
    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        Point::new(from.x + 1, from.y + 1, from.z),
        "with the corner open the diagonal is a step like any other"
    );
}

#[test]
fn a_server_step_for_an_unknown_serial_is_a_no_op() {
    // A script can name a serial that has logged out between the event and
    // the command it queued in response. That is a miss, not a crash.
    let now = Instant::now();
    let mut world = world();
    enter(&mut world, now);
    let mut moved: Cursor<MobileMoved> = world.bus().cursor();
    world.queue(Command::Step {
        serial:    Serial::new(0x4000_0001).unwrap(),
        direction: 0,
    });
    world.tick(now);
    assert_eq!(world.bus().read(&mut moved).count(), 0);
}

#[test]
fn a_server_step_off_the_edge_is_refused_not_a_wrap() {
    // Stepping north from y=0 has no landing tile. Refuse it — the mobile
    // must not wrap to the far side of the map.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    teleport(&mut world, connection, Point::new(0, 0, 0));

    let mut refused: Cursor<StepRefused> = world.bus().cursor();
    // Twice: the first may only turn to face north, the second attempts it.
    for _ in 0..2 {
        world.queue(Command::Step {
            serial,
            direction: Direction::North.to_bits(),
        });
        world.tick(now);
    }
    assert!(
        world.bus().read(&mut refused).count() >= 1,
        "a step off the edge is refused"
    );
    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        Point::new(0, 0, 0),
        "and it did not move"
    );
}

/// The graphic of a gold coin — a real item id, used only so the tests read
/// like the thing they describe.
const GOLD: u16 = 0x0EED;

fn spawn_item_at(world: &mut World, point: Point, now: Instant) {
    world.queue(Command::SpawnItem {
        graphic:   openshard_protocol::wire::Graphic(GOLD),
        hue:       openshard_protocol::wire::Hue(0),
        amount:    1,
        stackable: false,
        position:  point,
        facet:     Facet(0),
    });
    world.tick(now);
}

/// Spawn a stackable pile of `amount` gold and return its serial.
fn spawn_gold(world: &mut World, point: Point, amount: u16, now: Instant) -> Serial {
    world.queue(Command::SpawnItem {
        graphic: openshard_protocol::wire::Graphic(GOLD),
        hue: openshard_protocol::wire::Hue(0),
        amount,
        stackable: true,
        position: point,
        facet: Facet(0),
    });
    world.tick(now);
    // The newest ground item, by serial.
    world
        .state
        .registry
        .query::<Position>()
        .filter(|(entity, _)| world.state.registry.has::<Stackable>(*entity))
        .filter_map(|(entity, _)| world.state.registry.serial_of(entity))
        .max()
        .expect("the gold was spawned")
}

#[test]
fn a_spawned_item_is_drawn_to_a_player_in_range() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection); // the login burst

    spawn_item_at(&mut world, Point::new(START.x, START.y, 0), now);

    let packets = packets_for(&mut world, connection);
    assert!(
        packets.iter().any(|p| p[0] == 0x1A),
        "the player standing on the tile is told about the item"
    );
}

#[test]
fn an_item_out_of_range_is_not_drawn() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);

    // Well past the view range.
    spawn_item_at(&mut world, Point::new(START.x + 50, START.y, 0), now);

    let packets = packets_for(&mut world, connection);
    assert!(
        !packets.iter().any(|p| p[0] == 0x1A),
        "an item across the map is not drawn"
    );
}

#[test]
fn walking_into_range_draws_an_item_and_out_of_range_forgets_it() {
    // The seen set at work, for items: an item is drawn exactly once when it
    // comes into range and removed with 0x1D when it leaves.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);

    // Put the player far away and the item back at the start, out of range.
    teleport(&mut world, connection, Point::new(START.x + 50, START.y, 0));
    spawn_item_at(&mut world, Point::new(START.x, START.y, 0), now);
    let _ = packets_for(&mut world, connection);

    // Come into range: the item is drawn.
    teleport(&mut world, connection, Point::new(START.x, START.y, 0));
    let arriving = packets_for(&mut world, connection);
    assert!(
        arriving.iter().any(|p| p[0] == 0x1A),
        "walking up to the item draws it"
    );

    // Leave again: the item is taken off the screen with 0x1D.
    teleport(&mut world, connection, Point::new(START.x + 50, START.y, 0));
    let leaving = packets_for(&mut world, connection);
    assert!(
        leaving.iter().any(|p| p[0] == 0x1D),
        "walking away forgets the item"
    );
}

#[test]
fn a_stacked_item_keeps_its_amount_when_drawn() {
    // A pile of gold is one entity with an amount, and the amount rides the
    // 0x1A that draws it — the packet sets the serial's top bit for it.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::SpawnItem {
        graphic:   openshard_protocol::wire::Graphic(GOLD),
        hue:       openshard_protocol::wire::Hue(0),
        amount:    500,
        stackable: false,
        position:  Point::new(START.x, START.y, 0),
        facet:     Facet(0),
    });
    world.tick(now);

    let packets = packets_for(&mut world, connection);
    let item = packets.iter().find(|p| p[0] == 0x1A).expect("the item was drawn");
    // The amount bit on the serial says a stack; a single item would not set it.
    assert_ne!(item[3] & 0x80, 0, "the stack sets the amount flag");
}

/// The serial of the one item in the world.
fn only_item_serial(world: &World) -> Serial {
    // The one spawned test item — never a worn backpack, which every character
    // now carries (an item with a `Drawn`, worn via `Equipped`).
    let (entity, _) = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(entity, _)| !world.state.registry.has::<Equipped>(*entity))
        .expect("a loose item is in the world");
    world.state.registry.serial_of(entity).unwrap()
}

#[test]
fn picking_up_then_dropping_moves_an_item_on_everyone_elses_screen() {
    // Two players on the same tile, an item between them. When one lifts it,
    // the other's client is told to forget it (0x1D); when it is set back
    // down, the other is told to draw it again (0x1A).
    let now = Instant::now();
    let mut world = world();
    let picker = enter(&mut world, now);
    let watcher = enter_as(&mut world, ConnectionId::from_raw(2), now);
    spawn_item_at(&mut world, Point::new(START.x, START.y, 0), now);
    let _ = packets_for(&mut world, picker);
    let _ = packets_for(&mut world, watcher);
    let serial = only_item_serial(&world);

    world.queue(Command::PickUpItem {
        connection: picker,
        serial:     RawSerial(serial.raw()),
        amount:     1,
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, watcher).iter().any(|p| p[0] == 0x1D),
        "the other player is told to forget the lifted item"
    );

    world.queue(Command::DropItem {
        connection:  picker,
        serial:      RawSerial(serial.raw()),
        destination: DropDestination::Ground(Point::new(START.x, START.y, 0)),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, watcher).iter().any(|p| p[0] == 0x1A),
        "and to draw it again where it was dropped"
    );
    let drop_sound = items::drop_sound(Graphic(GOLD), 1, SoundId(0x0042))
        .0
        .to_be_bytes();
    assert!(
        packets_for(&mut world, picker)
            .iter()
            .any(|packet| packet[0] == 0x54 && packet[2..4] == drop_sound),
        "a successful drop plays its sound for the player who made it"
    );
}

#[test]
fn picking_up_out_of_reach_is_rejected_and_leaves_the_item() {
    let now = Instant::now();
    let mut world = world();
    let picker = enter(&mut world, now);
    spawn_item_at(&mut world, Point::new(START.x + 20, START.y, 0), now);
    let _ = packets_for(&mut world, picker);
    let serial = only_item_serial(&world);
    let item = world.state.registry.entity_of(serial).unwrap();

    world.queue(Command::PickUpItem {
        connection: picker,
        serial:     RawSerial(serial.raw()),
        amount:     1,
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, picker).iter().any(|p| p == &[0x27, 0x01]),
        "the client is told the item is out of range"
    );
    assert!(
        world.state.registry.has::<Position>(item),
        "the item stays on the ground"
    );
    assert!(nothing_is_held(&world), "and nothing is on the cursor");
}

#[test]
fn a_second_lift_recovers_the_item_already_on_the_cursor() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let first = spawn_plain_item_at(&mut world, here, now);
    let second = spawn_plain_item_at(&mut world, here, now);
    let first_item = entity(&world, first);
    let second_item = entity(&world, second);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(first.raw()),
        amount:     1,
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);
    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(second.raw()),
        amount:     1,
    });
    world.tick(now);

    assert!(
        nothing_is_held(&world),
        "the confused client and authoritative cursor converge on empty"
    );
    assert!(
        world.state.registry.has::<Position>(first_item),
        "the first item is recovered at its remembered origin"
    );
    assert!(
        world.state.registry.has::<Position>(second_item),
        "the rejected second item was never detached from its origin"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
        "the client receives a terminal answer for the second lift"
    );
}

#[test]
fn dropping_out_of_reach_bounces_the_item_back_to_where_it_was() {
    let now = Instant::now();
    let mut world = world();
    let picker = enter(&mut world, now);
    let origin = Point::new(START.x, START.y, 0);
    spawn_item_at(&mut world, origin, now);
    let serial = only_item_serial(&world);
    let item = world.state.registry.entity_of(serial).unwrap();

    world.queue(Command::PickUpItem {
        connection: picker,
        serial:     RawSerial(serial.raw()),
        amount:     1,
    });
    world.tick(now);
    let _ = packets_for(&mut world, picker);

    // Drop it far from the player: refused, and put back where it started.
    world.queue(Command::DropItem {
        connection:  picker,
        serial:      RawSerial(serial.raw()),
        destination: DropDestination::Ground(Point::new(START.x + 40, START.y, 0)),
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, picker).iter().any(|p| p[0] == 0x27),
        "the drag is cancelled"
    );
    assert_eq!(
        world.state.registry.get::<Position>(item).map(|p| p.0),
        Some(origin),
        "and the item is back where it was lifted"
    );
    assert!(nothing_is_held(&world));
}

#[test]
fn logging_out_while_holding_an_item_returns_it_to_the_ground() {
    let now = Instant::now();
    let mut world = world();
    let picker = enter(&mut world, now);
    let watcher = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let origin = Point::new(START.x, START.y, 0);
    spawn_item_at(&mut world, origin, now);
    let serial = only_item_serial(&world);
    let item = world.state.registry.entity_of(serial).unwrap();

    world.queue(Command::PickUpItem {
        connection: picker,
        serial:     RawSerial(serial.raw()),
        amount:     1,
    });
    world.tick(now);
    let _ = packets_for(&mut world, watcher);

    world.queue(Command::Disconnect { connection: picker });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Position>(item).map(|p| p.0),
        Some(origin),
        "the item is back on the ground, not lost with the cursor"
    );
    assert!(
        packets_for(&mut world, watcher).iter().any(|p| p[0] == 0x1A),
        "and the player still online sees it reappear"
    );
    assert_eq!(
        openshard_state::audit_item_graph(&world.state),
        Vec::new(),
        "logout leaves no item between cursor and ground"
    );
}

#[test]
fn you_cannot_pick_up_a_mobile() {
    // A body has no `Drawn`, so lifting one is refused rather than yanking
    // a person onto the cursor.
    let now = Instant::now();
    let mut world = world();
    let picker = enter(&mut world, now);
    let other = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let mobile_serial = serial_of(&world, other);
    let _ = packets_for(&mut world, picker);

    world.queue(Command::PickUpItem {
        connection: picker,
        serial:     RawSerial(mobile_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, picker).iter().any(|p| p == &[0x27, 0x00]),
        "cannot-lift is the reason"
    );
}

/// A backpack graphic and its gump.
const BACKPACK: u16 = 0x0E75;
const BACKPACK_GUMP: Graphic = Graphic(0x003C);

fn spawn_container_at(world: &mut World, point: Point, now: Instant) -> Serial {
    world.queue(Command::SpawnContainer {
        graphic:  openshard_protocol::wire::Graphic(BACKPACK),
        gump:     BACKPACK_GUMP,
        hue:      openshard_protocol::wire::Hue(0),
        position: point,
        facet:    Facet(0),
    });
    world.tick(now);
    // The ground container just spawned — not a worn backpack, which is also a
    // container now that every character has one.
    let (entity, _) = world
        .state
        .registry
        .query::<Container>()
        .find(|(entity, _)| world.state.registry.has::<Position>(*entity))
        .expect("a container is on the ground");
    world.state.registry.serial_of(entity).unwrap()
}

/// The serial of the one item that is not a container.
fn loose_item_serial(world: &World) -> Serial {
    let (entity, _) = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(entity, _)| !world.state.registry.has::<Container>(*entity))
        .expect("a non-container item exists");
    world.state.registry.serial_of(entity).unwrap()
}

fn entity(world: &World, serial: Serial) -> EntityId {
    world.state.registry.entity_of(serial).unwrap()
}

#[test]
fn double_clicking_a_container_opens_it() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let container = spawn_container_at(&mut world, Point::new(START.x, START.y, 0), now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(container.raw())),
    });
    world.tick(now);

    let packets = packets_for(&mut world, player);
    assert!(packets.iter().any(|p| p[0] == 0x24), "the gump opens");
    assert!(packets.iter().any(|p| p[0] == 0x3C), "the contents follow");
}

#[test]
fn double_clicking_an_invalid_serial_is_silent() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let _ = packets_for(&mut world, player);
    let mut item_used: Cursor<crate::ItemUsed> = world.bus().cursor();
    let mut mobile_used: Cursor<crate::MobileUsed> = world.bus().cursor();

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(0)),
    });
    world.tick(now);

    assert_eq!(world.bus().read(&mut item_used).count(), 0);
    assert_eq!(world.bus().read(&mut mobile_used).count(), 0);
    assert!(packets_for(&mut world, player).is_empty());
}

/// A regular wooden chair — one of the four graphics players can craft and the
/// client recognizes as a sitting surface.
const WOODEN_CHAIR: Graphic = Graphic(0x0B57);

#[test]
fn walking_onto_a_chair_seats_one_player_and_the_next_step_leaves_it() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let chair_at = Point::new(START.x, START.y - 1, 0);
    world.queue(Command::SpawnItem {
        graphic:   WOODEN_CHAIR,
        hue:       Hue::NONE,
        amount:    1,
        stackable: false,
        position:  chair_at,
        facet:     Facet(0),
    });
    world.tick(now);
    let player_entity = entity(&world, serial_of(&world, player));
    let mut walker = world
        .state
        .registry
        .get::<Movement>(player_entity)
        .copied()
        .expect("a player can walk");
    walker.0.facing = Facing::walking(Direction::North);
    world.state.registry.insert(player_entity, walker);
    let _ = packets_for(&mut world, player);

    world.queue(Command::Walk {
        connection: player,
        request:    walk(0, Direction::North),
    });
    world.tick(now + WALK_INTERVAL);

    assert_eq!(
        world.state.registry.get::<Position>(player_entity),
        Some(&Position(chair_at)),
        "the player walks onto the chair's own z, which is the client's seating predicate"
    );
    assert!(world.registry().has::<openshard_state::Seated>(player_entity));
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|packet| packet[0] == 0x22),
        "walking onto a chair remains an ordinary accepted movement"
    );

    world.queue(Command::Walk {
        connection: player,
        request:    walk(0, Direction::North),
    });
    world.tick(now + WALK_INTERVAL + WALK_INTERVAL);

    assert!(!world.registry().has::<openshard_state::Seated>(player_entity));
    assert_eq!(
        world.state.registry.get::<Position>(player_entity),
        Some(&Position(Point::new(START.x, START.y - 2, 0))),
        "the first directional request walks out of the seat instead of merely turning"
    );
}

#[test]
fn an_occupied_chair_does_not_move_a_second_player() {
    let now = Instant::now();
    let mut world = world();
    let first = enter(&mut world, now);
    let second = enter(&mut world, now + WALK_INTERVAL);
    let chair_at = Point::new(START.x + 1, START.y, 0);
    teleport(&mut world, second, Point::new(START.x + 2, START.y, 0));
    world.queue(Command::SpawnItem {
        graphic:   WOODEN_CHAIR,
        hue:       Hue::NONE,
        amount:    1,
        stackable: false,
        position:  chair_at,
        facet:     Facet(0),
    });
    world.tick(now + WALK_INTERVAL);
    let first_entity = entity(&world, serial_of(&world, first));
    let mut walker = world
        .state
        .registry
        .get::<Movement>(first_entity)
        .copied()
        .expect("a player can walk");
    walker.0.facing = Facing::walking(Direction::East);
    world.state.registry.insert(first_entity, walker);

    let second_entity = entity(&world, serial_of(&world, second));
    let mut walker = world
        .state
        .registry
        .get::<Movement>(second_entity)
        .copied()
        .expect("a player can walk");
    walker.0.facing = Facing::walking(Direction::West);
    world.state.registry.insert(second_entity, walker);
    world.queue(Command::Walk {
        connection: first,
        request:    walk(0, Direction::East),
    });
    world.tick(now + WALK_INTERVAL + WALK_INTERVAL);
    assert!(
        world.registry().has::<openshard_state::Seated>(first_entity),
        "the first player reached the chair before the second tried to enter it"
    );

    world.queue(Command::Walk {
        connection: second,
        request:    walk(0, Direction::West),
    });
    world.tick(now + WALK_INTERVAL + WALK_INTERVAL + WALK_INTERVAL);

    // The seat, not the tile. A body in a chair is a body, so a rested second
    // player shoves past it and stands on the same tile for ten stamina — which
    // is what the shove rule says everywhere else and there is no reason a chair
    // would be the exception. What a chair *does* have is one occupant, and that
    // is the assertion: reaching the tile is not taking the seat.
    assert_eq!(
        world.state.registry.get::<Position>(second_entity),
        Some(&Position(chair_at)),
        "the second player shoved onto the chair's tile"
    );
    assert!(
        world.registry().has::<openshard_state::Seated>(first_entity),
        "and the occupant is still sitting in it"
    );
    assert!(
        !world.registry().has::<openshard_state::Seated>(second_entity),
        "a second character cannot overwrite the seat occupant"
    );
}

/// A bottle graphic — the engine has no built-in double-click behaviour for it,
/// so it is the plain-item case the trigger seam exists for.
const POTION_GRAPHIC: Graphic = Graphic(0x0F0E);

/// Spawn a plain item at `point` and return its serial.
fn spawn_plain_item_at(world: &mut World, point: Point, now: Instant) -> Serial {
    world.queue(Command::SpawnItem {
        graphic:   POTION_GRAPHIC,
        hue:       openshard_protocol::wire::Hue(0),
        amount:    1,
        stackable: false,
        position:  point,
        facet:     Facet(0),
    });
    world.tick(now);
    world
        .state
        .registry
        .query::<Drawn>()
        .filter(|(_, g)| g.id == POTION_GRAPHIC)
        .filter_map(|(e, _)| world.state.registry.serial_of(e))
        .max()
        .expect("the item spawned")
}

#[test]
fn a_registered_spawn_command_creates_semantic_identity() {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };
    use openshard_state::components::{
        ItemKind,
        Material,
    };

    let now = Instant::now();
    let mut world = world();
    world.queue(Command::SpawnItem {
        graphic:   Graphic(0x1415),
        hue:       Hue(0x08AB),
        amount:    1,
        stackable: false,
        position:  Point::new(START.x, START.y, 0),
        facet:     Facet(0),
    });
    world.tick(now);
    let item = world
        .registry()
        .query::<Drawn>()
        .find_map(|(item, drawn)| {
            (*drawn
                == Drawn {
                    id:  Graphic(0x1415),
                    hue: Hue(0x08AB),
                })
            .then_some(item)
        })
        .expect("spawned plate chest");
    assert_eq!(
        world.registry().get::<ItemKind>(item),
        Some(&ItemKind(ItemKindId(5)))
    );
    assert_eq!(
        world.registry().get::<Material>(item),
        Some(&Material(MaterialId(9)))
    );
}

#[test]
fn a_dagger_carves_an_animal_corpse_once() {
    // Carving is a two-packet action: use the blade, then point at the corpse.
    // Keep both halves in one test, since an item in a corpse is the result that
    // proves the target cursor did not merely appear.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();
    let dagger =
        openshard_items::spawn_item(&mut world.state, Graphic(0x0F52), Hue(0), 1, false, at, Facet(0))
            .expect("a dagger");
    let dagger_serial = world.registry().serial_of(dagger).unwrap();
    let corpse = spawn_plain_item_at(&mut world, Point::new(at.x + 1, at.y, at.z), now);
    let corpse_entity = world.registry().entity_of(corpse).unwrap();
    world.state.registry.insert(
        corpse_entity,
        Drawn {
            id:  openshard_state::components::CORPSE_GRAPHIC,
            hue: Hue(0),
        },
    );
    world.state.registry.insert(
        corpse_entity,
        Container {
            gump: openshard_state::components::CORPSE_GUMP,
        },
    );
    world.state.registry.insert(
        corpse_entity,
        CorpseBody {
            body:   Graphic(0x00D8), // cow
            facing: openshard_protocol::direction::Direction::North,
        },
    );
    world
        .state
        .registry
        .insert(corpse_entity, Corpse::from_death("a cow".to_owned(), None));

    let carve = |world: &mut World| {
        world.queue(Command::DoubleClick {
            connection,
            request: UseRequest::Use(RawSerial(dagger_serial.raw())),
        });
        world.tick(now);
        world.queue(Command::TargetResponse {
            connection,
            response: openshard_protocol::target::TargetResponse {
                cursor_id: openshard_protocol::wire::CursorId(0),
                object:    Some(corpse),
                location:  Point::new(0, 0, 0),
                graphic:   None,
                cancelled: false,
            },
        });
        world.tick(now);
    };
    carve(&mut world);

    let contained: Vec<(Graphic, u16)> = world
        .registry()
        .query::<Contained>()
        .filter(|(_, contained)| contained.container == corpse)
        .filter_map(|(item, _)| {
            world.registry().get::<Drawn>(item).map(|drawn| {
                (
                    drawn.id,
                    world.registry().get::<Amount>(item).map_or(1, |amount| amount.0),
                )
            })
        })
        .collect();
    assert!(
        contained.contains(&(Graphic(0x09F1), 10)),
        "the cow yielded raw ribs: {contained:?}"
    );
    assert!(
        contained.contains(&(Graphic(0x1078), 10)),
        "the cow yielded hides: {contained:?}"
    );
    assert!(world.registry().get::<Corpse>(corpse_entity).unwrap().carved);

    carve(&mut world);
    let count = world
        .registry()
        .query::<Contained>()
        .filter(|(_, contained)| contained.container == corpse)
        .count();
    assert_eq!(count, 2, "a carcass yields its resources only once");
}

#[test]
fn scissors_cut_a_pile_of_hides_into_leather_of_the_same_grade() {
    // The other end of carving, and the step without which Tailoring cannot be
    // reached from butchering: fifty-six tailoring rows eat leather and nothing
    // else in the engine made any. Two packets, like carving — use the scissors,
    // then point at the pile — so both halves belong in one test.
    //
    // Cut *spined* hides on purpose: a cut that dropped the grade would still
    // pass with regular ones, and dropping it is the failure that matters (the
    // whole reason a hide carries a material is that the grade is worth money).
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };
    use openshard_state::components::{
        ItemKind,
        Material,
    };

    const SPINED: MaterialId = MaterialId(41);

    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();
    let owner = world.registry().serial_of(player).unwrap();
    let pack = openshard_items::backpack_of(&world.state, owner).unwrap();

    let scissors =
        openshard_items::spawn_item(&mut world.state, Graphic(0x0F9F), Hue(0), 1, false, at, Facet(0))
            .expect("a pair of scissors");
    let scissors_serial = world.registry().serial_of(scissors).unwrap();
    let hides = openshard_items::give_kind(
        &mut world.state,
        pack,
        openshard_items::HIDES_KIND,
        Some(SPINED),
        5,
    )
    .expect("spined hides")
    .last
    .expect("the pile was created");
    let hides_serial = world.registry().serial_of(hides).unwrap();

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(scissors_serial.raw())),
    });
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(hides_serial),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    let in_pack: Vec<(ItemKindId, Option<MaterialId>, u16)> = world
        .registry()
        .query::<Contained>()
        .filter(|(_, contained)| contained.container == pack)
        .filter_map(|(item, _)| {
            world.registry().get::<ItemKind>(item).map(|kind| {
                (
                    kind.0,
                    world.registry().get::<Material>(item).map(|material| material.0),
                    world.registry().get::<Amount>(item).map_or(1, |amount| amount.0),
                )
            })
        })
        .collect();
    assert!(
        in_pack.contains(&(openshard_items::LEATHER_KIND, Some(SPINED), 5)),
        "five spined hides cut into five spined leather: {in_pack:?}"
    );
    assert!(
        !in_pack
            .iter()
            .any(|(kind, _, _)| *kind == openshard_items::HIDES_KIND),
        "the hides were spent: {in_pack:?}"
    );
}

#[test]
fn scissors_refuse_a_pile_of_hides_still_lying_in_the_corpse() {
    // ServUO's `IsChildOf(from.Backpack)`, and the branch a player hits first:
    // carving leaves the hides in the corpse, so a cut that only asked "is it in
    // reach" would let a pile be cut out of any container nearby — one somebody
    // else is looting included.
    //
    // The pile is inside a *container* on purpose. On the bare ground the cut
    // refuses anyway, for a duller reason (there is nowhere to put the leather),
    // and a test written that way passes with the pack check deleted.
    use openshard_protocol::item_kind::MaterialId;
    use openshard_state::components::ItemKind;

    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();

    let scissors =
        openshard_items::spawn_item(&mut world.state, Graphic(0x0F9F), Hue(0), 1, false, at, Facet(0))
            .expect("a pair of scissors");
    let scissors_serial = world.registry().serial_of(scissors).unwrap();
    let corpse = spawn_plain_item_at(&mut world, Point::new(at.x + 1, at.y, at.z), now);
    let corpse_entity = world.registry().entity_of(corpse).unwrap();
    world.state.registry.insert(
        corpse_entity,
        Container {
            gump: openshard_state::components::CORPSE_GUMP,
        },
    );
    let hides = openshard_items::give_kind(
        &mut world.state,
        corpse,
        openshard_items::HIDES_KIND,
        Some(MaterialId(40)),
        5,
    )
    .expect("hides")
    .last
    .expect("the pile was created");
    let hides_serial = world.registry().serial_of(hides).unwrap();
    // Pinned so the refusal cannot quietly become "that is not hides at all":
    // the pile must be recognisable, or this test passes through the wrong door.
    assert_eq!(
        world.registry().get::<ItemKind>(hides).map(|kind| kind.0),
        Some(openshard_items::HIDES_KIND)
    );

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(scissors_serial.raw())),
    });
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(hides_serial),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert_eq!(
        world.registry().entity_of(hides_serial),
        Some(hides),
        "the pile in the corpse was not consumed"
    );
    assert_eq!(
        world.registry().get::<Amount>(hides).map(|amount| amount.0),
        Some(5)
    );
    assert!(
        !world
            .registry()
            .query::<ItemKind>()
            .any(|(_, kind)| kind.0 == openshard_items::LEATHER_KIND),
        "no leather was made"
    );
}

/// A cotton plant, standing one tile east of the player.
fn a_cotton_plant(world: &mut World, at: Point) -> EntityId {
    openshard_items::plant(
        &mut world.state,
        openshard_state::components::CropKind::Cotton,
        at,
        Facet(0),
    )
    .expect("a cotton plant")
}

/// The cotton lying loose on the ground, tile by tile.
fn loose_cotton(world: &World) -> Vec<(Point, u16)> {
    world
        .registry()
        .query::<Drawn>()
        .filter(|(_, drawn)| drawn.id == Graphic(0x0DF9))
        .filter_map(|(item, _)| {
            world.registry().get::<Position>(item).map(|at| {
                (
                    at.0,
                    world.registry().get::<Amount>(item).map_or(1, |amount| amount.0),
                )
            })
        })
        .collect()
}

#[test]
fn a_cotton_plant_pays_cotton_once_and_stands_picked() {
    // The head of the cloth chain: the wheel spins cotton, and until the field
    // nothing on the shard grew any. Two things are being pinned at once, and
    // the second is the one that matters — a plant that stayed `Standing` after
    // a pick is an unlimited cotton fountain, one double-click per tick, which
    // no amount of spinning downstream would ever notice.
    use openshard_state::components::Crop;

    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();
    let tile = Point::new(at.x + 1, at.y, at.z);

    let plant = a_cotton_plant(&mut world, tile);
    let plant_serial = world.registry().serial_of(plant).unwrap();
    assert!(
        matches!(
            world.registry().get::<Crop>(plant),
            Some(Crop::Standing(openshard_state::components::CropKind::Cotton))
        ),
        "a fresh plant is standing"
    );

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(plant_serial.raw())),
    });
    world.tick(now);

    assert_eq!(
        loose_cotton(&world),
        vec![(tile, 1)],
        "one pile of cotton, on the plant's own tile"
    );
    assert!(
        matches!(world.registry().get::<Crop>(plant), Some(Crop::Picked { .. })),
        "the plant is a picked stub"
    );
    assert_eq!(
        world.registry().get::<Drawn>(plant).map(|drawn| drawn.id),
        Some(openshard_state::components::CropKind::Cotton.picked_art()),
        "and is drawn as the bare furrow"
    );

    // The second click, which is the whole point.
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(plant_serial.raw())),
    });
    world.tick(now);
    assert_eq!(
        loose_cotton(&world),
        vec![(tile, 1)],
        "a picked plant pays nothing a second time"
    );
}

#[test]
fn a_cotton_plant_cannot_be_lifted_out_of_the_field() {
    // ServUO builds every `FarmableCrop` with `Movable = false`, and this is why:
    // a plant that lifted into a pack would be a field harvested by dragging,
    // which pays the bush rather than the cotton and empties the field for
    // everyone else at no cost.
    use openshard_state::components::Crop;

    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();
    let tile = Point::new(at.x + 1, at.y, at.z);

    let plant = a_cotton_plant(&mut world, tile);
    let plant_serial = world.registry().serial_of(plant).unwrap();
    world.queue(Command::PickUpItem {
        connection,
        serial: RawSerial(plant_serial.raw()),
        amount: 1,
    });
    world.tick(now);

    assert_eq!(
        world.registry().get::<Position>(plant).map(|position| position.0),
        Some(tile),
        "the plant is still standing where it grew"
    );
    assert!(
        matches!(world.registry().get::<Crop>(plant), Some(Crop::Standing(_))),
        "and is still pickable"
    );
}

#[test]
fn a_crop_field_plants_itself_full_and_regrows_what_was_picked() {
    // The field is the reason the plant is worth having: a patch that emptied
    // once and stayed empty would put cotton back where it started, on a
    // vendor's shelf. Registering lays it full (ServUO's own `Respawn` on a
    // region loading) and the tick puts back what a player takes.
    use openshard_state::components::Crop;

    let standing = |world: &World| -> usize {
        world
            .registry()
            .query::<Crop>()
            .filter(|(_, crop)| matches!(crop, Crop::Standing(_)))
            .count()
    };

    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();

    world.queue(Command::RegisterCropField {
        field: crate::crops::CropField::new(
            "A Test Field".to_owned(),
            openshard_state::components::CropKind::Cotton,
            crate::spawner::SpawnArea {
                x:      at.x,
                y:      at.y,
                width:  8,
                height: 8,
                facet:  Facet(0),
            },
            3,
            // Four ticks rather than the data's twenty seconds: the pace is the
            // data's and the mechanism is what is under test. Long enough that
            // the pick and the regrowth are separate ticks, which is what lets
            // the count in between be asserted at all.
            4,
        ),
    });
    world.tick(now);
    assert_eq!(standing(&world), 3, "a fresh field is laid full");

    // Pick one — the ordinary way, so the count falls the way it does in play.
    let picked = world
        .registry()
        .query::<Crop>()
        .find(|(_, crop)| matches!(crop, Crop::Standing(_)))
        .map(|(entity, _)| world.registry().serial_of(entity).unwrap())
        .expect("a plant to pick");
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(picked.raw())),
    });
    world.tick(now);
    assert_eq!(standing(&world), 2, "the picked plant stopped counting at once");

    // And it comes back once the field's own delay has run out.
    for _ in 0..5 {
        world.tick(now);
    }
    assert_eq!(standing(&world), 3, "the field regrew what was picked");
}

#[test]
fn a_field_of_cotton_is_not_saved_but_the_cotton_it_paid_is() {
    // The split the plant's own doc names. A restored plant would be a second
    // copy of what the boot's `populate:` is about to sow, and a restored stub
    // would be a bare furrow with no timer left to clear it — the eternal static
    // a restored field tile used to be. What a pick *paid* is an ordinary item
    // and has to survive, or a player who logged out beside their cotton would
    // come back to nothing.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();

    let standing = a_cotton_plant(&mut world, Point::new(at.x + 1, at.y, at.z));
    let picked = a_cotton_plant(&mut world, Point::new(at.x + 2, at.y, at.z));
    let picked_serial = world.registry().serial_of(picked).unwrap();
    let standing_serial = world.registry().serial_of(standing).unwrap();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(picked_serial.raw())),
    });
    world.tick(now);

    let saved = world.ground_items();
    assert!(
        !saved
            .iter()
            .any(|record| record.serial == standing_serial || record.serial == picked_serial),
        "neither a standing plant nor a picked stub is saved: {saved:?}"
    );
    assert!(
        saved.iter().any(|record| record.graphic == 0x0DF9),
        "the cotton it paid is: {saved:?}"
    );
}

/// A sheep in fleece, standing where it is told.
fn a_sheep(world: &mut World, at: Point, now: Instant) -> Serial {
    world.queue(Command::SpawnMobile {
        body:        openshard_state::components::WOOLLY_SHEEP,
        hue:         openshard_protocol::wire::Hue(0),
        hits:        12,
        notoriety:   Notoriety::from_bits(3),
        damage:      2,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    at,
        facet:       Facet(0),
        name:        None,
        title:       None,
        shoe:        0,
        fame:        300,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    world
        .state
        .registry
        .query::<Body>()
        .filter(|(entity, _)| !world.state.registry.has::<Client>(*entity))
        .filter_map(|(entity, _)| world.state.registry.serial_of(entity))
        .max()
        .expect("a sheep")
}

/// How much wool is in a container, counting every pile in it.
fn wool_in(world: &World, container: Serial) -> u16 {
    world
        .registry()
        .query::<Contained>()
        .filter(|(_, contained)| contained.container == container)
        .filter(|(item, _)| {
            world
                .registry()
                .get::<Drawn>(*item)
                .is_some_and(|drawn| drawn.id == Graphic(0x0DF8))
        })
        .map(|(item, _)| world.registry().get::<Amount>(item).map_or(1, |amount| amount.0))
        .sum()
}

#[test]
fn a_blade_shears_a_sheep_once_and_leaves_it_shorn() {
    // Wool's one source on the shard, and the same two packets carving uses —
    // ServUO reaches a sheep through `BladedItemTarget` and `ICarvable`, not
    // through the scissors, so this rides the carve seam rather than the cut.
    //
    // The second shearing is the half that matters: the fleece is the timer's
    // public face, and a sheep that stayed woolly would be two wool per tick
    // for as long as somebody kept clicking.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();
    let owner = world.registry().serial_of(player).unwrap();
    let pack = openshard_items::backpack_of(&world.state, owner).unwrap();

    let dagger =
        openshard_items::spawn_item(&mut world.state, Graphic(0x0F52), Hue(0), 1, false, at, Facet(0))
            .expect("a dagger");
    let dagger_serial = world.registry().serial_of(dagger).unwrap();
    let sheep = a_sheep(&mut world, Point::new(at.x + 1, at.y, at.z), now);
    let sheep_entity = world.registry().entity_of(sheep).unwrap();

    let shear = |world: &mut World| {
        world.queue(Command::DoubleClick {
            connection,
            request: UseRequest::Use(RawSerial(dagger_serial.raw())),
        });
        world.tick(now);
        world.queue(Command::TargetResponse {
            connection,
            response: openshard_protocol::target::TargetResponse {
                cursor_id: openshard_protocol::wire::CursorId(0),
                object:    Some(sheep),
                location:  Point::new(0, 0, 0),
                graphic:   None,
                cancelled: false,
            },
        });
        world.tick(now);
    };

    shear(&mut world);
    assert_eq!(
        wool_in(&world, pack),
        2,
        "Felucca pays two wool, ServUO's own rate"
    );
    assert_eq!(
        world.registry().get::<Body>(sheep_entity).map(|body| body.id),
        Some(openshard_state::components::SHORN_SHEEP),
        "and the sheep is drawn shorn"
    );

    shear(&mut world);
    assert_eq!(wool_in(&world, pack), 2, "a shorn sheep has nothing left to give");
}

#[test]
fn a_blade_on_something_alive_that_is_not_a_sheep_takes_nothing() {
    // ServUO's `BladedItemTarget` answers every other living thing with "You can
    // only skin dead creatures", and the branch is worth pinning because the one
    // above it now *does* something to a mobile: a shear that read the body
    // loosely would take wool off a llama, or worse, off a player.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();
    let owner = world.registry().serial_of(player).unwrap();
    let pack = openshard_items::backpack_of(&world.state, owner).unwrap();

    let dagger =
        openshard_items::spawn_item(&mut world.state, Graphic(0x0F52), Hue(0), 1, false, at, Facet(0))
            .expect("a dagger");
    let dagger_serial = world.registry().serial_of(dagger).unwrap();
    let goat = spawn_mobile_at(&mut world, Point::new(at.x + 1, at.y, at.z), 12, now);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(dagger_serial.raw())),
    });
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(goat),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert_eq!(wool_in(&world, pack), 0, "nothing was shorn");
    let goat_entity = world.registry().entity_of(goat).unwrap();
    assert!(
        world.registry().get::<Body>(goat_entity).is_some(),
        "and it is still standing there, unchanged"
    );
}

#[test]
fn a_shorn_sheep_comes_back_in_fleece_after_a_restart() {
    // The wheel's bargain, one shelf over: the body is saved and the timer that
    // would regrow the wool is not, so a sheep restored exactly as it was would
    // be shorn for ever with nothing left to change it — the wheel that turns
    // for ever. Losing the timer instead costs a player nothing.
    let now = Instant::now();
    let mut pen = world();
    let connection = enter(&mut pen, now);
    let player = pen.state.players[&connection];
    let Position(at) = *pen.registry().get::<Position>(player).unwrap();

    let dagger = openshard_items::spawn_item(&mut pen.state, Graphic(0x0F52), Hue(0), 1, false, at, Facet(0))
        .expect("a dagger");
    let dagger_serial = pen.registry().serial_of(dagger).unwrap();
    let sheep = a_sheep(&mut pen, Point::new(at.x + 1, at.y, at.z), now);

    pen.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(dagger_serial.raw())),
    });
    pen.tick(now);
    pen.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(sheep),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    pen.tick(now);
    let records = pen.mobile_records();
    assert!(
        records
            .iter()
            .any(|record| Graphic(record.body) == openshard_state::components::SHORN_SHEEP),
        "the save recorded the shorn body, which is what makes the stamp necessary"
    );

    let mut shard = world();
    let characters = shard.restore_characters(Vec::new());
    let filed = shard.restore_items(Vec::new(), &characters);
    shard.restore_mobiles(records, &filed);

    let restored = shard
        .registry()
        .entity_of(sheep)
        .expect("the sheep came back on its serial");
    assert_eq!(
        shard.registry().get::<Body>(restored).map(|body| body.id),
        Some(openshard_state::components::WOOLLY_SHEEP),
        "and came back in fleece rather than shorn for ever"
    );
}

#[test]
fn double_clicking_a_plain_item_fires_the_use_trigger() {
    // The item-trigger seam (Sphere's @DClick): an item with no engine behaviour
    // — not a door, container, spellbook, mount or mobile — hands its double-click
    // to the pack as an ItemUsed carrying the graphic, so a shard can make a
    // bottle drinkable without the engine knowing what a bottle is.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let item = spawn_plain_item_at(&mut world, Point::new(START.x, START.y, 0), now);
    let player_serial = world
        .registry()
        .serial_of(world.state.players[&player])
        .unwrap()
        .raw();

    let mut used: Cursor<crate::ItemUsed> = world.bus().cursor();
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(item.raw())),
    });
    world.tick(now);

    let events: Vec<crate::ItemUsed> = world.bus().read(&mut used).copied().collect();
    assert_eq!(events.len(), 1, "one use trigger for one double-click");
    assert_eq!(events[0].graphic, POTION_GRAPHIC, "keyed by the tile");
    assert_eq!(events[0].item, item, "on the item clicked");
    assert_eq!(events[0].by.raw(), player_serial, "by the clicker");
    assert_eq!(
        events[0].item_kind,
        Some(openshard_protocol::item_kind::ItemKindId(39)),
        "the common empty bottle is now an explicit semantic item"
    );
    assert_eq!(events[0].material, None);
}

#[test]
fn the_use_trigger_carries_a_typed_items_identity() {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };

    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let item = items::spawn_item_kind(
        &mut world.state,
        ItemKindId(5), // plate chest: a plain use-trigger item on the ground
        Some(MaterialId(9)),
        1,
        false,
        Point::new(START.x, START.y, 0),
        Facet(0),
    )
    .expect("registered item");
    let serial = world.registry().serial_of(item).expect("item serial");

    let mut used: Cursor<crate::ItemUsed> = world.bus().cursor();
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(serial.raw())),
    });
    world.tick(now);

    let event = world.bus().read(&mut used).copied().next().expect("item use");
    assert_eq!(event.item_kind, Some(ItemKindId(5)));
    assert_eq!(event.material, Some(MaterialId(9)));
}

#[test]
fn a_ghost_cannot_use_a_plain_item() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_serial = serial_of(&world, player);
    let item = spawn_plain_item_at(&mut world, Point::new(START.x, START.y, 0), now);

    world.queue(Command::Damage {
        serial:      player_serial,
        amount:      500,
        damage_type: 0,
        by:          None,
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);
    let mut used: Cursor<crate::ItemUsed> = world.bus().cursor();

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(item.raw())),
    });
    world.tick(now);

    assert!(
        world.bus().read(&mut used).next().is_none(),
        "the generic use trigger is behind the dead-use gate too"
    );
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| { (p[0] == 0x1C || p[0] == 0xAE) && String::from_utf8_lossy(p).contains("I am dead") }),
        "and the central gate says why"
    );
}

#[test]
fn the_use_trigger_respects_reach() {
    // Reach is server-authoritative: a double-click on an item across the map
    // fires nothing, the same guard a lift uses. Otherwise a pack could be made
    // to act on an item the player cannot touch.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    // Well beyond ITEM_REACH from START.
    let item = spawn_plain_item_at(&mut world, Point::new(START.x + 50, START.y, 0), now);

    let mut used: Cursor<crate::ItemUsed> = world.bus().cursor();
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(item.raw())),
    });
    world.tick(now);

    assert_eq!(
        world.bus().read(&mut used).count(),
        0,
        "an out-of-reach item fires no use trigger"
    );
}

/// The serial of the backpack a connection's character is wearing.
pub(super) fn backpack_serial(world: &World, connection: ConnectionId) -> Serial {
    let owner = world
        .registry()
        .serial_of(world.state.players[&connection])
        .unwrap();
    world
        .registry()
        .query::<Equipped>()
        .find(|(_, worn)| worn.mobile == owner && worn.layer == items::BACKPACK_LAYER)
        .and_then(|(item, _)| world.registry().serial_of(item))
        .expect("a character wears a backpack")
}

#[test]
fn entering_the_world_equips_a_backpack_and_tells_the_client() {
    // A fresh character has a bag: worn on the backpack layer, a real
    // container, and named to the client in a 0x78 about itself so the client
    // knows the serial to double-click open.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);

    let pack = backpack_serial(&world, player);
    let pack_entity = entity(&world, pack);
    assert!(
        world.registry().has::<Container>(pack_entity),
        "the bag is a container"
    );
    assert!(
        !world.registry().has::<Position>(pack_entity),
        "a worn bag is off the ground"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x78),
        "the client is told its own equipment"
    );
}

#[test]
fn double_clicking_your_own_backpack_opens_it() {
    // The bag is worn, not on the ground, so the old ground-only open would
    // have refused it. Your own pack is always in reach.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let pack = backpack_serial(&world, player);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(pack.raw())),
    });
    world.tick(now);

    let packets = packets_for(&mut world, player);
    assert!(packets.iter().any(|p| p[0] == 0x24), "the bag gump opens");
    assert!(packets.iter().any(|p| p[0] == 0x3C), "its contents follow");
}

#[test]
fn dropping_an_item_into_your_worn_backpack_stores_it() {
    // The bug the user hit: a worn bag has no `Position`, so the drop-into
    // reach check bounced the item and the client's cursor desynced. The
    // wearer's tile has to stand in for the bag's.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let pack = backpack_serial(&world, player);
    let here = world
        .registry()
        .get::<Position>(world.state.players[&player])
        .unwrap()
        .0;
    let item_serial = spawn_plain_item_at(&mut world, here, now);
    let item = entity(&world, item_serial);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(item_serial.raw()),
        destination: DropDestination::Item {
            item: pack,
            at:   GumpPoint::new(0, 0),
        },
    });
    world.tick(now);

    assert!(
        world.state.registry.has::<Contained>(item),
        "the item is now inside the worn bag"
    );
    assert_eq!(world.registry().get::<Contained>(item).unwrap().container, pack);
    assert!(
        world.state.held_of(player).is_none(),
        "and off the cursor, not bounced"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|packet| {
            packet.len() >= 4 && packet[0] == 0x54 && packet[2..4] == 0x0048_u16.to_be_bytes()
        }),
        "a successful backpack drop plays the container sound (0x0048)"
    );
    assert_eq!(
        openshard_state::audit_item_graph(&world.state),
        Vec::new(),
        "the backpack view and canonical graph agree after drop"
    );
}

#[test]
fn double_clicking_yourself_opens_the_paperdoll() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = world
        .registry()
        .serial_of(world.state.players[&player])
        .unwrap()
        .raw();
    let _ = packets_for(&mut world, player);

    // Bit 31 is the client's paperdoll *request* (the login-time open, the
    // paperdoll macro) — ServUO's `UseReq` routes it straight to the paperdoll
    // and nothing else. `DoubleClick::interpret` is what reads the bit, so what
    // reaches the queue is already the one request or the other. A raw
    // self-double-click (no bit) opens the paperdoll too, through the ordinary
    // use rule, when the player is on foot.
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Paperdoll(RawSerial(serial)),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x88),
        "the paperdoll request opens the paperdoll"
    );

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(serial)),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x88),
        "and so does a raw self-double-click on foot"
    );
}

#[test]
fn a_connection_can_be_answered_before_it_has_a_character() {
    // The thing that was in the way of answering the character screen out of a
    // tick. `send_packet` frames a packet for the connection's client version,
    // and that version used to live on the *player's* `Client` component — so a
    // connection that had not picked a character yet resolved to no version, and
    // every packet addressed to it was dropped without a word. There is no
    // character here on purpose: the hand-off is all that has happened.
    let mut world = world();
    let connection = connection();

    authenticate(&mut world, connection, Instant::now());
    // The hand-off's own answer — the character list — is what this used to be
    // unable to send at all; drained here so what follows is only this test's.
    let _ = packets_for(&mut world, connection);

    assert_eq!(
        world.state.version_of(connection),
        Some(ClientVersion::TOL),
        "the world knows what this client is with no entity to hang it on"
    );
    world
        .state
        .send_packet(connection, &ServerPacket::LogoutAck(LogoutAck));
    assert_eq!(
        packets_for(&mut world, connection).len(),
        1,
        "and a packet addressed to it actually leaves"
    );
}

#[test]
fn a_disconnect_forgets_the_connection_itself() {
    // The row is the connection's, not the character's, so it has to go when the
    // socket does. A `ConnectionId` is reused — the gateway hands out fresh ones
    // per accept, but nothing here may assume it — and a row left behind would
    // give the next client on that id a version it never negotiated, which is
    // exactly the silent wrong-dialect encode the version gate exists to prevent.
    let now = Instant::now();
    let mut world = world();
    // Entered without a hand-off, the way every test does it: `enter` attaches
    // the session itself, so the version is on the connection either way.
    let connection = enter(&mut world, now);
    assert_eq!(world.state.version_of(connection), Some(ClientVersion::TOL));

    world.queue(Command::Disconnect { connection });
    world.tick(now);

    assert_eq!(
        world.state.version_of(connection),
        None,
        "the world holds nothing for a socket that is gone"
    );
}

#[test]
fn logging_out_despawns_the_backpack() {
    // Equipment is not persisted yet, so it must not outlive its wearer as an
    // orphan equipped on a serial about to be reused.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let pack = backpack_serial(&world, player);
    let pack_entity = entity(&world, pack);

    world.queue(Command::Disconnect { connection: player });
    world.tick(now);

    assert!(
        !world.registry().contains(pack_entity),
        "the bag went with the character"
    );
}

#[test]
fn dropping_an_item_into_a_container_puts_it_inside() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);
    let _ = packets_for(&mut world, player);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(item_serial.raw()),
        // Gump coordinates, not tiles — and now the type says so rather than a
        // comment: `DropDestination` chose the space when it read the target.
        destination: DropDestination::Item {
            item: container,
            at:   GumpPoint::new(50, 60),
        },
    });
    world.tick(now);

    let contained = world
        .state
        .registry
        .get::<Contained>(item)
        .expect("the item is now in a container");
    assert_eq!(contained.container, container);
    assert_eq!((contained.position.x, contained.position.y), (50, 60));
    assert!(
        !world.state.registry.has::<Position>(item),
        "and no longer on the ground"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x25),
        "the client is told the item went in"
    );
}

/// A full container refuses the drop, says which ceiling it hit, and the item
/// comes back to the hand that offered it.
///
/// ServUO's `Container.CheckHold`, which this engine did not have: a chest took
/// anything. The bounce is what makes the refusal readable — the player watches
/// the item return and reads the line beside it, rather than watching it vanish.
#[test]
fn a_full_container_refuses_the_drop_and_bounces_it_back() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);

    // Filled to the brim. Not through the drop path, which is the thing under
    // test: `place_one` is the decree door and has no ceiling of its own.
    for _ in 0..openshard_items::MAX_ITEMS {
        openshard_items::place_one(&mut world.state, container, Graphic(0x1BE3), Hue(0), 1)
            .expect("the serial pool is not dry");
    }
    let _ = packets_for(&mut world, player);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(item_serial.raw()),
        destination: DropDestination::Item {
            item: container,
            at:   GumpPoint::new(50, 60),
        },
    });
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<Contained>(item)
            .map(|held| held.container),
        None,
        "a full container took it anyway"
    );
    let said = packets_for(&mut world, player);
    assert!(
        said.iter()
            .any(|packet| String::from_utf8_lossy(packet).contains("That container cannot hold more items")),
        "the player was refused and not told why"
    );
    assert!(
        said.iter().any(|packet| packet[0] == 0x27),
        "the item did not bounce back to the hand that offered it"
    );
}

/// Staff are never refused — ServUO's `IsStaff` guard, and what lets a game
/// master fill a chest to see what a full one does.
#[test]
fn a_full_container_still_takes_what_a_game_master_drops_in() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity_of_player = world.state.players[&player];
    world
        .state
        .registry
        .insert(entity_of_player, openshard_state::components::Staff);
    let here = Point::new(START.x, START.y, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);
    for _ in 0..openshard_items::MAX_ITEMS {
        openshard_items::place_one(&mut world.state, container, Graphic(0x1BE3), Hue(0), 1)
            .expect("the serial pool is not dry");
    }

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(item_serial.raw()),
        destination: DropDestination::Item {
            item: container,
            at:   GumpPoint::new(50, 60),
        },
    });
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<Contained>(item)
            .map(|held| held.container),
        Some(container),
        "a game master was refused"
    );
}

/// A bag counts against the pack it is put into, contents and all.
///
/// ServUO's `TotalItems` is the whole subtree and `CheckHold` walks upward, so
/// filling a pack with bags of bags is not a way around the ceiling. A one-level
/// scan would let it through, which is the shape this asserts against.
#[test]
fn a_bag_counts_against_the_container_it_goes_into() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let outer = spawn_container_at(&mut world, here, now);
    let inner = spawn_container_at(&mut world, here, now);
    let inner_entity = entity(&world, inner);

    // The outer one is nearly full; the bag holds more than the room left.
    for _ in 0..openshard_items::MAX_ITEMS - 5 {
        openshard_items::place_one(&mut world.state, outer, Graphic(0x1BE3), Hue(0), 1)
            .expect("the serial pool is not dry");
    }
    for _ in 0..10 {
        openshard_items::place_one(&mut world.state, inner, Graphic(0x1BE3), Hue(0), 1)
            .expect("the serial pool is not dry");
    }
    let _ = packets_for(&mut world, player);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(inner.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(inner.raw()),
        destination: DropDestination::Item {
            item: outer,
            at:   GumpPoint::new(50, 60),
        },
    });
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<Contained>(inner_entity)
            .map(|held| held.container),
        None,
        "the bag went in with only itself counted"
    );
}

#[test]
fn an_opened_container_lists_what_was_put_in_it() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);

    // Put the item in, then open the container and read the count.
    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(item_serial.raw()),
        destination: DropDestination::Item {
            item: container,
            at:   GumpPoint::new(50, 60),
        },
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(container.raw())),
    });
    world.tick(now);

    let contents = packets_for(&mut world, player)
        .into_iter()
        .find(|p| p[0] == 0x3C)
        .expect("a contents packet");
    assert_eq!(
        u16::from_be_bytes([contents[3], contents[4]]),
        1,
        "the one item is listed"
    );
}

#[test]
fn picking_an_item_out_of_a_container_holds_it() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);

    // In, then straight back out.
    for _ in 0..1 {
        world.queue(Command::PickUpItem {
            connection: player,
            serial:     RawSerial(item_serial.raw()),
            amount:     1,
        });
        world.tick(now);
        world.queue(Command::DropItem {
            connection:  player,
            serial:      RawSerial(item_serial.raw()),
            destination: DropDestination::Item {
                item: container,
                at:   GumpPoint::new(50, 60),
            },
        });
        world.tick(now);
    }
    assert!(world.state.registry.has::<Contained>(item));

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    assert!(
        !world.state.registry.has::<Contained>(item),
        "lifting it out drops the containment"
    );
    assert!(world.state.held_of(player).is_some(), "and it is on the cursor");
}

#[test]
fn dropping_into_something_that_is_not_a_container_bounces() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    // Plain items, rather than coins: gold is a valid stack target now.
    let target = spawn_plain_item_at(&mut world, here, now);
    let held_serial = spawn_plain_item_at(&mut world, here, now);
    let held_item = entity(&world, held_serial);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(held_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    let origin = Point::new(START.x, START.y, 0);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(held_serial.raw()),
        // A real item, but not a container — which is exactly why the variant is
        // `Item` and not `Container`: the wire cannot tell the two apart.
        destination: DropDestination::Item {
            item: target,
            at:   GumpPoint::new(0, 0),
        },
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
        "the drag is cancelled"
    );
    assert_eq!(
        world.state.registry.get::<Position>(held_item).map(|p| p.0),
        Some(origin),
        "and the item is back on the ground where it was"
    );
}

/// A `0x08` whose destination addresses nothing still owes the client its item
/// back.
///
/// The variant that is easiest to get wrong: `Nowhere` reads like "nothing to
/// do", and doing nothing leaves the item on the client's cursor forever with
/// the server believing it is held — the classic way an item goes quietly
/// missing. Making it a `match` arm rather than an `Option` the caller may
/// ignore is what puts the obligation somewhere the compiler can see it, and
/// this test is what proves the arm does the work.
#[test]
fn a_drop_that_addresses_nothing_bounces_rather_than_swallowing_the_item() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    spawn_item_at(&mut world, here, now);
    let held_serial = loose_item_serial(&world);
    let held_item = entity(&world, held_serial);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(held_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    assert!(
        world.state.held_of(player).is_some(),
        "on the cursor to begin with"
    );
    let _ = packets_for(&mut world, player);

    // What a client sends when it has lost track of what it is dropping onto:
    // a zero. It is neither the ground sentinel nor a serial, and the packet
    // reads it as `Nowhere` — see `DropItem::destination`.
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(held_serial.raw()),
        destination: DropDestination::Nowhere,
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
        "the drag is cancelled"
    );
    assert!(
        world.state.held_of(player).is_none(),
        "and the server is no longer holding it"
    );
    assert_eq!(
        world.state.registry.get::<Position>(held_item).map(|p| p.0),
        Some(here),
        "the item is back where it was lifted from"
    );
}

/// Whether a 4-byte serial appears anywhere in a packet's body.
fn mentions(packet: &[u8], serial: Serial) -> bool {
    packet.windows(4).any(|w| w == serial.raw().to_be_bytes())
}

/// Spawn a ground item at the player's feet and pick it up. Returns the item
/// it just made — the newest one, by serial, so earlier items in the world do
/// not confuse it.
fn take_loose_item(world: &mut World, connection: ConnectionId, now: Instant) -> (Serial, EntityId) {
    spawn_item_at(world, Point::new(START.x, START.y, 0), now);
    let (item, serial) = world
        .state
        .registry
        .query::<Position>()
        .filter(|(entity, _)| {
            world.state.registry.has::<Drawn>(*entity) && !world.state.registry.has::<Container>(*entity)
        })
        .filter_map(|(entity, _)| world.state.registry.serial_of(entity).map(|s| (entity, s)))
        .max_by_key(|(_, serial)| *serial)
        .expect("a ground item to lift");
    world.queue(Command::PickUpItem {
        connection,
        serial: RawSerial(serial.raw()),
        amount: 1,
    });
    world.tick(now);
    (serial, item)
}

/// A plausible armour layer.
const LAYER_TORSO: Layer = Layer(5);

#[test]
fn equipping_a_held_item_dresses_the_mobile() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let me = serial_of(&world, player);
    let (item_serial, item) = take_loose_item(&mut world, player, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::EquipItem {
        connection: player,
        item:       RawSerial(item_serial.raw()),
        layer:      RawLayer(LAYER_TORSO.0),
        mobile:     RawSerial(me.raw()),
    });
    world.tick(now);

    let worn = world
        .state
        .registry
        .get::<Equipped>(item)
        .expect("the item is now worn");
    assert_eq!(worn.mobile, me);
    assert_eq!(worn.layer, LAYER_TORSO);
    // Three worn things now: the torso item, and the backpack and bank box every
    // character is given on entry.
    assert_eq!(world.state.equipment_of(me).len(), 3);
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x2E),
        "the wearer is told they put it on"
    );
}

#[test]
fn double_clicking_a_backpack_weapon_equips_it_and_stows_the_previous_one() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let wearer_serial = serial_of(&world, player);
    let backpack = items::backpack_of(&world.state, wearer_serial).expect("a fresh player has a backpack");
    let sword = items::place_one(&mut world.state, backpack, Graphic(0x0F61), Hue(0), 1)
        .expect("a sword fits in the backpack");
    let sword_serial = world.registry().serial_of(sword).unwrap();
    let katana = items::place_one(&mut world.state, backpack, Graphic(0x13FF), Hue(0), 1)
        .expect("a katana fits in the backpack");
    let katana_serial = world.registry().serial_of(katana).unwrap();
    let bow = items::place_one(&mut world.state, backpack, Graphic(0x13B2), Hue(0), 1)
        .expect("a bow fits in the backpack");
    let bow_serial = world.registry().serial_of(bow).unwrap();
    let _ = packets_for(&mut world, player);

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(sword_serial.raw())),
    });
    world.tick(now);
    assert_eq!(
        world.state.registry.get::<Equipped>(sword),
        Some(&Equipped {
            mobile: wearer_serial,
            layer:  Layer(1),
        }),
        "a backpack weapon goes into the one-handed slot without a drag"
    );

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(katana_serial.raw())),
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Equipped>(katana),
        Some(&Equipped {
            mobile: wearer_serial,
            layer:  Layer(1),
        }),
        "the replacement is equipped"
    );
    assert!(
        matches!(
            openshard_state::item_location(&world.state, sword),
            Some(openshard_state::ItemLocation::Settled(
                SettledItemLocation::Contained(Contained { container, .. })
            ))
                if container == backpack
        ),
        "the replaced weapon returns to the backpack"
    );
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|packet| packet[0] == 0x2E),
        "the client is told about the new paperdoll equipment"
    );

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(bow_serial.raw())),
    });
    world.tick(now);
    assert_eq!(
        world.state.registry.get::<Equipped>(bow),
        Some(&Equipped {
            mobile: wearer_serial,
            layer:  Layer(2),
        }),
        "a two-handed weapon selects its own hand layer"
    );
    assert!(
        matches!(
            openshard_state::item_location(&world.state, katana),
            Some(openshard_state::ItemLocation::Settled(
                SettledItemLocation::Contained(Contained { container, .. })
            )) if container == backpack
        ),
        "equipping two-handed gear clears the occupied one-handed slot"
    );
}

#[test]
fn a_newcomer_sees_a_dressed_mobile_in_its_0x78() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let me = serial_of(&world, player);
    let (item_serial, _) = take_loose_item(&mut world, player, now);
    world.queue(Command::EquipItem {
        connection: player,
        item:       RawSerial(item_serial.raw()),
        layer:      RawLayer(LAYER_TORSO.0),
        mobile:     RawSerial(me.raw()),
    });
    world.tick(now);

    // A second player walks up and is drawn the first, now dressed.
    let newcomer = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let drawn = packets_for(&mut world, newcomer)
        .into_iter()
        .find(|p| p[0] == 0x78 && mentions(p, me))
        .expect("the dressed mobile is drawn");
    assert!(
        mentions(&drawn, item_serial),
        "the worn item rides along in the 0x78"
    );
}

#[test]
fn unequipping_lifts_the_item_off_and_forgets_it_for_others() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let watcher = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let me = serial_of(&world, player);
    let (item_serial, item) = take_loose_item(&mut world, player, now);
    world.queue(Command::EquipItem {
        connection: player,
        item:       RawSerial(item_serial.raw()),
        layer:      RawLayer(LAYER_TORSO.0),
        mobile:     RawSerial(me.raw()),
    });
    world.tick(now);
    let _ = packets_for(&mut world, watcher);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);

    assert!(!world.state.registry.has::<Equipped>(item), "it comes off");
    assert!(world.state.held_of(player).is_some(), "and onto the cursor");
    assert!(
        packets_for(&mut world, watcher)
            .iter()
            .any(|p| p == &encode_packet(&Remove { serial: item_serial }, ClientVersion::TOL)),
        "the other player is told to forget it"
    );
}

#[test]
fn consuming_a_ground_item_removes_it_and_clears_every_screen() {
    // The one-shot primitive: a used item vanishes wherever it is. On the ground
    // that is the decay path — off the sector grid, out of the registry, a 0x1D
    // to everyone who had it drawn.
    let now = Instant::now();
    let mut world = world();
    let watcher = enter(&mut world, now);
    let serial = spawn_plain_item_at(&mut world, Point::new(START.x, START.y, 0), now);
    let item = entity(&world, serial);
    let _ = packets_for(&mut world, watcher);

    world.queue(Command::ConsumeItem { serial, amount: 0 });
    world.tick(now);

    assert!(
        !world.state.registry.contains(item),
        "the item is gone from the world"
    );
    assert!(
        packets_for(&mut world, watcher)
            .iter()
            .any(|p| p == &encode_packet(&Remove { serial }, ClientVersion::TOL)),
        "and off the watcher's screen"
    );
}

#[test]
fn consuming_an_item_on_the_cursor_releases_the_drag_transaction() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let (item_serial, item) = take_loose_item(&mut world, player, now);
    let _ = packets_for(&mut world, player);

    assert!(world.state.held_of(player).is_some(), "the lift is in flight");
    world.queue(Command::ConsumeItem {
        serial: item_serial,
        amount: 0,
    });
    world.tick(now);

    assert!(!world.state.registry.contains(item), "the item was consumed");
    assert!(nothing_is_held(&world), "the server cursor was released with it");
    assert!(
        {
            let packets = packets_for(&mut world, player);
            packets.iter().any(|p| p[0] == 0x27)
                && packets
                    .iter()
                    .any(|p| p == &encode_packet(&Remove { serial: item_serial }, ClientVersion::TOL))
        },
        "the client was told to clear both its cursor and stale source projection"
    );
}

#[test]
fn consuming_a_contained_item_removes_it_from_the_open_gump() {
    // In a container the only client that need hear is one with the gump open —
    // the reagent-burn path. Put an item in, open the bag, consume it, and the
    // viewer is told to forget it live.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(item_serial.raw()),
        destination: DropDestination::Item {
            item: container,
            at:   GumpPoint::new(50, 60),
        },
    });
    world.tick(now);
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(container.raw())),
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::ConsumeItem {
        serial: item_serial,
        amount: 0,
    });
    world.tick(now);

    assert!(!world.state.registry.contains(item), "the contained item is gone");
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| p == &encode_packet(&Remove { serial: item_serial }, ClientVersion::TOL)),
        "and the open gump is told to forget it"
    );
}

#[test]
fn consuming_a_worn_item_takes_it_off_for_everyone_including_the_wearer() {
    // Worn is the third place. There is no "remove from paperdoll" packet, so
    // everyone forgets the item by its serial — and unlike a lift, the wearer's
    // own client is told too, because it is not the one holding it on a cursor.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let watcher = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let me = serial_of(&world, player);
    let (item_serial, item) = take_loose_item(&mut world, player, now);
    world.queue(Command::EquipItem {
        connection: player,
        item:       RawSerial(item_serial.raw()),
        layer:      RawLayer(LAYER_TORSO.0),
        mobile:     RawSerial(me.raw()),
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);
    let _ = packets_for(&mut world, watcher);

    world.queue(Command::ConsumeItem {
        serial: item_serial,
        amount: 0,
    });
    world.tick(now);

    assert!(!world.state.registry.contains(item), "the worn item is gone");
    // One drain: `packets_for` empties the whole outbox, so checking two
    // connections means partitioning a single sweep, not calling it twice.
    let forget = encode_packet(&Remove { serial: item_serial }, ClientVersion::TOL);
    let mut watcher_forgot = false;
    let mut wearer_forgot = false;
    for out in world.drain_outbound() {
        if out.packet == forget {
            watcher_forgot |= out.connection == watcher;
            wearer_forgot |= out.connection == player;
        }
    }
    assert!(watcher_forgot, "the onlooker forgets it");
    assert!(wearer_forgot, "and so does the wearer");
}

#[test]
fn consuming_part_of_a_stack_leaves_the_rest() {
    // A potion out of a lot of five: an amount smaller than the pile decrements
    // it rather than deleting the stack. The realistic case lives in a pack, but
    // a ground pile is the simplest to assert against.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = spawn_gold(&mut world, Point::new(START.x, START.y, 0), 5, now);
    let item = entity(&world, serial);
    let _ = packets_for(&mut world, player);

    world.queue(Command::ConsumeItem { serial, amount: 2 });
    world.tick(now);

    assert!(world.state.registry.contains(item), "the pile is still there");
    assert_eq!(
        world.state.registry.get::<Amount>(item).map(|a| a.0),
        Some(3),
        "with two taken off"
    );
}

#[test]
fn consuming_a_container_takes_its_contents_with_it() {
    // Consuming a container cascades into its contents, the same as a decaying
    // corpse — no orphan loot pointing at a gone serial.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);
    let container_entity = entity(&world, container);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(item_serial.raw()),
        destination: DropDestination::Item {
            item: container,
            at:   GumpPoint::new(50, 60),
        },
    });
    world.tick(now);

    world.queue(Command::ConsumeItem {
        serial: container,
        amount: 0,
    });
    world.tick(now);

    assert!(
        !world.state.registry.contains(container_entity),
        "the container is gone"
    );
    assert!(
        !world.state.registry.contains(item),
        "and its contents with it, not left orphaned"
    );
}

#[test]
fn consuming_a_stray_serial_does_nothing() {
    // Guarded like add_loot: a stale or bogus serial removes nothing rather than
    // erroring, and touches no other item.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = spawn_plain_item_at(&mut world, Point::new(START.x, START.y, 0), now);
    let item = entity(&world, serial);
    let _ = packets_for(&mut world, player);

    world.queue(Command::ConsumeItem {
        serial: Serial::new(0x4000_0000).unwrap(),
        amount: 0,
    });
    world.tick(now);

    assert!(world.state.registry.contains(item), "the real item is untouched");
}

#[test]
fn equipping_into_an_occupied_layer_swaps_the_old_item_into_the_backpack() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let me = serial_of(&world, player);

    // First item onto the torso.
    let (first, _) = take_loose_item(&mut world, player, now);
    world.queue(Command::EquipItem {
        connection: player,
        item:       RawSerial(first.raw()),
        layer:      RawLayer(LAYER_TORSO.0),
        mobile:     RawSerial(me.raw()),
    });
    world.tick(now);

    // Second item, same layer: the worn item is stowed and this one replaces it.
    let (second, second_item) = take_loose_item(&mut world, player, now);
    let _ = packets_for(&mut world, player);
    world.queue(Command::EquipItem {
        connection: player,
        item:       RawSerial(second.raw()),
        layer:      RawLayer(LAYER_TORSO.0),
        mobile:     RawSerial(me.raw()),
    });
    world.tick(now);

    assert!(
        !packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
        "the replacement is accepted"
    );
    assert!(
        matches!(
            openshard_state::item_location(&world.state, second_item),
            Some(openshard_state::ItemLocation::Settled(
                SettledItemLocation::Equipped(Equipped { mobile, layer })
            )) if mobile == me && layer == LAYER_TORSO
        ),
        "the second item occupies the torso layer"
    );
    let backpack = items::backpack_of(&world.state, me).expect("a fresh player has a backpack");
    let first_item = entity(&world, first);
    assert!(
        matches!(
            openshard_state::item_location(&world.state, first_item),
            Some(openshard_state::ItemLocation::Settled(
                SettledItemLocation::Contained(Contained { container, .. })
            )) if container == backpack
        ),
        "the old garment returns to the backpack"
    );
    assert!(nothing_is_held(&world), "the accepted equip releases the cursor");
}

#[test]
fn you_cannot_equip_an_item_onto_another_player() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let other = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let other_serial = serial_of(&world, other);
    let (held, held_item) = take_loose_item(&mut world, player, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::EquipItem {
        connection: player,
        item:       RawSerial(held.raw()),
        layer:      RawLayer(LAYER_TORSO.0),
        mobile:     RawSerial(other_serial.raw()),
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
        "the shard rejects dressing another mobile"
    );
    assert!(
        world.state.registry.has::<Position>(held_item),
        "the item returns to its remembered origin"
    );
    assert!(nothing_is_held(&world), "and the cursor is released");
}

#[test]
fn you_cannot_equip_onto_something_that_is_not_a_mobile() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    // A second ground item to (wrongly) equip onto.
    spawn_item_at(&mut world, Point::new(START.x, START.y, 0), now);
    let target = loose_item_serial(&world);
    let (held, held_item) = take_loose_item(&mut world, player, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::EquipItem {
        connection: player,
        item:       RawSerial(held.raw()),
        layer:      RawLayer(LAYER_TORSO.0),
        mobile:     RawSerial(target.raw()), // an item, not a mobile
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
        "refused"
    );
    assert!(
        world.state.registry.has::<Position>(held_item),
        "and bounced back"
    );
}

#[test]
fn dropping_a_stack_onto_an_identical_one_merges_them() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let pile = spawn_gold(&mut world, here, 100, now);
    let loose = spawn_gold(&mut world, here, 50, now);
    let pile_item = entity(&world, pile);
    let loose_item = entity(&world, loose);
    let _ = packets_for(&mut world, player);

    // Lift the small pile and drop it onto the big one.
    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(loose.raw()),
        amount:     50,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(loose.raw()),
        // Dropping onto the other stack. It is on the ground, so the gump point
        // is nobody's coordinate; the merge arm never reads it.
        destination: DropDestination::Item {
            item: pile,
            at:   GumpPoint::new(0, 0),
        },
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Amount>(pile_item).map(|a| a.0),
        Some(150),
        "the amounts add"
    );
    assert!(
        !world.state.registry.contains(loose_item),
        "and the dropped pile is gone"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x1A),
        "the surviving pile is redrawn with its new amount"
    );
}

#[test]
fn a_single_spawned_gold_coin_stacks_with_a_pile() {
    // A single coin used to inherit `stackable: false` from the generic spawn
    // path, so `.add 0xeed` produced gold that could never join a pile.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let pile = spawn_gold(&mut world, here, 100, now);
    spawn_item_at(&mut world, here, now);
    let coin = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(item, drawn)| {
            drawn.id == Graphic(GOLD)
                && !world.state.registry.has::<Amount>(*item)
                && world.state.registry.has::<Position>(*item)
        })
        .and_then(|(item, _)| world.state.registry.serial_of(item))
        .expect("the spawned single coin");
    let pile_item = entity(&world, pile);
    let coin_item = entity(&world, coin);

    assert!(
        world.state.registry.has::<Stackable>(coin_item),
        "a one-coin gold spawn is still stackable"
    );
    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(coin.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(coin.raw()),
        destination: DropDestination::Item {
            item: pile,
            at:   GumpPoint::new(0, 0),
        },
    });
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<Amount>(pile_item)
            .map(|amount| amount.0),
        Some(101),
        "the coin joined the gold pile"
    );
    assert!(!world.state.registry.contains(coin_item));
}

#[test]
fn single_arrows_and_bolts_are_stackable_from_spawn() {
    // Ammunition is often created one piece at a time (staff tools and older
    // saves do this), but it must still behave like the piles a vendor sells.
    // The stack-all client pass can then safely send this exact lift/drop pair.
    for graphic in [0x0F3F, 0x1BFB] {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let here = Point::new(START.x, START.y, 0);
        let spawn = |world: &mut World| {
            world.queue(Command::SpawnItem {
                graphic:   Graphic(graphic),
                hue:       Hue(0),
                amount:    1,
                stackable: false,
                position:  here,
                facet:     Facet(0),
            });
            world.tick(now);
            world
                .state
                .registry
                .query::<Drawn>()
                .filter(|(item, drawn)| {
                    drawn.id == Graphic(graphic) && world.state.registry.has::<Position>(*item)
                })
                .filter_map(|(item, _)| world.state.registry.serial_of(item))
                .max()
                .expect("the single piece of ammunition")
        };
        let target = spawn(&mut world);
        let source = spawn(&mut world);
        let target_item = entity(&world, target);
        assert!(
            world.state.registry.has::<Stackable>(target_item),
            "0x{graphic:04X} is marked stackable while it is still one item"
        );

        world.queue(Command::PickUpItem {
            connection: player,
            serial:     RawSerial(source.raw()),
            amount:     1,
        });
        world.tick(now);
        world.queue(Command::DropItem {
            connection:  player,
            serial:      RawSerial(source.raw()),
            destination: DropDestination::Item {
                item: target,
                at:   GumpPoint::new(0, 0),
            },
        });
        world.tick(now);

        assert_eq!(
            world
                .state
                .registry
                .get::<Amount>(target_item)
                .map(|amount| amount.0),
            Some(2),
            "0x{graphic:04X} merged with the identical singleton"
        );
    }
}

#[test]
fn merging_past_the_stack_cap_keeps_the_remainder() {
    // The bug this exists to not have again: two 50,000 piles merged into one
    // clamped 65,535 and the difference was simply gone. Sphere's `CItem::Stack`
    // is the model — fill the destination to its maximum and leave the rest on
    // the source, which loses nothing. (ServUO refuses the merge outright; either
    // is honest, and this one is kinder.)
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let pile = spawn_gold(&mut world, here, 50_000, now);
    let loose = spawn_gold(&mut world, here, 50_000, now);
    let pile_item = entity(&world, pile);
    let loose_item = entity(&world, loose);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(loose.raw()),
        amount:     50_000,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(loose.raw()),
        destination: DropDestination::Item {
            item: pile,
            at:   GumpPoint::new(0, 0),
        },
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Amount>(pile_item).map(|a| a.0),
        Some(openshard_items::MAX_STACK),
        "the target filled to the cap"
    );
    assert!(
        world.state.registry.contains(loose_item),
        "and the source still exists"
    );
    assert_eq!(
        world.state.registry.get::<Amount>(loose_item).map(|a| a.0),
        Some(100_000u32.saturating_sub(u32::from(openshard_items::MAX_STACK)) as u16),
        "holding exactly what did not fit — no coin lost"
    );
}

#[test]
fn a_payout_past_the_stack_cap_lands_in_a_second_pile() {
    // The same rule where the world hands gold over rather than a player: a sale
    // or a loot drop bigger than one pile leaves two, the way a container in UO
    // ends up with two piles of gold. Clamping here paid 65,535 for a 100,000
    // sale and said nothing about the rest.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let backpack = backpack_serial(&world, connection);

    assert!(
        openshard_items::give(
            &mut world.state,
            backpack,
            openshard_protocol::wire::Graphic(GOLD),
            openshard_protocol::wire::Hue(0),
            100_000,
        )
        .is_complete()
    );

    let piles: Vec<u16> = world
        .state
        .registry
        .query::<Contained>()
        .filter(|(entity, held)| {
            held.container == backpack
                && world
                    .state
                    .registry
                    .get::<Drawn>(*entity)
                    .is_some_and(|g| g.id == openshard_protocol::wire::Graphic(GOLD))
        })
        .map(|(entity, _)| openshard_items::amount_of(&world.state, entity))
        .collect();
    assert_eq!(piles.len(), 2, "one full pile and a second for the rest");
    assert_eq!(
        piles.iter().map(|&a| u32::from(a)).sum::<u32>(),
        100_000,
        "and every coin arrived"
    );
    assert_eq!(
        openshard_items::total_gold(&world.state, player),
        100_000,
        "the status bar counts them together"
    );
}

#[test]
fn a_non_stackable_item_does_not_merge() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    // Two plain (non-stackable) items. Gold is deliberately not used here:
    // even a single coin is currency and must stack.
    let target = spawn_plain_item_at(&mut world, here, now);
    let held = spawn_plain_item_at(&mut world, here, now);
    let held_item = entity(&world, held);
    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(held.raw()),
        amount:     1,
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(held.raw()),
        destination: DropDestination::Item {
            item: target,
            at:   GumpPoint::new(0, 0),
        },
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
        "dropping one onto the other is refused"
    );
    assert!(
        world.state.registry.has::<Position>(held_item),
        "and it bounces back to the ground"
    );
}

#[test]
fn a_ground_item_decays_after_its_time() {
    let now = Instant::now();
    let mut world = world();
    let watcher = enter(&mut world, now);
    spawn_item_at(&mut world, Point::new(START.x, START.y, 0), now);
    let serial = loose_item_serial(&world);
    let item = entity(&world, serial);
    let _ = packets_for(&mut world, watcher);

    // Bring the decay forward rather than run twenty minutes of ticks.
    let soon = world.state.ticks + 1;
    world.state.registry.insert(item, Decays { at_tick: soon });
    world.tick(now);

    assert!(!world.state.registry.contains(item), "the item has rotted away");
    assert!(
        packets_for(&mut world, watcher)
            .iter()
            .any(|p| p == &encode_packet(&Remove { serial }, ClientVersion::TOL)),
        "and left every screen"
    );
}

#[test]
fn gameplay_config_reaches_the_systems() {
    // The [gameplay] knobs flow through WorldState to the systems: a five-second
    // decay here gives a spawned item a clock of a hundred ticks, not the
    // twenty-minute default's twenty-four thousand.
    let now = Instant::now();
    let gameplay = Gameplay {
        combat_era: CombatEra::new(2),
        speed_scale_factor: 40000,
        skill_cap: 700,
        decay_ticks: Gameplay::ticks(5),
        criminal_ticks: Gameplay::ticks(60),
        ..Gameplay::default()
    };
    let mut world = World::new(START).with_gameplay(gameplay);
    world.queue(Command::SpawnItem {
        graphic:   openshard_protocol::wire::Graphic(0x0EED),
        hue:       openshard_protocol::wire::Hue(0),
        amount:    1,
        stackable: false,
        position:  Point::new(START.x, START.y, 0),
        facet:     Facet(0),
    });
    world.tick(now);

    let serial = loose_item_serial(&world);
    let item = entity(&world, serial);
    let decay = world.state.registry.get::<Decays>(item).unwrap();
    assert!(
        decay.at_tick > world.state.ticks && decay.at_tick <= world.state.ticks + Gameplay::ticks(5),
        "the five-second decay reached mark_decay (at_tick {}, now {})",
        decay.at_tick,
        world.state.ticks
    );
}

#[test]
fn zero_item_decay_keeps_ground_items_and_corpses() {
    let now = Instant::now();
    let mut world = World::new(START).with_gameplay(Gameplay {
        decay_ticks: 0,
        ..Gameplay::default()
    });
    let watcher = enter(&mut world, now);
    spawn_item_at(&mut world, Point::new(START.x, START.y, 0), now);
    let item = entity(&world, loose_item_serial(&world));

    // A loose item created with cleanup off has no clock at all.
    assert!(
        !world.state.registry.has::<Decays>(item),
        "cleanup off does not mark a new ground item"
    );

    // Even an already-marked item (including a corpse's direct clock) remains.
    world.state.registry.insert(
        item,
        Decays {
            at_tick: world.state.ticks,
        },
    );
    world.tick(now);
    assert!(
        world.state.registry.contains(item),
        "cleanup off does not sweep items"
    );
    assert!(
        !packets_for(&mut world, watcher)
            .iter()
            .any(|packet| packet[0] == 0x1D),
        "the watcher is not told that the item vanished"
    );
}

#[test]
fn a_container_does_not_decay_even_after_being_moved() {
    // A backpack is a ground item too, but it must not rot — and picking it
    // up and setting it back down must not hand it a decay clock either.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let container = spawn_container_at(&mut world, here, now);
    let container_item = entity(&world, container);
    assert!(
        !world.state.registry.has::<Decays>(container_item),
        "a fresh container has no decay clock"
    );

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(container.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(container.raw()),
        destination: DropDestination::Ground(here),
    });
    world.tick(now);

    assert!(world.state.registry.has::<Position>(container_item), "back down");
    assert!(
        !world.state.registry.has::<Decays>(container_item),
        "and still no decay clock after moving it"
    );
}

#[test]
fn an_item_off_the_ground_does_not_decay() {
    // Lifting an item takes the decay clock off it: a stack on a cursor, in a
    // pack or worn does not rot.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let (_, item) = take_loose_item(&mut world, player, now);
    assert!(
        !world.state.registry.has::<Decays>(item),
        "a held item carries no decay clock"
    );
}

#[test]
fn picking_up_part_of_a_stack_splits_it() {
    // Take 30 of 100: the original keeps its serial and holds the 30 on the
    // cursor, and a new pile of 70 is left on the ground where it was — the
    // way Sphere's UnStackSplit does it.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let pile = spawn_gold(&mut world, here, 100, now);
    let pile_item = entity(&world, pile);
    let _ = packets_for(&mut world, player);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(pile.raw()),
        amount:     30,
    });
    world.tick(now);

    // The original, still serial `pile`, is on the cursor holding 30.
    assert!(world.state.held_of(player).is_some());
    assert_eq!(openshard_items::amount_of(&world.state, pile_item), 30);
    assert!(!world.state.registry.has::<Position>(pile_item), "off the ground");

    // A brand-new pile of 70 sits where the stack was.
    let (leftover, _) = world
        .state
        .registry
        .query::<Position>()
        .find(|(entity, _)| world.state.registry.has::<Stackable>(*entity) && *entity != pile_item)
        .expect("a leftover pile on the ground");
    assert_eq!(openshard_items::amount_of(&world.state, leftover), 70);
    assert_ne!(
        world.state.registry.serial_of(leftover).unwrap(),
        pile,
        "the leftover is a new object with a new serial"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x1A),
        "and the player is drawn the leftover pile"
    );
}

#[test]
fn splitting_a_typed_pile_copies_its_identity_without_reinterpreting_art() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let pile_item = items::spawn_item_kind(
        &mut world.state,
        openshard_protocol::item_kind::ItemKindId(1),
        Some(openshard_protocol::item_kind::MaterialId(9)),
        100,
        true,
        here,
        Facet(0),
    )
    .expect("typed ingot pile");
    let pile = world.registry().serial_of(pile_item).expect("item serial");

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(pile.raw()),
        amount:     30,
    });
    world.tick(now);

    let leftover = world
        .state
        .registry
        .query::<Position>()
        .find_map(|(item, _)| {
            (item != pile_item && world.state.registry.has::<Stackable>(item)).then_some(item)
        })
        .expect("split remainder");
    for item in [pile_item, leftover] {
        assert_eq!(
            world
                .state
                .registry
                .get::<openshard_state::components::ItemKind>(item),
            Some(&openshard_state::components::ItemKind(
                openshard_protocol::item_kind::ItemKindId(1)
            ))
        );
        assert_eq!(
            world
                .state
                .registry
                .get::<openshard_state::components::Material>(item),
            Some(&openshard_state::components::Material(
                openshard_protocol::item_kind::MaterialId(9)
            ))
        );
    }
}

#[test]
fn a_typed_payout_never_merges_with_a_same_drawn_different_kind() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    let owner = world.registry().serial_of(player_entity).expect("player serial");
    let pack = items::backpack_of(&world.state, owner).expect("backpack");

    let impostor = items::give(&mut world.state, pack, Graphic(0x1BF2), Hue(0x08AB), 7)
        .last
        .expect("legacy-shaped pile");
    world.state.registry.insert(
        impostor,
        openshard_state::components::ItemKind(openshard_protocol::item_kind::ItemKindId(999)),
    );
    world.state.registry.insert(
        impostor,
        openshard_state::components::Material(openshard_protocol::item_kind::MaterialId(9)),
    );

    let outcome = items::give_kind(
        &mut world.state,
        pack,
        openshard_protocol::item_kind::ItemKindId(1),
        Some(openshard_protocol::item_kind::MaterialId(9)),
        5,
    )
    .expect("typed ingot definition");
    let real = outcome.last.expect("typed payout");
    assert_ne!(real, impostor);
    assert_eq!(openshard_items::amount_of(&world.state, impostor), 7);
    assert_eq!(openshard_items::amount_of(&world.state, real), 5);
    assert_eq!(
        world
            .state
            .registry
            .get::<openshard_state::components::ItemKind>(real),
        Some(&openshard_state::components::ItemKind(
            openshard_protocol::item_kind::ItemKindId(1)
        ))
    );
}

#[test]
fn a_wrong_kind_same_art_pile_cannot_bypass_typed_backpack_capacity() {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };

    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    let owner = world.registry().serial_of(player_entity).expect("player serial");
    let pack = items::backpack_of(&world.state, owner).expect("backpack");
    // Fill 124 of the 125 direct slots. The final item is an art-identical but
    // deliberately different semantic pile, so a genuine typed ingot still
    // needs the last (unavailable) slot rather than a merge.
    assert!(items::give_containers_to_backpack(
        &mut world.state,
        owner,
        items::BACKPACK_GRAPHIC,
        items::BACKPACK_GUMP,
        Hue::NONE,
        124,
    ));
    let impostor = items::give(&mut world.state, pack, Graphic(0x1BF2), Hue(0x08AB), 7)
        .last
        .expect("the final occupied slot");
    world
        .state
        .registry
        .insert(impostor, openshard_state::components::ItemKind(ItemKindId(999)));
    world
        .state
        .registry
        .insert(impostor, openshard_state::components::Material(MaterialId(9)));

    assert!(
        !items::give_kind_to_backpack(
            &mut world.state,
            owner,
            ItemKindId(1),
            Some(MaterialId(9)),
            5,
            true,
        ),
        "same art is not a free merge slot for a different semantic kind"
    );
    assert_eq!(
        openshard_items::amount_of(&world.state, impostor),
        7,
        "the existing pile was untouched"
    );
}

#[test]
fn the_split_portion_keeps_its_serial_and_can_be_dropped() {
    // The reason the original keeps its serial: the client's cursor still
    // names it, so the 0x08 that drops the 30 back matches the held item.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let pile = spawn_gold(&mut world, here, 100, now);
    let pile_item = entity(&world, pile);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(pile.raw()),
        amount:     30,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(pile.raw()), // the client drops the same serial it lifted
        destination: DropDestination::Ground(here),
    });
    world.tick(now);

    assert!(nothing_is_held(&world), "the drop landed, not bounced");
    assert!(world.state.registry.has::<Position>(pile_item));
    assert_eq!(openshard_items::amount_of(&world.state, pile_item), 30);
}

#[test]
fn picking_up_a_whole_stack_does_not_split_it() {
    // Asking for the whole amount, or more, lifts the pile itself — no
    // leftover, one object.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let pile = spawn_gold(&mut world, here, 100, now);
    let pile_item = entity(&world, pile);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(pile.raw()),
        amount:     100,
    });
    world.tick(now);

    assert_eq!(
        openshard_items::amount_of(&world.state, pile_item),
        100,
        "the whole pile is held"
    );
    assert_eq!(
        world
            .state
            .registry
            .query::<Stackable>()
            .filter(|(entity, _)| world.state.registry.has::<Position>(*entity))
            .count(),
        0,
        "nothing is left on the ground"
    );
}

/// Put a pile of `amount` gold inside a fresh ground container and open it, so a
/// gump watcher exists. Returns `(container_serial, gold_serial)`; the gold keeps
/// its serial across the move into the container.
fn gold_in_open_container(
    world: &mut World,
    player: ConnectionId,
    point: Point,
    amount: u16,
    now: Instant,
) -> (Serial, Serial) {
    let container = spawn_container_at(world, point, now);
    let gold = spawn_gold(world, point, amount, now);
    world.queue(Command::PickUpItem {
        connection: player,
        serial: RawSerial(gold.raw()),
        amount, // the whole pile, so no split and the serial is kept
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(gold.raw()),
        destination: DropDestination::Item {
            item: container,
            at:   GumpPoint::new(50, 60),
        },
    });
    world.tick(now);
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(container.raw())),
    });
    world.tick(now);
    (container, gold)
}

#[test]
fn dropping_a_stack_onto_an_identical_one_inside_a_container_merges_them() {
    // The ground merge, but the target pile is inside a container: the amounts
    // add, the dropped pile is gone, and every open gump is told the new total.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let (_container, pile) = gold_in_open_container(&mut world, player, here, 100, now);
    let pile_item = entity(&world, pile);
    let loose = spawn_gold(&mut world, here, 50, now);
    let loose_item = entity(&world, loose);
    let _ = packets_for(&mut world, player);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(loose.raw()),
        amount:     50,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  player,
        serial:      RawSerial(loose.raw()),
        // Onto the contained stack — here the gump point would be a real one,
        // and the merge arm still does not read it.
        destination: DropDestination::Item {
            item: pile,
            at:   GumpPoint::new(0, 0),
        },
    });
    world.tick(now);

    assert_eq!(
        openshard_items::amount_of(&world.state, pile_item),
        150,
        "the amounts add"
    );
    assert!(
        !world.state.registry.contains(loose_item),
        "and the dropped pile is gone"
    );
    assert!(
        world.state.registry.has::<Contained>(pile_item),
        "the survivor is still in the container"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x25),
        "the open gump is told the new amount"
    );
}

#[test]
fn picking_up_part_of_a_stack_from_a_container_splits_it() {
    // Take 30 of 100 out of a container: the original keeps its serial and holds
    // 30 on the cursor, and a new pile of 70 stays behind in the container — the
    // ground split's UnStackSplit, but the remainder stays contained.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let (container, pile) = gold_in_open_container(&mut world, player, here, 100, now);
    let pile_item = entity(&world, pile);
    // Saves from before gold was intrinsically stackable can still be alive in
    // a running world. Splitting uses the same graphic fallback as merging.
    world.state.registry.remove::<Stackable>(pile_item);
    let _ = packets_for(&mut world, player);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(pile.raw()),
        amount:     30,
    });
    world.tick(now);

    assert!(world.state.held_of(player).is_some(), "the original is held");
    assert_eq!(openshard_items::amount_of(&world.state, pile_item), 30);
    assert!(
        !world.state.registry.has::<Contained>(pile_item),
        "the held original is out of the container"
    );

    let container_serial = container;
    let (leftover, _) = world
        .state
        .registry
        .query::<Contained>()
        .find(|(entity, held)| {
            held.container == container_serial
                && world.state.registry.has::<Stackable>(*entity)
                && *entity != pile_item
        })
        .expect("a leftover pile in the container");
    assert_eq!(openshard_items::amount_of(&world.state, leftover), 70);
    assert_ne!(
        world.state.registry.serial_of(leftover).unwrap(),
        pile,
        "the leftover is a new object with a new serial"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x25),
        "the open gump is drawn the leftover pile"
    );
}

#[test]
fn picking_up_a_whole_stack_from_a_container_does_not_split_it() {
    // Asking for the whole amount lifts the pile itself — no leftover, one
    // object, nothing left contained.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    let (container, pile) = gold_in_open_container(&mut world, player, here, 100, now);
    let pile_item = entity(&world, pile);

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(pile.raw()),
        amount:     100,
    });
    world.tick(now);

    assert_eq!(
        openshard_items::amount_of(&world.state, pile_item),
        100,
        "the whole pile is held"
    );
    let container_serial = container;
    assert_eq!(
        world
            .state
            .registry
            .query::<Contained>()
            .filter(|(_, held)| held.container == container_serial)
            .count(),
        0,
        "nothing is left in the container"
    );
}

/// Spawn a creature at `point` with `hits` and return its serial. An orange
/// enemy, no armour — the plain punching bag most combat tests want.
pub(super) fn spawn_mobile_at(world: &mut World, point: Point, hits: u16, now: Instant) -> Serial {
    spawn_mobile_full(world, point, hits, 5, combat::SWING_DAMAGE, 0, now)
}

/// Spawn a creature with every combat field spelled out, and return its serial.
fn spawn_mobile_full(
    world: &mut World,
    point: Point,
    hits: u16,
    notoriety: u8,
    damage: u16,
    resistance: u8,
    now: Instant,
) -> Serial {
    world.queue(Command::SpawnMobile {
        body: openshard_protocol::wire::Graphic(0x0190),
        hue: openshard_protocol::wire::Hue(0),
        hits,
        notoriety: Notoriety::from_bits(notoriety),
        damage,
        resistance: openshard_protocol::world::PhysicalResistance::new(resistance),
        swing: 0,        // the default pace
        sight: Sight(0), // passive by default; tests that want a brain set it
        aggression: Aggression::from_bits(2),
        beat: 0,
        ranged: None,
        ranged_kind: DamageType::Physical,
        wander: false,
        position: point,
        facet: Facet(0),
        name: None,
        title: None,
        shoe: 0,
        fame: 0,
        karma: 0,
        night_home: None,
        banker: false,
        vendor: false,
        healer: false,
        equipment: Vec::new(),
        skills: Vec::new(),
        stock: Vec::new(),
        escort_to: None,
        quests: Vec::new(),
    });
    world.tick(now);
    // The newest mobile that no client drives — the creature just made.
    world
        .state
        .registry
        .query::<Body>()
        .filter(|(entity, _)| !world.state.registry.has::<Client>(*entity))
        .filter_map(|(entity, _)| world.state.registry.serial_of(entity))
        .max()
        .expect("a spawned creature")
}

/// **What the shard puts on the sector grid is filed as what it is.**
///
/// The kind is declared where the thing is put down — by calling
/// `WorldState::place_mobile` rather than `place_item` — and never worked out
/// from the registry, which is what makes it impossible to go stale, at the cost
/// of the one thing no compiler catches: a caller reaching for the wrong one of
/// the two. A mobile filed as an item is invisible to sight, to chat, to a
/// guard's call and to the crowd a step is decided against, and *nothing errors*.
///
/// So this is the guard, and it holds the grid against the registry rather than
/// against itself: the real spawn paths run — a player entering, a creature
/// spawned, an item and a container placed, and a corpse left behind by a death
/// — and every row of both lists has to agree with the [`Body`] the registry
/// has.
#[test]
fn the_shard_files_what_it_spawns_as_what_it_is() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let at = Point::new(START.x, START.y, 0);

    // Eight hits, five a swing: dead on the second, and what it leaves is the
    // case most worth asserting — a corpse carries a body *graphic* and is not a
    // body.
    let mob = spawn_mobile_at(&mut world, at, 8, now);
    spawn_item_at(&mut world, Point::new(START.x + 1, START.y, 0), now);
    spawn_container_at(&mut world, Point::new(START.x + 2, START.y, 0), now);
    engage(&mut world, connection, mob, now);
    for _ in 0..(2 * WRESTLING_SWING_TICKS) {
        world.tick(now);
    }

    let sectors = world.state.facet_state(Facet(0)).sectors();
    let range = openshard_state::VIEW_RANGE;
    let mobiles: Vec<EntityId> = sectors.mobiles_near(at, range).map(|(id, _)| id).collect();
    let items: Vec<EntityId> = sectors.items_near(at, range).map(|(id, _)| id).collect();

    assert!(
        mobiles.contains(&player),
        "the player who entered is on the mobile list"
    );
    assert!(
        items.iter().any(|&entity| {
            world
                .state
                .registry
                .get::<Drawn>(entity)
                .is_some_and(|drawn| drawn.id == openshard_state::components::CORPSE_GRAPHIC)
        }),
        "the corpse the death left is on the item list"
    );
    assert!(items.len() >= 3, "the corpse, the item and the container");

    for entity in mobiles {
        assert!(
            world.state.registry.has::<Body>(entity),
            "filed as a mobile with no body: everything that reads the mobile list \
             would have to re-check what the list is for"
        );
    }
    for entity in items {
        assert!(
            !world.state.registry.has::<Body>(entity),
            "a body filed as furniture is invisible to everything that looks for people"
        );
    }
}

#[test]
fn a_spawned_creature_is_drawn_to_nearby_players() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let _ = packets_for(&mut world, player);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);

    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| p[0] == 0x78 && mentions(p, mob)),
        "the creature is drawn to the player"
    );
}

#[test]
fn damage_lowers_hits_and_updates_the_bar() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    let _ = packets_for(&mut world, player);

    world.queue(Command::Damage {
        serial:      mob,
        amount:      20,
        damage_type: 0,
        by:          None,
    });
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(mob_entity)
            .map(|h| h.current),
        Some(30),
        "50 minus 20"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0xA1),
        "the health bar is redrawn"
    );
}

#[test]
fn a_creature_dies_at_zero_hits() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 10, now);
    let mob_entity = entity(&world, mob);
    let _ = packets_for(&mut world, player);
    let mut died: Cursor<MobileDied> = world.bus().cursor();

    // Overkill: it dies once, not into the negatives.
    world.queue(Command::Damage {
        serial:      mob,
        amount:      100,
        damage_type: 0,
        by:          None,
    });
    world.tick(now);

    assert_eq!(world.bus().read(&mut died).count(), 1, "death is announced");
    assert!(
        !world.state.registry.contains(mob_entity),
        "and the creature is removed"
    );
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| p == &encode_packet(&Remove { serial: mob }, ClientVersion::TOL)),
        "and taken off the player's screen"
    );
}

#[test]
fn a_dead_mobile_is_not_killed_again() {
    // A player lies at zero hits without being despawned; a second blow must
    // not announce a second death.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    let mut died: Cursor<MobileDied> = world.bus().cursor();

    world.queue(Command::Damage {
        serial,
        amount: 200,
        damage_type: 0,
        by: None,
    });
    world.tick(now); // 100 -> 0
    assert_eq!(world.bus().read(&mut died).count(), 1, "the killing blow");

    world.queue(Command::Damage {
        serial,
        amount: 50,
        damage_type: 0,
        by: None,
    });
    world.tick(now); // already dead
    assert_eq!(
        world.bus().read(&mut died).count(),
        0,
        "a second blow on a corpse announces nothing"
    );
}

#[test]
fn a_player_who_dies_becomes_a_ghost() {
    // A dead player is not yanked from the world (despawning someone connected is
    // worse than a ghost) — they stay, at zero hits, wearing the grey ghost body,
    // and the client is told it is dead so it greys the world.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    let player_entity = world.state.players[&player];
    let mut died: Cursor<MobileDied> = world.bus().cursor();

    // Die from a real engaged state, so the transition has both halves to
    // settle rather than merely preserving an already-established peace state.
    let target = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    engage(&mut world, player, target, now);
    assert!(
        world
            .registry()
            .get::<Combat>(player_entity)
            .is_some_and(|combat| combat.warmode() && combat.target() == Some(target)),
        "engaged before death"
    );
    let _ = packets_for(&mut world, player);

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);

    assert_eq!(world.bus().read(&mut died).count(), 1, "death is announced");
    assert!(
        world.state.registry.contains(player_entity),
        "the player is still here"
    );
    assert!(
        world.state.registry.has::<Ghost>(player_entity),
        "and it is a ghost now"
    );
    assert_eq!(
        world.registry().get::<Body>(player_entity).map(|b| b.id.0),
        Some(0x0192),
        "wearing the male ghost body"
    );
    assert_eq!(
        world.registry().get::<Ghost>(player_entity).map(|g| g.body.id.0),
        Some(0x0190),
        "with its living body remembered for resurrection"
    );
    assert!(
        world
            .state
            .registry
            .get::<Combat>(player_entity)
            .is_some_and(|combat| combat.is_at_peace()),
        "the player keeps its session combat row, at peace and with no target"
    );
    assert_eq!(
        world
            .registry()
            .get::<Hitpoints>(player_entity)
            .map(|h| h.current),
        Some(0),
    );
    let packets = packets_for(&mut world, player);
    assert!(
        packets.iter().any(|p| p.as_slice() == [0x2C, 0x00]),
        "the client is told it is dead (0x2C)"
    );
    assert!(
        packets
            .iter()
            .any(|p| p.as_slice() == [0x72, 0x00, 0x00, 0x32, 0x00]),
        "death settles the client's war stance at peace"
    );
    assert!(
        packets
            .iter()
            .any(|p| p.as_slice() == [0xAA, 0x00, 0x00, 0x00, 0x00]),
        "death clears the client's attack marker"
    );

    // Intents are allowed to disagree with authoritative state. The server
    // settles both of these requests back to the ghost's actual state rather
    // than echoing what was requested or relying on the client to suppress it.
    world.queue(Command::WarMode {
        connection: player,
        war:        true,
    });
    world.queue(Command::Attack {
        connection: player,
        target:     Some(target),
    });
    world.tick(now);
    assert!(
        world
            .state
            .registry
            .get::<Combat>(player_entity)
            .is_some_and(|combat| combat.is_at_peace()),
        "a ghost's conflicting intents cannot change authoritative combat state"
    );
    let packets = packets_for(&mut world, player);
    assert!(
        packets
            .iter()
            .any(|p| p.as_slice() == [0x72, 0x00, 0x00, 0x32, 0x00]),
        "the requested war stance is answered with the settled stance"
    );
    assert!(
        packets
            .iter()
            .any(|p| p.as_slice() == [0xAA, 0x00, 0x00, 0x00, 0x00]),
        "the requested target is answered with the settled target"
    );
}

#[test]
fn a_dead_player_leaves_a_corpse_but_keeps_its_backpack() {
    // The corpse holds the worn armour; the backpack (a worn container, not loot)
    // stays on the ghost so a resurrected player is not left empty-handed.
    use openshard_protocol::serial::SerialKind;
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    let serial_obj = serial;
    let player_entity = world.state.players[&player];

    // Wear a robe (outer torso) so there is gear to fall to the corpse.
    let (robe, robe_serial) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        robe,
        Drawn {
            id:  openshard_protocol::wire::Graphic(0x1F03),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    openshard_state::establish_item_location(
        &mut world.state,
        robe,
        openshard_state::ItemLocation::equipped(Equipped {
            mobile: serial_obj,
            layer:  Layer(0x16),
        }),
    )
    .unwrap();

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);

    // The corpse is a container drawn as the player's body.
    let corpse = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(_, g)| g.id == openshard_protocol::wire::Graphic(0x2006))
        .map(|(entity, _)| entity)
        .expect("a player corpse was laid");
    let corpse_serial = world.registry().serial_of(corpse).unwrap();
    assert_eq!(
        world.registry().get::<CorpseBody>(corpse).unwrap().body,
        Graphic(0x0190),
        "a human corpse draws right"
    );
    assert!(
        !world.registry().has::<Amount>(corpse),
        "the corpse body is not an item stack"
    );

    // The robe fell into the corpse.
    assert_eq!(
        world.registry().get::<Contained>(robe).map(|c| c.container),
        Some(corpse_serial),
        "the worn robe is in the corpse"
    );
    assert!(
        world.registry().get::<Equipped>(robe).is_none(),
        "and no longer worn"
    );
    assert_eq!(
        world
            .registry()
            .get::<Corpse>(corpse)
            .map(|story| story.equipment.as_slice()),
        Some(
            &[openshard_protocol::items::CorpseEquipmentItem {
                layer: Layer(0x16),
                item:  robe_serial,
            }][..]
        ),
        "the corpse retains the layer after the robe has left the living body"
    );
    // A ghost cannot interact with anything, including its own corpse. A second
    // living character in the same starting tile is the client that asks the
    // container path which emits the layer map.
    let looter = enter(&mut world, now);
    let _ = packets_for(&mut world, looter);
    world.queue(Command::DoubleClick {
        connection: looter,
        request:    UseRequest::Use(RawSerial(corpse_serial.raw())),
    });
    world.tick(now);
    let packets = packets_for(&mut world, looter);
    assert!(
        packets.iter().any(|packet| packet.first() == Some(&0x89)),
        "opening a corpse sends the layer map beside its contents: {packets:?}"
    );
    assert!(
        packets.iter().any(|packet| packet.first() == Some(&0x24)),
        "the living client opens the corpse"
    );

    // The backpack is still worn on the (now ghost) player.
    let keeps_backpack = world
        .state
        .registry
        .query::<Equipped>()
        .any(|(_, worn)| worn.mobile == serial_obj && worn.layer == items::BACKPACK_LAYER);
    assert!(keeps_backpack, "the ghost keeps its backpack");
    let _ = (player_entity, robe_serial);
}

#[test]
fn a_weapon_on_the_cursor_when_its_owner_dies_falls_into_the_corpse() {
    // Lifting worn gear removes `Equipped` while the drag is in flight. Death
    // must collect that fourth, temporary location explicitly; restoring the
    // drag origin would otherwise equip the axe on the newly-created ghost.
    use openshard_protocol::serial::SerialKind;

    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mobile = serial_of(&world, player);

    let (axe, axe_serial) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        axe,
        Drawn {
            id:  Graphic(0x0F49),
            hue: Hue(0),
        },
    );
    openshard_state::establish_item_location(
        &mut world.state,
        axe,
        openshard_state::ItemLocation::equipped(Equipped {
            mobile,
            layer: openshard_state::weapon::LAYER_TWO_HANDED,
        }),
    )
    .unwrap();

    world.queue(Command::PickUpItem {
        connection: player,
        serial:     RawSerial(axe_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    assert_eq!(world.state.held_of(player).map(|held| held.entity), Some(axe));
    assert!(
        world.registry().get::<Equipped>(axe).is_none(),
        "the axe is in drag limbo"
    );
    let _ = packets_for(&mut world, player);

    world.queue(Command::Damage {
        serial:      mobile,
        amount:      500,
        damage_type: 0,
        by:          None,
    });
    world.tick(now);

    let corpse = world
        .registry()
        .query::<Corpse>()
        .next()
        .map(|(entity, _)| entity)
        .expect("a corpse was laid");
    let corpse_serial = world.registry().serial_of(corpse).unwrap();
    assert!(world.state.held_of(player).is_none(), "death clears the cursor");
    assert_eq!(
        world
            .registry()
            .get::<Contained>(axe)
            .map(|inside| inside.container),
        Some(corpse_serial),
        "the held axe became corpse loot"
    );
    assert!(
        world.registry().get::<Equipped>(axe).is_none(),
        "the axe was not restored onto the ghost"
    );
    assert_eq!(
        world
            .registry()
            .get::<Corpse>(corpse)
            .map(|story| story.equipment.as_slice()),
        Some(
            &[openshard_protocol::items::CorpseEquipmentItem {
                layer: openshard_state::weapon::LAYER_TWO_HANDED,
                item:  axe_serial,
            }][..]
        ),
        "the corpse preserves the axe's former hand layer"
    );
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|packet| packet.first() == Some(&0x27)),
        "the dead player's client is told to release its drag cursor"
    );
    assert_eq!(
        openshard_state::audit_item_graph(&world.state),
        Vec::new(),
        "death leaves corpse loot, backpack and shroud in one sound graph"
    );
}

#[test]
fn resurrection_brings_a_ghost_back() {
    // The staff `.res` command (and the Resurrection spell) call the same core
    // path: the ghost marker lifts, the living body and original outfit return,
    // and the consumed corpse is removed from the world.
    let now = Instant::now();
    let mut world = world();
    let player = enter_gm(&mut world, now);
    let serial = serial_of(&world, player);
    let player_entity = world.state.players[&player];
    let robe = items::equip_worn_item(&mut world.state, serial, Graphic(0x1F03), Hue(0), Layer(0x16))
        .expect("an original robe");
    let axe = items::equip_worn_item(
        &mut world.state,
        serial,
        Graphic(0x0F49),
        Hue(0),
        openshard_state::weapon::LAYER_TWO_HANDED,
    )
    .expect("an original axe");
    let robe_serial = world.registry().serial_of(robe).unwrap();
    let axe_serial = world.registry().serial_of(axe).unwrap();

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);
    assert!(world.state.registry.has::<Ghost>(player_entity), "dead first");
    let _ = packets_for(&mut world, player);

    // The Resurrection cursor may name the old body rather than the ghost.
    let corpse = world
        .registry()
        .query::<Corpse>()
        .find_map(|(entity, corpse)| {
            (corpse.player == Some(serial))
                .then(|| world.registry().serial_of(entity))
                .flatten()
        })
        .expect("the dead player has their own corpse");
    world.resurrect_target(corpse, false);
    world.tick(now);

    assert!(
        !world.state.registry.has::<Ghost>(player_entity),
        "no longer a ghost"
    );
    assert_eq!(
        world.registry().get::<Body>(player_entity).map(|b| b.id.0),
        Some(0x0190),
        "the living body is back"
    );
    assert!(
        world
            .registry()
            .get::<Hitpoints>(player_entity)
            .is_some_and(|h| h.current > 0),
        "and it is not standing at zero hits"
    );
    let equipment = world.state.equipment_of(serial);
    for (graphic, layer) in [
        (Graphic(0x1F03), Layer(0x16)),
        (Graphic(0x0F49), openshard_state::weapon::LAYER_TWO_HANDED),
    ] {
        assert!(
            equipment
                .iter()
                .any(|item| item.graphic == graphic && item.layer == layer),
            "the revived player wears 0x{:04X} on layer {}",
            graphic.0,
            layer.0,
        );
    }
    assert!(
        equipment.iter().any(|item| item.serial == robe_serial)
            && equipment.iter().any(|item| item.serial == axe_serial),
        "resurrection returns the exact items that were worn at death"
    );
    assert!(
        !world
            .registry()
            .query::<Corpse>()
            .any(|(_, corpse)| corpse.player == Some(serial)),
        "the reclaimed player corpse is gone"
    );
    assert_eq!(
        openshard_combat::weapons::equipped_weapon(&world.state, player_entity).map(|weapon| weapon.graphic),
        Some(Graphic(0x0F49)),
        "the resurrection axe is the active weapon"
    );
    assert!(
        world
            .registry()
            .get::<Combat>(player_entity)
            .is_some_and(|combat| combat.is_at_peace()),
        "the player's session combat row survived death at peace"
    );
    let packets = packets_for(&mut world, player);
    assert!(
        packets.iter().any(|p| p.as_slice() == [0x2C, 0x02]),
        "the client is told it is alive again (0x2C 0x02)"
    );

    // Exercise the two requests that used to become no-ops after resurrection,
    // then follow the freshly-armed swing through its visible lead-in and
    // impact. This proves both the session-long combat row and the exact weapon
    // animation survived the transition.
    let target = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let target_entity = entity(&world, target);
    engage(&mut world, player, target, now);
    let opening = packets_for(&mut world, player);
    assert!(
        opening
            .iter()
            .any(|packet| packet[0] == 0xE2 && packet[7..9] == [0x00, 0x07]),
        "the resurrection axe visibly begins its two-handed slash immediately"
    );
    let combat = world
        .state
        .registry
        .get::<Combat>(player_entity)
        .expect("combat is a player-session invariant");
    assert!(combat.warmode(), "war mode is recorded after resurrection");
    assert_eq!(
        combat.target(),
        Some(target),
        "the target is recorded after resurrection"
    );
    let impact = combat.next_swing().expect("an aimed combat row has an impact");
    let timing_at = opening
        .iter()
        .position(|packet| packet.len() == 13 && packet[0] == 0xBF && packet[3..5] == [0xE0, 0x0B])
        .expect("the server sends the axe wind-up duration before impact");
    let animation_at = opening
        .iter()
        .position(|packet| packet[0] == 0xE2 && packet[7..9] == [0x00, 0x07])
        .expect("the paired axe animation");
    assert_eq!(
        animation_at,
        timing_at + 1,
        "the timing is immediately followed by the action it stretches"
    );
    while world.state.ticks < impact {
        world.tick(now);
    }
    assert!(
        world
            .registry()
            .get::<Hitpoints>(target_entity)
            .is_some_and(|hits| hits.current < 50),
        "the resurrected player lands a blow"
    );
    let repeat = packets_for(&mut world, player);
    assert_eq!(
        repeat.iter().filter(|packet| packet[0] == 0xE2).count(),
        1,
        "impact starts exactly one next swing instead of replaying the completed one"
    );
    assert!(
        repeat
            .iter()
            .any(|packet| packet[0] == 0xBF && packet[3..5] == [0xE0, 0x0B]),
        "the repeated swing receives its own authoritative duration"
    );
}

#[test]
fn ghosts_cannot_be_selected_or_swung_at() {
    let now = Instant::now();
    let mut world = world();
    let attacker = enter(&mut world, now);
    let ghost = enter(&mut world, now);
    let attacker_entity = world.state.players[&attacker];
    let ghost_entity = world.state.players[&ghost];
    let ghost_serial = serial_of(&world, ghost);
    let living_body = *world
        .registry()
        .get::<Body>(ghost_entity)
        .expect("a body to become a ghost from");
    world
        .state
        .registry
        .insert(ghost_entity, Ghost { body: living_body });
    world.state.registry.insert(
        ghost_entity,
        Hitpoints {
            current: 0,
            max:     100,
        },
    );
    let _ = packets_for(&mut world, attacker);

    // A fresh player request clears the target instead of accepting a ghost's
    // still-valid mobile serial.
    world.queue(Command::Attack {
        connection: attacker,
        target:     Some(ghost_serial),
    });
    world.tick(now);
    assert_eq!(
        world
            .registry()
            .get::<Combat>(attacker_entity)
            .and_then(|combat| combat.target()),
        None,
        "a ghost cannot become an attack target"
    );

    // The same is true of a target chosen before its victim died: no extra
    // attack animation may be sent while the next AI beat is pending.
    world.state.registry.insert(
        attacker_entity,
        Combat::creature_engaged(ghost_serial, openshard_state::WorldTick::ZERO),
    );
    world.tick(now);
    assert_eq!(
        world
            .registry()
            .get::<Combat>(attacker_entity)
            .and_then(|combat| combat.target()),
        None,
        "a ghost clears a stale combat target before swinging"
    );
}

#[test]
fn a_mob_immediately_forgets_a_player_who_becomes_a_ghost() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    let player_serial = serial_of(&world, player);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    world.state.registry.insert(
        mob_entity,
        Combat::creature_engaged(player_serial, openshard_state::WorldTick::ZERO),
    );

    world.enter_ghost_state(player_entity, player_serial, true);

    assert_eq!(
        world
            .registry()
            .get::<Combat>(mob_entity)
            .and_then(|combat| combat.target()),
        None,
        "a mob does not retain a dead player as its quarry"
    );
}

#[test]
fn a_mob_cannot_see_or_reacquire_a_ghost() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    let player_serial = serial_of(&world, player);
    let creature = spawn_brained(&mut world, 0x00D1, Point::new(START.x, START.y + 4, 0), 8, now);

    // `enter_ghost_state` deliberately leaves the restored hit-point value
    // alone; visibility must therefore reject the Ghost marker itself, rather
    // than merely relying on a zero-hit-point death.
    world.enter_ghost_state(player_entity, player_serial, true);
    assert!(
        world
            .registry()
            .get::<Hitpoints>(player_entity)
            .is_some_and(|hits| hits.current > 0),
        "the visibility assertion is independent of a dead player's hit points"
    );
    assert!(
        !world.state.can_see_mobile(creature, player_entity),
        "the creature cannot see the ghost"
    );

    ai::think_one(&mut world.state, creature);
    assert_eq!(
        world
            .registry()
            .get::<Combat>(creature)
            .and_then(|combat| combat.target()),
        None,
        "the creature does not acquire the invisible ghost as prey"
    );
}

fn spawn_healer(world: &mut World, at: Point, now: Instant) -> (EntityId, Serial) {
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        100,
        notoriety:   Notoriety::from_bits(1),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    at,
        facet:       Facet(0),
        name:        None,
        title:       Some("the healer".to_owned()),
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      true,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let healer = world
        .registry()
        .query::<openshard_state::components::Healer>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a spawned healer");
    let serial = world.registry().serial_of(healer).unwrap();
    (healer, serial)
}

/// A `Command::GumpResponse` naming the healer confirm and a chosen button —
/// the shape `admin_response` uses for the admin gump.
fn healer_response(connection: ConnectionId, player: Serial, button: u32) -> Command {
    Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial:       openshard_protocol::gump::RawGumpKey(player.raw()),
            gump_id:      openshard_protocol::gump::RawGumpId(super::healer::HEALER_GUMP.0),
            button:       openshard_protocol::gump::RawButtonId(button),
            switches:     Vec::new(),
            text_entries: Vec::new(),
        },
    }
}

#[test]
fn a_ghost_double_clicking_a_healer_is_offered_a_free_resurrection() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    let (healer, healer_serial) = spawn_healer(&mut world, Point::new(START.x, START.y - 1, 0), now);

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);
    assert!(world.state.registry.has::<Ghost>(entity), "dead first");
    let _ = packets_for(&mut world, player);

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(healer_serial.raw())),
    });
    world.tick(now);

    assert_eq!(
        world.state.row_of(entity).and_then(|row| row.healer_gump),
        Some(healer),
        "the confirm is remembered against this healer, so the reply can be checked against it"
    );
    let packets = packets_for(&mut world, player);
    assert!(
        packets.iter().any(|p| p[0] == 0xB0),
        "the confirm gump is drawn (0xB0)"
    );

    // Answer "yes" (CONTINUE) — full hit points, unlike a spell's or a
    // bandage's tenth.
    world.queue(healer_response(player, serial, 1));
    world.tick(now);

    assert!(!world.state.registry.has::<Ghost>(entity), "alive again");
    let hits = world.registry().get::<Hitpoints>(entity).unwrap();
    assert_eq!(
        hits.current, hits.max,
        "a healer's free resurrection gives full hit points"
    );
}

#[test]
fn cancelling_the_healer_gump_leaves_the_ghost_dead() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    let (_, healer_serial) = spawn_healer(&mut world, Point::new(START.x, START.y - 1, 0), now);

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(healer_serial.raw())),
    });
    world.tick(now);

    // CANCEL — button 0, the close box's own id.
    world.queue(healer_response(player, serial, 0));
    world.tick(now);

    assert!(
        world.state.registry.has::<Ghost>(entity),
        "still a ghost — cancelling asked for nothing"
    );
    assert_eq!(
        world.state.row_of(entity).and_then(|row| row.healer_gump),
        None,
        "the answered offer is forgotten either way"
    );
}

#[test]
fn double_clicking_a_healer_out_of_reach_says_so_instead_of_nothing() {
    // A double-click that reached a real healer and still failed should not
    // read the same as a click on empty ground or the wrong NPC — see
    // `click_healer`'s doc comment.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    let (_, healer_serial) = spawn_healer(&mut world, Point::new(START.x, START.y - 5, 0), now);

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(healer_serial.raw())),
    });
    world.tick(now);

    assert_eq!(
        world.state.row_of(entity).and_then(|row| row.healer_gump),
        None,
        "too far — no confirm was opened"
    );
    let packets = packets_for(&mut world, player);
    assert!(
        packets.iter().any(|p| p[0] == 0x1C),
        "told why, rather than left to wonder if the click even landed"
    );
}

#[test]
fn walking_near_a_healer_offers_resurrection_with_no_click_at_all() {
    // ServUO's `BaseHealer.OnMovement`: the healer notices a ghost arriving,
    // no double-click needed. And a ghost that walks back out of reach is not
    // left with a stale, unanswerable window — the offer clears, so
    // approaching again asks afresh.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    let (healer, _) = spawn_healer(&mut world, Point::new(START.x, START.y - 3, 0), now);

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);
    assert!(world.state.registry.has::<Ghost>(entity));
    assert_eq!(
        world.state.row_of(entity).and_then(|row| row.healer_gump),
        None,
        "three tiles off — too far to be noticed yet"
    );

    // Walk north, toward the healer, one step per tick and well spaced in
    // time so the pace budget never refuses a step.
    let mut step_time = now;
    for sequence in 0..2u8 {
        step_time += std::time::Duration::from_millis(300);
        world.queue(Command::Walk {
            connection: player,
            request:    walk(sequence, Direction::North),
        });
        world.tick(step_time);
    }
    assert_eq!(
        world.state.row_of(entity).and_then(|row| row.healer_gump),
        Some(healer),
        "coming within reach offers a resurrection with no double-click"
    );

    // And walk back out again — four steps south is well past the range
    // whichever tile the offer first fired on.
    for sequence in 2..6u8 {
        step_time += std::time::Duration::from_millis(300);
        world.queue(Command::Walk {
            connection: player,
            request:    walk(sequence, Direction::South),
        });
        world.tick(step_time);
    }
    assert_eq!(
        world.state.row_of(entity).and_then(|row| row.healer_gump),
        None,
        "walking back out of range clears the pending offer"
    );
}

#[test]
fn a_ghost_is_hidden_from_the_living() {
    // The living cannot see the dead: when a player dies, every living watcher is
    // told to forget it (0x1D) and it drops off their screen.
    let now = Instant::now();
    let mut world = world();
    let watcher = enter_as(&mut world, ConnectionId::from_raw(1), now);
    let dying = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let dying_serial = serial_of(&world, dying);
    let dying_entity = world.state.players[&dying];
    let watcher_entity = world.state.players[&watcher];

    assert!(
        world
            .state
            .seen
            .get(&watcher_entity)
            .is_some_and(|s| s.contains(&dying_entity)),
        "the living watcher sees the living player first"
    );
    let _ = packets_for(&mut world, watcher);

    world.queue(Command::Damage {
        serial:      dying_serial,
        amount:      500,
        damage_type: 0,
        by:          None,
    });
    world.tick(now);

    assert!(
        !world
            .state
            .seen
            .get(&watcher_entity)
            .is_some_and(|s| s.contains(&dying_entity)),
        "the living watcher no longer sees the ghost"
    );
    let packets = packets_for(&mut world, watcher);
    assert!(
        packets.iter().any(|p| p[0] == 0x1D && mentions(p, dying_serial)),
        "and was told to remove it (0x1D)"
    );
}

/// Put a player in war mode, aimed at `target`, in one tick.
/// Bring an already-committed action's impact forward to this tick and let the
/// remaining two passes run over it.
///
/// The passes are the ordinary ones; only the impact moves. Waiting out a whole
/// swing interval in ticks would make a test about the interval rather than
/// about the blow, and would let stamina and hit regeneration drift the numbers
/// such a test asserts on.
fn land_the_committed_swing(world: &mut World, attacker: EntityId) {
    let impact = world.state.ticks;
    let action = *world
        .state
        .registry
        .get::<CombatAction>(attacker)
        .expect("a fighter with a target in reach commits a swing");
    world.state.registry.insert(
        attacker,
        CombatAction {
            phase: Phase::Releasing { impact },
            ..action
        },
    );
    combat::sustain_actions(&mut world.state);
    combat::resolve_actions(&mut world.state);
}

fn engage(world: &mut World, player: ConnectionId, target: Serial, now: Instant) {
    world.queue(Command::WarMode {
        connection: player,
        war:        true,
    });
    world.queue(Command::Attack {
        connection: player,
        target:     Some(target),
    });
    world.tick(now);
}

#[test]
fn war_mode_and_attack_are_confirmed_to_the_client() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::WarMode {
        connection: player,
        war:        true,
    });
    world.queue(Command::Attack {
        connection: player,
        target:     Some(mob),
    });
    world.tick(now);

    let packets = packets_for(&mut world, player);
    assert!(
        packets.iter().any(|p| p == &[0x72, 0x01, 0x00, 0x32, 0x00]),
        "war mode is confirmed"
    );
    assert!(
        packets.iter().any(|p| p[0] == 0xAA && mentions(p, mob)),
        "and the target is set"
    );

    world.queue(Command::WarMode {
        connection: player,
        war:        false,
    });
    world.tick(now);
    let packets = packets_for(&mut world, player);
    assert!(
        packets.iter().any(|p| p == &[0x72, 0x00, 0x00, 0x32, 0x00]),
        "the settled peace stance is confirmed"
    );
    assert!(
        packets.iter().any(|p| p == &[0xAA, 0, 0, 0, 0]),
        "and the target destroyed by that transition is cleared too"
    );
}

#[test]
fn a_player_in_war_mode_retaliates_when_struck_without_a_target() {
    let now = Instant::now();
    let mut world = world();
    let defender = enter(&mut world, now);
    let attacker = enter(&mut world, now);
    let defender_entity = world.state.players[&defender];
    let defender_serial = serial_of(&world, defender);
    let attacker_serial = serial_of(&world, attacker);
    teleport(&mut world, attacker, Point::new(START.x + 1, START.y, 0));
    world
        .state
        .registry
        .insert(defender_entity, Heading(Facing::walking(Direction::South)));

    world.queue(Command::WarMode {
        connection: defender,
        war:        true,
    });
    world.tick(now);
    let _ = packets_for(&mut world, defender);

    combat::damage(
        &mut world.state,
        defender_serial,
        1,
        openshard_state::DamageType::Physical,
        Some(attacker_serial),
    );
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<Combat>(defender_entity)
            .and_then(|combat| combat.target()),
        Some(attacker_serial),
        "a struck war-mode player aims back at the attacker"
    );
    assert_eq!(
        world.state.registry.get::<Heading>(defender_entity),
        Some(&Heading(Facing::walking(Direction::East))),
        "a struck war-mode player immediately faces the attacker"
    );
    let packets = packets_for(&mut world, defender);
    assert!(
        packets.iter().any(|packet| {
            packet.first() == Some(&0x20) && packet.get(17) == Some(&Direction::East.to_bits())
        }),
        "the defender receives a 0x20 update for its own visible turn"
    );
    assert!(
        !world.state.registry.has::<CriminalUntil>(defender_entity),
        "self-defence does not flag the defending player criminal"
    );
    assert!(
        packets
            .iter()
            .any(|packet| packet[0] == 0xAA && mentions(packet, attacker_serial)),
        "the client receives the new confirmed target for its marker"
    );
}

#[test]
fn a_player_in_war_mode_swings_at_an_adjacent_target() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);

    // One swing interval later, a blow has landed.
    for _ in 0..WRESTLING_SWING_TICKS {
        world.tick(now);
    }
    assert!(
        world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current < 50,
        "the target has taken damage"
    );
}

#[test]
fn a_landed_blow_plays_a_hit_sound() {
    // A silent fight reads as broken; every landed melee blow thwacks. The 0x54
    // reaches the attacker, who always has a client and hears their own swing.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    engage(&mut world, player, mob, now);
    let opening = packets_for(&mut world, player);
    assert!(
        opening.iter().any(|packet| packet[0] == 0xE2 && packet[9] == 0),
        "the visible swing starts as soon as the server schedules it"
    );
    assert!(
        opening.iter().any(|packet| {
            packet.len() == 13
                && packet[0] == 0xBF
                && packet[3..5] == [0xE0, 0x0B]
                && packet[9..13] == 1_000_u32.to_be_bytes()
        }),
        "the client is told that the wrestling wind-up occupies one second"
    );

    for _ in 0..WRESTLING_SWING_TICKS {
        world.tick(now);
    }
    let packets = packets_for(&mut world, player);
    let fists = combat::MELEE_HIT_SOUND.0.to_be_bytes();
    assert!(
        packets
            .iter()
            .any(|p| p[0] == 0x54 && p.len() >= 4 && p[2..4] == fists),
        "a human blow plays the fists thwack (0x0137), not a creature's sound"
    );
    assert_eq!(
        packets.iter().filter(|packet| packet[0] == 0xE2).count(),
        1,
        "impact starts exactly one next swing instead of replaying the one just finished"
    );
}

#[test]
fn a_dying_creature_plays_its_death_throe() {
    // Death is a throe, not a silent vanish: the killing blow sends a 0xE2 Die
    // animation (type 3) on the creature's serial to everyone watching, while it
    // is still on screen to play it. A one-hit-point mob dies on the first swing.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 1, now);
    engage(&mut world, player, mob, now);
    let _ = packets_for(&mut world, player);

    for _ in 0..WRESTLING_SWING_TICKS {
        world.tick(now);
    }
    assert!(
        world.registry().entity_of(mob).is_none(),
        "the creature died and was removed"
    );
    let mob_serial = mob.raw().to_be_bytes();
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| { p[0] == 0xE2 && p.len() >= 7 && p[1..5] == mob_serial && p[5..7] == [0x00, 0x03] }),
        "the creature played a 0xE2 Die animation (type 3) on its own serial"
    );
}

#[test]
fn a_creature_dies_with_its_own_voice() {
    // Per-creature sound, the point of the sound rule: an orc's death cry is its
    // own (ServUO BaseSoundID 0x45A, death = +4), not the human death gasp or the
    // fists thwack every mobile used to share.
    const ORC_BODY: Graphic = Graphic(0x0011);
    const ORC_DEATH_SOUND: u16 = 0x045A + 4;
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    world.queue(Command::SpawnMobile {
        body:        ORC_BODY,
        hue:         openshard_protocol::wire::Hue(0),
        hits:        1, // one blow fells it
        notoriety:   Notoriety::from_bits(5),
        damage:      5,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x, START.y, 0),
        facet:       Facet(0),
        name:        None,
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let orc = world
        .state
        .registry
        .query::<Body>()
        .find(|(entity, body)| body.id == ORC_BODY && !world.state.registry.has::<Client>(*entity))
        .and_then(|(entity, _)| world.state.registry.serial_of(entity))
        .expect("the orc spawned");
    engage(&mut world, player, orc, now);
    let _ = packets_for(&mut world, player);

    for _ in 0..WRESTLING_SWING_TICKS {
        world.tick(now);
    }
    assert!(world.registry().entity_of(orc).is_none(), "the orc died");
    let cry = ORC_DEATH_SOUND.to_be_bytes();
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| p[0] == 0x54 && p.len() >= 4 && p[2..4] == cry),
        "the orc died with its own death cry (0x45E), not a human's or a fist"
    );
}

#[test]
fn a_slain_creature_leaves_a_corpse_with_loot() {
    // Death is no longer a vanishing: a slain creature leaves a corpse at the
    // spot — item 0x2006 whose payload is its body — a container holding a little
    // gold, the engine's baseline beneath whatever `loot::table` adds.
    const CORPSE: u16 = 0x2006;
    const GOLD: u16 = 0x0EED;
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let spot = Point::new(START.x, START.y, 0);
    let mob = spawn_mobile_at(&mut world, spot, 8, now); // dies on the second swing
    engage(&mut world, player, mob, now);

    for _ in 0..(2 * WRESTLING_SWING_TICKS) {
        world.tick(now);
    }
    assert!(
        world.registry().entity_of(mob).is_none(),
        "the creature was reaped off the map"
    );
    let corpse = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(_, g)| g.id == openshard_protocol::wire::Graphic(CORPSE))
        .map(|(entity, _)| entity)
        .expect("a corpse was laid where it fell");
    assert_eq!(
        world.registry().get::<Position>(corpse).unwrap().0,
        spot,
        "the corpse lies on the death tile"
    );
    assert!(
        world.registry().has::<Container>(corpse),
        "the corpse is a container to be looted"
    );
    assert_eq!(
        world.registry().get::<CorpseBody>(corpse).unwrap().body,
        Graphic(0x0190),
        "the corpse body is a human male"
    );
    assert!(
        !world.registry().has::<Amount>(corpse),
        "the corpse body is not an item stack"
    );
    let corpse_serial = world.registry().serial_of(corpse).unwrap();
    let gold = world
        .state
        .registry
        .query::<Contained>()
        .filter(|(_, c)| c.container == corpse_serial)
        .any(|(entity, _)| {
            world
                .state
                .registry
                .get::<Drawn>(entity)
                .is_some_and(|g| g.id == openshard_protocol::wire::Graphic(GOLD))
        });
    assert!(gold, "the corpse holds a gold pile");
}

#[test]
fn a_death_tells_the_watchers_which_corpse_the_body_became() {
    // `0xAF`, and the only thing on the wire that pairs a fall with the body
    // lying there. Without it a client matches the two by tile, and two of the
    // same creature dying together swap their falls.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let spot = Point::new(START.x + 2, START.y, 0);
    let mob = spawn_mobile_at(&mut world, spot, 8, now);
    let _ = packets_for(&mut world, player);
    world.queue(Command::Damage {
        serial:      mob,
        amount:      500,
        damage_type: 0,
        by:          None,
    });
    world.tick(now);

    let corpse = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(_, g)| g.id == openshard_protocol::wire::Graphic(0x2006))
        .map(|(entity, _)| entity)
        .expect("a corpse was laid");
    let corpse_serial = world.registry().serial_of(corpse).unwrap();

    let death = packets_for(&mut world, player)
        .into_iter()
        .find(|p| p[0] == 0xAF)
        .expect("the watching player was told the creature died");
    assert_eq!(death.len(), 13);
    assert_eq!(
        u32::from_be_bytes(death[1..5].try_into().unwrap()),
        mob.raw(),
        "the body that fell"
    );
    assert_eq!(
        u32::from_be_bytes(death[5..9].try_into().unwrap()),
        corpse_serial.raw(),
        "and the corpse it became"
    );
}

#[test]
fn a_dying_player_is_not_told_about_its_own_corpse() {
    // ServUO excludes the dying player's own connection from the `0xAF` loop:
    // that client is told by `0x2C` and has a ghost to watch, not a corpse to
    // pair a fall with.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    let _ = packets_for(&mut world, player);
    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);

    let packets = packets_for(&mut world, player);
    assert!(
        packets.iter().any(|p| p.as_slice() == [0x2C, 0x00]),
        "it is told it died"
    );
    assert!(
        !packets.iter().any(|p| p[0] == 0xAF),
        "and not told to watch its own body fall"
    );
}

#[test]
fn a_corpse_lies_the_way_its_body_was_facing() {
    // A body falls the way it was facing, and the client draws the death group
    // *for a direction*. Until the corpse carried one, every body on the shard
    // ended up lying southeast whichever way it had died facing — the death
    // animation played right and then the corpse spun as it settled.
    const CORPSE: u16 = 0x2006;
    let now = Instant::now();
    let mut world = world();
    let _player = enter(&mut world, now);
    let spot = Point::new(START.x + 2, START.y, 0);
    let mob = spawn_mobile_at(&mut world, spot, 8, now);
    let entity = world.registry().entity_of(mob).expect("the creature spawned");
    // Turned west and then killed outright, so the heading under test is the one
    // this test set rather than one a swing turned it to.
    world
        .state
        .registry
        .insert(entity, Heading(Facing::walking(Direction::West)));
    world.queue(Command::Damage {
        serial:      mob,
        amount:      500,
        damage_type: 0,
        by:          None,
    });
    world.tick(now);

    let corpse = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(_, g)| g.id == openshard_protocol::wire::Graphic(CORPSE))
        .map(|(entity, _)| entity)
        .expect("a corpse was laid where it fell");
    assert_eq!(
        world.registry().get::<CorpseBody>(corpse).unwrap().facing,
        Direction::West,
        "the corpse kept the heading its body died with"
    );
    assert_eq!(
        world.state.world_item(corpse).unwrap().payload,
        openshard_protocol::items::WorldItemPayload::Corpse {
            body:   Graphic(0x0190),
            facing: Direction::West,
        },
        "and the 0x1A that draws it says so"
    );
}

#[test]
fn the_shipped_loot_table_rolls_on_the_seeded_generator() {
    // The loot move in one test. `roll_shipped_loot` is what `die` calls with the
    // corpse it has just laid; here it is called with a container a test can hold,
    // because what is under test is the table and the roll, not the fight.
    //
    // The rng is the point. The Community Pack owned these tables and rolled them
    // with `Math.random`, writing itself an exemption from the engine's
    // replayable-tick guarantee. Two worlds on the same seed loot identically now.
    const SKELETON: u16 = 0x0032;

    let fill = || -> Vec<(u16, u16)> {
        let now = Instant::now();
        let mut world = world();
        let player = enter(&mut world, now);
        let serial = serial_of(&world, player);
        let backpack = world
            .state
            .registry
            .query::<Equipped>()
            .find(|(_, w)| w.mobile == serial && w.layer == items::BACKPACK_LAYER)
            .map(|(e, _)| world.registry().serial_of(e).unwrap())
            .expect("the player wears a backpack");

        world.roll_shipped_loot(backpack, Graphic(SKELETON));
        let mut held: Vec<(u16, u16)> = world
            .state
            .registry
            .query::<Contained>()
            .filter(|(_, c)| c.container == backpack)
            .filter_map(|(entity, _)| {
                let graphic = world.state.registry.get::<Drawn>(entity)?.id.0;
                let amount = world.state.registry.get::<Amount>(entity).map_or(1, |a| a.0);
                Some((graphic, amount))
            })
            .collect();
        held.sort_unstable();
        held
    };

    let first = fill();
    assert!(!first.is_empty(), "the table dropped nothing at all");
    let table = crate::loot::table(Graphic(SKELETON)).expect("0x0032 has a shipped table");
    for &(graphic, amount) in &first {
        let drop = table
            .iter()
            .find(|drop| drop.graphic.0 == graphic)
            .unwrap_or_else(|| panic!("dropped {graphic:#06x}, which the table does not list"));
        assert!(
            (drop.least..=drop.most).contains(&amount),
            "dropped {amount} of {graphic:#06x}, outside {}..{}",
            drop.least,
            drop.most
        );
    }
    // Gold has no chance on it, so it is the one drop that must always be there.
    assert!(
        first.iter().any(|&(graphic, _)| graphic == 0x0EED),
        "no gold: {first:?}"
    );
    assert_eq!(first, fill(), "two worlds on the same seed looted differently");
}

#[test]
fn a_body_with_no_shipped_table_is_left_to_the_baseline() {
    // Most creatures have no table, and that is not a hole: the engine's own gold
    // and the gear it wore are what they leave.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    let backpack = world
        .state
        .registry
        .query::<Equipped>()
        .find(|(_, w)| w.mobile == serial && w.layer == items::BACKPACK_LAYER)
        .map(|(e, _)| world.registry().serial_of(e).unwrap())
        .expect("the player wears a backpack");
    let before = world
        .state
        .registry
        .query::<Contained>()
        .filter(|(_, c)| c.container == backpack)
        .count();

    world.roll_shipped_loot(backpack, Graphic(0x0190));
    assert_eq!(
        world
            .state
            .registry
            .query::<Contained>()
            .filter(|(_, c)| c.container == backpack)
            .count(),
        before,
        "a human body dropped shipped loot"
    );
}

#[test]
fn a_slain_creature_fires_the_loot_hook() {
    // The seam the pack fills: a corpse laid emits `CorpseCreated` carrying the
    // corpse serial and the body, so a script can add the real per-creature loot
    // on top of the core's baseline gold.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let spot = Point::new(START.x, START.y, 0);
    let mob = spawn_mobile_at(&mut world, spot, 8, now);
    let mut corpses: Cursor<CorpseCreated> = world.bus().cursor();
    engage(&mut world, player, mob, now);

    // Read every tick, so the event is caught the frame it fires rather than
    // after the double-buffer has retired it.
    let mut fired: Vec<CorpseCreated> = Vec::new();
    for _ in 0..(2 * WRESTLING_SWING_TICKS) {
        world.tick(now);
        fired.extend(world.bus().read(&mut corpses).copied());
    }
    assert_eq!(fired.len(), 1, "one corpse, one hook");
    assert_eq!(
        fired[0].body,
        openshard_protocol::wire::Graphic(0x0190),
        "the hook carries the body"
    );
    let corpse_entity = world
        .registry()
        .entity_of(fired[0].corpse)
        .expect("the hook's serial names a live corpse");
    assert!(
        world.registry().has::<Container>(corpse_entity),
        "and that corpse is a container to fill"
    );
}

#[test]
fn add_loot_fills_a_container_and_ignores_a_stray_serial() {
    // The op behind the loot hook: `AddLoot` drops an item into a container by
    // serial — a stackable merges, a discrete piece is placed whole — and a
    // serial that is not a container adds nothing rather than a floating item.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    let serial_obj = serial;
    let backpack = world
        .state
        .registry
        .query::<Equipped>()
        .find(|(_, w)| w.mobile == serial_obj && w.layer == items::BACKPACK_LAYER)
        .map(|(e, _)| world.registry().serial_of(e).unwrap())
        .expect("the player wears a backpack");

    world.queue(Command::AddLoot {
        container: backpack,
        graphic:   openshard_protocol::wire::Graphic(0x0EED), // gold
        hue:       openshard_protocol::wire::Hue(0),
        amount:    50,
        stackable: true,
    });
    world.queue(Command::AddLoot {
        container: backpack,
        graphic:   openshard_protocol::wire::Graphic(0x0F5E), // a broadsword
        hue:       openshard_protocol::wire::Hue(0),
        amount:    1,
        stackable: false,
    });
    // A stray serial: addressable, but nothing exists at it, so nothing is placed.
    world.queue(Command::AddLoot {
        container: Serial::new(0x4EAD_BEEF).unwrap(),
        graphic:   openshard_protocol::wire::Graphic(0x0EED),
        hue:       openshard_protocol::wire::Hue(0),
        amount:    999,
        stackable: true,
    });
    world.tick(now);

    let in_pack: Vec<Graphic> = world
        .state
        .registry
        .query::<Contained>()
        .filter(|(_, c)| c.container == backpack)
        .filter_map(|(e, _)| world.registry().get::<Drawn>(e).map(|g| g.id))
        .collect();
    assert!(in_pack.contains(&Graphic(0x0EED)), "the gold landed");
    assert!(in_pack.contains(&Graphic(0x0F5E)), "and the sword");
    // The stray-serial gold never appeared anywhere.
    let gold_piles = world
        .state
        .registry
        .query::<Drawn>()
        .filter(|(_, g)| g.id == openshard_protocol::wire::Graphic(0x0EED))
        .count();
    assert_eq!(gold_piles, 1, "the stray-serial loot was dropped, not floated");
}

#[test]
fn a_decaying_corpse_takes_its_loot_with_it() {
    // A corpse rots away with whatever was never lifted — no gold left orphaned,
    // pointing at a container that is gone.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 8, now);
    engage(&mut world, player, mob, now);
    for _ in 0..(2 * WRESTLING_SWING_TICKS) {
        world.tick(now);
    }
    let corpse = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(_, g)| g.id == openshard_protocol::wire::Graphic(0x2006))
        .map(|(entity, _)| entity)
        .expect("a corpse");
    let corpse_serial = world.registry().serial_of(corpse).unwrap();

    // Bring its decay clock forward to now and let the tick reap it.
    let tick = world.state.ticks;
    world
        .state
        .registry
        .insert(corpse, openshard_state::components::Decays { at_tick: tick });
    world.tick(now);

    assert!(!world.state.registry.contains(corpse), "the corpse rotted away");
    assert_eq!(
        world
            .state
            .registry
            .query::<Contained>()
            .filter(|(_, c)| c.container == corpse_serial)
            .count(),
        0,
        "and took its loot with it — nothing orphaned points at the gone corpse"
    );
}

#[test]
fn cleanup_disabled_does_not_start_a_corpse_decay_clock() {
    let now = Instant::now();
    let mut world = World::new(START).with_gameplay(Gameplay {
        decay_ticks: 0,
        ..Gameplay::default()
    });
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 8, now);
    engage(&mut world, player, mob, now);
    for _ in 0..(2 * WRESTLING_SWING_TICKS) {
        world.tick(now);
    }
    let corpse = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(_, g)| g.id == openshard_protocol::wire::Graphic(0x2006))
        .map(|(entity, _)| entity)
        .expect("a corpse");

    assert!(
        !world.state.registry.has::<Decays>(corpse),
        "a corpse remains when map cleanup is disabled"
    );
}

#[test]
fn no_swing_without_war_mode() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);

    // Aim, but stay at peace.
    world.queue(Command::Attack {
        connection: player,
        target:     Some(mob),
    });
    world.tick(now);
    for _ in 0..(WRESTLING_SWING_TICKS + 1) {
        world.tick(now);
    }
    assert_eq!(
        world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current,
        50,
        "a mobile at peace does not swing"
    );
}

#[test]
fn no_swing_out_of_reach() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    // Well outside melee range, but on screen.
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 5, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);
    for _ in 0..(WRESTLING_SWING_TICKS + 1) {
        world.tick(now);
    }
    assert_eq!(
        world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current,
        50,
        "a swing out of reach lands nothing"
    );
}

/// The outcome and reason bytes of the first "this action ended" the shard
/// broadcast, if it said one ended at all.
fn action_end(packets: &[Vec<u8>]) -> Option<(u8, u8)> {
    packets.iter().find_map(|packet| {
        (packet.first() == Some(&0xBF)
            && packet.get(3..5) == Some(&CombatActionEnded::SUBCOMMAND.to_be_bytes()[..]))
        .then(|| (packet[9], packet[10]))
    })
}

/// Every "cannot begin" and "can begin again" in the batch, in order.
///
/// All of them and not the first: the claim these tests make is about the
/// *edges* — that a refusal is said once and lifted once — and a helper that
/// returned only the first could not tell one packet from forty identical ones.
/// Both of these read one *actor's* packets out of the batch, and that is not
/// fussiness: every fighter on screen broadcasts these, and a creature walking
/// towards its quarry is held up by reach for as long as it is walking. A helper
/// that took the batch whole would be asserting on whoever spoke first.
fn action_balks(packets: &[Vec<u8>], actor: Serial) -> Vec<u8> {
    subcommand_bytes(packets, CombatActionBalked::SUBCOMMAND, actor)
}

/// Every stage transition this actor announced, in order.
fn action_stages(packets: &[Vec<u8>], actor: Serial) -> Vec<u8> {
    subcommand_bytes(packets, CombatActionStage::SUBCOMMAND, actor)
}

/// The trailing byte of every `0xBF` of this subcommand that names `actor` —
/// the shape both packets share: id, length, subcommand, actor, one byte.
fn subcommand_bytes(packets: &[Vec<u8>], subcommand: u16, actor: Serial) -> Vec<u8> {
    packets
        .iter()
        .filter(|packet| {
            packet.first() == Some(&0xBF)
                && packet.get(3..5) == Some(&subcommand.to_be_bytes()[..])
                && packet.get(5..9) == Some(&actor.raw().to_be_bytes()[..])
        })
        .map(|packet| packet[9])
        .collect()
}

/// The kind, phase and interval of the first "this action began" it broadcast.
fn action_phase(packets: &[Vec<u8>]) -> Option<(u8, u8, u32)> {
    packets.iter().find_map(|packet| {
        (packet.first() == Some(&0xBF)
            && packet.get(3..5) == Some(&CombatActionPhase::SUBCOMMAND.to_be_bytes()[..]))
        .then(|| {
            (
                packet[13],
                packet[14],
                u32::from_be_bytes([packet[15], packet[16], packet[17], packet[18]]),
            )
        })
    })
}

/// The last phase transition in a batch. A completed draw may announce Armed
/// and immediately Releasing in the same tick when sight is already open.
fn last_action_phase(packets: &[Vec<u8>]) -> Option<(u8, u8, u32)> {
    packets.iter().rev().find_map(|packet| {
        (packet.first() == Some(&0xBF)
            && packet.get(3..5) == Some(&CombatActionPhase::SUBCOMMAND.to_be_bytes()[..]))
        .then(|| {
            (
                packet[13],
                packet[14],
                u32::from_be_bytes([packet[15], packet[16], packet[17], packet[18]]),
            )
        })
    })
}

/// The commit says what is being done, to whom, and how long it has left — the
/// half of the wire that used to be a bare duration with no subject.
#[test]
fn a_committed_swing_announces_its_phase_and_the_time_to_the_impact() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let _ = packets_for(&mut world, player);
    engage(&mut world, player, mob, now);

    assert_eq!(
        action_phase(&packets_for(&mut world, player)),
        Some((0, 1, 1_000)),
        "a swing, released, landing one wrestling interval from now"
    );
}

/// The defect the whole model was built for. A telegraph that was cancelled had
/// no way to say so, so the watcher ran it out over an empty tile.
#[test]
fn a_target_that_dies_mid_swing_ends_its_attackers_action_with_a_reason() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);
    let _ = packets_for(&mut world, player);

    // Killed by something that is not this swing, a full interval before it
    // would have landed.
    world.state.registry.insert(
        mob_entity,
        Hitpoints {
            current: 0,
            max:     50,
        },
    );
    world.tick(now);

    assert_eq!(
        action_end(&packets_for(&mut world, player)),
        Some((
            CombatActionOutcome::Interrupted(InterruptReason::TargetGone).to_bits(),
            InterruptReason::TargetGone.to_bits()
        )),
        "the stroke stops on the spot, and the watcher is told why"
    );
}

/// The other half of the same rule: what the fighter committed to is the reach
/// it is held to, and losing it is an outcome with a name rather than a silent
/// `continue` at the impact.
#[test]
fn a_swing_whose_target_leaves_the_committed_reach_says_so() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);
    let _ = packets_for(&mut world, player);

    world
        .state
        .teleport(mob_entity, Point::new(START.x + 5, START.y, 0));
    world.tick(now);

    assert_eq!(
        action_end(&packets_for(&mut world, player)),
        Some((
            CombatActionOutcome::Interrupted(InterruptReason::OutOfReach).to_bits(),
            InterruptReason::OutOfReach.to_bits()
        )),
        "a swing that loses its reach ends, and names the reason"
    );
}

/// Nothing arms yet — that is the last phase of `docs/combat_actions.md` — but
/// the endurance is not decoration: an arm that is never released has to give
/// out, or a couched lance becomes a permanent property of a rider. Constructed
/// by hand here for exactly that reason.
#[test]
fn an_armed_action_that_is_never_released_expires() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    engage(&mut world, player, mob, now);
    let _ = packets_for(&mut world, player);

    let armed_at = world.state.ticks;
    world.state.registry.insert(
        player_entity,
        CombatAction {
            target:      mob,
            kind:        ActionKind::Swing {
                reach: openshard_combat::MELEE_REACH,
            },
            phase:       Phase::Armed {
                watch:      Watch::TargetInSight,
                expires_at: armed_at,
            },
            started_at:  armed_at,
            accuracy:    0,
            applied:     ConditionSet::EMPTY,
            telegraphed: true,
            stage:       ActionStage::FIRST,
        },
    );
    world.tick(now);

    assert_eq!(
        action_end(&packets_for(&mut world, player)),
        Some((CombatActionOutcome::Expired.to_bits(), 0)),
        "an arm nothing released gives out, and says that rather than a reason"
    );
}

#[test]
fn a_prepared_blow_releases_after_its_short_contact_interval() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    world.state.registry.remove::<Skills>(player_entity);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 5, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);
    for _ in 0..=WRESTLING_SWING_TICKS {
        world.tick(now);
    }
    let _ = packets_for(&mut world, player);

    world
        .state
        .teleport(mob_entity, Point::new(START.x + 1, START.y, 0));
    world.tick(now);
    let lead = openshard_combat::swing_speed(&world.state, player_entity);
    assert_eq!(lead, WRESTLING_SWING_TICKS);
    let release = lead / 5;
    assert_eq!(world.registry().get::<Hitpoints>(mob_entity).unwrap().current, 50);
    assert!(
        packets_for(&mut world, player).iter().any(|packet| {
            packet[0] == 0xBF
                && packet.len() == 13
                && packet[9..13]
                    == u32::try_from(release * 1_000 / TICKS_PER_SECOND)
                        .unwrap()
                        .to_be_bytes()
        }),
        "contact releases the already-prepared blow instead of landing it invisibly"
    );

    for _ in 1..release {
        world.tick(now);
    }
    assert_eq!(world.registry().get::<Hitpoints>(mob_entity).unwrap().current, 50);
    world.tick(now);
    assert!(
        world.registry().get::<Hitpoints>(mob_entity).unwrap().current < 50,
        "damage lands only when the visible action window ends"
    );
}

// `no_melee_swing_through_an_adjacent_wall` was here, and it was fiction.
//
// It put a mobile on the tile next to the player and a `sight_clear` that
// answered `false` from anywhere to anywhere, then asserted the blow did not
// land. No map can say that: `line_tiles` returns the tiles strictly between two
// points, and between neighbours there are none — a wall is a whole tile, so
// standing behind one puts you two tiles away and out of `MELEE_RANGE`.
//
// The gate it claimed to exercise (`combat/src/lib.rs`'s melee `sight_clear`,
// under a comment reading *"Adjacent tiles can still be separated by a closed
// door or wall"*) therefore cannot fire at all. The check is left in place —
// it becomes live the moment a sight line learns about height — and the finding
// is filed in `docs/world/research/terrain_seam.md`. What is deleted is a test that only
// ever proved its own double answered `false`.

#[test]
fn a_forced_critical_multiplies_a_landed_melee_blow_before_defences() {
    let now = Instant::now();
    let mut world = world();
    // A deterministic critical isolates its place in the formula: with no skills,
    // armour or resistance, fists are the base five and a 200% critical is ten.
    world.state.gameplay.critical_chance = 1000;
    world.state.gameplay.critical_damage_percent = 200;
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);
    for _ in 0..=WRESTLING_SWING_TICKS {
        world.tick(now);
    }

    assert_eq!(
        world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current,
        40,
        "a guaranteed 200% critical turns a five-damage fist blow into ten"
    );
}

#[test]
fn a_wielded_weapon_sets_the_swing_pace() {
    // A player derives their pace from the weapon in hand; bare hands stay
    // wrestling. Read directly, no fight needed — `swing_speed` is the seam.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(entity).unwrap();
    let scale = world.state.gameplay.speed_scale_factor;

    assert_eq!(
        combat::swing_speed(&world.state, entity),
        WRESTLING_SWING_TICKS,
        "bare-handed is wrestling pace"
    );

    // Longsword (old_speed 35) keeps its own pace rather than inheriting the
    // bare-hand base (50).
    let sword = items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x0F61),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();
    assert_eq!(
        combat::swing_speed(&world.state, entity),
        swing_ticks(100, 35, 1, scale),
        "a longsword swings by its own speed, not wrestling's"
    );

    // A katana (58) is faster than a mace (30): the table drives the ordering.
    world.state.registry.despawn(sword);
    let katana = items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x13FF),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();
    let katana_pace = combat::swing_speed(&world.state, entity);
    world.state.registry.despawn(katana);
    items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x0F5C),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();
    assert!(
        katana_pace < combat::swing_speed(&world.state, entity),
        "the katana swings sooner than the mace"
    );
}

#[test]
fn taking_the_weapon_off_reverts_to_wrestling() {
    // The read-site derivation means there is nothing to undo on unequip: the
    // weapon gone, the next read finds none and falls back on its own.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(entity).unwrap();

    let sword = items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x0F61),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();
    assert_ne!(combat::swing_speed(&world.state, entity), WRESTLING_SWING_TICKS);
    world.state.registry.despawn(sword);
    assert_eq!(
        combat::swing_speed(&world.state, entity),
        WRESTLING_SWING_TICKS,
        "the pace reverts with no other bookkeeping"
    );
}

#[test]
fn a_wielded_weapon_rolls_its_damage_within_range_and_replays() {
    // A longsword hits for old_min..=old_max (5..=33), rolled off the world's
    // seeded rng — so two identically-built worlds roll the same sequence.
    fn setup() -> (World, EntityId) {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let entity = world.state.players[&connection];
        let serial = world.state.registry.serial_of(entity).unwrap();
        items::equip_worn_item(
            &mut world.state,
            serial,
            openshard_protocol::wire::Graphic(0x0F61),
            openshard_protocol::wire::Hue(0),
            Layer(1),
        )
        .unwrap();
        (world, entity)
    }

    let (mut a, ea) = setup();
    let (mut b, eb) = setup();
    let mut seq_a = Vec::new();
    for _ in 0..64 {
        let blow = combat::melee_blow(&mut a.state, ea);
        assert!((5..=33).contains(&blow), "blow {blow} out of the weapon's range");
        seq_a.push(blow);
    }
    let seq_b: Vec<u16> = (0..64).map(|_| combat::melee_blow(&mut b.state, eb)).collect();
    assert_eq!(seq_a, seq_b, "the damage roll replays for a fixed seed");
}

#[test]
fn a_weapon_damage_affix_changes_its_instance_not_the_weapon_table() {
    // The same longsword graphic still reads 5..=33; this one physical sword
    // carries +10..+12. That is the distinction that lets loot be individual.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let wielder = world.state.players[&connection];
    let serial = world.state.registry.serial_of(wielder).unwrap();
    let sword = items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x0F61),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();
    let sword_serial = world.state.registry.serial_of(sword).unwrap();
    items::set_affixes(
        &mut world.state,
        sword_serial,
        vec![ItemAffix::DamageBonus {
            minimum: 10,
            maximum: 12,
        }],
    );

    for _ in 0..64 {
        let blow = combat::melee_blow(&mut world.state, wielder);
        assert!((15..=45).contains(&blow), "affixed blow {blow} out of range");
    }
}

#[test]
fn a_natural_blow_beats_a_wielded_weapon() {
    // A creature's `MeleeDamage` (its natural blow, or a script's pin) wins over
    // whatever it happens to hold — the override precedence combat already had.
    let now = Instant::now();
    let mut world = world();
    let spot = Point::new(START.x, START.y, 0);
    let mob = spawn_mobile_full(&mut world, spot, 50, 4, 7, 0, now);
    let mob_entity = entity(&world, mob);
    let serial = world.state.registry.serial_of(mob_entity).unwrap();
    items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x0F61),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();
    for _ in 0..20 {
        assert_eq!(
            combat::melee_blow(&mut world.state, mob_entity),
            7,
            "the natural blow ignores the sword in its hand"
        );
    }
}

#[test]
fn era_two_reads_the_aos_weapon_numbers() {
    // The same katana, an AoS shard: speed 46 (not 58) and damage 10..=14.
    let now = Instant::now();
    let mut world = world();
    world.state.gameplay.combat_era = CombatEra::new(2);
    let scale = world.state.gameplay.speed_scale_factor;
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(entity).unwrap();
    items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x13FF),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();

    assert_eq!(
        combat::swing_speed(&world.state, entity),
        swing_ticks(100, 46, 2, scale),
        "era 2 uses the katana's AoS speed"
    );
    for _ in 0..20 {
        let blow = combat::melee_blow(&mut world.state, entity);
        assert!((10..=14).contains(&blow), "AoS damage band");
    }
}

#[test]
fn a_skilled_swing_lands_and_trains_its_weapon_skill() {
    // A trained fighter's swing rolls to hit (and, whether it lands or whiffs,
    // trains the weapon skill by the attempt). Against a skill-less creature the
    // odds are high, so damage lands; and the swordsman's Swords creeps up.
    use combat::weapons::SWORDS_SKILL;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player_entity).unwrap();
    openshard_skills::set_skill(&mut world.state, serial, SWORDS_SKILL.id(), 300);
    items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x0F61),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();
    // A tough, skill-less dummy so the fight runs long enough to train.
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 1000, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, connection, mob, now);

    for _ in 0..(20 * WRESTLING_SWING_TICKS) {
        world.tick(now);
    }
    assert!(
        world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current < 1000,
        "a skilled swordsman's blows land on a skill-less dummy"
    );
    let swords = world
        .state
        .registry
        .get::<Skills>(player_entity)
        .unwrap()
        .get(SWORDS_SKILL);
    assert!(
        swords > 300,
        "the swings trained Swords past its start (got {swords})"
    );
}

#[test]
fn an_even_unskilled_duel_sometimes_misses() {
    // Two poorly-matched fighters (low attacker skill, a guarded defender) do not
    // land every blow: over a run of swings the attacker's own client hears both
    // the thwack of a hit and the whistle of a miss.
    use combat::weapons::{
        SWORDS_SKILL,
        WRESTLING_SKILL,
    };
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player_entity).unwrap();
    openshard_skills::set_skill(&mut world.state, serial, SWORDS_SKILL.id(), 200);
    items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x0F61),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 2000, now);
    openshard_skills::set_skill(&mut world.state, mob, WRESTLING_SKILL.id(), 1000);
    engage(&mut world, connection, mob, now);
    let _ = packets_for(&mut world, connection);

    for _ in 0..(30 * WRESTLING_SWING_TICKS) {
        world.tick(now);
    }
    let packets = packets_for(&mut world, connection);
    let hit = combat::MELEE_HIT_SOUND.0.to_be_bytes();
    // A longsword whiffs with its own class sound (ServUO's DefMissSound), not the
    // generic bare-hands swish.
    let miss = openshard_state::weapon::weapon_data(openshard_protocol::wire::Graphic(0x0F61))
        .unwrap()
        .miss_sound
        .0
        .to_be_bytes();
    let sound = |id: [u8; 2]| {
        packets
            .iter()
            .any(|p| p[0] == 0x54 && p.len() >= 4 && p[2..4] == id)
    };
    assert!(sound(hit), "some blows landed (a thwack)");
    assert!(sound(miss), "and some whiffed (the sword's swish)");
}

#[test]
fn tactics_scales_the_blow() {
    // The one difference between two otherwise-identical fights is the attacker's
    // Tactics; the same seed rolls the same base damage, so more Tactics must mean
    // more damage dealt. Swords is pinned at the cap so every blow lands and no
    // gain draw perturbs the shared rng sequence.
    use combat::weapons::{
        SWORDS_SKILL,
        TACTICS_SKILL,
    };
    fn total_dealt(tactics: u16) -> u16 {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let player_entity = world.state.players[&connection];
        let serial = world.state.registry.serial_of(player_entity).unwrap();
        openshard_skills::set_skill(&mut world.state, serial, SWORDS_SKILL.id(), 1000);
        openshard_skills::set_skill(&mut world.state, serial, TACTICS_SKILL.id(), tactics);
        items::equip_worn_item(
            &mut world.state,
            serial,
            openshard_protocol::wire::Graphic(0x0F61),
            openshard_protocol::wire::Hue(0),
            Layer(1),
        )
        .unwrap();
        let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 60000, now);
        let mob_entity = entity(&world, mob);
        engage(&mut world, connection, mob, now);
        for _ in 0..(10 * WRESTLING_SWING_TICKS) {
            world.tick(now);
        }
        60000 - world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current
    }
    assert!(
        total_dealt(1000) > total_dealt(0),
        "grandmaster Tactics hits harder than none"
    );
}

#[test]
fn lumberjacking_lends_an_axe_its_bite() {
    // Same axe, same seed: a lumberjack swings harder — but only with an axe. A
    // sword ignores Lumberjacking entirely.
    use combat::weapons::{
        LUMBERJACKING_SKILL,
        SWORDS_SKILL,
    };
    fn total_dealt(graphic: u16, lumber: u16) -> u16 {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let player_entity = world.state.players[&connection];
        let serial = world.state.registry.serial_of(player_entity).unwrap();
        openshard_skills::set_skill(&mut world.state, serial, SWORDS_SKILL.id(), 1000);
        openshard_skills::set_skill(&mut world.state, serial, LUMBERJACKING_SKILL.id(), lumber);
        items::equip_worn_item(
            &mut world.state,
            serial,
            openshard_protocol::wire::Graphic(graphic),
            openshard_protocol::wire::Hue(0),
            Layer(1),
        )
        .unwrap();
        let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 60000, now);
        let mob_entity = entity(&world, mob);
        engage(&mut world, connection, mob, now);
        for _ in 0..(10 * WRESTLING_SWING_TICKS) {
            world.tick(now);
        }
        60000 - world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current
    }
    // 0x0F49 is the Axe (is_axe); 0x0F61 the Longsword (not).
    assert!(
        total_dealt(0x0F49, 1000) > total_dealt(0x0F49, 0),
        "Lumberjacking sharpens an axe"
    );
    assert_eq!(
        total_dealt(0x0F61, 1000),
        total_dealt(0x0F61, 0),
        "but does nothing for a sword"
    );
}

#[test]
fn a_creature_can_be_given_combat_skills() {
    // The pack can hand a monster Wrestling and Tactics; the creature then carries
    // a Skills sheet — which is exactly what turns on its to-hit roll and damage
    // scaling, so a skilled monster fights the way a skilled player does.
    use combat::weapons::{
        TACTICS_SKILL,
        WRESTLING_SKILL,
    };
    let now = Instant::now();
    let mut world = world();
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(5),
        damage:      8,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x, START.y, 0),
        facet:       Facet(0),
        name:        None,
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      vec![(WRESTLING_SKILL, 700), (TACTICS_SKILL, 500)],
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let creature = world
        .state
        .registry
        .query::<Body>()
        .find(|(entity, _)| !world.state.registry.has::<Client>(*entity))
        .map(|(entity, _)| entity)
        .expect("a creature was spawned");
    let skills = world
        .state
        .registry
        .get::<Skills>(creature)
        .expect("the creature carries the skills it was spawned with");
    assert_eq!(skills.get(WRESTLING_SKILL), 700);
    assert_eq!(skills.get(TACTICS_SKILL), 500);
}

#[test]
fn a_bow_deals_its_own_damage_band() {
    // A ranged attacker with a bow rolls the bow's damage (9..=41), not the flat
    // bare-hands 5 — the weapon table drives a shot the way it drives a blow.
    use openshard_state::components::RangedAttack;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player_entity).unwrap();
    items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x13B2),
        openshard_protocol::wire::Hue(0),
        Layer(2),
    )
    .unwrap(); // bow
    world.state.registry.insert(
        player_entity,
        RangedAttack {
            range: RangedRange::new(6).expect("a bow has reach"),
            kind:  DamageType::Physical,
        },
    );
    // A target three tiles off: out of melee reach, inside bow range.
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 4000, now);
    engage(&mut world, connection, mob, now);
    let mut damaged: Cursor<MobileDamaged> = world.bus().cursor();

    // Read every tick: the bus is double-buffered, so events age out if left.
    let mut blows: Vec<u16> = Vec::new();
    for _ in 0..(12 * WRESTLING_SWING_TICKS) {
        world.tick(now);
        blows.extend(world.bus().read(&mut damaged).map(|d| d.amount));
    }
    assert!(!blows.is_empty(), "the archer's volleys landed");
    assert!(
        blows.iter().all(|&b| (9..=41).contains(&b)),
        "every arrow hits for the bow's band, not the flat default: {blows:?}"
    );
}

#[test]
fn a_wielded_bow_fights_at_range_with_no_ranged_attack_component() {
    // A player who merely equips a bow and has arrows in their pack fights at
    // range on that alone — no script-inserted `RangedAttack`. The commit
    // derives the reach and the round from the weapon table itself.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player_entity).unwrap();
    items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x13B2),
        openshard_protocol::wire::Hue(0),
        Layer(2),
    )
    .unwrap(); // bow
    assert!(
        items::give_to_backpack(&mut world.state, serial, Graphic(0x0F3F), Hue(0), 20, true),
        "the fresh backpack takes a pile of arrows"
    );
    // Three tiles off: out of melee reach, inside the bow's ten-tile reach.
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 4000, now);
    engage(&mut world, connection, mob, now);
    let mut damaged: Cursor<MobileDamaged> = world.bus().cursor();

    let mut blows: Vec<u16> = Vec::new();
    for _ in 0..(12 * WRESTLING_SWING_TICKS) {
        world.tick(now);
        blows.extend(world.bus().read(&mut damaged).map(|d| d.amount));
    }
    assert!(
        !blows.is_empty(),
        "the wielded bow fired without a RangedAttack component"
    );
    assert!(
        blows.iter().all(|&b| (9..=41).contains(&b)),
        "still the bow's own damage band: {blows:?}"
    );
    assert!(
        items::count_in_container(
            &world.state,
            items::backpack_of(&world.state, serial).unwrap(),
            Graphic(0x0F3F)
        ) < 20,
        "firing spent arrows out of the pack"
    );
}

#[test]
fn an_archer_with_no_arrows_cannot_fire() {
    // Same setup as above, minus the arrows: the shot must not fire, and the
    // shooter is told why instead of silently missing forever.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player_entity).unwrap();
    items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x13B2),
        openshard_protocol::wire::Hue(0),
        Layer(2),
    )
    .unwrap(); // bow, no arrows given
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 4000, now);
    engage(&mut world, connection, mob, now);
    let mut damaged: Cursor<MobileDamaged> = world.bus().cursor();
    let _ = packets_for(&mut world, connection); // drain the engage burst

    let mut blows: Vec<u16> = Vec::new();
    for _ in 0..(12 * WRESTLING_SWING_TICKS) {
        world.tick(now);
        blows.extend(world.bus().read(&mut damaged).map(|d| d.amount));
    }
    assert!(blows.is_empty(), "an empty quiver must not land a hit: {blows:?}");
    let refusal = packets_for(&mut world, connection);
    assert!(
        refusal
            .iter()
            .any(|p| p.first() == Some(&0x1C) && p.windows(6).any(|w| w == b"arrows")),
        "the shooter is told their quiver is empty"
    );
    assert!(
        action_phase(&refusal).is_none(),
        "and told at the nock: no bow was ever drawn, so there is no action to interrupt"
    );
}

/// `.sight` reports **both** halves of a refusal, and the reach is the half no
/// ray can carry.
///
/// `obstruction` asks `in_range` before it asks the sight line, so a clear look
/// across open ground is not a shot when the weapon does not carry that far.
/// This command answered only the ray's half, which read as permission.
#[test]
fn dot_sight_names_the_distance_and_the_reach_of_the_weapon_in_hand() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let actor = world.state.players[&connection];
    let far = Point::new(START.x + 14, START.y, 0);

    // Bare hands first: arm's length, and everything past the next tile is out
    // of reach however open the ground is.
    crate::gm::report_sight(&mut world.state, actor, far);
    let barehanded = packets_for(&mut world, connection);
    assert!(
        barehanded
            .iter()
            .any(|p| p.first() == Some(&0x1C) && p.windows(20).any(|w| w == b"14 tiles away, reach")),
        "the distance and the reach are not both in the report"
    );
    assert!(
        barehanded
            .iter()
            .any(|p| p.first() == Some(&0x1C) && p.windows(12).any(|w| w == b"out of reach")),
        "fourteen tiles from a fist read as a shot"
    );

    // And with a bow on: the same look, the same ground, a reach of ten — still
    // short of fourteen, which is the point of saying the number rather than a
    // verdict alone.
    arm_with_bow(&mut world, connection);
    crate::gm::report_sight(&mut world.state, actor, far);
    let armed = packets_for(&mut world, connection);
    assert!(
        armed
            .iter()
            .any(|p| p.first() == Some(&0x1C) && p.windows(8).any(|w| w == b"reach 10")),
        "the bow's own reach is not what the report read"
    );
}

/// A bow on the back and a quiver in the pack — what every archery scene below
/// starts from, and the fixture that has to agree with itself: the graphic worn
/// is the one the weapon table calls a bow, and the round given is the one that
/// bow nocks.
fn arm_with_bow(world: &mut World, connection: ConnectionId) {
    let entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(entity).unwrap();
    items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x13B2),
        openshard_protocol::wire::Hue(0),
        Layer(2),
    )
    .unwrap(); // bow
    assert!(items::give_to_backpack(
        &mut world.state,
        serial,
        Graphic(0x0F3F),
        Hue(0),
        20,
        true
    ));
}

/// Targeted bow overwatch: a cut sight line is a held shot, either participant
/// may move to open it, and opening it starts a real (non-zero) loose rather
/// than teleporting an arrow out on the discovery tick.
#[test]
fn a_drawn_bow_walks_out_of_cover_and_releases_on_sight() {
    let now = Instant::now();
    let mut world = world();
    stand_on(&mut world, a_wall_two_tiles_across());
    let connection = enter(&mut world, now);
    let archer = world.state.players[&connection];
    let archer_serial = world.state.registry.serial_of(archer).unwrap();
    arm_with_bow(&mut world, connection);
    let target = spawn_mobile_at(&mut world, Point::new(START.x + 2, START.y, 0), 50, now);
    let quiver = items::backpack_of(&world.state, archer_serial).unwrap();
    let _ = packets_for(&mut world, connection);

    engage(&mut world, connection, target, now);
    let held = packets_for(&mut world, connection);
    let arming_ticks = match world.state.registry.get::<CombatAction>(archer).map(|a| a.phase) {
        Some(Phase::Arming { ready_at, .. }) => ready_at.saturating_sub(world.state.ticks),
        other => panic!("the covered shot did not begin its draw: {other:?}"),
    };
    assert_eq!(
        action_phase(&held),
        Some((
            openshard_protocol::feedback::CombatActionKind::Shot.to_bits(),
            2,
            (arming_ticks * (1_000 / TICKS_PER_SECOND)) as u32,
        )),
        "the wall starts a mandatory draw instead of skipping straight to held"
    );
    assert_eq!(
        action_stages(&held, archer_serial),
        Vec::<u8>::new(),
        "the commit packet itself already implies the first preparation stage"
    );
    assert!(matches!(
        world.state.registry.get::<CombatAction>(archer).map(|a| a.phase),
        Some(Phase::Arming {
            watch: Watch::TargetInSight,
            ..
        })
    ));
    assert_eq!(
        items::count_in_container(&world.state, quiver, Graphic(0x0F3F)),
        20,
        "holding only reserves the idea of a round; it consumes no arrow"
    );

    // Move south until the line rounds the wall. The accepted walk is applied
    // before sustain, so the sight watch sees the same authoritative position
    // in this tick that the movement packet announced.
    let mut released_packets = Vec::new();
    let hold_ticks =
        Gameplay::ticks_from_ms(u64::try_from(openshard_movement::RUN_HOLD.as_millis()).unwrap());
    for sequence in 0..4 {
        world.queue(Command::Walk {
            connection,
            request: run(sequence, Direction::South),
        });
        world.tick(now);
        released_packets.extend(packets_for(&mut world, connection));
        // Respect the same crossing deadline the client does before asking for
        // another step; rejected movement would prove nothing about walking
        // while armed.
        for _ in 1..hold_ticks {
            world.tick(now);
            released_packets.extend(packets_for(&mut world, connection));
        }
    }
    let ready_at = match world.state.registry.get::<CombatAction>(archer).map(|a| a.phase) {
        Some(Phase::Arming { ready_at, .. }) => ready_at,
        other => panic!("opening sight during the draw released too early: {other:?}"),
    };
    while world.state.ticks < ready_at {
        world.tick(now);
        released_packets.extend(packets_for(&mut world, connection));
    }
    assert_ne!(
        world.state.registry.get::<Position>(archer).unwrap().0,
        Point::new(START.x, START.y, 0),
        "the archer walked while the bow was held"
    );
    let action = *world
        .state
        .registry
        .get::<CombatAction>(archer)
        .expect("walking out from the wall released the held shot");
    let Phase::Releasing { impact } = action.phase else {
        panic!("the opened sight line did not release overwatch: {action:?}");
    };
    let release_ticks = impact.saturating_sub(world.state.ticks);
    assert!(release_ticks > 0, "a watch never resolves on its trigger tick");
    assert_eq!(
        last_action_phase(&released_packets),
        Some((
            openshard_protocol::feedback::CombatActionKind::Shot.to_bits(),
            1,
            (release_ticks * (1_000 / TICKS_PER_SECOND)) as u32,
        )),
        "the client is told the loose began and how long it has"
    );

    for _ in 1..release_ticks {
        world.tick(now);
    }
    assert_eq!(
        items::count_in_container(&world.state, quiver, Graphic(0x0F3F)),
        20,
        "the arrow remains in the quiver throughout the loose"
    );
    world.tick(now);
    assert_eq!(
        items::count_in_container(&world.state, quiver, Graphic(0x0F3F)),
        19,
        "the arrow is spent only when the released shot actually flies"
    );
}

/// The watch belongs to the line between the two mobiles, not to the archer's
/// movement seam alone: the selected enemy stepping out is the same release.
#[test]
fn a_drawn_bow_releases_when_its_enemy_steps_out_of_cover() {
    let now = Instant::now();
    let mut world = world();
    stand_on(&mut world, a_wall_two_tiles_across());
    let connection = enter(&mut world, now);
    let archer = world.state.players[&connection];
    arm_with_bow(&mut world, connection);
    let target = spawn_mobile_at(&mut world, Point::new(START.x + 2, START.y, 0), 50, now);
    let target_entity = entity(&world, target);
    engage(&mut world, connection, target, now);
    let _ = packets_for(&mut world, connection);
    let ready_at = match world.state.registry.get::<CombatAction>(archer).map(|a| a.phase) {
        Some(Phase::Arming {
            watch: Watch::TargetInSight,
            ready_at,
            ..
        }) => ready_at,
        other => panic!("a covered shot did not begin its draw: {other:?}"),
    };
    while world.state.ticks < ready_at {
        world.tick(now);
    }
    assert!(matches!(
        world.state.registry.get::<CombatAction>(archer).map(|a| a.phase),
        Some(Phase::Armed {
            watch: Watch::TargetInSight,
            ..
        })
    ));
    let _ = packets_for(&mut world, connection);

    world
        .state
        .teleport(target_entity, Point::new(START.x, START.y + 2, 0));
    world.tick(now);

    assert!(matches!(
        world.state.registry.get::<CombatAction>(archer).map(|a| a.phase),
        Some(Phase::Releasing { impact }) if impact > world.state.ticks
    ));
    assert_eq!(
        action_phase(&packets_for(&mut world, connection)).map(|(_, phase, _)| phase),
        Some(1),
        "the target opening the line announces the same non-instant release"
    );
}

/// A shot committed during a crossing must not rewrite the facing that owns the
/// walk. The next request in that direction is therefore another step, not the
/// turn-on-the-spot that made shooting on the run visibly stumble.
#[test]
fn a_running_archer_keeps_the_stride_when_a_shot_commits() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let archer = world.state.players[&connection];
    arm_with_bow(&mut world, connection);
    let target = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 50, now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::Walk {
        connection,
        request: run(0, Direction::South),
    });
    world.tick(now);
    let first = world.state.registry.get::<Position>(archer).unwrap().0;
    assert_eq!(first, Point::new(START.x, START.y + 1, 0), "a real running step");

    // Commit halfway through the crossing rather than on the movement tick: the
    // detector is the step's real presentation lifetime, not a tick-local flag.
    let hold_ticks =
        Gameplay::ticks_from_ms(u64::try_from(openshard_movement::RUN_HOLD.as_millis()).unwrap());
    for _ in 0..(hold_ticks / 2) {
        world.tick(now + openshard_movement::RUN_HOLD / 2);
    }
    world.queue(Command::WarMode {
        connection,
        war: true,
    });
    world.queue(Command::Attack {
        connection,
        target: Some(target),
    });
    world.tick(now + openshard_movement::RUN_HOLD / 2);

    assert_eq!(
        world.state.registry.get::<Heading>(archer).unwrap().0.direction,
        Direction::South,
        "the nock did not turn the running body toward its target"
    );
    world.queue(Command::Walk {
        connection,
        request: run(1, Direction::South),
    });
    world.tick(now + openshard_movement::RUN_HOLD);
    assert_eq!(
        world.state.registry.get::<Position>(archer).unwrap().0,
        Point::new(START.x, START.y + 2, 0),
        "the following request continued the run instead of paying for a combat turn"
    );
}

/// The run bit is the last requested pace, not a movement state. Once its
/// crossing has ended, an archer is standing and should turn into the shot even
/// though that bit remains set in `Movement`.
#[test]
fn an_archer_turns_to_the_target_after_the_last_step_finishes() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let archer = world.state.players[&connection];
    arm_with_bow(&mut world, connection);
    let target_at = Point::new(START.x + 3, START.y, 0);
    let target = spawn_mobile_at(&mut world, target_at, 50, now);

    world.queue(Command::Walk {
        connection,
        request: run(0, Direction::South),
    });
    world.tick(now);
    let hold_ticks =
        Gameplay::ticks_from_ms(u64::try_from(openshard_movement::RUN_HOLD.as_millis()).unwrap());
    for _ in 1..hold_ticks {
        world.tick(now + openshard_movement::RUN_HOLD);
    }
    let from = world.state.registry.get::<Position>(archer).unwrap().0;
    let toward = openshard_movement::direction_toward(from, target_at).unwrap();
    assert!(
        world
            .state
            .registry
            .get::<Movement>(archer)
            .unwrap()
            .0
            .facing
            .running,
        "the stale run bit is present, so it cannot be the detector"
    );

    world.queue(Command::WarMode {
        connection,
        war: true,
    });
    world.queue(Command::Attack {
        connection,
        target: Some(target),
    });
    world.tick(now + openshard_movement::RUN_HOLD);

    assert_eq!(
        world.state.registry.get::<Heading>(archer).unwrap().0.direction,
        toward,
        "after the crossing the standing archer turns into the shot"
    );
}

/// The visible half of Ф2. A shot is a committed action like any other, so it
/// announces itself at the start of the interval it will take — the archer is a
/// body drawing a bow for the whole of it, not a statue that spits an arrow.
#[test]
fn a_drawn_bow_announces_a_shot_for_the_whole_interval() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 50, now);
    let _ = packets_for(&mut world, connection);
    // The bow's own pace, not wrestling's: the interval announced has to be the
    // one the shard will actually wait, or the drawing body and the arrow part
    // company.
    let draw = combat::swing_speed(&world.state, player_entity);
    engage(&mut world, connection, mob, now);

    assert_eq!(
        action_phase(&packets_for(&mut world, connection)),
        Some((
            openshard_protocol::feedback::CombatActionKind::Shot.to_bits(),
            1,
            (draw * (1_000 / openshard_state::TICKS_PER_SECOND)) as u32,
        )),
        "a shot, released, landing one bow interval from now"
    );
}

/// D3, from the end that matters to a player: a draw that is spoiled costs no
/// arrow. Nothing is taken at the nock, so there is nothing to hand back through
/// death, a logout or a shutdown — the three the plan warned would leak one
/// apiece if any of them were forgotten.
#[test]
fn an_interrupted_draw_costs_the_archer_no_arrow() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player_entity).unwrap();
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, connection, mob, now);
    let _ = packets_for(&mut world, connection);
    let quiver = items::backpack_of(&world.state, serial).unwrap();
    assert_eq!(
        items::count_in_container(&world.state, quiver, Graphic(0x0F3F)),
        20,
        "the nock only looked; the arrow is still in the pack"
    );

    // Out past the bow's reach while the string is still bent.
    world
        .state
        .teleport(mob_entity, Point::new(START.x + 15, START.y, 0));
    world.tick(now);

    assert_eq!(
        action_end(&packets_for(&mut world, connection)),
        Some((
            CombatActionOutcome::Interrupted(InterruptReason::OutOfReach).to_bits(),
            InterruptReason::OutOfReach.to_bits()
        )),
        "the draw ends, and names why"
    );
    assert_eq!(
        items::count_in_container(&world.state, quiver, Graphic(0x0F3F)),
        20,
        "and the archer is not robbed of the arrow it never loosed"
    );
}

/// A ranged target out of reach is watched just like one out of sight: the bow
/// draws, then holds until the same target returns to a shootable position.
#[test]
fn an_archer_holds_a_drawn_bow_for_a_target_out_of_reach() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, connection, mob, now);
    let _ = packets_for(&mut world, connection);

    // Out past the bow's reach. The draw already in flight ends with a reason,
    // and its replacement begins the watched draw.
    world
        .state
        .teleport(mob_entity, Point::new(START.x + 20, START.y, 0));
    world.tick(now);
    assert!(
        action_phase(&packets_for(&mut world, connection)).is_some(),
        "the bow begins drawing instead of standing in an out-of-reach refusal"
    );

    // Let preparation finish: it must now be held, rather than spoiled by range.
    for _ in 0..100 {
        world.tick(now);
        if matches!(
            world.state.registry.get::<CombatAction>(player_entity),
            Some(action) if matches!(action.phase, Phase::Armed { .. })
        ) {
            break;
        }
    }
    assert!(
        matches!(
            world.state.registry.get::<CombatAction>(player_entity),
            Some(action) if matches!(action.phase, Phase::Armed { .. })
        ),
        "a completed draw waits for the target to return within bow range"
    );

    // Back within reach, and the held bow releases against that same target.
    world
        .state
        .teleport(mob_entity, Point::new(START.x + 3, START.y, 0));
    world.tick(now);
    let packets = packets_for(&mut world, connection);
    assert!(
        action_phase(&packets).is_some(),
        "the held shot releases in the same tick the target returns"
    );
}

/// Melee must prepare while its target is still ahead of it. Otherwise a player
/// who runs into reach starts the whole wind-up only after arriving, which makes
/// a charge look like it never produced a blow at all.
#[test]
fn a_melee_weapon_prepares_while_running_to_its_target() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let fighter = world.state.players[&connection];
    let fighter_serial = world.state.registry.serial_of(fighter).unwrap();
    items::equip_worn_item(
        &mut world.state,
        fighter_serial,
        Graphic(0x0F61), // longsword
        Hue(0),
        Layer(1),
    )
    .unwrap();
    let target = spawn_mobile_at(&mut world, Point::new(START.x, START.y + 3, 0), 50, now);

    engage(&mut world, connection, target, now);
    let ready_at = match world
        .state
        .registry
        .get::<CombatAction>(fighter)
        .map(|action| action.phase)
    {
        Some(Phase::Arming {
            watch: Watch::TargetInReach,
            ready_at,
            ..
        }) => ready_at,
        other => panic!("a distant melee target did not start a prepared blow: {other:?}"),
    };

    // The player's initial facing is south, so these are two real running
    // strides. The target is three tiles south: after them the bodies are
    // adjacent, not overlapping.
    let hold_ticks =
        Gameplay::ticks_from_ms(u64::try_from(openshard_movement::RUN_HOLD.as_millis()).unwrap());
    for sequence in 0..2 {
        world.queue(Command::Walk {
            connection,
            request: run(sequence, Direction::South),
        });
        world.tick(now);
        for _ in 1..hold_ticks {
            world.tick(now);
        }
    }
    assert_eq!(
        world.state.registry.get::<Position>(fighter).unwrap().0,
        Point::new(START.x, START.y + 2, 0),
        "the fighter ran into melee reach"
    );

    while world.state.ticks < ready_at {
        world.tick(now);
    }
    assert!(
        matches!(
            world.state.registry.get::<CombatAction>(fighter),
            Some(action) if matches!(action.phase, Phase::Releasing { impact } if impact > world.state.ticks)
        ),
        "contact releases the prepared blow through its visible strike interval"
    );
}

/// The other silent exit from the commit pass, and the one a fighter meets every
/// time it wins: a drawn weapon with nothing in front of it.
///
/// This refusal used to be taken *before* the loop — a fighter with no aim was
/// filtered out of the pass entirely — so it could not be recorded, and the
/// sweep at the end then lifted whatever the fighter had been standing in.
/// Killing what you were fighting therefore took the bar, the glyph and the word
/// off the screen and put nothing at all in their place: the same blank the balk
/// state exists to end, reached by the commonest route there is.
#[test]
fn a_drawn_weapon_with_nothing_aimed_at_says_so() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let fighter = world.state.registry.serial_of(player_entity).unwrap();
    let _ = packets_for(&mut world, connection);

    // War drawn, nobody aimed at.
    world.queue(Command::WarMode {
        connection,
        war: true,
    });
    world.tick(now);
    assert_eq!(
        action_balks(&packets_for(&mut world, connection), fighter),
        vec![InterruptReason::NoTarget.to_bits()],
        "a fighter at the ready with no quarry is told which of the two it is"
    );

    // A standing state like every other one here: the edges cross the wire, and
    // the ticks in between do not.
    for _ in 0..5 {
        world.tick(now);
    }
    assert!(
        action_balks(&packets_for(&mut world, connection), fighter).is_empty(),
        "a refusal that has not changed is not re-sent every tick"
    );

    // Aim at something and it lifts, in the tick the swing commits.
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let _ = packets_for(&mut world, connection);
    engage(&mut world, connection, mob, now);
    let packets = packets_for(&mut world, connection);
    assert_eq!(
        action_balks(&packets, fighter),
        vec![0],
        "clear is a zero, and it is sent exactly once"
    );
    assert!(
        action_phase(&packets).is_some(),
        "and the blow the fighter had nobody to aim is committed in the same tick"
    );
}

/// The interval, in milliseconds, of every phase this actor announced in the
/// batch — the number a watcher measures its bar against.
fn action_phase_intervals(packets: &[Vec<u8>], actor: Serial) -> Vec<u32> {
    packets
        .iter()
        .filter(|packet| {
            packet.first() == Some(&0xBF)
                && packet.get(3..5) == Some(&CombatActionPhase::SUBCOMMAND.to_be_bytes()[..])
                && packet.get(5..9) == Some(&actor.raw().to_be_bytes()[..])
        })
        .filter_map(|packet| {
            let bytes: [u8; 4] = packet.get(15..19)?.try_into().ok()?;
            Some(u32::from_be_bytes(bytes))
        })
        .collect()
}

/// What a watcher's screen holds, rebuilt from the packets alone.
///
/// The three marks of `crowd::ActionRecord`, in ticks rather than in wall time:
/// a bar that runs for the interval its phase announced, a verdict that fades on
/// its own hold, and a refusal that stands until it is lifted. This is the whole
/// of what `Crowd::preparing` answers with, so a tick where all three are empty
/// is a tick with nothing over that fighter's head.
///
/// Modelled here rather than asserted through the client because the direction
/// rule forbids a server crate from naming one. What it costs is that the two
/// copies can drift; what it buys is the only oracle that answers the question
/// actually asked — *is anything written* — inside a test that can run a fight.
struct Screen {
    /// Ticks of bar left to fill, `0` for none.
    running: u32,
    /// Ticks of verdict left before it fades.
    ended:   u32,
    /// The standing refusal, which times out never.
    balked:  Option<InterruptReason>,
}

impl Screen {
    /// `OUTCOME_HOLD` in the client, in ticks: 1200ms at 40 a second.
    const OUTCOME_HOLD_TICKS: u32 = 48;

    const fn blank() -> Self {
        Self {
            running: 0,
            ended:   0,
            balked:  None,
        }
    }

    /// Age by one tick, then apply everything the shard said on it.
    ///
    /// **In the order it was said.** One tick carries the end of one action and
    /// the commit of the next — `resolve_actions` runs before `commit_actions`,
    /// so the ending arrives first — and a reader that sorted by kind instead of
    /// by arrival would let the ending wipe the bar that was opened after it.
    /// The client walks its inbox in order, and so does this.
    fn advance(&mut self, packets: &[Vec<u8>], actor: Serial) {
        self.running = self.running.saturating_sub(1);
        self.ended = self.ended.saturating_sub(1);
        for packet in packets {
            if packet.first() != Some(&0xBF) || packet.get(5..9) != Some(&actor.raw().to_be_bytes()[..]) {
                continue;
            }
            let Some(subcommand) = packet.get(3..5) else {
                continue;
            };
            if subcommand == CombatActionPhase::SUBCOMMAND.to_be_bytes() {
                // A tick is 25ms, and a bar with any interval at all is drawn
                // for at least the tick it was announced on.
                if let Some(interval) = action_phase_intervals(std::slice::from_ref(packet), actor).first() {
                    self.running = (interval / 25).max(1);
                }
            } else if subcommand == CombatActionEnded::SUBCOMMAND.to_be_bytes() {
                self.running = 0;
                self.ended = Self::OUTCOME_HOLD_TICKS;
            } else if subcommand == CombatActionBalked::SUBCOMMAND.to_be_bytes() {
                self.balked = packet.get(9).copied().and_then(InterruptReason::from_bits);
            }
        }
    }

    /// Whether there is anything over this fighter's head at all.
    const fn blank_now(&self) -> bool {
        self.running == 0 && self.ended == 0 && self.balked.is_none()
    }
}

/// What one fighter was doing on one tick, in the words the shard itself would
/// use — the row of the timeline [`fight_timeline`] builds.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Doing {
    /// Not fighting: at peace, or dead. Nothing is owed and nothing is said.
    Peace,
    /// Mid-action, and which stretch of it.
    Acting(&'static str, ActionStage),
    /// Standing in a refusal, and what is in the way.
    Balked(InterruptReason),
    /// **In war, acting on nothing, and refusing nothing.** The tick that has no
    /// answer to *"why is my character just standing there"*, and the one this
    /// timeline exists to find.
    Silent,
    /// The shard knows what it is doing and the *screen* does not: nothing is
    /// drawn over this fighter's head on this tick. The other half of the same
    /// report, and the half a server-state assertion cannot see.
    Blank(&'static str),
    /// A bar is drawn and it is measuring the wrong thing: the interval the
    /// watcher is filling against and the ticks the shard will actually wait
    /// have come apart by more than a tick. Held (ticks on screen, ticks owed).
    ///
    /// The picture lies rather than vanishes, which is the harder half to see by
    /// looking: a bar that finishes early sits full while the fighter goes on
    /// preparing, and a bar that finishes late is still filling when the blow
    /// lands. Both read as *"the bar has nothing to do with the swing"*.
    Adrift(u32, u32),
}

/// Walk `ticks` ticks and write down what `fighter` was doing on each one.
///
/// Read off the shard's own state rather than off the wire on purpose: the wire
/// carries *edges*, so a watcher's screen is a fold of the whole history and a
/// silent tick there could be a packet that was never sent or a packet that was
/// sent and superseded. The state is the ground truth about which of the two a
/// stall is, and the wire is asked separately once the state is known to be
/// sound.
fn fight_timeline<S: FnMut(&mut World, u32)>(
    world: &mut World,
    connection: ConnectionId,
    fighter: EntityId,
    ticks: u32,
    now: Instant,
    mut script: S,
) -> Vec<Doing> {
    let actor = world.state.registry.serial_of(fighter).unwrap();
    let mut screen = Screen::blank();
    (0..ticks)
        .map(|tick| {
            script(world, tick);
            world.tick(now);
            screen.advance(&packets_for(world, connection), actor);
            let warmode = world
                .registry()
                .get::<Combat>(fighter)
                .is_some_and(|combat| combat.warmode());
            if !warmode {
                return Doing::Peace;
            }
            let action = world.registry().get::<CombatAction>(fighter).copied();
            let doing = match action {
                Some(action) => {
                    let kind = match action.kind {
                        ActionKind::Swing { .. } => "swing",
                        ActionKind::Shot { .. } => "shot",
                        ActionKind::Breath { .. } => "breath",
                    };
                    Doing::Acting(kind, action.stage)
                }
                None => {
                    match world
                        .registry()
                        .get::<openshard_state::components::Balked>(fighter)
                    {
                        Some(balked) => Doing::Balked(balked.reason),
                        None => Doing::Silent,
                    }
                }
            };
            if screen.blank_now() {
                return match &doing {
                    Doing::Acting(kind, _) => Doing::Blank(kind),
                    _ => Doing::Blank("held up"),
                };
            }
            // The bar is there — is it measuring the right thing? The watcher is
            // filling against the interval it was last announced; the shard is
            // counting down to an impact it owns. One tick of slack, because the
            // packet is read on the tick after the one that produced it.
            if let Some(impact) = action.and_then(|action| action.impact()) {
                let owed = u32::try_from(impact.saturating_sub(world.state.ticks)).unwrap_or(u32::MAX);
                if screen.running.abs_diff(owed) > 1 {
                    return Doing::Adrift(screen.running, owed);
                }
            }
            doing
        })
        .collect()
}

/// The timeline as runs of equal rows, which is the only readable form of it:
/// four hundred ticks of a fight are a dozen stretches.
fn runs(timeline: &[Doing]) -> Vec<(usize, usize, &Doing)> {
    let mut out: Vec<(usize, usize, &Doing)> = Vec::new();
    for (tick, doing) in timeline.iter().enumerate() {
        match out.last_mut() {
            Some((_, end, last)) if *last == doing => *end = tick,
            _ => out.push((tick, tick, doing)),
        }
    }
    out
}

/// Every tick of a whole fight, and not one of them with nothing to say.
///
/// The user's report was *"there is a moment when nothing is written and he just
/// stands there"*, and a report shaped like that cannot be chased by reading the
/// commit pass: the stall is wherever the three verbs hand off to each other, and
/// which handoff it is depends on the weapon. So the fight is run instead — a
/// character and a mob, standing — and every tick is written down. A tick that is
/// [`Doing::Silent`] is the defect, by name and by tick number.
///
/// Both weapons, because they fail differently: a blow reaches one tile and a bow
/// reaches ten, so the melee half spends its life at the impact seam and the
/// ranged half spends its life in the draw.
#[test]
fn a_whole_fight_has_no_tick_the_shard_cannot_account_for() {
    for bow in [false, true] {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let fighter = world.state.players[&connection];
        if bow {
            arm_with_bow(&mut world, connection);
        }
        // Far enough that a bow is at range and near enough that a fist lands,
        // and tough enough to outlive the run: what is under test is the loop,
        // not the kill.
        let at = if bow { 3 } else { 1 };
        let mob = spawn_mobile_at(&mut world, Point::new(START.x + at, START.y, 0), 20_000, now);
        engage(&mut world, connection, mob, now);

        let timeline = fight_timeline(&mut world, connection, fighter, 600, now, |_, _| {});
        let unaccounted: Vec<usize> = timeline
            .iter()
            .enumerate()
            .filter_map(|(tick, doing)| {
                matches!(doing, Doing::Silent | Doing::Blank(_) | Doing::Adrift(..)).then_some(tick)
            })
            .collect();
        assert!(
            unaccounted.is_empty(),
            "{} ticks with nothing to show, or a bar measuring the wrong thing, with a {}: {:?}\n\
             the fight, in runs: {:#?}",
            unaccounted.len(),
            if bow { "bow" } else { "fist" },
            unaccounted,
            runs(&timeline)
        );
    }
}

/// The same fight, with everything a player actually does to it.
///
/// The standing fight above is the loop with nothing happening to it, and it is
/// the *unhappy* ticks that a report of "he just stands there" is about. So this
/// one scripts them, in the order an archer meets them: stepping while the bow
/// is bent, the quarry walking out past the reach and back, the quiver running
/// dry and being refilled, and the quarry dying with the weapon still drawn.
/// Every one of those is a seam between two of the four verbs, and the claim is
/// the same at all of them — the shard knows what this fighter is doing, and
/// says enough for a watcher to know it too.
#[test]
fn a_fight_with_everything_that_happens_to_one_still_has_no_blank_tick() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let fighter = world.state.players[&connection];
    let serial = world.state.registry.serial_of(fighter).unwrap();
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 20_000, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, connection, mob, now);

    let timeline = fight_timeline(&mut world, connection, fighter, 900, now, |world, tick| {
        match tick {
            // Two steps mid-draw, which is what the condition table is for.
            60 | 61 => {
                world.queue(Command::Walk {
                    connection,
                    request: walk((tick - 60) as u8, Direction::South),
                })
            }
            // Out past the bow's reach, and back inside it.
            200 => {
                world
                    .state
                    .teleport(mob_entity, Point::new(START.x + 25, START.y, 0))
            }
            300 => {
                world
                    .state
                    .teleport(mob_entity, Point::new(START.x + 3, START.y, 0))
            }
            // The quiver runs dry, and is refilled.
            // One at a time: the take is all-or-nothing against the amount asked
            // for, so a round number bigger than the quiver takes nothing at all.
            450 => while items::take_from_backpack(&mut world.state, serial, Graphic(0x0F3F), 1) > 0 {},
            600 => {
                items::give_to_backpack(&mut world.state, serial, Graphic(0x0F3F), Hue(0), 20, true);
            }
            // And the quarry dies with the weapon still drawn.
            750 => {
                world.state.registry.insert(
                    mob_entity,
                    openshard_state::components::Hitpoints {
                        current: 0,
                        max:     100,
                    },
                );
            }
            _ => {}
        }
    });

    let unaccounted: Vec<usize> = timeline
        .iter()
        .enumerate()
        .filter_map(|(tick, doing)| {
            matches!(doing, Doing::Silent | Doing::Blank(_) | Doing::Adrift(..)).then_some(tick)
        })
        .collect();
    assert!(
        unaccounted.is_empty(),
        "{} ticks with nothing to show, or a bar measuring the wrong thing: {:?}\n\
         the fight, in runs: {:#?}",
        unaccounted.len(),
        unaccounted,
        runs(&timeline)
    );
    // And the scenario actually happened. A script that quietly stopped biting —
    // a teleport inside the reach, a quiver the take did not empty — would leave
    // the assertion above passing over an ordinary fight, which is the way a
    // test like this dies without anyone noticing.
    assert!(
        timeline.contains(&Doing::Acting("shot", ActionStage::Aim)),
        "the out-of-reach target did not leave the bow held at full draw: {:#?}",
        runs(&timeline)
    );
    for expected in [InterruptReason::NoAmmo, InterruptReason::NoTarget] {
        assert!(
            timeline.contains(&Doing::Balked(expected)),
            "the script never drove the fighter into {expected:?}: {:#?}",
            runs(&timeline)
        );
    }
}

/// The fight, printed rather than asserted — the diagnostic behind every
/// assertion above.
///
/// `#[ignore]`d because it proves nothing: it is the thing you run when somebody
/// says *"he just stands there"* and you need to see the shard's own cadence in
/// ticks before arguing about frames. Run it with
/// `cargo test -p openshard-world print_a_bow_fight -- --ignored --nocapture`.
#[test]
#[ignore = "a diagnostic, not an assertion: run it by name with --nocapture"]
fn print_a_bow_fight() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let fighter = world.state.players[&connection];
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 20_000, now);
    engage(&mut world, connection, mob, now);
    println!(
        "swing_speed = {} ticks",
        combat::swing_speed(&world.state, fighter)
    );
    let timeline = fight_timeline(&mut world, connection, fighter, 400, now, |_, _| {});

    // The intervals cannot be read back out of the timeline, so run the wire
    // side of it separately: what a watcher was told to measure its bar against.
    let mut wire = super::tests::world();
    let wire_connection = enter(&mut wire, now);
    arm_with_bow(&mut wire, wire_connection);
    let wire_actor = wire
        .state
        .registry
        .serial_of(wire.state.players[&wire_connection])
        .unwrap();
    let wire_mob = spawn_mobile_at(&mut wire, Point::new(START.x + 3, START.y, 0), 20_000, now);
    engage(&mut wire, wire_connection, wire_mob, now);
    let mut announced: Vec<(usize, u32)> = Vec::new();
    for tick in 0..400 {
        wire.tick(now);
        for interval in action_phase_intervals(&packets_for(&mut wire, wire_connection), wire_actor) {
            announced.push((tick, interval));
        }
    }
    println!("phase intervals announced: {announced:?}");
    println!("the fight, in runs: {:#?}", runs(&timeline));
}

/// Walking up to a fight, which is the one thing an edge cannot tell you about.
///
/// A phase and a refusal both cross the wire only when they *change*, so a
/// client that was not watching at the moment of the change is never told at
/// all. Approaching an archer held off by a wall drew a body standing still with
/// nothing over its head — and a standing refusal never changes, so "until its
/// next transition" is, for that case, never. The health bar has ridden along
/// with the draw for exactly this reason since it was written; combat state now
/// does too.
#[test]
fn a_fighter_you_walk_up_to_arrives_with_what_it_is_doing() {
    // Mid-draw: the newcomer is owed the bar.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let fighter = world.state.players[&connection];
    let archer = world.state.registry.serial_of(fighter).unwrap();
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 20_000, now);
    engage(&mut world, connection, mob, now);
    for _ in 0..40 {
        world.tick(now);
    }
    let newcomer = enter(&mut world, now);
    let arrival = packets_for(&mut world, newcomer);
    let intervals = action_phase_intervals(&arrival, archer);
    assert_eq!(
        intervals.len(),
        1,
        "a body drawn mid-draw arrives with the draw on it"
    );
    assert!(
        intervals[0] > 0 && intervals[0] < 2_500,
        "and with what is *left* of the interval ({}ms of 2500), so the newcomer's bar \
         lines up with everybody else's rather than starting over",
        intervals[0]
    );
    assert_eq!(
        action_stages(&arrival, archer),
        vec![ActionStage::Load.to_bits()],
        "including which stretch of it the bow is in"
    );
}

/// The other half of walking up to a fight: a bow that has lost range is already
/// drawing and must be sent as that action, not as an old refusal that will not
/// describe the state it is actually in.
#[test]
fn a_fighter_you_walk_up_to_arrives_wearing_its_draw() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let fighter = world.state.players[&connection];
    let archer = world.state.registry.serial_of(fighter).unwrap();
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 20_000, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, connection, mob, now);
    world
        .state
        .teleport(mob_entity, Point::new(START.x + 25, START.y, 0));
    for _ in 0..40 {
        world.tick(now);
    }
    let newcomer = enter(&mut world, now);
    let arrival = packets_for(&mut world, newcomer);
    assert_eq!(
        action_phase_intervals(&arrival, archer).len(),
        1,
        "and a fighter drawing for its out-of-range target arrives wearing that draw"
    );
}

/// A concealed fighter's own screen, which is the one hole the scripted chain
/// above cannot reach: it needs the *commit* to happen from cover, and a running
/// action reveals its owner at the impact.
///
/// An untelegraphed action is deliberately invisible — no wind-up packet, no
/// stroke, because drawing one would break the cover an ambush *is*. That rule
/// is about **watchers**, and the commit pass applies it to everybody, the
/// archer included. So an ambusher with a bow stands stock still with nothing
/// over their head for a whole draw, which is precisely the picture a player
/// reports as *"he just stands there and nothing is written"*.
#[test]
fn an_ambusher_is_hidden_from_watchers_and_not_from_itself() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let fighter = world.state.players[&connection];
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 20_000, now);
    // Somebody else standing there, whose screen is the other half of the claim.
    let bystander = enter(&mut world, now);
    let archer = world.state.registry.serial_of(fighter).unwrap();
    // Into cover *before* aiming, so the commit itself happens from concealment.
    world.state.registry.insert(fighter, openshard_state::Hidden);
    engage(&mut world, connection, mob, now);
    let _ = packets_for(&mut world, bystander);

    // Short of the impact on purpose: the shot itself breaks cover, and every
    // draw after that one is an ordinary telegraphed draw that watchers are
    // *supposed* to hear. What is under test is the concealed one. Read off the
    // draw rather than written down, because the draw is an operator setting
    // now (`gameplay.action_speed`) and a constant here would quietly start
    // testing the second draw the day somebody retuned it.
    let draw = u32::try_from(combat::swing_speed(&world.state, fighter)).unwrap();
    let timeline = fight_timeline(&mut world, connection, fighter, draw - 2, now, |_, _| {});
    let blank: Vec<usize> = timeline
        .iter()
        .enumerate()
        .filter_map(|(tick, doing)| matches!(doing, Doing::Blank(_)).then_some(tick))
        .collect();
    assert!(
        blank.is_empty(),
        "{} ticks of a draw the archer cannot see: {:?}\nthe draw, in runs: {:#?}",
        blank.len(),
        blank,
        runs(&timeline)
    );
    // And the ambush is still an ambush. The rule that was over-applied is not
    // repealed: nothing about the concealed draw reached anybody else. Read
    // after the run rather than per tick, because `packets_for` leaves the other
    // connection's queue alone and this is the whole of it.
    let overheard = packets_for(&mut world, bystander);
    assert!(
        action_phase_intervals(&overheard, archer).is_empty(),
        "a concealed draw was narrated to a watcher"
    );
    assert!(
        action_stages(&overheard, archer).is_empty(),
        "and its stages were narrated too"
    );
}

/// A creature that loses its quarry must not report the quarry as *gone*.
///
/// Every way a fight could end used to arrive at one word. `clear_target` wrote
/// [`InterruptReason::TargetGone`] whoever called it, and two of its callers are
/// the brain giving up — on a chase it cannot finish, or on a foe it can no
/// longer see. The player is standing right there watching the monster in front
/// of them announce that they are gone, which is a lie about the one fact the
/// packet exists to carry. What ended the swing is the creature abandoning it.
///
/// Hiding is the cheapest way to make the brain lose sight of somebody who has
/// not moved an inch — which is exactly the shape of the defect.
#[test]
fn a_creature_that_loses_its_quarry_does_not_call_the_quarry_gone() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let creature = spawn_brained(&mut world, 0x00D1, Point::new(START.x + 1, START.y, 0), 8, now);

    // Let it notice and start swinging.
    for _ in 0..(AI_THINK_TICKS + 1) {
        world.tick(now);
    }
    assert!(
        world.registry().has::<CombatAction>(creature),
        "the creature is mid-swing before anything is taken away from it"
    );

    // Out of sight without going anywhere.
    world
        .state
        .registry
        .insert(player_entity, openshard_state::Hidden);
    let _ = packets_for(&mut world, connection);
    for _ in 0..(AI_THINK_TICKS + 1) {
        world.tick(now);
    }

    assert_eq!(
        action_end(&packets_for(&mut world, connection)),
        Some((
            CombatActionOutcome::Interrupted(InterruptReason::Abandoned).to_bits(),
            InterruptReason::Abandoned.to_bits()
        )),
        "the creature gave the fight up; it did not watch its foe die"
    );
    assert!(
        !world.registry().has::<Combat>(creature),
        "and a creature's war state lives only as long as the fight, so it is gone \
         rather than left standing as a fighter with nobody to fight"
    );
}

/// The other half of what a player is owed while a bow is bending: **which part
/// of the draw this is.** A bar answers *how far along* and cannot answer *how
/// far along what* — a bow coming up and a bow held at full draw fill the same
/// rectangle — so the shard walks the stages and announces each one it enters.
///
/// The shipped shares put a shot's boundaries at 10 and 80 percent of the
/// interval, so a whole draw crosses two of them. `Ready` is not among them:
/// every action opens in it, and a packet saying what the commit already implied
/// would be a packet nobody needed. Neither is `Aim`, and that is the load-bearing
/// half of this test — see `action_stages`: aiming is *holding*, an action
/// running through an interval holds nothing, and the stretch that used to sit
/// between the draw and the loose read on screen as a delay with no cause.
#[test]
fn a_drawn_bow_is_announced_stage_by_stage_as_it_bends() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let archer = world.state.registry.serial_of(player_entity).unwrap();
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 50, now);
    engage(&mut world, connection, mob, now);
    let draw = combat::swing_speed(&world.state, player_entity);
    let _ = packets_for(&mut world, connection);

    let mut seen = Vec::new();
    for _ in 0..draw {
        world.tick(now);
        seen.extend(action_stages(&packets_for(&mut world, connection), archer));
        if seen.len() >= 2 {
            break;
        }
    }
    assert_eq!(
        seen,
        vec![ActionStage::Load.to_bits(), ActionStage::Release.to_bits()],
        "the string is drawn, then loosed — in that order and each once, with no \
         stretch in between in which the bow is bent and nothing is happening"
    );
}

/// Ф3, and the sentence the shipped table is meant to read as: **an archer may
/// fire at a walk and sways at a run.** Both halves in one scene, because the
/// interesting claim is the *difference* — a rule that fired on every step would
/// pass a test that only ran.
///
/// The third assertion is the one the model turns on: a condition is a fact
/// about the action, charged once. A ten-second draw takes twenty steps, and a
/// sway per step would put a running archer's chance at zero for crossing a
/// room.
#[test]
fn a_shot_sways_at_a_run_is_free_at_a_walk_and_is_swayed_only_once() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    arm_with_bow(&mut world, connection);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 3, START.y, 0), 50, now);
    engage(&mut world, connection, mob, now);

    let accuracy = |world: &World| {
        world
            .state
            .registry
            .get::<CombatAction>(player_entity)
            .expect("the bow is drawn")
            .accuracy
    };
    assert_eq!(accuracy(&world), 0, "nothing has happened to the draw yet");
    let start = world.state.registry.get::<Position>(player_entity).unwrap().0;

    // The first request only turns: a mobile facing elsewhere spends one on
    // coming about, and a turn is not a step. The second is the walk. Sequences
    // start at zero — a fresh walker refuses anything else.
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::North),
    });
    world.tick(now);
    world.queue(Command::Walk {
        connection,
        request: walk(1, Direction::North),
    });
    world.tick(now);
    assert_ne!(
        world.state.registry.get::<Position>(player_entity).unwrap().0,
        start,
        "the archer really walked, or 'walking is free' is free of a walk"
    );
    assert_eq!(accuracy(&world), 0, "walking is free");

    let later = now + Duration::from_secs(1);
    let walked = world.state.registry.get::<Position>(player_entity).unwrap().0;
    world.queue(Command::Walk {
        connection,
        request: run(2, Direction::North),
    });
    world.tick(later);
    assert_ne!(
        world.state.registry.get::<Position>(player_entity).unwrap().0,
        walked,
        "and really ran — it was already facing north, so this one is a step"
    );
    assert_eq!(
        accuracy(&world),
        -ActionRules::RUNNING_SHOT_SWAY,
        "and running sways the shot that is already on the string"
    );

    let later_still = later + Duration::from_secs(1);
    world.queue(Command::Walk {
        connection,
        request: run(3, Direction::North),
    });
    world.tick(later_still);
    assert_eq!(
        accuracy(&world),
        -ActionRules::RUNNING_SHOT_SWAY,
        "a second stride is the same run: the rule is a fact about the draw, not a toll per step"
    );
}

/// D4 from the operator's end: *"a wound spoils it"* is a line in the config and
/// not a branch in the tick. The shipped shard lets a fighter swing through a
/// blow, so this scene has to say otherwise before it can watch one break.
#[test]
fn a_shard_whose_table_says_a_wound_spoils_a_blow_gets_one() {
    let now = Instant::now();
    let spoiled_by_wounds = ActionRules {
        swing: ConditionEffects {
            running: None,
            walking: None,
            mounted: None,
            struck:  Some(ActionEffect::Break),
            blinded: Some(ActionEffect::Break),
        },
        ..ActionRules::shipped()
    };
    let mut world = World::new(START).with_gameplay(Gameplay {
        action_rules: spoiled_by_wounds,
        ..Gameplay::default()
    });
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let player = world.state.registry.serial_of(player_entity).unwrap();
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    engage(&mut world, connection, mob, now);
    let _ = packets_for(&mut world, connection);
    assert!(
        world.state.registry.has::<CombatAction>(player_entity),
        "the swing is under way"
    );

    // Through the one door every wound passes, which is where the condition is
    // pushed from — not from a pass that went looking for it.
    combat::damage(&mut world.state, player, 5, DamageType::Physical, None);

    assert!(
        !world.state.registry.has::<CombatAction>(player_entity),
        "the blow the fighter was winding up is gone"
    );
    assert_eq!(
        action_end(&packets_for(&mut world, connection)),
        Some((
            CombatActionOutcome::Interrupted(InterruptReason::Struck).to_bits(),
            InterruptReason::Struck.to_bits()
        )),
        "and the watcher is told which condition took it"
    );
}

/// The third effect, and the one with a picture attached: an impact that moves
/// has to say so, or the stroke a watcher was given an interval to stretch runs
/// out over a blow that has not landed yet.
#[test]
fn a_slowed_blow_pushes_its_impact_and_re_announces_the_interval() {
    let now = Instant::now();
    let slowed_by_walking = ActionRules {
        swing: ConditionEffects {
            running: None,
            walking: Some(ActionEffect::Slow { percent: 100 }),
            mounted: None,
            struck:  None,
            blinded: Some(ActionEffect::Break),
        },
        ..ActionRules::shipped()
    };
    let mut world = World::new(START).with_gameplay(Gameplay {
        action_rules: slowed_by_walking,
        ..Gameplay::default()
    });
    let connection = enter(&mut world, now);
    let player_entity = world.state.players[&connection];
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    engage(&mut world, connection, mob, now);
    let impact = |world: &World| {
        world
            .state
            .registry
            .get::<CombatAction>(player_entity)
            .expect("the swing is under way")
            .impact()
            .expect("released, so it has a clock")
    };
    // The fighter is facing its target, so the first request turns it away and
    // only the second is a step. A turn spoils nothing, which is why the impact
    // is read after both.
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::North),
    });
    world.tick(now);
    let promised = impact(&world);
    let _ = packets_for(&mut world, connection);
    let before = world.state.registry.get::<Position>(player_entity).unwrap().0;

    world.queue(Command::Walk {
        connection,
        request: walk(1, Direction::North),
    });
    world.tick(now);
    assert_ne!(
        world.state.registry.get::<Position>(player_entity).unwrap().0,
        before,
        "the fighter really stepped, or there is no condition to have pushed anything"
    );

    let pushed = impact(&world);
    let remaining = promised - world.state.ticks;
    assert_eq!(
        pushed,
        promised + remaining,
        "a hundred percent doubles what the blow had left"
    );
    let phases = packets_for(&mut world, connection);
    let (_, phase, interval) = action_phase(&phases).expect("a fresh phase packet");
    assert_eq!(phase, 1, "still releasing, just later");
    assert_eq!(
        u64::from(interval) * openshard_state::TICKS_PER_SECOND / 1_000,
        pushed - world.state.ticks,
        "and the interval the watcher is given is the one the shard will actually wait"
    );
}

#[test]
fn a_weapon_override_beats_the_core_table() {
    // The pack's magic sword: a longsword item stamped with its own speed and
    // damage, which `equipped_weapon` reads ahead of the core table. Faster and
    // harder than a plain longsword (35 / 5..33), it keeps the graphic's skill.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(entity).unwrap();
    let sword = items::equip_worn_item(
        &mut world.state,
        serial,
        openshard_protocol::wire::Graphic(0x0F61),
        openshard_protocol::wire::Hue(0),
        Layer(1),
    )
    .unwrap();
    let sword_serial = world.state.registry.serial_of(sword).unwrap();
    let scale = world.state.gameplay.speed_scale_factor;

    // A plain longsword first, for contrast.
    assert_eq!(
        combat::swing_speed(&world.state, entity),
        swing_ticks(100, 35, 1, scale)
    );
    // Now the enchantment: speed 60 (faster), damage a fixed 40..40.
    items::set_weapon(&mut world.state, sword_serial, 60, 40, 40);
    assert_eq!(
        combat::swing_speed(&world.state, entity),
        swing_ticks(100, 60, 1, scale),
        "the override's speed wins over the table's 35"
    );
    assert_eq!(
        combat::melee_blow(&mut world.state, entity),
        40,
        "and its damage band (40..=40) wins over 5..33"
    );
}

#[test]
fn a_creatures_notoriety_colours_its_health_bar() {
    // Spawn an orange enemy and read the notoriety byte out of the 0x78 that
    // draws it — the health-bar colour on the wire.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let _ = packets_for(&mut world, player);
    let mob = spawn_mobile_full(&mut world, Point::new(START.x, START.y, 0), 50, 5, 5, 0, now);

    let drawn = packets_for(&mut world, player)
        .into_iter()
        .find(|p| p[0] == 0x78 && mentions(p, mob))
        .expect("the creature is drawn");
    assert_eq!(drawn[18], 0x05, "the notoriety byte is Enemy/orange");
}

#[test]
fn an_invulnerable_mobile_cannot_be_attacked() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    // Notoriety 7 is invulnerable — a yellow, untouchable townsperson.
    let mob = spawn_mobile_full(&mut world, Point::new(START.x, START.y, 0), 50, 7, 5, 0, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::Attack {
        connection: player,
        target:     Some(mob),
    });
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<Combat>(player_entity)
            .and_then(|combat| combat.target()),
        None,
        "the attack is refused"
    );
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| p == &[0xAA, 0, 0, 0, 0]),
        "and the client's target is cleared"
    );
}

#[test]
fn attacking_an_innocent_turns_the_attacker_grey() {
    let now = Instant::now();
    let mut world = world();
    let aggressor = enter(&mut world, now);
    let victim = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let aggressor_entity = world.state.players[&aggressor];
    let aggressor_serial = serial_of(&world, aggressor);
    let victim_serial = serial_of(&world, victim);
    let _ = packets_for(&mut world, victim);

    engage(&mut world, aggressor, victim_serial, now);

    assert_eq!(
        world.state.notoriety_of(aggressor_entity),
        Notoriety::Criminal,
        "raising a hand against an innocent is a crime"
    );
    assert!(
        packets_for(&mut world, victim)
            .iter()
            .any(|p| p[0] == 0x77 && mentions(p, aggressor_serial)),
        "and everyone watching sees them turn grey"
    );
}

#[test]
fn five_innocent_kills_turn_the_killer_red() {
    // Murderer flagging: the tally of killed innocents is persistent, and the
    // fifth turns the killer red for good.
    let now = Instant::now();
    let mut world = world();
    let killer = enter(&mut world, now);
    let killer_entity = world.state.players[&killer];

    for kill in 1..=5 {
        // A blue, one-hit victim on the killer's tile.
        let victim = spawn_mobile_full(
            &mut world,
            Point::new(START.x, START.y, 0),
            1,
            Notoriety::Innocent.to_bits(),
            0,
            0,
            now,
        );
        engage(&mut world, killer, victim, now);
        for _ in 0..=WRESTLING_SWING_TICKS {
            world.tick(now);
        }
        assert!(
            world.state.registry.entity_of(victim).is_none(),
            "the innocent is dead"
        );
        if kill < 5 {
            assert_ne!(
                world.state.notoriety_of(killer_entity),
                Notoriety::Murderer,
                "still short of the murder threshold after {kill} kills"
            );
        }
    }

    assert_eq!(
        world.state.notoriety_of(killer_entity),
        Notoriety::Murderer,
        "the fifth innocent killed makes a murderer"
    );
}

#[test]
fn murder_counts_fade_and_wash_the_killer_blue() {
    // The count is persistent, not permanent: old kills age off one at a time,
    // and once the killer drops below the threshold it goes back to innocent.
    let now = Instant::now();
    let mut world = world();
    let killer = enter(&mut world, now);
    let killer_entity = world.state.players[&killer];
    let killer_serial = serial_of(&world, killer);

    for _ in 0..5 {
        let victim = spawn_mobile_full(
            &mut world,
            Point::new(START.x + 5, START.y, 0),
            1,
            Notoriety::Innocent.to_bits(),
            0,
            0,
            now,
        );
        world.queue(Command::Damage {
            serial:      victim,
            amount:      100,
            damage_type: 0,
            by:          Some(killer_serial),
        });
        world.tick(now);
    }
    assert_eq!(
        world.state.notoriety_of(killer_entity),
        Notoriety::Murderer,
        "five kills, red"
    );

    // Bring the decay forward rather than run eight hours of ticks: one count
    // fades, dropping to four — below the threshold — and the killer washes
    // blue.
    let soon = world.state.ticks + 1;
    world
        .state
        .registry
        .insert(killer_entity, MurderDecay { at_tick: soon });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Murders>(killer_entity).map(|m| m.0),
        Some(4),
        "one murder aged off"
    );
    assert_eq!(
        world.state.notoriety_of(killer_entity),
        Notoriety::Innocent,
        "below the threshold, no longer a murderer"
    );
}

#[test]
fn an_attributed_spell_kill_is_a_murder_too() {
    // Attribution is not melee-only: damage that names its dealer — a script's
    // spell blaming its caster — tallies a murder just as a swing does.
    let now = Instant::now();
    let mut world = world();
    let killer = enter(&mut world, now);
    let killer_entity = world.state.players[&killer];
    let killer_serial = serial_of(&world, killer);

    for _ in 0..5 {
        let victim = spawn_mobile_full(
            &mut world,
            Point::new(START.x + 5, START.y, 0),
            1,
            Notoriety::Innocent.to_bits(),
            0,
            0,
            now,
        );
        world.queue(Command::Damage {
            serial:      victim,
            amount:      100,
            damage_type: 0,
            by:          Some(killer_serial),
        });
        world.tick(now);
    }

    assert_eq!(
        world.state.notoriety_of(killer_entity),
        Notoriety::Murderer,
        "five innocents killed by attributed spell damage is murder"
    );
}

#[test]
fn unattributed_damage_kills_without_blame() {
    // The other side of it: damage with no dealer named (a script's raw
    // Command::Damage with no `by`, an environmental hazard) kills but pins no murder.
    let now = Instant::now();
    let mut world = world();
    let bystander = enter(&mut world, now);
    let bystander_entity = world.state.players[&bystander];

    for _ in 0..5 {
        let victim = spawn_mobile_full(
            &mut world,
            Point::new(START.x + 5, START.y, 0),
            1,
            Notoriety::Innocent.to_bits(),
            0,
            0,
            now,
        );
        world.queue(Command::Damage {
            serial:      victim,
            amount:      100,
            damage_type: 0,
            by:          None,
        });
        world.tick(now);
    }

    assert_ne!(
        world.state.notoriety_of(bystander_entity),
        Notoriety::Murderer,
        "nobody was blamed for unattributed kills"
    );
}

#[test]
fn attacking_an_enemy_is_not_a_crime() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    // A plain orange enemy.
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);

    engage(&mut world, player, mob, now);

    assert_eq!(
        world.state.notoriety_of(player_entity),
        Notoriety::Innocent,
        "attacking what is already an enemy costs no standing"
    );
}

#[test]
fn the_criminal_flag_lifts_when_its_time_runs_out() {
    let now = Instant::now();
    let mut world = world();
    let aggressor = enter(&mut world, now);
    let victim = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let aggressor_entity = world.state.players[&aggressor];
    let victim_serial = serial_of(&world, victim);

    engage(&mut world, aggressor, victim_serial, now);
    assert_eq!(world.state.notoriety_of(aggressor_entity), Notoriety::Criminal);

    // Bring the flag's expiry forward rather than run two minutes of ticks.
    let soon = world.state.ticks + 1;
    world
        .state
        .registry
        .insert(aggressor_entity, CriminalUntil { tick: soon });
    world.tick(now);

    assert_eq!(
        world.state.notoriety_of(aggressor_entity),
        Notoriety::Innocent,
        "the flag lifts and they are blue again"
    );
}

#[test]
fn resistance_is_by_damage_type() {
    // Fifty percent fire resistance halves a fireball but does nothing to a
    // sword: resistance is per type, applied in one place for every source.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 100, now);
    let mob_entity = entity(&world, mob);
    world.state.registry.insert(
        mob_entity,
        Resistance {
            fire:     50,
            physical: 0,
            cold:     0,
            poison:   0,
            energy:   0,
        },
    );
    let _ = packets_for(&mut world, player);

    // 10 fire, halved to 5.
    world.queue(Command::Damage {
        serial:      mob,
        amount:      10,
        damage_type: 1, // fire
        by:          None,
    });
    world.tick(now);
    assert_eq!(
        world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current,
        95
    );

    // 10 physical, unresisted.
    world.queue(Command::Damage {
        serial:      mob,
        amount:      10,
        damage_type: 0, // physical
        by:          None,
    });
    world.tick(now);
    assert_eq!(
        world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current,
        85
    );
}

#[test]
fn armour_reduces_a_blow() {
    // Same five-damage swing, but the target's 50% physical resistance halves
    // it: two through, not five.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_full(&mut world, Point::new(START.x, START.y, 0), 50, 5, 5, 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);

    for _ in 0..WRESTLING_SWING_TICKS {
        world.tick(now);
    }
    assert_eq!(
        world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current,
        48,
        "five damage minus half is two"
    );
}

#[test]
fn swing_speed_sets_the_cadence() {
    // A faster swinger lands a blow in fewer ticks than the default interval
    // would allow.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    world
        .state
        .registry
        .insert(player_entity, SwingSpeed { ticks: 5 });
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 100, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);

    // Five is fewer than the default interval. The configured pace still wins;
    // the client compresses the seven visible frames into those five ticks.
    const _: () = assert!(5 < WRESTLING_SWING_TICKS);
    // What five ticks is in milliseconds, which is the unit the wire carries —
    // `WorldState::animate_timed` does this same conversion, and writing the
    // answer out here would be a second place the tick rate is spelled.
    let five_ticks_ms = (5 * 1_000 / TICKS_PER_SECOND) as u32;
    assert!(packets_for(&mut world, player).iter().any(|packet| {
        packet[0] == 0xBF && packet.len() == 13 && packet[9..13] == five_ticks_ms.to_be_bytes()
    }));
    for _ in 0..5 {
        world.tick(now);
    }
    assert!(
        world.state.registry.get::<Hitpoints>(mob_entity).unwrap().current < 100,
        "the quicker swing has already landed"
    );
}

#[test]
fn a_spawned_creature_derives_its_swing_speed() {
    // Spawned with `swing == 0`, a creature carries no explicit `SwingSpeed`;
    // its pace is derived from dexterity through Sphere's formula — the
    // wrestling default here, since it has no stats set.
    let now = Instant::now();
    let mut world = world();
    enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let mob_entity = entity(&world, mob);
    assert!(
        world.state.registry.get::<SwingSpeed>(mob_entity).is_none(),
        "zero on spawn pins nothing"
    );
    assert_eq!(
        combat::swing_speed(&world.state, mob_entity),
        WRESTLING_SWING_TICKS,
        "and the derived pace is the wrestling default"
    );
}

#[test]
fn dexterity_quickens_the_swing() {
    // Sphere's era-1 formula: a nimbler mobile swings sooner. Raising
    // dexterity above the default shortens the interval `swing_speed` reports.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    let serial = serial_of(&world, player);

    let slow = combat::swing_speed(&world.state, player_entity);
    world.queue(Command::SetStats {
        serial,
        strength: DEFAULT_HITPOINTS,
        dexterity: 200,
        intelligence: DEFAULT_MANA,
    });
    world.tick(now);
    let fast = combat::swing_speed(&world.state, player_entity);

    assert_eq!(slow, WRESTLING_SWING_TICKS, "default dexterity, default pace");
    assert!(fast < slow, "more dexterity swings sooner: {fast} < {slow}");
}

#[test]
fn killing_the_target_ends_the_attack() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    // Eight hits, five a swing: dead on the second.
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 8, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);

    for _ in 0..(2 * WRESTLING_SWING_TICKS) {
        world.tick(now);
    }
    assert!(
        !world.state.registry.contains(mob_entity),
        "the creature is dead and gone"
    );
    assert_eq!(
        world
            .state
            .registry
            .get::<Combat>(player_entity)
            .and_then(|combat| combat.target()),
        None,
        "and the attacker is no longer swinging at it"
    );
}

/// A mobile's value in a skill, in tenths.
fn skill_value(world: &World, entity: EntityId, skill: u8) -> u16 {
    let Some(skill) = Skill::from_id(skill) else {
        return 0;
    };
    world
        .state
        .registry
        .get::<Skills>(entity)
        .map_or(0, |s| s.get(skill))
}

#[test]
fn setting_a_skill_stores_it() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);

    world.queue(Command::SetSkill {
        serial,
        skill: 1,
        value: 755,
    });
    world.tick(now);
    assert_eq!(skill_value(&world, entity, 1), 755);
}

#[test]
fn a_skill_query_is_answered_with_the_skill_list() {
    // Opening the skill window sends a 0x34 type 0x05; it must be answered with
    // the 0x3A list, not the status the paperdoll's 0x34 type 0x04 gets.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::RequestSkills { connection });
    world.tick(now);
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x3A && p[3] == 0x02),
        "the skill window request is answered with the full list"
    );
}

#[test]
fn entering_the_world_sends_the_skill_window() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    // The full skill list rode out on login: a 0x3A whose type byte is the
    // capped-absolute form a modern (TOL) client gets.
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x3A && p[3] == 0x02),
        "the skill window is filled on login"
    );
}

#[test]
fn a_skill_lock_arrow_is_stored() {
    use openshard_protocol::skill::SkillLock;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];

    world.queue(Command::SetSkillLock {
        connection,
        skill: RawSkillId(45), // Mining
        lock: SkillLock::Down,
    });
    world.tick(now);
    assert_eq!(
        world
            .registry()
            .get::<Skills>(entity)
            .map_or(SkillLock::Up, |s| s.lock(Skill::Mining)),
        SkillLock::Down,
        "the down arrow was stored"
    );
}

#[test]
fn a_skill_gain_updates_the_open_window() {
    // A low skill used against a trivial task gains within a few tries; each
    // gain pushes a single-line 0x3A update (the delta-capped type 0xDF) to the
    // owner so an open window follows it live.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let serial = serial_of(&world, connection);
    world.queue(Command::SetSkill {
        serial,
        skill: 1,
        value: 100,
    });
    world.tick(now);
    let _ = packets_for(&mut world, connection);

    let mut saw_update = false;
    let mut saw_message = false;
    for _ in 0..80 {
        world.queue(Command::UseSkill {
            serial,
            skill: 1,
            min_skill: 0,
            max_skill: 500,
        });
        world.tick(now);
        let packets = packets_for(&mut world, connection);
        saw_update |= packets.iter().any(|p| p[0] == 0x3A && p[3] == 0xDF);
        saw_message |= packets.iter().any(|p| {
            // The wording and the hue are the subject; the *size* of the gain is
            // a roll off the world's seeded stream and is deliberately not
            // pinned. It was once — as "increased by 0.1.  It is now 10.1." —
            // and the first gain is 0.3 today, so the assertion failed on a
            // number nothing here is about.
            let text = String::from_utf8_lossy(p);
            p[0] == 0x1C
                && p[10..12] == [0x00, 0x58]
                && text.contains("Your skill in Anatomy has increased by ")
                && text.contains(".  It is now 1")
        });
        if saw_update && saw_message {
            break;
        }
    }
    assert!(saw_update, "a gain pushed a single-skill update to the window");
    assert!(
        saw_message,
        "a gain wrote ClassicUO's coloured, quantified line to the journal"
    );
}

#[test]
fn a_characters_stats_and_skills_survive_a_relogin() {
    use openshard_protocol::skill::SkillLock;
    use openshard_state::components::Stats;
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let serial = serial_of(&world, conn);

    // Train a skill, set stats, and lock the skill down.
    world.queue(Command::SetSkill {
        serial,
        skill: 25, // Magery
        value: 501,
    });
    world.queue(Command::SetStats {
        serial,
        strength: 55,
        dexterity: 40,
        intelligence: 90,
    });
    world.tick(now);
    world.queue(Command::SetSkillLock {
        connection: conn,
        skill:      RawSkillId(25),
        lock:       SkillLock::Down,
    });
    world.tick(now);

    // The save captures the stats, the skill, and its lock.
    world.take_snapshot();
    let snapshot = world.drain_saves().next_back().expect("a snapshot");
    let record = snapshot
        .characters
        .iter()
        .find(|c| c.serial == serial)
        .cloned()
        .expect("the character was saved");
    assert_eq!(record.strength, 55);
    assert_eq!(record.intelligence, 90);
    let magery = record.skills.iter().find(|s| s.id == 25).expect("magery saved");
    assert_eq!(magery.value, 501);
    assert_eq!(magery.lock, SkillLock::Down.to_bits(), "the lock is saved too");

    // Relogin, threading the record back through Enter the way the server does.
    world.queue(Command::Disconnect { connection: conn });
    world.tick(now);
    let conn = connection();
    world.queue(Command::Enter(Entering {
        connection: conn,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        // Nothing but the name: the world reads its own roster, which the
        // logout above wrote. Handing the row in would test an unpacking the
        // shard no longer does — see S4 in
        // `docs/server/evidence/2026-07-30-the-connection-state-machine.md`.
        character:  Character::Saved,
    }));
    world.tick(now);
    let player = world.state.players[&conn];
    assert_eq!(
        world.registry().get::<Stats>(player).unwrap().strength,
        55,
        "stats came back"
    );
    assert_eq!(skill_value(&world, player, 25), 501, "the skill came back");
    assert_eq!(
        world
            .registry()
            .get::<Skills>(player)
            .unwrap()
            .lock(Skill::Magery),
        SkillLock::Down,
        "and its lock arrow"
    );
}

// -- spell casting --------------------------------------------------------

/// A reagent graphic used by the cast tests.
const BLACK_PEARL: u16 = 0x0F7A;
/// The other two the travel family wants.
const BLOOD_MOSS: u16 = 0x0F7B;
const MANDRAKE_ROOT: u16 = 0x0F86;

/// A player ready to cast: grandmaster Magery and a pack full of a reagent.
/// Returns its connection and entity.
fn ready_caster(world: &mut World, reagent: u16, now: Instant) -> (ConnectionId, EntityId) {
    let connection = enter(world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(world, connection);
    world.queue(Command::SetSkill {
        serial,
        skill: 25, // Magery
        value: 1000,
    });
    world.tick(now);
    let backpack = backpack_serial(world, connection);
    assert!(
        openshard_items::give(
            &mut world.state,
            backpack,
            openshard_protocol::wire::Graphic(reagent),
            openshard_protocol::wire::Hue(0),
            20,
        )
        .is_complete()
    );
    // A full spellbook, so the cast gate — you may cast only what your book holds
    // — passes for every spell a cast test tries.
    if let Some(book) = openshard_items::give(
        &mut world.state,
        backpack,
        openshard_state::components::SPELLBOOK_GRAPHIC,
        openshard_protocol::wire::Hue(0),
        1,
    )
    .last
    {
        world
            .state
            .registry
            .insert(book, openshard_state::components::Spellbook::full());
    }
    let _ = packets_for(world, connection);
    (connection, entity)
}

/// Give a connection's character a full spellbook so the cast gate passes,
/// without stocking reagents or setting skill — the parts a cost test controls.
fn give_full_spellbook(world: &mut World, connection: ConnectionId) {
    let backpack = backpack_serial(world, connection);
    if let Some(book) = openshard_items::give(
        &mut world.state,
        backpack,
        openshard_state::components::SPELLBOOK_GRAPHIC,
        openshard_protocol::wire::Hue(0),
        1,
    )
    .last
    {
        world
            .state
            .registry
            .insert(book, openshard_state::components::Spellbook::full());
    }
}

/// A world whose spells cast Sphere-style — resolve at once, no rooting.
fn sphere_world() -> World {
    World::new(START).with_gameplay(Gameplay {
        cast_style: openshard_state::CastStyle::Walk,
        ..Default::default()
    })
}

#[test]
fn a_sphere_cast_resolves_at_once() {
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);
    let mana_before = world.registry().get::<Mana>(entity).unwrap().current;

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    }); // Fireball
    world.tick(now);

    assert!(
        world
            .registry()
            .get::<openshard_state::components::Casting>(entity)
            .is_none(),
        "the sphere style roots nobody"
    );
    assert!(
        world.registry().get::<Mana>(entity).unwrap().current < mana_before,
        "the mana was paid at once"
    );
    assert!(
        packets_for(&mut world, connection).iter().any(|p| p[0] == 0x6C),
        "and the target cursor came up at once"
    );
}

#[test]
fn a_servuo_cast_waits_out_its_delay_then_targets() {
    let now = Instant::now();
    let mut world = world(); // the default is the ServUO stop-to-cast style
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);
    let mana_before = world.registry().get::<Mana>(entity).unwrap().current;

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    });
    world.tick(now);
    assert!(
        world
            .registry()
            .get::<openshard_state::components::Casting>(entity)
            .is_some(),
        "the caster is committed to the cast"
    );
    assert_eq!(
        world.registry().get::<Mana>(entity).unwrap().current,
        mana_before,
        "mana is not spent until the cast resolves"
    );
    let _ = packets_for(&mut world, connection);

    // Wait out the cast delay — a second of it.
    let mut later = now;
    for _ in 0..Gameplay::ticks(1) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        world
            .registry()
            .get::<openshard_state::components::Casting>(entity)
            .is_none(),
        "the cast finished"
    );
    assert!(
        world.registry().get::<Mana>(entity).unwrap().current < mana_before,
        "and paid its mana"
    );
    assert!(
        packets_for(&mut world, connection).iter().any(|p| p[0] == 0x6C),
        "then the target cursor came up"
    );
}

/// The power words go out with the *beginning* of a cast, not with its end.
///
/// ServUO's `Spell.Cast` says the mantra on the line after `RevealingAction`,
/// before a single tick of the cast delay is measured, and the reason is
/// gameplay rather than ceremony: a mage's words are the warning everyone nearby
/// gets. Said at resolution they would arrive together with the fireball, which
/// is no warning at all — so this uses the rooted style, where beginning and
/// landing are a second apart, and looks for the words on the first tick.
#[test]
fn the_power_words_are_said_as_the_cast_begins() {
    let now = Instant::now();
    let mut world = world(); // the rooted ServUO style: held, not yet resolved
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17), // Fireball, "Vas Flam"
    });
    world.tick(now);
    assert!(
        world
            .registry()
            .get::<openshard_state::components::Casting>(entity)
            .is_some(),
        "the cast is still being held, so anything below happened at its start"
    );

    let spoken: Vec<Vec<u8>> = packets_for(&mut world, connection)
        .into_iter()
        .filter(|p| p[0] == 0xAE)
        .collect();
    assert!(
        spoken.iter().any(|p| {
            // Unicode `0xAE`: strip the zero bytes and the ASCII reads through.
            let text: Vec<u8> = p.iter().copied().filter(|&b| b != 0).collect();
            String::from_utf8_lossy(&text).contains("Vas Flam")
        }),
        "the caster spoke the spell's own words"
    );
    // And in the spell mode rather than as an ordinary sentence. The mode byte
    // sits after `0xAE`'s id, length, speaker serial and body graphic.
    assert!(
        spoken.iter().any(|p| p[9] == 10),
        "said as ServUO's MessageType.Spell"
    );
}

/// The gesture is thrown when the cast begins, and not again when it lands.
///
/// It used to play at resolution, which put the arm movement *after* the second
/// of rooted casting it is supposed to fill — and left a fizzle, which resolves
/// without an effect, with no gesture at all.
#[test]
fn the_cast_gesture_is_thrown_at_the_start_and_not_at_the_end() {
    let now = Instant::now();
    let mut world = world();
    let (connection, _) = ready_caster(&mut world, BLACK_PEARL, now);
    // Either animation packet counts: the classic `0x6E` and the newer `0xE2`
    // are one fact told to two client generations.
    let animated = |packets: &[Vec<u8>]| packets.iter().any(|p| p[0] == 0x6E || p[0] == 0xE2);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    });
    world.tick(now);
    assert!(
        animated(&packets_for(&mut world, connection)),
        "the gesture goes out with the start of the cast"
    );

    // Wait the cast out; the spell resolves and raises its cursor.
    let mut later = now;
    for _ in 0..Gameplay::ticks(1) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    let landing = packets_for(&mut world, connection);
    assert!(
        landing.iter().any(|p| p[0] == 0x6C),
        "the cast finished and asked for its target"
    );
    assert!(
        !animated(&landing),
        "and did not throw a second gesture on the way out"
    );
}

/// The art a spell lands with is its own, not its archetype's.
///
/// Fireball and Flamestrike are both `Damage(Fire, _)`, and while the visual was
/// keyed on that they were one picture and one sound between them: the same bolt
/// for the third circle and the seventh. ServUO throws a fireball for one and
/// burns the ground under the target's feet for the other, and the spell table
/// now carries which.
#[test]
fn two_fire_spells_no_longer_share_one_picture() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let caster = world.state.players[&connection];
    let at = Point::new(START.x + 1, START.y, 0);
    let victim = spawn_mobile_at(&mut world, at, 30_000, now);
    world.tick(now);

    // The effect straight from the core, so nothing has to be paid for: what is
    // under test is the art, and `0x70` carries it after the kind and the two
    // serials it links.
    let art_of = |world: &mut World, spell: SpellId| -> Vec<u16> {
        world.drain_outbound().count();
        world.apply_spell_effect(caster, spell, Some(victim), at);
        packets_for(world, connection)
            .iter()
            .filter(|p| p[0] == 0x70)
            .map(|p| u16::from_be_bytes([p[10], p[11]]))
            .collect()
    };

    let fireball = art_of(&mut world, SpellId(17));
    let flamestrike = art_of(&mut world, SpellId(50));
    assert!(
        fireball.contains(&0x36D4),
        "Fireball throws ServUO's fireball: {fireball:04X?}"
    );
    assert!(
        flamestrike.contains(&0x3709),
        "Flamestrike burns where the target stands: {flamestrike:04X?}"
    );
    assert!(
        !flamestrike.contains(&0x36D4),
        "and is no longer drawn as a fireball"
    );
}

#[test]
fn a_travel_spell_asks_for_an_object_and_not_a_patch_of_ground() {
    let now = Instant::now();
    let mut world = sphere_world(); // resolve at once, so the cursor comes up this tick
    let (connection, _) = ready_caster(&mut world, BLACK_PEARL, now);
    let backpack = backpack_serial(&world, connection);
    for reagent in [BLOOD_MOSS, MANDRAKE_ROOT] {
        assert!(
            openshard_items::give(
                &mut world.state,
                backpack,
                openshard_protocol::wire::Graphic(reagent),
                openshard_protocol::wire::Hue(0),
                20,
            )
            .is_complete()
        );
    }

    // Recall aims at a rune, so the client itself must refuse bare ground.
    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(31),
    });
    world.tick(now);
    let cursor = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0x6C)
        .expect("the cursor came up");
    assert_eq!(cursor[1], 0, "an object cursor, not a location one");

    // A mobile-targeted spell still raises the permissive cursor, so the change
    // is to the travel family and not to targeting at large.
    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17), // Magic Arrow
    });
    world.tick(now);
    let cursor = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0x6C)
        .expect("the cursor came up");
    assert_eq!(cursor[1], 1, "still a location cursor");
}

#[test]
fn stepping_breaks_a_cast() {
    let now = Instant::now();
    let mut world = world();
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);
    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    });
    world.tick(now);
    assert!(
        world
            .registry()
            .get::<openshard_state::components::Casting>(entity)
            .is_some()
    );

    world.queue(Command::Walk {
        connection,
        request: walk(1, Direction::North),
    });
    world.tick(now);
    assert!(
        world
            .registry()
            .get::<openshard_state::components::Casting>(entity)
            .is_none(),
        "a step chose the walk over the spell"
    );
}

#[test]
fn a_blow_disturbs_a_cast_when_the_shard_says_so() {
    let now = Instant::now();
    let mut world = world(); // spell_disturb is on by default
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);
    let serial = serial_of(&world, connection);
    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    });
    world.tick(now);
    assert!(
        world
            .registry()
            .get::<openshard_state::components::Casting>(entity)
            .is_some()
    );

    world.queue(Command::Damage {
        serial,
        amount: 5,
        damage_type: 0,
        by: None,
    });
    world.tick(now);
    assert!(
        world
            .registry()
            .get::<openshard_state::components::Casting>(entity)
            .is_none(),
        "the blow broke the cast"
    );
}

#[test]
fn a_fireball_damages_the_mobile_it_is_aimed_at() {
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, _) = ready_caster(&mut world, BLACK_PEARL, now);
    let target = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    });
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(target),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    let target_entity = world.registry().entity_of(target).expect("the target");
    assert!(
        world.registry().get::<Hitpoints>(target_entity).unwrap().current < 50,
        "the fireball hurt what it was aimed at"
    );
}

/// The reagent the Poison spell consumes.
const NIGHTSHADE: u16 = 0x0F88;

#[test]
fn poison_pulses_damage_then_wears_off() {
    use openshard_state::components::Poisoned;
    let now = Instant::now();
    let mut world = world();
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let entity = world.registry().entity_of(mob).unwrap();
    let ticks = world.state.ticks;
    combat::apply_poison(
        &mut world.state,
        mob,
        openshard_protocol::world::PoisonLevel::new(2),
        ticks,
    ); // greater
    assert!(world.registry().get::<Poisoned>(entity).is_some(), "poisoned");

    let hp_before = world.registry().get::<Hitpoints>(entity).unwrap().current;
    let mut later = now;
    for _ in 0..(combat::POISON_INTERVAL * u64::from(combat::POISON_PULSES) + 5) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    let hp_after = world.registry().get::<Hitpoints>(entity).unwrap().current;
    assert!(
        hp_after < hp_before,
        "poison hurt the mobile ({hp_before} -> {hp_after})"
    );
    assert!(
        world.registry().get::<Poisoned>(entity).is_none(),
        "and wore off after its pulses"
    );
}

#[test]
fn cure_clears_poison() {
    use openshard_state::components::Poisoned;
    let now = Instant::now();
    let mut world = world();
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let entity = world.registry().entity_of(mob).unwrap();
    let ticks = world.state.ticks;
    combat::apply_poison(
        &mut world.state,
        mob,
        openshard_protocol::world::PoisonLevel::new(2),
        ticks,
    );
    assert!(
        combat::cure_poison(&mut world.state, mob),
        "it had poison to cure"
    );
    assert!(world.registry().get::<Poisoned>(entity).is_none());
}

#[test]
fn poison_survives_a_relogin() {
    // The cheese this closes: log out poisoned, log back in clean, and a relog
    // is a free cure. ServUO keeps the logged-out mobile in-world with the timer
    // still running; this shard saves the effect to the character row instead, so
    // it comes back on the sheet. The same path carries buffs and debuffs later.
    use openshard_state::components::Poisoned;
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let serial = serial_of(&world, conn);

    // Poison the character, then let the save sweep the world.
    let ticks = world.state.ticks;
    combat::apply_poison(
        &mut world.state,
        serial,
        openshard_protocol::world::PoisonLevel::new(2),
        ticks,
    ); // greater
    world.take_snapshot();
    let snapshot = world.drain_saves().next_back().expect("a snapshot");
    let record = snapshot
        .characters
        .iter()
        .find(|c| c.serial == serial)
        .cloned()
        .expect("the character was saved");
    let poison = record
        .effects
        .iter()
        .find(|e| e.kind == openshard_persistence::EFFECT_POISON)
        .expect("the poison went to disk");
    assert_eq!(poison.amount, 2, "at the level it was applied");

    // Relogin, threading the record back through Enter the way the server does.
    world.queue(Command::Disconnect { connection: conn });
    world.tick(now);
    let conn = connection();
    world.queue(Command::Enter(Entering {
        connection: conn,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        // Nothing but the name: the world reads its own roster, which the
        // logout above wrote. Handing the row in would test an unpacking the
        // shard no longer does — see S4 in
        // `docs/server/evidence/2026-07-30-the-connection-state-machine.md`.
        character:  Character::Saved,
    }));
    world.tick(now);

    let player = world.state.players[&conn];
    let poisoned = world
        .registry()
        .get::<Poisoned>(player)
        .expect("still poisoned after the relog — no free cure");
    assert_eq!(
        poisoned.level,
        openshard_protocol::world::PoisonLevel::new(2),
        "and at the same strength"
    );
}

#[test]
fn a_poisoned_creature_comes_back_poisoned() {
    // The mobile half of the same rule: a creature's effects ride the mobile
    // sweep the way its wounds do, so a restart does not cure the region's
    // monsters either.
    use openshard_state::components::Poisoned;
    let now = Instant::now();
    let mut home = world();
    let mob = spawn_mobile_at(&mut home, Point::new(START.x, START.y, 0), 50, now);
    let ticks = home.state.ticks;
    combat::apply_poison(
        &mut home.state,
        mob,
        openshard_protocol::world::PoisonLevel::new(1),
        ticks,
    ); // lesser

    home.take_snapshot();
    let snapshot = home.drain_saves().next_back().expect("a snapshot");
    let mobiles = snapshot.mobiles.expect("a mobile sweep");
    assert!(
        mobiles
            .iter()
            .find(|m| m.serial == mob)
            .expect("the creature was swept")
            .effects
            .iter()
            .any(|e| e.kind == openshard_persistence::EFFECT_POISON),
        "its poison went to disk"
    );

    let mut shard = world();
    let filed = nothing_restored_first(&mut shard);
    shard.restore_mobiles(mobiles, &filed);
    let creature = shard.registry().entity_of(mob).expect("the creature came back");
    assert_eq!(
        shard
            .registry()
            .get::<Poisoned>(creature)
            .expect("still poisoned")
            .level,
        openshard_protocol::world::PoisonLevel::new(1),
    );
}

#[test]
fn a_stat_buff_shifts_stats_and_pools_then_expires() {
    use openshard_state::components::{
        Mana,
        StatEffectKind,
        StatMods,
        Stats,
    };
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);

    let base = *world.registry().get::<Stats>(entity).unwrap();
    let base_hits_max = world.registry().get::<Hitpoints>(entity).unwrap().max;
    let base_mana_max = world.registry().get::<Mana>(entity).unwrap().max;

    // Bless folds into the live stats and the caps that hang off them at once.
    let expires_at = world.state.ticks + 100;
    magic::apply_stat_buff(&mut world.state, serial, StatEffectKind::BLESS, 10, expires_at);
    let blessed = *world.registry().get::<Stats>(entity).unwrap();
    assert_eq!(blessed.strength, base.strength + 10, "str rose");
    assert_eq!(blessed.dexterity, base.dexterity + 10, "dex rose");
    assert_eq!(blessed.intelligence, base.intelligence + 10, "int rose");
    assert_eq!(
        world.registry().get::<Hitpoints>(entity).unwrap().max,
        base_hits_max + 10,
        "the hit-point cap rose with strength"
    );
    assert_eq!(
        world.registry().get::<Mana>(entity).unwrap().max,
        base_mana_max + 10,
        "and the mana cap with intelligence"
    );

    // Run past the expiry: the ledger backs the shift out exactly.
    let mut later = now;
    while world.state.ticks <= expires_at {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert_eq!(
        *world.registry().get::<Stats>(entity).unwrap(),
        base,
        "the stats came back exactly"
    );
    assert_eq!(
        world.registry().get::<Hitpoints>(entity).unwrap().max,
        base_hits_max,
        "and so did the hit-point cap"
    );
    assert!(
        world.registry().get::<StatMods>(entity).is_none(),
        "the emptied ledger was removed"
    );
}

#[test]
fn recasting_a_buff_refreshes_rather_than_stacks() {
    use openshard_state::components::{
        StatEffectKind,
        StatMods,
        Stats,
    };
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let base = *world.registry().get::<Stats>(entity).unwrap();

    let at = world.state.ticks;
    magic::apply_stat_buff(&mut world.state, serial, StatEffectKind::STRENGTH, 5, at + 100);
    magic::apply_stat_buff(&mut world.state, serial, StatEffectKind::STRENGTH, 5, at + 200);

    assert_eq!(
        world.registry().get::<Stats>(entity).unwrap().strength,
        base.strength + 5,
        "a recast refreshes the same +5, it does not stack a second"
    );
    assert_eq!(
        world.registry().get::<StatMods>(entity).unwrap().active.len(),
        1,
        "one entry, not two"
    );
}

#[test]
fn a_debuff_clamps_the_current_pool_to_the_lowered_cap() {
    use openshard_state::components::StatEffectKind;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let full = world.registry().get::<Hitpoints>(entity).unwrap().max;

    // Curse lowers strength, so the hit-point cap drops; a full bar must follow it
    // down rather than sit above the new maximum.
    let at = world.state.ticks;
    magic::apply_stat_buff(&mut world.state, serial, StatEffectKind::CURSE, -10, at + 100);
    let hits = *world.registry().get::<Hitpoints>(entity).unwrap();
    assert_eq!(hits.max, full - 10, "the cap dropped");
    assert_eq!(hits.current, full - 10, "and the full bar dropped with it");
}

#[test]
fn a_stat_buff_survives_a_relogin() {
    // The buff half of the persistence rule: a Bless in flight is saved with the
    // character (its shift folded into the saved stats, its timer on the effects
    // list) and comes back on relog — still buffed, and still counting down to the
    // same base it would have returned to.
    use openshard_state::components::{
        StatEffectKind,
        StatMods,
        Stats,
    };
    use openshard_state::effect;
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let entity = world.state.players[&conn];
    let serial = serial_of(&world, conn);
    let base = *world.registry().get::<Stats>(entity).unwrap();

    let at = world.state.ticks;
    magic::apply_stat_buff(&mut world.state, serial, StatEffectKind::BLESS, 10, at + 100);
    let buffed = *world.registry().get::<Stats>(entity).unwrap();

    world.take_snapshot();
    let snapshot = world.drain_saves().next_back().expect("a snapshot");
    let record = snapshot
        .characters
        .iter()
        .find(|c| c.serial == serial)
        .cloned()
        .expect("saved");
    assert_eq!(record.strength, buffed.strength, "the buffed stat went to disk");
    assert!(
        record.effects.iter().any(|e| e.kind == effect::BLESS),
        "and the buff's ledger entry with it"
    );

    // Relogin, threading the record back the way the server does.
    world.queue(Command::Disconnect { connection: conn });
    world.tick(now);
    let conn = connection();
    world.queue(Command::Enter(Entering {
        connection: conn,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        // Nothing but the name: the world reads its own roster, which the
        // logout above wrote. Handing the row in would test an unpacking the
        // shard no longer does — see S4 in
        // `docs/server/evidence/2026-07-30-the-connection-state-machine.md`.
        character:  Character::Saved,
    }));
    world.tick(now);

    let player = world.state.players[&conn];
    assert_eq!(
        *world.registry().get::<Stats>(player).unwrap(),
        buffed,
        "came back still blessed, not double-applied"
    );
    let expires_at = world
        .registry()
        .get::<StatMods>(player)
        .expect("the ledger was restored")
        .active[0]
        .expires_at;

    // And it still lifts, back to the same base it would have without the relog.
    let mut later = now;
    while world.state.ticks <= expires_at {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert_eq!(
        *world.registry().get::<Stats>(player).unwrap(),
        base,
        "the restored buff wore off to the true base"
    );
}

#[test]
fn reactive_armor_reflects_a_share_of_a_blow_to_the_attacker() {
    // A melee physical blow on a mobile wearing Reactive Armor bounces a share
    // back at the swinger — read at the one damage door, off the buff's percent.
    let now = Instant::now();
    let mut world = world();
    let victim = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 100, now);
    let attacker = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 100, now);
    let victim_entity = entity(&world, victim);
    let attacker_entity = entity(&world, attacker);
    let until = world.state.ticks + 1000;
    magic::apply_behaviour_buff(
        &mut world.state,
        victim,
        openshard_state::BehaviourBuffKind::REACTIVE_ARMOR,
        50, // half
        until,
    );
    let attacker_before = world
        .registry()
        .get::<Hitpoints>(attacker_entity)
        .unwrap()
        .current;

    combat::damage(
        &mut world.state,
        victim,
        20,
        openshard_state::DamageType::Physical,
        Some(attacker),
    );

    assert_eq!(
        world.registry().get::<Hitpoints>(victim_entity).unwrap().current,
        80,
        "the victim took the whole blow"
    );
    assert_eq!(
        attacker_before
            - world
                .registry()
                .get::<Hitpoints>(attacker_entity)
                .unwrap()
                .current,
        10,
        "and half of it bounced back at the attacker"
    );
}

#[test]
fn reactive_armor_does_not_ping_pong() {
    // Both sides armored: the reflected blow is unattributed, so it cannot reflect
    // a second time — no infinite bounce.
    let now = Instant::now();
    let mut world = world();
    let victim = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 100, now);
    let attacker = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 100, now);
    let victim_entity = entity(&world, victim);
    let attacker_entity = entity(&world, attacker);
    let at = world.state.ticks + 1000;
    magic::apply_behaviour_buff(
        &mut world.state,
        victim,
        openshard_state::BehaviourBuffKind::REACTIVE_ARMOR,
        50,
        at,
    );
    magic::apply_behaviour_buff(
        &mut world.state,
        attacker,
        openshard_state::BehaviourBuffKind::REACTIVE_ARMOR,
        50,
        at,
    );

    combat::damage(
        &mut world.state,
        victim,
        20,
        openshard_state::DamageType::Physical,
        Some(attacker),
    );

    assert_eq!(
        world.registry().get::<Hitpoints>(victim_entity).unwrap().current,
        80,
        "the victim took the blow but no bounce came back"
    );
    assert_eq!(
        world
            .registry()
            .get::<Hitpoints>(attacker_entity)
            .unwrap()
            .current,
        90,
        "the attacker took only the one reflected hit"
    );
}

#[test]
fn protection_holds_a_cast_against_a_blow() {
    // Protection is the chance a blow does not break concentration. With a certain
    // chance, a hit mid-cast leaves the Casting standing where it would otherwise
    // fizzle (compare `a_blow_disturbs_a_cast_when_the_shard_says_so`).
    use openshard_state::components::Casting;
    let now = Instant::now();
    let mut world = world(); // spell_disturb on by default
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);
    let serial = serial_of(&world, connection);
    let until = world.state.ticks + 1000;
    magic::apply_behaviour_buff(
        &mut world.state,
        serial,
        openshard_state::BehaviourBuffKind::PROTECTION,
        100, // certain
        until,
    );
    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    });
    world.tick(now);
    assert!(world.registry().get::<Casting>(entity).is_some());

    world.queue(Command::Damage {
        serial,
        amount: 5,
        damage_type: 0,
        by: None,
    });
    world.tick(now);
    assert!(
        world.registry().get::<Casting>(entity).is_some(),
        "protection held the concentration"
    );
}

#[test]
fn magic_reflection_bounces_a_spell_back_at_the_caster() {
    // An offensive spell aimed at a mobile carrying Magic Reflection lands on the
    // caster instead, and the buff is spent doing it.
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, caster) = ready_caster(&mut world, BLACK_PEARL, now);
    let target = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let target_entity = entity(&world, target);
    let until = world.state.ticks + 1000;
    magic::apply_behaviour_buff(
        &mut world.state,
        target,
        openshard_state::BehaviourBuffKind::MAGIC_REFLECT,
        0,
        until,
    );
    let caster_before = world.registry().get::<Hitpoints>(caster).unwrap().current;

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    }); // Fireball
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(target),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert!(
        world.registry().get::<Hitpoints>(caster).unwrap().current < caster_before,
        "the caster took his own fireball"
    );
    assert_eq!(
        world.registry().get::<Hitpoints>(target_entity).unwrap().current,
        50,
        "the reflecting target was untouched"
    );
    assert!(
        magic::behaviour_buff(
            &world.state,
            target_entity,
            openshard_state::BehaviourBuffKind::MAGIC_REFLECT
        )
        .is_none(),
        "and the reflect was spent"
    );
}

#[test]
fn night_sight_lights_the_targets_screen() {
    // Night Sight sends the caster its personal light (0x4F, level 0 — brightest).
    // Reagents off so the one-reagent test caster can cast a two-reagent spell.
    use openshard_state::components::BehaviourBuffs;
    let now = Instant::now();
    let mut world = World::new(START).with_gameplay(Gameplay {
        cast_style: openshard_state::CastStyle::Walk,
        reagents: false,
        ..Default::default()
    });
    let (connection, caster) = ready_caster(&mut world, BLACK_PEARL, now);
    let caster_serial = serial_of(&world, connection);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(5),
    }); // Night Sight
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(caster_serial),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p.as_slice() == [0x4F, 0]),
        "the overall-light packet lit the screen"
    );
    assert!(
        world.registry().get::<BehaviourBuffs>(caster).is_some(),
        "and the buff is on the mobile"
    );
}

#[test]
fn a_behaviour_buff_expires_on_its_tick_and_a_recast_refreshes() {
    use openshard_state::components::BehaviourBuffs;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);

    let at = world.state.ticks;
    magic::apply_behaviour_buff(
        &mut world.state,
        serial,
        openshard_state::BehaviourBuffKind::REACTIVE_ARMOR,
        30,
        at + 50,
    );
    magic::apply_behaviour_buff(
        &mut world.state,
        serial,
        openshard_state::BehaviourBuffKind::REACTIVE_ARMOR,
        40,
        at + 100,
    );
    assert_eq!(
        world
            .registry()
            .get::<BehaviourBuffs>(entity)
            .unwrap()
            .active
            .len(),
        1,
        "a recast refreshes rather than stacking a second entry"
    );
    assert_eq!(
        magic::behaviour_buff(
            &world.state,
            entity,
            openshard_state::BehaviourBuffKind::REACTIVE_ARMOR
        ),
        Some(40),
        "and it is the fresh magnitude and timer"
    );

    let mut later = now;
    while world.state.ticks <= at + 100 {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        world.registry().get::<BehaviourBuffs>(entity).is_none(),
        "the buff lifted on its tick"
    );
}

#[test]
fn a_behaviour_buff_survives_a_relogin() {
    // The non-stat buffs ride the same effects list a poison or a Bless does: saved
    // with the character, restored on relog, still counting down.
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let serial = serial_of(&world, conn);
    let at = world.state.ticks;
    magic::apply_behaviour_buff(
        &mut world.state,
        serial,
        openshard_state::BehaviourBuffKind::REACTIVE_ARMOR,
        40,
        at + 500,
    );

    world.take_snapshot();
    let snapshot = world.drain_saves().next_back().expect("a snapshot");
    let record = snapshot
        .characters
        .iter()
        .find(|c| c.serial == serial)
        .cloned()
        .expect("saved");
    assert!(
        record
            .effects
            .iter()
            .any(|e| e.kind == openshard_state::BehaviourBuffKind::REACTIVE_ARMOR.as_u8() && e.amount == 40),
        "the buff went to disk"
    );

    world.queue(Command::Disconnect { connection: conn });
    world.tick(now);
    let conn = connection();
    world.queue(Command::Enter(Entering {
        connection: conn,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        // Nothing but the name: the world reads its own roster, which the
        // logout above wrote. Handing the row in would test an unpacking the
        // shard no longer does — see S4 in
        // `docs/server/evidence/2026-07-30-the-connection-state-machine.md`.
        character:  Character::Saved,
    }));
    world.tick(now);

    let player = world.state.players[&conn];
    assert_eq!(
        magic::behaviour_buff(
            &world.state,
            player,
            openshard_state::BehaviourBuffKind::REACTIVE_ARMOR
        ),
        Some(40),
        "and came back on relog"
    );
}

/// A world whose spells cast Sphere-style with reagents off — the field spells
/// carry several reagents the one-reagent test caster does not stock.
fn field_world() -> World {
    World::new(START).with_gameplay(Gameplay {
        cast_style: openshard_state::CastStyle::Walk,
        reagents: false,
        ..Default::default()
    })
}

#[test]
fn fire_field_lays_a_row_and_burns_who_stands_in_it() {
    use openshard_state::components::Field;
    let now = Instant::now();
    let mut world = field_world();
    let (connection, _caster) = ready_caster(&mut world, BLACK_PEARL, now);
    let spot = Point::new(START.x + 3, START.y, 0);
    let victim = spawn_mobile_at(&mut world, spot, 50, now);
    let victim_entity = entity(&world, victim);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(27),
    }); // Fire Field
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  spot,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert_eq!(
        world.state.registry.query::<Field>().count(),
        5,
        "a five-tile row was laid"
    );

    // Burn for a couple of seconds — two pulses at a one-second cadence.
    let mut later = now;
    for _ in 0..45 {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        world.registry().get::<Hitpoints>(victim_entity).unwrap().current < 50,
        "the fire field burned who stood in it"
    );
}

#[test]
fn poison_field_poisons_who_stands_in_it() {
    use openshard_state::components::Poisoned;
    let now = Instant::now();
    let mut world = field_world();
    let (connection, _caster) = ready_caster(&mut world, BLACK_PEARL, now);
    let spot = Point::new(START.x + 3, START.y, 0);
    let victim = spawn_mobile_at(&mut world, spot, 50, now);
    let victim_entity = entity(&world, victim);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(38),
    }); // Poison Field
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  spot,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    // The poison pulse lands a second and a half in, so two seconds sees it.
    let mut later = now;
    for _ in 0..Gameplay::ticks(2) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        world.registry().get::<Poisoned>(victim_entity).is_some(),
        "the poison field poisoned who stood in it"
    );
}

#[test]
fn wall_of_stone_blocks_the_way_then_clears() {
    use openshard_state::FieldKind;
    use openshard_state::components::Field;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let caster = world.state.players[&connection];
    // Aim due south: the line of fire runs north–south, so the wall runs east–west.
    let spot = Point::new(START.x, START.y + 3, 0);
    world.lay_field(caster, FieldKind::Stone, spot);

    assert!(
        world
            .state
            .facet_state(Facet(0))
            .obstructions()
            .holds_anything(spot.x, spot.y),
        "the wall blocks its centre tile"
    );

    // Force expiry rather than run ten seconds of ticks.
    let soon = world.state.ticks + 1;
    let tiles: Vec<EntityId> = world.state.registry.query::<Field>().map(|(e, _)| e).collect();
    for e in tiles {
        let mut field = world.state.registry.get::<Field>(e).copied().unwrap();
        field.expires_at = soon;
        world.state.registry.insert(e, field);
    }
    world.tick(now + TICK_INTERVAL);

    assert_eq!(
        world.state.registry.query::<Field>().count(),
        0,
        "the tiles are gone"
    );
    assert!(
        !world
            .state
            .facet_state(Facet(0))
            .obstructions()
            .holds_anything(spot.x, spot.y),
        "and the way is free again"
    );
}

#[test]
fn a_field_row_lies_perpendicular_to_the_line_of_fire() {
    use openshard_state::FieldKind;
    use openshard_state::components::Field;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now); // caster at START
    let caster = world.state.players[&connection];
    // Aim due east: the row should run north–south (vary Y, share X).
    let spot = Point::new(START.x + 5, START.y, 0);
    world.lay_field(caster, FieldKind::Fire, spot);

    let tiles: Vec<Point> = world
        .state
        .registry
        .query::<Field>()
        .filter_map(|(e, _)| world.state.registry.get::<Position>(e).map(|p| p.0))
        .collect();
    assert_eq!(tiles.len(), 5);
    assert!(
        tiles.iter().all(|p| p.x == spot.x),
        "every tile shares the caster–target X axis"
    );
    assert!(
        tiles.iter().any(|p| p.y == spot.y - 2) && tiles.iter().any(|p| p.y == spot.y + 2),
        "and the row spreads along Y, perpendicular to the line of fire"
    );
}

#[test]
fn a_field_tile_is_not_saved() {
    use openshard_state::FieldKind;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let caster = world.state.players[&connection];
    world.lay_field(caster, FieldKind::Fire, Point::new(START.x + 3, START.y, 0));

    world.take_snapshot();
    let snapshot = world.drain_saves().next_back().expect("a snapshot");
    assert!(
        snapshot.ground.unwrap_or_default().is_empty(),
        "a transient field tile is not swept into the save"
    );
}

#[test]
fn a_frozen_player_cannot_walk_and_can_again_when_it_lifts() {
    use openshard_state::components::Frozen;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let Position(start) = *world.registry().get::<Position>(entity).unwrap();
    let until = world.state.ticks + 5;
    world.state.registry.insert(entity, Frozen { until });
    let _ = world.drain_outbound().count();

    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(now);

    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        start,
        "a frozen player does not move"
    );
    assert!(
        world
            .drain_outbound()
            .any(|out| out.packet.first() == Some(&0x21)),
        "and gets a walk reject"
    );

    // Run past the freeze, then the same step lands.
    let mut later = now;
    while world.state.ticks <= until {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    let _ = world.drain_outbound().count();
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(later);
    assert_ne!(
        world.registry().get::<Position>(entity).unwrap().0,
        start,
        "once it lifts, the player walks"
    );
}

#[test]
fn a_frozen_creature_does_not_step() {
    use openshard_state::components::Frozen;
    let now = Instant::now();
    let mut world = world();
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 5, START.y, 0), 50, now);
    let entity = entity(&world, mob);
    // Face it south first (turn-as-step), so a further south step would move it.
    world.queue(Command::Step {
        serial:    mob,
        direction: Direction::South.to_bits(),
    });
    world.tick(now);
    let Position(before) = *world.registry().get::<Position>(entity).unwrap();

    world.state.registry.insert(
        entity,
        Frozen {
            until: world.state.ticks + 100,
        },
    );
    world.queue(Command::Step {
        serial:    mob,
        direction: Direction::South.to_bits(),
    });
    world.tick(now);
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        before,
        "a frozen creature stays put"
    );

    world.state.registry.remove::<Frozen>(entity);
    world.queue(Command::Step {
        serial:    mob,
        direction: Direction::South.to_bits(),
    });
    world.tick(now);
    assert_ne!(
        world.registry().get::<Position>(entity).unwrap().0,
        before,
        "thawed, it moves"
    );
}

#[test]
fn the_paralyze_spell_freezes_its_target() {
    use openshard_state::components::Frozen;
    let now = Instant::now();
    let mut world = field_world(); // reagents off, so the one-reagent caster can cast it
    let (connection, _caster) = ready_caster(&mut world, BLACK_PEARL, now);
    let victim = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let victim_entity = entity(&world, victim);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(37),
    }); // Paralyze
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(victim),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert!(
        world.registry().get::<Frozen>(victim_entity).is_some(),
        "the target is paralyzed"
    );
}

#[test]
fn a_blow_breaks_paralysis() {
    use openshard_state::components::Frozen;
    let now = Instant::now();
    let mut world = world();
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let entity = entity(&world, mob);
    world.state.registry.insert(
        entity,
        Frozen {
            until: world.state.ticks + 100,
        },
    );

    combat::damage(
        &mut world.state,
        mob,
        5,
        openshard_state::DamageType::Physical,
        None,
    );

    assert!(
        world.registry().get::<Frozen>(entity).is_none(),
        "a blow wakes a paralyzed mobile"
    );
}

#[test]
fn paralysis_lifts_on_its_tick() {
    use openshard_state::components::Frozen;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let until = world.state.ticks + 5;
    world.state.registry.insert(entity, Frozen { until });

    let mut later = now;
    while world.state.ticks <= until {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        world.registry().get::<Frozen>(entity).is_none(),
        "the paralysis lifted on its tick"
    );
}

#[test]
fn paralysis_survives_a_relogin() {
    use openshard_state::components::Frozen;
    use openshard_state::effect;
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let entity = world.state.players[&conn];
    let serial = serial_of(&world, conn);
    let until = world.state.ticks + 500;
    world.state.registry.insert(entity, Frozen { until });

    world.take_snapshot();
    let snapshot = world.drain_saves().next_back().expect("a snapshot");
    let record = snapshot
        .characters
        .iter()
        .find(|c| c.serial == serial)
        .cloned()
        .expect("saved");
    assert!(
        record.effects.iter().any(|e| e.kind == effect::PARALYZE),
        "the paralysis went to disk"
    );

    world.queue(Command::Disconnect { connection: conn });
    world.tick(now);
    let conn = connection();
    world.queue(Command::Enter(Entering {
        connection: conn,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        // Nothing but the name: the world reads its own roster, which the
        // logout above wrote. Handing the row in would test an unpacking the
        // shard no longer does — see S4 in
        // `docs/server/evidence/2026-07-30-the-connection-state-machine.md`.
        character:  Character::Saved,
    }));
    world.tick(now);

    let player = world.state.players[&conn];
    assert!(
        world.registry().get::<Frozen>(player).is_some(),
        "and came back on relog, still frozen"
    );
}

#[test]
fn paralyze_field_freezes_who_stands_in_it() {
    use openshard_state::FieldKind;
    use openshard_state::components::Frozen;
    assert!(
        !FieldKind::Paralyze.blocks(),
        "you must be able to step onto a paralyze field to be caught"
    );
    let now = Instant::now();
    let mut world = field_world();
    let (connection, _caster) = ready_caster(&mut world, BLACK_PEARL, now);
    let spot = Point::new(START.x + 3, START.y, 0);
    let victim = spawn_mobile_at(&mut world, spot, 50, now);
    let victim_entity = entity(&world, victim);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(46),
    }); // Paralyze Field
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  spot,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    // The pulse catches who stands on it within a beat, and a second is one.
    let mut later = now;
    for _ in 0..Gameplay::ticks(1) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        world.registry().get::<Frozen>(victim_entity).is_some(),
        "the field froze who stood in it"
    );
}

/// Summon Creature, the self-cast summon: it needs no cursor, so one tick of a
/// Sphere-style world is the whole cast.
const SUMMON_CREATURE: SpellId = SpellId(39);

/// The summon standing in the world, or `None`. There is at most one in these
/// tests, and finding it by its marker rather than by a serial the cast never
/// handed back is what makes them read.
fn the_summon(world: &World) -> Option<EntityId> {
    world
        .registry()
        .query::<openshard_state::components::Summoned>()
        .next()
        .map(|(entity, _)| entity)
}

#[test]
fn a_summoned_creature_is_its_casters_follower_for_a_magery_scaled_while() {
    use openshard_state::components::{
        Pet,
        Summoned,
    };
    let now = Instant::now();
    let mut world = field_world();
    // `ready_caster` sets Magery to grandmaster (1000 tenths), which is what the
    // four-hundred-second span below is worked out from.
    let (connection, caster) = ready_caster(&mut world, BLACK_PEARL, now);
    let caster_serial = serial_of(&world, connection);

    world.queue(Command::RequestCast {
        connection,
        spell: SUMMON_CREATURE,
    });
    world.tick(now);

    let summon = the_summon(&world).expect("the spell called something up");
    assert_eq!(
        world.registry().get::<Pet>(summon).map(|pet| pet.owner),
        Some(caster_serial),
        "a summon is its caster's, by the one path a creature becomes somebody's"
    );
    assert_eq!(
        openshard_skills::followers_of(&world.state, caster),
        2,
        "and it fills the two follower slots its spell demanded be free"
    );
    // ServUO's `(2 * Magery.Fixed) / 5` seconds: four hundred at grandmaster. Pinned
    // because it is the only thing skill buys a summon — the stat block is the same
    // for a novice — so a wrong divisor here is invisible from every other angle.
    assert_eq!(
        world.registry().get::<Summoned>(summon).unwrap().expires_at - world.state.ticks,
        400 * TICKS_PER_SECOND,
        "grandmaster Magery buys four hundred seconds"
    );
}

#[test]
fn a_summon_goes_when_its_span_runs_out() {
    // Driven through `npc::summon` rather than a cast, because the span is
    // Magery-scaled and a caster with no skill sheet at all gets the one-second
    // floor — forty ticks to wait out instead of sixteen thousand. The floor is
    // worth exercising in its own right: without it a skill-less summon would
    // expire on the tick it appeared.
    let now = Instant::now();
    let mut world = world();
    let at = Point::new(START.x, START.y, 0);
    let caster_serial = spawn_mobile_at(&mut world, at, 50, now);
    let caster = entity(&world, caster_serial);
    let summoned = openshard_npc::summon(
        &mut world.state,
        caster,
        openshard_state::SummonKind::Creature,
        at,
    )
    .expect("a creature was called up");
    assert!(world.registry().serial_of(summoned).is_some());

    let mut later = now;
    for _ in 0..Gameplay::ticks(2) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        the_summon(&world).is_none(),
        "the summon went out on its own timer"
    );
    assert!(
        world.registry().serial_of(summoned).is_none(),
        "and took its whole mobile with it"
    );
}

#[test]
fn a_summon_killed_leaves_no_corpse() {
    // Pre-AoS ServUO deletes a summon's corpse (`DeleteCorpseOnDeath`), and here it
    // is never made. Not cosmetic: a corpse is filled by `fill_creature_loot`, whose
    // gold baseline scales with the dead thing's hit points, so a conjured creature
    // that could be killed for loot would be a coin press.
    let now = Instant::now();
    let mut world = field_world();
    let (connection, _caster) = ready_caster(&mut world, BLACK_PEARL, now);
    world.queue(Command::RequestCast {
        connection,
        spell: SUMMON_CREATURE,
    });
    world.tick(now);
    let summon = the_summon(&world).expect("the spell called something up");
    let serial = world.registry().serial_of(summon).unwrap();
    let corpses_before = world.registry().query::<Corpse>().count();

    world.queue(Command::Damage {
        serial,
        amount: 5000,
        damage_type: 0,
        by: None,
    });
    world.tick(now);

    assert!(the_summon(&world).is_none(), "the summon died");
    assert_eq!(
        world.registry().query::<Corpse>().count(),
        corpses_before,
        "and left nothing behind to loot"
    );
}

#[test]
fn the_follower_cap_refuses_a_summon_there_is_no_room_for() {
    // ServUO's per-summon `CheckCast`. Two beasts fill four of the five slots; the
    // third would want six, and the refusal must cost nothing — a mage charged
    // eighth-circle mana to be told the daemon will not fit has paid for a "no".
    let now = Instant::now();
    let mut world = field_world();
    let (connection, caster) = ready_caster(&mut world, BLACK_PEARL, now);
    for _ in 0..2 {
        world.queue(Command::RequestCast {
            connection,
            spell: SUMMON_CREATURE,
        });
        world.tick(now);
    }
    assert_eq!(
        openshard_skills::followers_of(&world.state, caster),
        4,
        "two beasts at two slots each"
    );
    let standing = world
        .registry()
        .query::<openshard_state::components::Summoned>()
        .count();
    let mana_before = world.registry().get::<Mana>(caster).unwrap().current;

    world.queue(Command::RequestCast {
        connection,
        spell: SUMMON_CREATURE,
    });
    world.tick(now);

    assert_eq!(
        world
            .registry()
            .query::<openshard_state::components::Summoned>()
            .count(),
        standing,
        "the third summon was refused"
    );
    assert_eq!(
        world.registry().get::<Mana>(caster).unwrap().current,
        mana_before,
        "and refused before a point of mana"
    );
}

#[test]
fn a_summon_is_not_written_down() {
    // On the field tile's and the spell gate's own terms: restored, a five-minute
    // creature becomes a permanent one whose caster no longer exists, standing as
    // somebody's pet against a follower cap nothing will ever free.
    let now = Instant::now();
    let mut world = field_world();
    let (connection, _caster) = ready_caster(&mut world, BLACK_PEARL, now);
    world.queue(Command::RequestCast {
        connection,
        spell: SUMMON_CREATURE,
    });
    world.tick(now);
    let summon = the_summon(&world).expect("the spell called something up");
    let serial = world.registry().serial_of(summon).unwrap();

    assert!(
        world
            .mobile_records()
            .iter()
            .all(|record| record.serial != serial),
        "the save sweep passed the summon over"
    );
}

#[test]
fn the_bless_spell_raises_the_targets_stats() {
    use openshard_state::components::Stats;
    const GARLIC: u16 = 0x0F84;
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, entity) = ready_caster(&mut world, GARLIC, now);
    let self_serial = serial_of(&world, connection);
    let backpack = backpack_serial(&world, connection);
    assert!(
        openshard_items::give(
            &mut world.state,
            backpack,
            openshard_protocol::wire::Graphic(MANDRAKE_ROOT),
            openshard_protocol::wire::Hue(0),
            20,
        )
        .is_complete()
    );
    let base = *world.registry().get::<Stats>(entity).unwrap();

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(16),
    }); // Bless
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(self_serial),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert!(
        world.registry().get::<Stats>(entity).unwrap().strength > base.strength,
        "the Bless spell raised the target's stats through the full cast"
    );
}

#[test]
fn the_poison_spell_poisons_what_it_is_aimed_at() {
    use openshard_state::components::Poisoned;
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, _) = ready_caster(&mut world, NIGHTSHADE, now);
    let target = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(19),
    }); // Poison
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(target),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    let entity = world.registry().entity_of(target).unwrap();
    assert!(
        world.registry().get::<Poisoned>(entity).is_some(),
        "the Poison spell left its mark"
    );
}

#[test]
fn a_resolved_cast_plays_its_sound_and_shows_its_bolt() {
    // The most visible gap against a real client, closed: a spell that lands is
    // no longer silent and invisible. Fireball plays its 0x54 sound and flings a
    // 0x70 graphical effect at the mark, both reaching the caster.
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, _) = ready_caster(&mut world, BLACK_PEARL, now);
    let target = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let _ = packets_for(&mut world, connection); // drain the setup burst

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    }); // Fireball
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(target),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    let packets = packets_for(&mut world, connection);
    assert!(packets.iter().any(|p| p[0] == 0x54), "the cast plays a sound");
    assert!(packets.iter().any(|p| p[0] == 0x70), "and flings a visible bolt");
    assert!(
        packets.iter().any(|p| p[0] == 0xE2),
        "and the caster gestures — a 0xE2 animation to the modern client"
    );
}

/// Spending mana puts the new pool on the wire.
///
/// It did not. `Mana` was mutated in place by the cast and nothing was sent, so
/// the blue line under the character read whatever the last `0x11` had said — and
/// `refresh_statuses` only sends one when an *inventory-derived* number moves, so
/// a mage could empty the pool in a fight and watch a full bar the whole time.
#[test]
fn casting_sends_the_client_its_new_mana_pool() {
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);
    let before = world.registry().get::<Mana>(entity).unwrap().current;
    let _ = packets_for(&mut world, connection); // drain the setup burst

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    }); // Fireball, nine mana at the third circle
    world.tick(now);

    let after = world.registry().get::<Mana>(entity).unwrap().current;
    assert!(after < before, "the cast paid for itself");
    let packets = packets_for(&mut world, connection);
    let bar = packets
        .iter()
        .rev()
        .find(|packet| packet[0] == 0xA2)
        .expect("the cast sent a 0xA2 mana bar");
    assert_eq!(
        u16::from_be_bytes([bar[7], bar[8]]),
        after,
        "and the bar carries the pool the world now holds, unscaled"
    );
}

/// And so does getting it back: the trickle is the other half of the same bar,
/// and a pool that only ever *falls* on screen is no better than one that never
/// moves.
#[test]
fn the_mana_trickle_reaches_the_client_too() {
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);
    let Mana { max, .. } = *world.registry().get::<Mana>(entity).unwrap();
    world.state.registry.insert(
        entity,
        Mana {
            current: max - 1,
            max,
        },
    );
    // The regen is stateless: a mobile gets a point when the tick counter divides
    // its own rate. So put the counter on a multiple of that rate rather than
    // ticking a world for the seconds it would otherwise take.
    let rate = openshard_magic::mana_regen_ticks(&world.state, entity);
    world.state.ticks = openshard_state::WorldTick::ZERO + rate;
    let _ = packets_for(&mut world, connection);

    openshard_magic::regen_mana(&mut world.state);

    assert_eq!(
        world.registry().get::<Mana>(entity).unwrap().current,
        max,
        "the trickle gave the point back"
    );
    let packets = packets_for(&mut world, connection);
    let bar = packets
        .iter()
        .find(|packet| packet[0] == 0xA2)
        .expect("the trickle sent a 0xA2 too");
    assert_eq!(u16::from_be_bytes([bar[7], bar[8]]), max);
}

/// Casting is a revealing act, and a disruptive one.
///
/// ServUO's `Spell.Cast` calls `RevealingAction` the moment the state turns to
/// casting, and that call ends in a `DisruptiveAction`. Neither happened here: a
/// hidden mage stayed hidden through a fireball, and a meditating one cast out of
/// the trance and went on regenerating at twice the rate.
#[test]
fn a_cast_gives_a_hidden_mage_away_and_ends_his_trance() {
    use openshard_state::components::{
        Hidden,
        Meditating,
    };
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);
    world.state.registry.insert(entity, Hidden);
    world.state.registry.insert(entity, Meditating);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    });
    world.tick(now);

    assert!(
        !world.registry().has::<Hidden>(entity),
        "the cast gave the caster away"
    );
    assert!(
        !world.registry().has::<Meditating>(entity),
        "and broke the trance rather than casting out of it"
    );
}

/// A refused cast costs nothing — including the hiding.
///
/// The reveal sits *after* the free refusals for the reference's reason: a spell
/// the book does not hold was never begun, so it cannot be the thing that gives a
/// hidden thief away.
#[test]
fn a_spell_the_book_does_not_hold_does_not_reveal_the_caster() {
    use openshard_state::components::Hidden;
    let now = Instant::now();
    let mut world = sphere_world();
    // Enter with reagents and skill but no spellbook at all.
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    world.state.registry.insert(entity, Hidden);

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    });
    world.tick(now);

    assert!(
        world.registry().has::<Hidden>(entity),
        "a cast that never began reveals nobody"
    );
}

/// The Resisting-Spells curve, against a live skill sheet.
///
/// ServUO's `GetResistPercentForCircle`, in tenths of a per-cent. The numbers are
/// hand-evaluated from the reference expression, and the shape they pin is the
/// point: the *contested* reading falls with the caster's Magery and the circle,
/// while the flat `resist / 5` is a floor that a novice keeps against an
/// eighth-circle spell.
#[test]
fn the_resist_chance_follows_the_reference_curve() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let caster = world.state.players[&connection];
    let target_serial = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let target = entity(&world, target_serial);
    let caster_serial = serial_of(&world, connection);
    // The wire's own skill ids: 25 Magery, 26 Resisting Spells.
    let set = |world: &mut World, serial, skill, value| {
        world.queue(Command::SetSkill { serial, skill, value });
        world.tick(now);
    };
    let circle = |n: u8| openshard_magic::SpellCircle::new(n).unwrap();

    // A grandmaster warder against a grandmaster's eighth circle: the contested
    // reading is 1000 - ((1000-200)/5 + 8*50) = 440, halved 220 — 22%.
    set(&mut world, caster_serial, 25, 1000);
    set(&mut world, target_serial, 26, 1000);
    assert_eq!(
        openshard_magic::resist_chance(&world.state, caster, target, circle(8)),
        220
    );
    // The same pair at the first circle: 1000 - (160 + 50) = 790, halved 395.
    assert_eq!(
        openshard_magic::resist_chance(&world.state, caster, target, circle(1)),
        395
    );
    // A novice warder against that eighth circle: the contested reading goes
    // negative, and the flat fifth halved — one per-cent — is what is left.
    set(&mut world, target_serial, 26, 100);
    assert_eq!(
        openshard_magic::resist_chance(&world.state, caster, target, circle(8)),
        10
    );
    // No skill, no resist.
    set(&mut world, target_serial, 26, 0);
    assert_eq!(
        openshard_magic::resist_chance(&world.state, caster, target, circle(1)),
        0
    );
}

/// Resisting Spells softens what lands — the read site the skill never had.
///
/// The skill sat in the table, on the trainers' lists and in every saved sheet
/// while nothing anywhere consulted it. Two casts of the same bolt at two targets
/// that differ only in that skill: the warder takes strictly less over a run, and
/// the target with no skill takes the full bolt every single time, which is the
/// half of this that is exact rather than statistical.
#[test]
fn resisting_spells_softens_a_bolt_and_no_skill_resists_nothing() {
    const CASTS: u16 = 30;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let caster = world.state.players[&connection];
    let caster_serial = serial_of(&world, connection);
    // Flamestrike: seventh circle, 28 fire damage before any resistance.
    let spell = SpellId(50);
    let base = u32::from(CASTS) * 28;

    let hammer = |world: &mut World, resist: u16| -> u32 {
        let victim = spawn_mobile_at(world, Point::new(START.x + 1, START.y, 0), 30_000, now);
        let victim_entity = entity(world, victim);
        world.queue(Command::SetSkill {
            serial: victim,
            skill:  26, // Resisting Spells
            value:  resist,
        });
        world.tick(now);
        // No `Resistance` component, so the bolt arrives at its face value and the
        // only thing between it and the hit points is the roll under test.
        let before = world.registry().get::<Hitpoints>(victim_entity).unwrap().current;
        for _ in 0..CASTS {
            world.apply_spell_effect(caster, spell, Some(victim), Point::new(START.x + 1, START.y, 0));
        }
        let after = world.registry().get::<Hitpoints>(victim_entity).unwrap().current;
        u32::from(before - after)
    };

    // A caster with no Magery at all, which is the *worst* case for the contested
    // reading and so the best for the warder: a grandmaster resists just under half
    // of these. Thirty casts without a single one is a 1-in-a-billion run.
    let _ = caster_serial;
    let warded = hammer(&mut world, 1000);
    let bare = hammer(&mut world, 0);

    assert_eq!(
        bare, base,
        "a target with no Resisting Spells takes every point of every bolt"
    );
    assert!(
        warded < bare,
        "a grandmaster warder takes less over {CASTS} bolts: {warded} against {bare}"
    );
}

#[test]
fn a_cast_without_reagents_fizzles() {
    let now = Instant::now();
    let mut world = sphere_world();
    // Enter without stocking the pack — no reagents.
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    world.queue(Command::SetSkill {
        serial,
        skill: 25,
        value: 1000,
    });
    world.tick(now);
    let mana_before = world.registry().get::<Mana>(entity).unwrap().current;

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    });
    world.tick(now);
    assert_eq!(
        world.registry().get::<Mana>(entity).unwrap().current,
        mana_before,
        "a fizzle for want of a reagent spends nothing"
    );
}

#[test]
fn with_reagents_off_a_cast_needs_no_reagents() {
    // The [gameplay] reagents = false knob (Sphere's no-reagent shards): a spell
    // casts from mana alone, with an empty pack, where the default would fizzle
    // (see the test above). The mana being spent is proof the cast proceeded.
    let gameplay = Gameplay {
        cast_style: openshard_state::CastStyle::Walk,
        reagents: false,
        ..Default::default()
    };
    let now = Instant::now();
    let mut world = World::new(START).with_gameplay(gameplay);
    let connection = enter(&mut world, now); // no reagents stocked
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    world.queue(Command::SetSkill {
        serial,
        skill: 25,
        value: 1000,
    });
    world.tick(now);
    give_full_spellbook(&mut world, connection); // the cast gate, but no reagents
    let mana_before = world.registry().get::<Mana>(entity).unwrap().current;

    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    }); // Fireball — normally needs a black pearl
    world.tick(now);
    assert!(
        world.registry().get::<Mana>(entity).unwrap().current < mana_before,
        "with reagents off, the cast paid its mana and proceeded despite an empty pack"
    );
}

#[test]
fn mana_loss_on_fail_off_refunds_a_fizzle() {
    // Sphere's ManaLossFail, confirmed and made a knob: mana is spent at
    // resolution, and with mana_loss_on_fail = false a *failed* cast keeps it. The
    // test is outcome-agnostic — whichever way the seeded roll lands, the rule
    // holds: mana is spent exactly when the cast succeeds, never on a fizzle here.
    let gameplay = Gameplay {
        cast_style: openshard_state::CastStyle::Walk,
        reagents: false, // isolate mana — no reagent fizzle in the way
        mana_loss_on_fail: false,
        ..Default::default()
    };
    let now = Instant::now();
    let mut world = World::new(START).with_gameplay(gameplay);
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let target = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    // A middling skill, so the roll can plausibly go either way.
    world.queue(Command::SetSkill {
        serial,
        skill: 25,
        value: 300,
    });
    world.tick(now);
    give_full_spellbook(&mut world, connection); // the cast gate
    let mana_before = world.registry().get::<Mana>(entity).unwrap().current;

    let mut cast: Cursor<SpellCast> = world.bus().cursor();
    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(17),
    }); // Fireball, a targeted spell
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    Some(target),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    let succeeded = world.bus().read(&mut cast).any(|e| e.success);
    let mana_after = world.registry().get::<Mana>(entity).unwrap().current;
    if succeeded {
        assert!(mana_after < mana_before, "a successful cast spends its mana");
    } else {
        assert_eq!(
            mana_after, mana_before,
            "a fizzle with mana_loss_on_fail off keeps the mana"
        );
    }
}

#[test]
fn using_a_skill_announces_the_outcome() {
    // A grandmaster (100.0) at a trivial task always succeeds, and the event
    // carries the result for a script to reward.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    world.queue(Command::SetSkill {
        serial,
        skill: 1,
        value: 1000,
    });
    world.tick(now);

    let mut used: Cursor<SkillUsed> = world.bus().cursor();
    world.queue(Command::UseSkill {
        serial,
        skill: 1,
        min_skill: 0,
        max_skill: 500,
    });
    world.tick(now);

    let events: Vec<SkillUsed> = world.bus().read(&mut used).copied().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].skill, Skill::Anatomy);
    assert!(events[0].success, "a sure thing succeeds");
}

#[test]
fn a_skill_gains_from_use() {
    // From nothing, thirty percent a use — over fifty tries the value climbs.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    world.queue(Command::SetSkill {
        serial,
        skill: 1,
        value: 0,
    });
    world.tick(now);

    for _ in 0..50 {
        world.queue(Command::UseSkill {
            serial,
            skill: 1,
            min_skill: 0,
            max_skill: 500,
        });
        world.tick(now);
    }
    assert!(skill_value(&world, entity, 1) > 0, "practice taught something");
}

#[test]
fn a_capped_skill_does_not_gain() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    world.queue(Command::SetSkill {
        serial,
        skill: 1,
        value: openshard_state::DEFAULT_SKILL_CAP,
    });
    world.tick(now);

    for _ in 0..30 {
        world.queue(Command::UseSkill {
            serial,
            skill: 1,
            min_skill: 0,
            max_skill: 1500,
        });
        world.tick(now);
    }
    assert_eq!(
        skill_value(&world, entity, 1),
        openshard_state::DEFAULT_SKILL_CAP,
        "there is nothing left to learn at the cap"
    );
}

#[test]
fn a_locked_skill_does_not_gain() {
    // The arrow the player set on the window is a rule now, not decoration:
    // `Locked` holds a skill exactly where it is, however much it is used.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    world.queue(Command::SetSkill {
        serial,
        skill: Skill::Mining.id(),
        value: 500,
    });
    world.queue(Command::SetSkillLock {
        connection: player,
        skill:      RawSkillId(Skill::Mining.id()),
        lock:       SkillLock::Locked,
    });
    world.tick(now);

    for _ in 0..200 {
        world.queue(Command::UseSkill {
            serial,
            skill: Skill::Mining.id(),
            min_skill: 0,
            max_skill: 1000,
        });
        world.tick(now);
    }
    assert_eq!(
        skill_value(&world, entity, Skill::Mining.id()),
        500,
        "a locked skill is held exactly where it was"
    );
}

#[test]
fn a_down_skill_gives_ground_at_the_total_cap() {
    // The rule that makes a character a build rather than a list: at the total
    // cap a skill only rises if another, set to "down", gives up the same amount.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    fill_to_the_total_cap(&mut world, player, serial, now);
    world.queue(Command::SetSkillLock {
        connection: player,
        skill:      RawSkillId(Skill::Fishing.id()),
        lock:       SkillLock::Down,
    });
    world.tick(now);
    let fishing_before = skill_value(&world, entity, Skill::Fishing.id());

    for _ in 0..200 {
        world.queue(Command::UseSkill {
            serial,
            skill: Skill::Mining.id(),
            min_skill: 0,
            max_skill: 1000,
        });
        world.tick(now);
    }

    let mining = skill_value(&world, entity, Skill::Mining.id());
    let fishing = skill_value(&world, entity, Skill::Fishing.id());
    assert!(mining > 500, "mining still climbed: {mining}");
    assert!(
        fishing < fishing_before,
        "and fishing paid for it: {fishing} was {fishing_before}"
    );
    assert!(
        total_skill(&world, entity) <= world.state.gameplay.total_skill_cap,
        "the total cap held throughout"
    );
}

#[test]
fn the_total_cap_stops_a_gain_with_nothing_to_give_ground() {
    // The same corner with every arrow left pointing up: there is nowhere for the
    // points to come from, so the skill simply stops.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    fill_to_the_total_cap(&mut world, player, serial, now);
    for _ in 0..200 {
        world.queue(Command::UseSkill {
            serial,
            skill: Skill::Mining.id(),
            min_skill: 0,
            max_skill: 1000,
        });
        world.tick(now);
    }
    assert_eq!(
        skill_value(&world, entity, Skill::Mining.id()),
        500,
        "full is full"
    );
}

/// Train a character up to exactly the shard's total skill cap, spread over as
/// many skills as it takes — no single skill may hold it, since each is capped at
/// 100.0 of its own. Mining and Fishing are left at 500 as the two the caller
/// plays with; the rest are filled to their individual caps.
fn fill_to_the_total_cap(world: &mut World, connection: ConnectionId, serial: Serial, now: Instant) {
    let per = world.state.gameplay.skill_cap;
    let total = world.state.gameplay.total_skill_cap;
    world.queue(Command::SetSkill {
        serial,
        skill: Skill::Mining.id(),
        value: 500,
    });
    world.queue(Command::SetSkill {
        serial,
        skill: Skill::Fishing.id(),
        value: 500,
    });
    let mut filled = u32::from(per); // the two above
    // Any skills but those two, and none that the caller then trains.
    let mut spare = [
        Skill::Alchemy,
        Skill::Anatomy,
        Skill::ArmsLore,
        Skill::Begging,
        Skill::Blacksmith,
        Skill::Camping,
        Skill::Carpentry,
        Skill::Cartography,
        Skill::Cooking,
        Skill::Herding,
    ]
    .into_iter();
    while filled < total {
        let skill = spare.next().expect("enough spare skills to reach the cap");
        let value = u16::try_from(total - filled).unwrap_or(per).min(per);
        world.queue(Command::SetSkill {
            serial,
            skill: skill.id(),
            value,
        });
        // Every filler is locked, so only the caller's two can move.
        world.queue(Command::SetSkillLock {
            connection,
            skill: RawSkillId(skill.id()),
            lock: SkillLock::Locked,
        });
        filled += u32::from(value);
    }
    world.tick(now);
    assert_eq!(
        total_skill(world, world.state.players[&connection]),
        total,
        "the character starts exactly at the total cap"
    );
}

/// Everything a mobile is trained in, added up, in tenths.
fn total_skill(world: &World, entity: EntityId) -> u32 {
    world
        .state
        .registry
        .get::<Skills>(entity)
        .map_or(0, Skills::total)
}

#[test]
fn a_skill_that_trains_nudges_its_stat() {
    // Mining leans wholly on strength (its row gives dexterity and intelligence
    // nothing), so a miner gets stronger and no quicker or wiser.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    world.queue(Command::SetStats {
        serial,
        strength: 20,
        dexterity: 20,
        intelligence: 20,
    });
    world.queue(Command::SetSkill {
        serial,
        skill: Skill::Mining.id(),
        value: 300,
    });
    world.tick(now);

    for _ in 0..400 {
        world.queue(Command::UseSkill {
            serial,
            skill: Skill::Mining.id(),
            min_skill: 0,
            max_skill: 1000,
        });
        world.tick(now);
    }
    let stats = *world
        .state
        .registry
        .get::<openshard_state::Stats>(entity)
        .expect("the character has stats");
    assert!(stats.strength > 20, "strength rose: {}", stats.strength);
    assert_eq!(stats.dexterity, 20, "and dexterity did not");
    assert_eq!(stats.intelligence, 20, "nor intelligence");
}

#[test]
fn a_stat_stops_at_the_total_cap_unless_one_gives_ground() {
    // The classic 225 is a budget, not a wall: with nothing set to "down" a
    // character at the cap gains nothing, and with dexterity set to fall,
    // strength climbs on its points.
    let stiff = trained_strength(StatLock::Up);
    let giving = trained_strength(StatLock::Down);
    assert_eq!(stiff, 75, "at the cap with nothing to give, nothing moves");
    assert!(
        giving > stiff,
        "a stat set to fall funds the one that rises: {giving} against {stiff}"
    );
}

/// Train Mining hard on a character sitting exactly at the total stat cap, with
/// dexterity's arrow set to `dex_lock`, and report the strength it reaches.
fn trained_strength(dex_lock: StatLock) -> u16 {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    world.queue(Command::SetStats {
        serial,
        strength: 75,
        dexterity: 75,
        intelligence: 75, // 225 exactly: the cap
    });
    world.queue(Command::SetSkill {
        serial,
        skill: Skill::Mining.id(),
        value: 300,
    });
    world.tick(now);
    // The stat arrows are their own component, and the only way to move one
    // today is directly — the `0xBF` packet that carries them is the next slice.
    world.state.registry.insert(
        entity,
        openshard_state::StatLocks {
            strength:     StatLock::Up,
            dexterity:    dex_lock,
            intelligence: StatLock::Up,
        },
    );
    for _ in 0..400 {
        world.queue(Command::UseSkill {
            serial,
            skill: Skill::Mining.id(),
            min_skill: 0,
            max_skill: 1000,
        });
        world.tick(now);
    }
    world
        .state
        .registry
        .get::<openshard_state::Stats>(entity)
        .expect("the character has stats")
        .strength
}

#[test]
fn a_passive_skills_button_says_so_and_starts_nothing() {
    // Thirty-five of the fifty-eight cannot be used from the window, and the
    // client has its own line for exactly that: cliloc 500014. Before this, the
    // 0x12 that carries the click was decoded and routed nowhere, so pressing a
    // skill did nothing at all — no message, no error, nothing in a log.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::UseSkillButton {
        connection: player,
        skill:      RawSkillId(Skill::Tactics.id()), // passive: there is no using it directly
    });
    world.tick(now);

    let cliloc = localized_cliloc(&mut world, player);
    assert_eq!(cliloc, Some(500_014), "\"That skill cannot be used directly.\"");
    assert!(
        !world
            .state
            .registry
            .has::<openshard_state::SkillCooldown>(world.state.players[&player]),
        "and nothing was started, so nothing is on cooldown"
    );
}

#[test]
fn a_usable_skills_button_announces_it_and_holds_the_button() {
    // The twenty-three that *can* be used raise a `SkillRequested` for whoever
    // owns the effect, and arm the cooldown before the handler runs — so a
    // handler that forgets to set its own delay still cannot be spammed.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let mut asked: Cursor<openshard_skills::SkillRequested> = world.bus().cursor();

    world.queue(Command::UseSkillButton {
        connection: player,
        skill:      RawSkillId(Skill::Hiding.id()),
    });
    world.tick(now);

    let events: Vec<_> = world.bus().read(&mut asked).copied().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].skill, Skill::Hiding);
    assert!(world.state.registry.has::<openshard_state::SkillCooldown>(entity));

    // A second press inside the cooldown is refused out loud, and announces
    // nothing.
    let _ = packets_for(&mut world, player);
    world.queue(Command::UseSkillButton {
        connection: player,
        skill:      RawSkillId(Skill::Hiding.id()),
    });
    world.tick(now);
    assert_eq!(
        localized_cliloc(&mut world, player),
        Some(500_118),
        "\"You must wait a few moments to use another skill.\""
    );
    assert!(
        world.bus().read(&mut asked).next().is_none(),
        "and the second press started nothing"
    );
}

#[test]
fn a_ghost_cannot_use_a_skill_at_all() {
    // ServUO's `CheckAlive` gate, and it is silent on purpose: there is no
    // message for this, the button simply does not work.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    world.state.registry.insert(
        entity,
        openshard_state::Ghost {
            body: Body {
                id:  Graphic(0x0190),
                hue: openshard_protocol::wire::Hue(0),
            },
        },
    );
    let _ = packets_for(&mut world, player);

    world.queue(Command::UseSkillButton {
        connection: player,
        skill:      RawSkillId(Skill::Hiding.id()),
    });
    world.tick(now);
    assert_eq!(localized_cliloc(&mut world, player), None, "not a word");
}

#[test]
fn a_stat_arrow_is_stored_and_relayed() {
    // Unlike a skill arrow, this one is sent back: the status bar's arrows come
    // out of their own `0xBF 0x19`, and a client that never gets one draws all
    // three pointing up whatever the character saved.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let _ = packets_for(&mut world, player);

    world.queue(Command::SetStatLock {
        connection: player,
        stat:       Stat::Dexterity,
        lock:       StatLock::Down,
    });
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<openshard_state::StatLocks>(entity)
            .expect("the arrows are stored")
            .dexterity,
        StatLock::Down
    );
    let relay = packets_for(&mut world, player)
        .into_iter()
        .find(|p| p[0] == 0xBF && u16::from_be_bytes([p[3], p[4]]) == 0x19)
        .expect("the client is told what it now draws");
    assert_eq!(relay[11], 0b00_01_00, "str up, dex down, int up");
}

#[test]
fn the_window_shows_the_effective_value_beside_the_trained_one() {
    // The `0x3A` has carried two numbers since the beginning and they were the
    // same number: nothing lent a skill anything. Parrying leans on strength and
    // dexterity, so a strong character's parry is worth more than it is trained.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    world.queue(Command::SetStats {
        serial,
        strength: 100,
        dexterity: 100,
        intelligence: 10,
    });
    world.queue(Command::SetSkill {
        serial,
        skill: Skill::Parry.id(),
        value: 200,
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::RequestSkills { connection: player });
    world.tick(now);
    let full = packets_for(&mut world, player)
        .into_iter()
        .find(|p| p[0] == 0x3A && p[3] == 0x02)
        .expect("the full skill list");
    // Entries are nine bytes each from offset 4: id+1, value, base, lock, cap.
    let parry = 4 + usize::from(Skill::Parry.id()) * 9;
    assert_eq!(
        u16::from_be_bytes([full[parry], full[parry + 1]]),
        u16::from(Skill::Parry.id()) + 1
    );
    let value = u16::from_be_bytes([full[parry + 2], full[parry + 3]]);
    let base = u16::from_be_bytes([full[parry + 4], full[parry + 5]]);
    assert_eq!(base, 200, "the trained number");
    assert!(value > base, "and the effective one is higher: {value}");
}

#[test]
fn anatomy_raises_a_cursor_and_reads_the_target() {
    // The whole shape of a lore skill: press the button, get a cursor, click
    // somebody, and be told about them — over their head, and only on your own
    // screen.
    let now = Instant::now();
    let mut world = world();
    let looker = enter(&mut world, now);
    let entity = world.state.players[&looker];
    let serial = serial_of(&world, looker);
    world.queue(Command::SetSkill {
        serial,
        skill: Skill::Anatomy.id(),
        value: 1000, // a grandmaster: the roll is a sure thing and the guess exact
    });
    world.tick(now);

    // Somebody to look at, one tile away.
    let mob = spawn_mobile_at(&mut world, Point::new(START.x + 1, START.y, 0), 50, now);
    let _ = packets_for(&mut world, looker);

    world.queue(Command::UseSkillButton {
        connection: looker,
        skill:      RawSkillId(Skill::Anatomy.id()),
    });
    world.tick(now);
    let cursor = packets_for(&mut world, looker)
        .into_iter()
        .find(|p| p[0] == 0x6C)
        .expect("a targeting cursor went up");
    assert_eq!(cursor[1], 0, "an object cursor: bare ground has no anatomy");
    assert!(
        world.state.has_target(entity),
        "and the world remembers which skill asked"
    );

    world.queue(Command::TargetResponse {
        connection: looker,
        response:   openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(serial.raw()),
            object:    Some(mob),
            location:  Point::new(START.x + 1, START.y, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    let said = packets_for(&mut world, looker)
        .into_iter()
        .find(|p| p[0] == 0xC1)
        .expect("the answer came back");
    let cliloc = u32::from_be_bytes([said[14], said[15], said[16], said[17]]);
    // 1038045 is "That looks [very weak] and [very clumsy]", the base of an
    // eleven-by-eleven block; a hundred-strength, hundred-dexterity creature
    // lands on 10*11 + 10 past it.
    assert!(
        (1_038_045..=1_038_045 + 120).contains(&cliloc),
        "an Anatomy result line, not something else: {cliloc}"
    );
    assert_eq!(
        u32::from_be_bytes([said[3], said[4], said[5], said[6]]),
        mob.raw(),
        "drawn over the thing looked at, not over the system"
    );
}

/// The cliloc of the last `0xC1` localized message sent to a connection, if any.
fn localized_cliloc(world: &mut World, connection: ConnectionId) -> Option<u32> {
    packets_for(world, connection)
        .into_iter()
        .rfind(|p| p[0] == 0xC1)
        .map(|p| u32::from_be_bytes([p[14], p[15], p[16], p[17]]))
}

#[test]
fn stats_lend_a_skill_its_effective_value_before_aos() {
    // ServUO's `Skill.NonRacialValue`: Parrying scales 7.5 with strength and 2.5
    // with dexterity, so a strong character parries better than the trained
    // number alone — and the bonus fades as the training rises.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    world.queue(Command::SetStats {
        serial,
        strength: 100,
        dexterity: 100,
        intelligence: 10,
    });
    world.queue(Command::SetSkill {
        serial,
        skill: Skill::Parry.id(),
        value: 0,
    });
    world.tick(now);
    // 100 strength at 7.5 plus 100 dexterity at 2.5 is ten skill points, and the
    // row's ceiling is exactly that, so an untrained parry is worth 10.0.
    assert_eq!(
        openshard_skills::skill_value(&world.state, entity, Skill::Parry),
        100
    );
    // A skill with no stat scales at all is worth exactly what is trained.
    assert_eq!(
        openshard_skills::skill_value(&world.state, entity, Skill::Hiding),
        0
    );
}

#[test]
fn the_stat_bonus_is_gone_from_aos_on() {
    // ServUO zeroes the three scale columns at startup on an AoS shard
    // (`AOS.DisableStatInfluences`), so the effective value is the base.
    let now = Instant::now();
    let mut world = World::new(START).with_gameplay(Gameplay {
        combat_era: CombatEra::new(2),
        ..Gameplay::default()
    });
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    world.queue(Command::SetStats {
        serial,
        strength: 100,
        dexterity: 100,
        intelligence: 100,
    });
    world.queue(Command::SetSkill {
        serial,
        skill: Skill::Parry.id(),
        value: 0,
    });
    world.tick(now);
    assert_eq!(
        openshard_skills::skill_value(&world.state, entity, Skill::Parry),
        0,
        "no stat lends to a skill from AoS on"
    );
}

#[test]
fn skill_rolls_are_replayable() {
    // The whole reason the generator lives in the world: the same commands
    // from the same start reach the same skill, roll for roll.
    fn run() -> u16 {
        let now = Instant::now();
        let mut world = world();
        let connection = enter(&mut world, now);
        let serial = serial_of(&world, connection);
        let entity = world.state.players[&connection];
        world.queue(Command::SetSkill {
            serial,
            skill: 3,
            value: 400,
        });
        world.tick(now);
        for _ in 0..40 {
            world.queue(Command::UseSkill {
                serial,
                skill: 3,
                min_skill: 0,
                max_skill: 1000,
            });
            world.tick(now);
        }
        skill_value(&world, entity, 3)
    }
    assert_eq!(run(), run(), "two identical runs land on the same value");
}

#[test]
fn casting_a_spell_pays_mana_and_announces_it() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    // Grandmaster mage, so the skill roll is a sure thing.
    world.queue(Command::SetSkill {
        serial,
        skill: 1,
        value: 1000,
    });
    world.tick(now);

    let mut cast: Cursor<SpellCast> = world.bus().cursor();
    world.queue(Command::CastSpell {
        serial,
        spell: SpellId(5),
        target: None,
        mana: 10,
        min_skill: 0,
        max_skill: 0,
        skill: 1,
        pack: None,
        reagents: Vec::new(),
    });
    world.tick(now);

    let events: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].spell, SpellId(5));
    assert!(events[0].success, "a mana-full grandmaster casts it");
    assert_eq!(
        world.state.registry.get::<Mana>(entity).unwrap().current,
        90,
        "ten mana is spent"
    );
}

#[test]
fn reagents_are_consumed_on_a_cast_and_a_short_pack_fizzles() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    world.queue(Command::SetSkill {
        serial,
        skill: 1,
        value: 1000,
    });
    world.tick(now);

    // A pack with three of one reagent.
    const REAGENT: u16 = 0x0F7A;
    let pack = spawn_container_at(&mut world, Point::new(START.x, START.y, 0), now);
    let container = pack;
    for _ in 0..3 {
        let (item, _) = world
            .state
            .registry
            .spawn_with_serial(openshard_protocol::serial::SerialKind::Item)
            .unwrap();
        world.state.registry.insert(
            item,
            Drawn {
                id:  openshard_protocol::wire::Graphic(REAGENT),
                hue: openshard_protocol::wire::Hue(0),
            },
        );
        openshard_state::establish_item_location(
            &mut world.state,
            item,
            openshard_state::ItemLocation::contained(Contained {
                container,
                position: GumpPoint::new(0, 0),
                grid: GridSlot(0),
            }),
        )
        .unwrap();
    }

    let spell = |reagents: Vec<(Graphic, u16)>| {
        Command::CastSpell {
            serial,
            spell: SpellId(5),
            target: None,
            mana: 10,
            min_skill: 0,
            max_skill: 0,
            skill: 1,
            pack: Some(pack),
            reagents,
        }
    };
    let mut cast: Cursor<SpellCast> = world.bus().cursor();

    // First cast needs two; the pack has three, so it takes them and casts.
    world.queue(spell(vec![(Graphic(REAGENT), 2)]));
    world.tick(now);
    let first: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
    assert!(first[0].success, "the stocked pack lets it cast");
    assert_eq!(
        openshard_items::count_in_container(&world.state, container, Graphic(REAGENT)),
        1,
        "two of the three reagents were consumed"
    );

    // One left; a second cast needing two fizzles and spends nothing.
    let mana = world.state.registry.get::<Mana>(entity).unwrap().current;
    world.queue(spell(vec![(Graphic(REAGENT), 2)]));
    world.tick(now);
    let second: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
    assert!(!second[0].success, "one reagent left is not enough");
    assert_eq!(
        world.state.registry.get::<Mana>(entity).unwrap().current,
        mana,
        "a fizzle spends no mana"
    );
    assert_eq!(
        openshard_items::count_in_container(&world.state, container, Graphic(REAGENT)),
        1,
        "and consumes no reagent"
    );
}

#[test]
fn consuming_a_reagent_redraws_an_open_pack() {
    // A pack the player has open updates live: a reagent burned out of it
    // vanishes from the gump, a `0x1D` pushed to the watcher.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    world.queue(Command::SetSkill {
        serial,
        skill: 1,
        value: 1000,
    });
    world.tick(now);

    // A container on the player's tile, one reagent inside.
    const REAGENT: u16 = 0x0F7A;
    let pack = spawn_container_at(&mut world, Point::new(START.x, START.y, 0), now);
    let container = pack;
    let (_, item_serial) = world
        .state
        .registry
        .spawn_with_serial(openshard_protocol::serial::SerialKind::Item)
        .unwrap();
    let item = world.state.registry.entity_of(item_serial).unwrap();
    world.state.registry.insert(
        item,
        Drawn {
            id:  openshard_protocol::wire::Graphic(REAGENT),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    openshard_state::establish_item_location(
        &mut world.state,
        item,
        openshard_state::ItemLocation::contained(Contained {
            container,
            position: GumpPoint::new(0, 0),
            grid: GridSlot(0),
        }),
    )
    .unwrap();

    // Open it, then clear what has been sent so far.
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(pack.raw())),
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);

    // Cast, burning the reagent out of the open pack.
    world.queue(Command::CastSpell {
        serial,
        spell: SpellId(5),
        target: None,
        mana: 10,
        min_skill: 0,
        max_skill: 0,
        skill: 1,
        pack: Some(pack),
        reagents: vec![(openshard_protocol::wire::Graphic(REAGENT), 1)],
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| p == &encode_packet(&Remove { serial: item_serial }, ClientVersion::TOL)),
        "the watcher is told the reagent left the pack"
    );
}

#[test]
fn a_spell_beyond_the_mana_fizzles() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);

    let mut cast: Cursor<SpellCast> = world.bus().cursor();
    world.queue(Command::CastSpell {
        serial,
        spell: SpellId(1),
        target: None,
        mana: 200, // more than the 100 on hand
        min_skill: 0,
        max_skill: 0,
        skill: 1,
        pack: None,
        reagents: Vec::new(),
    });
    world.tick(now);

    let events: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
    assert!(!events[0].success, "it fizzles");
    assert_eq!(
        world.state.registry.get::<Mana>(entity).unwrap().current,
        100,
        "and no mana is spent on a fizzle"
    );
}

#[test]
fn healing_raises_hits_but_not_past_max() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);

    world.queue(Command::Damage {
        serial,
        amount: 60,
        damage_type: 0,
        by: None,
    });
    world.tick(now); // 100 -> 40
    world.queue(Command::Heal { serial, amount: 1000 });
    world.tick(now);

    assert_eq!(
        world.state.registry.get::<Hitpoints>(entity).unwrap().current,
        100,
        "healed to the maximum, no further"
    );
}

#[test]
fn mana_trickles_back() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);
    world.queue(Command::SetSkill {
        serial,
        skill: 1,
        value: 1000,
    });
    world.tick(now);
    world.queue(Command::CastSpell {
        serial,
        spell: SpellId(1),
        target: None,
        mana: 20,
        min_skill: 0,
        max_skill: 0,
        skill: 1,
        pack: None,
        reagents: Vec::new(),
    });
    world.tick(now);
    let spent = world.state.registry.get::<Mana>(entity).unwrap().current;

    // The regen cadence is per mobile now (Intelligence and Meditation set it),
    // so wait out this character's own rate rather than a shard-wide constant.
    let rate = openshard_magic::mana_regen_ticks(&world.state, entity);
    for _ in 0..=rate {
        world.tick(now);
    }
    assert!(
        world.state.registry.get::<Mana>(entity).unwrap().current > spent,
        "mana came back over time"
    );
}

#[test]
fn stamina_is_a_real_pool_that_trickles_back() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];

    // It exists as its own pool, full at dexterity, not a stand-in for the stat.
    let full = world.state.registry.get::<Stamina>(entity).copied().unwrap();
    assert_eq!(full.current, full.max, "a new character starts rested");
    assert!(full.max > 0, "the pool is dexterity, not zero");

    // Drain it (a future combat or overweight cost), then let it recover.
    world.state.registry.insert(
        entity,
        Stamina {
            current: 1,
            max:     full.max,
        },
    );
    for _ in 0..combat::STAMINA_REGEN_TICKS {
        world.tick(now);
    }
    assert!(
        world.state.registry.get::<Stamina>(entity).unwrap().current > 1,
        "stamina came back over time"
    );
}

/// Spawn a creature with a brain (sight, wander) and return its serial.
fn spawn_creature(world: &mut World, point: Point, sight: u8, wander: bool, now: Instant) -> Serial {
    world.queue(Command::SpawnMobile {
        body: openshard_protocol::wire::Graphic(0x0190),
        hue: openshard_protocol::wire::Hue(0),
        hits: 50,
        notoriety: Notoriety::from_bits(5),
        damage: combat::SWING_DAMAGE,
        resistance: openshard_protocol::world::PhysicalResistance::new(0),
        swing: 0,
        sight: Sight(sight),
        aggression: Aggression::from_bits(2),
        beat: 0,
        ranged: None,
        ranged_kind: DamageType::Physical,
        wander,
        position: point,
        facet: Facet(0),
        name: None,
        title: None,
        shoe: 0,
        fame: 0,
        karma: 0,
        night_home: None,
        banker: false,
        vendor: false,
        healer: false,
        equipment: Vec::new(),
        skills: Vec::new(),
        stock: Vec::new(),
        escort_to: None,
        quests: Vec::new(),
    });
    world.tick(now);
    world
        .state
        .registry
        .query::<Body>()
        .filter(|(entity, _)| !world.state.registry.has::<Client>(*entity))
        .filter_map(|(entity, _)| world.state.registry.serial_of(entity))
        .max()
        .expect("a spawned creature")
}

#[test]
fn an_aggressive_creature_attacks_a_nearby_player() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    // Aggressive, standing on the player's tile.
    spawn_creature(&mut world, Point::new(START.x, START.y, 0), 10, false, now);

    // A beat to notice, a swing interval to strike.
    for _ in 0..(AI_THINK_TICKS + WRESTLING_SWING_TICKS + 2) {
        world.tick(now);
    }
    assert!(
        world
            .state
            .registry
            .get::<Hitpoints>(player_entity)
            .unwrap()
            .current
            < DEFAULT_HITPOINTS,
        "the creature noticed the player and hit them"
    );
}

#[test]
fn a_melee_swing_turns_the_attacker_toward_its_target() {
    let now = Instant::now();
    let mut world = world();
    let attacker = enter(&mut world, now);
    let defender = enter(&mut world, now);
    teleport(&mut world, defender, Point::new(START.x + 1, START.y, 0));

    let attacker_entity = world.state.players[&attacker];
    let defender_serial = serial_of(&world, defender);
    world
        .state
        .registry
        .insert(attacker_entity, Heading(Facing::walking(Direction::South)));
    combat::war_mode(&mut world.state, attacker, true);
    combat::attack(&mut world.state, attacker, Some(defender_serial));
    let turn = packets_for(&mut world, attacker);
    combat::commit_actions(&mut world.state);
    land_the_committed_swing(&mut world, attacker_entity);

    assert_eq!(
        world.state.registry.get::<Heading>(attacker_entity),
        Some(&Heading(Facing::walking(Direction::East))),
        "the attacking body faces the adjacent target before its animation"
    );
    assert_eq!(
        world
            .state
            .registry
            .get::<Movement>(attacker_entity)
            .map(|movement| movement.0.facing.direction),
        Some(Direction::East),
        "the next AI/player step agrees with the displayed heading"
    );
    assert!(
        turn.iter().any(|packet| {
            packet.first() == Some(&0x20) && packet.get(17) == Some(&Direction::East.to_bits())
        }),
        "target selection immediately turns the attacking player toward its opponent"
    );
    assert!(
        packets_for(&mut world, attacker)
            .iter()
            .all(|packet| packet.first() != Some(&0x20)),
        "impact does not repeat the already-confirmed combat turn"
    );
}

#[test]
fn a_hidden_wrestler_arms_an_immediate_target_bound_ambush() {
    let now = Instant::now();
    let mut world = world();
    let attacker = enter(&mut world, now);
    let defender = enter(&mut world, now);
    teleport(&mut world, defender, Point::new(START.x + 1, START.y, 0));

    let attacker_entity = world.state.players[&attacker];
    let defender_serial = serial_of(&world, defender);
    world
        .state
        .registry
        .insert(attacker_entity, openshard_state::Hidden);

    combat::war_mode(&mut world.state, attacker, true);
    combat::attack(&mut world.state, attacker, Some(defender_serial));

    assert_eq!(
        world
            .state
            .registry
            .get::<Combat>(attacker_entity)
            .unwrap()
            .next_swing(),
        Some(world.state.ticks),
        "an ambush does not wait through the normal first swing timer"
    );
    assert_eq!(
        world.state.registry.get::<WrestlingOpener>(attacker_entity),
        Some(&WrestlingOpener {
            target:     defender_serial,
            expires_at: world.state.ticks + 2 * openshard_state::TICKS_PER_SECOND,
        })
    );
}

#[test]
fn three_recent_wrestling_steps_shorten_only_the_next_new_engagement() {
    let now = Instant::now();
    let mut world = world();
    let attacker = enter(&mut world, now);
    let defender = enter(&mut world, now);
    let attacker_entity = world.state.players[&attacker];
    let defender_serial = serial_of(&world, defender);

    for _ in 0..3 {
        combat::record_wrestling_step(&mut world.state, attacker_entity);
    }
    assert_eq!(
        world
            .state
            .registry
            .get::<WrestlingStride>(attacker_entity)
            .unwrap()
            .steps,
        3
    );

    combat::war_mode(&mut world.state, attacker, true);
    combat::attack(&mut world.state, attacker, Some(defender_serial));

    assert_eq!(
        world
            .state
            .registry
            .get::<Combat>(attacker_entity)
            .unwrap()
            .next_swing(),
        Some(world.state.ticks + WRESTLING_SWING_TICKS / 2),
        "intercept spends the footwork on first contact, not every following hit"
    );
    assert!(
        !world.state.registry.has::<WrestlingStride>(attacker_entity),
        "the stride cannot be reused after it earned an intercept"
    );
}

#[test]
fn a_wrestlers_third_consecutive_hit_is_a_combo_and_restores_stamina() {
    let now = Instant::now();
    let mut world = world();
    let attacker = enter(&mut world, now);
    let defender = enter(&mut world, now);
    teleport(&mut world, defender, Point::new(START.x + 1, START.y, 0));

    let attacker_entity = world.state.players[&attacker];
    let defender_entity = world.state.players[&defender];
    let defender_serial = serial_of(&world, defender);
    // An unskilled attacker lands deterministically; a fixed natural blow makes
    // the combo's extra point visible through pre-AoS PvP's halving rule.
    world.state.registry.remove::<Skills>(attacker_entity);
    world
        .state
        .registry
        .insert(attacker_entity, MeleeDamage { amount: 10 });
    world.state.registry.insert(
        attacker_entity,
        Stamina {
            current: 80,
            max:     100,
        },
    );

    for _ in 0..3 {
        let due = world.state.ticks;
        let combat = world
            .state
            .registry
            .get_mut::<Combat>(attacker_entity)
            .expect("player combat state");
        assert!(combat.enter_war());
        assert!(combat.aim(defender_serial, due));
        combat::commit_actions(&mut world.state);
        land_the_committed_swing(&mut world, attacker_entity);
    }

    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(defender_entity)
            .unwrap()
            .current,
        84
    );
    assert_eq!(
        world
            .state
            .registry
            .get::<Stamina>(attacker_entity)
            .unwrap()
            .current,
        85
    );
    assert!(
        !world.state.registry.has::<WrestlingCombo>(attacker_entity),
        "the third hit pays the combo out and starts a fresh sequence"
    );
}

#[test]
fn an_aggressive_creature_chases_a_player() {
    let now = Instant::now();
    let mut world = world();
    enter(&mut world, now); // a player at START to be chased
    let start = Point::new(START.x + 4, START.y, 0);
    let mob = spawn_creature(&mut world, start, 10, false, now);
    let mob_entity = entity(&world, mob);

    // Several beats: it turns, then walks toward the player.
    for _ in 0..(5 * AI_THINK_TICKS) {
        world.tick(now);
    }
    assert!(
        world.state.registry.get::<Position>(mob_entity).unwrap().0.x < start.x,
        "the creature closed the distance"
    );
}

#[test]
fn a_passive_creature_ignores_players() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    // Sight 0, no wander: no brain at all.
    spawn_creature(&mut world, Point::new(START.x, START.y, 0), 0, false, now);

    for _ in 0..(WRESTLING_SWING_TICKS + AI_THINK_TICKS + 5) {
        world.tick(now);
    }
    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(player_entity)
            .unwrap()
            .current,
        DEFAULT_HITPOINTS,
        "a passive creature never lifts a finger"
    );
}

#[test]
fn a_wandering_creature_drifts() {
    let now = Instant::now();
    let mut world = world();
    let start = Point::new(START.x, START.y, 0);
    // Wanders, sees nothing to fight.
    let mob = spawn_creature(&mut world, start, 0, true, now);
    let mob_entity = entity(&world, mob);

    for _ in 0..(15 * AI_THINK_TICKS) {
        world.tick(now);
    }
    assert_ne!(
        world.state.registry.get::<Position>(mob_entity).unwrap().0,
        start,
        "given time, a wanderer moves"
    );
}

#[test]
fn stats_recap_hits_and_mana() {
    // Strength caps hit points, intelligence mana; lowering a stat below the
    // current value drags it down.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let serial = serial_of(&world, player);

    world.queue(Command::SetStats {
        serial,
        strength: 60,
        dexterity: 80,
        intelligence: 40,
    });
    world.tick(now);

    let hp = world.state.registry.get::<Hitpoints>(entity).unwrap();
    assert_eq!((hp.current, hp.max), (60, 60), "hits follow strength");
    let mana = world.state.registry.get::<Mana>(entity).unwrap();
    assert_eq!((mana.current, mana.max), (40, 40), "mana follows intelligence");
    assert_eq!(
        world.state.registry.get::<Stats>(entity).unwrap().dexterity,
        80,
        "and dexterity is stored for what will derive from it"
    );
}

#[test]
fn speech_reaches_nearby_players_and_the_speaker() {
    let now = Instant::now();
    let mut world = world();
    let speaker = enter(&mut world, now);
    let listener = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let _ = packets_for(&mut world, speaker);
    let _ = packets_for(&mut world, listener);

    world.queue(Command::Say {
        connection: speaker,
        mode:       RawTalkMode(0),
        hue:        RawHue(0x0384),
        font:       RawFont(3),
        text:       "hail".to_owned(),
    });
    world.tick(now);

    // Drain once — both players' packets came out of the same tick.
    let all: Vec<Outbound> = world.drain_outbound().collect();
    assert!(
        all.iter().any(|o| o.connection == speaker && o.packet[0] == 0xAE),
        "the speaker sees their own words"
    );
    assert!(
        all.iter()
            .any(|o| o.connection == listener && o.packet[0] == 0xAE),
        "and so does the player beside them"
    );
}

#[test]
fn speech_does_not_carry_out_of_earshot() {
    let now = Instant::now();
    let mut world = world();
    let speaker = enter(&mut world, now);
    let listener = enter_as(&mut world, ConnectionId::from_raw(2), now);
    // Move the listener well past speech range.
    teleport(&mut world, listener, Point::new(START.x + 40, START.y, 0));
    let _ = packets_for(&mut world, listener);

    world.queue(Command::Say {
        connection: speaker,
        mode:       RawTalkMode(0),
        hue:        RawHue(0),
        font:       RawFont(3),
        text:       "hail".to_owned(),
    });
    world.tick(now);

    assert!(
        !packets_for(&mut world, listener).iter().any(|p| p[0] == 0xAE),
        "a shout across a field is not heard"
    );
}

#[test]
fn a_whisper_carries_only_to_those_right_beside() {
    // Ten tiles is within normal earshot but far past a whisper's three, so
    // the same listener who would hear a word spoken hears nothing whispered.
    let now = Instant::now();
    let mut world = world();
    let speaker = enter(&mut world, now);
    let listener = enter_as(&mut world, ConnectionId::from_raw(2), now);
    teleport(&mut world, listener, Point::new(START.x + 10, START.y, 0));
    let _ = packets_for(&mut world, listener);

    world.queue(Command::Say {
        connection: speaker,
        mode:       RawTalkMode(TalkMode::Whisper.to_wire()),
        hue:        RawHue(0),
        font:       RawFont(3),
        text:       "psst".to_owned(),
    });
    world.tick(now);

    assert!(
        !packets_for(&mut world, listener).iter().any(|p| p[0] == 0xAE),
        "a whisper does not reach ten tiles off"
    );
}

#[test]
fn a_yell_carries_past_normal_earshot() {
    // Twenty-five tiles is beyond the normal eighteen but inside a yell's
    // thirty-one, so only shouting reaches this listener.
    let now = Instant::now();
    let mut world = world();
    let speaker = enter(&mut world, now);
    let listener = enter_as(&mut world, ConnectionId::from_raw(2), now);
    teleport(&mut world, listener, Point::new(START.x + 25, START.y, 0));
    let _ = packets_for(&mut world, listener);

    // Said normally, it does not reach.
    world.queue(Command::Say {
        connection: speaker,
        mode:       RawTalkMode(0),
        hue:        RawHue(0),
        font:       RawFont(3),
        text:       "here".to_owned(),
    });
    world.tick(now);
    assert!(
        !packets_for(&mut world, listener).iter().any(|p| p[0] == 0xAE),
        "normal speech stops short of twenty-five tiles"
    );

    // Yelled, it does.
    world.queue(Command::Say {
        connection: speaker,
        mode:       RawTalkMode(TalkMode::Yell.to_wire()),
        hue:        RawHue(0),
        font:       RawFont(3),
        text:       "here".to_owned(),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, listener).iter().any(|p| p[0] == 0xAE),
        "but a yell carries that far"
    );
}

#[test]
fn all_speech_goes_out_as_unicode() {
    // Every line rides Unicode `0xAE`, plain ASCII and accented alike, so the
    // font never flips: a Brazilian player's "olá" keeps its accent, and the
    // ASCII "hail" tested above draws in the same modern font rather than the
    // client's antique `0x1C` one.
    let now = Instant::now();
    let mut world = world();
    let speaker = enter(&mut world, now);

    for text in ["hail", "olá"] {
        world.queue(Command::Say {
            connection: speaker,
            mode:       RawTalkMode(0),
            hue:        RawHue(0),
            font:       RawFont(3),
            text:       text.to_owned(),
        });
        world.tick(now);

        let packets = packets_for(&mut world, speaker);
        assert!(
            packets.iter().any(|p| p[0] == 0xAE),
            "{text:?} takes the Unicode path"
        );
        assert!(
            !packets.iter().any(|p| p[0] == 0x1C),
            "and not the ASCII one, which mangles accents and flips the font"
        );
    }
}

#[test]
fn speaking_puts_the_words_on_the_bus() {
    let now = Instant::now();
    let mut world = world();
    let speaker = enter(&mut world, now);
    let mut spoke: Cursor<MobileSpoke> = world.bus().cursor();

    world.queue(Command::Say {
        connection: speaker,
        mode:       RawTalkMode(0),
        hue:        RawHue(0),
        font:       RawFont(3),
        text:       "hello world".to_owned(),
    });
    world.tick(now);

    let events: Vec<MobileSpoke> = world.bus().read(&mut spoke).cloned().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text, "hello world");
}

fn gm_say(world: &mut World, connection: ConnectionId, text: &str, now: Instant) {
    world.queue(Command::Say {
        connection,
        mode: RawTalkMode(0),
        hue: RawHue(0),
        font: RawFont(3),
        text: text.to_owned(),
    });
    world.tick(now);
}

#[test]
fn a_gm_dot_command_is_run_not_spoken() {
    // `.where` from a game master answers privately and is never put over
    // their head — a command is not speech.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let _ = packets_for(&mut world, gm);
    let mut spoke: Cursor<MobileSpoke> = world.bus().cursor();

    gm_say(&mut world, gm, ".where", now);

    assert_eq!(world.bus().read(&mut spoke).count(), 0, "no one heard a command");
    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x1C),
        "the GM got a private system answer"
    );
}

/// `.dummy` — **a target that does nothing back**, which is the whole of what a
/// scarecrow is for: chasing a report about combat against a live creature means
/// the mob, its brain and the sight line are all moving at once and no two runs
/// are the same run.
///
/// Everything asserted here is an *absence*, and deliberately: the scarecrow is
/// built out of the parts `spawn` leaves out rather than out of special cases in
/// the passes. No brain is the one that carries the rest — nothing thinks for
/// it, nothing walks it, and `ai::retaliate` skips it where it looks one up.
#[test]
fn a_scarecrow_stands_there_and_does_nothing_back() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let at = world.registry().get::<Position>(player).unwrap().0;

    gm_say(&mut world, gm, ".dummy", now);

    let (dummy, _) = world
        .registry()
        .query::<openshard_state::components::Scarecrow>()
        .next()
        .expect("a scarecrow was put down");
    assert!(
        !world.registry().has::<openshard_state::components::Brain>(dummy),
        "a scarecrow with a brain would wander off, or fight back"
    );
    assert_eq!(
        world.registry().get::<Notoriety>(dummy).copied(),
        Some(Notoriety::Criminal),
        "grey, so shooting one flags nobody criminal"
    );
    let stood = world.registry().get::<Position>(dummy).unwrap().0;
    assert!(
        openshard_state::sectors::distance(at, stood) <= 1,
        "it stands where the operator was looking, not across the map"
    );

    // A hundred ticks of the world running, and it has done nothing: no step, no
    // fight picked, no action committed. The claim a `Brain` assertion cannot
    // make on its own — something else could always have given it one.
    for _ in 0..100 {
        world.tick(now);
    }
    assert_eq!(
        world.registry().get::<Position>(dummy).unwrap().0,
        stood,
        "it has not moved"
    );
    assert!(!world.registry().has::<Combat>(dummy), "and has picked no fight");

    // And it is a real target: a shot at it commits like a shot at anything.
    let serial = world.registry().serial_of(dummy).unwrap();
    arm_with_bow(&mut world, gm);
    engage(&mut world, gm, serial, now);
    assert!(
        world.registry().has::<CombatAction>(player),
        "a scarecrow can be shot at, which is the entire point of one"
    );
}

/// `.dummy off` takes the nearest away, and every fight it was in ends saying so.
///
/// The second half is the interesting one. A body that simply stopped existing
/// leaves its attacker's bar filling towards an impact that will never come —
/// which is the desync `CombatActionEnded` was added for, arriving by a door
/// nobody had walked through yet.
#[test]
fn removing_a_scarecrow_ends_the_fight_it_was_in() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    gm_say(&mut world, gm, ".dummy", now);
    let (dummy, _) = world
        .registry()
        .query::<openshard_state::components::Scarecrow>()
        .next()
        .expect("a scarecrow was put down");
    let serial = world.registry().serial_of(dummy).unwrap();
    engage(&mut world, gm, serial, now);
    assert!(world.registry().has::<CombatAction>(player), "a fight is on");
    let _ = packets_for(&mut world, gm);

    gm_say(&mut world, gm, ".dummy off", now);

    assert_eq!(
        world
            .registry()
            .query::<openshard_state::components::Scarecrow>()
            .count(),
        0,
        "it is gone"
    );
    assert!(
        !world.registry().has::<CombatAction>(player),
        "and the swing at it went with it"
    );
    assert_eq!(
        action_end(&packets_for(&mut world, gm)),
        Some((
            CombatActionOutcome::Interrupted(InterruptReason::TargetGone).to_bits(),
            InterruptReason::TargetGone.to_bits()
        )),
        "with a reason, or the bar fills towards an impact that is never coming"
    );
}

/// A scarecrow comes back from a save as a scarecrow.
///
/// It is recognisable by a marker and not by its shape — everything else about
/// one is an absence, and a mobile identified by what it lacks is one something
/// else will eventually resemble. That marker has to survive the round trip or
/// `.dummy off` is guessing again, one restart later.
#[test]
fn a_saved_scarecrow_is_still_a_scarecrow() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    gm_say(&mut world, gm, ".dummy", now);
    let records = world.mobile_records();
    let record = records
        .iter()
        .find(|record| record.name.as_deref() == Some("a scarecrow"))
        .expect("the scarecrow is in the save");
    assert!(record.scarecrow, "and is saved as one");
}

#[test]
fn a_players_dot_text_is_ordinary_speech() {
    // A non-GM saying ".hello" just talks: no command, no privilege leak, and
    // the words go on the bus like any other speech.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mut spoke: Cursor<MobileSpoke> = world.bus().cursor();

    gm_say(&mut world, player, ".hello", now);

    let events: Vec<MobileSpoke> = world.bus().read(&mut spoke).cloned().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text, ".hello", "a player's dot-text is spoken verbatim");
}

#[test]
fn dot_save_forces_a_snapshot_and_tells_everyone() {
    // A staff `.save` writes now, without pausing, even with the periodic save
    // turned off — and every player is told it happened.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let gm = enter_gm(&mut world, now);
    let _ = world.drain_saves().count();
    let _ = packets_for(&mut world, gm);

    gm_say(&mut world, gm, ".save", now);

    assert!(
        world.drain_saves().next().is_some(),
        "the save was forced despite the cadence being off"
    );
    assert!(
        packets_for(&mut world, gm)
            .iter()
            .any(|p| { p[0] == 0x1C && String::from_utf8_lossy(p).contains("being saved") }),
        "players were told the world is being saved"
    );
}

#[test]
fn a_gm_can_teleport_add_and_set() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let entity = world.state.players[&gm];

    // Teleport by coordinates — Sphere's `.go`.
    gm_say(
        &mut world,
        gm,
        &format!(".go {} {}", START.x + 5, START.y + 7),
        now,
    );
    let Position(at) = *world.registry().get::<Position>(entity).unwrap();
    assert_eq!((at.x, at.y), (START.x + 5, START.y + 7), "the GM moved");

    // Add an item at the GM's feet — the GM's own screen is drawn the 0x1A.
    let _ = packets_for(&mut world, gm);
    gm_say(&mut world, gm, ".add 0x0eed 5", now);
    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x1A),
        "the spawned item was drawn"
    );

    // Set a stat, through the skills system that owns the cap.
    gm_say(&mut world, gm, ".set str 73", now);
    assert_eq!(world.registry().get::<Stats>(entity).unwrap().strength, 73);
}

fn admin_response(connection: ConnectionId, button: u32) -> Command {
    Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial:       openshard_protocol::gump::RawGumpKey(0),
            gump_id:      openshard_protocol::gump::RawGumpId(crate::admin::ADMIN_GUMP.0),
            button:       openshard_protocol::gump::RawButtonId(button),
            switches:     Vec::new(),
            text_entries: Vec::new(),
        },
    }
}

fn admin_item_response(
    connection: ConnectionId,
    graphic: &str,
    hue: &str,
    amount: &str,
    stackable: bool,
) -> Command {
    Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial:       openshard_protocol::gump::RawGumpKey(0),
            gump_id:      openshard_protocol::gump::RawGumpId(crate::admin::ADMIN_ITEM_GUMP.0),
            button:       openshard_protocol::gump::RawButtonId(1),
            switches:     stackable
                .then_some(openshard_protocol::gump::RawSwitchId(1))
                .into_iter()
                .collect(),
            text_entries: vec![
                (1, graphic.to_owned()),
                (2, hue.to_owned()),
                (3, amount.to_owned()),
            ],
        },
    }
}

fn admin_item_kind_response(
    connection: ConnectionId,
    kind: openshard_protocol::item_kind::ItemKindId,
    material: Option<openshard_protocol::item_kind::MaterialId>,
    amount: u16,
) -> Command {
    Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial:       openshard_protocol::gump::RawGumpKey(0),
            gump_id:      openshard_protocol::gump::RawGumpId(crate::admin::ADMIN_ITEM_GUMP.0),
            button:       openshard_protocol::gump::RawButtonId(
                openshard_protocol::gump::admin::ITEM_CREATE_KIND.0,
            ),
            switches:     Vec::new(),
            text_entries: vec![
                (
                    openshard_protocol::gump::admin::ITEM_KIND_FIELD,
                    kind.0.to_string(),
                ),
                (
                    openshard_protocol::gump::admin::ITEM_MATERIAL_FIELD,
                    material.map_or(0, |material| material.0).to_string(),
                ),
                (
                    openshard_protocol::gump::admin::ITEM_AMOUNT_FIELD,
                    amount.to_string(),
                ),
            ],
        },
    }
}

fn admin_creature_response(connection: ConnectionId, kind: u16) -> Command {
    Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial:       openshard_protocol::gump::RawGumpKey(0),
            gump_id:      openshard_protocol::gump::RawGumpId(crate::admin::ADMIN_CREATURE_GUMP.0),
            button:       openshard_protocol::gump::RawButtonId(
                openshard_protocol::gump::admin::CREATURE_CREATE.0,
            ),
            switches:     Vec::new(),
            text_entries: vec![(
                openshard_protocol::gump::admin::CREATURE_KIND_FIELD,
                kind.to_string(),
            )],
        },
    }
}

#[test]
fn tele_raises_a_cursor_and_the_click_teleports() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let entity = world.state.players[&gm];
    let _ = packets_for(&mut world, gm);

    // `.tele` raises a targeting cursor and does not move the GM yet.
    gm_say(&mut world, gm, ".tele", now);
    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x6C),
        "a targeting cursor is sent"
    );
    let before = *world.registry().get::<Position>(entity).unwrap();
    assert_eq!(before.0.x, START.x, "the GM has not moved on raising the cursor");

    // The click comes back as a 0x6C response; the GM jumps to the spot.
    let target = Point::new(START.x + 9, START.y + 3, before.0.z);
    world.queue(Command::TargetResponse {
        connection: gm,
        response:   openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  target,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);
    let Position(at) = *world.registry().get::<Position>(entity).unwrap();
    assert_eq!((at.x, at.y), (target.x, target.y), "the click teleported the GM");
}

#[test]
fn a_cancelled_tele_does_not_move() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let entity = world.state.players[&gm];

    gm_say(&mut world, gm, ".tele", now);
    let before = *world.registry().get::<Position>(entity).unwrap();
    world.queue(Command::TargetResponse {
        connection: gm,
        response:   openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  Point::new(START.x + 9, START.y + 3, before.0.z),
            graphic:   None,
            cancelled: true,
        },
    });
    world.tick(now);
    let after = *world.registry().get::<Position>(entity).unwrap();
    assert_eq!(before.0, after.0, "a right-clicked cursor moves nobody");
}

#[test]
fn admin_opens_a_gump_for_a_game_master() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let _ = packets_for(&mut world, gm);

    gm_say(&mut world, gm, ".admin", now);

    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0xB0),
        "the admin gump is sent"
    );
}

#[test]
fn an_admin_button_from_a_game_master_is_answered() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let _ = packets_for(&mut world, gm);

    world.queue(admin_response(gm, 13)); // Populate Felucca
    world.tick(now);

    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x1C),
        "the button is acted on"
    );
}

#[test]
fn an_admin_can_create_a_stacked_item_in_their_backpack() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let actor_serial = world.registry().serial_of(actor).unwrap();
    let backpack = items::backpack_of(&world.state, actor_serial).expect("the entering GM wears a backpack");
    let _ = packets_for(&mut world, gm);

    world.queue(admin_response(gm, 40)); // Create item in backpack
    world.tick(now);
    assert!(
        packets_for(&mut world, gm).iter().any(|packet| packet[0] == 0xB0),
        "the item creator form is sent"
    );

    world.queue(admin_item_response(gm, "0x0eed", "0x0481", "25", true));
    world.tick(now);

    let created = world
        .registry()
        .query::<Contained>()
        .find(|(item, held)| {
            held.container == backpack
                && world
                    .registry()
                    .get::<Drawn>(*item)
                    .is_some_and(|drawn| drawn.id == Graphic(0x0eed) && drawn.hue == Hue(0x0481))
        })
        .map(|(item, _)| item)
        .expect("the chosen item is in the backpack");
    assert!(
        world.registry().has::<Stackable>(created),
        "the checked box makes a stack"
    );
    assert_eq!(items::amount_of(&world.state, created), 25);
}

#[test]
fn an_admin_created_backpack_is_a_container() {
    // The quick catalogue presents a backpack as ordinary item art.  It must
    // nevertheless be born with Container, or it looks like a bag but cannot
    // be opened or receive a drop.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let actor_serial = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, actor_serial).unwrap();
    let _ = packets_for(&mut world, gm);

    world.queue(admin_item_response(gm, "0x0e75", "0", "1", false));
    world.tick(now);

    let created = world
        .registry()
        .query::<Contained>()
        .find(|(item, held)| {
            held.container == pack
                && world
                    .registry()
                    .get::<Drawn>(*item)
                    .is_some_and(|drawn| drawn.id == items::BACKPACK_GRAPHIC)
        })
        .map(|(item, _)| item)
        .expect("the new backpack is in the GM's pack");
    assert!(world.registry().has::<Container>(created));

    let created_serial = world.registry().serial_of(created).unwrap();
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(created_serial.raw())),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, gm).iter().any(|packet| packet[0] == 0x24),
        "double-clicking the created backpack opens its container gump"
    );

    let here = world.registry().get::<Position>(actor).unwrap().0;
    let item_serial = spawn_plain_item_at(&mut world, here, now);
    let item = entity(&world, item_serial);
    world.queue(Command::PickUpItem {
        connection: gm,
        serial:     RawSerial(item_serial.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  gm,
        serial:      RawSerial(item_serial.raw()),
        destination: DropDestination::Item {
            item: created_serial,
            at:   GumpPoint::new(60, 60),
        },
    });
    world.tick(now);
    assert_eq!(
        world.registry().get::<Contained>(item).unwrap().container,
        created_serial,
        "the created backpack accepts a dropped item"
    );
}

#[test]
fn the_f1_gameplay_catalogue_creates_by_kind_with_real_components() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::ItemKind;

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let actor_serial = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, actor_serial).unwrap();

    world.queue(admin_item_kind_response(gm, ItemKindId(7), None, 1));
    world.tick(now);

    let created = world
        .registry()
        .query::<Contained>()
        .find_map(|(item, held)| {
            (held.container == pack
                && world.registry().get::<ItemKind>(item) == Some(&ItemKind(ItemKindId(7))))
            .then_some(item)
        })
        .expect("the semantic F1 request placed a backpack kind");
    assert!(
        world.registry().has::<Container>(created),
        "the kind constructor creates a usable container, not backpack art"
    );
}

#[test]
fn an_admin_created_spellbook_is_a_spellbook() {
    // F1 is only another front end for this administrator form. It must use the
    // same item factory as any other creation path, so an art id that represents
    // a spellbook does not become an inert `0x0EFA` picture in the GM's pack.
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        ItemKind,
        SPELLBOOK_GRAPHIC,
        Spellbook,
    };

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let actor_serial = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, actor_serial).unwrap();

    world.queue(admin_item_response(gm, "0x0efa", "0", "1", false));
    world.tick(now);

    let book = world
        .registry()
        .query::<Contained>()
        .find_map(|(item, held)| {
            (held.container == pack
                && world
                    .registry()
                    .get::<Drawn>(item)
                    .is_some_and(|drawn| drawn.id == SPELLBOOK_GRAPHIC))
            .then_some(item)
        })
        .expect("the F1 item form placed a spellbook");
    assert!(
        world.registry().has::<Spellbook>(book),
        "the created book can be opened and taught spells"
    );
    assert_eq!(
        world.registry().get::<ItemKind>(book),
        Some(&ItemKind(ItemKindId(6))),
        "the functional object also carries the registry identity"
    );
}

#[test]
fn an_admin_created_runebook_is_a_runebook() {
    // Runebooks use a different item-state component from spellbooks.  F1 must
    // therefore pass the registry result through the normal item factory, not
    // merely put the book art into the administrator's backpack.
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        ItemKind,
        RUNEBOOK_GRAPHIC,
        Runebook,
    };

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let actor_serial = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, actor_serial).unwrap();

    world.queue(admin_item_response(gm, "0x22c5", "0", "1", false));
    world.tick(now);

    let book = world
        .registry()
        .query::<Contained>()
        .find_map(|(item, held)| {
            (held.container == pack
                && world
                    .registry()
                    .get::<Drawn>(item)
                    .is_some_and(|drawn| drawn.id == RUNEBOOK_GRAPHIC))
            .then_some(item)
        })
        .expect("the F1 item form placed a runebook");
    assert!(
        world.registry().has::<Runebook>(book),
        "the created book opens the runebook interface"
    );
    assert_eq!(
        world.registry().get::<ItemKind>(book),
        Some(&ItemKind(ItemKindId(8))),
        "the functional object also carries the registry identity"
    );
}

#[test]
fn an_admin_created_pickaxe_is_a_semantic_harvesting_tool() {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };
    use openshard_state::components::{
        ItemKind,
        Material,
        Tool,
    };

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let actor_serial = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, actor_serial).unwrap();

    world.queue(admin_item_response(gm, "0x0e86", "0", "1", false));
    world.tick(now);

    let pickaxe = world
        .registry()
        .query::<Contained>()
        .find_map(|(item, held)| {
            (held.container == pack
                && world
                    .registry()
                    .get::<Drawn>(item)
                    .is_some_and(|drawn| drawn.id == Graphic(0x0E86)))
            .then_some(item)
        })
        .expect("the F1 item form placed a pickaxe");
    assert!(world.registry().has::<Tool>(pickaxe));
    assert_eq!(
        world.registry().get::<ItemKind>(pickaxe),
        Some(&ItemKind(ItemKindId(9)))
    );
    assert_eq!(
        world.registry().get::<Material>(pickaxe),
        Some(&Material(MaterialId(1)))
    );
}

#[test]
fn f1_normalizes_a_flipped_registered_tool_to_its_same_semantic_kind() {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };
    use openshard_state::components::{
        ItemKind,
        Material,
        Tool,
    };

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let owner = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, owner).unwrap();

    world.queue(admin_item_response(gm, "0x0e85", "0x08ab", "1", false));
    world.tick(now);
    assert!(world.registry().query::<Contained>().any(|(item, held)| {
        held.container == pack
            && world.registry().get::<ItemKind>(item) == Some(&ItemKind(ItemKindId(9)))
            && world.registry().get::<Material>(item) == Some(&Material(MaterialId(9)))
            && world.registry().has::<Tool>(item)
    }));
}

#[test]
fn f1_creates_registered_shovel_and_fishing_pole_as_typed_harvest_tools() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        ItemKind,
        Tool,
    };

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let owner = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, owner).unwrap();
    for (graphic, kind) in [(0x0F39, ItemKindId(17)), (0x0DC0, ItemKindId(18))] {
        world.queue(admin_item_response(
            gm,
            &format!("0x{graphic:04x}"),
            "0",
            "1",
            false,
        ));
        world.tick(now);
        assert!(world.registry().query::<Contained>().any(|(item, held)| {
            held.container == pack
                && world.registry().get::<ItemKind>(item) == Some(&ItemKind(kind))
                && world.registry().has::<Tool>(item)
        }));
    }
}

#[test]
fn an_admin_created_tongs_open_blacksmithy() {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };
    use openshard_state::components::{
        ItemKind,
        Material,
        Tool,
    };

    let mut now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let actor_serial = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, actor_serial).unwrap();

    world.queue(admin_item_response(gm, "0x0fbb", "0", "1", false));
    world.tick(now);

    let tongs = world
        .registry()
        .query::<Contained>()
        .find_map(|(item, held)| {
            (held.container == pack
                && world
                    .registry()
                    .get::<Drawn>(item)
                    .is_some_and(|drawn| drawn.id == Graphic(0x0FBB)))
            .then_some(item)
        })
        .expect("the F1 item form placed tongs");
    assert!(world.registry().has::<Tool>(tongs));
    assert_eq!(
        world.registry().get::<ItemKind>(tongs),
        Some(&ItemKind(ItemKindId(10)))
    );
    assert_eq!(
        world.registry().get::<Material>(tongs),
        Some(&Material(MaterialId(1)))
    );

    let serial = world.registry().serial_of(tongs).unwrap();
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(serial.raw())),
    });
    now += TICK_INTERVAL;
    world.tick(now);
    assert_eq!(
        world
            .state
            .row_of(actor)
            .and_then(|row| row.craft_gump)
            .map(|context| context.system),
        Some(0),
        "the typed F1 tool opens its blacksmithy craft window"
    );
}

#[test]
fn f1_creates_registered_primary_craft_tools_with_identity_and_uses() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        ItemKind,
        Tool,
    };

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let owner = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, owner).unwrap();
    for (graphic, kind) in [
        (0x13E3, ItemKindId(19)), // smith hammer
        (0x0FB4, ItemKindId(20)), // sledge
        (0x0F9D, ItemKindId(21)), // sewing kit
        (0x1034, ItemKindId(22)), // saw
        (0x1EB8, ItemKindId(23)), // tinker's tools
        (0x0E9B, ItemKindId(24)), // mortar
        (0x1022, ItemKindId(25)), // fletcher's tools
        (0x1028, ItemKindId(26)), // dovetail saw
        (0x1030, ItemKindId(27)), // jointing plane
        (0x102C, ItemKindId(28)), // moulding plane
        (0x1032, ItemKindId(29)), // smoothing plane
        (0x102E, ItemKindId(30)), // carpenter nails
        (0x102A, ItemKindId(31)), // carpenter hammer
        (0x10E4, ItemKindId(32)), // draw knife
        (0x10E5, ItemKindId(33)), // froe
        (0x10E6, ItemKindId(34)), // inshave
        (0x10E7, ItemKindId(35)), // scorp
    ] {
        world.queue(admin_item_response(
            gm,
            &format!("0x{graphic:04x}"),
            "0",
            "1",
            false,
        ));
        world.tick(now);
        assert!(world.registry().query::<Contained>().any(|(item, held)| {
            held.container == pack
                && world.registry().get::<ItemKind>(item) == Some(&ItemKind(kind))
                && world.registry().has::<Tool>(item)
        }));
    }
}

#[test]
fn f1_creates_every_registered_definition_with_its_semantic_role() {
    use openshard_protocol::item_kind::ItemTag;
    use openshard_state::components::{
        Instrument,
        ItemKind,
        Runebook,
        Spellbook,
        Tool,
    };

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let owner = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, owner).unwrap();

    for definition in openshard_state::item_definition::ITEM_DEFINITIONS {
        // A `shared_art` kind's graphic is deliberately ambiguous (several
        // deeds draw the same generic scroll), so F1's legacy graphic lookup
        // cannot reach it by design — it is only ever constructed by naming
        // its `ItemKindId` directly, the same door a real deed recipe uses.
        if definition.shared_art {
            world.queue(admin_item_kind_response(gm, definition.id, None, 1));
        } else {
            world.queue(admin_item_response(
                gm,
                &format!("0x{:04x}", definition.graphic.0),
                "0",
                "1",
                false,
            ));
        }
        world.tick(now);
        let item = world
            .registry()
            .query::<Contained>()
            .find_map(|(item, held)| {
                (held.container == pack
                    && world.registry().get::<ItemKind>(item) == Some(&ItemKind(definition.id)))
                .then_some(item)
            })
            .unwrap_or_else(|| panic!("F1 did not create {} as its registered kind", definition.name));

        if definition.tags.contains(&ItemTag::Weapon) {
            assert!(
                openshard_state::weapon::weapon_data_for_kind(definition.id).is_some(),
                "{} has no semantic combat row",
                definition.name
            );
        }
        if definition.tags.contains(&ItemTag::Armor) {
            assert!(
                openshard_state::armor::armor_data_for_kind(definition.id).is_some(),
                "{} has no semantic armour row",
                definition.name
            );
        }
        if definition.tags.contains(&ItemTag::Tool) {
            assert!(
                world.registry().has::<Tool>(item),
                "{} has no tool state",
                definition.name
            );
            assert!(
                openshard_state::harvest::tool_data_for_kind(definition.id).is_some()
                    || openshard_state::craft::craft_tool_for_kind(definition.id).is_some(),
                "{} has no semantic harvest or craft role",
                definition.name
            );
        }
        if definition.tags.contains(&ItemTag::Instrument) {
            assert!(
                world.registry().has::<Instrument>(item),
                "{} has no instrument state",
                definition.name
            );
            assert!(
                openshard_state::instrument::instrument_data_for_kind(definition.id).is_some(),
                "{} has no semantic instrument role",
                definition.name
            );
        }
        if definition.tags.contains(&ItemTag::Container) {
            assert!(
                world.registry().has::<Container>(item),
                "{} has no container state",
                definition.name
            );
        }
        if definition.tags.contains(&ItemTag::Spellbook) {
            assert!(
                world.registry().has::<Spellbook>(item),
                "{} has no spellbook state",
                definition.name
            );
        }
        if definition.tags.contains(&ItemTag::Runebook) {
            assert!(
                world.registry().has::<Runebook>(item),
                "{} has no runebook state",
                definition.name
            );
        }
    }
}

#[test]
fn an_admin_created_lute_is_an_instrument() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        Instrument,
        ItemKind,
    };

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let actor_serial = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, actor_serial).unwrap();

    world.queue(admin_item_response(gm, "0x0eb3", "0", "1", false));
    world.tick(now);
    let lute = world
        .registry()
        .query::<Contained>()
        .find_map(|(item, held)| {
            (held.container == pack
                && world
                    .registry()
                    .get::<Drawn>(item)
                    .is_some_and(|drawn| drawn.id == Graphic(0x0EB3)))
            .then_some(item)
        })
        .expect("the F1 item form placed a lute");
    assert!(world.registry().has::<Instrument>(lute));
    assert_eq!(
        world.registry().get::<ItemKind>(lute),
        Some(&ItemKind(ItemKindId(11)))
    );
}

#[test]
fn f1_creates_every_registered_instrument_as_a_typed_playable_item() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        Instrument,
        ItemKind,
    };

    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let actor = world.state.players[&gm];
    let actor_serial = world.registry().serial_of(actor).unwrap();
    let pack = items::backpack_of(&world.state, actor_serial).unwrap();
    for (graphic, kind) in [
        (0x0EB1, ItemKindId(12)), // harp
        (0x0EB2, ItemKindId(13)), // lap harp
        (0x0E9C, ItemKindId(14)), // drums
        (0x0E9D, ItemKindId(15)), // tambourine
        (0x0E9E, ItemKindId(16)), // tasselled tambourine
    ] {
        world.queue(admin_item_response(
            gm,
            &format!("0x{graphic:04x}"),
            "0",
            "1",
            false,
        ));
        world.tick(now);
        assert!(world.registry().query::<Contained>().any(|(item, held)| {
            held.container == pack
                && world.registry().get::<ItemKind>(item) == Some(&ItemKind(kind))
                && world.registry().has::<Instrument>(item)
        }));
    }
}

#[test]
fn an_admin_can_place_a_catalogue_animal_on_a_targeted_tile() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let at = Point::new(START.x + 3, START.y + 2, 0);
    let _ = packets_for(&mut world, gm);

    world.queue(admin_creature_response(gm, 1)); // horse
    world.tick(now);
    assert!(
        packets_for(&mut world, gm).iter().any(|packet| packet[0] == 0x6C),
        "the location cursor is sent"
    );

    world.queue(Command::TargetResponse {
        connection: gm,
        response:   openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(0),
            object:    None,
            location:  at,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert!(
        world.registry().query::<Body>().any(|(entity, body)| {
            body.id == Graphic(200)
                && world
                    .registry()
                    .get::<Position>(entity)
                    .is_some_and(|position| position.0 == at)
        }),
        "the chosen horse is a real mobile at the targeted tile"
    );
}

#[test]
fn decorate_places_statics_and_clear_removes_them() {
    use openshard_state::components::Decoration;
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);

    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    vec![
            (
                openshard_protocol::wire::Graphic(0x07C1),
                openshard_protocol::wire::Hue(0),
                Point::new(START.x + 1, START.y, 0),
            ),
            (
                openshard_protocol::wire::Graphic(0x08DA),
                openshard_protocol::wire::Hue(0),
                Point::new(START.x + 2, START.y, 0),
            ),
        ],
        doors:      Vec::new(),
        containers: Vec::new(),
    });
    world.tick(now);
    assert_eq!(
        world.registry().query::<Decoration>().count(),
        2,
        "both decorations were placed"
    );
    // Decoration never decays.
    let decor = world.registry().query::<Decoration>().next().unwrap().0;
    assert!(!world.registry().has::<Decays>(decor), "decoration does not rot");

    world.queue(Command::ClearDecorations);
    world.tick(now);
    assert_eq!(
        world.registry().query::<Decoration>().count(),
        0,
        "clear removed the decoration"
    );
}

#[test]
fn a_guildmate_is_green_and_a_guild_at_war_is_orange() {
    use openshard_state::components::GuildMember;
    use openshard_state::guild::GuildId;

    // The colour a client is told, which is the only notoriety that is relative:
    // the same mobile is green to a guildmate and blue to a stranger.
    let now = Instant::now();
    let mut world = world();
    let first = enter(&mut world, now);
    let second = enter_as(&mut world, connection(), now);
    let one = world.state.players[&first];
    let two = world.state.players[&second];

    // Strangers first, so what changes is visible.
    assert_eq!(
        world.state.notoriety_toward(one, two),
        Notoriety::Innocent,
        "an unguilded stranger is not blue"
    );

    let serial = world.registry().serial_of(one).unwrap();
    let ours = world
        .state
        .guilds
        .found("The Silver Serpent".to_owned(), "OSS".to_owned(), serial);
    let member = |guild: GuildId| {
        GuildMember {
            guild,
            title: String::new(),
            rank: openshard_state::Rank::Member,
        }
    };
    world.state.registry.insert(one, member(ours));
    world.state.registry.insert(two, member(ours));
    assert_eq!(
        world.state.notoriety_toward(one, two),
        Notoriety::Friend,
        "a guildmate is not green"
    );

    // A second guild: unrelated is still blue, allied is green, at war is orange.
    let theirs = world
        .state
        .guilds
        .found("The Black Rose".to_owned(), "TBR".to_owned(), serial);
    world.state.registry.insert(two, member(theirs));
    assert_eq!(
        world.state.notoriety_toward(one, two),
        Notoriety::Innocent,
        "an undeclared guild is not blue"
    );

    // An alliance is a named group both are in, and green follows membership of
    // it rather than a fact about the pair.
    let alliance = world
        .state
        .alliances
        .found("The Northern Compact".to_owned(), ours, theirs);
    world.state.alliances.accept(alliance, theirs);
    for guild in [ours, theirs] {
        world.state.guilds.get_mut(guild).unwrap().alliance = Some(alliance);
    }
    assert_eq!(world.state.notoriety_toward(one, two), Notoriety::Friend);
    // Both ways: the colour must not depend on which one is asked about.
    assert_eq!(world.state.notoriety_toward(two, one), Notoriety::Friend);

    world.state.guilds.declare(ours, theirs);
    assert_eq!(
        world.state.notoriety_toward(one, two),
        Notoriety::Enemy,
        "a war inside an alliance read green"
    );
    assert_eq!(world.state.notoriety_toward(two, one), Notoriety::Enemy);
}

#[test]
fn a_murderer_stays_red_inside_a_guild_tabard() {
    use openshard_state::components::GuildMember;

    // ServUO's order, and the reason for it: standing is asked before any guild
    // question, so a red cannot hide behind a guildmate's green.
    let now = Instant::now();
    let mut world = world();
    let first = enter(&mut world, now);
    let second = enter_as(&mut world, connection(), now);
    let one = world.state.players[&first];
    let two = world.state.players[&second];

    let serial = world.registry().serial_of(one).unwrap();
    let ours = world
        .state
        .guilds
        .found("The Silver Serpent".to_owned(), "OSS".to_owned(), serial);
    for who in [one, two] {
        world.state.registry.insert(
            who,
            GuildMember {
                guild: ours,
                title: String::new(),
                rank:  openshard_state::Rank::Member,
            },
        );
    }
    assert_eq!(world.state.notoriety_toward(one, two), Notoriety::Friend);

    world.state.registry.insert(two, Notoriety::Murderer);
    assert_eq!(
        world.state.notoriety_toward(one, two),
        Notoriety::Murderer,
        "a murderer read green to its own guild"
    );
    world.state.registry.insert(two, Notoriety::Criminal);
    assert_eq!(
        world.state.notoriety_toward(one, two),
        Notoriety::Criminal,
        "a criminal read green to its own guild"
    );
}

#[test]
fn decorating_twice_does_not_lay_a_second_britain() {
    use openshard_state::components::Decoration;
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);

    // Two statics on two tiles, one door, one container — one of each kind that
    // `decorate` places, because each was its own loop and each needed the guard.
    let batch = || {
        Command::Decorate {
            facet:      Facet(0),
            statics:    vec![
                (
                    openshard_protocol::wire::Graphic(0x07C1),
                    openshard_protocol::wire::Hue(0),
                    Point::new(START.x + 1, START.y, 0),
                ),
                (
                    openshard_protocol::wire::Graphic(0x08DA),
                    openshard_protocol::wire::Hue(0),
                    Point::new(START.x + 2, START.y, 0),
                ),
            ],
            doors:      vec![DecorDoor {
                lock:     None,
                closed:   openshard_protocol::wire::Graphic(0x06A5),
                open:     openshard_protocol::wire::Graphic(0x06A6),
                offset_x: -1,
                offset_y: 1,
                position: Point::new(START.x + 3, START.y, 0),
            }],
            containers: vec![DecorContainer {
                lock:     None,
                graphic:  openshard_protocol::wire::Graphic(0x0E77),
                gump:     openshard_protocol::wire::Graphic(0x003E),
                hue:      openshard_protocol::wire::Hue(0),
                position: Point::new(START.x + 4, START.y, 0),
            }],
        }
    };

    world.queue(batch());
    world.tick(now);
    let after_first = world.registry().query::<Decoration>().count();
    assert_eq!(after_first, 4, "the batch did not lay what it was given");

    // The case this guards: a staff member pressing the button again, or a boot
    // that seeds `decorate:` on a shard whose decoration was restored. Before,
    // this left two of every sign and a door opening into its own twin.
    world.queue(batch());
    world.tick(now);
    assert_eq!(
        world.registry().query::<Decoration>().count(),
        after_first,
        "decorating twice laid a second copy of everything"
    );
}

#[test]
fn placing_the_townsfolk_twice_does_not_double_the_town() {
    use openshard_state::components::Title;
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);

    // Two shopkeepers, one of them stocked: the stocking is additive, so a
    // second placement would both duplicate the vendor and refill its crate.
    let blacksmith = |title: &str, x: u16| {
        Command::SpawnMobile {
            body:        openshard_protocol::wire::Graphic(400),
            hue:         openshard_protocol::wire::Hue(0),
            hits:        100,
            notoriety:   Notoriety::from_bits(1),
            damage:      0,
            resistance:  openshard_protocol::world::PhysicalResistance::new(0),
            swing:       0,
            sight:       openshard_protocol::world::Sight(0),
            aggression:  openshard_protocol::world::Aggression::Passive,
            beat:        0,
            ranged:      None,
            ranged_kind: DamageType::Physical,
            wander:      false,
            position:    Point::new(x, START.y + 2, 0),
            facet:       Facet(0),
            name:        None,
            title:       Some(title.to_owned()),
            shoe:        1,
            fame:        0,
            karma:       0,
            night_home:  None,
            banker:      false,
            vendor:      true,
            healer:      false,
            equipment:   Vec::new(),
            skills:      Vec::new(),
            stock:       vec![openshard_npc::StockLine {
                graphic:   openshard_protocol::wire::Graphic(0x1BEF),
                hue:       openshard_protocol::wire::Hue(0),
                item_kind: None,
                material:  None,
                amount:    openshard_state::components::Amount(16),
                price:     openshard_state::components::Price(5),
                name:      "iron ingot".to_owned(),
            }],
            escort_to:   None,
            quests:      Vec::new(),
        }
    };

    for command in [
        blacksmith("the blacksmith", START.x + 1),
        blacksmith("the baker", START.x + 3),
    ] {
        world.queue(command);
    }
    world.tick(now);
    let placed = world.registry().query::<Title>().count();
    assert_eq!(placed, 2, "the two townsfolk were not placed");

    // The second press. Before this, a restored shard seeded with `populate:`
    // grew a second banker inside the first — and half of them were missed by a
    // check against where they were *standing*, because a townsperson drifts
    // around its post between beats.
    for command in [
        blacksmith("the blacksmith", START.x + 1),
        blacksmith("the baker", START.x + 3),
    ] {
        world.queue(command);
    }
    world.tick(now);
    assert_eq!(
        world.registry().query::<Title>().count(),
        placed,
        "placing the townsfolk twice doubled the town"
    );
}

#[test]
fn a_townsperson_of_another_trade_may_share_a_post() {
    use openshard_state::components::Title;
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);

    // The key is the post *and* the trade. Two trades on one tile is not what the
    // shipped data does — `build.rs` rejects it — but the guard must not be a
    // blanket "something already stands here", or a future dataset that stacks a
    // guard beside a banker would silently lose one of them.
    let at = Point::new(START.x + 1, START.y + 2, 0);
    let person = |title: &str| {
        Command::SpawnMobile {
            body:        openshard_protocol::wire::Graphic(400),
            hue:         openshard_protocol::wire::Hue(0),
            hits:        100,
            notoriety:   Notoriety::from_bits(1),
            damage:      0,
            resistance:  openshard_protocol::world::PhysicalResistance::new(0),
            swing:       0,
            sight:       openshard_protocol::world::Sight(0),
            aggression:  openshard_protocol::world::Aggression::Passive,
            beat:        0,
            ranged:      None,
            ranged_kind: DamageType::Physical,
            wander:      false,
            position:    at,
            facet:       Facet(0),
            name:        None,
            title:       Some(title.to_owned()),
            shoe:        1,
            fame:        0,
            karma:       0,
            night_home:  None,
            banker:      false,
            vendor:      false,
            healer:      false,
            equipment:   Vec::new(),
            skills:      Vec::new(),
            stock:       Vec::new(),
            escort_to:   None,
            quests:      Vec::new(),
        }
    };
    world.queue(person("the banker"));
    world.queue(person("the guard"));
    world.tick(now);
    assert_eq!(
        world.registry().query::<Title>().count(),
        2,
        "the second trade on the tile was mistaken for a duplicate"
    );
}

#[test]
fn a_batch_may_repeat_itself_and_both_copies_are_placed() {
    use openshard_state::components::Decoration;
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);

    // Thirty-nine of the shipped statics repeat an exact graphic and position, and
    // that is ordinary in UO decoration. The de-duplication is against the world as
    // it stood when the batch began, so a batch that repeats itself still lays both
    // — otherwise moving the data in-tree would have quietly deleted thirty-nine
    // pieces of Britain.
    let same = (
        openshard_protocol::wire::Graphic(0x07C1),
        openshard_protocol::wire::Hue(0),
        Point::new(START.x + 1, START.y, 0),
    );
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    vec![same, same],
        doors:      Vec::new(),
        containers: Vec::new(),
    });
    world.tick(now);
    assert_eq!(
        world.registry().query::<Decoration>().count(),
        2,
        "a batch that repeats a row had one of them dropped"
    );
}

#[test]
fn decoration_cannot_be_picked_up() {
    use openshard_state::components::Decoration;
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    vec![(
            openshard_protocol::wire::Graphic(0x07C1),
            openshard_protocol::wire::Hue(0),
            Point::new(START.x, START.y, 0),
        )],
        doors:      Vec::new(),
        containers: Vec::new(),
    });
    world.tick(now);
    let decor = world.registry().query::<Decoration>().next().unwrap().0;
    let serial = world.registry().serial_of(decor).unwrap();
    let _ = packets_for(&mut world, gm);

    world.queue(Command::PickUpItem {
        connection: gm,
        serial:     RawSerial(serial.raw()),
        amount:     1,
    });
    world.tick(now);

    assert!(
        world.state.held_of(gm).is_none(),
        "a town's fittings are not loot"
    );
    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x27),
        "the lift is refused with a drag-cancel"
    );
}

#[test]
fn a_door_opens_and_closes_on_double_click() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    // A metal door one tile from the GM, well within reach.
    let at = Point::new(START.x + 1, START.y, 0);
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    Vec::new(),
        doors:      vec![DecorDoor {
            lock:     None,
            closed:   openshard_protocol::wire::Graphic(0x0675),
            open:     openshard_protocol::wire::Graphic(0x0676),
            offset_x: -1,
            offset_y: 1,
            position: at,
        }],
        containers: Vec::new(),
    });
    world.tick(now);
    let door = world.registry().query::<Door>().next().unwrap().0;
    let serial = world.registry().serial_of(door).unwrap();

    let _ = packets_for(&mut world, gm); // drain the login/decorate burst

    // Double-click opens it: the graphic becomes the open art and it hops by
    // the hinge offset.
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(serial.raw())),
    });
    world.tick(now);
    assert_eq!(
        world.registry().get::<Drawn>(door).unwrap().id,
        openshard_protocol::wire::Graphic(0x0676),
        "the door drew open"
    );
    assert_eq!(
        world.registry().get::<Position>(door).unwrap().0,
        Point::new(START.x, START.y + 1, 0),
        "it swung aside by its hinge offset"
    );
    assert!(world.registry().get::<Door>(door).unwrap().is_open);
    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x54),
        "the door creaks — a 0x54 sound to everyone who sees it swing"
    );

    // Double-clicking again shuts it and returns it to its frame.
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(serial.raw())),
    });
    world.tick(now);
    assert_eq!(
        world.registry().get::<Drawn>(door).unwrap().id,
        openshard_protocol::wire::Graphic(0x0675)
    );
    assert_eq!(world.registry().get::<Position>(door).unwrap().0, at);
    assert!(!world.registry().get::<Door>(door).unwrap().is_open);
}

#[test]
fn an_open_door_swings_shut_on_its_own() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let at = Point::new(START.x + 1, START.y, 0);
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    Vec::new(),
        doors:      vec![DecorDoor {
            lock:     None,
            closed:   openshard_protocol::wire::Graphic(0x0675),
            open:     openshard_protocol::wire::Graphic(0x0676),
            offset_x: -1,
            offset_y: 1,
            position: at,
        }],
        containers: Vec::new(),
    });
    world.tick(now);
    let door = world.registry().query::<Door>().next().unwrap().0;
    let serial = world.registry().serial_of(door).unwrap();

    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(serial.raw())),
    });
    world.tick(now);
    assert!(world.registry().get::<Door>(door).unwrap().is_open);

    // Run past the auto-close delay: the door closes itself, untouched.
    let close_at = world.registry().get::<Door>(door).unwrap().close_at;
    while world.state.ticks < close_at {
        world.tick(now);
    }
    assert!(
        !world.registry().get::<Door>(door).unwrap().is_open,
        "the door swung shut on its own"
    );
    assert_eq!(world.registry().get::<Position>(door).unwrap().0, at);
}

#[test]
fn linked_leaves_toggle_and_auto_close_as_one_doorway() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let first_at = Point::new(START.x + 1, START.y, 0);
    let second_at = Point::new(START.x + 2, START.y, 0);
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    Vec::new(),
        doors:      vec![
            DecorDoor {
                lock:     None,
                closed:   openshard_protocol::wire::Graphic(0x06A5),
                open:     openshard_protocol::wire::Graphic(0x06A6),
                offset_x: -1,
                offset_y: 1,
                position: first_at,
            },
            DecorDoor {
                lock:     None,
                closed:   openshard_protocol::wire::Graphic(0x06A7),
                open:     openshard_protocol::wire::Graphic(0x06A8),
                offset_x: 1,
                offset_y: 1,
                position: second_at,
            },
        ],
        containers: Vec::new(),
    });
    world.tick(now);

    let first = world
        .registry()
        .query::<Door>()
        .find(|(entity, _)| {
            world
                .registry()
                .get::<Position>(*entity)
                .is_some_and(|p| p.0 == first_at)
        })
        .unwrap()
        .0;
    let second = world
        .registry()
        .query::<Door>()
        .find(|(entity, _)| {
            world
                .registry()
                .get::<Position>(*entity)
                .is_some_and(|p| p.0 == second_at)
        })
        .unwrap()
        .0;
    let first_serial = world.registry().serial_of(first).unwrap();
    let second_serial = world.registry().serial_of(second).unwrap();
    let mut first_door = *world.registry().get::<Door>(first).unwrap();
    first_door.link = Some(second_serial);
    world.state.registry.insert(first, first_door);
    let mut second_door = *world.registry().get::<Door>(second).unwrap();
    second_door.link = Some(first_serial);
    world.state.registry.insert(second, second_door);

    openshard_items::toggle_door(&mut world.state, player, first, first_serial);
    assert!(world.registry().get::<Door>(first).unwrap().is_open);
    assert!(world.registry().get::<Door>(second).unwrap().is_open);
    assert!(
        world
            .state
            .facet_state(Facet(0))
            .obstructions()
            .blocker_at(first_at.x, first_at.y)
            .is_none()
    );
    assert!(
        world
            .state
            .facet_state(Facet(0))
            .obstructions()
            .blocker_at(second_at.x, second_at.y)
            .is_none()
    );

    // One body anywhere under the two closed positions defers the whole pair.
    teleport(&mut world, gm, second_at);
    let close_at = world.registry().get::<Door>(first).unwrap().close_at;
    let mut later = now;
    while world.state.ticks < close_at {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(world.registry().get::<Door>(first).unwrap().is_open);
    assert!(world.registry().get::<Door>(second).unwrap().is_open);

    teleport(&mut world, gm, Point::new(START.x, START.y, 0));
    world.tick(later + TICK_INTERVAL);
    assert!(!world.registry().get::<Door>(first).unwrap().is_open);
    assert!(!world.registry().get::<Door>(second).unwrap().is_open);
}

/// The auto-door opens every shut tile a diagonal crosses. That is two use
/// packets for a double doorway, but linked leaves are one toggle: after the
/// first packet opens both, the second must not slam them shut before the walk.
#[test]
fn a_diagonal_auto_door_use_keeps_a_linked_double_doorway_open() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let west_at = Point::new(START.x, START.y - 1, Z_WITHOUT_A_MAP);
    let east_at = Point::new(START.x + 1, START.y - 1, Z_WITHOUT_A_MAP);
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    Vec::new(),
        doors:      vec![
            DecorDoor {
                lock:     None,
                closed:   openshard_protocol::wire::Graphic(0x06A5),
                open:     openshard_protocol::wire::Graphic(0x06A6),
                offset_x: -1,
                offset_y: 1,
                position: west_at,
            },
            DecorDoor {
                lock:     None,
                closed:   openshard_protocol::wire::Graphic(0x06A7),
                open:     openshard_protocol::wire::Graphic(0x06A8),
                offset_x: 1,
                offset_y: 1,
                position: east_at,
            },
        ],
        containers: Vec::new(),
    });
    world.tick(now);

    let leaves: Vec<_> = world
        .registry()
        .query::<Door>()
        .map(|(entity, _)| (entity, world.registry().serial_of(entity).unwrap()))
        .collect();
    assert_eq!(leaves.len(), 2, "the fixture has the two leaves to link");
    let (west, west_serial) = leaves
        .iter()
        .copied()
        .find(|(entity, _)| {
            world
                .registry()
                .get::<Position>(*entity)
                .is_some_and(|at| at.0 == west_at)
        })
        .unwrap();
    let (east, east_serial) = leaves
        .iter()
        .copied()
        .find(|(entity, _)| {
            world
                .registry()
                .get::<Position>(*entity)
                .is_some_and(|at| at.0 == east_at)
        })
        .unwrap();
    let mut west_door = *world.registry().get::<Door>(west).unwrap();
    west_door.link = Some(east_serial);
    world.state.registry.insert(west, west_door);
    let mut east_door = *world.registry().get::<Door>(east).unwrap();
    east_door.link = Some(west_serial);
    world.state.registry.insert(east, east_door);

    // This is the exact wire order of `App::open_door_ahead` then `App::walk`:
    // the landing leaf, the blocked flank, then the diagonal step.
    let entity = world.state.players[&player];
    world.state.registry.insert(
        entity,
        Movement(openshard_movement::Walker::new(
            Point::new(START.x, START.y, Z_WITHOUT_A_MAP),
            Facing::walking(Direction::NorthEast),
        )),
    );
    for serial in [east_serial, west_serial] {
        world.queue(Command::DoubleClick {
            connection: player,
            request:    UseRequest::Use(RawSerial(serial.raw())),
        });
    }
    world.queue(Command::Walk {
        connection: player,
        request:    walk(0, Direction::NorthEast),
    });
    world.tick(now);

    assert!(world.registry().get::<Door>(west).unwrap().is_open);
    assert!(world.registry().get::<Door>(east).unwrap().is_open);
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        east_at,
        "the now-open flank no longer refuses the diagonal"
    );

    // The coalescing stops at the tick boundary: a subsequent real click is
    // still the normal command to close the doorway.
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(east_serial.raw())),
    });
    world.tick(now);
    assert!(!world.registry().get::<Door>(west).unwrap().is_open);
    assert!(!world.registry().get::<Door>(east).unwrap().is_open);
}

/// One west door frame at (100, 100) and one east frame at (102, 100) — a
/// single-door gap for the generator to fill. `walled` bricks the gap up.
///
/// **A real map.** The frames are statics on a [`Scene`]'s
/// [`WorldMap`](openshard_map::map::WorldMap) under their own id, and the bricked
/// gap is a wall standing in it — so `can_fit` refuses the doorway through the
/// shard's own rule rather than by a fixture matching a coordinate. The double
/// this replaced answered `can_fit` false at exactly (101, 100) and true
/// everywhere else, including inside its own frames, which is not a building.
fn door_frames(walled: bool) -> Scene {
    // 0x0007 is both a west and an east frame.
    const FRAME: u16 = 0x0007;
    let mut scene = Scene::flat_holding(102, 100, 0);
    scene.art(FRAME, WALL_FLAGS, 20);
    scene.put(100, 100, 0, FRAME);
    scene.put(102, 100, 0, FRAME);
    if walled {
        scene.wall(101, 100, 0, 20);
    }
    scene
}

/// The same frame pair three tiles apart: two gap tiles, hence two linked
/// leaves rather than a single door.
fn double_door_frames() -> Scene {
    const FRAME: u16 = 0x0007;
    let mut scene = Scene::flat_holding(103, 100, 0);
    scene.art(FRAME, WALL_FLAGS, 20);
    scene.put(100, 100, 0, FRAME);
    scene.put(103, 100, 0, FRAME);
    scene
}

/// Give the default facet a scene, and the shard the table that scene reads.
fn stand_on(world: &mut World, scene: Scene) {
    let (map, tiles) = scene.into_shard(Facet(0));
    world.state.facet_state_mut(Facet(0)).set_map(Some(map), &tiles);
    world.state.set_tiles(tiles);
}

fn generate_britain_doors(world: &mut World, now: Instant) {
    world.queue(Command::GenerateDoors {
        facet:  Facet(0),
        x:      100,
        y:      100,
        width:  3,
        height: 1,
    });
    world.tick(now);
}

#[test]
fn doors_are_generated_between_static_frames() {
    let now = Instant::now();
    let mut world = world();
    stand_on(&mut world, door_frames(false));

    generate_britain_doors(&mut world, now);

    let (entity, door) = world
        .registry()
        .query::<Door>()
        .next()
        .expect("a door was generated");
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        Point::new(101, 100, 0),
        "the door fills the gap between the frames"
    );
    // A DarkWoodDoor, WestCW: closed 0x06A5, open 0x06A6, hinge (-1, 1).
    assert_eq!(door.closed, openshard_protocol::wire::Graphic(0x06A5));
    assert_eq!(door.open, openshard_protocol::wire::Graphic(0x06A6));
    assert_eq!((door.offset_x, door.offset_y), (-1, 1));
    assert!(
        world.registry().has::<Decoration>(entity),
        "a generated door is decoration"
    );

    // Running the pass again puts no second door on the same gap.
    generate_britain_doors(&mut world, now);
    assert_eq!(
        world.registry().query::<Door>().count(),
        1,
        "a tile that already has a door is not doored again"
    );
}

#[test]
fn generated_double_door_leaves_link_each_other() {
    let now = Instant::now();
    let mut world = world();
    stand_on(&mut world, double_door_frames());
    world.queue(Command::GenerateDoors {
        facet:  Facet(0),
        x:      100,
        y:      100,
        width:  4,
        height: 1,
    });
    world.tick(now);

    let mut leaves: Vec<_> = world
        .registry()
        .query::<Door>()
        .map(|(entity, door)| {
            (
                world.registry().get::<Position>(entity).unwrap().0.x,
                world.registry().serial_of(entity).unwrap(),
                door.link,
            )
        })
        .collect();
    leaves.sort_unstable_by_key(|leaf| leaf.0);
    assert_eq!(leaves.len(), 2);
    assert_eq!(leaves[0].2, Some(leaves[1].1));
    assert_eq!(leaves[1].2, Some(leaves[0].1));

    let records = world.decoration_records();
    let mut restored = World::new(START);
    restored.restore_decorations(records);
    for &(_, serial, link) in &leaves {
        let entity = restored
            .registry()
            .entity_of(serial)
            .expect("the linked leaf came back");
        assert_eq!(restored.registry().get::<Door>(entity).unwrap().link, link);
        assert!(
            link.and_then(|other| restored.registry().entity_of(other))
                .is_some()
        );
    }
}

#[test]
fn no_door_is_generated_into_a_wall() {
    let now = Instant::now();
    let mut world = world();
    stand_on(&mut world, door_frames(true));

    generate_britain_doors(&mut world, now);

    assert_eq!(
        world.registry().query::<Door>().count(),
        0,
        "an obstructed gap is a wall, not a doorway"
    );
}

#[test]
fn a_decoration_container_opens_on_double_click() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    Vec::new(),
        doors:      Vec::new(),
        containers: vec![DecorContainer {
            lock:     None,
            graphic:  openshard_protocol::wire::Graphic(0x0E42),
            gump:     openshard_protocol::wire::Graphic(0x49),
            hue:      openshard_protocol::wire::Hue(0),
            position: Point::new(START.x + 1, START.y, 0),
        }],
    });
    world.tick(now);
    // The one container that is decoration — the GM also wears a backpack,
    // which is a container too.
    let chest = world
        .registry()
        .query::<Container>()
        .map(|(entity, _)| entity)
        .find(|&entity| world.registry().has::<Decoration>(entity))
        .expect("a decoration container is on the ground");
    let serial = world.registry().serial_of(chest).unwrap();
    let _ = packets_for(&mut world, gm);

    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(serial.raw())),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x24),
        "the container gump opened"
    );
}

#[test]
fn the_deco_button_emits_the_pack_verb() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let mut actions: Cursor<AdminMenuAction> = world.bus().cursor();

    world.queue(admin_response(gm, 22)); // Decorate Felucca
    world.tick(now);

    let events: Vec<AdminMenuAction> = world.bus().read(&mut actions).cloned().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, "decorate:felucca");
}

#[test]
fn a_pressed_button_draws_the_menu_again() {
    // A reply button closes the gump at the client's end, so a menu that is
    // meant to stay up is one the shard re-draws on every press. Without this
    // the operator pays a `.admin` between every two verbs.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let _ = packets_for(&mut world, gm);

    world.queue(admin_response(gm, 22)); // Decorate Felucca
    world.tick(now);

    let packets = packets_for(&mut world, gm);
    assert!(
        packets.iter().any(|packet| packet[0] == 0xB0),
        "the menu was drawn again after the button"
    );
}

#[test]
fn the_populate_button_emits_an_admin_action_for_the_pack() {
    // The engine holds no spawn data now: the button emits a verb the script
    // pack acts on. Here we assert the verb reaches the bus; the pack turning
    // it into spawners is a scripting test.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let mut actions: Cursor<AdminMenuAction> = world.bus().cursor();

    world.queue(admin_response(gm, 13)); // Populate Felucca
    world.tick(now);

    let events: Vec<AdminMenuAction> = world.bus().read(&mut actions).cloned().collect();
    assert_eq!(events.len(), 1, "one admin action was emitted");
    assert_eq!(events[0].action, "populate:felucca");
}

#[test]
fn a_seeded_verb_is_the_button_without_a_game_master() {
    // What `--seed` is: the same event the button sends, from a shard with no
    // client attached at all. The serial is `None` and must stay so — a
    // placeholder there would name whichever entity happened to hold it, and a
    // pack reading it would be told a lie rather than "nobody".
    let now = Instant::now();
    let mut world = world();
    let mut actions: Cursor<AdminMenuAction> = world.bus().cursor();

    world.seed("decorate:felucca");
    world.tick(now);

    let events: Vec<AdminMenuAction> = world.bus().read(&mut actions).cloned().collect();
    assert_eq!(events.len(), 1, "one admin action was emitted");
    assert_eq!(events[0].action, "decorate:felucca");
    assert_eq!(events[0].serial, None, "nobody pressed it");
}

#[test]
fn a_verb_seeded_before_the_first_tick_survives_that_tick() {
    // The window `run_shard` seeds in: after the script host takes its cursors,
    // before any tick has run. The bus retires events at the *end* of a tick, so
    // a verb sent here has to still be readable after the first one — if it were
    // retired with the tick it was sent before, a seeded shard would lay nothing
    // and say nothing about it.
    let now = Instant::now();
    let mut world = world();
    // The cursor a script bridge would hold, taken before the seed as `Scripts`
    // is built before it.
    let mut actions: Cursor<AdminMenuAction> = world.bus().cursor();

    world.seed("regions:felucca");
    world.tick(now);

    let events: Vec<AdminMenuAction> = world.bus().read(&mut actions).cloned().collect();
    assert_eq!(
        events.iter().map(|e| e.action.as_str()).collect::<Vec<_>>(),
        ["regions:felucca"],
        "the verb survived the tick it was sent before"
    );
}

#[test]
fn an_admin_button_from_a_non_staff_client_is_ignored() {
    // The gump id is not a secret, so a plain player could forge a 0xB1 for
    // it. The gate must be on the response, not only the .admin that opened it.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now); // ordinary Player access
    let _ = packets_for(&mut world, player);

    world.queue(admin_response(player, 12)); // Clear
    world.tick(now);

    assert!(
        !packets_for(&mut world, player).iter().any(|p| p[0] == 0x1C),
        "a non-staff forged response does nothing"
    );
}

#[test]
fn a_spawner_fills_to_its_ceiling_and_clear_empties_it() {
    use crate::spawner::{
        CreatureTemplate,
        SpawnArea,
        Spawner,
    };
    let now = Instant::now();
    let mut world = world();
    let creature = CreatureTemplate {
        fame:        0,
        karma:       0,
        body:        openshard_protocol::wire::Graphic(0x0009),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        10,
        notoriety:   openshard_protocol::mobile::Notoriety::Neutral,
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        skills:      Vec::new(),
    };
    let area = SpawnArea {
        x:      START.x,
        y:      START.y,
        width:  3,
        height: 3,
        facet:  Facet(0),
    };
    world.queue(Command::RegisterSpawner {
        spawner: Spawner::new(
            openshard_state::SpawnerId::PLACEHOLDER,
            area,
            vec![creature],
            3,
            0,
        ),
    });

    // One creature per region per pass, so a few ticks fill it to the ceiling
    // and no further.
    for _ in 0..6 {
        world.tick(now);
    }
    assert_eq!(
        world.registry().query::<SpawnedBy>().count(),
        3,
        "the region filled to its ceiling and stopped"
    );

    world.queue(Command::ClearSpawners);
    world.tick(now);
    assert_eq!(
        world.registry().query::<SpawnedBy>().count(),
        0,
        "clear removed the region and its creatures"
    );
}

#[test]
fn clear_also_removes_placed_npcs_and_their_gear_but_not_players() {
    // "Populate" places named townsfolk and vendors directly, with no SpawnedBy
    // tag; a clear that only swept SpawnedBy left them standing, which read as
    // "clear did nothing". The full reset takes them and their stock crate too,
    // while the living player is untouched.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];

    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(1),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x + 1, START.y, 0),
        facet:       Facet(0),
        name:        Some("Mirabel".to_owned()),
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      true,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let vendor = world
        .state
        .registry
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a placed vendor");
    let vendor_serial = world.registry().serial_of(vendor).unwrap();
    world.queue(Command::StockVendor {
        serial: vendor_serial,
        stock:  vec![npc::StockLine {
            graphic:   openshard_protocol::wire::Graphic(0x0F7A),
            hue:       openshard_protocol::wire::Hue(0),
            item_kind: None,
            material:  None,
            amount:    openshard_state::components::Amount(50),
            price:     openshard_state::components::Price(4),
            name:      "black pearl".to_owned(),
        }],
    });
    world.tick(now);
    assert!(
        world
            .registry()
            .query::<openshard_state::components::Price>()
            .next()
            .is_some(),
        "the vendor was stocked"
    );

    world.queue(Command::ClearSpawners);
    world.tick(now);
    assert!(
        world
            .registry()
            .query::<openshard_state::components::Vendor>()
            .next()
            .is_none(),
        "the placed vendor is gone, SpawnedBy or not"
    );
    assert!(
        world
            .registry()
            .query::<openshard_state::components::Price>()
            .next()
            .is_none(),
        "and its stock crate and wares went with it"
    );
    assert!(
        world.registry().get::<Position>(player).is_some(),
        "the living player is left standing"
    );
}

#[test]
fn a_creature_can_be_made_to_speak() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.x, START.y, 0), 50, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::Speak {
        serial: mob,
        hue:    Hue(0),
        text:   "grrr".to_owned(),
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| p[0] == 0xAE && mentions(p, mob)),
        "the player hears the creature the script gave a voice"
    );
}

#[test]
fn a_command_does_nothing_until_the_tick() {
    // The whole boundary. If queueing acted immediately, world code would run
    // on a network thread at an arbitrary point, and two clients racing would
    // produce a different world depending on which packet won.
    let mut world = world();
    world.queue(Command::Enter(Entering {
        connection: connection(),
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        character:  Character::fresh(Facet(0)),
    }));

    assert_eq!(world.player_count(), 0, "queued, not applied");
    assert_eq!(world.drain_outbound().count(), 0, "and nothing sent");

    world.tick(Instant::now());
    assert_eq!(world.player_count(), 1);
}

#[test]
fn entering_sends_the_sequence_the_client_needs() {
    let mut world = world();
    enter(&mut world, Instant::now());

    let ids: Vec<u8> = world.drain_outbound().map(|out| out.packet[0]).collect();
    assert_eq!(
        ids,
        vec![
            0x1B, 0xBF, 0xB9, 0xBC, 0x65, 0x20, 0x4F, 0x11, 0x3A, 0xBF, 0x78, 0xBF, 0x55
        ],
        "0x1B first or there is no body; 0x55 last or the client draws early; \
             0xB9 AoS features after the map change (ServUO's DoLogin order), or a \
             modern client shows no tooltips; 0xBC season and 0x65 weather between \
             the map change and the player update, before the first world frame; \
             0x11 status and the 0x78 of the \
             player's own equipment before it, or the client has no stamina and no \
             backpack serial to open; 0x3A fills the skill window, and the second \
             0xBF after it carries the three stat arrows — nothing else sends them, \
             so without it the bar draws all three pointing up; the third 0xBF is \
             this engine's own authority notice, last before the 0x55 because it is \
             the one packet here no reference client asked for — a stock client \
             skips it, and ours needs it before a word can be typed"
    );
}

#[test]
fn entering_sends_a_status_with_running_stamina() {
    // The fix for "cannot run": the client reads stamina from the 0x11, and a
    // zero there means walk-only. This is the byte that lets a player run.
    let mut world = world();
    enter(&mut world, Instant::now());

    let status = world
        .drain_outbound()
        .map(|out| out.packet)
        .find(|p| p[0] == 0x11)
        .expect("a status packet on world entry");
    let stamina = u16::from_be_bytes([status[50], status[51]]);
    assert!(stamina > 0, "stamina is zero; the client will refuse to run");
}

#[test]
fn a_status_request_is_answered_with_a_status() {
    // Opening the paperdoll (0x34) after entry resends the status.
    let mut world = world();
    let connection = enter(&mut world, Instant::now());
    let _ = world.drain_outbound().count();

    world.queue(Command::RequestStatus { connection });
    world.tick(Instant::now());

    assert!(
        packets_for(&mut world, connection).iter().any(|p| p[0] == 0x11),
        "a 0x34 should be answered with a 0x11"
    );
}

#[test]
fn entering_builds_an_entity_out_of_components() {
    let mut world = world();
    enter(&mut world, Instant::now());

    let entity = *world.state.players.values().next().unwrap();
    assert!(world.registry().has::<Position>(entity));
    assert!(world.registry().has::<Body>(entity));
    assert!(world.registry().has::<Name>(entity));
    assert!(world.registry().has::<Movement>(entity), "a player walks");
    assert!(world.registry().has::<Client>(entity), "and has a connection");
    assert!(world.registry().serial_of(entity).is_some());
}

#[test]
fn a_created_character_enters_with_its_chosen_body() {
    // Character creation carries the body and hue the player picked; the
    // world must spawn that rather than its default human male.
    let mut world = world();
    let connection = connection();
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Nyx".to_owned()),
        access: AccessLevel::Player,
        character: Character::Fresh(FreshCharacter {
            facet:      Facet(0),
            start:      None,
            appearance: Some(Appearance {
                body: Graphic(0x025E),
                hue:  openshard_protocol::wire::Hue(0x0430),
            }),
            sheet:      None,
        }),
    }));
    world.tick(Instant::now());

    let entity = world.state.players[&connection];
    let body = world.registry().get::<Body>(entity).copied().unwrap();
    assert_eq!(body.id.0, 0x025E, "the elf-female body the client chose");
    assert_eq!(body.hue.0, 0x0430);

    // And 0x1B tells the client the same body.
    let start = packets_for(&mut world, connection)
        .into_iter()
        .find(|packet| packet[0] == 0x1B)
        .expect("a PlayerStart");
    assert_eq!(
        &start[9..11],
        &0x025Eu16.to_be_bytes(),
        "0x1B carries the chosen body"
    );
}

#[test]
fn a_played_character_keeps_the_default_body() {
    // The `None` path: playing an existing character has no appearance yet,
    // so the world uses its default and does not send a body of zero.
    let mut world = world();
    let connection = enter(&mut world, Instant::now());
    let entity = world.state.players[&connection];
    let body = world.registry().get::<Body>(entity).copied().unwrap();
    assert_eq!(body.id, BODY_HUMAN_MALE);
    assert_eq!(body.hue.0, DEFAULT_HUE);
}

#[test]
fn a_characters_inventory_survives_a_logout_and_restore() {
    use openshard_protocol::serial::SerialKind;

    // A character with something in its backpack logs out; a fresh shard loads
    // the saved items and the same character logs back in to find them.
    let mut home = world();
    let now = Instant::now();
    let conn_a = enter(&mut home, now);
    let entity = home.state.players[&conn_a];
    let char_serial = home.registry().serial_of(entity).unwrap();

    // The backpack it was equipped on entry.
    let (backpack, _) = home
        .registry()
        .query::<Equipped>()
        .find(|(_, worn)| worn.layer == items::BACKPACK_LAYER)
        .expect("a backpack was equipped");
    let backpack_serial = home.registry().serial_of(backpack).unwrap();

    // A stack of gold inside it.
    let (gold, gold_serial) = home.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    home.state.registry.insert(
        gold,
        Drawn {
            id:  openshard_protocol::wire::Graphic(0x0EED),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    home.state.registry.insert(gold, Amount(500));
    home.state.registry.insert(gold, Stackable);
    openshard_state::establish_item_location(
        &mut home.state,
        gold,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack_serial,
            position:  GumpPoint::new(40, 65),
            grid:      GridSlot(0),
        }),
    )
    .unwrap();

    // What persistence would carry: the backpack (worn) and the gold (inside).
    let records = home.inventory_of(entity);
    assert!(
        records.iter().any(|r| r.serial == gold_serial && r.stackable),
        "the gold is saved as stackable"
    );
    assert!(
        records
            .iter()
            .any(|r| r.serial == backpack_serial && matches!(r.location, ItemLocation::Equipped { .. })),
        "the backpack is saved as worn"
    );
    let gold_record = records
        .iter()
        .find(|record| record.serial == gold_serial)
        .expect("the gold is in the saved inventory");
    assert_eq!(gold_record.amount, 500);
    assert_eq!(
        gold_record.location,
        ItemLocation::Contained {
            container: backpack_serial,
            x:         40,
            y:         65,
            grid:      0,
        },
        "the gold keeps its exact container position"
    );

    // Log out — the character and its items leave the world.
    home.queue(Command::Disconnect { connection: conn_a });
    home.tick(now);

    // A fresh shard: reserve the serials, load the items, play the character.
    let mut shard = world();
    let characters = on_file(
        &mut shard,
        char_serial,
        Point::new(1500, 1000, 0),
        Appearance::default_human(),
    );
    shard.restore_items(records, &characters);
    let conn_b = connection();
    shard.queue(Command::Enter(Entering {
        connection: conn_b,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        character:  Character::Saved,
    }));
    shard.tick(now);

    // Exactly one backpack (the restored one, not a fresh starter too), with the
    // gold back inside it.
    let backpacks = shard
        .registry()
        .query::<Equipped>()
        .filter(|(_, worn)| worn.mobile == char_serial && worn.layer == items::BACKPACK_LAYER)
        .count();
    assert_eq!(backpacks, 1, "the saved backpack came back, no starter added");
    let gold = shard
        .registry()
        .entity_of(gold_serial)
        .expect("the gold is back on its serial");
    assert_eq!(shard.registry().get::<Amount>(gold).unwrap().0, 500);
    assert!(
        shard.registry().has::<Stackable>(gold),
        "the gold came back stackable, so it still merges with more"
    );
    assert_eq!(
        *shard.registry().get::<Contained>(gold).unwrap(),
        Contained {
            container: backpack_serial,
            position:  GumpPoint::new(40, 65),
            grid:      GridSlot(0),
        },
        "and back at the same point inside the same backpack"
    );
}

#[test]
fn a_spellbook_keeps_its_spells_across_a_logout_and_restore() {
    use openshard_protocol::serial::SerialKind;
    use openshard_state::components::{
        SPELLBOOK_GRAPHIC,
        Spellbook,
    };

    // A bought spellbook with spells learned into it must open again after a
    // relog: without the mask on disk it comes back as a graphic with no
    // Spellbook component, and double-click falls through — the exact bug a
    // player hit buying a book, scribing scrolls, and logging back in.
    let mut home = world();
    let now = Instant::now();
    let conn_a = enter(&mut home, now);
    let entity = home.state.players[&conn_a];
    let char_serial = home.registry().serial_of(entity).unwrap();

    let (backpack, _) = home
        .registry()
        .query::<Equipped>()
        .find(|(_, worn)| worn.layer == items::BACKPACK_LAYER)
        .expect("a backpack was equipped");
    let backpack_serial = home.registry().serial_of(backpack).unwrap();

    // Spells 0, 5 and 63 learned — 63 is the top bit, which only survives the
    // store's i64 bit-cast if it is treated as an unsigned mask.
    let learned = (1u64 << 0) | (1u64 << 5) | (1u64 << 63);
    let (book, book_serial) = home.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    home.state.registry.insert(
        book,
        Drawn {
            id:  SPELLBOOK_GRAPHIC,
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    home.state.registry.insert(book, Spellbook(learned));
    openshard_state::establish_item_location(
        &mut home.state,
        book,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack_serial,
            position:  GumpPoint::new(40, 65),
            grid:      GridSlot(0),
        }),
    )
    .unwrap();

    // The sweep carries the mask.
    let records = home.inventory_of(entity);
    assert!(
        records
            .iter()
            .any(|r| r.serial == book_serial && r.spellbook == Some(learned)),
        "the spellbook is saved with its learned spells"
    );

    home.queue(Command::Disconnect { connection: conn_a });
    home.tick(now);

    let mut shard = world();
    let characters = on_file(
        &mut shard,
        char_serial,
        Point::new(1500, 1000, 0),
        Appearance::default_human(),
    );
    shard.restore_items(records, &characters);
    let conn_b = connection();
    shard.queue(Command::Enter(Entering {
        connection: conn_b,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        character:  Character::Saved,
    }));
    shard.tick(now);

    let book = shard
        .registry()
        .entity_of(book_serial)
        .expect("the spellbook is back on its serial");
    assert_eq!(
        shard.registry().get::<Spellbook>(book).map(|b| b.0),
        Some(learned),
        "the restored book still knows the spells it was scribed with"
    );
}

#[test]
fn a_relogin_in_the_same_run_keeps_the_inventory() {
    use openshard_protocol::serial::SerialKind;

    // The bug the user hit: logging out and back in *without a restart* lost the
    // backpack, because the pending-inventory cache was only filled at boot.
    let mut world = world();
    let now = Instant::now();
    let conn = enter(&mut world, now);
    let entity = world.state.players[&conn];
    let char_serial = world.registry().serial_of(entity).unwrap();
    let (backpack, _) = world
        .registry()
        .query::<Equipped>()
        .find(|(_, w)| w.layer == items::BACKPACK_LAYER)
        .unwrap();
    let backpack_serial = world.registry().serial_of(backpack).unwrap();
    // Put the gold two levels down. Logout used to despawn only direct backpack
    // contents, leaving this gold alive; restore then found the stale serial and
    // panicked while trying to establish its already-established location.
    let (bag, bag_serial) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        bag,
        Drawn {
            id:  openshard_protocol::wire::Graphic(0x0E76),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    world.state.registry.insert(
        bag,
        Container {
            gump: openshard_protocol::wire::Graphic(0x003C),
        },
    );
    openshard_state::establish_item_location(
        &mut world.state,
        bag,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack_serial,
            position:  GumpPoint::new(0, 0),
            grid:      GridSlot(0),
        }),
    )
    .unwrap();
    let (gold, gold_serial) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        gold,
        Drawn {
            id:  openshard_protocol::wire::Graphic(0x0EED),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    world.state.registry.insert(gold, Amount(300));
    openshard_state::establish_item_location(
        &mut world.state,
        gold,
        openshard_state::ItemLocation::contained(Contained {
            container: bag_serial,
            position:  GumpPoint::new(0, 0),
            grid:      GridSlot(0),
        }),
    )
    .unwrap();

    // Log out and, in the same world, log the same character back in.
    world.queue(Command::Disconnect { connection: conn });
    world.tick(now);
    let conn = connection();
    world.queue(Command::Enter(Entering {
        connection: conn,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        character:  Character::Saved,
    }));
    world.tick(now);

    // On its own serial: the world found the roster row its logout wrote, which
    // is also what makes the inventory below findable — it is filed under this
    // number.
    assert_eq!(
        world.registry().serial_of(world.state.players[&conn]).unwrap(),
        char_serial,
        "the same character came back, not a fresh one on a new serial"
    );
    let gold = world
        .registry()
        .entity_of(gold_serial)
        .expect("the gold came back on relog");
    assert_eq!(world.registry().get::<Amount>(gold).unwrap().0, 300);
    assert_eq!(
        world.registry().get::<Contained>(gold).unwrap().container,
        bag_serial
    );
}

#[test]
fn a_spawner_respawn_timer_survives_a_restart() {
    use crate::spawner::{
        SpawnArea,
        Spawner,
    };

    // The user's case: a rare spawn on a long timer, killed with time still to
    // wait, must come back with that wait ahead of it — not pop again the moment
    // the shard restarts.
    let mut home = world();
    let area = SpawnArea {
        x:      START.x,
        y:      START.y,
        width:  1,
        height: 1,
        facet:  Facet(0),
    };
    // A 100-second respawn region.
    home.register_spawner(Spawner::new(
        openshard_state::SpawnerId::PLACEHOLDER,
        area,
        vec![],
        1,
        100 * TICKS_PER_SECOND,
    ));
    // Pretend it spawned a while ago and has 60 seconds left to wait.
    home.state.ticks = openshard_state::WorldTick::from_raw(5_000);
    home.spawners[0].next_spawn = home.state.ticks + 60 * TICKS_PER_SECOND;

    // What the save carries.
    let records = home.spawner_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].remaining_secs, 60, "sixty seconds still to wait");
    assert_eq!(records[0].respawn_secs, 100);
    assert_eq!(
        records[0].id,
        openshard_state::SpawnerId::PLACEHOLDER,
        "the first region's id is its slot, which is where its creatures point"
    );

    // Restart: a fresh world, tick counter back at zero, restores the region.
    let mut shard = world();
    shard.restore_spawners(records);
    assert_eq!(shard.spawners.len(), 1);
    assert_eq!(
        shard.spawners[0].next_spawn,
        openshard_state::WorldTick::from_raw(60 * TICKS_PER_SECOND),
        "the sixty seconds are still ahead of it, not reset to zero"
    );
    assert_eq!(shard.spawners[0].respawn_delay, 100 * TICKS_PER_SECOND);
}

#[test]
fn re_registering_a_region_keeps_the_first_and_its_timer() {
    use crate::spawner::{
        SpawnArea,
        Spawner,
    };

    let mut world = world();
    let area = SpawnArea {
        x:      100,
        y:      100,
        width:  5,
        height: 5,
        facet:  Facet(0),
    };
    world.register_spawner(Spawner::new(
        openshard_state::SpawnerId::PLACEHOLDER,
        area,
        vec![],
        3,
        40,
    ));
    // Give the standing region a timer with time still to wait, as a restore from
    // the database would.
    world.spawners[0].next_spawn = openshard_state::WorldTick::from_raw(5_000);
    // A second registration over the same box — a boot re-populate, or a second
    // staff click — must not stack a spawner nor reset the waiting one.
    world.register_spawner(Spawner::new(
        openshard_state::SpawnerId::PLACEHOLDER,
        area,
        vec![],
        3,
        40,
    ));
    assert_eq!(
        world.spawners.len(),
        1,
        "the same region registered twice is one spawner, not two"
    );
    assert_eq!(
        world.spawners[0].next_spawn,
        openshard_state::WorldTick::from_raw(5_000),
        "and the restored timer is left alone, not reset by the re-populate"
    );
}

/// Two regions may share one box, and both must be laid.
///
/// Britannia's converted spawn data has 74 boxes carrying two regions each — an
/// orc camp and a patch of undead over the same 60×60 north-east of Britain is the
/// one a player notices. De-duplicating on the box alone read the second as the
/// first laid twice and dropped it, taking the skeletons out of the forest with no
/// error anywhere. What a re-populate must not stack is the *same* region.
#[test]
fn two_different_regions_over_one_box_are_both_laid() {
    use crate::spawner::{
        CreatureTemplate,
        SpawnArea,
        Spawner,
    };

    let mut world = world();
    let area = SpawnArea {
        x:      100,
        y:      100,
        width:  60,
        height: 60,
        facet:  Facet(0),
    };
    let creature = |body: u16| {
        CreatureTemplate {
            fame:        0,
            karma:       0,
            body:        openshard_protocol::wire::Graphic(body),
            hue:         openshard_protocol::wire::Hue(0),
            hits:        10,
            notoriety:   openshard_protocol::mobile::Notoriety::Murderer,
            damage:      1,
            resistance:  openshard_protocol::world::PhysicalResistance::new(0),
            swing:       0,
            sight:       Sight(0),
            aggression:  Aggression::from_bits(2),
            beat:        0,
            ranged:      None,
            ranged_kind: DamageType::Physical,
            wander:      false,
            skills:      Vec::new(),
        }
    };
    // An orc camp, and the undead patch that overlaps it.
    let orcs = Spawner::new(
        openshard_state::SpawnerId::PLACEHOLDER,
        area,
        vec![creature(0x0011)],
        5,
        40,
    );
    let undead = Spawner::new(
        openshard_state::SpawnerId::PLACEHOLDER,
        area,
        vec![creature(0x0032)],
        7,
        40,
    );
    world.register_spawner(orcs.clone());
    world.register_spawner(undead.clone());
    assert_eq!(
        world.spawners.len(),
        2,
        "a second region over the same box is a region, not a re-registration"
    );
    // And a re-populate still stacks neither of them.
    world.register_spawner(orcs);
    world.register_spawner(undead);
    assert_eq!(
        world.spawners.len(),
        2,
        "re-laying the same two regions stacks nothing"
    );
}

/// A region's id is its slot, through every path that builds the list.
///
/// This is the invariant `SpawnedBy` rides on. The tag is written into a creature,
/// saved with it, and read back against a list some later boot rebuilt — so if a
/// region's id can ever be anything but its position, a restart re-points live
/// creatures at their neighbours: a region silently over its ceiling and one
/// silently at it, never spawning again. It used to be a counter that started at
/// one and was only ever bumped, which agreed with the index by luck.
#[test]
fn a_regions_id_is_its_slot_however_the_list_was_built() {
    use crate::spawner::{
        SpawnArea,
        Spawner,
    };

    let area = |x: u16| {
        SpawnArea {
            x,
            y: 100,
            width: 5,
            height: 5,
            facet: Facet(0),
        }
    };
    let slots_hold = |world: &World, what: &str| {
        for (slot, spawner) in world.spawners.iter().enumerate() {
            assert_eq!(spawner.id.index(), slot, "{what}: a region's id left its slot");
        }
    };

    let mut laid = world();
    for x in 0..5u16 {
        laid.register_spawner(Spawner::new(
            openshard_state::SpawnerId::PLACEHOLDER,
            area(x * 10),
            vec![],
            3,
            40,
        ));
    }
    slots_hold(&laid, "freshly registered");

    // A restart: the records carry the ids, and the rebuilt list must land on the
    // same numbers rather than trusting them.
    let records = laid.spawner_records();
    let mut restarted = world();
    restarted.restore_spawners(records);
    slots_hold(&restarted, "restored from the save");
    // And a boot re-populate over the restored list neither stacks nor renumbers.
    for x in 0..5u16 {
        restarted.register_spawner(Spawner::new(
            openshard_state::SpawnerId::PLACEHOLDER,
            area(x * 10),
            vec![],
            3,
            40,
        ));
    }
    assert_eq!(restarted.spawners.len(), 5, "the re-populate stacked a region");
    slots_hold(&restarted, "after a boot re-populate");

    // A staff Clear takes every region and every creature that pointed at one, so
    // the numbering may start again from zero without stranding a tag.
    restarted.clear_spawners();
    assert!(restarted.spawners.is_empty());
    for x in 0..3u16 {
        restarted.register_spawner(Spawner::new(
            openshard_state::SpawnerId::PLACEHOLDER,
            area(x * 10),
            vec![],
            3,
            40,
        ));
    }
    slots_hold(&restarted, "after Clear and Populate");
    assert_eq!(
        restarted.spawners[0].id,
        openshard_state::SpawnerId::PLACEHOLDER,
        "the ids start again at zero rather than carrying a counter past the clear"
    );
}

/// Every region the tree ships reaches the world.
///
/// The engine gets its regions in one flat stream and answers each with a yes or a
/// silent no, so a de-duplication rule that is too coarse costs content and says
/// nothing. This pins the count: the only regions the shipped data loses are the
/// ones that are byte-for-byte the same region written twice.
#[test]
fn every_region_the_tree_ships_reaches_the_world() {
    let shipped: Vec<crate::spawner::Spawner> = crate::spawner::shipped()
        .into_iter()
        .flat_map(|set| set.spawners)
        .collect();
    assert!(!shipped.is_empty(), "the tree ships no spawn regions at all");

    // What the data itself says is distinct, counted the way the world will.
    let mut distinct: Vec<&crate::spawner::Spawner> = Vec::new();
    for spawner in &shipped {
        if !distinct.iter().any(|kept| kept.is_the_same_region(spawner)) {
            distinct.push(spawner);
        }
    }

    let mut world = world();
    for spawner in shipped.iter().cloned() {
        world.register_spawner(spawner);
    }
    assert_eq!(
        world.spawners.len(),
        distinct.len(),
        "the world dropped regions the data means to be different"
    );
}

#[test]
fn a_vendor_and_its_priced_stock_survive_a_restart() {
    use openshard_state::components::{
        Price,
        Vendor,
    };

    // The whole-world save: a staff Populate seeds the vendor once, and from
    // then on the *save* is the truth — a restart brings the shopkeeper back
    // with its crate, wares, prices and labels, with no re-populate anywhere.
    let now = Instant::now();
    let mut home = world();
    let _gm = enter_gm(&mut home, now);
    home.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(7),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x + 1, START.y, 0),
        facet:       Facet(0),
        name:        Some("Mirabel".to_owned()),
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      true,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    home.tick(now);
    let vendor = home
        .state
        .registry
        .query::<Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a shopkeeper");
    let vendor_serial = home.registry().serial_of(vendor).unwrap();
    home.queue(Command::StockVendor {
        serial: vendor_serial,
        stock:  vec![npc::StockLine {
            graphic:   openshard_protocol::wire::Graphic(0),
            hue:       openshard_protocol::wire::Hue(0),
            item_kind: Some(openshard_protocol::item_kind::ItemKindId(1)),
            material:  Some(openshard_protocol::item_kind::MaterialId(9)),
            amount:    openshard_state::components::Amount(50),
            price:     openshard_state::components::Price(4),
            name:      "valorite ingot".to_owned(),
        }],
    });
    home.tick(now);

    home.take_snapshot();
    let snapshot = home.drain_saves().next_back().expect("a snapshot");
    let mobiles = snapshot.mobiles.clone().expect("a mobile sweep");
    assert!(
        mobiles.iter().any(|m| m.serial == vendor_serial && m.vendor),
        "the vendor is in the mobile sweep, marked as one"
    );
    assert!(
        mobiles.iter().any(|mobile| {
            mobile.serial == vendor_serial
                && mobile.restock.as_ref().is_some_and(|restock| {
                    restock.typed_lines.iter().any(|line| {
                        line.item_kind == Some(1) && line.material == Some(9) && line.amount == 50
                    })
                })
        }),
        "the typed restock identity is saved independently of art"
    );
    // What the store would hand back at boot: every saved item, inventories
    // and ground alike.
    let mut items: Vec<ItemRecord> = snapshot
        .inventories
        .iter()
        .flat_map(|inventory| inventory.items.clone())
        .collect();
    items.extend(snapshot.ground.unwrap_or_default());

    // The restart: a fresh world restored from the records alone. No characters
    // are on file — the owners here are a vendor and its stock crate — but the
    // restore still runs in its order, and says so by handing the token over.
    let mut shard = world();
    let characters = shard.restore_characters(Vec::new());
    let filed = shard.restore_items(items, &characters);
    shard.restore_mobiles(mobiles, &filed);

    let vendor = shard
        .registry()
        .entity_of(vendor_serial)
        .expect("the vendor came back on its serial");
    assert!(
        shard.registry().has::<Vendor>(vendor),
        "and is still a shopkeeper"
    );
    assert_eq!(
        shard.registry().get::<Name>(vendor).unwrap().0,
        "Mirabel",
        "with its name"
    );
    let (stock_item, price) = shard
        .state
        .registry
        .query::<Price>()
        .next()
        .expect("its priced stock came back");
    assert_eq!(price.0, 4, "at the price it was stocked at");
    assert_eq!(
        shard.registry().get::<Name>(stock_item).unwrap().0,
        "valorite ingot",
        "under its label"
    );
    assert_eq!(
        shard.registry().get::<Amount>(stock_item).unwrap().0,
        50,
        "at its full amount"
    );
    assert_eq!(
        shard
            .registry()
            .get::<openshard_state::components::ItemKind>(stock_item),
        Some(&openshard_state::components::ItemKind(
            openshard_protocol::item_kind::ItemKindId(1),
        )),
        "the typed stock kind persisted"
    );
    assert_eq!(
        shard
            .registry()
            .get::<openshard_state::components::Material>(stock_item),
        Some(&openshard_state::components::Material(
            openshard_protocol::item_kind::MaterialId(9),
        )),
        "and its material persisted"
    );
    // And the stock sits in a crate the vendor actually wears.
    let held_in = shard
        .registry()
        .get::<Contained>(stock_item)
        .expect("stock lives in a container")
        .container;
    let crate_entity = shard.registry().entity_of(held_in).expect("the crate");
    let worn = shard
        .registry()
        .get::<Equipped>(crate_entity)
        .expect("the crate is worn");
    assert_eq!(worn.mobile, vendor_serial);
    assert_eq!(worn.layer, npc::STOCK_LAYER);
    assert!(
        shard
            .registry()
            .get::<openshard_state::components::Restock>(vendor)
            .is_some_and(|restock| {
                restock.lines.iter().any(|line| {
                    line.item_kind == Some(openshard_protocol::item_kind::ItemKindId(1))
                        && line.material == Some(openshard_protocol::item_kind::MaterialId(9))
                        && line.amount.0 == 50
                })
            }),
        "restock restores its direct kind/material without inferring them from its saved art"
    );
}

#[test]
fn a_wounded_spawner_creature_survives_a_restart_and_is_counted() {
    use crate::spawner::{
        CreatureTemplate,
        SpawnArea,
        Spawner,
    };

    // ServUO's model exactly: a live creature is saved as it stands — wounded
    // stays wounded — and its region re-counts it on load, so a restart neither
    // heals it, loses it, nor spawns a double over it.
    let now = Instant::now();
    let mut home = world();
    let creature = CreatureTemplate {
        fame:        0,
        karma:       0,
        body:        openshard_protocol::wire::Graphic(0x0009),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        10,
        notoriety:   openshard_protocol::mobile::Notoriety::Neutral,
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        skills:      Vec::new(),
    };
    let area = SpawnArea {
        x:      START.x,
        y:      START.y,
        width:  2,
        height: 2,
        facet:  Facet(0),
    };
    home.queue(Command::RegisterSpawner {
        spawner: Spawner::new(
            openshard_state::SpawnerId::PLACEHOLDER,
            area,
            vec![creature],
            1,
            1000,
        ),
    });
    home.tick(now); // applies the register (its first spawn is jittered out)
    home.spawners[0].next_spawn = openshard_state::WorldTick::ZERO; // this test wants the creature now, not the jitter
    for _ in 0..3 {
        home.tick(now);
    }
    let (spawned, _) = home
        .state
        .registry
        .query::<SpawnedBy>()
        .next()
        .expect("the region filled");
    let spawned_serial = home.registry().serial_of(spawned).unwrap();
    // Wound it, as a fight would.
    home.state.registry.insert(
        spawned,
        Hitpoints {
            current: 3,
            max:     10,
        },
    );

    home.take_snapshot();
    let snapshot = home.drain_saves().next_back().expect("a snapshot");
    let mobiles = snapshot.mobiles.clone().expect("a mobile sweep");
    let spawners = snapshot.spawners.expect("a spawner sweep");

    let mut shard = world();
    shard.restore_spawners(spawners);
    let filed = nothing_restored_first(&mut shard);
    shard.restore_mobiles(mobiles, &filed);

    let creature = shard
        .registry()
        .entity_of(spawned_serial)
        .expect("the creature came back on its serial");
    assert_eq!(
        shard.registry().get::<Hitpoints>(creature).unwrap().current,
        3,
        "still wounded, not respawned fresh"
    );
    assert!(
        shard.registry().has::<SpawnedBy>(creature),
        "and still tied to its region"
    );
    // Many ticks later the region holds its ceiling of one: the restored
    // creature is counted, not spawned over.
    let mut later = now;
    for _ in 0..8 {
        later += TICK_INTERVAL;
        shard.tick(later);
    }
    assert_eq!(
        shard.registry().query::<SpawnedBy>().count(),
        1,
        "the region counts the restored creature and does not over-spawn"
    );
}

#[test]
fn decoration_and_door_state_survive_a_restart() {
    use openshard_state::components::Decoration;

    use crate::tick::command::DecorDoor;

    // Decoration is saved like everything else — and a door left open stays
    // open across the restart, its doorway unblocked until it swings shut.
    let now = Instant::now();
    let mut home = world();
    let _gm = enter_gm(&mut home, now);
    let shut_at = Point::new(START.x + 2, START.y, 0);
    let open_at = Point::new(START.x + 4, START.y, 0);
    home.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    vec![(
            openshard_protocol::wire::Graphic(0x07C1),
            openshard_protocol::wire::Hue(0),
            Point::new(START.x + 6, START.y, 0),
        )],
        doors:      vec![
            DecorDoor {
                lock:     None,
                closed:   openshard_protocol::wire::Graphic(0x0675),
                open:     openshard_protocol::wire::Graphic(0x0676),
                offset_x: -1,
                offset_y: 1,
                position: shut_at,
            },
            DecorDoor {
                lock:     None,
                closed:   openshard_protocol::wire::Graphic(0x0675),
                open:     openshard_protocol::wire::Graphic(0x0676),
                offset_x: -1,
                offset_y: 1,
                position: open_at,
            },
        ],
        containers: Vec::new(),
    });
    home.tick(now);
    // Swing the second door open.
    let door_to_open = home
        .state
        .registry
        .query::<Door>()
        .find(|(entity, _)| {
            home.registry()
                .get::<Position>(*entity)
                .is_some_and(|p| p.0.x == open_at.x)
        })
        .map(|(entity, _)| entity)
        .expect("the second door");
    openshard_items::open_door(&mut home.state, door_to_open);
    home.tick(now);

    home.take_snapshot();
    let snapshot = home.drain_saves().next_back().expect("a snapshot");
    let decorations = snapshot.decorations.expect("a decoration sweep");
    assert_eq!(decorations.len(), 3, "one static, two doors");

    let mut shard = world();
    shard.restore_decorations(decorations);
    assert_eq!(
        shard.registry().query::<Decoration>().count(),
        3,
        "everything re-laid"
    );
    let restored_open = shard
        .state
        .registry
        .query::<Door>()
        .find(|(_, door)| door.is_open)
        .expect("the open door is still open");
    assert_eq!(restored_open.1.open, openshard_protocol::wire::Graphic(0x0676));
    // The shut door seals its doorway; the open one blocks nobody.
    assert!(
        shard
            .state
            .facet_state(Facet(0))
            .obstructions()
            .blocker_at(shut_at.x, shut_at.y)
            .is_some(),
        "the shut door blocks its tile again"
    );
    let open_pos = shard.registry().get::<Position>(restored_open.0).unwrap().0;
    assert!(
        shard
            .state
            .facet_state(Facet(0))
            .obstructions()
            .blocker_at(open_pos.x, open_pos.y)
            .is_none(),
        "the open door does not"
    );
}

#[test]
fn a_snapshot_saves_an_idle_online_character_and_the_ground() {
    use openshard_protocol::serial::SerialKind;

    // A save must capture an online character's inventory and loose ground items
    // even when nobody moved — an item picked up without a step, gold dropped and
    // left. The old save only ran when the journal was dirty and only walked
    // dirty characters, which is how backpacks and dropped gold went missing.
    let mut world = world();
    let now = Instant::now();
    let conn = enter(&mut world, now);
    let entity = world.state.players[&conn];
    let (backpack, _) = world
        .registry()
        .query::<Equipped>()
        .find(|(_, w)| w.layer == items::BACKPACK_LAYER)
        .unwrap();
    let backpack_serial = world.registry().serial_of(backpack).unwrap();
    // A backpack item and a loose ground item.
    let (bagged, _) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        bagged,
        Drawn {
            id:  openshard_protocol::wire::Graphic(0x0EED),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    openshard_state::establish_item_location(
        &mut world.state,
        bagged,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack_serial,
            position:  GumpPoint::new(0, 0),
            grid:      GridSlot(0),
        }),
    )
    .unwrap();
    items::spawn_item(
        &mut world.state,
        openshard_protocol::wire::Graphic(0x1BFB),
        openshard_protocol::wire::Hue(0),
        1,
        false,
        Point::new(1365, 1600, 0),
        Facet(0),
    );

    // Tick once to settle, draining any snapshots the enter produced, then force
    // a fresh snapshot with no movement in between.
    world.tick(now);
    let _ = world.drain_saves().count();
    world.take_snapshot();

    let snapshot = world.drain_saves().next().expect("a snapshot was taken");
    let owner = world.registry().serial_of(entity).unwrap();
    assert!(
        snapshot.characters.iter().any(|c| c.serial == owner),
        "the idle online character was saved"
    );
    let inv = snapshot
        .inventories
        .iter()
        .find(|inv| inv.owner == owner)
        .expect("its inventory was walked");
    assert!(
        inv.items.iter().any(|i| i.graphic == 0x0EED),
        "the backpack gold is in the saved inventory"
    );
    let ground = snapshot.ground.as_ref().expect("the ground was swept");
    assert!(
        ground.iter().any(|i| i.graphic == 0x1BFB),
        "the loose ground item was saved"
    );
}

fn spawn_banker(world: &mut World, at: Point, now: Instant) {
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        100,
        notoriety:   Notoriety::from_bits(7), // invulnerable
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    at,
        facet:       Facet(0),
        name:        Some("the banker".to_owned()),
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      true,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
}

fn say(world: &mut World, connection: ConnectionId, text: &str, now: Instant) {
    world.queue(Command::Say {
        connection,
        mode: RawTalkMode(0),
        hue: RawHue(0),
        font: RawFont(3),
        text: text.to_owned(),
    });
    world.tick(now);
}

#[test]
fn entering_the_world_equips_a_bank_box() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let owner = world
        .registry()
        .serial_of(world.state.players[&connection])
        .unwrap();
    assert!(
        world.registry().query::<Equipped>().any(|(item, worn)| {
            worn.mobile == owner && worn.layer == npc::BANK_LAYER && world.registry().has::<Container>(item)
        }),
        "a character wears a bank box on the bank layer"
    );
}

#[test]
fn saying_bank_near_a_banker_opens_the_bank_box() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    spawn_banker(&mut world, Point::new(START.x + 1, START.y, 0), now);
    let _ = packets_for(&mut world, connection);

    say(&mut world, connection, "bank", now);
    assert!(
        packets_for(&mut world, connection).iter().any(|p| p[0] == 0x24),
        "the bank box gump opened"
    );
}

#[test]
fn a_banker_greets_a_nearby_player() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    // The banker two tiles off — inside the greet range. Its first beat is jittered
    // across one beat's span (so a whole facet's townsfolk do not beat in lockstep),
    // so give it that long to come due. The line is one of several, but every one
    // names the visitor.
    spawn_banker(&mut world, Point::new(START.x + 2, START.y, 0), now);
    world.drain_outbound().count();
    // One beat and its whole jitter span, off the constants themselves rather
    // than a tick count that only held at one tick rate.
    let beat = openshard_npc::BEAT_TICKS;
    for _ in 0..(beat + beat / openshard_npc::BEAT_JITTER_FRACTION) {
        world.tick(now);
    }
    // Speech is Unicode `0xAE` now, so the name is UTF-16; strip the zero bytes
    // and the ASCII characters read straight through.
    let greeted = packets_for(&mut world, connection).iter().any(|p| {
        p[0] == 0xAE && {
            let text: Vec<u8> = p.iter().copied().filter(|&b| b != 0).collect();
            String::from_utf8_lossy(&text).contains("Lord British")
        }
    });
    assert!(greeted, "the banker greeted the nearby player by name");
}

/// Spawn a townsperson of a trade, dressed and named by the core.
pub(super) fn spawn_townsperson(world: &mut World, trade: &str, at: Point, now: Instant) -> EntityId {
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        100,
        notoriety:   Notoriety::from_bits(7),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    at,
        facet:       Facet(0),
        // No name and no equipment: the core dresses it and names it, which is the
        // path the pack takes.
        name:        None,
        title:       Some(trade.to_owned()),
        shoe:        1,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    world
        .registry()
        .query::<openshard_state::components::Title>()
        .filter(|(_, title)| title.0 == trade)
        .map(|(entity, _)| entity)
        .next_back()
        .expect("a townsperson of that trade")
}

#[test]
fn a_townsperson_is_dressed_and_named_by_the_core() {
    // What the pack sends is a trade and a tile. Everything a client draws — a
    // gender, a skin, hair, clothes, a personal name — is the core's roll, because
    // the pack shipped one robe and one haircut for all 738 of Felucca's townsfolk.
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    let smith = spawn_townsperson(
        &mut world,
        "the blacksmith",
        Point::new(START.x + 4, START.y, 0),
        now,
    );

    let name = world
        .registry()
        .get::<Name>(smith)
        .expect("a townsperson is named")
        .0
        .clone();
    assert!(name.ends_with(" the blacksmith"), "{name}");
    assert_ne!(name, "the blacksmith", "a person, not just a trade");

    let body = world.registry().get::<Body>(smith).expect("a body");
    assert!(
        body.id.0 == 0x0190 || body.id.0 == 0x0191,
        "a human body, either gender: {:#06x}",
        body.id.0
    );
    assert_ne!(body.hue.0, 0, "a rolled skin hue, not the flat default");

    let worn: Vec<Layer> = world
        .registry()
        .query::<Equipped>()
        .filter(|(_, w)| Some(w.mobile) == world.registry().serial_of(smith))
        .map(|(_, w)| w.layer)
        .collect();
    // The regression that catches "everyone is back in the one generic robe":
    // hair, a torso, legs and shoes, on four distinct layers.
    assert!(worn.contains(&Layer(0x0B)), "hair: {worn:?}");
    assert!(worn.contains(&Layer(0x03)), "shoes: {worn:?}");
    assert!(
        worn.contains(&Layer(0x05)) || worn.contains(&Layer(0x11)),
        "a torso: {worn:?}"
    );
    assert!(
        worn.contains(&Layer(0x04)) || worn.contains(&Layer(0x17)),
        "legs: {worn:?}"
    );
}

/// Place a door at `at`, locked to `key`, and return it.
fn place_lockable_door(
    world: &mut World,
    at: Point,
    key: openshard_state::KeyValue,
    now: Instant,
) -> EntityId {
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    Vec::new(),
        doors:      vec![DecorDoor {
            lock:     Some(openshard_state::LockKind::Key(key)),
            closed:   openshard_protocol::wire::Graphic(0x0675),
            open:     openshard_protocol::wire::Graphic(0x0676),
            offset_x: 0,
            offset_y: 0,
            position: at,
        }],
        containers: Vec::new(),
    });
    world.tick(now);
    world
        .registry()
        .query::<openshard_state::components::Door>()
        .map(|(entity, _)| entity)
        .next_back()
        .expect("a door")
}

#[test]
fn a_kill_pays_the_killer_in_fame_and_karma() {
    // ServUO's `BaseCreature.OnDeath`: the killer takes the victim's fame and the
    // *negation* of its karma. The sign is the whole rule — a creature carries negative
    // karma when it is evil, so slaying it earns karma and slaying something innocent
    // costs, and nothing has to know what a murder is for that to work.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let killer = world.state.players[&connection];

    let evil = spawn_creature_with_standing(&mut world, 2000, -3000, now);
    let evil_serial = world.registry().serial_of(evil).unwrap();
    let killer_serial = world.registry().serial_of(killer).unwrap();
    openshard_combat::damage(
        &mut world.state,
        evil_serial,
        10_000,
        openshard_state::DamageType::Physical,
        Some(killer_serial),
    );
    world.tick(now);
    let fame = world
        .registry()
        .get::<openshard_state::components::Fame>(killer)
        .map_or(0, |f| f.0);
    let karma = world
        .registry()
        .get::<openshard_state::components::Karma>(killer)
        .map_or(0, |k| k.0);
    assert_eq!(fame, 2000, "the victim's fame, undiminished on a first kill");
    assert_eq!(karma, 3000, "and the negation of its karma: killing evil is good");

    // An innocent costs. The curve bites now that the killer has standing.
    let innocent = spawn_creature_with_standing(&mut world, 100, 2000, now);
    let innocent_serial = world.registry().serial_of(innocent).unwrap();
    openshard_combat::damage(
        &mut world.state,
        innocent_serial,
        10_000,
        openshard_state::DamageType::Physical,
        Some(killer_serial),
    );
    world.tick(now);
    let after = world
        .registry()
        .get::<openshard_state::components::Karma>(killer)
        .map_or(0, |k| k.0);
    assert!(
        after < karma,
        "killing an innocent costs karma: {karma} -> {after}"
    );
}

/// Spawn a creature carrying standing to give up.
fn spawn_creature_with_standing(world: &mut World, fame: i32, karma: i32, now: Instant) -> EntityId {
    world.queue(Command::SpawnMobile {
        body: openshard_protocol::wire::Graphic(0x00D0),
        hue: openshard_protocol::wire::Hue(0),
        hits: 10,
        notoriety: openshard_protocol::mobile::Notoriety::Neutral,
        damage: 0,
        resistance: openshard_protocol::world::PhysicalResistance::new(0),
        swing: 0,
        sight: Sight(0),
        aggression: Aggression::from_bits(0),
        beat: 0,
        ranged: None,
        ranged_kind: DamageType::Physical,
        wander: false,
        position: Point::new(START.x + 1, START.y + 1, 0),
        facet: Facet(0),
        name: None,
        title: None,
        shoe: 0,
        fame,
        karma,
        night_home: None,
        banker: false,
        vendor: false,
        healer: false,
        equipment: Vec::new(),
        skills: Vec::new(),
        stock: Vec::new(),
        escort_to: None,
        quests: Vec::new(),
    });
    world.tick(now);
    world
        .registry()
        .query::<openshard_state::components::Fame>()
        .filter(|(entity, _)| !world.registry().has::<Client>(*entity))
        .map(|(entity, _)| entity)
        .next_back()
        .expect("a creature with standing")
}

#[test]
fn a_murderer_stays_red_across_a_restart() {
    // The count that makes a repeat killer permanently red lived only in memory, so
    // every restart washed every murderer blue — while the decay clock and the
    // notoriety rule built on top of it were both already correct.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    world
        .state
        .registry
        .insert(player, openshard_state::components::Murders(5));
    world
        .state
        .registry
        .insert(player, openshard_state::components::Fame(7000));
    world
        .state
        .registry
        .insert(player, openshard_state::components::Karma(-9000));

    let now_ticks = world.state.ticks;
    let record = World::record_of(world.registry(), player, now_ticks).expect("a character");
    assert_eq!(record.murders, 5, "the count is swept into the save");
    assert_eq!(record.fame, 7000);
    assert_eq!(record.karma, -9000);

    // And a fresh login from that record comes back red and infamous.
    let mut booted = World::new(START);
    booted.queue(Command::Enter(Entering {
        connection: ConnectionId::from_raw(9),
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       record.name.clone(),
        access:     AccessLevel::Player,
        character:  Character::Fresh(FreshCharacter {
            facet:      Facet(0),
            start:      None,
            appearance: None,
            sheet:      Some(Box::new(CharacterSheet {
                strength:        record.strength,
                dexterity:       record.dexterity,
                intelligence:    record.intelligence,
                skills:          Vec::new(),
                effects:         Vec::new(),
                stat_locks:      Default::default(),
                dead:            false,
                fame:            record.fame,
                karma:           record.karma,
                murders:         record.murders,
                quests:          Vec::new(),
                done_quests:     Vec::new(),
                guild:           None,
                guild_candidate: None,
            })),
        }),
    }));
    booted.tick(now);
    let back = booted.state.players[&ConnectionId::from_raw(9)];
    assert_eq!(
        booted
            .registry()
            .get::<openshard_state::components::Murders>(back)
            .map(|m| m.0),
        Some(5),
        "still a murderer"
    );
    // And famous enough to have earned a title, which is the visible half.
    let titled = openshard_state::titled_name(&booted.state, back, "Rowena");
    assert_ne!(titled, "Rowena", "a famous infamous character has a title");
    assert!(titled.contains("Rowena"));
}

#[test]
fn a_locked_door_refuses_a_player_and_an_npc_alike() {
    // ServUO's `BaseDoor.OnDoubleClick`: locked and shut, say cliloc 502503 and stop.
    // And hands are not a key — without the second half a townsperson walking home
    // strolls straight through a locked shopfront and the lock is decoration.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(player_at) = *world.registry().get::<Position>(player).unwrap();
    let door = place_lockable_door(
        &mut world,
        Point::new(player_at.x + 1, player_at.y, player_at.z),
        openshard_state::KeyValue::new(0xBEEF).expect("non-zero test key"),
        now,
    );
    let door_serial = world.registry().serial_of(door).unwrap();
    world.drain_outbound().count();

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(door_serial.raw())),
    });
    world.tick(now);
    assert!(
        !world
            .registry()
            .get::<openshard_state::components::Door>(door)
            .unwrap()
            .is_open,
        "a locked door stays shut"
    );
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x1C || p[0] == 0xAE),
        "and the player is told so"
    );

    // The AI's decree does not open it either.
    openshard_items::open_door(&mut world.state, door);
    assert!(
        !world
            .registry()
            .get::<openshard_state::components::Door>(door)
            .unwrap()
            .is_open,
        "hands are not a key"
    );
}

#[test]
fn a_key_turns_only_the_lock_it_fits() {
    // ServUO's `Key.OnTarget` matches the *value*, not the item, so a copied key works
    // and a key to another door does not. And a fitting key both unlocks and locks —
    // one key, two directions, which is what ServUO does.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(player_at) = *world.registry().get::<Position>(player).unwrap();
    let door = place_lockable_door(
        &mut world,
        Point::new(player_at.x + 1, player_at.y, player_at.z),
        openshard_state::KeyValue::new(0xBEEF).expect("non-zero test key"),
        now,
    );

    // A key to another lock does nothing.
    let wrong = openshard_items::spawn_item(
        &mut world.state,
        openshard_protocol::wire::Graphic(0x100E),
        openshard_protocol::wire::Hue(0),
        1,
        false,
        player_at,
        Facet(0),
    )
    .expect("a key");
    world.state.registry.insert(
        wrong,
        openshard_state::components::KeyValue::new(0x1234).expect("non-zero test key"),
    );
    assert!(!openshard_items::turn_key(&mut world.state, player, wrong, door));
    assert!(
        world.registry().has::<openshard_state::components::Lock>(door),
        "the wrong key leaves it locked"
    );

    // The right one unlocks it, and turning it again locks it back.
    let right = openshard_items::spawn_item(
        &mut world.state,
        openshard_protocol::wire::Graphic(0x100E),
        openshard_protocol::wire::Hue(0),
        1,
        false,
        player_at,
        Facet(0),
    )
    .expect("a key");
    world.state.registry.insert(
        right,
        openshard_state::components::KeyValue::new(0xBEEF).expect("non-zero test key"),
    );
    assert!(openshard_items::turn_key(&mut world.state, player, right, door));
    assert!(!world.registry().has::<openshard_state::components::Lock>(door));
    assert!(openshard_items::turn_key(&mut world.state, player, right, door));
    assert!(
        world.registry().has::<openshard_state::components::Lock>(door),
        "the same key locks it again"
    );

    // And the door opens now that it can be unlocked.
    openshard_items::turn_key(&mut world.state, player, right, door);
    openshard_items::open_door(&mut world.state, door);
    assert!(
        world
            .registry()
            .get::<openshard_state::components::Door>(door)
            .unwrap()
            .is_open
    );
}

#[test]
fn a_locked_door_comes_back_locked() {
    // A set-piece that unbars itself at every reboot is not a set-piece.
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    let door = place_lockable_door(
        &mut world,
        Point::new(START.x + 5, START.y, 0),
        openshard_state::KeyValue::new(0xBEEF).expect("non-zero test key"),
        now,
    );
    assert!(world.registry().has::<openshard_state::components::Lock>(door));

    let records = world.decoration_records();
    assert!(
        records.iter().any(|r| r.key_value == 0xBEEF),
        "the lock is swept into the save"
    );
    let mut booted = World::new(START);
    booted.restore_decorations(records);
    assert!(
        booted
            .registry()
            .query::<openshard_state::components::Lock>()
            .any(|(_, lock)| {
                lock.kind
                    == openshard_state::LockKind::Key(
                        openshard_state::KeyValue::new(0xBEEF).expect("non-zero test key"),
                    )
            }),
        "and comes back"
    );
}

#[test]
fn a_keyless_lock_is_not_an_unlocked_zero_after_a_restart() {
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    let at = Point::new(START.x + 6, START.y, 0);
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    Vec::new(),
        doors:      vec![DecorDoor {
            lock:     Some(openshard_state::LockKind::Unopenable),
            closed:   openshard_protocol::wire::Graphic(0x0675),
            open:     openshard_protocol::wire::Graphic(0x0676),
            offset_x: 0,
            offset_y: 0,
            position: at,
        }],
        containers: Vec::new(),
    });
    world.tick(now);

    let records = world.decoration_records();
    assert!(
        records
            .iter()
            .any(|record| record.locked && record.key_value == 0),
        "the save distinguishes a keyless lock from an unlocked decoration"
    );
    let mut booted = World::new(START);
    booted.restore_decorations(records);
    assert!(
        booted
            .registry()
            .query::<openshard_state::Lock>()
            .any(|(_, lock)| { lock.kind == openshard_state::LockKind::Unopenable })
    );
}

#[test]
fn a_non_human_townsperson_keeps_its_own_body() {
    // `InitOutfit` dresses a human. Britannia has one non-human town NPC — ServUO's
    // `FrightenedDryad`, a `MondainQuester` with `Body = 266` — and rolling a gender
    // for it would replace the body the pack asked for with a human one and put a
    // shirt and trousers on a dryad.
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(266),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        100,
        notoriety:   Notoriety::from_bits(7),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x + 7, START.y, 0),
        facet:       Facet(0),
        name:        None,
        title:       Some("the frightened dryad".to_owned()),
        shoe:        1,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let dryad = world
        .registry()
        .query::<openshard_state::components::Title>()
        .map(|(entity, _)| entity)
        .next_back()
        .expect("a townsperson");
    assert_eq!(
        world.registry().get::<Body>(dryad).map(|b| b.id.0),
        Some(266),
        "the pack's body stands"
    );
    assert!(
        !world
            .registry()
            .query::<Equipped>()
            .any(|(_, w)| Some(w.mobile) == world.registry().serial_of(dryad)),
        "and it is not wearing a shopkeeper's shirt"
    );
    // It still lives, greets and answers keywords — the trade is what does that, not
    // the outfit.
    assert!(
        world.registry().has::<openshard_state::components::Npc>(dryad),
        "a non-human townsperson still keeps a beat"
    );
}

#[test]
fn a_townspersons_hair_cannot_be_lifted_off_its_head() {
    // Hair is an ordinary worn item on the wire, so without the fixed-layer guard
    // the lift path takes it and a shopkeeper goes bald onto someone's cursor.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let smith = spawn_townsperson(
        &mut world,
        "the blacksmith",
        Point::new(START.x + 1, START.y, 0),
        now,
    );
    let smith_serial = world.registry().serial_of(smith).unwrap();
    let hair = world
        .registry()
        .query::<Equipped>()
        .find(|(_, w)| w.mobile == smith_serial && w.layer == Layer(0x0B))
        .map(|(item, _)| item)
        .expect("a townsperson has hair");
    let hair_serial = world.registry().serial_of(hair).unwrap();
    world.drain_outbound().count();

    world.queue(Command::PickUpItem {
        connection,
        serial: RawSerial(hair_serial.raw()),
        amount: 1,
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, connection).iter().any(|p| p[0] == 0x27),
        "the lift is refused with a drag-cancel"
    );
    assert!(
        world.registry().has::<Equipped>(hair),
        "and the hair stays on its head"
    );
}

#[test]
fn a_wandering_townsperson_changes_tiles_and_not_only_its_heading() {
    // The pirouette regression. The motion path implements turn-as-step: a step in
    // a direction you are not already facing only *turns* you. The old wander rolled
    // a fresh random heading every beat, so seven beats in eight were a spin and the
    // NPC read as frozen. ServUO's `WalkRandom` keeps the current heading most of
    // the time, which is what makes the step land on a new tile.
    let now = Instant::now();
    let mut world = world();
    let start = Point::new(START.x + 6, START.y + 6, 0);
    let wanderer = spawn_townsperson(&mut world, "the peasant", start, now);
    // A wide home range, so heading back to the post is not what moves it.
    let home = *world
        .registry()
        .get::<openshard_state::components::Npc>(wanderer)
        .expect("a townsperson keeps a beat");
    world
        .state
        .registry
        .insert(wanderer, openshard_state::components::Npc { wander: 8, ..home });

    // Fifty beats. Under the old roll that was ~3 translating steps at best; under
    // `WalkRandom` it is a dozen or more, so the two are not close.
    let mut tiles = std::collections::HashSet::new();
    for _ in 0..2000 {
        world.tick(now);
        if let Some(&Position(at)) = world.registry().get::<Position>(wanderer) {
            tiles.insert((at.x, at.y));
        }
    }
    assert!(
        tiles.len() >= 6,
        "a wandering townsperson should get about, saw {} tiles: {tiles:?}",
        tiles.len()
    );
}

#[test]
fn a_shopkeeper_stands_still_while_a_customer_is_at_the_counter() {
    // ServUO's `VendorAI.DoActionInteract` faces the customer and takes no step.
    // Before it, `try_greet` bailed on anything that was not a banker and the vendor
    // fell straight through to the wander — so it walked off mid-transaction.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(player_at) = *world.registry().get::<Position>(player).unwrap();
    let keeper = spawn_townsperson(
        &mut world,
        "the provisioner",
        Point::new(player_at.x + 1, player_at.y, player_at.z),
        now,
    );
    let post = world.registry().get::<Position>(keeper).unwrap().0;

    for _ in 0..400 {
        world.tick(now);
        let at = world.registry().get::<Position>(keeper).unwrap().0;
        assert_eq!(
            (at.x, at.y),
            (post.x, post.y),
            "the shopkeeper left the counter with a customer standing at it"
        );
    }

    // And it turned to face them rather than staring past.
    let facing = world.registry().get::<Heading>(keeper).unwrap().0;
    assert_eq!(facing.direction, openshard_protocol::direction::Direction::West);
}

#[test]
fn a_trade_answers_its_own_keyword_and_only_within_earshot() {
    // The headline path end to end: a table is registered by trade, someone
    // speaks nearby, and the NPC of that trade answers. ServUO's
    // `VendorAI.HandlesOnSpeech` bounds it to four tiles, which is what stops a
    // shopkeeper across the square replying to a private conversation.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(player_at) = *world.registry().get::<Position>(player).unwrap();

    world.queue(Command::RegisterNpcSpeech {
        trades: vec![(
            "the baker".to_owned(),
            openshard_state::SpeechTable {
                greetings: vec!["Fresh bread, {name}.".to_owned()],
                barks:     Vec::new(),
                entries:   vec![openshard_state::SpeechEntry {
                    keywords: vec!["bread".to_owned()],
                    lines:    vec!["My bread is fresh and hot.".to_owned()],
                }],
                fallback:  None,
            },
        )],
    });
    world.tick(now);

    // Well out of earshot: five tiles, one past `HandlesOnSpeech`.
    let far = spawn_townsperson(
        &mut world,
        "the baker",
        Point::new(player_at.x + 5, player_at.y, player_at.z),
        now,
    );
    world.drain_outbound().count();
    say(&mut world, connection, "I should like some bread", now);
    let far_serial = world.registry().serial_of(far).unwrap();
    assert!(
        !packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0xAE && mentions(p, far_serial)),
        "a baker five tiles off must not answer"
    );

    // Two tiles: in earshot, and it answers over its own head.
    let near = spawn_townsperson(
        &mut world,
        "the baker",
        Point::new(player_at.x + 2, player_at.y, player_at.z),
        now,
    );
    let near_serial = world.registry().serial_of(near).unwrap();
    world.drain_outbound().count();
    say(&mut world, connection, "I should like some bread", now);
    let packets = packets_for(&mut world, connection);
    let answered = packets.iter().any(|p| {
        p[0] == 0xAE && mentions(p, near_serial) && {
            let text: Vec<u8> = p.iter().copied().filter(|&b| b != 0).collect();
            String::from_utf8_lossy(&text).contains("fresh and hot")
        }
    });
    assert!(answered, "the baker answered its keyword");

    // And a trade with no table stays quiet rather than borrowing the baker's line.
    let smith = spawn_townsperson(
        &mut world,
        "the blacksmith",
        Point::new(player_at.x, player_at.y + 2, player_at.z),
        now,
    );
    let smith_serial = world.registry().serial_of(smith).unwrap();
    world.drain_outbound().count();
    say(&mut world, connection, "tell me of iron", now);
    assert!(
        !packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0xAE && mentions(p, smith_serial)),
        "an unregistered trade has nothing to say"
    );
}

/// Everything a shop says, read back the way the client reads it.
///
/// The defect this exists for lives in neither end: `open_shop` writes six
/// kinds of packet and the client's own table has to know all six. The two
/// ways it can fail are not equally loud, and the shop had one of each.
///
/// A missing **length** is fatal. `Connection::poll` cannot find where the next
/// packet starts, so it ends the session — and the tooltip a shop sends per
/// stocked item (`0xD6`, written as bytes by `PropertyList`, named by no
/// `ServerPacket` variant) was not in the table. From inside the game that read
/// as a trade window that would not draw and a paperdoll that would not open
/// afterwards, because by then there was no shard on the other end.
///
/// A missing **decoder** is silent. The stream stays in step and the client
/// simply never learns what it was told: `0x2E` carried the stock crate and
/// `0x74` the catalogue, and without arms for either the window opened over an
/// empty shelf while every byte of its contents had arrived.
#[test]
fn a_shop_says_nothing_the_client_cannot_read() {
    use openshard_protocol::packet::Frame;
    use openshard_protocol::server_packet::{
        ServerPacket,
        frame_server_packet,
    };

    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let (_keeper, keeper_serial) = spawn_shopkeeper(&mut world, now);
    // Stock, so the catalogue has a line in it: an empty shelf would open a
    // window and prove nothing about the packets that describe one.
    world.queue(Command::StockVendor {
        serial: keeper_serial,
        stock:  vec![npc::StockLine {
            graphic:   Graphic(0x0F7B),
            hue:       Hue(0),
            item_kind: None,
            material:  None,
            amount:    openshard_state::components::Amount(5),
            price:     openshard_state::components::Price(3),
            name:      "black pearl".to_owned(),
        }],
    });
    world.tick(now);

    world.drain_outbound().count();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(keeper_serial.raw())),
    });
    world.tick(now);
    let packets = packets_for(&mut world, connection);
    assert!(
        packets.iter().any(|p| p.first() == Some(&0x74)),
        "the shop opened at all"
    );

    for packet in &packets {
        assert_eq!(
            frame_server_packet(packet, ClientVersion::TOL),
            Ok(Frame::Complete(packet.len())),
            "the client cannot frame {:#04x} and would drop the connection on it",
            packet[0]
        );
    }

    // And the four the window is actually built out of are understood, not
    // merely stepped over.
    let mut seen: Vec<u8> = Vec::new();
    for packet in &packets {
        if let Ok(Some(decoded)) = ServerPacket::decode(packet, ClientVersion::TOL) {
            seen.push(decoded.id());
            if let ServerPacket::BuyList(list) = decoded {
                assert_eq!(list.lines.len(), 1, "the catalogue arrived with its line");
                assert_eq!(list.lines[0].price, 3);
                assert_eq!(list.lines[0].name, "black pearl");
            }
        }
    }
    for id in [0x2E, 0x3C, 0x74, 0x24] {
        assert!(
            seen.contains(&id),
            "{id:#04x} reached the client as itself, not as an undecoded id"
        );
    }
}

#[test]
fn a_criminal_is_refused_at_every_door_into_a_shop() {
    // ServUO's `CheckVendorAccess`, and the reason it is checked in four places
    // rather than one: a client that already has the buy window open can still send
    // a `0x3B` purchase, so refusing only at the open leaves the deal reachable.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    // A real shopkeeper: the `vendor` flag is what gives it the stock crate
    // `open_shop` reads, and an empty crate still opens a window.
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        100,
        notoriety:   Notoriety::from_bits(7),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x + 1, START.y, 0),
        facet:       Facet(0),
        name:        None,
        title:       Some("the provisioner".to_owned()),
        shoe:        1,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      true,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let keeper = world
        .registry()
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a shopkeeper");
    let keeper_serial = world.registry().serial_of(keeper).unwrap();

    // Blue, and the shop opens.
    world.drain_outbound().count();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(keeper_serial.raw())),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p.first() == Some(&0x74)),
        "an innocent customer is served"
    );

    // Grey, and it is not — with the refusal said out loud, not swallowed.
    world
        .state
        .registry
        .insert(player, openshard_protocol::mobile::Notoriety::Criminal);
    world.drain_outbound().count();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(keeper_serial.raw())),
    });
    world.tick(now);
    let packets = packets_for(&mut world, connection);
    assert!(
        !packets.iter().any(|p| p.first() == Some(&0x74)),
        "a criminal gets no shop"
    );
    assert!(
        packets.iter().any(|p| p[0] == 0xAE),
        "and is told why, over the shopkeeper's head"
    );
}

/// A shard that keeps shop hours, started at `hour` o'clock.
fn shop_hours_world(hour: u64) -> World {
    World::new(START)
        .with_gameplay(Gameplay {
            npc_schedule: true,
            ..Default::default()
        })
        .with_clock_minutes(hour * 60)
}

/// Place a shopkeeper next to the player and return it.
///
/// Through `SpawnMobile` with `vendor: true`, not by inserting the marker after
/// the fact: the flag is what makes `make_vendor` hang the stock crate on the
/// mobile, and `open_shop` reads the crate. A `Vendor` with no crate is a
/// shopkeeper that silently refuses every customer.
fn spawn_shopkeeper(world: &mut World, now: Instant) -> (EntityId, Serial) {
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        100,
        notoriety:   Notoriety::from_bits(7),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x + 1, START.y, 0),
        facet:       Facet(0),
        name:        None,
        title:       Some("the provisioner".to_owned()),
        shoe:        1,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      true,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let keeper = world
        .registry()
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a shopkeeper");
    let serial = world.registry().serial_of(keeper).unwrap();
    (keeper, serial)
}

#[test]
fn a_shop_that_keeps_hours_is_shut_after_them() {
    // Ours, not either reference's — and it earns its place for a structural
    // reason rather than a flavourful one. A vendor's stock crate is *worn*, so
    // the shop is wherever the shopkeeper is standing; a shopkeeper that has
    // walked off for the night is still a shop unless something says otherwise.
    // Checked at the same door the criminal refusal uses, so all four ways in are
    // covered by one predicate.
    let now = Instant::now();

    // Midday: open.
    let mut world = shop_hours_world(12);
    let connection = enter(&mut world, now);
    let (_, keeper_serial) = spawn_shopkeeper(&mut world, now);
    world.drain_outbound().count();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(keeper_serial.raw())),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p.first() == Some(&0x74)),
        "a customer at midday is served"
    );

    // Ten at night, past the default 21:00: shut, and said out loud.
    let mut world = shop_hours_world(22);
    let connection = enter(&mut world, now);
    let (_, keeper_serial) = spawn_shopkeeper(&mut world, now);
    world.drain_outbound().count();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(keeper_serial.raw())),
    });
    world.tick(now);
    let packets = packets_for(&mut world, connection);
    assert!(
        !packets.iter().any(|p| p.first() == Some(&0x74)),
        "the shop is shut for the night"
    );
    assert!(
        packets.iter().any(|p| p[0] == 0xAE),
        "and the shopkeeper says so rather than ignoring the customer"
    );
}

#[test]
fn a_shard_with_no_schedule_never_closes() {
    // The whole routine hangs off one setting. A shard that has not asked for a
    // daily routine has no closing time either, whatever hour it happens to be —
    // otherwise turning the clock on would quietly shut every shop at night.
    let now = Instant::now();
    let mut world = World::new(START).with_clock_minutes(22 * 60);
    let connection = enter(&mut world, now);
    let (_, keeper_serial) = spawn_shopkeeper(&mut world, now);
    world.drain_outbound().count();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(keeper_serial.raw())),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p.first() == Some(&0x74)),
        "no schedule, no closing time"
    );
}

#[test]
fn a_traveller_asks_for_an_escort_out_loud() {
    // ServUO's `BaseEscortable.OnMovement` says "I am looking to go to X, will you take
    // me?" when someone comes near, and that is what makes sixty travellers scattered
    // across a facet findable at all. It has to be *speech*, not a system line to the
    // player: the ask is heard, so a second player standing there knows an escort is
    // going begging.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(player_at) = *world.registry().get::<Position>(player).unwrap();

    let traveller = spawn_townsperson(
        &mut world,
        "a wandering healer",
        Point::new(player_at.x + 2, player_at.y, player_at.z),
        now,
    );
    world.state.registry.insert(
        traveller,
        openshard_state::components::Escortable {
            destination: "Britain".to_owned(),
            escorter:    None,
            last_seen:   openshard_state::WorldTick::ZERO,
        },
    );
    let traveller_serial = world.registry().serial_of(traveller).unwrap();
    world.drain_outbound().count();
    for _ in 0..45 {
        world.tick(now);
    }
    let asked = packets_for(&mut world, connection).iter().any(|p| {
        p[0] == 0xAE && mentions(p, traveller_serial) && {
            let text: Vec<u8> = p.iter().copied().filter(|&b| b != 0).collect();
            String::from_utf8_lossy(&text).contains("Britain")
        }
    });
    assert!(asked, "the traveller named where it wants to go");

    // Once it is being led it has nothing left to ask for, and falls back to whatever
    // its trade would say.
    world.state.registry.insert(
        traveller,
        openshard_state::components::Escortable {
            destination: "Britain".to_owned(),
            escorter:    world.registry().serial_of(player),
            last_seen:   openshard_state::WorldTick::ZERO,
        },
    );
    world.drain_outbound().count();
    for _ in 0..(15 * 20 + 45) {
        world.tick(now);
    }
    let asked_again = packets_for(&mut world, connection).iter().any(|p| {
        p[0] == 0xAE && mentions(p, traveller_serial) && {
            let text: Vec<u8> = p.iter().copied().filter(|&b| b != 0).collect();
            String::from_utf8_lossy(&text).contains("will you take me")
        }
    });
    assert!(!asked_again, "a traveller already being led must not keep asking");
}

#[test]
fn a_crowd_of_townsfolk_does_not_beat_in_lockstep() {
    // A `Populate` places seven hundred townsfolk on one tick. With a shared
    // `next_beat` of zero every one of their beats falls on the same tick for ever
    // after — a curiosity until they all path home at dusk together and a whole
    // facet's A* bill lands at once. `spawn` jitters the first beat across one beat's
    // span, the way `register_spawner` jitters a region's first spawn.
    let now = Instant::now();
    let mut world = world();
    for i in 0..40u16 {
        spawn_townsperson(
            &mut world,
            "the peasant",
            Point::new(START.x + 20 + i, START.y + 20, 0),
            now,
        );
    }
    let beats: std::collections::HashSet<u64> = world
        .registry()
        .query::<openshard_state::components::Npc>()
        .map(|(_, npc)| npc.next_beat.raw())
        .collect();
    assert!(
        beats.len() > 10,
        "forty townsfolk should not share a handful of beats, saw {} distinct",
        beats.len()
    );
}

#[test]
fn a_townsperson_walks_home_at_night_when_the_shard_asks_for_it() {
    // `gameplay.npc_schedule` is ours, not a port, and it is only reachable because a
    // spawn can name a `night_home` — without that field the setting was a flag
    // nothing in the engine could ever satisfy.
    let now = Instant::now();
    let post = Point::new(START.x + 4, START.y + 4, 0);
    let home = Point::new(START.x + 12, START.y + 4, 0);

    let gameplay = Gameplay {
        npc_schedule: true,
        ..Gameplay::default()
    };
    let mut world = World::new(START).with_gameplay(gameplay);
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        100,
        notoriety:   Notoriety::from_bits(7),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    post,
        facet:       Facet(0),
        name:        None,
        title:       Some("the peasant".to_owned()),
        shoe:        1,
        fame:        0,
        karma:       0,
        night_home:  Some(home),
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let peasant = world
        .registry()
        .query::<openshard_state::components::Title>()
        .map(|(entity, _)| entity)
        .next_back()
        .expect("a townsperson");
    assert!(
        world
            .registry()
            .has::<openshard_state::components::NightHome>(peasant),
        "the spawn's night home reached the mobile"
    );

    // Wind the clock into the small hours. The hour is derived from the tick counter
    // at ServUO's rate, so this is a number of ticks and not a wall clock.
    let per_hour = world.state.gameplay.uo_minute_ticks * 60;
    world.state.ticks += per_hour * 2; // 02:00 — outside working hours
    let start = world.registry().get::<Position>(peasant).unwrap().0;
    for _ in 0..3000 {
        world.tick(now);
    }
    let at = world.registry().get::<Position>(peasant).unwrap().0;
    let moved_toward_home =
        i32::from(at.x).abs_diff(i32::from(home.x)) < i32::from(start.x).abs_diff(i32::from(home.x));
    assert!(
        moved_toward_home,
        "at night a townsperson heads home: started {start:?}, reached {at:?}, home {home:?}"
    );
}

#[test]
fn a_restored_townsperson_still_knows_its_trade() {
    // The `quest_giver` lesson applied ahead of time: the trade is the key an NPC's
    // speech table is looked up by on every word spoken near it, so an NPC restored
    // without it is a mute statue no save file can be told apart from a working one.
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    let smith = spawn_townsperson(
        &mut world,
        "the blacksmith",
        Point::new(START.x + 3, START.y, 0),
        now,
    );
    let name = world.registry().get::<Name>(smith).unwrap().0.clone();

    let records = world.mobile_records();
    assert!(
        records
            .iter()
            .any(|r| r.title.as_deref() == Some("the blacksmith")),
        "the trade is swept into the save"
    );

    // A fresh world restoring that sweep gets its trade and its beat back.
    let mut booted = World::new(START);
    let filed = nothing_restored_first(&mut booted);
    booted.restore_mobiles(records, &filed);
    let restored = booted
        .registry()
        .query::<openshard_state::components::Title>()
        .find(|(_, title)| title.0 == "the blacksmith")
        .map(|(entity, _)| entity)
        .expect("the trade came back");
    assert_eq!(
        booted.registry().get::<Name>(restored).unwrap().0,
        name,
        "and so did the person"
    );
    assert!(
        booted
            .registry()
            .has::<openshard_state::components::Npc>(restored),
        "and its beat, or it stands frozen after every restart"
    );
}

#[test]
fn restored_townsfolk_do_not_all_beat_on_the_same_tick() {
    // The bug this protects against was invisible from a fresh shard and permanent
    // on a real one. `spawn` jitters an NPC's first beat, so a `Populate` reads
    // fine — but a shard is populated once and *restored* on every boot after
    // that, and the restore path handed every NPC a beat of zero. So the jitter
    // ran once in a shard's life and the first save undid it: from then on the
    // whole town greeted, turned and wandered on one tick, for ever, because a
    // beat re-armed to a constant preserves whatever phase it is given.
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    for i in 0..12u16 {
        spawn_townsperson(
            &mut world,
            "the peasant",
            Point::new(START.x + 3, START.y + i, 0),
            now,
        );
    }

    let mut booted = World::new(START);
    let filed = nothing_restored_first(&mut booted);
    booted.restore_mobiles(world.mobile_records(), &filed);
    let beats: std::collections::BTreeSet<u64> = booted
        .registry()
        .query::<openshard_state::components::Npc>()
        .map(|(_, npc)| npc.next_beat.raw())
        .collect();
    assert!(
        beats.len() > 1,
        "twelve restored townsfolk share {} beat(s): the town moves as one body",
        beats.len()
    );

    // And they stay apart. A constant re-arm would keep whatever spread the
    // restore happened to give them but never widen it, and any two that landed
    // together would be welded together — so assert on the pairs, not the count.
    for _ in 0..(openshard_npc::BEAT_TICKS * 4) {
        booted.tick(now);
    }
    let beats: Vec<u64> = booted
        .registry()
        .query::<openshard_state::components::Npc>()
        .map(|(_, npc)| npc.next_beat.raw())
        .collect();
    let unique: std::collections::BTreeSet<u64> = beats.iter().copied().collect();
    assert!(
        unique.len() * 2 > beats.len(),
        "after four beats most townsfolk still share a tick ({} distinct of {})",
        unique.len(),
        beats.len()
    );
}

#[test]
fn single_clicking_a_named_mobile_draws_its_name() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    spawn_banker(&mut world, Point::new(START.x + 1, START.y, 0), now);
    let banker = world.registry().query::<Banker>().next().map(|(e, _)| e).unwrap();
    let banker_serial = world.registry().serial_of(banker).unwrap();
    let _ = packets_for(&mut world, connection);

    world.queue(Command::SingleClick {
        connection,
        serial: banker_serial,
    });
    world.tick(now);

    // A 0x1C label naming the banker, in the invulnerable (yellow) hue.
    let label = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0x1C)
        .expect("a name label was sent");
    // hue is at bytes 10..12 of a 0x1C.
    let hue = u16::from_be_bytes([label[10], label[11]]);
    assert_eq!(hue, 0x0035, "the banker's name is drawn yellow");
    assert!(
        String::from_utf8_lossy(&label).contains("the banker"),
        "the label carries the name"
    );
}

/// A tiledata that names one graphic — enough to test that a single-click on an
/// item reads its name off the shard's table.
///
/// A real `TileData` and not a hand-written `Terrain`: the name is a row in
/// `tiledata.mul`, placeholders and all, and the reader that decides `"NoName"`
/// means *no name* is the one under test.
fn named(graphic: u16, name: &str) -> openshard_tiles::TileData {
    let mut tiles = openshard_tiles::TileData::empty();
    tiles.set_static_tile(
        graphic,
        openshard_tiles::StaticTile {
            name: name.to_owned(),
            ..openshard_tiles::StaticTile::default()
        },
    );
    tiles
}

#[test]
fn single_clicking_an_item_draws_its_tiledata_name() {
    let now = Instant::now();
    let mut world = world();
    world.state.set_tiles(named(GOLD, "gold coins"));
    let connection = enter(&mut world, now);
    // A stack of three on the player's tile, so it is drawn and clickable.
    let serial = spawn_gold(&mut world, Point::new(START.x, START.y, 0), 3, now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::SingleClick { connection, serial });
    world.tick(now);

    let label = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0x1C)
        .expect("a name label was sent");
    assert!(
        String::from_utf8_lossy(&label).contains("3 gold coins"),
        "the label carries the amount and the tiledata name"
    );
}

#[test]
fn querying_a_stacks_properties_sends_the_amount_cliloc() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let serial = spawn_gold(&mut world, Point::new(START.x, START.y, 0), 3, now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::QueryProperties {
        connection,
        serials: vec![RawSerial(serial.raw())],
    });
    world.tick(now);

    let opl = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0xD6)
        .expect("a property list was sent");
    // The first entry's cliloc sits at offset 15; a stack uses 1050039
    // (~1_NUMBER~ ~2_ITEMNAME~), not the bare tiledata cliloc.
    let cliloc = u32::from_be_bytes([opl[15], opl[16], opl[17], opl[18]]);
    assert_eq!(cliloc, 1_050_039, "a stack labels through the amount cliloc");
}

#[test]
fn a_drawn_object_carries_a_tooltip_revision() {
    // A modern client (TOL) with the default send-version tooltips gets a 0xDC
    // revision alongside the 0x78 that draws a mobile.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);
    spawn_banker(&mut world, Point::new(START.x + 1, START.y, 0), now);

    let packets = packets_for(&mut world, connection);
    assert!(
        packets.iter().any(|p| p[0] == 0xDC),
        "the banker's tooltip revision rides along with its draw"
    );
}

#[test]
fn tooltips_off_sends_no_revision() {
    let now = Instant::now();
    let mut world = world();
    world.state.gameplay.tooltip_mode = openshard_state::TooltipMode::Off;
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);
    spawn_banker(&mut world, Point::new(START.x + 1, START.y, 0), now);

    let packets = packets_for(&mut world, connection);
    assert!(
        !packets.iter().any(|p| p[0] == 0xDC),
        "no tooltips means no revision packet"
    );
}

/// A shop floor seven units above ground nobody stands on — the only walkable
/// surface, and out of one step's reach.
///
/// **A real map, and the two answers come out of one shape.** The double asserted
/// `stand_z: None` and `spawn_z: Some(7)` side by side, which is two claims that
/// could disagree; here they are consequences of a floor at 7 over impassable
/// land, which is what a shop built on a rock face actually is. Seven is more
/// than [`MAX_STEP_UP`](openshard_movement::MAX_STEP_UP), so a step cannot reach
/// it and a placement can.
fn a_raised_floor() -> Scene {
    // Land id `0` is what a flat scene is paved with, so flagging the id makes
    // the whole square ground nobody stands on — no pass over its cells.
    let mut scene = Scene::flat_holding(START.x + 4, START.y + 4, 0);
    scene.land_art(0, openshard_tiles::TileFlags::BLOCK);
    scene.floor(START.x, START.y, 0, 7);
    scene
}

#[test]
fn a_spawn_stands_on_the_floor_not_under_it() {
    let now = Instant::now();
    let mut world = world();
    stand_on(&mut world, a_raised_floor());
    // Placed at z=0 (as the pack does), the shop floor is at z=7.
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        100,
        notoriety:   Notoriety::from_bits(7),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x, START.y, 0),
        facet:       Facet(0),
        name:        Some("the tailor".to_owned()),
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);

    let (entity, _) = world
        .state
        .registry
        .query::<Body>()
        .find(|(e, _)| !world.state.registry.has::<Client>(*e))
        .expect("the tailor spawned");
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0.z,
        7,
        "the NPC is placed on the shop floor, not the ground beneath it"
    );
}

#[test]
fn an_unnamed_creature_takes_its_body_default_name() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    // A chicken (body 0xD0) with no name given.
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x00D0),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        10,
        notoriety:   Notoriety::from_bits(1),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x + 1, START.y, 0),
        facet:       Facet(0),
        name:        None,
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let chicken = world
        .state
        .registry
        .query::<Body>()
        .filter(|(e, _)| !world.state.registry.has::<Client>(*e))
        .filter_map(|(e, _)| world.state.registry.serial_of(e))
        .max()
        .expect("a chicken was spawned");
    let _ = packets_for(&mut world, connection);

    world.queue(Command::SingleClick {
        connection,
        serial: chicken,
    });
    world.tick(now);

    let label = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0x1C)
        .expect("a name label was sent");
    assert!(
        String::from_utf8_lossy(&label).contains("a chicken"),
        "an unnamed creature names itself from its body"
    );
}

#[test]
fn entering_the_world_advertises_aos_features() {
    // The 0xB9 ClassicUO reads to turn on in-world tooltips and context menus —
    // ServUO sends it at world entry, and without it a modern client never asks.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let packets = packets_for(&mut world, connection);
    assert!(
        packets.iter().any(|p| p[0] == 0xB9),
        "world entry advertises AoS SupportedFeatures"
    );
}

#[test]
fn entering_with_aos_off_advertises_nothing() {
    let now = Instant::now();
    let mut world = world();
    world.state.gameplay.tooltip_mode = openshard_state::TooltipMode::Off;
    world.state.gameplay.context_menus = false;
    let connection = enter(&mut world, now);
    let packets = packets_for(&mut world, connection);
    assert!(
        !packets.iter().any(|p| p[0] == 0xB9),
        "no AoS is advertised when both tooltips and context menus are off"
    );
}

#[test]
fn a_drawn_mobile_carries_its_health_bar() {
    // The bar is populated on sight, so it reads full before any fight — not the
    // empty frame you get when health is only sent on a blow.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);
    // A placid creature (sight 0) so nothing but the draw sends a 0xA1.
    spawn_creature(&mut world, Point::new(START.x + 1, START.y, 0), 0, false, now);

    let packets = packets_for(&mut world, connection);
    assert!(
        packets.iter().any(|p| p[0] == 0xA1),
        "the health bar rides along with the draw"
    );
}

#[test]
fn a_context_menu_on_a_container_offers_open() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let container = spawn_container_at(&mut world, Point::new(START.x, START.y, 0), now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::ContextMenuRequest {
        connection,
        serial: RawSerial(container.raw()),
    });
    world.tick(now);

    // A 0xBF display-popup (subcommand 0x14 at bytes 3..5).
    let popup = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0xBF && p[3] == 0x00 && p[4] == 0x14)
        .expect("a context menu was sent");
    // The first entry's cliloc sits at offset 12: 3000362 "Open".
    let cliloc = u32::from_be_bytes([popup[12], popup[13], popup[14], popup[15]]);
    assert_eq!(cliloc, 3_000_362, "a container offers Open");
}

#[test]
fn selecting_open_on_a_container_opens_it() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let container = spawn_container_at(&mut world, Point::new(START.x, START.y, 0), now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::ContextMenuSelect {
        connection,
        serial: RawSerial(container.raw()),
        index: openshard_protocol::context::RawContextMenuIndex(0),
    });
    world.tick(now);

    let packets = packets_for(&mut world, connection);
    assert!(
        packets.iter().any(|p| p[0] == 0x24),
        "picking Open routes to the same use rule a double-click does"
    );
}

#[test]
fn context_menus_off_sends_no_popup() {
    let now = Instant::now();
    let mut world = world();
    world.state.gameplay.context_menus = false;
    let connection = enter(&mut world, now);
    let container = spawn_container_at(&mut world, Point::new(START.x, START.y, 0), now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::ContextMenuRequest {
        connection,
        serial: RawSerial(container.raw()),
    });
    world.tick(now);

    assert!(
        !packets_for(&mut world, connection).iter().any(|p| p[0] == 0xBF),
        "context menus off means no popup"
    );
}

/// Flat ground with one wall standing at `(START.x + 1, START.y)` — the tile
/// between somebody at [`START`] and somebody two east of it.
///
/// **A wall is a tile, so it takes a tile to be behind one.** `line_tiles`
/// returns the tiles strictly *between* two points, which for neighbours is
/// none: no map can make a sight line between adjacent tiles anything but clear.
/// The double this replaced said sight was never clear from anywhere to
/// anywhere, which let a test claim a wall stood between two tiles that touch.
/// See `docs/world/research/terrain_seam.md` — the gate it was standing in for cannot fire
/// at melee range at all.
fn a_wall_two_tiles_across() -> Scene {
    let mut scene = Scene::flat_holding(START.x + 4, START.y + 4, 0);
    scene.wall(START.x + 1, START.y, 0, 20);
    scene
}

/// Spawn a stocked vendor at `point` and return its serial.
pub(super) fn spawn_stocked_vendor(world: &mut World, point: Point, now: Instant) -> Serial {
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(1),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    point,
        facet:       Facet(0),
        name:        Some("the tailor".to_owned()),
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      true,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let vendor = world
        .state
        .registry
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a vendor spawned");
    let serial = world.registry().serial_of(vendor).unwrap();
    world.queue(Command::StockVendor {
        serial,
        stock: vec![npc::StockLine {
            graphic:   openshard_protocol::wire::Graphic(0x0F7A),
            hue:       openshard_protocol::wire::Hue(0),
            item_kind: None,
            material:  None,
            amount:    openshard_state::components::Amount(50),
            price:     openshard_state::components::Price(4),
            name:      "black pearl".to_owned(),
        }],
    });
    world.tick(now);
    serial
}

#[test]
fn a_vendor_at_the_counter_sells() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let vendor = spawn_stocked_vendor(&mut world, Point::new(START.x + 1, START.y, 0), now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(vendor.raw())),
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, connection).iter().any(|p| p[0] == 0x74),
        "the buy window opens for a customer at the counter"
    );
}

#[test]
fn a_vendor_behind_a_wall_will_not_sell() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    // Well inside the four-tile trade range, with a wall on the one tile the
    // sight line crosses. Two apart rather than adjacent because that is what it
    // takes for anything to be *between* them: a wall is a whole tile.
    let vendor = spawn_stocked_vendor(&mut world, Point::new(START.x + 2, START.y, 0), now);
    stand_on(&mut world, a_wall_two_tiles_across());
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(vendor.raw())),
    });
    world.tick(now);

    assert!(
        !packets_for(&mut world, connection).iter().any(|p| p[0] == 0x74),
        "no buying through a wall, however near"
    );
}

#[test]
fn a_vendor_across_the_street_is_out_of_reach() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    // In the open (sight clear), but well beyond the trade range.
    let vendor = spawn_stocked_vendor(&mut world, Point::new(START.x + 10, START.y, 0), now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(vendor.raw())),
    });
    world.tick(now);

    assert!(
        !packets_for(&mut world, connection).iter().any(|p| p[0] == 0x74),
        "no buying from across the street"
    );
}

#[test]
fn saying_bank_with_no_banker_near_does_nothing() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    // A banker, but far out of the 12-tile reach.
    spawn_banker(&mut world, Point::new(START.x + 40, START.y, 0), now);
    let _ = packets_for(&mut world, connection);

    say(&mut world, connection, "bank", now);
    assert!(
        !packets_for(&mut world, connection).iter().any(|p| p[0] == 0x24),
        "no banker in reach, no bank box"
    );
}

#[test]
fn a_loaded_character_returns_on_its_saved_serial_and_spot() {
    // Load-on-play: a stored character is played with its saved serial and
    // position, and must come back exactly there — not at the start point,
    // and not on a fresh serial that would orphan every reference to it.
    let mut world = world();
    let connection = connection();
    on_file(
        &mut world,
        Serial::new(0x0000_0202).unwrap(),
        Point::new(1500, 1000, -5),
        Appearance {
            body: Graphic(0x0191),
            hue:  openshard_protocol::wire::Hue(0x83EA),
        },
    );
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Lord British".to_owned()),
        access: AccessLevel::Player,
        character: Character::Saved,
    }));
    world.tick(Instant::now());

    let entity = world.state.players[&connection];
    assert_eq!(
        world.registry().serial_of(entity).unwrap().raw(),
        0x0000_0202,
        "it kept its saved serial"
    );
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        Point::new(1500, 1000, -5),
        "and its saved spot, z and all"
    );
}

#[test]
fn a_loaded_character_saved_under_a_live_floor_returns_on_top_of_it() {
    let mut world = world();
    let floor = world.state.registry.spawn();
    world.state.facet_state_mut(Facet(0)).block(
        1500,
        1000,
        floor,
        openshard_map::overlay::Cover::standing(7, 0),
    );
    let connection = connection();
    on_file(
        &mut world,
        Serial::new(0x0000_0202).unwrap(),
        Point::new(1500, 1000, 0),
        Appearance::default_human(),
    );
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Lord British".to_owned()),
        access: AccessLevel::Player,
        character: Character::Saved,
    }));
    world.tick(Instant::now());

    let entity = world.state.players[&connection];
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        Point::new(1500, 1000, 7),
        "an invalid saved z below the live floor was preserved"
    );
}

#[test]
fn a_deleted_character_is_no_longer_on_file_under_its_name() {
    // The other end of the roster's life, and the half S4 moved into the world:
    // `0x83` now names the character rather than the serial the shard used to
    // look up, so this is the whole of what deletion does — the row stops being
    // there. Someone who creates the same name afterwards must get a new
    // character, not the deleted one's serial, gear and spot.
    //
    // The name is lower-cased on the way in on purpose: the client sends it back
    // as the player typed it, and the key folds. A delete that missed by case
    // would leave the row behind and this test would find the old serial.
    let now = Instant::now();
    let mut world = world();
    on_file(
        &mut world,
        Serial::new(0x0000_0202).unwrap(),
        Point::new(1500, 1000, -5),
        Appearance::default_human(),
    );
    delete_slot(&mut world, 0, now);

    let connection = connection();
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Lord British".to_owned()),
        access: AccessLevel::Player,
        character: Character::Saved,
    }));
    world.tick(now);

    let entity = world.state.players[&connection];
    assert_ne!(
        world.registry().serial_of(entity).unwrap().raw(),
        0x0000_0202,
        "the deleted serial is not handed back out"
    );
    let position = world.registry().get::<Position>(entity).unwrap().0;
    assert_eq!(
        Tile::new(position.x, position.y),
        START,
        "and the new character stands at the start, not where the deleted one stood"
    );
}

#[test]
fn a_character_is_on_its_account_list_from_the_moment_it_enters() {
    // S5's half of the roster: it says which characters *exist*, not only where
    // the saved ones were. A character created this run enters once and nothing
    // describes it until it logs out, so a list built from the saved records
    // alone would be missing the very character being played — which is the list
    // `0xA9` draws and `0x83` indexes.
    let now = Instant::now();
    let mut world = world();
    let admin = AccountName("admin".to_owned());
    assert!(
        world.characters(&admin).is_empty(),
        "a world nobody has entered has nobody on file"
    );

    enter(&mut world, now);
    assert_eq!(
        world
            .characters(&admin)
            .into_iter()
            .map(|entry| entry.name.0)
            .collect::<Vec<_>>(),
        ["Lord British"],
        "the character being played is on the list before it has ever been saved"
    );
}

#[test]
fn entering_a_character_boot_already_knew_does_not_list_it_twice() {
    // The idempotence `enter` relies on. Boot enrols every stored row, and then
    // the same character is played — two writers naming one character. A
    // duplicate would make `0x5D` ambiguous, because it echoes the name and not
    // the slot, and it would show the player two identical rows to pick from.
    let now = Instant::now();
    let mut world = world();
    on_file(
        &mut world,
        Serial::new(0x0000_0202).unwrap(),
        Point::new(1500, 1000, -5),
        Appearance::default_human(),
    );
    let admin = AccountName("admin".to_owned());
    assert_eq!(world.characters(&admin).len(), 1, "boot put it on the list");

    let connection = connection();
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: admin.clone(),
        name: CharacterName("Lord British".to_owned()),
        access: AccessLevel::Player,
        character: Character::Saved,
    }));
    world.tick(now);

    assert_eq!(world.characters(&admin).len(), 1, "and playing it kept one");
    assert_eq!(
        world
            .registry()
            .serial_of(world.state.players[&connection])
            .unwrap()
            .raw(),
        0x0000_0202,
        "on the serial the stored row carried, so the enrolment did not overwrite it"
    );
}

#[test]
fn a_deleted_character_leaves_the_list_even_with_nothing_saved() {
    // The case the old `forget` dropped on the floor: a character created this
    // run has no record, so the early return took the *list* removal with it and
    // the character came back on the next `0xA9`. Deleting is the one operation
    // where "no record" must not mean "nothing to do".
    let now = Instant::now();
    let mut world = world();
    let admin = AccountName("admin".to_owned());
    // Enrolled and never played, so nothing has written a record for it — which
    // is exactly the character the old code could not delete.
    world.enrol_character(&admin, &CharacterName("Lord British".to_owned()));
    assert_eq!(world.characters(&admin).len(), 1);

    delete_slot(&mut world, 0, now);

    assert!(
        world.characters(&admin).is_empty(),
        "it is off the account's list, and the slot indexed the list the screen was sent"
    );
}

#[test]
fn a_saved_character_remembers_whose_it_is() {
    // The other half: `record_of` fills the account from the entity, so a
    // saved character can be tied back to its owner on load. A blank account
    // here is what left every loaded character ownerless before.
    let mut world = world();
    enter(&mut world, Instant::now());
    world.take_snapshot();
    let snapshot = world
        .drain_saves()
        .next()
        .expect("entering the world is a change worth saving");
    assert_eq!(snapshot.characters[0].account, "admin");
    assert_eq!(snapshot.characters[0].name, "Lord British");
}

/// Register a mapless facet, so a test can populate more than one without
/// client files. Its interest grid is the same no-map size facet 0 uses.
pub(super) fn add_empty_facet(world: &mut World, facet: Facet) {
    add_empty_facet_sized(world, facet, FACET_WITHOUT_A_MAP.0, FACET_WITHOUT_A_MAP.1);
}

/// The same, at a size of the test's choosing — the facets are not all the
/// shape of Britannia, and what the client is told about that is a rule.
pub(super) fn add_empty_facet_sized(world: &mut World, facet: Facet, width: u32, height: u32) {
    world.state.facets.insert(
        facet,
        // No map, so there is nothing to bake a span index over: the table
        // is only along for the signature.
        FacetState::new(
            None,
            None,
            width,
            height,
            FacetRules::classic(facet),
            None,
            &openshard_tiles::TileData::empty(),
        ),
    );
}

pub(super) fn enter_on_facet(world: &mut World, connection: ConnectionId, facet: Facet, now: Instant) {
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("P".to_owned()),
        access: AccessLevel::Player,
        character: Character::fresh(facet),
    }));
    world.tick(now);
}

#[test]
fn two_facets_do_not_see_each_other() {
    // The whole point of a per-facet interest grid: two mobiles standing on
    // the very same coordinates, one on Felucca and one on Trammel, share no
    // screen. If this ever fails, someone reached for a single global grid.
    let mut world = world();
    add_empty_facet(&mut world, Facet(1));
    let now = Instant::now();
    let here = ConnectionId::from_raw(1);
    let there = ConnectionId::from_raw(2);
    enter_on_facet(&mut world, here, Facet(0), now);
    enter_on_facet(&mut world, there, Facet(1), now);

    let a = world.state.players[&here];
    let b = world.state.players[&there];
    assert!(
        !world.state.seen[&a].contains(&b),
        "a mobile on facet 0 must not have drawn one on facet 1"
    );
    assert!(!world.state.seen[&b].contains(&a), "nor the other way round");
}

#[test]
fn one_facet_at_the_same_spot_does_see() {
    // The control: the isolation above is facet-specific, not a bug that
    // hides everyone. Same coordinates, same facet — they see each other.
    let mut world = world();
    let now = Instant::now();
    let here = ConnectionId::from_raw(1);
    let there = ConnectionId::from_raw(2);
    enter_on_facet(&mut world, here, Facet(0), now);
    enter_on_facet(&mut world, there, Facet(0), now);

    let a = world.state.players[&here];
    let b = world.state.players[&there];
    assert!(
        world.state.seen[&a].contains(&b),
        "same facet, same spot: they see"
    );
    assert!(world.state.seen[&b].contains(&a));
}

#[test]
fn entering_twice_on_one_connection_is_ignored() {
    // "One connection" is now said rather than inherited: this test used two
    // bare `enter`s back when `connection()` handed back the same id every
    // time, so its subject was an accident of the helper. `enter_as` with a
    // named id is the same scene, stated.
    let mut world = world();
    let now = Instant::now();
    let one = ConnectionId::from_raw(1);
    enter_as(&mut world, one, now);
    enter_as(&mut world, one, now);
    assert_eq!(world.player_count(), 1);
}

#[test]
fn walking_moves_the_position_component_too() {
    // Two places hold a position — `Position` and the `Movement`'s walker —
    // and a system that reads one while the other has moved is a rubber-band
    // bug. The tick is what keeps them in step.
    let mut world = world();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let _ = world.drain_outbound().count();

    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(now);

    let entity = world.state.players[&connection];
    let Position(position) = *world.registry().get::<Position>(entity).unwrap();
    let Movement(walker) = *world.registry().get::<Movement>(entity).unwrap();
    assert_eq!(position, walker.position, "the two must not drift apart");
    assert_eq!(position, Point::new(START.x, START.y + 1, Z_WITHOUT_A_MAP));
}

#[test]
fn walking_emits_an_event_and_acks() {
    let mut world = world();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let _ = world.drain_outbound().count();
    let mut moves: Cursor<MobileMoved> = world.bus().cursor();

    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(now);

    let sent: Vec<Vec<u8>> = world.drain_outbound().map(|out| out.packet).collect();
    assert_eq!(sent, vec![vec![0x22, 0, Notoriety::Innocent.to_bits()]]);

    let moved: Vec<_> = world.bus().read(&mut moves).copied().collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].from, Point::new(START.x, START.y, Z_WITHOUT_A_MAP));
    assert_eq!(moved[0].to, Point::new(START.x, START.y + 1, Z_WITHOUT_A_MAP));
}

#[test]
fn turning_emits_a_turn_not_a_move() {
    // A listener that cares where things are should not have to filter out
    // events where nothing went anywhere.
    let mut world = world();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let mut moves: Cursor<MobileMoved> = world.bus().cursor();
    let mut turns: Cursor<MobileTurned> = world.bus().cursor();

    // Spawned facing south; ask for north.
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::North),
    });
    world.tick(now);

    assert_eq!(world.bus().read(&mut moves).count(), 0, "nothing moved");
    assert_eq!(world.bus().read(&mut turns).count(), 1);
}

#[test]
fn an_out_of_sequence_step_says_so() {
    let mut world = world();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let mut refused: Cursor<StepRefused> = world.bus().cursor();

    world.queue(Command::Walk {
        connection,
        request: walk(9, Direction::South),
    });
    world.tick(now);

    let events: Vec<_> = world.bus().read(&mut refused).copied().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].reason, RefusedReason::OutOfSequence);
}

#[test]
fn a_flood_is_refused_and_says_so() {
    // The pace, through the tick. Every step in one instant is a speedhack.
    let mut world = world();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let _ = world.drain_outbound().count();

    for sequence in 0..200u8 {
        world.queue(Command::Walk {
            connection,
            request: walk(sequence, Direction::South),
        });
    }
    world.tick(now);

    let rejects = world.drain_outbound().filter(|out| out.packet[0] == 0x21).count();
    assert!(rejects > 150, "only {rejects} of 200 instant steps refused");
}

#[test]
fn an_honest_walker_is_never_refused_across_ticks() {
    let mut world = world();
    let start = Instant::now();
    let connection = enter(&mut world, start);
    let _ = world.drain_outbound().count();

    let mut sequence = 0u8;
    for step in 0..200u32 {
        let now = start + WALK_INTERVAL * step;
        world.queue(Command::Walk {
            connection,
            request: walk(sequence, Direction::South),
        });
        world.tick(now);
        let refused = world.drain_outbound().filter(|out| out.packet[0] == 0x21).count();
        assert_eq!(refused, 0, "step {step} refused");
        sequence = if sequence == u8::MAX { 1 } else { sequence + 1 };
    }
}

#[test]
fn a_walk_from_a_connection_with_no_character_is_ignored() {
    let mut world = world();
    world.queue(Command::Walk {
        connection: connection(),
        request:    walk(0, Direction::South),
    });
    world.tick(Instant::now());
    assert_eq!(world.drain_outbound().count(), 0);
}

#[test]
fn disconnecting_releases_the_entity_and_its_serial() {
    let mut world = world();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = world.registry().serial_of(entity).unwrap();

    let mut left: Cursor<PlayerLeft> = world.bus().cursor();
    world.queue(Command::Disconnect { connection });
    world.tick(now);

    assert_eq!(world.player_count(), 0);
    assert!(!world.registry().contains(entity));
    assert_eq!(
        world.registry().entity_of(serial),
        None,
        "a dead serial resolves to nothing"
    );
    assert_eq!(world.bus().read(&mut left).count(), 1);
}

#[test]
fn a_departing_character_is_filed_where_it_walked_to() {
    // The re-login rewind bug: the logout must file the character's *current*
    // position, so a re-login this run spawns it where it left — not where it
    // logged in. It used to be asserted on the `departed` vector the shard
    // drained; since S4 the roster is the world's, and the only way to ask what
    // it filed is to log back in and look, which is also what a player does.
    let mut world = world();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let start = world.registry().get::<Position>(entity).unwrap().0;
    let walked_to = Point::new(start.x + 9, start.y + 4, start.z);
    teleport(&mut world, connection, walked_to);

    world.queue(Command::Disconnect { connection });
    world.tick(now);

    let again = ConnectionId::from_raw(2);
    world.queue(Command::Enter(Entering {
        connection: again,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::Player,
        character:  Character::Saved,
    }));
    world.tick(now);

    let entity = world.state.players[&again];
    let position = world.registry().get::<Position>(entity).unwrap().0;
    assert_eq!(
        (position.x, position.y),
        (walked_to.x, walked_to.y),
        "the logout filed the moved position, not the login one"
    );
}

#[test]
fn a_disconnect_takes_everything_the_connection_was_in_the_middle_of() {
    // The point of S7: the per-connection state is fields on one row, so letting
    // go of the row lets go of all of it. This used to be a list of maps cleared
    // by name in `disconnect`, and the map added without a line there leaked —
    // which is not hypothetical, it is what the four gump tables did.
    //
    // Asserted on the row's absence rather than on each field, deliberately: a
    // field added later is covered by this test without anybody remembering to
    // extend it, which is the property the shape was changed for.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);

    world.queue(Command::PickUpItem {
        connection,
        serial: RawSerial(item_serial.raw()),
        amount: 1,
    });
    world.tick(now);
    assert!(
        world.state.held_of(connection).is_some(),
        "the item is on the cursor, so the row has something in flight on it"
    );

    world.queue(Command::Disconnect { connection });
    world.tick(now);

    assert!(
        world.state.connection(connection).is_none(),
        "the row is gone, and with it every field it carried"
    );
    // The cursor is the one thing that could not simply cease to exist: an item on
    // it is off the ground and in no container, so dropping the row without
    // putting it back would delete it.
    assert!(
        world.state.registry.has::<Position>(item),
        "and the item it was dragging is back on the ground"
    );
}

#[test]
fn a_targeting_cursor_belongs_to_the_screen_it_is_drawn_on() {
    // It used to be keyed by the *mobile*, and `disconnect` swept it there by
    // name — one line for one map, which is the shape S7 exists to delete. Every
    // site that raises a cursor already refused to raise one for a mobile with no
    // client ("a creature has no cursor to raise"), so the connection was always
    // what it was really about; keying it by the entity only meant the invariant
    // had to be restated at each of the six of them.
    let now = Instant::now();
    let mut world = world();
    let connection = enter_gm(&mut world, now);
    let entity = world.state.players[&connection];

    gm_say(&mut world, connection, ".tele", now);
    assert!(
        world.state.has_target(entity),
        "the staff command put a cursor up"
    );

    world.queue(Command::Disconnect { connection });
    world.tick(now);

    assert_eq!(
        world
            .state
            .connections
            .values()
            .filter(|row| row.pending_target.is_some())
            .count(),
        0,
        "and it went with the screen it was drawn on"
    );
}

#[test]
fn a_second_hand_off_keeps_what_the_connection_was_doing() {
    // `attach` runs twice on an ordinary login — once when the login conversation
    // ends, once when a character enters — and it used to write a fresh row each
    // time. That was harmless while the row held only the identity, all three
    // fields being read off the same auth key. It stopped being harmless the
    // moment the row carried the cursor: a re-write would have left the dragged
    // item in limbo, off the ground and in no container, with nothing left
    // pointing at it.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let here = Point::new(START.x, START.y, 0);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);

    world.queue(Command::PickUpItem {
        connection,
        serial: RawSerial(item_serial.raw()),
        amount: 1,
    });
    world.tick(now);
    let held = world.state.held_of(connection).expect("on the cursor");

    world.queue(Command::Authenticated {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        access: AccessLevel::Player,
    });
    world.tick(now);

    assert_eq!(
        world.state.held_of(connection).map(|now| now.entity),
        Some(held.entity),
        "the hand-off said who the connection is, not what it has stopped doing"
    );
}

#[test]
fn disconnecting_a_connection_that_never_entered_is_harmless() {
    let mut world = world();
    world.queue(Command::Disconnect {
        connection: connection(),
    });
    world.tick(Instant::now());
}

#[test]
fn a_command_queued_during_a_tick_waits_for_the_next_one() {
    // The inbox is taken whole. Otherwise a system that queues work could
    // starve the loop, and a tick's length would depend on what happened in
    // it — which is the end of a fixed timestep.
    let mut world = world();
    let now = Instant::now();
    world.tick(now);
    let before = world.ticks();

    world.queue(Command::Enter(Entering {
        connection: connection(),
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("a".to_owned()),
        access:     AccessLevel::Player,
        character:  Character::fresh(Facet(0)),
    }));
    assert_eq!(world.player_count(), 0);
    world.tick(now);
    assert_eq!(world.ticks(), before + 1);
    assert_eq!(world.player_count(), 1);
}

#[test]
fn an_entry_that_does_not_happen_says_so() {
    // The obligation `World::enter` is a wrapper for. A connection that asked to
    // enter and did not is left in the binary's `Entering` phase, and only a
    // `PlayerRefused` moves it out of one — so an entry that fails quietly does
    // not fail quietly at all: it hangs the client on "logging into shard" with
    // nothing on the shard to say why.
    //
    // Entering twice on the same connection is the reachable one of the three
    // refusals; the other two need an exhausted serial pool or a serial the
    // registry already holds. What this pins is the wiring — that a `return Err`
    // in `try_enter` becomes an event — not the arithmetic of any one reason.
    let mut world = world();
    let now = Instant::now();
    let connection = enter(&mut world, now);

    let mut refused: Cursor<PlayerRefused> = world.bus().cursor();
    enter_as(&mut world, connection, now);

    let refusals: Vec<_> = world.bus().read(&mut refused).collect();
    assert_eq!(refusals.len(), 1, "the second entry is refused, and out loud");
    assert_eq!(refusals[0].connection, connection);
    assert_eq!(refusals[0].reason, RefusedEntry::AlreadyInWorld);
    assert_eq!(world.player_count(), 1, "and the first one is untouched");
}

#[test]
fn an_empty_tick_is_cheap_and_harmless() {
    let mut world = world();
    let now = Instant::now();
    for _ in 0..1000 {
        world.tick(now);
    }
    assert_eq!(world.ticks(), openshard_state::WorldTick::from_raw(1000));
    assert!(world.registry().is_empty());
}

#[test]
fn a_reader_that_polls_once_a_tick_never_misses_an_event() {
    // The property that matters, and the reason the bus is double-buffered.
    // A system reading once per tick sees everything, whatever order the
    // systems ran in — including one that polled *before* the emitter within
    // the same tick, which is what this simulates: the cursor is taken before
    // the tick that emits.
    let mut world = world();
    let now = Instant::now();
    let mut entered: Cursor<PlayerEntered> = world.bus().cursor();

    enter(&mut world, now);
    assert_eq!(world.bus().read(&mut entered).count(), 1);
}

#[test]
fn an_event_is_gone_a_tick_after_the_one_that_emitted_it() {
    // The lifetime, stated as it actually is. `tick` calls `bus.update()` at
    // its end, so the emitting tick already spends one of the event's two
    // buffers: it is readable after that tick, and gone after the next.
    //
    // That is not a bug, and the guarantee still holds — a reader polling
    // once per tick has a full tick to see it. But "events live two ticks"
    // is off by one if you count from outside, and this is where you find
    // that out.
    let mut world = world();
    let now = Instant::now();
    enter(&mut world, now);

    let mut after_emit: Cursor<PlayerEntered> = world.bus().cursor();
    assert_eq!(
        world.bus().read(&mut after_emit).count(),
        1,
        "readable after the tick that emitted it"
    );

    world.tick(now);
    let mut a_tick_later: Cursor<PlayerEntered> = world.bus().cursor();
    assert_eq!(
        world.bus().read(&mut a_tick_later).count(),
        0,
        "and gone after the next"
    );
}

#[test]
fn the_tick_interval_is_not_a_protocol_constant() {
    // 40Hz is ours to change. The client neither knows nor cares; it only
    // sees acks. Worth stating because the 200ms walk interval *is* the
    // client's, and the two are easy to confuse.
    assert_eq!(TICK_INTERVAL.as_millis(), 25);
    assert!(TICK_INTERVAL < WALK_INTERVAL, "a step must not span two ticks");
}

/// Decorate one door and return its entity and wire serial. The door sits at
/// `at` closed, and its open leaf swings a tile aside like the metal doors do.
fn place_door(world: &mut World, at: Point, now: Instant) -> (EntityId, Serial) {
    world.queue(Command::Decorate {
        facet:      Facet(0),
        statics:    Vec::new(),
        doors:      vec![DecorDoor {
            lock:     None,
            closed:   openshard_protocol::wire::Graphic(0x0675),
            open:     openshard_protocol::wire::Graphic(0x0676),
            offset_x: -1,
            offset_y: 1,
            position: at,
        }],
        containers: Vec::new(),
    });
    world.tick(now);
    let door = world.registry().query::<Door>().next().unwrap().0;
    let serial = world.registry().serial_of(door).unwrap();
    (door, serial)
}

#[test]
fn a_closed_door_blocks_a_walk() {
    // The doorway tile is open ground as far as the map knows — that is how the
    // doorway was chosen — so the closed door entity is the only thing standing
    // in the way. Walking into it must be refused, or every door is theatre.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let entity = world.state.players[&gm];
    place_door(&mut world, Point::new(START.x, START.y + 1, 0), now);

    let mut refused: Cursor<StepRefused> = world.bus().cursor();
    world.queue(Command::Walk {
        connection: gm,
        request:    walk(0, Direction::South),
    });
    world.tick(now);
    assert_eq!(
        world.bus().read(&mut refused).count(),
        1,
        "the walk into the shut door is refused"
    );
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        Point::new(START.x, START.y, Z_WITHOUT_A_MAP),
        "and nobody moved"
    );
}

/// **The other half of being dead, beside walking through bodies.**
///
/// ServUO's `MovementImpl.Check` sets `ignoreDoors` for anything not alive
/// (`Scripts/Services/Pathing/Movement.cs:173`) and `IsOk` then steps past
/// anything carrying `TileFlag.Door`. Without it a player who died behind a
/// shut door is sealed in: the living cannot see a ghost, so nobody is coming
/// to open it, and the ghost has no hands to open it with either.
///
/// The same player, alive and then dead, on the same leaf — because the rule is
/// a fact about the walker and not about the door.
#[test]
fn a_ghost_walks_through_a_shut_door_and_the_living_do_not() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let doorway = Point::new(START.x, START.y + 1, Z_WITHOUT_A_MAP);
    place_door(&mut world, doorway, now);

    let mut refused: Cursor<StepRefused> = world.bus().cursor();
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(now);
    assert_eq!(
        world.bus().read(&mut refused).count(),
        1,
        "alive, the leaf is in the way"
    );
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        Point::new(START.x, START.y, Z_WITHOUT_A_MAP),
    );

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);
    assert!(
        world.state.registry.has::<Ghost>(entity),
        "the player is a ghost now"
    );

    // The refusal reset the sequence, so the ghost's first step is a fresh one.
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::South),
    });
    world.tick(now);
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        doorway,
        "the ghost drifts through the shut leaf"
    );
    assert!(
        !world.registry().query::<Door>().next().unwrap().1.is_open,
        "and the door is still shut behind it — nothing was opened to get through"
    );
}

/// A ghost has no hands, so the leaf it walks through is one it cannot work.
///
/// ServUO gates every double-click on `CheckAlive` before the item is asked
/// (`Server/Mobile.cs:4402`). Here it matters more than as a refusal for its own
/// sake: the dead are invisible to the living, so a ghost that could swing a
/// door would be opening the town's shopfronts with nobody to blame.
#[test]
fn a_ghost_cannot_work_a_door() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let serial = serial_of(&world, connection);
    let (door, door_serial) = place_door(&mut world, Point::new(START.x + 1, START.y, 0), now);

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);
    let _ = packets_for(&mut world, connection); // drain the death burst

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(door_serial.raw())),
    });
    world.tick(now);
    assert!(
        !world.registry().get::<Door>(door).unwrap().is_open,
        "the door did not budge for the dead"
    );
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|p| { (p[0] == 0x1C || p[0] == 0xAE) && String::from_utf8_lossy(p).contains("I am dead") }),
        "and it says why"
    );
}

#[test]
fn an_opened_door_lets_a_step_through_and_blocks_again_when_it_shuts() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let entity = world.state.players[&gm];
    let serial = world.registry().serial_of(entity).unwrap();
    let at = Point::new(START.x + 1, START.y, 0);
    let (_door, door_serial) = place_door(&mut world, at, now);

    // Shut, it refuses the server-authoritative step an NPC would take — the
    // same gate a creature's chase goes through.
    let mut refused: Cursor<StepRefused> = world.bus().cursor();
    for _ in 0..2 {
        // Twice: the first may only turn to face east.
        world.queue(Command::Step {
            serial,
            direction: Direction::East.to_bits(),
        });
        world.tick(now);
    }
    assert!(
        world.bus().read(&mut refused).count() >= 1,
        "a step into a shut door is refused"
    );
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        Point::new(START.x, START.y, Z_WITHOUT_A_MAP)
    );

    // Open, the doorway is a doorway again.
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(door_serial.raw())),
    });
    world.tick(now);
    world.queue(Command::Step {
        serial,
        direction: Direction::East.to_bits(),
    });
    world.tick(now);
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        at,
        "an open door is walked through"
    );

    // And when it swings shut on its own, the tile seals behind it.
    teleport(&mut world, gm, Point::new(START.x, START.y, 0));
    let close_at = world.registry().query::<Door>().next().unwrap().1.close_at;
    let mut later = now;
    while world.state.ticks < close_at {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        !world.registry().query::<Door>().next().unwrap().1.is_open,
        "the door swung shut on its own"
    );
    let mut refused: Cursor<StepRefused> = world.bus().cursor();
    world.queue(Command::Step {
        serial,
        direction: Direction::East.to_bits(),
    });
    world.tick(later);
    assert_eq!(
        world.bus().read(&mut refused).count(),
        1,
        "the auto-closed door blocks again"
    );
}

#[test]
fn a_creature_does_not_notice_prey_through_a_shut_door() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let player_serial = world.registry().serial_of(player).unwrap();

    // A shut door directly south of the player, and a hungry creature beyond
    // it: the only sight line runs through the door.
    let (_door, door_serial) = place_door(&mut world, Point::new(START.x, START.y + 1, 0), now);
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(5),
        damage:      5,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(5),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x, START.y + 2, 0),
        facet:       Facet(0),
        name:        None,
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let creature = world
        .state
        .registry
        .query::<Brain>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a creature with a brain");

    // Many beats pass; the door hides the player the whole time.
    for _ in 0..(AI_THINK_TICKS * 3) {
        world.tick(now);
    }
    assert!(
        world
            .registry()
            .get::<Combat>(creature)
            .and_then(|combat| combat.target())
            .is_none(),
        "a shut door hides prey — no aggro through it"
    );

    // Open the door and the next beat notices.
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(door_serial.raw())),
    });
    world.tick(now);
    for _ in 0..(AI_THINK_TICKS + 1) {
        world.tick(now);
    }
    assert_eq!(
        world
            .registry()
            .get::<Combat>(creature)
            .and_then(|combat| combat.target()),
        Some(player_serial),
        "an open doorway is a sight line"
    );
}

/// Spawn a creature with a brain, returning its entity. `body` decides whether
/// it knows door handles (0x0190 human does; 0x00D1 goat does not).
fn spawn_brained(world: &mut World, body: u16, at: Point, sight: u8, now: Instant) -> EntityId {
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(body),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(5),
        damage:      5,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(sight),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    at,
        facet:       Facet(0),
        name:        None,
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    world
        .state
        .registry
        .query::<Brain>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a creature with a brain")
}

/// Ring a tile with crate obstacles, leaving sight clear — a fence, to a chase.
fn fence_around(world: &mut World, center: Point) {
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let crate_entity = world.state.registry.spawn();
            world.state.facet_state_mut(Facet(0)).block(
                (i32::from(center.x) + dx) as u16,
                (i32::from(center.y) + dy) as u16,
                crate_entity,
                openshard_map::overlay::Cover::blocking(0, openshard_state::DOOR_HEIGHT),
            );
        }
    }
}

#[test]
fn an_unreachable_quarry_is_given_up_not_wall_humped() {
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);
    // The player fenced in on all eight sides: visible, unreachable.
    fence_around(&mut world, Point::new(START.x, START.y, 0));
    let creature = spawn_brained(&mut world, 0x00D1, Point::new(START.x, START.y + 4, 0), 8, now);

    // Let it notice, try, and conclude.
    for _ in 0..(AI_THINK_TICKS * 4) {
        world.tick(now);
    }
    let brain = *world.registry().get::<Brain>(creature).unwrap();
    assert!(
        brain.guard_until > world.state.ticks,
        "no way through the fence: the creature stands guard instead of shuffling"
    );
    assert!(
        world
            .registry()
            .get::<Combat>(creature)
            .and_then(|combat| combat.target())
            .is_none(),
        "and the doomed chase was dropped"
    );
    // While guarding it holds its ground.
    let held = world.registry().get::<Position>(creature).unwrap().0;
    for _ in 0..(AI_THINK_TICKS * 3) {
        world.tick(now);
    }
    assert_eq!(
        world.registry().get::<Position>(creature).unwrap().0,
        held,
        "a guard stands watch, it does not pace into the fence"
    );
}

#[test]
fn a_chase_rounds_a_wall_of_crates() {
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);
    let player_at = Point::new(START.x, START.y, 0);
    // A five-tile wall between quarry and creature, open at both ends.
    for dx in -2i32..=2 {
        let crate_entity = world.state.registry.spawn();
        world.state.facet_state_mut(Facet(0)).block(
            (i32::from(player_at.x) + dx) as u16,
            player_at.y + 2,
            crate_entity,
            openshard_map::overlay::Cover::blocking(0, openshard_state::DOOR_HEIGHT),
        );
    }
    let creature = spawn_brained(&mut world, 0x00D1, Point::new(START.x, START.y + 4, 0), 10, now);

    // Enough beats to notice, plan, and walk around either end.
    let mut later = now;
    for _ in 0..(AI_THINK_TICKS * 30) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    let reached = world.registry().get::<Position>(creature).unwrap().0;
    assert!(
        distance(reached, player_at) <= openshard_combat::MELEE_RANGE,
        "the creature went around the wall and reached its quarry (ended at {reached:?})"
    );
}

/// Stand an inert body at `at`.
///
/// A bystander and not a creature: no [`Brain`], so nothing in the tick moves
/// it, and no `Client`, so no aggressive creature picks a fight with it — the
/// prey search is players only. What it has is the two things that make a
/// mobile an obstacle: the `Body` that says it is one, and the **sector row**,
/// which is what `crowd_near` actually reads. The tick writes that row beside
/// `Position` on every step; a body with a position and no row is a body
/// nothing can see.
fn a_bystander_at(world: &mut World, at: Point) -> EntityId {
    let entity = world.state.registry.spawn();
    world.state.registry.insert(
        entity,
        openshard_state::components::Body {
            id:  Graphic(0x0190),
            hue: Hue(0),
        },
    );
    world.state.registry.insert(entity, Position(at));
    world.state.registry.insert(entity, Facet(0));
    world.state.place_mobile(Facet(0), entity, at);
    entity
}

/// **The staff exemption reaches the client, or only half of it is real.**
///
/// The shard lets staff walk through bodies; the client keeps its own copy of
/// the rule and applies it to what it predicts, so without `0x10` in the flag
/// byte a game master's step is allowed at one end and refused at the other —
/// which a player experiences as a rubber-band, not as a permission.
///
/// Asserted on the `0x78` this shard would build rather than on the bytes: the
/// bit's *value* is `StatusFlags`' own test, and what is worth pinning here is
/// that the shard's `Staff` and the wire's `IGNORE_MOBILES` are wired to each
/// other at all.
#[test]
fn a_game_master_is_drawn_as_walking_through_bodies() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter_gm(&mut world, now);
    let gm = *world.state.players.get(&connection).expect("the GM entered");

    let seen = world
        .state
        .mobile_incoming(gm, gm)
        .expect("a game master is a drawable mobile");
    assert!(
        seen.flags
            .has(openshard_protocol::mobile::StatusFlags::IGNORE_MOBILES),
        "the client will refuse to predict a step the shard allows"
    );

    // And a body that has put the flag down is drawn like anybody else — the
    // permission is `Staff`, not the account's access level.
    world
        .state
        .registry
        .remove::<openshard_state::components::Staff>(gm);
    let mortal = world.state.mobile_incoming(gm, gm).expect("still a mobile");
    assert!(
        !mortal
            .flags
            .has(openshard_protocol::mobile::StatusFlags::IGNORE_MOBILES),
        "a game master playing by the rules is drawn playing by them"
    );
}

/// Where the flag byte sits in a `0x20` — after the id, the serial, the body, a
/// pad byte and the hue. `PlayerUpdate::encode_body` is the layout, and the
/// facing at 17 is what the turn tests above already read by hand.
const PLAYER_UPDATE_FLAGS: usize = 10;

/// The exemption a client needs is the one about **its own body**, and that one
/// travels in the `0x20`.
///
/// The test above pins the `0x78`, which is how a client learns about *somebody
/// else*. A client only ever predicts its own step, so the packet that decides
/// whether a game master rubber-bands is this one — and all three of its senders
/// used to write `StatusFlags::NONE` into it. The `0x78` a game master gets about
/// itself on entering does carry the bit, which is what made the gap survive:
/// it is true until the first step or relocation sends a `0x20` over it.
#[test]
fn a_game_master_s_own_0x20_says_it_walks_through_bodies() {
    let now = Instant::now();
    let mut world = world();

    let ordinary = enter(&mut world, now);
    let theirs = packets_for(&mut world, ordinary);
    let theirs = theirs
        .iter()
        .find(|packet| packet.first() == Some(&0x20))
        .expect("entering is told where it stands");
    assert!(
        !openshard_protocol::mobile::StatusFlags(theirs[PLAYER_UPDATE_FLAGS])
            .has(openshard_protocol::mobile::StatusFlags::IGNORE_MOBILES),
        "an ordinary player was told it walks through bodies, and the shard will refuse the steps it then predicts"
    );

    let connection = enter_gm(&mut world, now);
    let mine = packets_for(&mut world, connection);
    let mine = mine
        .iter()
        .find(|packet| packet.first() == Some(&0x20))
        .expect("a game master is told where it stands too");
    assert!(
        openshard_protocol::mobile::StatusFlags(mine[PLAYER_UPDATE_FLAGS])
            .has(openshard_protocol::mobile::StatusFlags::IGNORE_MOBILES),
        "a game master's own client was never told, and it is the only client that predicts this body's steps"
    );
}

/// **And a ghost is told the same thing, by the packet death sends.**
///
/// The dead are stopped by nobody — `walks_through_bodies`' other half — so a
/// ghost's walk home passes through whatever is standing in it. A client that
/// was not told keeps applying the rule to a body the shard has exempted, and
/// refuses to predict a step nobody would have refused: the walk stalls in front
/// of a bystander it cannot even see, since the living are drawn to a ghost and
/// a ghost is drawn to almost nobody.
#[test]
fn a_ghost_s_own_0x20_says_it_walks_through_bodies() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let serial = serial_of(&world, connection);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);

    let dying = packets_for(&mut world, connection);
    let update = dying
        .iter()
        .find(|packet| packet.first() == Some(&0x20))
        .expect("death redraws the player's own body");
    assert!(
        openshard_protocol::mobile::StatusFlags(update[PLAYER_UPDATE_FLAGS])
            .has(openshard_protocol::mobile::StatusFlags::IGNORE_MOBILES),
        "a ghost was left believing the living are in its way"
    );
}

/// **A chase goes round a crowd, and it used to walk into one for ever.**
///
/// `a_chase_rounds_a_wall_of_crates` with the crates replaced by people, which
/// before this was the whole difference between a chase that worked and one
/// that did not. A crate is in the overlay, so the route was planned around it;
/// a body was in neither the overlay nor the plan, so the route went straight
/// through the line, `World::step` refused the first step against a
/// `mobile_occupies` check bolted on after the fact, and the next beat decided
/// the same direction again. The creature butted into the same shoulder until
/// something else moved.
#[test]
fn a_chase_rounds_a_line_of_bystanders() {
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);
    let player_at = Point::new(START.x, START.y, 0);
    // Five people standing shoulder to shoulder between quarry and creature,
    // open at both ends.
    let line: Vec<Point> = (-2i32..=2)
        .map(|dx| Point::new((i32::from(player_at.x) + dx) as u16, player_at.y + 2, 0))
        .collect();
    for at in &line {
        a_bystander_at(&mut world, *at);
    }
    let creature = spawn_brained(&mut world, 0x00D1, Point::new(START.x, START.y + 4, 0), 10, now);

    let mut later = now;
    let mut walked = Vec::new();
    for _ in 0..(AI_THINK_TICKS * 30) {
        later += TICK_INTERVAL;
        world.tick(later);
        walked.push(world.registry().get::<Position>(creature).unwrap().0);
    }
    let reached = *walked.last().unwrap();
    assert!(
        distance(reached, player_at) <= openshard_combat::MELEE_RANGE,
        "the creature went around the line of people and reached its quarry (ended at {reached:?})"
    );
    // **Round, and not through.** Reaching the quarry is half the assertion: a
    // shard that had forgotten about bodies altogether would reach it too, by
    // walking over five of them. The step rule and the plan are one rule now, so
    // neither half can pass on its own.
    assert!(
        !walked.iter().any(|at| line.contains(at)),
        "the creature walked over somebody on its way: {walked:?}"
    );
}

/// Where a creature standing two tiles off both axes from its quarry first
/// steps, with `blockers` in its way.
///
/// The quarry is the GM at `START` and the creature comes from the south-east,
/// so `direction_toward` says north-west and the straight step at it is the
/// diagonal — which is the step `probe` answers for. `None` when it never
/// moved at all.
fn first_chase_step(blockers: &[Point]) -> Option<Point> {
    first_chase_step_among(blockers, &[])
}

/// The same, with `bystanders` standing where they stand.
///
/// The two lists are the two indexes a step reads — the overlay a crate is in,
/// and the sector grid a body is in — and the point of asking the same question
/// of both is that the answer is now the same rule.
fn first_chase_step_among(blockers: &[Point], bystanders: &[Point]) -> Option<Point> {
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);
    let start = Point::new(START.x + 2, START.y + 2, 0);
    for blocker in blockers {
        let crate_entity = world.state.registry.spawn();
        world.state.facet_state_mut(Facet(0)).block(
            blocker.x,
            blocker.y,
            crate_entity,
            openshard_map::overlay::Cover::blocking(0, openshard_state::DOOR_HEIGHT),
        );
    }
    for bystander in bystanders {
        a_bystander_at(&mut world, *bystander);
    }
    let creature = spawn_brained(&mut world, 0x00D1, start, 8, now);

    let mut later = now;
    for _ in 0..(AI_THINK_TICKS * 8) {
        later += TICK_INTERVAL;
        world.tick(later);
        let at = world.registry().get::<Position>(creature).unwrap().0;
        if at != start {
            return Some(at);
        }
    }
    None
}

#[test]
fn a_chase_does_not_cut_a_corner() {
    // `probe` is what a chase asks whether the way to its quarry is open, and
    // it used to ask `can_step` — one landing, which has nothing to say about a
    // diagonal's flanks. So a creature walking straight at its quarry cut the
    // corner its own `find_path` refuses to plan through.
    let cut = Point::new(START.x + 1, START.y + 1, 0);
    // The crate on the diagonal's northern flank. Its own tile is not on the
    // way anywhere: it is a flank and nothing else.
    let flank = Point::new(START.x + 2, START.y + 1, 0);

    let stepped = first_chase_step(&[flank]);
    assert_ne!(stepped, Some(cut), "the straight step clipped the crate's corner");
    assert!(
        stepped.is_some(),
        "and the rule refused a step rather than the whole chase"
    );

    // The control: the same chase with nothing flanking it takes the diagonal,
    // so what the assertion above sees is the corner rule and not the ground.
    assert_eq!(
        first_chase_step(&[]),
        Some(cut),
        "an unflanked diagonal is the straight step at the quarry"
    );
}

/// The corner rule reads a body the way it reads a crate, because both are
/// read as one thing: a *landing*, which is what each of `steps_out_of`'s eight
/// answers is.
///
/// The same two tiles as `a_chase_does_not_cut_a_corner`, with people standing
/// on them instead of crates. ServUO checks its diagonal's flanks for mobiles
/// too (`Scripts/Services/Pathing/Movement.cs:552`) — for uncontrolled
/// creatures only, where this engine gives everybody the strict reading, as it
/// does with the corner rule itself.
#[test]
fn a_chase_does_not_cut_a_corner_past_a_bystander() {
    let cut = Point::new(START.x + 1, START.y + 1, 0);
    let flank = Point::new(START.x + 2, START.y + 1, 0);

    let stepped = first_chase_step_among(&[], &[flank]);
    assert_ne!(stepped, Some(cut), "the straight step clipped a person's corner");
    assert!(
        stepped.is_some(),
        "and the rule refused a step rather than the whole chase"
    );

    // And the tile itself: somebody standing on the diagonal is not somewhere to
    // step, which is the destination half of the same rule.
    assert_ne!(
        first_chase_step_among(&[], &[cut]),
        Some(cut),
        "the creature stepped onto the tile somebody was standing on"
    );
}

/// Two corridors that meet a whole map away from where a walk between them
/// starts, with a walkway of statics over the northern one.
///
/// The shape is what makes N7's question askable. The way through is eighty-odd
/// tiles *away from* the goal and eighty back, so an exact search runs out of
/// budget long before it finds it — while the goal is thirty-two tiles off and
/// plainly walkable. A body handed the straight-line direction instead walks
/// south until the divider and stands there.
///
/// The walkway is the other half: five units up, laid on ground that has no
/// room under it, so standing on it is standing somewhere `ground_z` does not
/// report. That is the raised origin
/// [`docs/world/evidence/2026-08-25-the-span-layer.md`](../../../../../docs/world/evidence/2026-08-25-the-span-layer.md)'s
/// N7 asks for by name — the one that would have passed for the wrong reason
/// before N4 gave the graph a node per place rather than per tile.
fn two_corridors() -> Scene {
    let mut scene = Scene::flat_holding(95, 63, 0);
    for x in 0..=84u16 {
        scene.wall(x, 32, 0, 20);
    }
    for x in 0..=15u16 {
        scene.floor(x, 16, 0, 5);
    }
    scene
}

/// A shard standing on `scene`, with the graph baked over it or without one.
///
/// The tile table goes in before the facet does, because a span bake is a
/// statement about both — `with_tiles` rebakes what is already loaded, and
/// `with_facet` bakes against what the world is holding.
fn shard_over(scene: Scene, coarse: Option<openshard_movement::NavigationGraph>) -> World {
    let (map, tiles) = scene.into_shard(Facet(0));
    world()
        .with_tiles(tiles, openshard_uofiles::multi::Multis::default())
        .with_facet(Facet(0), map, coarse, FacetRules::classic(Facet(0)), None)
}

/// How many beats the walk below is given. The route is 168 steps; this is
/// slack, not the answer — a test that pinned the length would fail on a
/// corridor the router is free to choose differently.
const MOST_BEATS: usize = 300;

/// Walk from `from` toward `to` the way the tick does: the brain decides, the
/// step rule approves, one step per beat.
///
/// Both halves are the shard's own — [`ai::step_toward`] is what a chase asks
/// and `step_allowed` is what the world would allow — so this cannot walk a
/// step the shard would refuse. Stops where a body would: at the goal, or on
/// the first direction it may not take, which is a creature standing at a wall.
fn walk_toward(world: &mut World, from: Point, to: Point) -> Point {
    // A plan is decided for *somebody*: `step_toward` reads who is asking so it
    // can leave them out of their own crowd, and a ghost or a game master out of
    // everybody's. Nothing else about this walker matters — the scene has no
    // other mobiles in it, so the crowd is empty however the question is asked.
    let mover = world.state.registry.spawn();
    let footing = world.state.footing(Facet(0), Doors::AsTheyStand);
    let mut at = from;
    for _ in 0..MOST_BEATS {
        if at == to {
            break;
        }
        let Some(direction) = ai::step_toward(&world.state, mover, Facet(0), at, to, Doors::AsTheyStand)
        else {
            break;
        };
        let Some(next) = openshard_movement::step_allowed(&footing, at, direction) else {
            break;
        };
        at = next;
    }
    at
}

/// The shard walks a creature a route the exact search cannot see, and the
/// baked graph is what carries it.
///
/// **[`docs/world/evidence/2026-08-25-the-span-layer.md`](../../../../../docs/world/evidence/2026-08-25-the-span-layer.md)'s
/// N7**, and the first thing on the shard to read `FacetState::coarse`. The
/// artifact has been loaded, validated and paid for since the terrain-seam work;
/// what it had was one test for its only reader. Server AI planned with flat
/// `find_path` at [`ai::PATH_BUDGET`], so a creature could not route across a
/// town while the answer sat in the facet beside it.
///
/// The two origins are the two halves of the claim. The flat one says the
/// fall-back happens at all; the raised one says it happens *from a place the
/// land does not report*, which is the half that would have passed for the
/// wrong reason before N4 — a graph sampling `ground_z` would have joined the
/// endpoint at the ground under the walkway and answered about a body that is
/// not there.
///
/// The facet without a graph is the control, and it is what the shard was: the
/// same ground, the same creature, the same budget, and a body that walks south
/// until the divider and stands there for the rest of the walk.
#[test]
fn a_creature_routes_past_its_exact_budget_over_the_coarse_graph() {
    /// On the plain, south of the walkway.
    const FLAT: Point = Point::new(2, 20, 0);
    /// On the walkway, five units up, over ground nothing can stand on.
    const RAISED: Point = Point::new(2, 16, 5);
    /// The far corridor, thirty-two tiles south and a map's width away by foot.
    const GOAL: Point = Point::new(2, 48, 0);
    /// A walkway tile that is not the origin, so the flood says something about
    /// the walkway rather than about where the body was put.
    const ALONG: (u16, u16) = (10, 16);

    let scene = two_corridors();
    // The flood is the oracle: whatever a search says about finding the way,
    // this is ground a body can walk.
    for from in [FLAT, RAISED] {
        assert_eq!(
            scene.reachable(from).get(&(GOAL.x, GOAL.y)),
            Some(&GOAL.z),
            "the goal is walkable ground from {from:?}"
        );
    }
    // And the raised origin is raised: the walkway is a surface only the statics
    // put there, with no room for a body on the ground beneath it.
    assert_eq!(
        scene.reachable(RAISED).get(&ALONG),
        Some(&RAISED.z),
        "the walkway is walked at its own height"
    );
    assert_eq!(
        scene.reachable(FLAT).get(&ALONG),
        None,
        "and the plain cannot reach it — neither onto it nor under it"
    );

    // What the shard had before this node: an exact search that refuses, and
    // refuses for want of *budget* rather than for want of a way.
    for from in [FLAT, RAISED] {
        let search = openshard_movement::search_path(
            &scene.footing(),
            from,
            GOAL,
            ai::PATH_BUDGET,
            openshard_movement::Weight::EXACT,
        );
        assert!(!search.arrived, "flat A* must not find this route from {from:?}");
        assert_eq!(
            search.exit,
            openshard_movement::SearchExit::Budget,
            "and it must be the budget that stopped it, not a wall"
        );
    }

    let graph = openshard_movement::NavigationGraph::build(&scene.footing(), 96, 64)
        .expect("a 96x64 facet has a graph");
    let mut routed = shard_over(two_corridors(), Some(graph));
    let mut blind = shard_over(two_corridors(), None);

    for from in [FLAT, RAISED] {
        assert_eq!(
            walk_toward(&mut routed, from, GOAL),
            GOAL,
            "the graph carried the creature from {from:?} to the far corridor"
        );
        let stood = walk_toward(&mut blind, from, GOAL);
        assert_ne!(stood, GOAL, "a facet with no graph cannot plan this route");
        assert_eq!(
            stood,
            Point::new(2, 31, 0),
            "it walks the straight line south and stands at the divider"
        );
    }
}

/// A facet split by one wall with a single doorway in it, far from either
/// side's origin.
///
/// The way through is a route only the graph can find — the exact search runs
/// out of budget long before it — and one shut door on that tile makes it no
/// route at all, while the bare map the graph is baked over still has it. So the
/// corridor is proposed and the live layer refuses every hop of it, which is the
/// refusal that costs the most: the whole endpoint join at both ends, for
/// nothing.
fn one_doorway() -> Scene {
    let mut scene = Scene::flat_holding(95, 63, 0);
    for x in 0..=95u16 {
        if x != DOORWAY.0 {
            scene.wall(x, DOORWAY.1, 0, 20);
        }
    }
    scene
}

/// The one tile of the divider that is not a wall.
const DOORWAY: (u16, u16) = (90, 32);

/// A refused long route is remembered, and the graph is not asked again until
/// it lapses.
///
/// **[`docs/world/evidence/2026-08-25-the-span-layer.md`](../../../../../docs/world/evidence/2026-08-25-the-span-layer.md)'s
/// N7 finding.** `ai::step_toward` is a pure function of the world, so a body
/// following something it cannot reach paid the whole endpoint join on *every*
/// beat and had nowhere to write down that it had already asked. A chase does
/// not — `give_up` guards it for ten seconds — and a pet, a townsperson walking
/// home and an escortable all did.
///
/// The memory is deliberately blind for as long as it stands, and that blindness
/// is what this asserts, because it is the only thing about it a test can see: a
/// route that opens while it holds is not taken until [`ai::REFUSAL_TICKS`] have
/// passed. Without the memory the third step below would already be the
/// corridor's, and it is the straight line instead.
#[test]
fn a_refused_long_route_is_remembered_until_it_lapses() {
    /// North of the divider, at the far end from the doorway.
    const FROM: Point = Point::new(2, 20, 0);
    /// South of it, straight ahead through the wall.
    const GOAL: Point = Point::new(2, 48, 0);

    let now = Instant::now();
    let straight = openshard_movement::direction_toward(FROM, GOAL);
    let scene = one_doorway();
    // The oracle: this ground joins, and the exact search cannot say so.
    assert_eq!(
        scene.reachable(FROM).get(&(GOAL.x, GOAL.y)),
        Some(&GOAL.z),
        "the goal is walkable ground once the door is open"
    );
    let search = openshard_movement::search_path(
        &scene.footing(),
        FROM,
        GOAL,
        ai::PATH_BUDGET,
        openshard_movement::Weight::EXACT,
    );
    assert!(!search.arrived, "flat A* must not find this route");

    let graph = openshard_movement::NavigationGraph::build(&scene.footing(), 96, 64)
        .expect("a 96x64 facet has a graph");
    let mut world = shard_over(one_doorway(), Some(graph));
    let (door, _) = place_door(&mut world, Point::new(DOORWAY.0, DOORWAY.1, 0), now);
    // Sight enough to be given a brain at all, and no prey on the facet to use
    // it on: what this body is here for is somewhere to write a refusal.
    let walker = spawn_brained(&mut world, 0x0190, FROM, 8, now);

    // Shut: the corridor is proposed over the bare map and refused hop by hop,
    // and the body walks the straight line at the wall.
    let refused = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        // A place, and the caller the memory was written for walks to one: this
        // is the townsperson whose post is on the far side of the wall.
        ai::Goal::Fixed,
    );
    assert_eq!(
        refused, straight,
        "a refused query falls back to the straight line"
    );
    let remembered = world
        .registry()
        .get::<RouteRefused>(walker)
        .copied()
        .expect("the refusal was written on the body");
    assert_eq!(remembered.goal, GOAL, "and it is about the goal that was refused");

    // The way opens, and the memory is what keeps the body from noticing.
    openshard_items::open_door(&mut world.state, door);
    let blind = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        ai::Goal::Fixed,
    );
    assert_eq!(
        blind, straight,
        "the graph is not asked again while the refusal stands"
    );

    // It lapses on its own beat, and the corridor answers.
    for _ in 0..=ai::REFUSAL_TICKS {
        world.tick(now);
    }
    let routed = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        ai::Goal::Fixed,
    );
    assert!(
        routed.is_some(),
        "the corridor answers once the memory has lapsed"
    );
    assert_ne!(
        routed, straight,
        "and it aims at the doorway rather than at the wall"
    );
    assert!(
        world.registry().get::<RouteRefused>(walker).is_none(),
        "a corridor that answered clears the memory"
    );
}

/// The gap in the divider a shut door stands in, close to the walker.
const NEAR_WAY: (u16, u16) = (46, 32);
/// And the one at the far end of it, which is only ever open ground.
const FAR_WAY: (u16, u16) = (90, 32);

/// A facet split by one wall with two ways through it: a far one that is open
/// ground, and a near one a shut door stands in.
///
/// The two are an oracle between them. With the door shut the only way round is
/// the far one and a route to it heads east; with it open the near one is a
/// third of the distance and the route heads the other way. So *which way a body
/// steps* says which plan it is walking, without the test having to know a
/// single step of either.
fn two_ways_through() -> Scene {
    let mut scene = Scene::flat_holding(95, 63, 0);
    for x in 0..=95u16 {
        if x != NEAR_WAY.0 && x != FAR_WAY.0 {
            scene.wall(x, NEAR_WAY.1, 0, 20);
        }
    }
    scene
}

/// A body walks the route it planned, and does not plan another until that one
/// goes stale.
///
/// **The exact half of the waste [`RouteRefused`] took out of the coarse half.**
/// A search that arrives hands back every step of the way there;
/// `step_body_toward` kept the first and dropped the rest, so a body walking
/// twenty tiles planned twenty routes and walked one step of each. It keeps the
/// route now — a [`Route`], the same component and the same repath cadence a
/// chase has always followed its own by.
///
/// **What a test can see of that is the blindness**, exactly as for the refusal
/// memory above: a better way that opens while a route stands is not taken until
/// the route lapses. The oracle is [`ai::step_toward`], which is the same
/// decision with nowhere to keep it — so it says what a body with no route would
/// do, on the same ground, in the same instant.
#[test]
fn a_planned_route_is_walked_rather_than_planned_again() {
    /// North of the divider, between the two ways through it.
    const FROM: Point = Point::new(48, 20, 0);
    /// South of it, straight ahead through the wall.
    const GOAL: Point = Point::new(48, 48, 0);

    let now = Instant::now();
    let scene = two_ways_through();
    // The flood is the oracle for the ground itself: both sides join, once round
    // the far end, and they join with the near door shut.
    assert_eq!(
        scene.reachable(FROM).get(&(GOAL.x, GOAL.y)),
        Some(&GOAL.z),
        "the goal is walkable ground the long way round"
    );

    let graph = openshard_movement::NavigationGraph::build(&scene.footing(), 96, 64)
        .expect("a 96x64 facet has a graph");
    let mut world = shard_over(two_ways_through(), Some(graph));
    let (door, _) = place_door(&mut world, Point::new(NEAR_WAY.0, NEAR_WAY.1, 0), now);
    let walker = spawn_brained(&mut world, 0x0190, FROM, 8, now);
    // Facing away from wherever it is about to go, so every beat below is a
    // *turn* and the body stands where it was put: a route that has not been
    // walked is still due from where it was planned, which is what makes the
    // three decisions here comparable at all.
    world
        .state
        .registry
        .insert(walker, Heading(Facing::walking(Direction::North)));

    // Shut: the way round is the far one, and a route to it is what gets kept.
    let planned = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        // The window is what this test is about, so the goal is one that
        // carries it: see `ai::Goal`, and the sibling test below for a place.
        ai::Goal::Moving,
    )
    .expect("there is a way round the far end");
    let route = world
        .registry()
        .get::<Route>(walker)
        .cloned()
        .expect("the route was written on the body");
    assert_eq!(
        route.steps.first().copied(),
        Some(planned),
        "the step taken is the route's own first"
    );
    assert!(
        route.steps.len() > 1,
        "a route worth keeping is more than the step it starts with"
    );
    assert_eq!(route.goal, GOAL, "and it is aimed at what was asked for");

    // The near way opens, and it is a third of the distance — but the body has
    // a route, and a body with a route does not ask.
    openshard_items::open_door(&mut world.state, door);
    let fresh = ai::step_toward(&world.state, walker, Facet(0), FROM, GOAL, Doors::AsTheyStand);
    assert_ne!(fresh, Some(planned), "the open door really is a different way");
    let walked = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        ai::Goal::Moving,
    );
    assert_eq!(walked, Some(planned), "the body walks the route it has");
    assert_eq!(
        world.registry().get::<Route>(walker).map(|kept| kept.planned_at),
        Some(route.planned_at),
        "and it planned nothing to do it"
    );

    // It lapses on its own beat, and the shortcut is taken.
    for _ in 0..=ai::REPATH_TICKS {
        world.tick(now);
    }
    let lapsed = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        ai::Goal::Moving,
    );
    assert_eq!(
        lapsed, fresh,
        "a lapsed route is planned again, and the near door is in the new one"
    );
}

/// The same ground and the same shortcut, walked to a *place*: nothing lapses,
/// and the body is still on the route it planned.
///
/// **The control for the test above, and the defect it was written for.** A
/// window is the only one of the four ways a route goes stale that is about the
/// clock rather than about the world, and what it buys is noticing a better way
/// — which is why the assertion here is the mirror image of the one above: the
/// near door opens, a whole [`ai::REPATH_TICKS`] passes, and the body walks on
/// round the far end because nobody asked it to look again.
///
/// A townsperson beats every `npc::BEAT_TICKS`, which is the same forty ticks,
/// so before [`ai::Goal`] its route lapsed between every pair of beats it was
/// ever read on and the cache could not help the caller it was written for.
#[test]
fn a_route_to_a_place_is_walked_past_the_window_that_would_have_lapsed() {
    /// North of the divider, between the two ways through it.
    const FROM: Point = Point::new(48, 20, 0);
    /// South of it, straight ahead through the wall.
    const GOAL: Point = Point::new(48, 48, 0);

    let now = Instant::now();
    let scene = two_ways_through();
    let graph = openshard_movement::NavigationGraph::build(&scene.footing(), 96, 64)
        .expect("a 96x64 facet has a graph");
    let mut world = shard_over(two_ways_through(), Some(graph));
    let (door, _) = place_door(&mut world, Point::new(NEAR_WAY.0, NEAR_WAY.1, 0), now);
    let walker = spawn_brained(&mut world, 0x0190, FROM, 8, now);
    // Facing away, so every beat is a turn and the body stands where it was
    // put — the route stays due from where it was planned, exactly as above.
    world
        .state
        .registry
        .insert(walker, Heading(Facing::walking(Direction::North)));

    let planned = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        ai::Goal::Fixed,
    )
    .expect("there is a way round the far end");
    let held = world
        .registry()
        .get::<Route>(walker)
        .map(|route| route.planned_at)
        .expect("the route was written on the body");

    openshard_items::open_door(&mut world.state, door);
    let fresh = ai::step_toward(&world.state, walker, Facet(0), FROM, GOAL, Doors::AsTheyStand);
    assert_ne!(fresh, Some(planned), "the open door really is a different way");
    for _ in 0..=ai::REPATH_TICKS {
        world.tick(now);
    }
    let still = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        ai::Goal::Fixed,
    );
    assert_eq!(
        still,
        Some(planned),
        "a route to a place has no window on it, so it is still the route"
    );
    assert_eq!(
        world.registry().get::<Route>(walker).map(|kept| kept.planned_at),
        Some(held),
        "and it is the one it planned, not a new one that happens to agree"
    );
}

/// A door standing on the first step of a *freshly planned* route is opened,
/// and not walked into.
///
/// **The rule was written in three places and this was the one it was missing
/// from.** A cached route's door has been `ai`'s since routes were kept; a
/// chase opened its own; and `step_body_toward` handed back a step it had just
/// watched the live world refuse, leaving the opening to whoever called it.
/// One caller did it (`npc::walk_home`, out of the obstruction index, for the
/// first step only) and the other did not — so an escortable following its
/// master through a shop door planned the same route into the same shut door on
/// every beat of its walk.
///
/// The wall is what makes the assertion about the door rather than about
/// tie-breaking: with one way through, the route has to go through it.
#[test]
fn a_door_on_a_freshly_planned_step_is_opened_rather_than_walked_into() {
    /// Standing north of the divider, on the doorway's own column.
    const FROM: Point = Point::new(DOORWAY.0, DOORWAY.1 - 1, 0);
    /// And the goal is the far side of it, one step past the door.
    const GOAL: Point = Point::new(DOORWAY.0, DOORWAY.1 + 1, 0);

    let now = Instant::now();
    let mut world = shard_over(one_doorway(), None);
    let (door, _) = place_door(&mut world, Point::new(DOORWAY.0, DOORWAY.1, 0), now);
    let walker = spawn_brained(&mut world, 0x0190, FROM, 8, now);

    // A body that means to open its way plans through the shut door, so the
    // route's first step is the door tile and the live world refuses it.
    let step = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AllOpen,
        ai::Goal::Fixed,
    );
    assert_eq!(step, None, "the beat went on the door, so there is no step");
    assert!(
        world
            .registry()
            .get::<openshard_state::components::Door>(door)
            .is_some_and(|d| d.is_open),
        "and what it went on is opening it"
    );
    let route = world
        .registry()
        .get::<Route>(walker)
        .cloned()
        .expect("the route it planned is kept for the beat that walks it");
    assert_eq!(
        (route.next, route.at),
        (0, FROM),
        "with its first step still due, from where the body is standing"
    );

    // The beat after it: the way is open and the step is the one that was due.
    let through = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AllOpen,
        ai::Goal::Fixed,
    );
    assert_eq!(
        through,
        route.steps.first().copied(),
        "and it walks the step it opened the door for"
    );
}

/// A pet works a latch exactly as it did before it was tamed, and a llama does
/// not.
///
/// **The `opens_doors` a tamed creature carries was a dead field.** `pet_beat`
/// walked every pet on [`Doors::AllOpen`] whatever body it wore, so a horse
/// planned through a shut shop door — and, once a route outlived the beat that
/// planned it, opened one. ServUO asks the creature (`BaseAI.CanOpenDoors`),
/// which is the same read a wild brain gets in `chase_step`, and the flag is
/// set from the body at both the taming and the restore.
///
/// The pair is the oracle: one wall, one doorway, a shut door in it, and two
/// pets that differ in nothing but the body they wear.
#[test]
fn a_pet_works_a_latch_only_if_its_body_has_hands() {
    /// One step north of the doorway, so the door is the pet's very next step.
    const PET_AT: Point = Point::new(DOORWAY.0, DOORWAY.1 - 1, 0);
    /// And the owner south of it, further off than the follow gap.
    const OWNER_AT: Point = Point::new(DOORWAY.0, DOORWAY.1 + 3, 0);

    for (body, hands) in [(0x00C8_u16, false), (0x0190_u16, true)] {
        let now = Instant::now();
        let mut world = shard_over(one_doorway(), None);
        let (door, _) = place_door(&mut world, Point::new(DOORWAY.0, DOORWAY.1, 0), now);
        // The owner is spawned first, so the *other* brained body is the pet.
        // Sight is what earns a brain at all here; nothing is ticked after it,
        // so neither of them ever acts on what it can see.
        let owner = spawn_brained(&mut world, 0x0190, OWNER_AT, 8, now);
        spawn_brained(&mut world, body, PET_AT, 8, now);
        let pet = world
            .state
            .registry
            .query::<Brain>()
            .map(|(entity, _)| entity)
            .find(|&entity| entity != owner)
            .expect("the second creature is the pet");
        let owner_serial = world
            .state
            .registry
            .serial_of(owner)
            .expect("a spawned mobile has a serial");
        world.state.registry.insert(
            pet,
            openshard_state::components::Pet {
                owner:        owner_serial,
                slots:        openshard_protocol::world::FollowerSlots::ONE,
                order:        openshard_state::components::PetOrder::Follow,
                order_target: None,
            },
        );
        // The premise of the case, asserted rather than assumed: the body table
        // is what decides, and this test is about the read and not the table.
        assert_eq!(
            world.registry().get::<Brain>(pet).map(|brain| brain.opens_doors),
            Some(hands),
            "body {body:#06x} is the one this case is about"
        );

        let step = ai::pet_beat(&mut world.state, pet);
        let opened = world
            .registry()
            .get::<openshard_state::components::Door>(door)
            .is_some_and(|d| d.is_open);
        assert_eq!(
            opened, hands,
            "body {body:#06x}: whether the door is opened is whether the pet has hands"
        );
        assert_eq!(
            step.is_none(),
            hands,
            "body {body:#06x}: a beat spent on the door is not a step, and the \
             one that cannot work it walks at the door instead"
        );
    }
}

/// A route says nothing from anywhere except where its next step starts, and a
/// body that did not move is not standing there.
///
/// **The half of a kept route that is not the saving.** A body advances its
/// route on the beat it hands the direction out, which is before the world has
/// applied it — and the world may refuse it: a mobile stepped into the way, the
/// body is frozen, a decree moved it elsewhere. The route then runs from a place
/// the body is not in, and every step of it stays legal one at a time, so what
/// that costs is not a wall walked through but a body quietly walking a plan
/// nobody made. [`Route::at`] is what catches it.
///
/// Walked to a place ([`ai::Goal::Fixed`]) on purpose: the three checks that are
/// about the world rather than about the clock apply to every route whatever it
/// is aimed at, and dropping the window must not have quietly dropped them.
#[test]
fn a_body_that_did_not_move_plans_its_route_again() {
    const FROM: Point = Point::new(20, 20, 0);
    const GOAL: Point = Point::new(20, 40, 0);

    let now = Instant::now();
    let mut world = shard_over(Scene::flat_holding(63, 63, 0), None);
    let walker = spawn_brained(&mut world, 0x0190, FROM, 8, now);

    // The first beat plans, with the body facing the wrong way: turn-as-step, so
    // it stands still and the route's first step is still due.
    world
        .state
        .registry
        .insert(walker, Heading(Facing::walking(Direction::North)));
    let first = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        ai::Goal::Fixed,
    )
    .expect("open ground is always reachable");
    let turning = world
        .registry()
        .get::<Route>(walker)
        .cloned()
        .expect("the route was written on the body");
    assert_eq!(
        (turning.next, turning.at),
        (0, FROM),
        "a turn is not a step, and the route waits where it was planned"
    );

    // Facing it now: the same step, and this time the route moves on with it.
    world
        .state
        .registry
        .insert(walker, Heading(Facing::walking(first)));
    let walked = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        ai::Goal::Fixed,
    );
    assert_eq!(walked, Some(first), "the same step is due, and it is taken");
    let landing = {
        let footing = world.state.footing(Facet(0), Doors::AsTheyStand);
        openshard_movement::step_allowed(&footing, FROM, first).expect("the shard would allow that step")
    };
    let advanced = world
        .registry()
        .get::<Route>(walker)
        .cloned()
        .expect("the route was written on the body");
    assert_eq!(
        (advanced.next, advanced.at),
        (1, landing),
        "the route moved on, and it is due from where that step lands"
    );

    // And the step never happened — the body is still at `FROM`, which is not
    // where the route it is holding starts.
    world.tick(now);
    let replanned = ai::step_body_toward(
        &mut world.state,
        walker,
        Facet(0),
        FROM,
        GOAL,
        Doors::AsTheyStand,
        ai::Goal::Fixed,
    );
    assert_eq!(
        replanned,
        Some(first),
        "it plans again from where it actually is, which is the same way"
    );
    assert_ne!(
        world.registry().get::<Route>(walker).map(|kept| kept.planned_at),
        Some(advanced.planned_at),
        "and what it walks is that new route, not the one it was not standing on"
    );
}

/// What one counted walk over open ground came to.
struct Walk {
    /// Ticks between the walker's beats — the shipped `creature_step_ticks`.
    beat:  u64,
    /// Beats it took to arrive.
    beats: usize,
    /// Routes planned along the way, counted by their planning tick.
    plans: usize,
}

/// Twenty-four tiles of open ground, walked a beat at a time toward `goal`,
/// counting the searches it cost.
///
/// **Open ground is deliberately the easy case for the code this replaced** —
/// a straight walk with nothing to route around, so every plan it counts is one
/// a body with nowhere to keep an answer would have paid for in full. The tick
/// is deterministic and its dice are seeded, so what comes back is a count and
/// not a sample.
///
/// Both callers below assert arrival, so this does: a walk that did not get
/// there has counted plans for a journey nobody made.
fn walk_over_open_ground(goal: ai::Goal) -> Walk {
    const FROM: Point = Point::new(20, 20, 0);
    const GOAL: Point = Point::new(20, 44, 0);
    /// Slack, not the answer: the walk is 24 steps and a turn, and a cap that
    /// stops a broken route from looping is not a claim about how long it takes.
    const MOST: usize = 64;

    let now = Instant::now();
    let mut world = shard_over(Scene::flat_holding(63, 63, 0), None);
    let walker = spawn_brained(&mut world, 0x0190, FROM, 8, now);
    let beat = world.state.gameplay.creature_step_ticks;

    let mut at = FROM;
    let mut beats = 0;
    let mut plans = 0;
    let mut planned_at = None;
    while at != GOAL && beats < MOST {
        for _ in 0..beat {
            world.tick(now);
        }
        beats += 1;
        let Some(direction) = ai::step_body_toward(
            &mut world.state,
            walker,
            Facet(0),
            at,
            GOAL,
            Doors::AsTheyStand,
            goal,
        ) else {
            break;
        };
        // A route is planned exactly when the one on the body is a new one, and
        // the tick it was planned on is what says so.
        let kept = world
            .registry()
            .get::<Route>(walker)
            .map(|route| route.planned_at);
        if kept != planned_at {
            plans += 1;
            planned_at = kept;
        }
        // Applied the way `motion.rs` applies one, because that is what the
        // route is written down against: a body not yet facing this way turns
        // and stays put, and only then does a step move it.
        let facing = world.state.registry.get::<Heading>(walker).map(|h| h.0.direction);
        if facing == Some(direction) {
            let footing = world.state.footing(Facet(0), Doors::AsTheyStand);
            at = openshard_movement::step_allowed(&footing, at, direction)
                .expect("the shard would allow the step its own planner planned");
        } else {
            world
                .state
                .registry
                .insert(walker, Heading(Facing::walking(direction)));
        }
    }

    assert_eq!(at, GOAL, "the walk arrives");
    Walk { beat, beats, plans }
}

/// The saving, counted: a body chasing something across open ground plans once a
/// repath window and not once a beat.
///
/// **The factor is two constants and neither of them is the route's length.** A
/// route to a moving goal is trusted for [`ai::REPATH_TICKS`] and a creature
/// acts every `creature_step_ticks`, so one plan covers the whole of the first
/// against the second — five beats at the shipped two-second cadence and 400 ms
/// beat. Each of the four it saves was a full [`ai::PATH_BUDGET`] search.
#[test]
fn a_long_walk_plans_once_a_repath_window() {
    let Walk { beat, beats, plans } = walk_over_open_ground(ai::Goal::Moving);
    // Twenty-four and not twenty-five: the body is spawned facing south already,
    // so the first beat is a step rather than the turn a body facing elsewhere
    // would spend.
    assert_eq!(beats, 24, "one beat a tile, due south");
    assert_eq!(
        plans,
        1 + (beats - 1) * beat as usize / ai::REPATH_TICKS as usize,
        "and it planned once to start with, then once a window"
    );
    assert!(
        plans * 4 <= beats,
        "which is {plans} searches for {beats} beats, and it used to be one each"
    );
}

/// And the same walk to a *place*: one search for the whole journey.
///
/// **This is what a townsperson's walk home costs now.** The window is the only
/// thing that was ending a route that nothing had gone wrong with, and a place
/// does not move — so the count is not a function of how long the walk takes,
/// which is the whole difference from the test above. Sixty seconds of walking
/// used to be a full [`ai::PATH_BUDGET`] search every two seconds; it is one
/// search, and the twenty-four beats after it are a step apiece.
#[test]
fn a_long_walk_to_a_place_plans_once_altogether() {
    let Walk { beats, plans, .. } = walk_over_open_ground(ai::Goal::Fixed);
    assert_eq!(beats, 24, "one beat a tile, due south");
    assert_eq!(plans, 1, "planned at the first beat and never again");
}

#[test]
fn a_human_chaser_opens_the_door_in_its_way() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let door_at = Point::new(START.x, START.y + 1, 0);
    let (door, door_serial) = place_door(&mut world, door_at, now);

    // Open the door first so the creature can see and acquire its prey.
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(door_serial.raw())),
    });
    world.tick(now);
    let creature = spawn_brained(&mut world, 0x0190, Point::new(START.x, START.y + 3, 0), 8, now);
    // Only as far as noticing. Ticking a fixed padded count here let it also walk
    // *through* the doorway before the door was slammed, which left nothing in its
    // way and quietly turned this into a test of standing still.
    let now = tick_until(&mut world, now, AI_THINK_TICKS * 2, |w| {
        w.registry()
            .get::<Combat>(creature)
            .and_then(|combat| combat.target())
            .is_some()
    });
    assert!(
        world
            .registry()
            .get::<Combat>(creature)
            .and_then(|combat| combat.target())
            .is_some(),
        "through the open doorway it noticed the player"
    );

    // Slam the door in its face: a human body opens it rather than giving up.
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(door_serial.raw())),
    });
    world.tick(now);
    assert!(!world.registry().get::<Door>(door).unwrap().is_open);
    let mut later = now;
    for _ in 0..(AI_THINK_TICKS * 6) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        world.registry().get::<Door>(door).unwrap().is_open,
        "the chaser worked the handle"
    );
    let creature_at = world.registry().get::<Position>(creature).unwrap().0;
    assert!(
        distance(creature_at, Point::new(START.x, START.y, 0)) <= openshard_combat::MELEE_RANGE,
        "and came through the doorway (ended at {creature_at:?})"
    );
}

/// Spawn a creature with an explicit aggression posture, returning its entity.
fn spawn_postured(world: &mut World, at: Point, sight: u8, aggression: u8, now: Instant) -> EntityId {
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x00D1),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(1),
        damage:      5,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(sight),
        aggression:  Aggression::from_bits(aggression),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    at,
        facet:       Facet(0),
        name:        None,
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    world
        .state
        .registry
        .query::<Brain>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a creature with a brain")
}

#[test]
fn a_defensive_creature_answers_the_blow() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player_serial = world.registry().serial_of(world.state.players[&gm]).unwrap();
    // Defensive and blind: it hunts nothing, so only the blow can start this.
    let creature = spawn_postured(&mut world, Point::new(START.x, START.y + 2, 0), 0, 1, now);
    assert!(
        world.registry().get::<Combat>(creature).is_none(),
        "unprovoked, it minds its own business"
    );
    let creature_serial = world.registry().serial_of(creature).unwrap();
    world.queue(Command::Damage {
        serial:      creature_serial,
        amount:      5,
        damage_type: 0,
        by:          Some(player_serial),
    });
    world.tick(now);
    world.tick(now);
    let combat = world.registry().get::<Combat>(creature).expect("engaged");
    assert_eq!(combat.target(), Some(player_serial), "it turned on its attacker");
    assert!(combat.warmode(), "and it means it");
    assert_eq!(
        world.state.registry.get::<Heading>(creature),
        Some(&Heading(Facing::walking(Direction::North))),
        "it immediately faces the attacker while preparing its return swing"
    );
}

#[test]
fn a_passive_creature_runs_from_its_attacker() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player_at = Point::new(START.x, START.y, 0);
    let player_serial = world.registry().serial_of(world.state.players[&gm]).unwrap();
    let start_at = Point::new(START.x, START.y + 1, 0);
    let creature = spawn_postured(&mut world, start_at, 0, 0, now);
    let creature_serial = world.registry().serial_of(creature).unwrap();
    world.queue(Command::Damage {
        serial:      creature_serial,
        amount:      5,
        damage_type: 0,
        by:          Some(player_serial),
    });
    world.tick(now);
    let combat = world.registry().get::<Combat>(creature).expect("aware");
    assert!(!combat.warmode(), "fauna does not fight back");
    let mut later = now;
    for _ in 0..(AI_THINK_TICKS * 8) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    let fled_to = world.registry().get::<Position>(creature).unwrap().0;
    assert!(
        distance(fled_to, player_at) > distance(start_at, player_at) + 2,
        "the deer ran (ended at {fled_to:?})"
    );
}

#[test]
fn a_gutted_monster_turns_tail() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player_at = Point::new(START.x, START.y, 0);
    let player_serial = world.registry().serial_of(world.state.players[&gm]).unwrap();
    let start_at = Point::new(START.x, START.y + 1, 0);
    let creature = spawn_postured(&mut world, start_at, 8, 2, now);
    let creature_serial = world.registry().serial_of(creature).unwrap();
    // Cut it to under a fifth of its hits: 50 -> 9.
    world.queue(Command::Damage {
        serial:      creature_serial,
        amount:      41,
        damage_type: 0,
        by:          Some(player_serial),
    });
    world.tick(now);
    let mut later = now;
    for _ in 0..(AI_THINK_TICKS * 8) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    let fled_to = world.registry().get::<Position>(creature).unwrap().0;
    assert!(
        distance(fled_to, player_at) > distance(start_at, player_at) + 2,
        "badly hurt, it broke off (ended at {fled_to:?})"
    );
}

#[test]
fn the_chase_pace_is_the_operators_knob() {
    // Two identical hunts, one at the classic 400ms pace and one at the
    // 250ms "monsters catch runners" setting: over the same ticks, the fast
    // shard's creature closes on its prey and the classic one lags behind.
    let chased_distance = |step_ms: u64| {
        let now = Instant::now();
        let gameplay = Gameplay {
            creature_step_ticks: Gameplay::ticks_from_ms(step_ms),
            ..Gameplay::default()
        };
        let mut world = World::new(START).with_gameplay(gameplay);
        let _gm = enter_gm(&mut world, now);
        let player_at = Point::new(START.x, START.y, 0);
        spawn_brained(&mut world, 0x00D1, Point::new(START.x, START.y + 7, 0), 10, now);
        let creature = world
            .state
            .registry
            .query::<Brain>()
            .map(|(entity, _)| entity)
            .next()
            .unwrap();
        let mut later = now;
        for _ in 0..Gameplay::ticks(2) {
            later += TICK_INTERVAL;
            world.tick(later);
        }
        distance(world.registry().get::<Position>(creature).unwrap().0, player_at)
    };
    let classic = chased_distance(400);
    let fast = chased_distance(250);
    assert!(
        fast < classic,
        "the 250ms shard's hunter closed further (fast ended {fast}, classic {classic})"
    );
    assert!(
        fast <= openshard_combat::MELEE_RANGE,
        "at 250ms the hunter caught its prey over 2s from 7 tiles (ended {fast})"
    );
}

/// Spawn a bay horse next to the start and return its entity and serial.
fn spawn_horse(world: &mut World, at: Point, now: Instant) -> (EntityId, Serial) {
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x00C8),
        hue:         openshard_protocol::wire::Hue(0x0455),
        hits:        30,
        notoriety:   Notoriety::from_bits(1),
        damage:      3,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      true,
        position:    at,
        facet:       Facet(0),
        name:        None,
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let horse = world
        .state
        .registry
        .query::<Body>()
        .find(|(_, body)| body.id.0 == 0x00C8)
        .map(|(entity, _)| entity)
        .expect("a horse");
    let serial = world.registry().serial_of(horse).unwrap();
    (horse, serial)
}

#[test]
fn a_horse_is_mounted_and_dismounted_by_double_click() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let (horse, horse_serial) = spawn_horse(&mut world, Point::new(START.x + 1, START.y, 0), now);

    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(horse_serial.raw())),
    });
    world.tick(now);
    let riding = world
        .registry()
        .get::<Riding>(player)
        .copied()
        .expect("in the saddle");
    assert_eq!(riding.mount, horse);
    assert!(
        world.registry().get::<Position>(horse).is_none(),
        "a ridden horse is out of the world"
    );
    let saddle = world
        .registry()
        .get::<Equipped>(riding.item)
        .expect("a mount item");
    assert_eq!(saddle.layer, openshard_items::MOUNT_LAYER);
    assert_eq!(
        world.registry().get::<Drawn>(riding.item).unwrap().id,
        openshard_protocol::wire::Graphic(0x3E9F),
        "a bay horse draws as the bay mount item"
    );

    // A raw self-double-click (no bit 31 — that would be a paperdoll request)
    // dismounts, war mode or peace; the horse lands beside the rider.
    let saddle_serial = world.registry().serial_of(riding.item).unwrap();
    let _ = packets_for(&mut world, gm); // clear the outbox before the dismount
    let player_serial = world.registry().serial_of(player).unwrap();
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(player_serial.raw())),
    });
    world.tick(now);
    assert!(world.registry().get::<Riding>(player).is_none());
    // The rider's own client is told to remove the mount item (a 0x1D), or it
    // keeps drawing the saddle and the rider looks mounted on foot.
    assert!(
        packets_for(&mut world, gm)
            .iter()
            .any(|p| p[0] == 0x1D && mentions(p, saddle_serial)),
        "the saddle is removed from the rider's own screen"
    );
    let horse_at = world
        .registry()
        .get::<Position>(horse)
        .expect("back on the ground")
        .0;
    assert!(
        distance(horse_at, Point::new(START.x, START.y, 0)) <= 1,
        "the horse stands beside its rider"
    );
}

#[test]
fn a_dismount_does_not_put_a_horse_through_a_corner() {
    // Where a dismounted horse may stand is a step question, and the shard has
    // one step rule since `World::step` went through `step_allowed`. The
    // dismount asked eight `can_step` calls instead — one landing each, and a
    // landing has no corner rule in it — so it could put a creature on a tile
    // nothing could have walked to, and that the same rule may refuse to walk it
    // off again. It asks `steps_out_of` now, which answers the eight for the
    // price of one and brings the corner rule with it.
    //
    // Reverted to `can_step`, the horse stands at the south-east diagonal and
    // this fails at the first assertion.
    //
    // A note for anyone reading the loop: with the corner rule in it, a diagonal
    // is never what it picks. A legal diagonal needs both its flanking cardinals
    // to be steppable, and both come earlier in `Direction::to_bits` order — so
    // the choice is the first open cardinal, or under the rider.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let at = world.registry().get::<Position>(player).unwrap().0;
    let (horse, horse_serial) = spawn_horse(&mut world, Point::new(at.x + 1, at.y, 0), now);
    let player_serial = world.registry().serial_of(player).unwrap();
    let mount = |world: &mut World| {
        world.queue(Command::DoubleClick {
            connection: gm,
            request:    UseRequest::Use(RawSerial(horse_serial.raw())),
        });
        world.tick(now);
    };
    let dismount = |world: &mut World| {
        world.queue(Command::DoubleClick {
            connection: gm,
            request:    UseRequest::Use(RawSerial(player_serial.raw())),
        });
        world.tick(now);
    };

    mount(&mut world);
    assert!(world.registry().get::<Riding>(player).is_some(), "in the saddle");

    // Every neighbour but the south-east diagonal is a crate, and the two that
    // flank that diagonal — east and south — are among them. So the one tile
    // beside the rider a body could *stand* on is one no body could reach.
    let around: [(i16, i16); 7] = [(0, -1), (1, -1), (1, 0), (0, 1), (-1, 1), (-1, 0), (-1, -1)];
    let crates: Vec<EntityId> = around
        .iter()
        .map(|(dx, dy)| {
            let entity = world.state.registry.spawn();
            world.state.facet_state_mut(Facet(0)).block(
                at.x.wrapping_add_signed(*dx),
                at.y.wrapping_add_signed(*dy),
                entity,
                openshard_map::overlay::Cover::blocking(0, openshard_state::DOOR_HEIGHT),
            );
            entity
        })
        .collect();

    dismount(&mut world);
    let horse_at = world
        .registry()
        .get::<Position>(horse)
        .expect("back on the ground")
        .0;
    assert_eq!(
        (horse_at.x, horse_at.y),
        (at.x, at.y),
        "the only open tile is one the corner rule refuses, so the horse lands under its rider"
    );

    // The control: it is not that the loop stopped finding anywhere. Take the
    // crate to the north away and the identical dismount uses it.
    world
        .state
        .facet_state_mut(Facet(0))
        .unblock(at.x, at.y - 1, crates[0]);
    mount(&mut world);
    assert!(
        world.registry().get::<Riding>(player).is_some(),
        "back in the saddle"
    );
    dismount(&mut world);
    let horse_at = world
        .registry()
        .get::<Position>(horse)
        .expect("back on the ground")
        .0;
    assert_eq!(
        (horse_at.x, horse_at.y),
        (at.x, at.y - 1),
        "with one neighbour open the horse takes it"
    );
}

#[test]
fn a_paperdoll_request_leaves_the_rider_mounted() {
    // The relogin bug: ClassicUO opens the paperdoll on login with a 0x06 whose
    // serial carries bit 31 — a paperdoll *request*, not a use. ServUO's `UseReq`
    // routes it straight to the paperdoll; treating it as a raw self-double-click
    // is what used to throw the rider off a breath after logging in mounted.
    // The bit itself is read by `DoubleClick::interpret` (tested in
    // `openshard_protocol::containers`); what this holds is the half that
    // matters here — the two requests take different paths through the tick.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let (_horse, horse_serial) = spawn_horse(&mut world, Point::new(START.x + 1, START.y, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(horse_serial.raw())),
    });
    world.tick(now);
    assert!(world.registry().get::<Riding>(player).is_some(), "mounted");
    let _ = packets_for(&mut world, gm);

    let player_serial = world.registry().serial_of(player).unwrap();
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Paperdoll(RawSerial(player_serial.raw())),
    });
    world.tick(now);
    assert!(
        world.registry().get::<Riding>(player).is_some(),
        "the paperdoll request leaves the rider in the saddle"
    );
    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x88),
        "and still opens the paperdoll"
    );
}

#[test]
fn a_ridden_horse_does_not_wander_and_the_ride_survives_logout() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let (horse, horse_serial) = spawn_horse(&mut world, Point::new(START.x + 1, START.y, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(horse_serial.raw())),
    });
    world.tick(now);
    assert!(world.registry().get::<Position>(horse).is_none());

    // Many beats: a ridden wanderer stays exactly where it is — nowhere.
    let mut later = now;
    for _ in 0..(AI_THINK_TICKS * 6) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        world.registry().get::<Position>(horse).is_none(),
        "no brain beat moves a ridden mount"
    );

    // The rider logs out still mounted: the ride is saved on the saddle, not
    // grounded. The transient creature is dropped from limbo — it is rebuilt from
    // the saved saddle on relogin — so it is neither standing on the ground nor
    // leaked there.
    world.queue(Command::Disconnect { connection: gm });
    world.tick(later);
    assert!(
        world.registry().get::<Position>(horse).is_none(),
        "logout keeps the ride on the saddle rather than grounding the mount"
    );
}

#[test]
fn a_mounted_character_logs_back_in_still_mounted() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let char_serial = world.registry().serial_of(player).unwrap();
    let (_horse, horse_serial) = spawn_horse(&mut world, Point::new(START.x + 1, START.y, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(horse_serial.raw())),
    });
    world.tick(now);
    let mount_graphic = {
        let riding = world.registry().get::<Riding>(player).copied().expect("mounted");
        world.registry().get::<Drawn>(riding.item).unwrap().id
    };

    // The save now carries the saddle, on the mount layer.
    world.take_snapshot();
    let snapshot = world.drain_saves().next_back().expect("a snapshot");
    assert!(
        snapshot.inventories.iter().any(|inventory| {
            inventory.items.iter().any(|item| {
                matches!(
                    item.location,
                    ItemLocation::Equipped { layer, .. } if Layer(layer) == openshard_items::MOUNT_LAYER
                )
            })
        }),
        "the mount item rides along in the record"
    );

    // Log out and log the same character back in, in the same run: it returns to
    // the world still in the saddle, on a rebuilt mount that draws the same.
    world.queue(Command::Disconnect { connection: gm });
    world.tick(now);
    let gm = connection();
    world.queue(Command::Enter(Entering {
        connection: gm,
        version:    ClientVersion::TOL,
        account:    AccountName("admin".to_owned()),
        name:       CharacterName("Lord British".to_owned()),
        access:     AccessLevel::GameMaster,
        character:  Character::Saved,
    }));
    world.tick(now);
    let player = world.state.players[&gm];
    assert_eq!(
        world.registry().serial_of(player).unwrap(),
        char_serial,
        "the same character came back, so the saddle filed under its serial is findable"
    );
    let riding = world
        .registry()
        .get::<Riding>(player)
        .copied()
        .expect("still in the saddle after relogin");
    assert!(
        world.registry().get::<Ridden>(riding.mount).is_some(),
        "the ridden creature was rebuilt from the saved saddle"
    );
    assert_eq!(
        world.registry().get::<Drawn>(riding.item).unwrap().id,
        mount_graphic,
        "and it draws as the same mount it was"
    );

    // And dismounting the REBUILT mount draws it: the save kept only the saddle,
    // so the creature must be reconstituted whole — above all its `Heading`,
    // without which the 0x78 encoder returns nothing and the horse is invisible.
    let mount_serial = world.registry().serial_of(riding.mount).unwrap();
    let _ = packets_for(&mut world, gm);
    let player_serial = world.registry().serial_of(player).unwrap();
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(player_serial.raw())),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, gm)
            .iter()
            .any(|p| p[0] == 0x78 && mentions(p, mount_serial)),
        "the rebuilt horse is drawn for the rider on dismount"
    );
    let mount = riding.mount;
    assert!(
        world.registry().get::<Heading>(mount).is_some(),
        "the dismounted horse has a heading"
    );
    assert!(
        world.registry().get::<Movement>(mount).is_some(),
        "and a walker, so it can move"
    );
    assert!(
        world.registry().get::<Brain>(mount).is_some(),
        "and a brain, so it behaves like an animal"
    );
}

#[test]
fn a_dismounted_horse_stays_beside_the_rider_through_its_beats() {
    // The ride never moves the walker, so a horse ridden across the map used to
    // take its first post-dismount step from where it was *mounted* — teleporting
    // away and vanishing (0x1D) off the rider's screen a beat later.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let (horse, horse_serial) = spawn_horse(&mut world, Point::new(START.x + 1, START.y, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(horse_serial.raw())),
    });
    world.tick(now);
    assert!(world.registry().get::<Riding>(player).is_some(), "mounted");

    // Ride far from the mounting spot.
    let far = Point::new(START.x + 30, START.y, 0);
    teleport(&mut world, gm, far);

    // Dismount there, with a raw self-double-click.
    let player_serial = world.registry().serial_of(player).unwrap();
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(player_serial.raw())),
    });
    world.tick(now);
    let _ = packets_for(&mut world, gm);

    // Give the horse several brain beats; it must amble near the rider, not
    // teleport back to the mounting spot and drop off the rider's screen.
    let mut later = now;
    let mut forgotten = false;
    for _ in 0..(AI_THINK_TICKS * 6) {
        later += TICK_INTERVAL;
        world.tick(later);
        forgotten |= packets_for(&mut world, gm)
            .iter()
            .any(|p| p[0] == 0x1D && mentions(p, horse_serial));
    }
    let horse_at = world
        .registry()
        .get::<Position>(horse)
        .expect("still in the world")
        .0;
    assert!(
        distance(horse_at, far) <= 6,
        "the horse ambles near where it was dismounted, not back at the stable: {horse_at}"
    );
    assert!(!forgotten, "the horse never dropped off the rider's screen");
}

#[test]
fn a_shop_sells_goods_and_buys_them_back() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);

    // A shopkeeper one tile away, stocked with typed iron ingots by "the script".
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(1),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x + 1, START.y, 0),
        facet:       Facet(0),
        name:        Some("Mirabel".to_owned()),
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      true,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let vendor = world
        .state
        .registry
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a shopkeeper");
    let vendor_serial = world.registry().serial_of(vendor).unwrap();
    world.queue(Command::StockVendor {
        serial: vendor_serial,
        stock:  vec![npc::StockLine {
            // The legacy drawing is ignored for direct typed stock; projection
            // comes from the registry.
            graphic:   openshard_protocol::wire::Graphic(0),
            hue:       openshard_protocol::wire::Hue(0),
            item_kind: Some(openshard_protocol::item_kind::ItemKindId(1)),
            material:  Some(openshard_protocol::item_kind::MaterialId(1)),
            amount:    openshard_state::components::Amount(50),
            price:     openshard_state::components::Price(4),
            name:      "iron ingot".to_owned(),
        }],
    });
    world.tick(now);
    let stock_item = world
        .state
        .registry
        .query::<openshard_state::components::Price>()
        .map(|(entity, _)| entity)
        .next()
        .expect("stocked goods");
    let stock_serial = world.registry().serial_of(stock_item).unwrap();

    // A hundred coins in the pack, and a double-click opens the shop: the buy
    // list rides out with the contents.
    let backpack = backpack_serial(&world, gm);
    assert!(
        openshard_items::give(
            &mut world.state,
            backpack,
            openshard_protocol::wire::Graphic(GOLD),
            openshard_protocol::wire::Hue(0),
            100,
        )
        .is_complete()
    );
    world.drain_outbound().count();
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(vendor_serial.raw())),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, gm)
            .iter()
            .any(|p| p.first() == Some(&0x74)),
        "the shop opened with a price list"
    );

    // Three ingots at four coins: twelve gold change hands.
    world.queue(Command::Buy {
        connection: gm,
        vendor:     RawSerial(vendor_serial.raw()),
        purchases:  vec![openshard_protocol::vendor::Purchase {
            serial: RawSerial(stock_serial.raw()),
            amount: openshard_protocol::items::ItemAmount(3),
        }],
    });
    world.tick(now);
    assert_eq!(
        openshard_items::count_in_container(&world.state, backpack, openshard_protocol::wire::Graphic(GOLD)),
        88,
        "twelve gold paid"
    );
    assert_eq!(
        openshard_items::count_in_container(
            &world.state,
            backpack,
            openshard_protocol::wire::Graphic(0x1BF2)
        ),
        3,
        "three ingots delivered"
    );
    let bought = world
        .state
        .registry
        .query::<Contained>()
        .find_map(|(item, held)| {
            (held.container == backpack
                && world
                    .registry()
                    .get::<openshard_state::components::ItemKind>(item)
                    == Some(&openshard_state::components::ItemKind(
                        openshard_protocol::item_kind::ItemKindId(1),
                    ))
                && world
                    .registry()
                    .get::<openshard_state::components::Material>(item)
                    == Some(&openshard_state::components::Material(
                        openshard_protocol::item_kind::MaterialId(1),
                    )))
            .then(|| world.registry().serial_of(item).unwrap().raw())
        })
        .expect("bought ingots retain their typed stock identity");

    // Sell two back at half price: four gold returns.
    world.queue(Command::Sell {
        connection: gm,
        vendor:     RawSerial(vendor_serial.raw()),
        sales:      vec![openshard_protocol::vendor::Sale {
            serial: RawSerial(bought),
            amount: openshard_protocol::items::ItemAmount(2),
        }],
    });
    world.tick(now);
    assert_eq!(
        openshard_items::count_in_container(&world.state, backpack, openshard_protocol::wire::Graphic(GOLD)),
        92,
        "two pearls at half price is four gold"
    );
    assert_eq!(
        openshard_items::count_in_container(
            &world.state,
            backpack,
            openshard_protocol::wire::Graphic(0x1BF2)
        ),
        1,
        "one ingot kept"
    );

    // A pauper is refused: the vendor keeps its goods when gold runs short.
    world.queue(Command::Buy {
        connection: gm,
        vendor:     RawSerial(vendor_serial.raw()),
        purchases:  vec![openshard_protocol::vendor::Purchase {
            serial: RawSerial(stock_serial.raw()),
            amount: openshard_protocol::items::ItemAmount(47),
        }],
    });
    world.tick(now);
    assert_eq!(
        openshard_items::count_in_container(&world.state, backpack, openshard_protocol::wire::Graphic(GOLD)),
        92,
        "no gold moved on the refused purchase"
    );
}

#[test]
fn a_shop_keyword_needs_the_vendor_named_and_an_empty_sell_answers_overhead() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);

    // A shopkeeper one tile off, its stock crate empty.
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(1),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x + 1, START.y, 0),
        facet:       Facet(0),
        name:        Some("Mirabel".to_owned()),
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      true,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let vendor = world
        .state
        .registry
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a shopkeeper");
    let vendor_serial = world.registry().serial_of(vendor).unwrap();
    world.drain_outbound().count();

    // A bare "buy" reaches nobody: ServUO's `VendorAI.OnSpeech` opens a shop on an
    // unqualified word only for `vendor buy`, and on a bare "buy" only when the
    // shopkeeper was named. Before that rule this was a substring test on the whole
    // line, so "that sword is unsellable" opened a buy-back list and a bare "sell"
    // in a crowded bank opened whichever shop happened to be nearest.
    world.queue(Command::Say {
        connection: gm,
        mode:       RawTalkMode(0),
        hue:        RawHue(0),
        font:       RawFont(3),
        text:       "i wonder what to buy".to_owned(),
    });
    world.tick(now);
    assert!(
        !packets_for(&mut world, gm)
            .iter()
            .any(|p| p.first() == Some(&0x74)),
        "an unaddressed 'buy' must not open a shop"
    );

    // Naming the shopkeeper does open it, exactly as a double-click would.
    world.queue(Command::Say {
        connection: gm,
        mode:       RawTalkMode(0),
        hue:        RawHue(0),
        font:       RawFont(3),
        text:       "Mirabel buy".to_owned(),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, gm)
            .iter()
            .any(|p| p.first() == Some(&0x74)),
        "naming the shopkeeper and saying 'buy' opened the shop"
    );

    // And so does ServUO's unqualified keyword, which needs no name.
    world.queue(Command::Say {
        connection: gm,
        mode:       RawTalkMode(0),
        hue:        RawHue(0),
        font:       RawFont(3),
        text:       "vendor buy".to_owned(),
    });
    world.tick(now);
    let packets = packets_for(&mut world, gm);
    assert!(
        packets.iter().any(|p| p.first() == Some(&0x74)),
        "'vendor buy' opened the shop with no name"
    );
    let spoken = packets
        .iter()
        .position(|p| p.first() == Some(&0xAE))
        .expect("the player hears their own shop request");
    let opened = packets
        .iter()
        .position(|p| p.first() == Some(&0x24))
        .expect("the request opens the vendor's gump");
    assert!(
        spoken < opened,
        "speech reaches the client before the vendor gump covers the world"
    );

    // A trade window never changes the separate paperdoll request path. This
    // is the exact follow-up a player makes after speaking the shop keyword.
    let player_serial = world.registry().serial_of(world.state.players[&gm]).unwrap();
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Paperdoll(RawSerial(player_serial.raw())),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x88),
        "a vendor gump does not suppress the player's paperdoll response"
    );

    // "sell" with nothing the vendor wants is answered over the vendor's head as
    // ordinary speech (0xAE from the vendor), not a private system line (0x1C).
    world.queue(Command::Say {
        connection: gm,
        mode:       RawTalkMode(0),
        hue:        RawHue(0),
        font:       RawFont(3),
        text:       "vendor sell".to_owned(),
    });
    world.tick(now);
    let packets = packets_for(&mut world, gm);
    assert!(
        packets.iter().any(|p| p[0] == 0xAE && mentions(p, vendor_serial)),
        "the vendor spoke its refusal over its own head"
    );
    assert!(
        !packets.iter().any(|p| p[0] == 0x1C),
        "and not as a private system message"
    );
}

#[test]
fn a_bought_out_shelf_refills_when_its_hour_is_up() {
    // ServUO's `BaseVendor.Restock`, checked on shop-open (`DelayRestock`, an hour).
    // Without it a shelf someone cleaned out stayed empty for the life of the shard.
    // The price and the label have to come back with the goods: a sold-out line
    // leaves no item behind to copy them from, which is why the full shelf is
    // remembered rather than reconstructed.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(0x0190),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(1),
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(0),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        position:    Point::new(START.x + 1, START.y, 0),
        facet:       Facet(0),
        name:        Some("Mirabel".to_owned()),
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      true,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    let vendor = world
        .state
        .registry
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a shopkeeper");
    let vendor_serial = world.registry().serial_of(vendor).unwrap();
    world.queue(Command::StockVendor {
        serial: vendor_serial,
        stock:  vec![npc::StockLine {
            graphic:   openshard_protocol::wire::Graphic(0),
            hue:       openshard_protocol::wire::Hue(0),
            item_kind: Some(openshard_protocol::item_kind::ItemKindId(1)),
            material:  Some(openshard_protocol::item_kind::MaterialId(9)),
            amount:    openshard_state::components::Amount(20),
            price:     openshard_state::components::Price(4),
            name:      "valorite ingot".to_owned(),
        }],
    });
    world.tick(now);

    // Clear the shelf the way a buyer would: the item is simply gone.
    let pearls = world
        .state
        .registry
        .query::<openshard_state::components::Contained>()
        .map(|(item, _)| item)
        .next()
        .expect("stock on the shelf");
    world.state.registry.despawn(pearls);

    // Opening the shop before the hour is up finds it still empty.
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(vendor_serial.raw())),
    });
    world.tick(now);
    assert_eq!(
        world
            .state
            .registry
            .query::<openshard_state::components::Contained>()
            .count(),
        0,
        "the shelf must not refill early"
    );

    // Wind the clock past the delay and open it again.
    world.state.ticks += npc::RESTOCK_TICKS;
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(vendor_serial.raw())),
    });
    world.tick(now);
    let restocked: Vec<_> = world
        .state
        .registry
        .query::<openshard_state::components::Contained>()
        .map(|(item, _)| item)
        .collect();
    assert_eq!(restocked.len(), 1, "the line came back");
    let item = restocked[0];
    assert_eq!(
        world
            .registry()
            .get::<openshard_state::components::Amount>(item)
            .map(|a| a.0),
        Some(20),
        "at its full amount"
    );
    assert_eq!(
        world
            .registry()
            .get::<openshard_state::components::Price>(item)
            .map(|p| p.0),
        Some(4),
        "and at its price, not a default of one"
    );
    assert_eq!(
        world.registry().get::<Name>(item).map(|n| n.0.as_str()),
        Some("valorite ingot"),
        "and with its label"
    );
    assert_eq!(
        world
            .registry()
            .get::<openshard_state::components::ItemKind>(item),
        Some(&openshard_state::components::ItemKind(
            openshard_protocol::item_kind::ItemKindId(1),
        )),
        "restock reconstructed the semantic kind"
    );
    assert_eq!(
        world
            .registry()
            .get::<openshard_state::components::Material>(item),
        Some(&openshard_state::components::Material(
            openshard_protocol::item_kind::MaterialId(9),
        )),
        "and the selected material"
    );
}

/// Spawn an archer-shaped creature: ranged reach 8, energy bolts.
fn spawn_archer(world: &mut World, at: Point, now: Instant) -> EntityId {
    spawn_archer_bodied(world, 0x0190, at, now)
}

/// The same archer with a chosen body — a beast body cannot open doors.
fn spawn_archer_bodied(world: &mut World, body: u16, at: Point, now: Instant) -> EntityId {
    world.queue(Command::SpawnMobile {
        body:        openshard_protocol::wire::Graphic(body),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        50,
        notoriety:   Notoriety::from_bits(5),
        damage:      7,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        // Half a second between shots, stated as a span rather than as the bare
        // `10` it used to be. That ten was half a second at the 50ms tick and a
        // quarter at the 25ms one — and a quarter-second swing re-faces a kiting
        // archer at its quarry faster than its own beat can step away, so it
        // spent every beat turning round and never opened the gap.
        swing:       Gameplay::ticks_from_ms(500),
        sight:       Sight(10),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      Some(RangedRange::new(8).expect("an archer has reach")),
        ranged_kind: DamageType::Energy,
        wander:      false,
        position:    at,
        facet:       Facet(0),
        name:        None,
        title:       None,
        shoe:        0,
        fame:        0,
        karma:       0,
        night_home:  None,
        banker:      false,
        vendor:      false,
        healer:      false,
        equipment:   Vec::new(),
        skills:      Vec::new(),
        stock:       Vec::new(),
        escort_to:   None,
        quests:      Vec::new(),
    });
    world.tick(now);
    world
        .state
        .registry
        .query::<openshard_state::components::RangedAttack>()
        .map(|(entity, _)| entity)
        .next()
        .expect("an archer")
}

#[test]
fn a_ranged_creature_volleys_from_a_distance() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let before = world.registry().get::<Hitpoints>(player).unwrap().current;
    spawn_archer(&mut world, Point::new(START.x, START.y + 5, 0), now);

    let mut later = now;
    for _ in 0..Gameplay::ticks(2) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    let after = world.registry().get::<Hitpoints>(player).unwrap().current;
    assert!(
        after < before,
        "five tiles out and in sight, the bolts landed ({before} -> {after})"
    );
}

#[test]
fn a_pressed_archer_backs_away() {
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);
    let player_at = Point::new(START.x, START.y, 0);
    let archer = spawn_archer(&mut world, Point::new(START.x, START.y + 1, 0), now);

    let mut later = now;
    for _ in 0..Gameplay::ticks(2) {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    let stood = world.registry().get::<Position>(archer).unwrap().0;
    assert!(
        distance(stood, player_at) > 2,
        "an archer does not brawl: it opened the gap (ended at {stood:?})"
    );
}

#[test]
fn no_volley_passes_a_shut_door() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    // The archer boxed in a ring of crates whose only gap is a doorway: when
    // the door shuts there is no line to shoot down and no way around — and a
    // beast body cannot work the handle.
    let den = Point::new(START.x, START.y + 3, 0);
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            if dx == 0 && dy == 0 || (dx == 0 && dy == -1) {
                continue; // the north gap stays open for the door
            }
            let crate_entity = world.state.registry.spawn();
            world.state.facet_state_mut(Facet(0)).block(
                (i32::from(den.x) + dx) as u16,
                (i32::from(den.y) + dy) as u16,
                crate_entity,
                openshard_map::overlay::Cover::blocking(0, openshard_state::DOOR_HEIGHT),
            );
        }
    }
    let (_door, door_serial) = place_door(&mut world, Point::new(den.x, den.y - 1, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(door_serial.raw())),
    });
    world.tick(now);
    let archer = spawn_archer_bodied(&mut world, 0x00D1, den, now);
    let mut later = now;
    for _ in 0..12 {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    assert!(
        world
            .registry()
            .get::<Combat>(archer)
            .and_then(|combat| combat.target())
            .is_some(),
        "it took aim through the open door"
    );
    world.queue(Command::DoubleClick {
        connection: gm,
        request:    UseRequest::Use(RawSerial(door_serial.raw())),
    });
    world.tick(later);
    let before = world.registry().get::<Hitpoints>(player).unwrap().current;
    for _ in 0..40 {
        later += TICK_INTERVAL;
        world.tick(later);
    }
    let after = world.registry().get::<Hitpoints>(player).unwrap().current;
    assert_eq!(after, before, "a shut door stops arrows too");
}

// --- Level of detail (LOD) ------------------------------------------------
//
// `state.ticks` is bumped at the very top of `tick`, and `think` reschedules
// `next_think = now + gap` reading that same counter, so right after the tick a
// creature thought in, `next_think - state.ticks` is exactly the gap it chose.
// A far creature that has no player near is spawned beyond `lod_radius`; a near
// one within it. `spawn_brained` runs one tick, and its brain starts at
// `next_think = 0`, so it thinks (and reschedules) on that very tick.

/// A world with LOD on at the shipped-default radius (32) and idle factor (8).
fn lod_world() -> World {
    World::new(START).with_gameplay(Gameplay {
        lod: true,
        lod_radius: 32,
        lod_idle_factor: 8,
        ..Default::default()
    })
}

/// Tick until `creature` takes a beat, and return the gap it re-armed itself to.
///
/// Every beat is jittered (`npc::next_beat`), so neither the tick a beat lands on
/// nor the exact gap it sets is fixed. What the LOD tests are about is which
/// *rule* chose the gap — hunting, ambling, dozing — which is still legible: the
/// three are an order of magnitude apart, and the spread is a quarter of one
/// interval. So they assert a band, not a number.
fn beat_gap(world: &mut World, creature: EntityId, now: Instant) -> u64 {
    let before = world.registry().get::<Brain>(creature).unwrap().next_think;
    for _ in 0..500 {
        world.tick(now);
        let brain = *world.registry().get::<Brain>(creature).unwrap();
        if brain.next_think != before {
            return brain.next_think - world.state.ticks;
        }
    }
    panic!("the creature never took a beat");
}

/// The band a beat armed for `interval` ticks may land in.
fn beat_band(interval: u64) -> std::ops::Range<u64> {
    interval..interval + u64::from(openshard_npc::beat_jitter(interval))
}

/// Tick until `done` holds, up to `limit` ticks. Returns whether it did.
///
/// Waiting a *fixed* number of ticks for something a jittered beat decides is
/// wrong in both directions. Too few and the test is a coin flip on the seed; too
/// many and it can overshoot into the next thing entirely — which is not
/// hypothetical, since padding the wait for "did the creature notice me" also
/// gave it time to walk through the open doorway, so the test for opening a
/// slammed door stopped having a door in front of the creature at all.
fn tick_until(world: &mut World, from: Instant, limit: u64, done: impl Fn(&World) -> bool) -> Instant {
    let mut at = from;
    for _ in 0..limit {
        if done(world) {
            break;
        }
        at += TICK_INTERVAL;
        world.tick(at);
    }
    at
}

#[test]
fn lod_off_a_far_creature_still_ambles() {
    // Baseline: with LOD off, a creature no one is near still thinks each idle
    // beat — twice the hunting beat, the amble the default has always used.
    let now = Instant::now();
    let mut world = world();
    let _conn = enter(&mut world, now);
    let creature = spawn_brained(&mut world, 0x00D1, Point::new(START.x, START.y + 60, 0), 5, now);
    let base = world.state.gameplay.creature_step_ticks.max(1);
    let gap = beat_gap(&mut world, creature, now);
    let want = beat_band(base * 2);
    assert!(
        want.contains(&gap),
        "LOD off: a far creature ambles at twice the beat, as it always has \
         (gap {gap}, expected {want:?})"
    );
}

#[test]
fn lod_a_far_creature_dozes() {
    // With LOD on and no player within the radius, the creature skips the costly
    // decision and dozes: its next think is pushed out by the idle factor.
    let now = Instant::now();
    let mut world = lod_world();
    let _conn = enter(&mut world, now);
    let creature = spawn_brained(&mut world, 0x00D1, Point::new(START.x, START.y + 60, 0), 5, now);
    let base = world.state.gameplay.creature_step_ticks.max(1);
    let factor = world.state.gameplay.lod_idle_factor;
    let gap = beat_gap(&mut world, creature, now);
    let want = beat_band(base * factor);
    assert!(
        want.contains(&gap),
        "LOD on, no player near: the far creature dozes at the stretched beat \
         (gap {gap}, expected {want:?})"
    );
}

#[test]
fn lod_a_near_creature_thinks_at_full_rate() {
    // A creature a player is close to is never dozed — it thinks at full rate,
    // so the LOD gap never appears on it.
    let now = Instant::now();
    let mut world = lod_world();
    let _conn = enter(&mut world, now);
    // Well inside `lod_radius` (32) of the player at START.
    let creature = spawn_brained(&mut world, 0x00D1, Point::new(START.x, START.y + 5, 0), 5, now);
    let base = world.state.gameplay.creature_step_ticks.max(1);
    let factor = world.state.gameplay.lod_idle_factor;
    let brain = *world.registry().get::<Brain>(creature).unwrap();
    assert!(
        brain.next_think - world.state.ticks <= base * 2,
        "LOD on but a player is near: the creature thinks at full rate, not dozed"
    );
    assert!(
        brain.next_think - world.state.ticks < base * factor,
        "a near creature is never given the doze gap"
    );
}

#[test]
fn lod_an_engaged_creature_keeps_simulating() {
    // A creature already in a fight keeps simulating even with no player near —
    // a fight must not freeze because the target stepped a tile out of range.
    let now = Instant::now();
    let mut world = lod_world();
    let conn = enter(&mut world, now);
    let player = world.state.players[&conn];
    let target = world.state.registry.serial_of(player).unwrap();
    let creature = spawn_brained(&mut world, 0x00D1, Point::new(START.x, START.y + 60, 0), 5, now);
    // Engage it and let it think again this coming tick.
    world.state.registry.insert(
        creature,
        Combat::creature_engaged(target, openshard_state::WorldTick::ZERO),
    );
    world
        .state
        .registry
        .get_mut::<Brain>(creature)
        .unwrap()
        .next_think = openshard_state::WorldTick::ZERO;
    world.tick(now);
    let base = world.state.gameplay.creature_step_ticks.max(1);
    let factor = world.state.gameplay.lod_idle_factor;
    let brain = *world.registry().get::<Brain>(creature).unwrap();
    assert!(
        brain.next_think - world.state.ticks < base * factor,
        "an engaged creature is not dozed, even with no player near"
    );
}

#[test]
fn lod_walking_into_a_sleeping_town_wakes_it() {
    use openshard_state::components::Npc;
    // LOD's saving comes from letting a mobile nobody is near push its next beat
    // sixteen seconds out. Nothing used to take that back when someone arrived —
    // a dozing townsperson simply finished a timer set while the street was
    // empty. So walking into a town found a still tableau that came to life up to
    // sixteen seconds later, which is what "the NPCs only start acting when I get
    // close" is. Sphere's `_GoAwake` is the missing half.
    let now = Instant::now();
    let mut world = lod_world();
    let conn = enter(&mut world, now); // player at START
    let far = Point::new(START.x, START.y + 300, 0);
    let npc = spawn_townsperson(&mut world, "the peasant", far, now);

    // Let it notice nobody is there and doze.
    for _ in 0..(openshard_npc::BEAT_TICKS * 2) {
        world.tick(now);
    }
    let dozing = world.registry().get::<Npc>(npc).unwrap().next_beat;
    assert!(
        dozing > world.state.ticks + openshard_npc::BEAT_TICKS,
        "the far townsperson should be dozing, not beating"
    );

    // Walk up to it. The wake is what pulls that timer back in.
    teleport(&mut world, conn, Point::new(far.x, far.y + 2, 0));
    world.tick(now);
    let woken = world.registry().get::<Npc>(npc).unwrap().next_beat;
    assert!(
        woken <= world.state.ticks + openshard_npc::BEAT_TICKS,
        "arriving wakes it within a beat (was {dozing}, now {woken}, tick {})",
        world.state.ticks
    );
}

#[test]
fn lod_a_spawner_with_no_player_near_stays_dormant_then_wakes() {
    use crate::spawner::{
        CreatureTemplate,
        SpawnArea,
        Spawner,
    };
    // With LOD on, a spawn region no player is near keeps its timer held and puts
    // nothing down — the freeze a whole-facet Populate caused was a thousand such
    // regions all filling at once. It fills the moment a player arrives.
    let now = Instant::now();
    let mut world = lod_world();
    let conn = enter(&mut world, now); // player at START
    let creature = CreatureTemplate {
        fame:        0,
        karma:       0,
        body:        openshard_protocol::wire::Graphic(0x0009),
        hue:         openshard_protocol::wire::Hue(0),
        hits:        10,
        notoriety:   openshard_protocol::mobile::Notoriety::Neutral,
        damage:      0,
        resistance:  openshard_protocol::world::PhysicalResistance::new(0),
        swing:       0,
        sight:       Sight(0),
        aggression:  Aggression::from_bits(2),
        beat:        0,
        ranged:      None,
        ranged_kind: DamageType::Physical,
        wander:      false,
        skills:      Vec::new(),
    };
    // Far beyond the LOD radius of the player at START.
    let area = SpawnArea {
        x:      START.x,
        y:      START.y + 300,
        width:  2,
        height: 2,
        facet:  Facet(0),
    };
    world.queue(Command::RegisterSpawner {
        spawner: Spawner::new(
            openshard_state::SpawnerId::PLACEHOLDER,
            area,
            vec![creature],
            3,
            40,
        ),
    });
    world.tick(now);
    world.spawners[0].next_spawn = openshard_state::WorldTick::ZERO; // isolate the proximity gate from the jitter

    for _ in 0..12 {
        world.tick(now);
    }
    assert_eq!(
        world.registry().query::<SpawnedBy>().count(),
        0,
        "LOD on, no player near: the region stays dormant"
    );

    // Walk the player onto the region; the next passes fill it.
    teleport(&mut world, conn, Point::new(area.x, area.y, 0));
    for _ in 0..12 {
        world.tick(now);
    }
    assert!(
        world.registry().query::<SpawnedBy>().count() > 0,
        "a player arriving within range wakes the region"
    );
}

// --- Quest seams (MobileUsed, gump, give-item, quest persistence) ---------

#[test]
fn double_clicking_an_npc_fires_mobile_used_and_still_opens_the_paperdoll() {
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let npc = spawn_mobile_at(&mut world, Point::new(START.x, START.y + 1, 0), 50, now);
    let mut used: Cursor<crate::MobileUsed> = world.bus().cursor();
    let _ = packets_for(&mut world, conn);

    world.queue(Command::DoubleClick {
        connection: conn,
        request:    UseRequest::Use(RawSerial(npc.raw())),
    });
    world.tick(now);

    let events: Vec<crate::MobileUsed> = world.bus().read(&mut used).copied().collect();
    assert_eq!(events.len(), 1, "the click reached the pack as MobileUsed");
    assert_eq!(events[0].mobile, npc);
    assert!(
        packets_for(&mut world, conn).iter().any(|p| p[0] == 0x88),
        "and the paperdoll still opened alongside it"
    );
}

#[test]
fn give_item_lands_in_the_players_backpack() {
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let player = world.state.players[&conn];
    let serial = world.registry().serial_of(player).unwrap();
    // The backpack the enter equipped (worn container on the 0x15 layer).
    let backpack = world
        .registry()
        .query::<Equipped>()
        .find(|(item, eq)| {
            eq.mobile == serial
                && eq.layer == items::BACKPACK_LAYER
                && world.registry().has::<Container>(*item)
        })
        .and_then(|(item, _)| world.registry().serial_of(item))
        .expect("a backpack");
    let count = |world: &World| {
        world
            .registry()
            .query::<Contained>()
            .filter(|(_, c)| c.container == backpack)
            .count()
    };
    let before = count(&world);

    world.queue(Command::GiveItem {
        serial,
        graphic: openshard_protocol::wire::Graphic(0x0EED), // gold
        hue: openshard_protocol::wire::Hue(0),
        amount: 100,
        stackable: true,
    });
    world.tick(now);

    assert_eq!(count(&world), before + 1, "the reward is in the backpack");
}

#[test]
fn a_registered_give_item_reward_keeps_its_semantic_identity() {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };

    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let serial = world.registry().serial_of(world.state.players[&conn]).unwrap();
    world.queue(Command::GiveItem {
        serial,
        graphic: Graphic(0x1415), // plate chest
        hue: Hue(0x08ab),         // valorite
        amount: 1,
        stackable: false,
    });
    world.tick(now);

    let reward = world
        .registry()
        .query::<Contained>()
        .find_map(|(item, _)| {
            (world.registry().get::<Drawn>(item)
                == Some(&Drawn {
                    id:  Graphic(0x1415),
                    hue: Hue(0x08ab),
                }))
            .then_some(item)
        })
        .expect("plate reward in backpack");
    assert_eq!(
        world
            .registry()
            .get::<openshard_state::components::ItemKind>(reward),
        Some(&openshard_state::components::ItemKind(ItemKindId(5)))
    );
    assert_eq!(
        world
            .registry()
            .get::<openshard_state::components::Material>(reward),
        Some(&openshard_state::components::Material(MaterialId(9)))
    );
}

#[test]
fn give_item_kind_awards_a_semantic_item_without_art_in_the_command() {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };

    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let serial = world.registry().serial_of(world.state.players[&conn]).unwrap();
    world.queue(Command::GiveItemKind {
        serial,
        item_kind: ItemKindId(4), // longsword
        material: Some(MaterialId(9)),
        amount: 1,
        stackable: false,
    });
    world.tick(now);

    assert!(world.registry().query::<Contained>().any(|(item, _)| {
        world
            .registry()
            .get::<openshard_state::components::ItemKind>(item)
            == Some(&openshard_state::components::ItemKind(ItemKindId(4)))
            && world
                .registry()
                .get::<openshard_state::components::Material>(item)
                == Some(&openshard_state::components::Material(MaterialId(9)))
    }));
}

#[test]
fn give_item_kind_creates_a_functional_backpack() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        Container,
        ItemKind,
    };

    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let serial = world.registry().serial_of(world.state.players[&conn]).unwrap();
    world.queue(Command::GiveItemKind {
        serial,
        item_kind: ItemKindId(7),
        material: None,
        amount: 1,
        stackable: true,
    });
    world.tick(now);

    assert!(world.registry().query::<Contained>().any(|(item, _)| {
        world.registry().get::<ItemKind>(item) == Some(&ItemKind(ItemKindId(7)))
            && world.registry().has::<Container>(item)
    }));
}

#[test]
fn stackable_typed_loot_never_turns_a_backpack_into_a_pile() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        Container,
        ItemKind,
        Stackable,
    };

    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let serial = world
        .registry()
        .serial_of(world.state.players[&connection])
        .unwrap();
    let pack = items::backpack_of(&world.state, serial).unwrap();
    world.queue(Command::AddLootKind {
        container: pack,
        item_kind: ItemKindId(7),
        material:  None,
        amount:    1,
        stackable: true,
    });
    world.tick(now);

    assert!(world.registry().query::<Contained>().any(|(item, held)| {
        held.container == pack
            && world.registry().get::<ItemKind>(item) == Some(&ItemKind(ItemKindId(7)))
            && world.registry().has::<Container>(item)
            && !world.registry().has::<Stackable>(item)
    }));
}

#[test]
fn take_item_is_all_or_nothing_and_reports_what_it_took() {
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let serial = world.registry().serial_of(world.state.players[&conn]).unwrap();
    let mut taken: Cursor<crate::ItemsTaken> = world.bus().cursor();
    // Put five gold in the backpack.
    world.queue(Command::GiveItem {
        serial,
        graphic: openshard_protocol::wire::Graphic(0x0eed),
        hue: openshard_protocol::wire::Hue(0),
        amount: 5,
        stackable: true,
    });
    world.tick(now);
    let backpack_gold = |world: &World| -> u16 {
        world
            .registry()
            .query::<Contained>()
            .filter(|(item, _)| {
                world
                    .registry()
                    .get::<Drawn>(*item)
                    .is_some_and(|g| g.id == openshard_protocol::wire::Graphic(0x0eed))
            })
            .map(|(item, _)| openshard_items::amount_of(&world.state, item))
            .sum()
    };
    assert_eq!(backpack_gold(&world), 5);

    // Take three: enough, so three go and two remain.
    world.queue(Command::TakeItem {
        serial,
        graphic: openshard_protocol::wire::Graphic(0x0eed),
        amount: 3,
    });
    world.tick(now);
    let events: Vec<crate::ItemsTaken> = world.bus().read(&mut taken).copied().collect();
    assert_eq!(
        events.last().map(|e| e.taken),
        Some(3),
        "it reported taking three"
    );
    assert_eq!(events.last().and_then(|e| e.item_kind), None);
    assert_eq!(events.last().and_then(|e| e.material), None);
    assert_eq!(backpack_gold(&world), 2, "two gold remain");

    // Take ten: short, so nothing is taken and the two are kept.
    world.queue(Command::TakeItem {
        serial,
        graphic: openshard_protocol::wire::Graphic(0x0eed),
        amount: 10,
    });
    world.tick(now);
    let events: Vec<crate::ItemsTaken> = world.bus().read(&mut taken).copied().collect();
    assert_eq!(events.last().map(|e| e.taken), Some(0), "short: it took nothing");
    assert_eq!(backpack_gold(&world), 2, "and left the two untouched");
}

#[test]
fn take_item_kind_requires_the_exact_material_not_its_shared_ingot_art() {
    use openshard_protocol::item_kind::{
        ItemKindId,
        MaterialId,
    };

    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let serial = world.registry().serial_of(world.state.players[&conn]).unwrap();
    let mut taken: Cursor<crate::ItemsTaken> = world.bus().cursor();
    assert!(items::give_kind_to_backpack(
        &mut world.state,
        serial,
        ItemKindId(1),
        Some(MaterialId(1)), // iron
        3,
        true,
    ));
    assert!(items::give_kind_to_backpack(
        &mut world.state,
        serial,
        ItemKindId(1),
        Some(MaterialId(9)), // valorite: same ingot graphic, different identity
        4,
        true,
    ));

    world.queue(Command::TakeItemKind {
        serial,
        item_kind: ItemKindId(1),
        material: Some(MaterialId(1)),
        amount: 3,
    });
    world.tick(now);

    let events: Vec<crate::ItemsTaken> = world.bus().read(&mut taken).copied().collect();
    assert_eq!(
        events.last(),
        Some(&crate::ItemsTaken {
            player:    serial,
            graphic:   Graphic(0x1bf2),
            item_kind: Some(ItemKindId(1)),
            material:  Some(MaterialId(1)),
            taken:     3,
        })
    );
    let amounts: Vec<_> = world
        .registry()
        .query::<Contained>()
        .filter_map(|(item, _)| {
            let kind = world
                .registry()
                .get::<openshard_state::components::ItemKind>(item)?;
            let material = world
                .registry()
                .get::<openshard_state::components::Material>(item)?;
            (kind.0 == ItemKindId(1) && material.0 == MaterialId(9))
                .then(|| openshard_items::amount_of(&world.state, item))
        })
        .collect();
    assert_eq!(amounts, vec![4], "valorite pile was not payment for iron");
}

#[test]
fn a_non_admin_gump_reply_reaches_the_pack_as_gump_answered() {
    use openshard_protocol::gump::GumpResponse as WireGumpResponse;

    use crate::events::GumpAnswered;
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let player = world.state.players[&conn];
    let serial = world.registry().serial_of(player).unwrap();
    let mut answered: Cursor<GumpAnswered> = world.bus().cursor();

    // A reply to a *pack* gump — a gump id that is not the engine's admin menu.
    world.queue(Command::GumpResponse {
        connection: conn,
        response:   WireGumpResponse {
            serial:       openshard_protocol::gump::RawGumpKey(serial.raw()),
            gump_id:      openshard_protocol::gump::RawGumpId(0x1234_5678),
            button:       openshard_protocol::gump::RawButtonId(2),
            switches:     vec![],
            text_entries: vec![],
        },
    });
    world.tick(now);

    let events: Vec<GumpAnswered> = world.bus().read(&mut answered).cloned().collect();
    assert_eq!(events.len(), 1, "the pack heard the reply");
    assert_eq!(
        events[0].gump_id,
        openshard_protocol::gump::RawGumpId(0x1234_5678)
    );
    assert_eq!(events[0].button, openshard_protocol::gump::RawButtonId(2));
    assert_eq!(events[0].serial, serial);
}

/// The wire path, end to end: a real `0xBF 0x06` from a real connection, through
/// the dispatch, into the party system, and back out as packets.
///
/// The party crate's own tests call the rules directly, so none of them would
/// notice the `Command::Party` arm being unwired, the subcommand being decoded
/// as `Unknown`, or the target cursor never going up. This one would.
#[test]
fn a_party_forms_over_the_wire_and_talks_to_itself() {
    let mut world = world();
    let now = Instant::now();
    let leader_connection = enter(&mut world, now);
    let member_connection = enter_as(&mut world, ConnectionId::from_raw(4242), now + WALK_INTERVAL);
    world.tick(now + WALK_INTERVAL * 2);
    let leader = world.state.players[&leader_connection];
    let member = world.state.players[&member_connection];
    let _ = world.drain_outbound().count();

    // "Add" raises a cursor rather than acting — the client is asking *who*.
    world.queue(Command::Party {
        connection: leader_connection,
        request:    openshard_protocol::party::PartyRequest::Add,
    });
    world.tick(now + WALK_INTERVAL * 3);
    assert_eq!(
        world.state.take_target(leader),
        Some(openshard_state::TargetPurpose::PartyInvite)
    );

    // The invitation, then the acceptance. The serial on the accept is ignored
    // on purpose — the shard's own `PartyCandidate` is the record.
    openshard_party::invite(&mut world.state, leader, member).expect("a leader may ask");
    world.queue(Command::Party {
        connection: member_connection,
        request:    openshard_protocol::party::PartyRequest::Accept(openshard_protocol::serial::RawSerial(0)),
    });
    world.tick(now + WALK_INTERVAL * 4);
    let party = openshard_party::party_of(&world.state, leader).expect("a party formed");
    assert_eq!(openshard_party::roster(&world.state, party), vec![leader, member]);

    // And a line of chat reaches both of them as a `0xBF 0x06 0x04`.
    let _ = world.drain_outbound().count();
    let mut text = vec![0x04u8];
    text.extend("regroup".encode_utf16().flat_map(u16::to_be_bytes));
    text.extend_from_slice(&[0, 0]);
    let mut packet = vec![0xBF, 0, 0];
    packet.extend_from_slice(&openshard_protocol::party::SUBCOMMAND.to_be_bytes());
    packet.extend_from_slice(&text);
    let length = u16::try_from(packet.len()).unwrap();
    packet[1..3].copy_from_slice(&length.to_be_bytes());
    let request = match openshard_protocol::extended::ExtendedRequest::decode(&packet).unwrap() {
        openshard_protocol::extended::ExtendedRequest::Party(request) => request,
        other => panic!("decoded as {other:?}"),
    };
    world.queue(Command::Party {
        connection: leader_connection,
        request,
    });
    world.tick(now + WALK_INTERVAL * 5);

    let heard: Vec<_> = world
        .drain_outbound()
        .filter(|out| out.packet.first() == Some(&0xBF))
        .filter(|out| {
            out.packet.len() > 5
                && u16::from_be_bytes([out.packet[3], out.packet[4]]) == openshard_protocol::party::SUBCOMMAND
                && out.packet[5] == 0x04
        })
        .map(|out| out.connection)
        .collect();
    assert_eq!(heard.len(), 2, "both members heard it, and nobody else did");
    assert!(heard.contains(&leader_connection));
    assert!(heard.contains(&member_connection));
}

/// The branch in `World::say`, end to end. A guild line arrives as ordinary
/// `0xAD` speech with mode `0x0D`, and the thing worth asserting is what it does
/// **not** do: reach the stranger standing on the next tile.
///
/// Nothing in `openshard-guilds`' own tests would catch the branch being absent
/// — they call `say_to_guild` directly, and a missing branch would send the line
/// through the ordinary broadcast where everybody in earshot hears it.
#[test]
fn a_guild_line_is_not_said_out_loud() {
    let mut world = world();
    let now = Instant::now();
    let speaker_connection = enter(&mut world, now);
    let mate_connection = enter_as(&mut world, ConnectionId::from_raw(4243), now + WALK_INTERVAL);
    let stranger_connection = enter_as(&mut world, ConnectionId::from_raw(4244), now + WALK_INTERVAL * 2);
    world.tick(now + WALK_INTERVAL * 3);
    let speaker = world.state.players[&speaker_connection];
    let mate = world.state.players[&mate_connection];

    let serial = world.registry().serial_of(speaker).unwrap();
    let guild = world
        .state
        .guilds
        .found("The Silver Serpent".to_owned(), "OSS".to_owned(), serial);
    for (who, rank) in [
        (speaker, openshard_state::Rank::Leader),
        (mate, openshard_state::Rank::Member),
    ] {
        world.state.registry.insert(
            who,
            openshard_state::GuildMember {
                guild,
                title: String::new(),
                rank,
            },
        );
    }
    let _ = world.drain_outbound().count();

    world.queue(Command::Say {
        connection: speaker_connection,
        mode:       openshard_protocol::speech::RawTalkMode(
            openshard_protocol::speech::TalkMode::Guild.to_wire(),
        ),
        hue:        openshard_protocol::wire::RawHue(0x3B2),
        font:       openshard_protocol::speech::RawFont(3),
        text:       "regroup".to_owned(),
    });
    world.tick(now + WALK_INTERVAL * 4);

    let heard: Vec<_> = world
        .drain_outbound()
        .filter(|out| out.packet.first() == Some(&0xAE))
        .filter(|out| {
            String::from_utf16_lossy(
                &out.packet
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                    .collect::<Vec<_>>(),
            )
            .contains("regroup")
        })
        .map(|out| out.connection)
        .collect();
    assert!(heard.contains(&speaker_connection));
    assert!(heard.contains(&mate_connection));
    assert!(
        !heard.contains(&stranger_connection),
        "the stranger is standing right there and must not have heard it"
    );
}

/// `.house` puts a house at the operator's feet, and its walls stop people.
///
/// Through the command rather than through `openshard_housing::place`, because
/// the crate's own tests already cover the arithmetic and what this adds is the
/// two seams they cannot see: the command reaching the crate at all, and a
/// terrain with no multi table answering "no such multi" rather than panicking.
#[test]
fn the_house_command_is_refused_on_a_shard_with_no_multis() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let _ = packets_for(&mut world, player);

    // The test world has no terrain at all, which is the same answer a shard
    // whose install predates the multi format gives — and the point is that it
    // is *an answer*, said out loud, rather than a house with no walls.
    gm::run(&mut world.state, entity, "house 0x64");
    world.tick(now);
    let said = packets_for(&mut world, player);
    assert!(
        said.iter()
            .any(|packet| String::from_utf8_lossy(packet).contains("No house has that number")),
        "a shard with no multis placed a house anyway, or said nothing"
    );
    assert_eq!(
        world
            .registry()
            .query::<openshard_state::components::House>()
            .count(),
        0
    );

    // And a malformed one answers rather than doing nothing, `.skill`'s rule.
    gm::run(&mut world.state, entity, "house");
    world.tick(now);
    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|packet| String::from_utf8_lossy(packet).contains("Usage: .house")),
        "a bare .house said nothing"
    );
}

/// A deed raises the `0x99` cursor, and answering it builds the house and spends
/// the deed.
///
/// The whole H2 loop through the tick, because every link in it fails quietly:
/// a deed that raises a `0x6C` instead leaves the player picking a plot blind, a
/// cursor whose purpose is dropped answers nothing, and a placement that forgets
/// to consume the deed hands out free houses.
#[test]
fn a_deed_raises_the_house_cursor_and_answering_it_builds() {
    use openshard_state::components::HouseDeed;
    use openshard_uofiles::multi::Component;

    const COTTAGE: u16 = 0x64;
    const WALL: u16 = 0x0006;
    /// One wall, one tile east — the cottage the tables below describe.
    fn cottage() -> Vec<Component> {
        vec![Component {
            graphic: Graphic(WALL),
            dx:      1,
            dy:      0,
            dz:      0,
            flags:   1,
        }]
    }

    let now = Instant::now();
    let mut world = world();
    world.state.set_tiles(tiles_with(&[(WALL, WALL_FLAGS, 20)]));
    world.state.multis = multis_with(COTTAGE, cottage());
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];

    gm::run(&mut world.state, player, "deed 0x64");
    world.tick(now);
    let deed = world
        .state
        .registry
        .query::<HouseDeed>()
        .map(|(entity, _)| entity)
        .next()
        .expect("the command made a deed");
    // Into the pack: a deed on the ground is not one you hold, which the
    // placement re-checks.
    let owner = world.state.registry.serial_of(player).unwrap();
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
    openshard_state::relocate_item(
        &mut world.state,
        deed,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack,
            position:  GumpPoint::new(20, 20),
            grid:      openshard_protocol::containers::GridSlot(0),
        }),
    )
    .unwrap();
    let deed_serial = world.state.registry.serial_of(deed).unwrap();
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(deed_serial.raw())),
    });
    world.tick(now);
    let raised = packets_for(&mut world, connection);
    assert!(
        raised.iter().any(|packet| packet[0] == 0x99),
        "a deed raised no house cursor, so the player picks a plot blind"
    );

    let at = Point::new(START.x + 6, START.y + 6, 0);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(owner.raw()),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  at,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert_eq!(
        world
            .registry()
            .query::<openshard_state::components::House>()
            .count(),
        1,
        "answering the cursor built no house"
    );
    assert!(
        world.state.registry.entity_of(deed_serial).is_none(),
        "the deed was not spent, so one deed builds a village"
    );
    assert!(
        world
            .state
            .facet_state(Facet(0))
            .obstructions()
            .blocker_at_z(at.x + 1, at.y, 0)
            .is_some(),
        "the house has no walls"
    );
}

/// An addon supplied as content (admin-created, not crafted) carries its
/// `ItemKind` from spawn but gains the transient `AddonDeed` component only on
/// first use. Its first double-click has to recover that identity, raise the
/// ordinary tile cursor, and leave the oven locked down in the house.
///
/// Both elven facings are placed here because the facing *is* the whole
/// difference between them: one tile at the origin either way, and only the
/// graphic says which oven stands there.
#[test]
fn an_elven_oven_deed_places_the_single_component_its_facing_names() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        AddonDeed,
        AddonKind,
        House,
        LockedDown,
    };
    use openshard_uofiles::multi::Component;

    const COTTAGE: u16 = 0x64;
    const WALL: u16 = 0x0006;
    const FLOOR: u16 = 0x0007;
    // The elven oven's single tile is the *floor* offset: non-blocking, so the
    // real collision check `place_addon_from_deed` runs finds it clear. A wall
    // tile beside it keeps the house's own footprint non-empty
    // (`Refusal::DrawsNothing`) without standing where the oven goes. Two floor
    // tiles, because the two facings are placed one beside the other and a
    // locked-down oven refuses to share a tile with another.
    fn cottage() -> Vec<Component> {
        vec![
            Component {
                graphic: Graphic(WALL),
                dx:      -1,
                dy:      0,
                dz:      0,
                flags:   1,
            },
            Component {
                graphic: Graphic(FLOOR),
                dx:      0,
                dy:      0,
                dz:      0,
                flags:   1,
            },
            Component {
                graphic: Graphic(FLOOR),
                dx:      0,
                dy:      1,
                dz:      0,
                flags:   1,
            },
        ]
    }

    let now = Instant::now();
    let mut world = world();
    world
        .state
        .set_tiles(tiles_with(&[(WALL, WALL_FLAGS, 20), (FLOOR, 0, 0)]));
    world.state.multis = multis_with(COTTAGE, cottage());
    let connection = enter_gm(&mut world, now);
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).expect("a player serial");
    let at = Point::new(START.x, START.y, 0);

    gm::run(&mut world.state, player, "house 0x64");
    world.tick(now);
    assert_eq!(
        world.state.registry.query::<House>().count(),
        1,
        "the test house is absent"
    );

    let deed = items::spawn_item_kind(
        &mut world.state,
        ItemKindId(112), // AddonKind::ElvenOvenSouth's deed kind
        None,
        1,
        false,
        at,
        Facet(0),
    )
    .expect("an elven oven deed");
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
    openshard_state::relocate_item(
        &mut world.state,
        deed,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack,
            position:  GumpPoint::new(20, 20),
            grid:      GridSlot(0),
        }),
    )
    .expect("the deed goes into the owner's backpack");
    let deed_serial = world.state.registry.serial_of(deed).expect("a deed serial");
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(deed_serial.raw())),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|packet| packet[0] == 0x6C),
        "an admin-created elven oven deed raised no location cursor"
    );
    assert_eq!(
        world.state.registry.get::<AddonDeed>(deed),
        Some(&AddonDeed {
            addon: AddonKind::ElvenOvenSouth,
        }),
        "the deed did not acquire its placement identity"
    );

    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(owner.raw()),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  at,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert!(
        world.state.registry.entity_of(deed_serial).is_none(),
        "placing the oven did not spend its deed"
    );
    assert!(
        world.state.registry.query::<Drawn>().any(|(item, drawn)| {
            drawn.id == Graphic(0x2DDC) && world.state.registry.has::<LockedDown>(item)
        }),
        "the elven oven's locked-down component is absent"
    );

    // The east facing, on the floor tile beside it: a different registered kind
    // and a different graphic, everything else identical.
    let beside = Point::new(at.x, at.y + 1, at.z);
    let east = items::spawn_item_kind(
        &mut world.state,
        ItemKindId(113), // AddonKind::ElvenOvenEast's deed kind
        None,
        1,
        false,
        at,
        Facet(0),
    )
    .expect("an elven oven east deed");
    openshard_state::relocate_item(
        &mut world.state,
        east,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack,
            position:  GumpPoint::new(20, 40),
            grid:      GridSlot(1),
        }),
    )
    .expect("the deed goes into the owner's backpack");
    let east_serial = world.state.registry.serial_of(east).expect("a deed serial");
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(east_serial.raw())),
    });
    world.tick(now);
    assert_eq!(
        world.state.registry.get::<AddonDeed>(east),
        Some(&AddonDeed {
            addon: AddonKind::ElvenOvenEast,
        }),
        "the east deed did not acquire its placement identity"
    );
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(owner.raw()),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  beside,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);
    assert!(
        world.state.registry.query::<Drawn>().any(|(item, drawn)| {
            drawn.id == Graphic(0x2DDB)
                && world
                    .state
                    .registry
                    .get::<openshard_state::components::Position>(item)
                    == Some(&openshard_state::components::Position(beside))
                && world.state.registry.has::<LockedDown>(item)
        }),
        "the east-facing elven oven is absent, or drawn as the south one"
    );
}

/// A crafted (not admin-created) stone oven deed installs both of its
/// components at the offsets `data/deco_addons.json` gives `StoneOvenEastAddon`
/// — the geometry `place_addon_from_deed` now reads through
/// `decoration::addon_components` instead of a second, hand-kept copy.
#[test]
fn a_crafted_stone_oven_east_deed_places_both_locked_down_components() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        AddonDeed,
        AddonKind,
        House,
        LockedDown,
    };
    use openshard_uofiles::multi::Component;

    const COTTAGE: u16 = 0x64;
    const WALL: u16 = 0x0006;
    const FLOOR: u16 = 0x0007;
    // Both tiles a stone-oven-east occupies (offsets `(0, 0)` and `(0, 1)`) must
    // be inside the house footprint for the placement to be accepted — and, since
    // `place_addon_from_deed` now runs a real collision check, actually clear to
    // stand on: flat, non-blocking floor rather than a wall. One further-off wall
    // tile keeps the house's own footprint non-empty (`Refusal::DrawsNothing`)
    // without standing where the oven goes.
    fn cottage() -> Vec<Component> {
        vec![
            Component {
                graphic: Graphic(WALL),
                dx:      -1,
                dy:      0,
                dz:      0,
                flags:   1,
            },
            Component {
                graphic: Graphic(FLOOR),
                dx:      0,
                dy:      0,
                dz:      0,
                flags:   1,
            },
            Component {
                graphic: Graphic(FLOOR),
                dx:      0,
                dy:      1,
                dz:      0,
                flags:   1,
            },
        ]
    }

    let now = Instant::now();
    let mut world = world();
    world
        .state
        .set_tiles(tiles_with(&[(WALL, WALL_FLAGS, 20), (FLOOR, 0, 0)]));
    world.state.multis = multis_with(COTTAGE, cottage());
    let connection = enter_gm(&mut world, now);
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).expect("a player serial");
    let at = Point::new(START.x, START.y, 0);

    gm::run(&mut world.state, player, "house 0x64");
    world.tick(now);
    assert_eq!(
        world.state.registry.query::<House>().count(),
        1,
        "the test house is absent"
    );

    let deed = items::spawn_item_kind(
        &mut world.state,
        ItemKindId(110), // AddonKind::StoneOvenEast's deed kind
        None,
        1,
        false,
        at,
        Facet(0),
    )
    .expect("a stone oven east deed");
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
    openshard_state::relocate_item(
        &mut world.state,
        deed,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack,
            position:  GumpPoint::new(20, 20),
            grid:      GridSlot(0),
        }),
    )
    .expect("the deed goes into the owner's backpack");
    let deed_serial = world.state.registry.serial_of(deed).expect("a deed serial");
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(deed_serial.raw())),
    });
    world.tick(now);
    assert_eq!(
        world.state.registry.get::<AddonDeed>(deed),
        Some(&AddonDeed {
            addon: AddonKind::StoneOvenEast,
        }),
        "the crafted deed did not carry its placement identity"
    );

    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(owner.raw()),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  at,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    assert!(
        world.state.registry.entity_of(deed_serial).is_none(),
        "placing the oven did not spend its deed"
    );
    for (graphic, at) in [
        (Graphic(0x092C), at),
        (Graphic(0x092B), Point::new(at.x, at.y + 1, at.z)),
    ] {
        assert!(
            world.state.registry.query::<Drawn>().any(|(item, drawn)| {
                drawn.id == graphic
                    && world
                        .state
                        .registry
                        .get::<openshard_state::components::Position>(item)
                        == Some(&openshard_state::components::Position(at))
                    && world.state.registry.has::<LockedDown>(item)
            }),
            "the stone oven east's {graphic:?} component at {at} is absent"
        );
    }
}

/// A cottage with a crafted stone oven east installed in it, through the whole
/// player-facing path: deed in the pack, double-click, location cursor answered.
///
/// Written once for the two tests below, which are about what happens to an
/// **installed** oven rather than about installing one — the placement path
/// itself has its own test above.
fn a_house_with_a_stone_oven(now: Instant) -> (World, ConnectionId, EntityId, EntityId) {
    // AddonKind::StoneOvenEast's deed kind.
    a_house_with_the_addon(now, openshard_protocol::item_kind::ItemKindId(110))
}

/// A GM in a cottage with one installed addon of `deed_kind`, however many tiles
/// it has: the fixture above, generalized when the loom and the spinning wheel
/// wanted exactly the same house.
///
/// The cottage's two floor tiles at `(0, 0)` and `(0, 1)` fit every addon this
/// engine installs — the two-tile ovens and looms lie along that axis, and the
/// wheels are one tile at the origin.
fn a_house_with_the_addon(
    now: Instant,
    deed_kind: openshard_protocol::item_kind::ItemKindId,
) -> (World, ConnectionId, EntityId, EntityId) {
    use openshard_state::components::House;
    use openshard_uofiles::multi::Component;

    const COTTAGE: u16 = 0x64;
    const WALL: u16 = 0x0006;
    const FLOOR: u16 = 0x0007;
    // Real floor under both of the oven's tiles, and one wall elsewhere so the
    // house's own footprint is not empty — the crafted-placement test's fixture,
    // and for the reasons written there.
    let cottage = vec![
        Component {
            graphic: Graphic(WALL),
            dx:      -1,
            dy:      0,
            dz:      0,
            flags:   1,
        },
        Component {
            graphic: Graphic(FLOOR),
            dx:      0,
            dy:      0,
            dz:      0,
            flags:   1,
        },
        Component {
            graphic: Graphic(FLOOR),
            dx:      0,
            dy:      1,
            dz:      0,
            flags:   1,
        },
    ];

    let mut world = world();
    world
        .state
        .set_tiles(tiles_with(&[(WALL, WALL_FLAGS, 20), (FLOOR, 0, 0)]));
    world.state.multis = multis_with(COTTAGE, cottage);
    let connection = enter_gm(&mut world, now);
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).expect("a player serial");
    let at = Point::new(START.x, START.y, 0);

    gm::run(&mut world.state, player, "house 0x64");
    world.tick(now);
    let house = world
        .state
        .registry
        .query::<House>()
        .next()
        .map(|(entity, _)| entity)
        .expect("the test house is absent");

    let deed = items::spawn_item_kind(&mut world.state, deed_kind, None, 1, false, at, Facet(0))
        .expect("the addon's deed");
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
    openshard_state::relocate_item(
        &mut world.state,
        deed,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack,
            position:  GumpPoint::new(20, 20),
            grid:      GridSlot(0),
        }),
    )
    .expect("the deed goes into the owner's backpack");
    let deed_serial = world.state.registry.serial_of(deed).expect("a deed serial");
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(deed_serial.raw())),
    });
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(owner.raw()),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  at,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);
    (world, connection, player, house)
}

/// Releasing one tile of an installed oven takes the **whole** oven down and
/// puts its deed back in the pack.
///
/// Before the addon grouping existed, a release unpinned exactly the component
/// it was aimed at: half an oven left standing, nothing refunded, and no way to
/// reassemble it (docs/crafting.md's review, point 3). The rule copied here is
/// ServUO's `BaseAddon`, which deletes itself whole and re-deeds.
///
/// The release is aimed at the **second** component on purpose — the group is
/// named by the first one's serial, so aiming at the root would pass even if the
/// sweep only ever looked at the item in hand.
#[test]
fn releasing_one_stone_oven_component_takes_the_whole_oven_and_returns_its_deed() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::HouseStorage;
    use openshard_state::components::{
        AddonKind,
        AddonPart,
        ItemKind,
        Position,
    };

    let now = Instant::now();
    let (mut world, _connection, player, house) = a_house_with_a_stone_oven(now);
    let owner = world.state.registry.serial_of(player).expect("a player serial");
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");

    let mut components = openshard_housing::storage::locked_down(&world.state, house);
    assert_eq!(components.len(), 2, "the oven did not install both components");
    // The far tile — `(0, 1)` — is the one that is not the group's root.
    components.sort_by_key(|&item| {
        world
            .state
            .registry
            .get::<Position>(item)
            .map(|&Position(point)| point.y)
    });
    let far = *components.last().expect("two components");
    let root = world
        .state
        .registry
        .get::<AddonPart>(far)
        .expect("an installed component belongs to an addon")
        .root;
    assert_eq!(
        world.state.registry.get::<AddonPart>(far).map(|part| part.addon),
        Some(AddonKind::StoneOvenEast),
        "the component does not name the addon it is part of"
    );
    assert_ne!(
        world.state.registry.serial_of(far),
        Some(root),
        "the far component is the group's root, so this test would not prove the sweep"
    );
    let far_serial = world.state.registry.serial_of(far).expect("a component serial");

    world.change_house_storage(player, house, HouseStorage::Release, Some(far_serial));

    assert!(
        openshard_housing::storage::locked_down(&world.state, house).is_empty(),
        "releasing one component left the rest of the oven locked down"
    );
    for component in components {
        assert!(
            world.state.registry.get::<Position>(component).is_none(),
            "a released oven component is still standing in the world"
        );
    }
    let deeds = openshard_state::contained_items(&world.state, backpack)
        .filter(|&(item, _)| world.state.registry.get::<ItemKind>(item) == Some(&ItemKind(ItemKindId(110))))
        .count();
    assert_eq!(
        deeds, 1,
        "the released oven did not come back as one deed in the pack"
    );
}

/// The grouping is saved and restored, so a restart does not turn an oven back
/// into two unrelated locked-down graphics.
///
/// Nothing else on disk says which components were ever one addon — the tiles a
/// second oven could legally stand on are the same tiles — so an unsaved
/// grouping is one that cannot be re-derived at boot, and the release rule
/// silently reverts to the half-an-oven behaviour it replaced.
#[test]
fn an_installed_oven_keeps_its_grouping_across_a_save_and_restore() {
    use openshard_state::components::{
        AddonKind,
        AddonPart,
    };

    let now = Instant::now();
    let (home, _connection, _player, house) = a_house_with_a_stone_oven(now);
    let components = openshard_housing::storage::locked_down(&home.state, house);
    assert_eq!(components.len(), 2, "the oven did not install both components");
    let saved: Vec<_> = components
        .iter()
        .map(|&item| {
            home.state
                .registry
                .get::<AddonPart>(item)
                .copied()
                .expect("an installed component belongs to an addon")
        })
        .collect();

    let records = home.ground_items();
    let mut shard = world();
    let characters = shard.restore_characters(Vec::new());
    shard.restore_items(records, &characters);

    for (component, part) in components.iter().zip(saved) {
        let serial = home
            .state
            .registry
            .serial_of(*component)
            .expect("a component serial");
        let restored = shard
            .state
            .registry
            .entity_of(serial)
            .expect("the oven component is back on its serial");
        assert_eq!(
            shard.state.registry.get::<AddonPart>(restored),
            Some(&part),
            "the restored component forgot which oven it is part of"
        );
        assert_eq!(part.addon, AddonKind::StoneOvenEast);
    }
}

/// AddonKind::SpinningWheelSouth's deed kind.
const SPINNING_WHEEL_SOUTH_DEED: openshard_protocol::item_kind::ItemKindId =
    openshard_protocol::item_kind::ItemKindId(118);
/// AddonKind::LoomEast's deed kind.
const LOOM_EAST_DEED: openshard_protocol::item_kind::ItemKindId =
    openshard_protocol::item_kind::ItemKindId(115);

/// A pile of cotton, `0xDF9`.
const COTTON: Graphic = Graphic(0x0DF9);
/// A spool of thread, `0xFA0` — what cotton spins into.
const THREAD: Graphic = Graphic(0x0FA0);
/// A bolt of cloth, `0xF95` — what a loom weaves.
const BOLT: Graphic = Graphic(0x0F95);
/// Cut cloth, `0x1766` — what fifty-six tailoring rows actually eat.
const CLOTH: Graphic = Graphic(0x1766);
/// A spinning wheel south at rest, and turning.
const WHEEL_IDLE: Graphic = Graphic(0x1015);
/// The turning art, which is `WHEEL_IDLE + 1` in ServUO and written out here so
/// the test does not restate the implementation's arithmetic.
const WHEEL_TURNING: Graphic = Graphic(0x1016);
/// An arbitrary dye, carried the length of the chain.
const DYE: Hue = Hue(0x0021);
/// Long enough for a six-second spin at forty ticks a second, with slack.
const SPIN_LIMIT: u64 = 6 * 40 + 20;

/// Everything of one art in a container, as `(hue, amount)`.
fn piles_of(world: &World, container: Serial, graphic: Graphic) -> Vec<(Hue, u16)> {
    world
        .state
        .registry
        .query::<Contained>()
        .filter(|(_, contained)| contained.container == container)
        .filter_map(|(item, _)| {
            let drawn = world.state.registry.get::<Drawn>(item)?;
            (drawn.id == graphic).then(|| {
                (
                    drawn.hue,
                    world
                        .state
                        .registry
                        .get::<Amount>(item)
                        .map_or(1, |amount| amount.0),
                )
            })
        })
        .collect()
}

/// Point an item at another one through the two packets a player sends: the
/// double-click that raises the cursor, and the reply that names the target.
fn use_item_on(
    world: &mut World,
    connection: ConnectionId,
    owner: Serial,
    item: Serial,
    target: Serial,
    now: Instant,
) {
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(item.raw())),
    });
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(owner.raw()),
            object:    Some(target),
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);
}

/// The first step of the cloth chain, end to end: a pile of cotton onto an
/// installed spinning wheel, six seconds of turning, six spools of thread.
///
/// Cotton is **dyed** on purpose. ServUO carries the fibre's hue onto the yarn
/// (`BeginSpin(..., this.m_Cotton.Hue)`), and a spin that dropped it would still
/// pass with plain cotton while quietly turning every dyed pile on the shard
/// into undyed thread — the same failure shape the hides' grade has.
#[test]
fn a_spinning_wheel_turns_a_pile_of_cotton_into_thread() {
    use openshard_state::components::Spinning;

    let now = Instant::now();
    let (mut world, connection, player, house) = a_house_with_the_addon(now, SPINNING_WHEEL_SOUTH_DEED);
    let owner = world.state.registry.serial_of(player).expect("a player serial");
    let pack = items::backpack_of(&world.state, owner).expect("a backpack");

    let components = openshard_housing::storage::locked_down(&world.state, house);
    assert_eq!(components.len(), 1, "a spinning wheel is one tile");
    let wheel = components[0];
    let wheel_serial = world.state.registry.serial_of(wheel).expect("a wheel serial");
    assert_eq!(
        world.state.registry.get::<Drawn>(wheel).map(|drawn| drawn.id),
        Some(WHEEL_IDLE),
        "a freshly installed wheel is not at rest"
    );

    let cotton = items::give(&mut world.state, pack, COTTON, DYE, 1)
        .last
        .expect("a pile of cotton");
    let cotton_serial = world.state.registry.serial_of(cotton).expect("a cotton serial");
    use_item_on(&mut world, connection, owner, cotton_serial, wheel_serial, now);

    assert!(
        world.state.registry.entity_of(cotton_serial).is_none(),
        "the wheel did not take the cotton"
    );
    assert!(
        world.state.registry.has::<Spinning>(wheel),
        "the wheel is not turning"
    );
    assert_eq!(
        world.state.registry.get::<Drawn>(wheel).map(|drawn| drawn.id),
        Some(WHEEL_TURNING),
        "a turning wheel still draws the resting art"
    );
    assert!(
        piles_of(&world, pack, THREAD).is_empty(),
        "the thread arrived before the wheel had turned"
    );

    tick_until(&mut world, now, SPIN_LIMIT, |world| {
        !world.state.registry.has::<Spinning>(wheel)
    });

    assert!(
        !world.state.registry.has::<Spinning>(wheel),
        "the wheel never stopped"
    );
    assert_eq!(
        world.state.registry.get::<Drawn>(wheel).map(|drawn| drawn.id),
        Some(WHEEL_IDLE),
        "a stopped wheel is still drawn turning"
    );
    assert_eq!(
        piles_of(&world, pack, THREAD),
        vec![(DYE, 6)],
        "one dyed pile of cotton is six spools of thread in the same dye"
    );
}

/// A wheel already turning refuses a second pile — ServUO's `wheel.Spinning`
/// gate, which is the only thing stopping a player from feeding one wheel the
/// whole pack in a single tick.
///
/// The refusal that matters is the **material** one, not the message: without
/// the gate the second pile is consumed and its thread never arrives, because
/// the second `Spinning` simply overwrites the first.
#[test]
fn a_turning_spinning_wheel_refuses_a_second_pile() {
    use openshard_state::components::Spinning;

    let now = Instant::now();
    let (mut world, connection, player, house) = a_house_with_the_addon(now, SPINNING_WHEEL_SOUTH_DEED);
    let owner = world.state.registry.serial_of(player).expect("a player serial");
    let pack = items::backpack_of(&world.state, owner).expect("a backpack");
    let wheel = openshard_housing::storage::locked_down(&world.state, house)[0];
    let wheel_serial = world.state.registry.serial_of(wheel).expect("a wheel serial");

    // Two units in one pile: the first spin takes one and leaves the other,
    // which is the pile the busy wheel has to refuse.
    let cotton = items::give(&mut world.state, pack, COTTON, DYE, 2)
        .last
        .expect("a pile of cotton");
    let cotton_serial = world.state.registry.serial_of(cotton).expect("a cotton serial");
    use_item_on(&mut world, connection, owner, cotton_serial, wheel_serial, now);
    let started = world
        .state
        .registry
        .get::<Spinning>(wheel)
        .copied()
        .expect("the wheel took the first pile");

    use_item_on(&mut world, connection, owner, cotton_serial, wheel_serial, now);
    assert_eq!(
        piles_of(&world, pack, COTTON),
        vec![(DYE, 1)],
        "a busy wheel ate the second pile"
    );
    assert_eq!(
        world.state.registry.get::<Spinning>(wheel),
        Some(&started),
        "the second pile restarted the wheel's timer"
    );
}

/// The second step: five spools of thread on a loom, four that only load it and
/// a fifth that comes off as a bolt of cloth.
///
/// Aimed at the loom's **second** tile on purpose. The group is named by the
/// first component's serial, so a weave that only accepted the root would pass
/// here if the test clicked the root — and a player clicks whichever half of the
/// loom is under the cursor.
#[test]
fn a_loom_takes_five_spools_and_pays_a_bolt_of_cloth() {
    use openshard_state::components::{
        AddonPart,
        LoomPhase,
    };

    let now = Instant::now();
    let (mut world, connection, player, house) = a_house_with_the_addon(now, LOOM_EAST_DEED);
    let owner = world.state.registry.serial_of(player).expect("a player serial");
    let pack = items::backpack_of(&world.state, owner).expect("a backpack");

    let components = openshard_housing::storage::locked_down(&world.state, house);
    assert_eq!(components.len(), 2, "a loom is two tiles");
    let root = components
        .iter()
        .copied()
        .find(|&item| {
            world.state.registry.serial_of(item)
                == world.state.registry.get::<AddonPart>(item).map(|part| part.root)
        })
        .expect("one component names itself as the root");
    let leaf = components
        .iter()
        .copied()
        .find(|&item| item != root)
        .expect("the loom's other tile");
    let leaf_serial = world.state.registry.serial_of(leaf).expect("a component serial");

    let spools = items::give(&mut world.state, pack, THREAD, DYE, 5)
        .last
        .expect("five spools of thread");
    let spools_serial = world.state.registry.serial_of(spools).expect("a thread serial");

    for loaded in 1..=4 {
        use_item_on(&mut world, connection, owner, spools_serial, leaf_serial, now);
        assert_eq!(
            world.state.registry.get::<LoomPhase>(root),
            Some(&LoomPhase(loaded)),
            "the loom did not take spool {loaded}"
        );
        assert!(
            piles_of(&world, pack, BOLT).is_empty(),
            "the loom wove a bolt out of {loaded} spools"
        );
    }
    assert_eq!(
        piles_of(&world, pack, THREAD),
        vec![(DYE, 1)],
        "four spools were not spent loading the loom"
    );

    use_item_on(&mut world, connection, owner, spools_serial, leaf_serial, now);
    assert_eq!(
        piles_of(&world, pack, BOLT),
        vec![(DYE, 1)],
        "the fifth spool did not weave a bolt in the thread's own dye"
    );
    assert!(
        piles_of(&world, pack, THREAD).is_empty(),
        "the fifth spool was not spent"
    );
    assert!(
        !world.state.registry.has::<LoomPhase>(root),
        "the woven loom is still loaded"
    );
}

/// The last step: scissors turn a bolt into the fifty cloth a tailor spends.
///
/// Two bolts, so the multiplication is tested rather than the identity, and dyed
/// for the reason the cotton is: ServUO's `ScissorHelper` carries the hue, and a
/// cut that dropped it would bleach every dyed bolt on the shard.
#[test]
fn scissors_cut_a_bolt_of_cloth_into_fifty() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let Position(at) = *world.registry().get::<Position>(player).unwrap();
    let owner = world.registry().serial_of(player).unwrap();
    let pack = items::backpack_of(&world.state, owner).unwrap();

    let scissors = items::spawn_item(&mut world.state, Graphic(0x0F9F), Hue(0), 1, false, at, Facet(0))
        .expect("a pair of scissors");
    let scissors_serial = world.registry().serial_of(scissors).unwrap();
    let bolts = items::give(&mut world.state, pack, BOLT, DYE, 2)
        .last
        .expect("two bolts of cloth");
    let bolts_serial = world.registry().serial_of(bolts).unwrap();

    use_item_on(&mut world, connection, owner, scissors_serial, bolts_serial, now);

    assert_eq!(
        piles_of(&world, pack, CLOTH),
        vec![(DYE, 100)],
        "two bolts did not cut into a hundred cloth of the same dye"
    );
    assert!(
        piles_of(&world, pack, BOLT).is_empty(),
        "the bolts were not spent"
    );
}

/// A part-loaded loom keeps its count across a restart.
///
/// Unlike the wheel's timer, this one is **already paid for**: those spools are
/// gone. A restart that forgot the count would charge the weaver for them twice,
/// which is why `loom_phase` is a saved column and `Spinning` is not.
#[test]
fn a_half_loaded_loom_keeps_its_count_across_a_save_and_restore() {
    use openshard_state::components::{
        AddonPart,
        LoomPhase,
    };

    let now = Instant::now();
    let (mut home, connection, player, house) = a_house_with_the_addon(now, LOOM_EAST_DEED);
    let owner = home.state.registry.serial_of(player).expect("a player serial");
    let pack = items::backpack_of(&home.state, owner).expect("a backpack");
    let root = openshard_housing::storage::locked_down(&home.state, house)
        .into_iter()
        .find(|&item| {
            home.state.registry.serial_of(item)
                == home.state.registry.get::<AddonPart>(item).map(|part| part.root)
        })
        .expect("the loom's root component");
    let root_serial = home.state.registry.serial_of(root).expect("a root serial");

    let spools = items::give(&mut home.state, pack, THREAD, Hue(0), 3)
        .last
        .expect("three spools of thread");
    let spools_serial = home.state.registry.serial_of(spools).expect("a thread serial");
    for _ in 0..3 {
        use_item_on(&mut home, connection, owner, spools_serial, root_serial, now);
    }
    assert_eq!(
        home.state.registry.get::<LoomPhase>(root),
        Some(&LoomPhase(3)),
        "the loom did not take three spools"
    );

    let records = home.ground_items();
    let mut shard = world();
    let characters = shard.restore_characters(Vec::new());
    shard.restore_items(records, &characters);
    let restored = shard
        .state
        .registry
        .entity_of(root_serial)
        .expect("the loom is back on its serial");
    assert_eq!(
        shard.state.registry.get::<LoomPhase>(restored),
        Some(&LoomPhase(3)),
        "the restored loom forgot the three spools it had already eaten"
    );
}

/// A shard that went down mid-spin comes back up with the wheel at rest.
///
/// The save records whatever art the tile was wearing, and the six-second timer
/// is deliberately not saved — so without ServUO's own `OnComponentLoaded`
/// normalization the restored wheel would draw itself turning forever and never
/// pay anybody.
#[test]
fn a_restored_spinning_wheel_is_not_left_turning() {
    use openshard_state::components::Spinning;

    let now = Instant::now();
    let (mut home, connection, player, house) = a_house_with_the_addon(now, SPINNING_WHEEL_SOUTH_DEED);
    let owner = home.state.registry.serial_of(player).expect("a player serial");
    let pack = items::backpack_of(&home.state, owner).expect("a backpack");
    let wheel = openshard_housing::storage::locked_down(&home.state, house)[0];
    let wheel_serial = home.state.registry.serial_of(wheel).expect("a wheel serial");

    let cotton = items::give(&mut home.state, pack, COTTON, Hue(0), 1)
        .last
        .expect("a pile of cotton");
    let cotton_serial = home.state.registry.serial_of(cotton).expect("a cotton serial");
    use_item_on(&mut home, connection, owner, cotton_serial, wheel_serial, now);
    assert!(
        home.state.registry.has::<Spinning>(wheel),
        "the wheel is not turning, so the save proves nothing"
    );

    let records = home.ground_items();
    let mut shard = world();
    let characters = shard.restore_characters(Vec::new());
    shard.restore_items(records, &characters);
    let restored = shard
        .state
        .registry
        .entity_of(wheel_serial)
        .expect("the wheel is back on its serial");
    assert_eq!(
        shard.state.registry.get::<Drawn>(restored).map(|drawn| drawn.id),
        Some(WHEEL_IDLE),
        "the restored wheel is still drawn turning, with no timer to stop it"
    );
    assert!(
        !shard.state.registry.has::<Spinning>(restored),
        "a restored wheel invented a spin"
    );
}

/// A second oven cannot be planted on top of the first.
///
/// An ordinary locked-down item never registers itself in the facet's
/// obstruction index the way a wall or a door does (see
/// `World::addon_tile_is_free`'s doc, and docs/crafting.md's review, point 3),
/// so without a direct check against the house's own storage list a second
/// deed would stack invisibly on the components the first one already placed.
#[test]
fn a_second_stone_oven_east_refuses_to_stack_on_the_first() {
    use openshard_protocol::item_kind::ItemKindId;
    use openshard_state::components::{
        AddonDeed,
        House,
    };
    use openshard_uofiles::multi::Component;

    const COTTAGE: u16 = 0x64;
    const WALL: u16 = 0x0006;
    const FLOOR: u16 = 0x0007;
    fn cottage() -> Vec<Component> {
        vec![
            Component {
                graphic: Graphic(WALL),
                dx:      -1,
                dy:      0,
                dz:      0,
                flags:   1,
            },
            Component {
                graphic: Graphic(FLOOR),
                dx:      0,
                dy:      0,
                dz:      0,
                flags:   1,
            },
            Component {
                graphic: Graphic(FLOOR),
                dx:      0,
                dy:      1,
                dz:      0,
                flags:   1,
            },
        ]
    }

    /// Craft, carry, double-click and answer the location cursor at `at` — one
    /// full placement attempt. Returns the deed's serial so the caller can tell
    /// whether it was spent.
    fn place_stone_oven_east_at(
        world: &mut World,
        connection: ConnectionId,
        owner: Serial,
        at: Point,
        now: Instant,
    ) -> Serial {
        let deed = items::spawn_item_kind(
            &mut world.state,
            ItemKindId(110), // AddonKind::StoneOvenEast's deed kind
            None,
            1,
            false,
            at,
            Facet(0),
        )
        .expect("a stone oven east deed");
        let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
        openshard_state::relocate_item(
            &mut world.state,
            deed,
            openshard_state::ItemLocation::contained(Contained {
                container: backpack,
                position:  GumpPoint::new(20, 20),
                grid:      GridSlot(0),
            }),
        )
        .expect("the deed goes into the owner's backpack");
        let deed_serial = world.state.registry.serial_of(deed).expect("a deed serial");
        let _ = packets_for(world, connection);
        world.queue(Command::DoubleClick {
            connection,
            request: UseRequest::Use(RawSerial(deed_serial.raw())),
        });
        world.tick(now);
        world.queue(Command::TargetResponse {
            connection,
            response: openshard_protocol::target::TargetResponse {
                cursor_id: openshard_protocol::wire::CursorId(owner.raw()),
                object:    openshard_protocol::serial::Serial::new(0),
                location:  at,
                graphic:   None,
                cancelled: false,
            },
        });
        world.tick(now);
        deed_serial
    }

    let now = Instant::now();
    let mut world = world();
    world
        .state
        .set_tiles(tiles_with(&[(WALL, WALL_FLAGS, 20), (FLOOR, 0, 0)]));
    world.state.multis = multis_with(COTTAGE, cottage());
    let connection = enter_gm(&mut world, now);
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).expect("a player serial");
    let at = Point::new(START.x, START.y, 0);

    gm::run(&mut world.state, player, "house 0x64");
    world.tick(now);
    let house = world
        .state
        .registry
        .query::<House>()
        .next()
        .map(|(entity, _)| entity)
        .expect("the test house is absent");

    let first = place_stone_oven_east_at(&mut world, connection, owner, at, now);
    assert!(
        world.state.registry.entity_of(first).is_none(),
        "the first oven's deed was not spent"
    );
    assert_eq!(
        openshard_housing::storage::locked_down(&world.state, house).len(),
        2,
        "the first oven did not lock down both its components"
    );

    let second = place_stone_oven_east_at(&mut world, connection, owner, at, now);
    let Some(second_entity) = world.state.registry.entity_of(second) else {
        panic!("a second oven's deed was spent even though it could not fit");
    };
    assert_eq!(
        openshard_housing::storage::locked_down(&world.state, house).len(),
        2,
        "a second oven stacked invisibly on the first"
    );
    assert!(
        world.state.registry.get::<AddonDeed>(second_entity).is_some(),
        "the refused deed lost its placement identity and cannot be retried"
    );
}

/// **The whole design conversation, through the tick.**
///
/// Three packets and every one of them fails quietly on its own: a revision
/// nobody sends leaves a client redrawing a house it already has, a request with
/// no handler is a house that never draws, and a `0xD8` that never goes out is a
/// building whose walls are right and whose picture is somebody else's.
///
/// C1 wrote both packets on both ends and wired neither, so its own "done when"
/// could not be demonstrated. This is that demonstration.
#[test]
fn a_designed_house_announces_its_revision_and_answers_the_ask() {
    use openshard_uofiles::multi::Component;

    const COTTAGE: u16 = 0x64;
    const WALL: u16 = 0x0006;
    const VILLA_WALL: u16 = 0x0007;
    /// One wall, one tile east — the cottage the tables below describe.
    fn cottage() -> Vec<Component> {
        vec![Component {
            graphic: Graphic(WALL),
            dx:      1,
            dy:      0,
            dz:      0,
            flags:   1,
        }]
    }

    /// Every `0xBF` the connection was sent, by subcommand.
    fn extended(packets: &[Vec<u8>]) -> Vec<u16> {
        packets
            .iter()
            .filter(|packet| packet[0] == 0xBF && packet.len() >= 5)
            .map(|packet| u16::from_be_bytes([packet[3], packet[4]]))
            .collect()
    }

    let now = Instant::now();
    let mut world = world();
    world.state.set_tiles(tiles_with(&[
        (WALL, WALL_FLAGS, 20),
        (VILLA_WALL, WALL_FLAGS, 20),
    ]));
    world.state.multis = multis_with(COTTAGE, cottage());
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];

    gm::run(&mut world.state, player, "house 0x64");
    world.tick(now);
    let house = world
        .state
        .registry
        .query::<openshard_state::components::House>()
        .map(|(entity, _)| entity)
        .next()
        .expect("the staff verb built a house");
    let serial = world.state.registry.serial_of(house).expect("a house serial");
    let _ = packets_for(&mut world, connection);

    // A *classic* house announces nothing: its picture is in the client's own
    // files and has no revision. This is the half that costs nothing on the
    // overwhelmingly common shard.
    world.state.show(player, house);
    world.tick(now);
    assert!(
        !extended(&packets_for(&mut world, connection)).contains(&0x1D),
        "a house with no design announced a revision it does not have"
    );

    // Give it one, the way `.hdesign` does.
    openshard_housing::design::redesign(
        &mut world.state,
        player,
        house,
        vec![openshard_uofiles::multi::Component {
            graphic: Graphic(VILLA_WALL),
            dx:      2,
            dy:      0,
            dz:      0,
            flags:   1,
        }],
    )
    .expect("the owner may redesign");
    world.tick(now);

    // The commit told whoever was looking, with the new picture in the same
    // burst. Waiting for the client's query here leaves the old foundation
    // visible for one round-trip.
    let changed = packets_for(&mut world, connection);
    assert!(
        extended(&changed).contains(&0x1D),
        "the design committed and nobody was told"
    );
    let changed_detail = changed
        .iter()
        .find(|packet| packet[0] == 0xD8)
        .expect("the changed house was left as a bare foundation");
    assert_eq!(changed_detail[4], 0, "an offered D8 is not a query response");

    // And it rides with the draw for anyone arriving later.
    world.state.seen.clear();
    world.state.show(player, house);
    world.tick(now);
    let shown = packets_for(&mut world, connection);
    assert!(
        extended(&shown).contains(&0x1D),
        "the revision did not ride along with the draw"
    );
    let shown_detail = shown
        .iter()
        .find(|packet| packet[0] == 0xD8)
        .expect("a newly shown designed house was left as a bare foundation");
    assert_eq!(shown_detail[4], 0, "the immediate D8 is volunteered");

    // The ask, and the answer.
    world.queue(Command::DesignDetails {
        connection,
        serial: RawSerial(serial.raw()),
    });
    world.tick(now);
    let answer = packets_for(&mut world, connection);
    let detail = answer
        .iter()
        .find(|packet| packet[0] == 0xD8)
        .expect("the shard was asked for a design and sent none");

    // It is the design, not the multi: read it back through the decoder rather
    // than trusting the length, because a `0xD8` full of the wrong tiles is the
    // exact failure this whole track exists to stop.
    let bounds = openshard_protocol::design::DesignBounds {
        x_min:  2,
        y_min:  0,
        width:  1,
        height: 1,
    };
    let back = openshard_protocol::design::DesignDetail::decode(detail, bounds)
        .expect("the shard sent a design its own decoder refuses");
    assert_eq!(back.serial.0, serial.raw());
    assert_eq!(back.revision, openshard_protocol::design::Revision(0x0800_0001));
    assert!(back.response, "an answer to an ask is flagged as one");
    assert_eq!(
        back.tiles,
        vec![openshard_protocol::design::DesignTile {
            graphic: Graphic(VILLA_WALL),
            dx:      2,
            dy:      0,
            dz:      0,
        }],
        "the shard sent the foundation's shape instead of the design's"
    );
}

/// **A deed for a foundation builds a designed house, through the ordinary
/// path.**
///
/// C2's third step turned out to be no lines at all: the deed hands its multi
/// id to `housing::place`, and `place` is where the foundation refusal lived.
/// That is worth a test rather than a claim — "it should already work" is how a
/// path goes untested, and this one crosses the deed, the cursor and the
/// placement in one.
#[test]
fn a_deed_for_a_foundation_builds_a_house_with_a_design() {
    use openshard_state::components::HouseDeed;
    use openshard_uofiles::multi::Component;

    const FOUNDATION: u16 = 0x13EC;
    const WALL: u16 = 0x0006;

    /// A platform three tiles across. Width matters: the stair strip runs
    /// `1..width`, so a one-tile platform gets none — which is the reference's
    /// own arithmetic and not worth special-casing, but it does make a degenerate
    /// fixture prove nothing.
    fn platform() -> Vec<Component> {
        (-1..=1)
            .map(|dx| {
                Component {
                    graphic: Graphic(WALL),
                    dx,
                    dy: 0,
                    dz: 0,
                    flags: 1,
                }
            })
            .collect()
    }

    let now = Instant::now();
    let mut world = world();
    world.state.set_tiles(tiles_with(&[(WALL, WALL_FLAGS, 20)]));
    world.state.multis = multis_with(FOUNDATION, platform());
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();

    gm::run(&mut world.state, player, "deed 0x13ec");
    world.tick(now);
    let deed = world
        .state
        .registry
        .query::<HouseDeed>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a deed for a foundation");
    // Into the pack: a deed on the ground is not one you hold, which the
    // placement re-checks.
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
    openshard_state::relocate_item(
        &mut world.state,
        deed,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack,
            position:  GumpPoint::new(20, 20),
            grid:      openshard_protocol::containers::GridSlot(0),
        }),
    )
    .unwrap();
    let deed_serial = world.state.registry.serial_of(deed).unwrap();
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(deed_serial.raw())),
    });
    world.tick(now);
    let raised = packets_for(&mut world, connection);
    assert!(
        raised.iter().any(|packet| packet[0] == 0x99),
        "a deed for a foundation raised no cursor"
    );

    let at = Point::new(START.x + 6, START.y + 6, 0);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(owner.raw()),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  at,
            graphic:   None,
            cancelled: false,
        },
    });
    world.tick(now);

    let house = world
        .state
        .registry
        .query::<openshard_state::components::House>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a deed for a foundation built nothing");
    let shape = openshard_housing::design::shape_of_house(&world.state, house)
        .expect("a foundation placed from a deed has no design");
    assert!(
        shape.iter().any(|component| component.graphic == Graphic(0x0751)),
        "the house a deed built has no stairs"
    );
    assert!(
        world.state.registry.entity_of(deed_serial).is_none(),
        "the deed was not spent"
    );
}
