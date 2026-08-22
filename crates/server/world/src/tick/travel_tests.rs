//! Moving a mobile between facets: what the traveller's client is told, and
//! what the world it left has to forget.
//!
//! A child module rather than more of `tests.rs`, for the reason `region_tests`
//! gives: these read private world state, so they stay inside the module, but
//! they need not pile into the same file.
//!
//! Every case here is a cache that no compiler checks. A facet change that
//! forgets one of them produces no error and no failing single-facet test — it
//! produces a client drawing mobiles from a world it is no longer in, at
//! coordinates that now mean somewhere else.

use super::tests::{
    START, add_empty_facet, add_empty_facet_sized, backpack_serial, enter, enter_as, enter_on_facet,
    packets_for, serial_of, walk, world,
};
use super::*;
use openshard_protocol::casting::SpellId;
use openshard_protocol::items::DropDestination;
use openshard_protocol::serial::RawSerial;
use openshard_state::components::{
    Contained, CriminalUntil, Decays, InRegion, Mana, Moongate, Movement, Position, RECALL_RUNE_GRAPHIC,
    RuneMark, SPELLBOOK_GRAPHIC, Spellbook,
};
use openshard_state::{Region, RegionFlags, RegionId, RegionRect};

/// Ilshenar's shape, which is nothing like Britannia's — the whole reason the
/// client has to be told.
const ILSHENAR: (u32, u32) = (2304, 1600);

/// Where a traveller lands, inside every facet these tests register.
fn arrival() -> Point {
    Point::new(START.0, START.1, 0)
}

#[test]
fn a_traveller_leaves_the_old_facets_sector_grid() {
    // The removal `teleport` never had to do. Left out, the old facet keeps
    // handing this entity back to every `nearby` query on it forever.
    let now = Instant::now();
    let mut world = world();
    add_empty_facet(&mut world, Facet(1));
    let connection = enter(&mut world, now);
    let traveller = world.state.players[&connection];

    assert!(
        world.state.facet_state(Facet(0)).sectors.position_of(traveller) == Some(arrival()),
        "it starts on facet 0's grid"
    );

    world.state.move_to(traveller, Facet(1), arrival());

    assert_eq!(
        world.state.facet_state(Facet(0)).sectors.position_of(traveller),
        None,
        "and is gone from it"
    );
    assert_eq!(
        world.state.facet_state(Facet(1)).sectors.position_of(traveller),
        Some(arrival()),
        "and on the new one"
    );
    assert_eq!(
        world.state.facet_of(traveller),
        Facet(1),
        "and the world agrees which facet it is on"
    );
}

#[test]
fn a_watcher_on_the_old_facet_is_told_to_forget_the_traveller() {
    // Two mobiles on different facets never see each other, so a watcher left
    // holding the traveller would hold it until one of them logged out.
    let now = Instant::now();
    let mut world = world();
    add_empty_facet(&mut world, Facet(1));
    let watcher_connection = enter(&mut world, now);
    let traveller_connection = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let traveller = world.state.players[&traveller_connection];
    let traveller_serial = serial_of(&world, traveller_connection);
    let watcher = world.state.players[&watcher_connection];
    assert!(
        world.state.seen[&watcher].contains(&traveller),
        "they can see each other to begin with"
    );
    let _ = packets_for(&mut world, watcher_connection);

    world.state.move_to(traveller, Facet(1), arrival());

    assert!(
        !world.state.seen[&watcher].contains(&traveller),
        "the watcher no longer holds the traveller"
    );
    assert!(
        packets_for(&mut world, watcher_connection)
            .iter()
            .any(|p| p[0] == 0x1D && u32::from_be_bytes([p[1], p[2], p[3], p[4]]) == traveller_serial.raw()),
        "and was told to take it off the screen"
    );
}

#[test]
fn a_traveller_forgets_everything_on_the_old_facets_screen() {
    // ServUO's `ClearScreen`. Without it the client keeps drawing the mobiles of
    // a world it has left, and their serials go on meaning something.
    let now = Instant::now();
    let mut world = world();
    add_empty_facet(&mut world, Facet(1));
    let traveller_connection = enter(&mut world, now);
    let stayer_connection = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let traveller = world.state.players[&traveller_connection];
    let stayer_serial = serial_of(&world, stayer_connection);
    assert!(
        !world.state.seen[&traveller].is_empty(),
        "it has somebody on screen to begin with"
    );
    let _ = packets_for(&mut world, traveller_connection);

    world.state.move_to(traveller, Facet(1), arrival());

    assert!(
        world.state.seen[&traveller].is_empty(),
        "the traveller's screen is empty on arrival"
    );
    assert!(
        packets_for(&mut world, traveller_connection)
            .iter()
            .any(|p| p[0] == 0x1D && u32::from_be_bytes([p[1], p[2], p[3], p[4]]) == stayer_serial.raw()),
        "and it was told to forget who it left behind"
    );
}

#[test]
fn a_facet_change_sends_the_new_facets_map_dimensions() {
    // `0xBF 0x08` says which map to draw; `0x76` says where on it and how big it
    // is. Sending Britannia's size for Ilshenar puts the edge of the world in
    // the wrong place.
    let now = Instant::now();
    let mut world = world();
    add_empty_facet_sized(&mut world, Facet(1), ILSHENAR.0, ILSHENAR.1);
    let connection = enter(&mut world, now);
    let traveller = world.state.players[&connection];
    let _ = packets_for(&mut world, connection);

    world.state.move_to(traveller, Facet(1), arrival());
    let sent = packets_for(&mut world, connection);

    let map_change = sent
        .iter()
        .find(|p| p[0] == 0xBF && u16::from_be_bytes([p[3], p[4]]) == 0x08)
        .expect("told which map to draw");
    assert_eq!(map_change[5], 1, "facet 1");

    let change = sent
        .iter()
        .find(|p| p[0] == 0x76)
        .expect("told how big the new world is");
    assert_eq!(
        u16::from_be_bytes([change[12], change[13]]),
        ILSHENAR.0 as u16,
        "the new facet's width, not the old one's"
    );
    assert_eq!(
        u16::from_be_bytes([change[14], change[15]]),
        ILSHENAR.1 as u16,
        "and its height"
    );

    // And no `0x1B`: that is the packet that starts a session, not one that
    // moves it.
    assert!(
        !sent.iter().any(|p| p[0] == 0x1B),
        "a facet change is not a login"
    );
}

#[test]
fn login_sends_the_dimensions_of_the_facet_the_character_is_on() {
    // The same fact, at the other end: `0x1B` carried Britannia's size for every
    // facet, so a character saved in Ilshenar woke to a map three times too big.
    let now = Instant::now();
    let mut world = world();
    add_empty_facet_sized(&mut world, Facet(1), ILSHENAR.0, ILSHENAR.1);
    let connection = ConnectionId::from_raw(77);
    enter_on_facet(&mut world, connection, Facet(1), now);

    let start = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0x1B)
        .expect("the world-entry packet");
    // 0x1B: id(1) serial(4) pad(4) body(2) x(2) y(2) pad(1) z(1) facing(1)
    // pad(1) 0xFFFFFFFF(4) pad(4) width(2) height(2) — width at offset 27.
    assert_eq!(
        u16::from_be_bytes([start[27], start[28]]),
        ILSHENAR.0 as u16,
        "the facet it logged in on"
    );
    assert_eq!(u16::from_be_bytes([start[29], start[30]]), ILSHENAR.1 as u16,);
}

/// The same gap as above, one client version older: below
/// [`ClientVersion::WIDE_MAP`], Felucca's own 7168-wide files are not what the
/// login packet may say — see `MapSize::for_client`.
#[test]
fn a_pre_wide_map_client_is_told_felucca_is_the_old_width() {
    let now = Instant::now();
    let mut world = world();
    add_empty_facet_sized(&mut world, Facet(0), 7168, 4096);
    let connection = ConnectionId::from_raw(78);
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::new(4, 0, 11, 3),
        account: AccountName("admin".to_owned()),
        name: CharacterName("Lord British".to_owned()),
        access: AccessLevel::Player,
        character: Character::fresh(Facet(0)),
    }));
    world.tick(now);

    let start = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0x1B)
        .expect("the world-entry packet");
    assert_eq!(
        u16::from_be_bytes([start[27], start[28]]),
        6144,
        "the file is 7168 wide but this client predates that generation"
    );
    assert_eq!(
        u16::from_be_bytes([start[29], start[30]]),
        4096,
        "height never moved"
    );
}

/// The same rule again at the other call site: a mid-session facet change
/// (`0x76`, not `0x1B`) reads the traveller's version off the connection row
/// rather than a local the login path already had in hand.
#[test]
fn a_pre_wide_map_client_moving_onto_trammel_is_told_the_old_width() {
    let now = Instant::now();
    let mut world = world();
    add_empty_facet_sized(&mut world, Facet(1), 7168, 4096);
    let connection = ConnectionId::from_raw(79);
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::new(4, 0, 11, 3),
        account: AccountName("admin".to_owned()),
        name: CharacterName("Lord British".to_owned()),
        access: AccessLevel::Player,
        character: Character::fresh(Facet(0)),
    }));
    world.tick(now);
    let traveller = world.state.players[&connection];
    let _ = packets_for(&mut world, connection);

    world.state.move_to(traveller, Facet(1), arrival());
    let sent = packets_for(&mut world, connection);

    let change = sent
        .iter()
        .find(|p| p[0] == 0x76)
        .expect("told how big the new world is");
    assert_eq!(
        u16::from_be_bytes([change[12], change[13]]),
        6144,
        "Trammel's own files are 7168 wide but this client predates that generation"
    );
    assert_eq!(
        u16::from_be_bytes([change[14], change[15]]),
        4096,
        "height never moved"
    );
}

#[test]
fn the_same_region_id_on_two_facets_is_still_a_crossing() {
    // Every facet numbers its regions from zero, so an id alone is not an
    // answer. Compared without the facet, a traveller between two regions that
    // happen to share a number looks like somebody who never moved: no
    // `RegionChanged`, no music, and no guards.
    let now = Instant::now();
    let mut world = world();
    add_empty_facet(&mut world, Facet(1));
    let named = |name: &str| Region {
        id: RegionId(0),
        name: name.to_owned(),
        priority: 50,
        rects: vec![RegionRect::new(START.0 - 20, START.1 - 20, 40, 40)],
        flags: RegionFlags::default(),
        music: None,
        light: None,
    };
    for (facet, name) in [(0, "Britain"), (1, "Compassion")] {
        world.queue(Command::RegisterRegions {
            facet: Facet(facet),
            regions: vec![named(name)],
        });
    }
    world.tick(now);

    let connection = enter(&mut world, now);
    let traveller = world.state.players[&connection];
    world.tick(now);
    assert_eq!(
        world.state.registry.get::<InRegion>(traveller),
        Some(&InRegion {
            facet: Facet(0),
            region: Some(RegionId(0))
        }),
        "it is in Britain, region zero of facet zero"
    );
    let mut crossings = world.state.bus.cursor::<crate::events::RegionChanged>();

    world.state.move_to(traveller, Facet(1), arrival());
    world.tick(now);

    let names: Vec<String> = world
        .state
        .bus
        .read(&mut crossings)
        .map(|crossing| crossing.name.clone())
        .collect();
    assert!(
        names.iter().any(|name| name == "Compassion"),
        "arriving on another facet's region zero is a crossing, not a no-op: {names:?}"
    );
    assert_eq!(
        world.state.registry.get::<InRegion>(traveller),
        Some(&InRegion {
            facet: Facet(1),
            region: Some(RegionId(0))
        }),
        "and the memory now names the facet it is on"
    );
}

#[test]
fn a_facet_change_resets_the_walk_sequence() {
    // The client zeroes its own count on a jump it did not predict. A server
    // that keeps counting refuses the client's next step — which was correct —
    // and the two ends spend the rest of the session out of phase.
    let now = Instant::now();
    let mut world = world();
    add_empty_facet(&mut world, Facet(1));
    let connection = enter(&mut world, now);
    let traveller = world.state.players[&connection];

    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::North),
    });
    world.tick(now);
    let walker = world.registry().get::<Movement>(traveller).unwrap().0;
    assert!(!walker.sequence.is_fresh(), "it has taken a step");

    world.state.move_to(traveller, Facet(1), arrival());

    let walker = world.registry().get::<Movement>(traveller).unwrap().0;
    assert!(
        walker.sequence.is_fresh(),
        "and the jump put both ends back to zero"
    );
    assert_eq!(
        world.registry().get::<Position>(traveller).map(|p| p.0),
        Some(arrival()),
        "the walker's own copy of where it stands moved with it"
    );
}

#[test]
fn a_facet_the_shard_never_loaded_is_refused() {
    // A mobile there would have no ground, no neighbours and no way back.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let traveller = world.state.players[&connection];

    world.state.move_to(traveller, Facet(9), Point::new(100, 100, 0));

    assert_eq!(
        world.state.facet_of(traveller),
        Facet(0),
        "it did not go anywhere"
    );
    assert_eq!(
        world.state.facet_state(Facet(0)).sectors.position_of(traveller),
        Some(arrival()),
        "and is still on the grid it started on"
    );
}

// -- marking and recalling ---------------------------------------------------

/// Recall's and Mark's spell ids, their position in the classic book.
const RECALL: u16 = 31;
const MARK: u16 = 44;

/// The three reagents both spells want.
const BLACK_PEARL: u16 = 0x0F7A;
const BLOOD_MOSS: u16 = 0x0F7B;
const MANDRAKE_ROOT: u16 = 0x0F86;
const SULFUROUS_ASH: u16 = 0x0F8C;

/// A caster who can afford either spell, with a rune in its pack.
///
/// Sphere-style casting, so a cast resolves the tick it is asked for and the
/// cursor comes straight up — these tests are about the travel rules, not about
/// waiting out a cast delay.
fn caster_with_rune(now: Instant) -> (World, ConnectionId, EntityId, Serial) {
    let mut world = World::new(START).with_gameplay(Gameplay {
        cast_style: openshard_state::CastStyle::Walk,
        ..Default::default()
    });
    add_empty_facet(&mut world, Facet(1));
    let connection = enter(&mut world, now);
    let caster = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    world.queue(Command::SetSkill {
        serial,
        skill: 25, // Magery
        value: 1000,
    });
    world.tick(now);

    let backpack = backpack_serial(&world, connection);
    for reagent in [BLACK_PEARL, BLOOD_MOSS, MANDRAKE_ROOT, SULFUROUS_ASH] {
        openshard_items::give(
            &mut world.state,
            backpack,
            openshard_protocol::wire::Graphic(reagent),
            openshard_protocol::wire::Hue(0),
            50,
        );
    }
    if let Some(book) = openshard_items::give(
        &mut world.state,
        backpack,
        SPELLBOOK_GRAPHIC,
        openshard_protocol::wire::Hue(0),
        1,
    ) {
        world.state.registry.insert(book, Spellbook::full());
    }
    let rune = openshard_items::place_one(
        &mut world.state,
        backpack,
        RECALL_RUNE_GRAPHIC,
        openshard_protocol::wire::Hue(0),
        1,
    )
    .expect("a rune in the pack");
    let rune_serial = world.state.registry.serial_of(rune).unwrap();
    let _ = packets_for(&mut world, connection);
    (world, connection, caster, rune_serial)
}

/// Cast `spell` and answer its cursor with `target`.
fn cast_at(world: &mut World, connection: ConnectionId, spell: u16, target: Serial, now: Instant) {
    world.queue(Command::RequestCast {
        connection,
        spell: SpellId(spell),
    });
    world.tick(now);
    let cursor_id = serial_of(world, connection);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(cursor_id.raw()),
            object: Some(target),
            location: Point::new(0, 0, 0),
            graphic: None,
            cancelled: false,
        },
    });
    world.tick(now);
}

#[test]
fn marking_a_rune_writes_where_you_stand_and_recalling_takes_you_back() {
    // The two halves of one fact: what Mark writes is exactly what Recall reads,
    // and a disagreement between them is a rune that says Britain and lands you
    // in a swamp.
    let now = Instant::now();
    let (mut world, connection, caster, rune_serial) = caster_with_rune(now);
    let marked_at = world.registry().get::<Position>(caster).unwrap().0;

    cast_at(&mut world, connection, MARK, rune_serial, now);

    let rune = world.state.registry.entity_of(rune_serial).unwrap();
    assert_eq!(
        world.registry().get::<RuneMark>(rune).map(|m| m.destination),
        Some(marked_at),
        "the rune remembers the tile it was marked on"
    );

    // Walk away, then come back by rune.
    world
        .state
        .teleport(caster, Point::new(START.0 + 40, START.1 + 40, 0));
    world.tick(now);
    assert_ne!(world.registry().get::<Position>(caster).unwrap().0, marked_at);

    cast_at(&mut world, connection, RECALL, rune_serial, now);

    assert_eq!(
        world.registry().get::<Position>(caster).unwrap().0,
        marked_at,
        "and recall put the caster back on it"
    );
}

#[test]
fn a_blank_rune_recalls_nowhere() {
    // The absence of a `RuneMark` *is* "unmarked" — there is no flag to disagree
    // with a destination that would mean nothing when it is false.
    let now = Instant::now();
    let (mut world, connection, caster, rune_serial) = caster_with_rune(now);
    let before = world.registry().get::<Position>(caster).unwrap().0;

    cast_at(&mut world, connection, RECALL, rune_serial, now);

    assert_eq!(
        world.registry().get::<Position>(caster).unwrap().0,
        before,
        "an unmarked rune takes you nowhere"
    );
}

#[test]
fn a_rune_on_the_floor_cannot_be_marked_but_can_be_recalled_from() {
    // ServUO's asymmetry, and it is deliberate on both sides: Mark wants the rune
    // in your own pack (cliloc 1062422), while Recall accepts any rune you can
    // reach — a rune held out by a friend is a classic way to be fetched.
    let now = Instant::now();
    let (mut world, connection, caster, rune_serial) = caster_with_rune(now);
    let rune = world.state.registry.entity_of(rune_serial).unwrap();
    let at = world.registry().get::<Position>(caster).unwrap().0;

    // Mark it while it is still in the pack, then drop it at the caster's feet.
    cast_at(&mut world, connection, MARK, rune_serial, now);
    world.state.registry.remove::<Contained>(rune);
    world.state.registry.insert(rune, Position(at));
    world.state.registry.insert(rune, Facet(0));

    // Marking it again is refused now it is not carried.
    world.state.registry.remove::<RuneMark>(rune);
    cast_at(&mut world, connection, MARK, rune_serial, now);
    assert!(
        world.registry().get::<RuneMark>(rune).is_none(),
        "a rune on the floor is somebody else's to mark"
    );

    // But recalling from a reachable one still works.
    world.state.registry.insert(
        rune,
        RuneMark {
            facet: Facet(0),
            destination: Point::new(START.0 + 5, START.1 + 5, 0),
        },
    );
    cast_at(&mut world, connection, RECALL, rune_serial, now);
    assert_eq!(
        world.registry().get::<Position>(caster).unwrap().0,
        Point::new(START.0 + 5, START.1 + 5, 0),
        "recall does not ask whose pack the rune is in"
    );
}

#[test]
fn a_no_recall_region_bars_arriving_and_marking_but_not_leaving() {
    // ServUO's matrix collapsed: `RecallFrom` is the one permissive row, so a
    // jail you can recall out of is still one nobody can recall into or mark in.
    // Getting this backwards makes every dungeon a one-way trap.
    let now = Instant::now();
    let (mut world, connection, caster, rune_serial) = caster_with_rune(now);
    let inside = Point::new(START.0 + 3, START.1 + 3, 0);
    world.queue(Command::RegisterRegions {
        facet: Facet(0),
        regions: vec![Region {
            id: RegionId(0),
            name: "Wrong".to_owned(),
            priority: 50,
            rects: vec![RegionRect::new(inside.x - 1, inside.y - 1, 3, 3)],
            flags: RegionFlags {
                no_recall: true,
                ..RegionFlags::default()
            },
            music: None,
            light: None,
        }],
    });
    world.tick(now);

    // Standing inside it, a rune cannot be marked.
    world.state.teleport(caster, inside);
    cast_at(&mut world, connection, MARK, rune_serial, now);
    let rune = world.state.registry.entity_of(rune_serial).unwrap();
    assert!(
        world.registry().get::<RuneMark>(rune).is_none(),
        "no marking inside a no-recall region"
    );

    // Nor can anyone recall *into* it.
    world.state.registry.insert(
        rune,
        RuneMark {
            facet: Facet(0),
            destination: inside,
        },
    );
    world.state.teleport(caster, Point::new(START.0, START.1, 0));
    cast_at(&mut world, connection, RECALL, rune_serial, now);
    assert_ne!(
        world.registry().get::<Position>(caster).unwrap().0,
        inside,
        "no arriving in one either"
    );

    // But standing in it, you may still leave: `RecallFrom` is permissive.
    let out = Point::new(START.0 + 20, START.1, 0);
    world.state.teleport(caster, inside);
    world.state.registry.insert(
        rune,
        RuneMark {
            facet: Facet(0),
            destination: out,
        },
    );
    cast_at(&mut world, connection, RECALL, rune_serial, now);
    assert_eq!(
        world.registry().get::<Position>(caster).unwrap().0,
        out,
        "leaving is the direction ServUO allows"
    );
}

#[test]
fn a_rune_marked_on_another_facet_is_a_walk_unless_the_shard_says_otherwise() {
    // The classic pre-AoS rule. The machinery to cross facets works either way —
    // this is a rule, not a missing feature, which is why it is a setting and not
    // a limitation.
    let now = Instant::now();
    let (mut world, connection, caster, rune_serial) = caster_with_rune(now);
    let rune = world.state.registry.entity_of(rune_serial).unwrap();
    let far = Point::new(START.0, START.1, 0);
    world.state.registry.insert(
        rune,
        RuneMark {
            facet: Facet(1),
            destination: far,
        },
    );

    cast_at(&mut world, connection, RECALL, rune_serial, now);
    assert_eq!(world.state.facet_of(caster), Facet(0), "refused, by default");

    world.state.gameplay.cross_facet_travel = true;
    cast_at(&mut world, connection, RECALL, rune_serial, now);
    assert_eq!(
        world.state.facet_of(caster),
        Facet(1),
        "and allowed when the shard says so"
    );
}

#[test]
fn a_criminal_cannot_recall_away_and_it_costs_them_nothing_to_find_out() {
    // ServUO's `CheckCast`, which runs before the cast — a thief who cannot
    // escape should learn that for free, not for eleven mana and three reagents.
    let now = Instant::now();
    let (mut world, connection, caster, rune_serial) = caster_with_rune(now);
    let rune = world.state.registry.entity_of(rune_serial).unwrap();
    world.state.registry.insert(
        rune,
        RuneMark {
            facet: Facet(0),
            destination: Point::new(START.0 + 9, START.1, 0),
        },
    );
    world.state.registry.insert(caster, CriminalUntil { tick: 9_999 });
    let mana_before = world.registry().get::<Mana>(caster).unwrap().current;
    let at = world.registry().get::<Position>(caster).unwrap().0;

    cast_at(&mut world, connection, RECALL, rune_serial, now);

    assert_eq!(
        world.registry().get::<Position>(caster).unwrap().0,
        at,
        "the criminal stays where they are"
    );
    assert_eq!(
        world.registry().get::<Mana>(caster).unwrap().current,
        mana_before,
        "and paid nothing to be refused"
    );
}

// -- gates -------------------------------------------------------------------

/// Gate Travel's spell id.
const GATE_TRAVEL: u16 = 51;

#[test]
fn a_gate_opens_at_both_ends_and_each_leads_to_the_other() {
    // Cross-linked by construction: each gate points at the other's tile, so
    // there is no link field that can disagree with a destination.
    let now = Instant::now();
    let (mut world, connection, caster, rune_serial) = caster_with_rune(now);
    let here = world.registry().get::<Position>(caster).unwrap().0;
    let there = Point::new(START.0 + 15, START.1 + 15, 0);
    let rune = world.state.registry.entity_of(rune_serial).unwrap();
    world.state.registry.insert(
        rune,
        RuneMark {
            facet: Facet(0),
            destination: there,
        },
    );

    cast_at(&mut world, connection, GATE_TRAVEL, rune_serial, now);

    let near = world.gate_at(Facet(0), here).expect("a gate at the caster");
    let far = world
        .gate_at(Facet(0), there)
        .expect("and one at the destination");
    assert_eq!(world.registry().get::<Moongate>(near).unwrap().destination, there);
    assert_eq!(world.registry().get::<Moongate>(far).unwrap().destination, here);
}

#[test]
fn walking_into_a_gate_takes_you_through_it() {
    // There is no `OnMoveOver` here and there are two movement paths, so the
    // crossing is read off this tick's `MobileMoved` rather than called from
    // each — the shape `guard_crossings` uses.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let walker = world.state.players[&connection];
    let from = world.registry().get::<Position>(walker).unwrap().0;
    // Where the step actually lands, from the engine's own vector table — UO's
    // compass is diagonal on screen, so guessing the offset is how a test ends up
    // asserting about a tile nobody walked onto.
    let onto = openshard_movement::step_from(from, Direction::North).expect("a tile north");
    let far = Point::new(START.0 + 30, START.1, 0);
    world.spawn_gate(
        Facet(0),
        onto,
        Moongate {
            facet: Facet(0),
            destination: far,
            expires_at: None,
        },
    );

    // Two requests: the first only turns the character to face the way it is
    // going, as UO's walk does, and the second is the step that lands on the gate.
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

    assert_eq!(
        world.registry().get::<Position>(walker).unwrap().0,
        far,
        "the step onto the gate carried through it"
    );
}

#[test]
fn a_gate_closes_on_its_own_and_leaves_nothing_behind() {
    // Three ways a gate outlives its half-minute, all quiet: a second clock from
    // `spawn_item`, a save that restores it, or an expiry that forgets the
    // sector grid and leaves an invisible gate that still works.
    let now = Instant::now();
    let mut world = world();
    let at = Point::new(START.0 + 2, START.1, 0);
    let gate = world
        .spawn_gate(
            Facet(0),
            at,
            Moongate {
                facet: Facet(0),
                destination: Point::new(START.0 + 9, START.1, 0),
                expires_at: Some(3),
            },
        )
        .expect("a gate");
    assert!(
        world.registry().get::<Decays>(gate).is_none(),
        "a gate owns its own lifetime and carries no second clock"
    );

    let mut later = now;
    for _ in 0..5 {
        later += TICK_INTERVAL;
        world.tick(later);
    }

    assert!(world.registry().get::<Moongate>(gate).is_none(), "it closed");
    assert_eq!(
        world.state.facet_state(Facet(0)).sectors.position_of(gate),
        None,
        "and left nothing on the sector grid to walk into"
    );
}

#[test]
fn a_gate_is_not_swept_into_the_save() {
    // Restored, a half-minute portal becomes a permanent one whose caster no
    // longer exists — which is why ServUO deletes its own on deserialise.
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    world.spawn_gate(
        Facet(0),
        Point::new(START.0 + 2, START.1, 0),
        Moongate {
            facet: Facet(0),
            destination: Point::new(START.0 + 9, START.1, 0),
            expires_at: Some(9_999),
        },
    );
    world.tick(now);

    world.take_snapshot();
    let saved: Vec<_> = world.drain_saves().collect();
    let ground = saved
        .iter()
        .filter_map(|snapshot| snapshot.ground.as_ref())
        .flatten()
        .count();
    assert_eq!(ground, 0, "no gate reached the ground sweep");
}

#[test]
fn two_gates_never_stand_on_one_tile() {
    // ServUO checks both ends and Sphere checks both for a telepad: two gates on
    // one spot are two overlapping ways out of it, and closing one leaves the
    // other looking broken.
    let now = Instant::now();
    let (mut world, connection, caster, rune_serial) = caster_with_rune(now);
    let there = Point::new(START.0 + 15, START.1 + 15, 0);
    let rune = world.state.registry.entity_of(rune_serial).unwrap();
    world.state.registry.insert(
        rune,
        RuneMark {
            facet: Facet(0),
            destination: there,
        },
    );
    let here = world.registry().get::<Position>(caster).unwrap().0;

    cast_at(&mut world, connection, GATE_TRAVEL, rune_serial, now);
    let first = world.gate_at(Facet(0), here).expect("the first gate");
    cast_at(&mut world, connection, GATE_TRAVEL, rune_serial, now);

    assert_eq!(
        world.gate_at(Facet(0), here),
        Some(first),
        "the second cast opened nothing new"
    );
}

// -- the city moongates ------------------------------------------------------

#[test]
fn placing_the_city_moongates_twice_places_them_once() {
    // A staff command that doubles its work each time it is run is one nobody
    // can use twice, and the second nine would sit exactly on the first.
    let mut world = world();
    let first = world.place_public_moongates();
    assert_eq!(
        first,
        openshard_magic::PUBLIC_MOONGATES.len(),
        "all nine went down"
    );
    assert_eq!(world.place_public_moongates(), 0, "and none did the second time");
}

#[test]
fn a_city_moongate_does_not_seal_the_tile_it_stands_on() {
    // The whole point is to walk into it. `place_decoration` blocks a tile whose
    // tiledata calls the graphic impassable, which would make the walk-in trigger
    // dead code that reads as a movement bug rather than a missing rule.
    let mut world = world();
    world.place_public_moongates();
    let britain = openshard_magic::PUBLIC_MOONGATES
        .iter()
        .find(|gate| gate.name == "Britain")
        .unwrap();
    let gate = world
        .public_gate_entity(britain.facet, britain.at)
        .expect("a gate at Britain");

    assert!(
        !world
            .state
            .facet_state(britain.facet)
            .obstructions()
            .is_blocked(britain.at.x, britain.at.y),
        "nothing bars the tile"
    );
    assert!(
        world.registry().get::<Moongate>(gate).is_none(),
        "and it carries no component: its destination is derived from where it \
         stands, which is what keeps it out of the schema"
    );
}

#[test]
fn a_city_moongate_survives_a_restart_with_its_meaning_intact() {
    // Saved as ordinary decoration and re-derived at boot. The test that matters
    // is not that the item comes back — decoration always did — but that it is
    // still *a gate* afterwards, with no restore hook to forget.
    let mut world = world();
    world.place_public_moongates();
    let britain = openshard_magic::PUBLIC_MOONGATES
        .iter()
        .find(|gate| gate.name == "Britain")
        .unwrap();

    world.take_snapshot();
    let saved: Vec<_> = world.drain_saves().collect();
    let decorations: Vec<_> = saved
        .iter()
        .filter_map(|snapshot| snapshot.decorations.as_ref())
        .flatten()
        .filter(|record| record.graphic == openshard_state::components::MOONGATE_GRAPHIC.0)
        .collect();
    assert_eq!(
        decorations.len(),
        openshard_magic::PUBLIC_MOONGATES.len(),
        "every gate was swept as decoration"
    );

    let mut restored = super::tests::world();
    restored.restore_decorations(
        saved
            .iter()
            .filter_map(|snapshot| snapshot.decorations.as_ref())
            .flatten()
            .cloned()
            .collect(),
    );
    let gate = restored
        .public_gate_entity(britain.facet, britain.at)
        .expect("the gate came back");
    assert!(
        restored.is_gate(gate),
        "and is still a gate, with nothing restored onto it to make it one"
    );
}

// -- the runebook ------------------------------------------------------------

/// A Recall scroll's graphic — `0x1F2D + spell`.
const RECALL_SCROLL: u16 = 0x1F2D + 31;

/// Give the caster an empty runebook in its pack, and return it.
fn give_runebook(world: &mut World, connection: ConnectionId) -> EntityId {
    let backpack = backpack_serial(world, connection);
    openshard_items::give(
        &mut world.state,
        backpack,
        openshard_state::components::RUNEBOOK_GRAPHIC,
        openshard_protocol::wire::Hue(0),
        1,
    )
    .expect("a runebook")
}

#[test]
fn a_bought_runebook_is_a_book_and_not_a_pile() {
    // Two books merged into one stack of two would share the destinations of
    // neither — the same reason a spellbook bypasses the stack path.
    let now = Instant::now();
    let (mut world, connection, _, _) = caster_with_rune(now);
    let first = give_runebook(&mut world, connection);
    let second = give_runebook(&mut world, connection);

    assert_ne!(first, second, "two books, not one pile of two");
    for book in [first, second] {
        let owned = world
            .registry()
            .get::<openshard_state::components::Runebook>(book)
            .expect("a runebook off the shelf is a runebook");
        assert!(owned.charges > 0, "and comes with charges");
        assert!(owned.entries.is_empty(), "but no destinations");
    }
}

#[test]
fn a_marked_rune_dropped_on_a_book_becomes_an_entry_and_is_consumed() {
    // ServUO deletes the rune, which is why the entry carries its own
    // description rather than pointing back at something that will not be there.
    let now = Instant::now();
    let (mut world, connection, caster, rune_serial) = caster_with_rune(now);
    let book = give_runebook(&mut world, connection);
    let book_serial = world.state.registry.serial_of(book).unwrap();
    cast_at(&mut world, connection, MARK, rune_serial, now);
    let rune = world.state.registry.entity_of(rune_serial).unwrap();
    let marked_at = world.registry().get::<RuneMark>(rune).unwrap().destination;

    openshard_items::pick_up(&mut world.state, connection, RawSerial(rune_serial.raw()), 1);
    openshard_items::drop_item(
        &mut world.state,
        connection,
        RawSerial(rune_serial.raw()),
        DropDestination::Item {
            item: book_serial,
            at: GumpPoint::new(0, 0),
        },
    );

    let owned = world
        .registry()
        .get::<openshard_state::components::Runebook>(book)
        .unwrap();
    assert_eq!(owned.entries.len(), 1, "the destination was bound");
    assert_eq!(owned.entries[0].destination, marked_at);
    assert!(
        world.state.registry.entity_of(rune_serial).is_none(),
        "and the rune itself was spent"
    );
    let _ = caster;
}

#[test]
fn a_recall_scroll_recharges_a_book_and_the_surplus_stays_on_the_cursor() {
    // Clamping the overflow away is the shape of every quiet item-loss bug: the
    // book takes what it has room for and the rest is still the player's.
    let now = Instant::now();
    let (mut world, connection, _, _) = caster_with_rune(now);
    let book = give_runebook(&mut world, connection);
    let book_serial = world.state.registry.serial_of(book).unwrap();
    // Spend it down so there is room for exactly one.
    let mut owned = world
        .registry()
        .get::<openshard_state::components::Runebook>(book)
        .cloned()
        .unwrap();
    let max = owned.max_charges;
    owned.charges = max - 1;
    world.state.registry.insert(book, owned);

    let backpack = backpack_serial(&world, connection);
    let scrolls = openshard_items::give(
        &mut world.state,
        backpack,
        openshard_protocol::wire::Graphic(RECALL_SCROLL),
        openshard_protocol::wire::Hue(0),
        3,
    )
    .expect("scrolls");
    let scroll_serial = world.state.registry.serial_of(scrolls).unwrap();

    openshard_items::pick_up(&mut world.state, connection, RawSerial(scroll_serial.raw()), 3);
    openshard_items::drop_item(
        &mut world.state,
        connection,
        RawSerial(scroll_serial.raw()),
        DropDestination::Item {
            item: book_serial,
            at: GumpPoint::new(0, 0),
        },
    );

    let owned = world
        .registry()
        .get::<openshard_state::components::Runebook>(book)
        .unwrap();
    assert_eq!(owned.charges, max, "the book filled");
    let left = world
        .registry()
        .get::<openshard_state::components::Amount>(scrolls)
        .map(|a| a.0);
    assert_eq!(left, Some(2), "and the two it had no room for were not eaten");
}

#[test]
fn a_charge_takes_you_there_for_free_and_is_spent() {
    // The charge *is* the cost — no mana, no reagents — which is what makes a
    // runebook worth carrying over a handful of runes.
    let now = Instant::now();
    let (mut world, connection, caster, _) = caster_with_rune(now);
    let book = give_runebook(&mut world, connection);
    let there = Point::new(START.0 + 12, START.1, 0);
    let mut owned = world
        .registry()
        .get::<openshard_state::components::Runebook>(book)
        .cloned()
        .unwrap();
    owned.entries.push(openshard_state::components::RunebookEntry {
        facet: Facet(0),
        destination: there,
        description: "Britain".into(),
    });
    let charges_before = owned.charges;
    world.state.registry.insert(book, owned);
    let mana_before = world.registry().get::<Mana>(caster).unwrap().current;

    // Open it, then press the first row's charge button.
    assert!(world.click_runebook(caster, book), "the book opened");
    world.queue(Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial: openshard_protocol::gump::RawGumpKey(serial_of(&world, connection).raw()),
            gump_id: openshard_protocol::gump::RawGumpId(0x0053_0001),
            button: openshard_protocol::gump::RawButtonId(10), // BOOK_USE_CHARGE + slot 0
            switches: Vec::new(),
            text_entries: Vec::new(),
        },
    });
    world.tick(now);

    assert_eq!(
        world.registry().get::<Position>(caster).unwrap().0,
        there,
        "it took the caster there"
    );
    assert_eq!(
        world.registry().get::<Mana>(caster).unwrap().current,
        mana_before,
        "and cost no mana"
    );
    assert_eq!(
        world
            .registry()
            .get::<openshard_state::components::Runebook>(book)
            .unwrap()
            .charges,
        charges_before - 1,
        "but one charge"
    );
}

#[test]
fn a_reply_for_a_row_the_book_does_not_hold_does_nothing() {
    // Refused, not clamped — the `CraftGump` rule. Clamping to slot zero takes
    // somebody to whatever happens to be first, which is worse than nothing.
    let now = Instant::now();
    let (mut world, connection, caster, _) = caster_with_rune(now);
    let book = give_runebook(&mut world, connection);
    let at = world.registry().get::<Position>(caster).unwrap().0;
    assert!(world.click_runebook(caster, book));

    world.queue(Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial: openshard_protocol::gump::RawGumpKey(serial_of(&world, connection).raw()),
            gump_id: openshard_protocol::gump::RawGumpId(0x0053_0001),
            button: openshard_protocol::gump::RawButtonId(10 + 9), // a row an empty book has never had
            switches: Vec::new(),
            text_entries: Vec::new(),
        },
    });
    world.tick(now);

    assert_eq!(
        world.registry().get::<Position>(caster).unwrap().0,
        at,
        "nobody went anywhere"
    );
}

#[test]
fn a_plain_teleport_resets_the_walk_sequence_too() {
    // The facet-change test above covers the new path. This covers the old one,
    // which is where the bug actually lived: `teleport` has moved players since
    // the first staff command and never reset the sequence, so the client —
    // which zeroes its own count on a jump it did not predict — had its next
    // step refused as out of order, and the two ends stayed out of phase for the
    // rest of the session. Nothing errors and nothing else in the suite walks
    // after a teleport.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];

    // Take a step, so the sequence is no longer fresh.
    for step in 0..2 {
        world.queue(Command::Walk {
            connection,
            request: walk(step, Direction::North),
        });
        world.tick(now);
    }
    assert!(
        !world
            .registry()
            .get::<Movement>(player)
            .unwrap()
            .0
            .sequence
            .is_fresh(),
        "it has walked"
    );

    // A teleport on the very same facet — no `move_to`, no facet argument.
    world
        .state
        .teleport(player, Point::new(START.0 + 6, START.1 + 6, 0));

    assert!(
        world
            .registry()
            .get::<Movement>(player)
            .unwrap()
            .0
            .sequence
            .is_fresh(),
        "and the jump put the server back to zero, where the client already is"
    );

    // And the proof it matters: the client's next step *is* a zero, and it is
    // accepted rather than refused.
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::North),
    });
    world.tick(now);
    assert!(
        !packets_for(&mut world, connection).iter().any(|p| p[0] == 0x21),
        "no walk rejection followed the teleport"
    );
}
