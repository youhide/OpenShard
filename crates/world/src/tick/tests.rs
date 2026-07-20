use super::*;
use openshard_chat::{MobileSpoke, TALKMODE_WHISPER, TALKMODE_YELL};
use openshard_combat::{swing_ticks, MobileDied, WRESTLING_SPEED};
use openshard_events::Cursor;
use openshard_magic::{SpellCast, MANA_REGEN_TICKS};
use openshard_movement::WALK_INTERVAL;
use openshard_protocol::{encode_remove, DROP_TO_GROUND};
use openshard_skills::SkillUsed;
use openshard_state::components::Riding;
use openshard_state::components::{
    Amount, Contained, Container, CriminalUntil, Decays, Equipped, Graphic, MurderDecay, Murders,
    Skills, Stackable,
};
use openshard_state::components::{Banker, SwingSpeed};
use openshard_state::sectors::distance;

pub(super) const START: (u16, u16) = (1363, 1600);

/// A generous upper bound on ticks-per-beat, so a test loop that waits "a few
/// beats" survives any cadence the defaults settle on.
pub(super) const AI_THINK_TICKS: u64 = 10;

/// Ticks a bare-handed, default-dexterity mobile waits between swings under
/// the default rules — the pace the combat tests reckon against. `dex 100`,
/// wrestling, era 1, scale 15000: thirty ticks.
const WRESTLING_SWING_TICKS: u64 = swing_ticks(100, WRESTLING_SPEED, 1, 15000);

pub(super) fn world() -> World {
    World::new(START)
}

pub(super) fn connection() -> ConnectionId {
    ConnectionId::from_raw(1)
}

pub(super) fn enter(world: &mut World, now: Instant) -> ConnectionId {
    enter_as(world, connection(), now)
}

pub(super) fn enter_as(world: &mut World, connection: ConnectionId, now: Instant) -> ConnectionId {
    world.queue(Command::Enter {
        connection,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: None,
        position: None,
        facet: 0,
        appearance: None,
        sheet: None,
        access: AccessLevel::Player,
    });
    world.tick(now);
    connection
}

/// Enter as a game master — the authority the `.`-command tests need.
pub(super) fn enter_gm(world: &mut World, now: Instant) -> ConnectionId {
    let connection = connection();
    world.queue(Command::Enter {
        connection,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: None,
        position: None,
        facet: 0,
        appearance: None,
        sheet: None,
        access: AccessLevel::GameMaster,
    });
    world.tick(now);
    connection
}

/// Every packet the last tick produced for one connection.
pub(super) fn packets_for(world: &mut World, connection: ConnectionId) -> Vec<Vec<u8>> {
    world
        .drain_outbound()
        .filter(|out| out.connection == connection)
        .map(|out| out.packet)
        .collect()
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
    world
        .state
        .facet_state_mut(facet)
        .sectors
        .insert(entity, point);
    world.state.refresh_around(entity);
}

pub(super) fn walk(sequence: u8, direction: Direction) -> WalkRequest {
    WalkRequest {
        facing: Facing::walking(direction),
        sequence,
        fastwalk_key: 0,
    }
}

/// The serial the world gave the character a connection is driving.
fn serial_of(world: &World, connection: ConnectionId) -> u32 {
    let entity = world.state.players[&connection];
    world.state.registry.serial_of(entity).unwrap().raw()
}

#[test]
fn a_server_step_turns_first_then_moves() {
    // Turn-as-step, server side: the first `Step` in a new direction turns
    // and stays put; the second moves. The same rule a client walk follows,
    // because the clients watching cannot tell who ordered the step.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);

    let facing0 = world
        .state
        .registry
        .get::<Heading>(entity)
        .unwrap()
        .0
        .direction;
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
    assert_eq!(world.bus().read(&mut turned).count(), 1, "first step turns");
    assert_eq!(world.bus().read(&mut moved).count(), 0, "and does not move");
    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        from,
        "still on the same tile"
    );

    world.queue(Command::Step {
        serial,
        direction: dir.to_bits(),
    });
    world.tick(now);
    let moves: Vec<MobileMoved> = world.bus().read(&mut moved).copied().collect();
    assert_eq!(moves.len(), 1, "second step moves");
    assert_eq!(moves[0].from, from);
    assert_eq!(moves[0].to, step_from(from, dir).unwrap());
    assert_eq!(
        world.state.registry.get::<Position>(entity).unwrap().0,
        step_from(from, dir).unwrap(),
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
        serial: 0x4000_0001,
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
        graphic: GOLD,
        hue: 0,
        amount: 1,
        stackable: false,
        position: point,
        facet: 0,
    });
    world.tick(now);
}

/// Spawn a stackable pile of `amount` gold and return its serial.
fn spawn_gold(world: &mut World, point: Point, amount: u16, now: Instant) -> u32 {
    world.queue(Command::SpawnItem {
        graphic: GOLD,
        hue: 0,
        amount,
        stackable: true,
        position: point,
        facet: 0,
    });
    world.tick(now);
    // The newest ground item, by serial.
    world
        .state
        .registry
        .query::<Position>()
        .filter(|(entity, _)| world.state.registry.has::<Stackable>(*entity))
        .filter_map(|(entity, _)| world.state.registry.serial_of(entity).map(|s| s.raw()))
        .max()
        .expect("the gold was spawned")
}

#[test]
fn a_spawned_item_is_drawn_to_a_player_in_range() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection); // the login burst

    spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);

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
    spawn_item_at(&mut world, Point::new(START.0 + 50, START.1, 0), now);

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
    teleport(&mut world, connection, Point::new(START.0 + 50, START.1, 0));
    spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);
    let _ = packets_for(&mut world, connection);

    // Come into range: the item is drawn.
    teleport(&mut world, connection, Point::new(START.0, START.1, 0));
    let arriving = packets_for(&mut world, connection);
    assert!(
        arriving.iter().any(|p| p[0] == 0x1A),
        "walking up to the item draws it"
    );

    // Leave again: the item is taken off the screen with 0x1D.
    teleport(&mut world, connection, Point::new(START.0 + 50, START.1, 0));
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
        graphic: GOLD,
        hue: 0,
        amount: 500,
        stackable: false,
        position: Point::new(START.0, START.1, 0),
        facet: 0,
    });
    world.tick(now);

    let packets = packets_for(&mut world, connection);
    let item = packets
        .iter()
        .find(|p| p[0] == 0x1A)
        .expect("the item was drawn");
    // The amount bit on the serial says a stack; a single item would not set it.
    assert_ne!(item[3] & 0x80, 0, "the stack sets the amount flag");
}

/// The serial of the one item in the world.
fn only_item_serial(world: &World) -> u32 {
    // The one spawned test item — never a worn backpack, which every character
    // now carries (an item with a `Graphic`, worn via `Equipped`).
    let (entity, _) = world
        .state
        .registry
        .query::<Graphic>()
        .find(|(entity, _)| !world.state.registry.has::<Equipped>(*entity))
        .expect("a loose item is in the world");
    world.state.registry.serial_of(entity).unwrap().raw()
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
    spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);
    let _ = packets_for(&mut world, picker);
    let _ = packets_for(&mut world, watcher);
    let serial = only_item_serial(&world);

    world.queue(Command::PickUpItem {
        connection: picker,
        serial,
        amount: 1,
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, watcher)
            .iter()
            .any(|p| p[0] == 0x1D),
        "the other player is told to forget the lifted item"
    );

    world.queue(Command::DropItem {
        connection: picker,
        serial,
        position: Point::new(START.0, START.1, 0),
        container: DROP_TO_GROUND,
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, watcher)
            .iter()
            .any(|p| p[0] == 0x1A),
        "and to draw it again where it was dropped"
    );
}

#[test]
fn picking_up_out_of_reach_is_rejected_and_leaves_the_item() {
    let now = Instant::now();
    let mut world = world();
    let picker = enter(&mut world, now);
    spawn_item_at(&mut world, Point::new(START.0 + 20, START.1, 0), now);
    let _ = packets_for(&mut world, picker);
    let serial = only_item_serial(&world);
    let item = world
        .state
        .registry
        .entity_of(Serial::new(serial).unwrap())
        .unwrap();

    world.queue(Command::PickUpItem {
        connection: picker,
        serial,
        amount: 1,
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, picker)
            .iter()
            .any(|p| p == &[0x27, 0x01]),
        "the client is told the item is out of range"
    );
    assert!(
        world.state.registry.has::<Position>(item),
        "the item stays on the ground"
    );
    assert!(world.state.held.is_empty(), "and nothing is on the cursor");
}

#[test]
fn dropping_out_of_reach_bounces_the_item_back_to_where_it_was() {
    let now = Instant::now();
    let mut world = world();
    let picker = enter(&mut world, now);
    let origin = Point::new(START.0, START.1, 0);
    spawn_item_at(&mut world, origin, now);
    let serial = only_item_serial(&world);
    let item = world
        .state
        .registry
        .entity_of(Serial::new(serial).unwrap())
        .unwrap();

    world.queue(Command::PickUpItem {
        connection: picker,
        serial,
        amount: 1,
    });
    world.tick(now);
    let _ = packets_for(&mut world, picker);

    // Drop it far from the player: refused, and put back where it started.
    world.queue(Command::DropItem {
        connection: picker,
        serial,
        position: Point::new(START.0 + 40, START.1, 0),
        container: DROP_TO_GROUND,
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
    assert!(world.state.held.is_empty());
}

#[test]
fn logging_out_while_holding_an_item_returns_it_to_the_ground() {
    let now = Instant::now();
    let mut world = world();
    let picker = enter(&mut world, now);
    let watcher = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let origin = Point::new(START.0, START.1, 0);
    spawn_item_at(&mut world, origin, now);
    let serial = only_item_serial(&world);
    let item = world
        .state
        .registry
        .entity_of(Serial::new(serial).unwrap())
        .unwrap();

    world.queue(Command::PickUpItem {
        connection: picker,
        serial,
        amount: 1,
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
        packets_for(&mut world, watcher)
            .iter()
            .any(|p| p[0] == 0x1A),
        "and the player still online sees it reappear"
    );
}

#[test]
fn you_cannot_pick_up_a_mobile() {
    // A body has no `Graphic`, so lifting one is refused rather than yanking
    // a person onto the cursor.
    let now = Instant::now();
    let mut world = world();
    let picker = enter(&mut world, now);
    let other = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let mobile_serial = serial_of(&world, other);
    let _ = packets_for(&mut world, picker);

    world.queue(Command::PickUpItem {
        connection: picker,
        serial: mobile_serial,
        amount: 1,
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, picker)
            .iter()
            .any(|p| p == &[0x27, 0x00]),
        "cannot-lift is the reason"
    );
}

/// A backpack graphic and its gump.
const BACKPACK: u16 = 0x0E75;
const BACKPACK_GUMP: u16 = 0x003C;

fn spawn_container_at(world: &mut World, point: Point, now: Instant) -> u32 {
    world.queue(Command::SpawnContainer {
        graphic: BACKPACK,
        gump: BACKPACK_GUMP,
        hue: 0,
        position: point,
        facet: 0,
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
    world.state.registry.serial_of(entity).unwrap().raw()
}

/// The serial of the one item that is not a container.
fn loose_item_serial(world: &World) -> u32 {
    let (entity, _) = world
        .state
        .registry
        .query::<Graphic>()
        .find(|(entity, _)| !world.state.registry.has::<Container>(*entity))
        .expect("a non-container item exists");
    world.state.registry.serial_of(entity).unwrap().raw()
}

fn entity(world: &World, serial: u32) -> EntityId {
    world
        .state
        .registry
        .entity_of(Serial::new(serial).unwrap())
        .unwrap()
}

#[test]
fn double_clicking_a_container_opens_it() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let container = spawn_container_at(&mut world, Point::new(START.0, START.1, 0), now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DoubleClick {
        connection: player,
        serial: container,
    });
    world.tick(now);

    let packets = packets_for(&mut world, player);
    assert!(packets.iter().any(|p| p[0] == 0x24), "the gump opens");
    assert!(packets.iter().any(|p| p[0] == 0x3C), "the contents follow");
}

/// The serial of the backpack a connection's character is wearing.
fn backpack_serial(world: &World, connection: ConnectionId) -> u32 {
    let owner = world
        .registry()
        .serial_of(world.state.players[&connection])
        .unwrap();
    world
        .registry()
        .query::<Equipped>()
        .find(|(_, worn)| worn.mobile == owner && worn.layer == BACKPACK_LAYER)
        .and_then(|(item, _)| world.registry().serial_of(item))
        .expect("a character wears a backpack")
        .raw()
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
        serial: pack,
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
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);

    world.queue(Command::PickUpItem {
        connection: player,
        serial: item_serial,
        amount: 1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection: player,
        serial: item_serial,
        position: Point::new(0, 0, 0),
        container: pack,
    });
    world.tick(now);

    assert!(
        world.state.registry.has::<Contained>(item),
        "the item is now inside the worn bag"
    );
    assert_eq!(
        world
            .registry()
            .get::<Contained>(item)
            .unwrap()
            .container
            .raw(),
        pack
    );
    assert!(
        !world.state.held.contains_key(&player),
        "and off the cursor, not bounced"
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
    // and nothing else. A raw self-double-click (no bit) opens it too, through
    // the ordinary use rule, when the player is on foot.
    world.queue(Command::DoubleClick {
        connection: player,
        serial: serial | 0x8000_0000,
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x88),
        "the paperdoll request opens the paperdoll"
    );

    world.queue(Command::DoubleClick {
        connection: player,
        serial,
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x88),
        "and so does a raw self-double-click on foot"
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
    let here = Point::new(START.0, START.1, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);
    let _ = packets_for(&mut world, player);

    world.queue(Command::PickUpItem {
        connection: player,
        serial: item_serial,
        amount: 1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection: player,
        serial: item_serial,
        position: Point::new(50, 60, 0), // gump coordinates, not tiles
        container,
    });
    world.tick(now);

    let contained = world
        .state
        .registry
        .get::<Contained>(item)
        .expect("the item is now in a container");
    assert_eq!(contained.container.raw(), container);
    assert_eq!((contained.x, contained.y), (50, 60));
    assert!(
        !world.state.registry.has::<Position>(item),
        "and no longer on the ground"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x25),
        "the client is told the item went in"
    );
}

#[test]
fn an_opened_container_lists_what_was_put_in_it() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.0, START.1, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);

    // Put the item in, then open the container and read the count.
    world.queue(Command::PickUpItem {
        connection: player,
        serial: item_serial,
        amount: 1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection: player,
        serial: item_serial,
        position: Point::new(50, 60, 0),
        container,
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DoubleClick {
        connection: player,
        serial: container,
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
    let here = Point::new(START.0, START.1, 0);
    let container = spawn_container_at(&mut world, here, now);
    spawn_item_at(&mut world, here, now);
    let item_serial = loose_item_serial(&world);
    let item = entity(&world, item_serial);

    // In, then straight back out.
    for _ in 0..1 {
        world.queue(Command::PickUpItem {
            connection: player,
            serial: item_serial,
            amount: 1,
        });
        world.tick(now);
        world.queue(Command::DropItem {
            connection: player,
            serial: item_serial,
            position: Point::new(50, 60, 0),
            container,
        });
        world.tick(now);
    }
    assert!(world.state.registry.has::<Contained>(item));

    world.queue(Command::PickUpItem {
        connection: player,
        serial: item_serial,
        amount: 1,
    });
    world.tick(now);
    assert!(
        !world.state.registry.has::<Contained>(item),
        "lifting it out drops the containment"
    );
    assert!(
        world.state.held.contains_key(&player),
        "and it is on the cursor"
    );
}

#[test]
fn dropping_into_something_that_is_not_a_container_bounces() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.0, START.1, 0);
    // Two plain items: one to hold, one to (wrongly) drop onto.
    spawn_item_at(&mut world, here, now);
    let target = loose_item_serial(&world);
    world.queue(Command::SpawnItem {
        graphic: GOLD,
        hue: 0,
        amount: 1,
        stackable: false,
        position: here,
        facet: 0,
    });
    world.tick(now);
    // The held one is whichever loose item is not the target — not the worn
    // backpack, which is also an item with a graphic now.
    let held_serial = world
        .state
        .registry
        .query::<Graphic>()
        .filter(|(e, _)| !world.state.registry.has::<Equipped>(*e))
        .filter_map(|(e, _)| world.state.registry.serial_of(e).map(|s| s.raw()))
        .find(|s| *s != target)
        .unwrap();
    let held_item = entity(&world, held_serial);

    world.queue(Command::PickUpItem {
        connection: player,
        serial: held_serial,
        amount: 1,
    });
    world.tick(now);
    let origin = Point::new(START.0, START.1, 0);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DropItem {
        connection: player,
        serial: held_serial,
        position: Point::new(0, 0, 0),
        container: target, // a real item, but not a container
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

/// Whether a 4-byte serial appears anywhere in a packet's body.
fn mentions(packet: &[u8], serial: u32) -> bool {
    packet.windows(4).any(|w| w == serial.to_be_bytes())
}

/// Spawn a ground item at the player's feet and pick it up. Returns the item
/// it just made — the newest one, by serial, so earlier items in the world do
/// not confuse it.
fn take_loose_item(world: &mut World, connection: ConnectionId, now: Instant) -> (u32, EntityId) {
    spawn_item_at(world, Point::new(START.0, START.1, 0), now);
    let (item, serial) = world
        .state
        .registry
        .query::<Position>()
        .filter(|(entity, _)| {
            world.state.registry.has::<Graphic>(*entity)
                && !world.state.registry.has::<Container>(*entity)
        })
        .filter_map(|(entity, _)| {
            world
                .state
                .registry
                .serial_of(entity)
                .map(|s| (entity, s.raw()))
        })
        .max_by_key(|(_, serial)| *serial)
        .expect("a ground item to lift");
    world.queue(Command::PickUpItem {
        connection,
        serial,
        amount: 1,
    });
    world.tick(now);
    (serial, item)
}

/// A plausible armour layer.
const LAYER_TORSO: u8 = 5;

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
        item: item_serial,
        layer: LAYER_TORSO,
        mobile: me,
    });
    world.tick(now);

    let worn = world
        .state
        .registry
        .get::<Equipped>(item)
        .expect("the item is now worn");
    assert_eq!(worn.mobile.raw(), me);
    assert_eq!(worn.layer, LAYER_TORSO);
    // Three worn things now: the torso item, and the backpack and bank box every
    // character is given on entry.
    assert_eq!(world.state.equipment_of(Serial::new(me).unwrap()).len(), 3);
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x2E),
        "the wearer is told they put it on"
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
        item: item_serial,
        layer: LAYER_TORSO,
        mobile: me,
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
        item: item_serial,
        layer: LAYER_TORSO,
        mobile: me,
    });
    world.tick(now);
    let _ = packets_for(&mut world, watcher);

    world.queue(Command::PickUpItem {
        connection: player,
        serial: item_serial,
        amount: 1,
    });
    world.tick(now);

    assert!(!world.state.registry.has::<Equipped>(item), "it comes off");
    assert!(
        world.state.held.contains_key(&player),
        "and onto the cursor"
    );
    assert!(
        packets_for(&mut world, watcher)
            .iter()
            .any(|p| p == &encode_remove(item_serial)),
        "the other player is told to forget it"
    );
}

#[test]
fn a_layer_holds_only_one_item() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let me = serial_of(&world, player);

    // First item onto the torso.
    let (first, _) = take_loose_item(&mut world, player, now);
    world.queue(Command::EquipItem {
        connection: player,
        item: first,
        layer: LAYER_TORSO,
        mobile: me,
    });
    world.tick(now);

    // Second item, same layer: refused, and it bounces back to the ground.
    let (second, second_item) = take_loose_item(&mut world, player, now);
    let _ = packets_for(&mut world, player);
    world.queue(Command::EquipItem {
        connection: player,
        item: second,
        layer: LAYER_TORSO,
        mobile: me,
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x27),
        "the second is refused"
    );
    assert!(
        world.state.registry.has::<Position>(second_item),
        "and returns to where it was lifted"
    );
    assert!(!world.state.registry.has::<Equipped>(second_item));
}

#[test]
fn you_cannot_equip_onto_something_that_is_not_a_mobile() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    // A second ground item to (wrongly) equip onto.
    spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);
    let target = loose_item_serial(&world);
    let (held, held_item) = take_loose_item(&mut world, player, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::EquipItem {
        connection: player,
        item: held,
        layer: LAYER_TORSO,
        mobile: target, // an item, not a mobile
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
    let here = Point::new(START.0, START.1, 0);
    let pile = spawn_gold(&mut world, here, 100, now);
    let loose = spawn_gold(&mut world, here, 50, now);
    let pile_item = entity(&world, pile);
    let loose_item = entity(&world, loose);
    let _ = packets_for(&mut world, player);

    // Lift the small pile and drop it onto the big one.
    world.queue(Command::PickUpItem {
        connection: player,
        serial: loose,
        amount: 50,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection: player,
        serial: loose,
        position: here,
        container: pile, // dropping onto the other stack
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
fn a_non_stackable_item_does_not_merge() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.0, START.1, 0);
    // Two plain (non-stackable) items.
    spawn_item_at(&mut world, here, now);
    let target = loose_item_serial(&world);
    let (held, held_item) = take_loose_item(&mut world, player, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::DropItem {
        connection: player,
        serial: held,
        position: here,
        container: target,
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
    spawn_item_at(&mut world, Point::new(START.0, START.1, 0), now);
    let serial = loose_item_serial(&world);
    let item = entity(&world, serial);
    let _ = packets_for(&mut world, watcher);

    // Bring the decay forward rather than run twenty minutes of ticks.
    let soon = world.state.ticks + 1;
    world.state.registry.insert(item, Decays { at_tick: soon });
    world.tick(now);

    assert!(
        !world.state.registry.contains(item),
        "the item has rotted away"
    );
    assert!(
        packets_for(&mut world, watcher)
            .iter()
            .any(|p| p == &encode_remove(serial)),
        "and left every screen"
    );
}

#[test]
fn gameplay_config_reaches_the_systems() {
    // The [gameplay] knobs flow through WorldState to the systems: a five-second
    // decay here gives a spawned item a clock of a hundred ticks, not the
    // twenty-minute default's twenty-four thousand.
    let now = Instant::now();
    let gameplay = Gameplay::new(
        2,
        40000,
        700,
        5,
        60,
        18,
        3,
        31,
        400,
        openshard_state::CastStyle::Stop,
        true,
        openshard_state::TooltipMode::SendVersion,
        true,
    );
    let mut world = World::new(START).with_gameplay(gameplay);
    world.queue(Command::SpawnItem {
        graphic: 0x0EED,
        hue: 0,
        amount: 1,
        stackable: false,
        position: Point::new(START.0, START.1, 0),
        facet: 0,
    });
    world.tick(now);

    let serial = loose_item_serial(&world);
    let item = entity(&world, serial);
    let decay = world.state.registry.get::<Decays>(item).unwrap();
    assert!(
        decay.at_tick > world.state.ticks && decay.at_tick <= world.state.ticks + 100,
        "the five-second decay reached mark_decay (at_tick {}, now {})",
        decay.at_tick,
        world.state.ticks
    );
}

#[test]
fn a_container_does_not_decay_even_after_being_moved() {
    // A backpack is a ground item too, but it must not rot — and picking it
    // up and setting it back down must not hand it a decay clock either.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.0, START.1, 0);
    let container = spawn_container_at(&mut world, here, now);
    let container_item = entity(&world, container);
    assert!(
        !world.state.registry.has::<Decays>(container_item),
        "a fresh container has no decay clock"
    );

    world.queue(Command::PickUpItem {
        connection: player,
        serial: container,
        amount: 1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection: player,
        serial: container,
        position: here,
        container: DROP_TO_GROUND,
    });
    world.tick(now);

    assert!(
        world.state.registry.has::<Position>(container_item),
        "back down"
    );
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
    let here = Point::new(START.0, START.1, 0);
    let pile = spawn_gold(&mut world, here, 100, now);
    let pile_item = entity(&world, pile);
    let _ = packets_for(&mut world, player);

    world.queue(Command::PickUpItem {
        connection: player,
        serial: pile,
        amount: 30,
    });
    world.tick(now);

    // The original, still serial `pile`, is on the cursor holding 30.
    assert!(world.state.held.contains_key(&player));
    assert_eq!(openshard_items::amount_of(&world.state, pile_item), 30);
    assert!(
        !world.state.registry.has::<Position>(pile_item),
        "off the ground"
    );

    // A brand-new pile of 70 sits where the stack was.
    let (leftover, _) = world
        .state
        .registry
        .query::<Position>()
        .find(|(entity, _)| world.state.registry.has::<Stackable>(*entity) && *entity != pile_item)
        .expect("a leftover pile on the ground");
    assert_eq!(openshard_items::amount_of(&world.state, leftover), 70);
    assert_ne!(
        world.state.registry.serial_of(leftover).unwrap().raw(),
        pile,
        "the leftover is a new object with a new serial"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|p| p[0] == 0x1A),
        "and the player is drawn the leftover pile"
    );
}

#[test]
fn the_split_portion_keeps_its_serial_and_can_be_dropped() {
    // The reason the original keeps its serial: the client's cursor still
    // names it, so the 0x08 that drops the 30 back matches the held item.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let here = Point::new(START.0, START.1, 0);
    let pile = spawn_gold(&mut world, here, 100, now);
    let pile_item = entity(&world, pile);

    world.queue(Command::PickUpItem {
        connection: player,
        serial: pile,
        amount: 30,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection: player,
        serial: pile, // the client drops the same serial it lifted
        position: here,
        container: DROP_TO_GROUND,
    });
    world.tick(now);

    assert!(world.state.held.is_empty(), "the drop landed, not bounced");
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
    let here = Point::new(START.0, START.1, 0);
    let pile = spawn_gold(&mut world, here, 100, now);
    let pile_item = entity(&world, pile);

    world.queue(Command::PickUpItem {
        connection: player,
        serial: pile,
        amount: 100,
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

/// Spawn a creature at `point` with `hits` and return its serial. An orange
/// enemy, no armour — the plain punching bag most combat tests want.
fn spawn_mobile_at(world: &mut World, point: Point, hits: u16, now: Instant) -> u32 {
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
) -> u32 {
    world.queue(Command::SpawnMobile {
        body: 0x0190,
        hue: 0,
        hits,
        notoriety,
        damage,
        resistance,
        swing: 0, // the default pace
        sight: 0, // passive by default; tests that want a brain set it
        aggression: 2,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: point,
        facet: 0,
        name: None,
        banker: false,
        vendor: false,
        equipment: Vec::new(),
    });
    world.tick(now);
    // The newest mobile that no client drives — the creature just made.
    world
        .state
        .registry
        .query::<Body>()
        .filter(|(entity, _)| !world.state.registry.has::<Client>(*entity))
        .filter_map(|(entity, _)| world.state.registry.serial_of(entity).map(|s| s.raw()))
        .max()
        .expect("a spawned creature")
}

#[test]
fn a_spawned_creature_is_drawn_to_nearby_players() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let _ = packets_for(&mut world, player);
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);

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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
    let mob_entity = entity(&world, mob);
    let _ = packets_for(&mut world, player);

    world.queue(Command::Damage {
        serial: mob,
        amount: 20,
        damage_type: 0,
        by: 0,
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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 10, now);
    let mob_entity = entity(&world, mob);
    let _ = packets_for(&mut world, player);
    let mut died: Cursor<MobileDied> = world.bus().cursor();

    // Overkill: it dies once, not into the negatives.
    world.queue(Command::Damage {
        serial: mob,
        amount: 100,
        damage_type: 0,
        by: 0,
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
            .any(|p| p == &encode_remove(mob)),
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
        by: 0,
    });
    world.tick(now); // 100 -> 0
    assert_eq!(world.bus().read(&mut died).count(), 1, "the killing blow");

    world.queue(Command::Damage {
        serial,
        amount: 50,
        damage_type: 0,
        by: 0,
    });
    world.tick(now); // already dead
    assert_eq!(
        world.bus().read(&mut died).count(),
        0,
        "a second blow on a corpse announces nothing"
    );
}

#[test]
fn a_player_who_dies_stays_in_the_world() {
    // Ghosts and corpses are a later slice; for now death is announced but a
    // connected player is not yanked out of the world.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = serial_of(&world, player);
    let player_entity = world.state.players[&player];
    let mut died: Cursor<MobileDied> = world.bus().cursor();

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: 0,
    });
    world.tick(now);

    assert_eq!(world.bus().read(&mut died).count(), 1, "death is announced");
    assert!(
        world.state.registry.contains(player_entity),
        "but the player is still here"
    );
    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(player_entity)
            .map(|h| h.current),
        Some(0),
    );
}

/// Put a player in war mode, aimed at `target`, in one tick.
fn engage(world: &mut World, player: ConnectionId, target: u32, now: Instant) {
    world.queue(Command::WarMode {
        connection: player,
        war: true,
    });
    world.queue(Command::Attack {
        connection: player,
        target,
    });
    world.tick(now);
}

#[test]
fn war_mode_and_attack_are_confirmed_to_the_client() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::WarMode {
        connection: player,
        war: true,
    });
    world.queue(Command::Attack {
        connection: player,
        target: mob,
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
}

#[test]
fn a_player_in_war_mode_swings_at_an_adjacent_target() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);

    // One swing interval later, a blow has landed.
    for _ in 0..WRESTLING_SWING_TICKS {
        world.tick(now);
    }
    assert!(
        world
            .state
            .registry
            .get::<Hitpoints>(mob_entity)
            .unwrap()
            .current
            < 50,
        "the target has taken damage"
    );
}

#[test]
fn no_swing_without_war_mode() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
    let mob_entity = entity(&world, mob);

    // Aim, but stay at peace.
    world.queue(Command::Attack {
        connection: player,
        target: mob,
    });
    world.tick(now);
    for _ in 0..(WRESTLING_SWING_TICKS + 1) {
        world.tick(now);
    }
    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(mob_entity)
            .unwrap()
            .current,
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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0 + 5, START.1, 0), 50, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);
    for _ in 0..(WRESTLING_SWING_TICKS + 1) {
        world.tick(now);
    }
    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(mob_entity)
            .unwrap()
            .current,
        50,
        "a swing out of reach lands nothing"
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
    let mob = spawn_mobile_full(
        &mut world,
        Point::new(START.0, START.1, 0),
        50,
        5,
        5,
        0,
        now,
    );

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
    let mob = spawn_mobile_full(
        &mut world,
        Point::new(START.0, START.1, 0),
        50,
        7,
        5,
        0,
        now,
    );
    let _ = packets_for(&mut world, player);

    world.queue(Command::Attack {
        connection: player,
        target: mob,
    });
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<Combat>(player_entity)
            .unwrap()
            .target,
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

    world.queue(Command::Attack {
        connection: aggressor,
        target: victim_serial,
    });
    world.tick(now);

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
            Point::new(START.0, START.1, 0),
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
            world
                .state
                .registry
                .entity_of(Serial::new(victim).unwrap())
                .is_none(),
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
            Point::new(START.0 + 5, START.1, 0),
            1,
            Notoriety::Innocent.to_bits(),
            0,
            0,
            now,
        );
        world.queue(Command::Damage {
            serial: victim,
            amount: 100,
            damage_type: 0,
            by: killer_serial,
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
        world
            .state
            .registry
            .get::<Murders>(killer_entity)
            .map(|m| m.0),
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
            Point::new(START.0 + 5, START.1, 0),
            1,
            Notoriety::Innocent.to_bits(),
            0,
            0,
            now,
        );
        world.queue(Command::Damage {
            serial: victim,
            amount: 100,
            damage_type: 0,
            by: killer_serial,
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
    // op_damage, an environmental hazard) kills but pins no murder.
    let now = Instant::now();
    let mut world = world();
    let bystander = enter(&mut world, now);
    let bystander_entity = world.state.players[&bystander];

    for _ in 0..5 {
        let victim = spawn_mobile_full(
            &mut world,
            Point::new(START.0 + 5, START.1, 0),
            1,
            Notoriety::Innocent.to_bits(),
            0,
            0,
            now,
        );
        world.queue(Command::Damage {
            serial: victim,
            amount: 100,
            damage_type: 0,
            by: 0,
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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);

    world.queue(Command::Attack {
        connection: player,
        target: mob,
    });
    world.tick(now);

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

    world.queue(Command::Attack {
        connection: aggressor,
        target: victim_serial,
    });
    world.tick(now);
    assert_eq!(
        world.state.notoriety_of(aggressor_entity),
        Notoriety::Criminal
    );

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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 100, now);
    let mob_entity = entity(&world, mob);
    world.state.registry.insert(
        mob_entity,
        Resistance {
            fire: 50,
            ..Default::default()
        },
    );
    let _ = packets_for(&mut world, player);

    // 10 fire, halved to 5.
    world.queue(Command::Damage {
        serial: mob,
        amount: 10,
        damage_type: 1, // fire
        by: 0,
    });
    world.tick(now);
    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(mob_entity)
            .unwrap()
            .current,
        95
    );

    // 10 physical, unresisted.
    world.queue(Command::Damage {
        serial: mob,
        amount: 10,
        damage_type: 0, // physical
        by: 0,
    });
    world.tick(now);
    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(mob_entity)
            .unwrap()
            .current,
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
    let mob = spawn_mobile_full(
        &mut world,
        Point::new(START.0, START.1, 0),
        50,
        5,
        5,
        50,
        now,
    );
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);

    for _ in 0..WRESTLING_SWING_TICKS {
        world.tick(now);
    }
    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(mob_entity)
            .unwrap()
            .current,
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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 100, now);
    let mob_entity = entity(&world, mob);
    engage(&mut world, player, mob, now);

    // Five is fewer than the default interval, but a full fast one.
    const _: () = assert!(5 < WRESTLING_SWING_TICKS);
    for _ in 0..5 {
        world.tick(now);
    }
    assert!(
        world
            .state
            .registry
            .get::<Hitpoints>(mob_entity)
            .unwrap()
            .current
            < 100,
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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
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

    assert_eq!(
        slow, WRESTLING_SWING_TICKS,
        "default dexterity, default pace"
    );
    assert!(fast < slow, "more dexterity swings sooner: {fast} < {slow}");
}

#[test]
fn killing_the_target_ends_the_attack() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_entity = world.state.players[&player];
    // Eight hits, five a swing: dead on the second.
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 8, now);
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
            .unwrap()
            .target,
        None,
        "and the attacker is no longer swinging at it"
    );
}

/// A mobile's value in a skill, in tenths.
fn skill_value(world: &World, entity: EntityId, skill: u8) -> u16 {
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
    use openshard_protocol::SkillLock;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];

    world.queue(Command::SetSkillLock {
        connection,
        skill: 45, // Mining
        lock: SkillLock::Down,
    });
    world.tick(now);
    assert_eq!(
        world
            .registry()
            .get::<Skills>(entity)
            .map_or(SkillLock::Up, |s| s.lock(45)),
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
    for _ in 0..80 {
        world.queue(Command::UseSkill {
            serial,
            skill: 1,
            difficulty: 0,
        });
        world.tick(now);
        saw_update |= packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x3A && p[3] == 0xDF);
        if saw_update {
            break;
        }
    }
    assert!(
        saw_update,
        "a gain pushed a single-skill update to the window"
    );
}

#[test]
fn a_characters_stats_and_skills_survive_a_relogin() {
    use openshard_protocol::SkillLock;
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
        skill: 25,
        lock: SkillLock::Down,
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
    let magery = record
        .skills
        .iter()
        .find(|s| s.id == 25)
        .expect("magery saved");
    assert_eq!(magery.value, 501);
    assert_eq!(
        magery.lock,
        SkillLock::Down.to_bits(),
        "the lock is saved too"
    );

    // Relogin, threading the record back through Enter the way the server does.
    world.queue(Command::Disconnect { connection: conn });
    world.tick(now);
    let conn = connection();
    world.queue(Command::Enter {
        connection: conn,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: Some(serial),
        position: Some(Point::new(START.0, START.1, 0)),
        facet: 0,
        appearance: None,
        sheet: Some(CharacterSheet {
            strength: record.strength,
            dexterity: record.dexterity,
            intelligence: record.intelligence,
            skills: record
                .skills
                .iter()
                .map(|s| (s.id, s.value, SkillLock::from_bits(s.lock)))
                .collect(),
            effects: record.effects.clone(),
        }),
        access: AccessLevel::Player,
    });
    world.tick(now);
    let player = world.state.players[&conn];
    assert_eq!(
        world.registry().get::<Stats>(player).unwrap().strength,
        55,
        "stats came back"
    );
    assert_eq!(skill_value(&world, player, 25), 501, "the skill came back");
    assert_eq!(
        world.registry().get::<Skills>(player).unwrap().lock(25),
        SkillLock::Down,
        "and its lock arrow"
    );
}

// -- spell casting --------------------------------------------------------

/// A reagent graphic used by the cast tests.
const BLACK_PEARL: u16 = 0x0F7A;

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
    let backpack = Serial::new(backpack_serial(world, connection)).unwrap();
    openshard_items::give(&mut world.state, backpack, reagent, 0, 20);
    let _ = packets_for(world, connection);
    (connection, entity)
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
        spell: 17,
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
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x6C),
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
        spell: 17,
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

    // Wait out the cast delay.
    let mut later = now;
    for _ in 0..20 {
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
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x6C),
        "then the target cursor came up"
    );
}

#[test]
fn stepping_breaks_a_cast() {
    let now = Instant::now();
    let mut world = world();
    let (connection, entity) = ready_caster(&mut world, BLACK_PEARL, now);
    world.queue(Command::RequestCast {
        connection,
        spell: 17,
    });
    world.tick(now);
    assert!(world
        .registry()
        .get::<openshard_state::components::Casting>(entity)
        .is_some());

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
        spell: 17,
    });
    world.tick(now);
    assert!(world
        .registry()
        .get::<openshard_state::components::Casting>(entity)
        .is_some());

    world.queue(Command::Damage {
        serial,
        amount: 5,
        damage_type: 0,
        by: 0,
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
    let target = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 50, now);

    world.queue(Command::RequestCast {
        connection,
        spell: 17,
    });
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::TargetResponse {
            cursor_id: 0,
            serial: target,
            location: Point::new(0, 0, 0),
            graphic: 0,
            cancelled: false,
        },
    });
    world.tick(now);

    let target_entity = world
        .registry()
        .entity_of(Serial::new(target).unwrap())
        .expect("the target");
    assert!(
        world
            .registry()
            .get::<Hitpoints>(target_entity)
            .unwrap()
            .current
            < 50,
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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
    let entity = world
        .registry()
        .entity_of(Serial::new(mob).unwrap())
        .unwrap();
    let ticks = world.state.ticks;
    combat::apply_poison(&mut world.state, mob, 2, ticks); // greater
    assert!(
        world.registry().get::<Poisoned>(entity).is_some(),
        "poisoned"
    );

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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
    let entity = world
        .registry()
        .entity_of(Serial::new(mob).unwrap())
        .unwrap();
    let ticks = world.state.ticks;
    combat::apply_poison(&mut world.state, mob, 2, ticks);
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
    use openshard_protocol::SkillLock;
    use openshard_state::components::Poisoned;
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let serial = serial_of(&world, conn);

    // Poison the character, then let the save sweep the world.
    let ticks = world.state.ticks;
    combat::apply_poison(&mut world.state, serial, 2, ticks); // greater
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
    world.queue(Command::Enter {
        connection: conn,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: Some(serial),
        position: Some(Point::new(START.0, START.1, 0)),
        facet: 0,
        appearance: None,
        sheet: Some(CharacterSheet {
            strength: record.strength,
            dexterity: record.dexterity,
            intelligence: record.intelligence,
            skills: record
                .skills
                .iter()
                .map(|s| (s.id, s.value, SkillLock::from_bits(s.lock)))
                .collect(),
            effects: record.effects.clone(),
        }),
        access: AccessLevel::Player,
    });
    world.tick(now);

    let player = world.state.players[&conn];
    let poisoned = world
        .registry()
        .get::<Poisoned>(player)
        .expect("still poisoned after the relog — no free cure");
    assert_eq!(poisoned.level, 2, "and at the same strength");
}

#[test]
fn a_poisoned_creature_comes_back_poisoned() {
    // The mobile half of the same rule: a creature's effects ride the mobile
    // sweep the way its wounds do, so a restart does not cure the region's
    // monsters either.
    use openshard_state::components::Poisoned;
    let now = Instant::now();
    let mut home = world();
    let mob = spawn_mobile_at(&mut home, Point::new(START.0, START.1, 0), 50, now);
    let ticks = home.state.ticks;
    combat::apply_poison(&mut home.state, mob, 1, ticks); // lesser

    home.take_snapshot();
    let snapshot = home.drain_saves().next_back().expect("a snapshot");
    let mobiles = snapshot.mobiles.clone().expect("a mobile sweep");
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
    shard.restore_mobiles(mobiles);
    let creature = shard
        .registry()
        .entity_of(Serial::new(mob).unwrap())
        .expect("the creature came back");
    assert_eq!(
        shard
            .registry()
            .get::<Poisoned>(creature)
            .expect("still poisoned")
            .level,
        1,
    );
}

#[test]
fn a_stat_buff_shifts_stats_and_pools_then_expires() {
    use openshard_state::components::{Mana, StatMods, Stats};
    use openshard_state::effect;
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
    magic::apply_stat_buff(&mut world.state, serial, effect::BLESS, 10, expires_at);
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
    use openshard_state::components::{StatMods, Stats};
    use openshard_state::effect;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let base = *world.registry().get::<Stats>(entity).unwrap();

    let at = world.state.ticks;
    magic::apply_stat_buff(&mut world.state, serial, effect::STRENGTH, 5, at + 100);
    magic::apply_stat_buff(&mut world.state, serial, effect::STRENGTH, 5, at + 200);

    assert_eq!(
        world.registry().get::<Stats>(entity).unwrap().strength,
        base.strength + 5,
        "a recast refreshes the same +5, it does not stack a second"
    );
    assert_eq!(
        world
            .registry()
            .get::<StatMods>(entity)
            .unwrap()
            .active
            .len(),
        1,
        "one entry, not two"
    );
}

#[test]
fn a_debuff_clamps_the_current_pool_to_the_lowered_cap() {
    use openshard_state::effect;
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = serial_of(&world, connection);
    let full = world.registry().get::<Hitpoints>(entity).unwrap().max;

    // Curse lowers strength, so the hit-point cap drops; a full bar must follow it
    // down rather than sit above the new maximum.
    let at = world.state.ticks;
    magic::apply_stat_buff(&mut world.state, serial, effect::CURSE, -10, at + 100);
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
    use openshard_protocol::SkillLock;
    use openshard_state::components::{StatMods, Stats};
    use openshard_state::effect;
    let now = Instant::now();
    let mut world = world();
    let conn = enter(&mut world, now);
    let entity = world.state.players[&conn];
    let serial = serial_of(&world, conn);
    let base = *world.registry().get::<Stats>(entity).unwrap();

    let at = world.state.ticks;
    magic::apply_stat_buff(&mut world.state, serial, effect::BLESS, 10, at + 100);
    let buffed = *world.registry().get::<Stats>(entity).unwrap();

    world.take_snapshot();
    let snapshot = world.drain_saves().next_back().expect("a snapshot");
    let record = snapshot
        .characters
        .iter()
        .find(|c| c.serial == serial)
        .cloned()
        .expect("saved");
    assert_eq!(
        record.strength, buffed.strength,
        "the buffed stat went to disk"
    );
    assert!(
        record.effects.iter().any(|e| e.kind == effect::BLESS),
        "and the buff's ledger entry with it"
    );

    // Relogin, threading the record back the way the server does.
    world.queue(Command::Disconnect { connection: conn });
    world.tick(now);
    let conn = connection();
    world.queue(Command::Enter {
        connection: conn,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: Some(serial),
        position: Some(Point::new(START.0, START.1, 0)),
        facet: 0,
        appearance: None,
        sheet: Some(CharacterSheet {
            strength: record.strength,
            dexterity: record.dexterity,
            intelligence: record.intelligence,
            skills: record
                .skills
                .iter()
                .map(|s| (s.id, s.value, SkillLock::from_bits(s.lock)))
                .collect(),
            effects: record.effects.clone(),
        }),
        access: AccessLevel::Player,
    });
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
fn the_bless_spell_raises_the_targets_stats() {
    use openshard_state::components::Stats;
    const GARLIC: u16 = 0x0F84;
    const MANDRAKE_ROOT: u16 = 0x0F86;
    let now = Instant::now();
    let mut world = sphere_world();
    let (connection, entity) = ready_caster(&mut world, GARLIC, now);
    let self_serial = serial_of(&world, connection);
    let backpack = Serial::new(backpack_serial(&world, connection)).unwrap();
    openshard_items::give(&mut world.state, backpack, MANDRAKE_ROOT, 0, 20);
    let base = *world.registry().get::<Stats>(entity).unwrap();

    world.queue(Command::RequestCast {
        connection,
        spell: 16,
    }); // Bless
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::TargetResponse {
            cursor_id: 0,
            serial: self_serial,
            location: Point::new(0, 0, 0),
            graphic: 0,
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
    let target = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 50, now);

    world.queue(Command::RequestCast {
        connection,
        spell: 19,
    }); // Poison
    world.tick(now);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::TargetResponse {
            cursor_id: 0,
            serial: target,
            location: Point::new(0, 0, 0),
            graphic: 0,
            cancelled: false,
        },
    });
    world.tick(now);

    let entity = world
        .registry()
        .entity_of(Serial::new(target).unwrap())
        .unwrap();
    assert!(
        world.registry().get::<Poisoned>(entity).is_some(),
        "the Poison spell left its mark"
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
        spell: 17,
    });
    world.tick(now);
    assert_eq!(
        world.registry().get::<Mana>(entity).unwrap().current,
        mana_before,
        "a fizzle for want of a reagent spends nothing"
    );
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
        difficulty: 0,
    });
    world.tick(now);

    let events: Vec<SkillUsed> = world.bus().read(&mut used).copied().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].skill, 1);
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
            difficulty: 0,
        });
        world.tick(now);
    }
    assert!(
        skill_value(&world, entity, 1) > 0,
        "practice taught something"
    );
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
        value: skills::SKILL_CAP,
    });
    world.tick(now);

    for _ in 0..30 {
        world.queue(Command::UseSkill {
            serial,
            skill: 1,
            difficulty: 0,
        });
        world.tick(now);
    }
    assert_eq!(
        skill_value(&world, entity, 1),
        skills::SKILL_CAP,
        "there is nothing left to learn at the cap"
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
                difficulty: 40,
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
        spell: 5,
        target: 0,
        mana: 10,
        difficulty: 0,
        skill: 1,
        pack: 0,
        reagents: Vec::new(),
    });
    world.tick(now);

    let events: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].spell, 5);
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
    let pack = spawn_container_at(&mut world, Point::new(START.0, START.1, 0), now);
    let container = openshard_entities::Serial::new(pack).unwrap();
    for _ in 0..3 {
        let (item, _) = world
            .state
            .registry
            .spawn_with_serial(openshard_entities::SerialKind::Item)
            .unwrap();
        world.state.registry.insert(
            item,
            Graphic {
                id: REAGENT,
                hue: 0,
            },
        );
        world.state.registry.insert(
            item,
            Contained {
                container,
                x: 0,
                y: 0,
                grid: 0,
            },
        );
    }

    let spell = |reagents: Vec<(u16, u16)>| Command::CastSpell {
        serial,
        spell: 5,
        target: 0,
        mana: 10,
        difficulty: 0,
        skill: 1,
        pack,
        reagents,
    };
    let mut cast: Cursor<SpellCast> = world.bus().cursor();

    // First cast needs two; the pack has three, so it takes them and casts.
    world.queue(spell(vec![(REAGENT, 2)]));
    world.tick(now);
    let first: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
    assert!(first[0].success, "the stocked pack lets it cast");
    assert_eq!(
        openshard_items::count_in_container(&world.state, container, REAGENT),
        1,
        "two of the three reagents were consumed"
    );

    // One left; a second cast needing two fizzles and spends nothing.
    let mana = world.state.registry.get::<Mana>(entity).unwrap().current;
    world.queue(spell(vec![(REAGENT, 2)]));
    world.tick(now);
    let second: Vec<SpellCast> = world.bus().read(&mut cast).copied().collect();
    assert!(!second[0].success, "one reagent left is not enough");
    assert_eq!(
        world.state.registry.get::<Mana>(entity).unwrap().current,
        mana,
        "a fizzle spends no mana"
    );
    assert_eq!(
        openshard_items::count_in_container(&world.state, container, REAGENT),
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
    let pack = spawn_container_at(&mut world, Point::new(START.0, START.1, 0), now);
    let container = openshard_entities::Serial::new(pack).unwrap();
    let (_, item_serial) = world
        .state
        .registry
        .spawn_with_serial(openshard_entities::SerialKind::Item)
        .unwrap();
    let item = world.state.registry.entity_of(item_serial).unwrap();
    world.state.registry.insert(
        item,
        Graphic {
            id: REAGENT,
            hue: 0,
        },
    );
    world.state.registry.insert(
        item,
        Contained {
            container,
            x: 0,
            y: 0,
            grid: 0,
        },
    );

    // Open it, then clear what has been sent so far.
    world.queue(Command::DoubleClick {
        connection: player,
        serial: pack,
    });
    world.tick(now);
    let _ = packets_for(&mut world, player);

    // Cast, burning the reagent out of the open pack.
    world.queue(Command::CastSpell {
        serial,
        spell: 5,
        target: 0,
        mana: 10,
        difficulty: 0,
        skill: 1,
        pack,
        reagents: vec![(REAGENT, 1)],
    });
    world.tick(now);

    assert!(
        packets_for(&mut world, player)
            .iter()
            .any(|p| p == &encode_remove(item_serial.raw())),
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
        spell: 1,
        target: 0,
        mana: 200, // more than the 100 on hand
        difficulty: 0,
        skill: 1,
        pack: 0,
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
        by: 0,
    });
    world.tick(now); // 100 -> 40
    world.queue(Command::Heal {
        serial,
        amount: 1000,
    });
    world.tick(now);

    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(entity)
            .unwrap()
            .current,
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
        spell: 1,
        target: 0,
        mana: 20,
        difficulty: 0,
        skill: 1,
        pack: 0,
        reagents: Vec::new(),
    });
    world.tick(now);
    let spent = world.state.registry.get::<Mana>(entity).unwrap().current;

    for _ in 0..MANA_REGEN_TICKS {
        world.tick(now);
    }
    assert!(
        world.state.registry.get::<Mana>(entity).unwrap().current > spent,
        "mana came back over time"
    );
}

/// Spawn a creature with a brain (sight, wander) and return its serial.
fn spawn_creature(world: &mut World, point: Point, sight: u8, wander: bool, now: Instant) -> u32 {
    world.queue(Command::SpawnMobile {
        body: 0x0190,
        hue: 0,
        hits: 50,
        notoriety: 5,
        damage: combat::SWING_DAMAGE,
        resistance: 0,
        swing: 0,
        sight,
        aggression: 2,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander,
        position: point,
        facet: 0,
        name: None,
        banker: false,
        vendor: false,
        equipment: Vec::new(),
    });
    world.tick(now);
    world
        .state
        .registry
        .query::<Body>()
        .filter(|(entity, _)| !world.state.registry.has::<Client>(*entity))
        .filter_map(|(entity, _)| world.state.registry.serial_of(entity).map(|s| s.raw()))
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
    spawn_creature(&mut world, Point::new(START.0, START.1, 0), 10, false, now);

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
fn an_aggressive_creature_chases_a_player() {
    let now = Instant::now();
    let mut world = world();
    enter(&mut world, now); // a player at START to be chased
    let start = Point::new(START.0 + 4, START.1, 0);
    let mob = spawn_creature(&mut world, start, 10, false, now);
    let mob_entity = entity(&world, mob);

    // Several beats: it turns, then walks toward the player.
    for _ in 0..(5 * AI_THINK_TICKS) {
        world.tick(now);
    }
    assert!(
        world
            .state
            .registry
            .get::<Position>(mob_entity)
            .unwrap()
            .0
            .x
            < start.x,
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
    spawn_creature(&mut world, Point::new(START.0, START.1, 0), 0, false, now);

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
    let start = Point::new(START.0, START.1, 0);
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
    assert_eq!(
        (mana.current, mana.max),
        (40, 40),
        "mana follows intelligence"
    );
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
        mode: 0,
        hue: 0x0384,
        font: 3,
        text: "hail".to_owned(),
    });
    world.tick(now);

    // Drain once — both players' packets came out of the same tick.
    let all: Vec<Outbound> = world.drain_outbound().collect();
    assert!(
        all.iter()
            .any(|o| o.connection == speaker && o.packet[0] == 0xAE),
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
    teleport(&mut world, listener, Point::new(START.0 + 40, START.1, 0));
    let _ = packets_for(&mut world, listener);

    world.queue(Command::Say {
        connection: speaker,
        mode: 0,
        hue: 0,
        font: 3,
        text: "hail".to_owned(),
    });
    world.tick(now);

    assert!(
        !packets_for(&mut world, listener)
            .iter()
            .any(|p| p[0] == 0xAE),
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
    teleport(&mut world, listener, Point::new(START.0 + 10, START.1, 0));
    let _ = packets_for(&mut world, listener);

    world.queue(Command::Say {
        connection: speaker,
        mode: TALKMODE_WHISPER,
        hue: 0,
        font: 3,
        text: "psst".to_owned(),
    });
    world.tick(now);

    assert!(
        !packets_for(&mut world, listener)
            .iter()
            .any(|p| p[0] == 0xAE),
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
    teleport(&mut world, listener, Point::new(START.0 + 25, START.1, 0));
    let _ = packets_for(&mut world, listener);

    // Said normally, it does not reach.
    world.queue(Command::Say {
        connection: speaker,
        mode: 0,
        hue: 0,
        font: 3,
        text: "here".to_owned(),
    });
    world.tick(now);
    assert!(
        !packets_for(&mut world, listener)
            .iter()
            .any(|p| p[0] == 0xAE),
        "normal speech stops short of twenty-five tiles"
    );

    // Yelled, it does.
    world.queue(Command::Say {
        connection: speaker,
        mode: TALKMODE_YELL,
        hue: 0,
        font: 3,
        text: "here".to_owned(),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, listener)
            .iter()
            .any(|p| p[0] == 0xAE),
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
            mode: 0,
            hue: 0,
            font: 3,
            text: text.to_owned(),
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
        mode: 0,
        hue: 0,
        font: 3,
        text: "hello world".to_owned(),
    });
    world.tick(now);

    let events: Vec<MobileSpoke> = world.bus().read(&mut spoke).cloned().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].text, "hello world");
}

fn gm_say(world: &mut World, connection: ConnectionId, text: &str, now: Instant) {
    world.queue(Command::Say {
        connection,
        mode: 0,
        hue: 0,
        font: 3,
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

    assert_eq!(
        world.bus().read(&mut spoke).count(),
        0,
        "no one heard a command"
    );
    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x1C),
        "the GM got a private system answer"
    );
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
    assert_eq!(
        events[0].text, ".hello",
        "a player's dot-text is spoken verbatim"
    );
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
        &format!(".go {} {}", START.0 + 5, START.1 + 7),
        now,
    );
    let Position(at) = *world.registry().get::<Position>(entity).unwrap();
    assert_eq!((at.x, at.y), (START.0 + 5, START.1 + 7), "the GM moved");

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
        response: openshard_protocol::GumpResponse {
            serial: 0,
            gump_id: crate::admin::ADMIN_GUMP,
            button,
            switches: Vec::new(),
            text_entries: Vec::new(),
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
    assert_eq!(
        before.0.x, START.0,
        "the GM has not moved on raising the cursor"
    );

    // The click comes back as a 0x6C response; the GM jumps to the spot.
    let target = Point::new(START.0 + 9, START.1 + 3, before.0.z);
    world.queue(Command::TargetResponse {
        connection: gm,
        response: openshard_protocol::TargetResponse {
            cursor_id: 0,
            serial: 0,
            location: target,
            graphic: 0,
            cancelled: false,
        },
    });
    world.tick(now);
    let Position(at) = *world.registry().get::<Position>(entity).unwrap();
    assert_eq!(
        (at.x, at.y),
        (target.x, target.y),
        "the click teleported the GM"
    );
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
        response: openshard_protocol::TargetResponse {
            cursor_id: 0,
            serial: 0,
            location: Point::new(START.0 + 9, START.1 + 3, before.0.z),
            graphic: 0,
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

    world.queue(admin_response(gm, 10)); // Populate Britain
    world.tick(now);

    assert!(
        packets_for(&mut world, gm).iter().any(|p| p[0] == 0x1C),
        "the button is acted on"
    );
}

#[test]
fn decorate_places_statics_and_clear_removes_them() {
    use openshard_state::components::Decoration;
    let now = Instant::now();
    let mut world = world();
    let _gm = enter_gm(&mut world, now);

    world.queue(Command::Decorate {
        facet: 0,
        statics: vec![
            (0x07C1, 0, Point::new(START.0 + 1, START.1, 0)),
            (0x08DA, 0, Point::new(START.0 + 2, START.1, 0)),
        ],
        doors: Vec::new(),
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
    assert!(
        !world.registry().has::<Decays>(decor),
        "decoration does not rot"
    );

    world.queue(Command::ClearDecorations);
    world.tick(now);
    assert_eq!(
        world.registry().query::<Decoration>().count(),
        0,
        "clear removed the decoration"
    );
}

#[test]
fn decoration_cannot_be_picked_up() {
    use openshard_state::components::Decoration;
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    world.queue(Command::Decorate {
        facet: 0,
        statics: vec![(0x07C1, 0, Point::new(START.0, START.1, 0))],
        doors: Vec::new(),
        containers: Vec::new(),
    });
    world.tick(now);
    let decor = world.registry().query::<Decoration>().next().unwrap().0;
    let serial = world.registry().serial_of(decor).unwrap().raw();
    let _ = packets_for(&mut world, gm);

    world.queue(Command::PickUpItem {
        connection: gm,
        serial,
        amount: 1,
    });
    world.tick(now);

    assert!(
        !world.state.held.contains_key(&gm),
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
    let at = Point::new(START.0 + 1, START.1, 0);
    world.queue(Command::Decorate {
        facet: 0,
        statics: Vec::new(),
        doors: vec![DecorDoor {
            closed: 0x0675,
            open: 0x0676,
            offset_x: -1,
            offset_y: 1,
            position: at,
        }],
        containers: Vec::new(),
    });
    world.tick(now);
    let door = world.registry().query::<Door>().next().unwrap().0;
    let serial = world.registry().serial_of(door).unwrap().raw();

    // Double-click opens it: the graphic becomes the open art and it hops by
    // the hinge offset.
    world.queue(Command::DoubleClick {
        connection: gm,
        serial,
    });
    world.tick(now);
    assert_eq!(
        world.registry().get::<Graphic>(door).unwrap().id,
        0x0676,
        "the door drew open"
    );
    assert_eq!(
        world.registry().get::<Position>(door).unwrap().0,
        Point::new(START.0, START.1 + 1, 0),
        "it swung aside by its hinge offset"
    );
    assert!(world.registry().get::<Door>(door).unwrap().is_open);

    // Double-clicking again shuts it and returns it to its frame.
    world.queue(Command::DoubleClick {
        connection: gm,
        serial,
    });
    world.tick(now);
    assert_eq!(world.registry().get::<Graphic>(door).unwrap().id, 0x0675);
    assert_eq!(world.registry().get::<Position>(door).unwrap().0, at);
    assert!(!world.registry().get::<Door>(door).unwrap().is_open);
}

#[test]
fn an_open_door_swings_shut_on_its_own() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let at = Point::new(START.0 + 1, START.1, 0);
    world.queue(Command::Decorate {
        facet: 0,
        statics: Vec::new(),
        doors: vec![DecorDoor {
            closed: 0x0675,
            open: 0x0676,
            offset_x: -1,
            offset_y: 1,
            position: at,
        }],
        containers: Vec::new(),
    });
    world.tick(now);
    let door = world.registry().query::<Door>().next().unwrap().0;
    let serial = world.registry().serial_of(door).unwrap().raw();

    world.queue(Command::DoubleClick {
        connection: gm,
        serial,
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

/// A terrain whose only statics are one west door frame at (100, 100) and one
/// east frame at (102, 100) — a single-door gap for the generator to fill. The
/// gap has a surface (a door fits) unless `walled`, which stands in for a solid
/// wall where nothing fits.
struct FrameTerrain {
    walled: bool,
}
impl Terrain for FrameTerrain {
    fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
        Some(to)
    }
    fn statics_at(&self, x: u16, y: u16, out: &mut Vec<(u16, i8)>) {
        if y == 100 && (x == 100 || x == 102) {
            out.push((0x0007, 0)); // 0x0007 is both a west and an east frame
        }
    }
    fn can_fit(&self, x: u16, y: u16, _z: i32, _height: i32) -> bool {
        !(self.walled && (x, y) == (101, 100))
    }
}

fn generate_britain_doors(world: &mut World, now: Instant) {
    world.queue(Command::GenerateDoors {
        facet: 0,
        x: 100,
        y: 100,
        width: 3,
        height: 1,
    });
    world.tick(now);
}

#[test]
fn doors_are_generated_between_static_frames() {
    let now = Instant::now();
    let mut world = world();
    world.state.facet_state_mut(0).terrain = Some(Box::new(FrameTerrain { walled: false }));

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
    assert_eq!(door.closed, 0x06A5);
    assert_eq!(door.open, 0x06A6);
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
fn no_door_is_generated_into_a_wall() {
    let now = Instant::now();
    let mut world = world();
    world.state.facet_state_mut(0).terrain = Some(Box::new(FrameTerrain { walled: true }));

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
        facet: 0,
        statics: Vec::new(),
        doors: Vec::new(),
        containers: vec![DecorContainer {
            graphic: 0x0E42,
            gump: 0x49,
            hue: 0,
            position: Point::new(START.0 + 1, START.1, 0),
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
    let serial = world.registry().serial_of(chest).unwrap().raw();
    let _ = packets_for(&mut world, gm);

    world.queue(Command::DoubleClick {
        connection: gm,
        serial,
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

    world.queue(admin_response(gm, 20)); // Decorate Britain
    world.tick(now);

    let events: Vec<AdminMenuAction> = world.bus().read(&mut actions).cloned().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, "decorate:britain");
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

    world.queue(admin_response(gm, 10)); // Populate Britain
    world.tick(now);

    let events: Vec<AdminMenuAction> = world.bus().read(&mut actions).cloned().collect();
    assert_eq!(events.len(), 1, "one admin action was emitted");
    assert_eq!(events[0].action, "populate:britain");
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
    use crate::spawner::{CreatureTemplate, SpawnArea, Spawner};
    let now = Instant::now();
    let mut world = world();
    let creature = CreatureTemplate {
        body: 0x0009,
        hue: 0,
        hits: 10,
        notoriety: 3,
        damage: 0,
        resistance: 0,
        swing: 0,
        sight: 0,
        aggression: 2,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
    };
    let area = SpawnArea {
        x: START.0,
        y: START.1,
        width: 3,
        height: 3,
        facet: 0,
    };
    world.queue(Command::RegisterSpawner {
        spawner: Spawner::new(0, area, vec![creature], 3, 0),
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
        body: 0x0190,
        hue: 0,
        hits: 50,
        notoriety: 1,
        damage: 0,
        resistance: 0,
        swing: 0,
        sight: 0,
        aggression: 0,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: Point::new(START.0 + 1, START.1, 0),
        facet: 0,
        name: Some("Mirabel".to_owned()),
        banker: false,
        vendor: true,
        equipment: Vec::new(),
    });
    world.tick(now);
    let vendor = world
        .state
        .registry
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a placed vendor");
    let vendor_serial = world.registry().serial_of(vendor).unwrap().raw();
    world.queue(Command::StockVendor {
        serial: vendor_serial,
        stock: vec![npc::StockLine {
            graphic: 0x0F7A,
            hue: 0,
            amount: 50,
            price: 4,
            name: "black pearl".to_owned(),
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
    let mob = spawn_mobile_at(&mut world, Point::new(START.0, START.1, 0), 50, now);
    let _ = packets_for(&mut world, player);

    world.queue(Command::Speak {
        serial: mob,
        hue: 0,
        text: "grrr".to_owned(),
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
    world.queue(Command::Enter {
        connection: connection(),
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: None,
        position: None,
        facet: 0,
        appearance: None,
        sheet: None,
        access: AccessLevel::Player,
    });

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
        vec![0x1B, 0xBF, 0x20, 0x4F, 0x11, 0x3A, 0x78, 0x55],
        "0x1B first or there is no body; 0x55 last or the client draws early; \
             0x11 status and the 0x78 of the player's own equipment before it, or the \
             client has no stamina and no backpack serial to open; 0x3A fills the \
             skill window"
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
    assert!(
        stamina > 0,
        "stamina is zero; the client will refuse to run"
    );
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
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x11),
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
    assert!(
        world.registry().has::<Client>(entity),
        "and has a connection"
    );
    assert!(world.registry().serial_of(entity).is_some());
}

#[test]
fn a_created_character_enters_with_its_chosen_body() {
    // Character creation carries the body and hue the player picked; the
    // world must spawn that rather than its default human male.
    let mut world = world();
    let connection = connection();
    world.queue(Command::Enter {
        connection,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Nyx".to_owned(),
        serial: None,
        position: None,
        facet: 0,
        appearance: Some(Appearance {
            body: 0x025E,
            hue: 0x0430,
        }),
        sheet: None,
        access: AccessLevel::Player,
    });
    world.tick(Instant::now());

    let entity = world.state.players[&connection];
    let body = world.registry().get::<Body>(entity).copied().unwrap();
    assert_eq!(body.id, 0x025E, "the elf-female body the client chose");
    assert_eq!(body.hue, 0x0430);

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
    assert_eq!(body.hue, DEFAULT_HUE);
}

#[test]
fn a_characters_inventory_survives_a_logout_and_restore() {
    use openshard_entities::SerialKind;

    // A character with something in its backpack logs out; a fresh shard loads
    // the saved items and the same character logs back in to find them.
    let mut home = world();
    let now = Instant::now();
    let conn_a = enter(&mut home, now);
    let entity = home.state.players[&conn_a];
    let char_serial = home.registry().serial_of(entity).unwrap().raw();

    // The backpack it was equipped on entry.
    let (backpack, _) = home
        .registry()
        .query::<Equipped>()
        .find(|(_, worn)| worn.layer == BACKPACK_LAYER)
        .expect("a backpack was equipped");
    let backpack_serial = home.registry().serial_of(backpack).unwrap();

    // A stack of gold inside it.
    let (gold, gold_serial) = home
        .state
        .registry
        .spawn_with_serial(SerialKind::Item)
        .unwrap();
    home.state
        .registry
        .insert(gold, Graphic { id: 0x0EED, hue: 0 });
    home.state.registry.insert(gold, Amount(500));
    home.state.registry.insert(gold, Stackable);
    home.state.registry.insert(
        gold,
        Contained {
            container: backpack_serial,
            x: 40,
            y: 65,
            grid: 0,
        },
    );

    // What persistence would carry: the backpack (worn) and the gold (inside).
    let records = home.inventory_of(entity);
    assert!(
        records
            .iter()
            .any(|r| r.serial == gold_serial.raw() && r.stackable),
        "the gold is saved as stackable"
    );
    assert!(
        records.iter().any(|r| r.serial == backpack_serial.raw()
            && matches!(r.location, ItemLocation::Equipped { .. })),
        "the backpack is saved as worn"
    );
    assert!(
        records.iter().any(|r| r.serial == gold_serial.raw()
            && r.amount == 500
            && matches!(r.location, ItemLocation::Contained { .. })),
        "the gold is saved inside, amount and all"
    );

    // Log out — the character and its items leave the world.
    home.queue(Command::Disconnect { connection: conn_a });
    home.tick(now);

    // A fresh shard: reserve the serials, load the items, play the character.
    let mut shard = world();
    shard.reserve_serial(char_serial);
    shard.restore_items(records);
    let conn_b = connection();
    shard.queue(Command::Enter {
        connection: conn_b,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: Some(char_serial),
        position: Some(Point::new(1500, 1000, 0)),
        facet: 0,
        appearance: None,
        sheet: None,
        access: AccessLevel::Player,
    });
    shard.tick(now);

    // Exactly one backpack (the restored one, not a fresh starter too), with the
    // gold back inside it.
    let backpacks = shard
        .registry()
        .query::<Equipped>()
        .filter(|(_, worn)| worn.mobile.raw() == char_serial && worn.layer == BACKPACK_LAYER)
        .count();
    assert_eq!(
        backpacks, 1,
        "the saved backpack came back, no starter added"
    );
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
        shard.registry().get::<Contained>(gold).unwrap().container,
        backpack_serial,
        "and back inside the same backpack"
    );
}

#[test]
fn a_relogin_in_the_same_run_keeps_the_inventory() {
    use openshard_entities::SerialKind;

    // The bug the user hit: logging out and back in *without a restart* lost the
    // backpack, because the pending-inventory cache was only filled at boot.
    let mut world = world();
    let now = Instant::now();
    let conn = enter(&mut world, now);
    let entity = world.state.players[&conn];
    let char_serial = world.registry().serial_of(entity).unwrap().raw();
    let (backpack, _) = world
        .registry()
        .query::<Equipped>()
        .find(|(_, w)| w.layer == BACKPACK_LAYER)
        .unwrap();
    let backpack_serial = world.registry().serial_of(backpack).unwrap();
    let (gold, gold_serial) = world
        .state
        .registry
        .spawn_with_serial(SerialKind::Item)
        .unwrap();
    world
        .state
        .registry
        .insert(gold, Graphic { id: 0x0EED, hue: 0 });
    world.state.registry.insert(gold, Amount(300));
    world.state.registry.insert(
        gold,
        Contained {
            container: backpack_serial,
            x: 0,
            y: 0,
            grid: 0,
        },
    );

    // Log out and, in the same world, log the same character back in.
    world.queue(Command::Disconnect { connection: conn });
    world.tick(now);
    let conn = connection();
    world.queue(Command::Enter {
        connection: conn,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: Some(char_serial),
        position: Some(Point::new(1500, 1000, 0)),
        facet: 0,
        appearance: None,
        sheet: None,
        access: AccessLevel::Player,
    });
    world.tick(now);

    let gold = world
        .registry()
        .entity_of(gold_serial)
        .expect("the gold came back on relog");
    assert_eq!(world.registry().get::<Amount>(gold).unwrap().0, 300);
}

#[test]
fn a_spawner_respawn_timer_survives_a_restart() {
    use crate::spawner::{SpawnArea, Spawner};

    // The user's case: a rare spawn on a long timer, killed with time still to
    // wait, must come back with that wait ahead of it — not pop again the moment
    // the shard restarts.
    let mut home = world();
    let area = SpawnArea {
        x: START.0,
        y: START.1,
        width: 1,
        height: 1,
        facet: 0,
    };
    // A 100-second respawn region.
    home.register_spawner(Spawner::new(0, area, vec![], 1, 100 * TICKS_PER_SECOND));
    // Pretend it spawned a while ago and has 60 seconds left to wait.
    home.state.ticks = 5_000;
    home.spawners[0].next_spawn = home.state.ticks + 60 * TICKS_PER_SECOND;

    // What the save carries.
    let records = home.spawner_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].remaining_secs, 60, "sixty seconds still to wait");
    assert_eq!(records[0].respawn_secs, 100);
    assert!(records[0].id > 0, "it was given a real id on registration");

    // Restart: a fresh world, tick counter back at zero, restores the region.
    let mut shard = world();
    shard.restore_spawners(records);
    assert_eq!(shard.spawners.len(), 1);
    assert_eq!(
        shard.spawners[0].next_spawn,
        60 * TICKS_PER_SECOND,
        "the sixty seconds are still ahead of it, not reset to zero"
    );
    assert_eq!(shard.spawners[0].respawn_delay, 100 * TICKS_PER_SECOND);
}

#[test]
fn re_registering_a_region_keeps_the_first_and_its_timer() {
    use crate::spawner::{SpawnArea, Spawner};

    let mut world = world();
    let area = SpawnArea {
        x: 100,
        y: 100,
        width: 5,
        height: 5,
        facet: 0,
    };
    world.register_spawner(Spawner::new(0, area, vec![], 3, 40));
    // Give the standing region a timer with time still to wait, as a restore from
    // the database would.
    world.spawners[0].next_spawn = 5_000;
    // A second registration over the same box — a boot re-populate, or a second
    // staff click — must not stack a spawner nor reset the waiting one.
    world.register_spawner(Spawner::new(0, area, vec![], 3, 40));
    assert_eq!(
        world.spawners.len(),
        1,
        "the same region registered twice is one spawner, not two"
    );
    assert_eq!(
        world.spawners[0].next_spawn, 5_000,
        "and the restored timer is left alone, not reset by the re-populate"
    );
}

#[test]
fn a_vendor_and_its_priced_stock_survive_a_restart() {
    use openshard_state::components::{Price, Vendor};

    // The whole-world save: a staff Populate seeds the vendor once, and from
    // then on the *save* is the truth — a restart brings the shopkeeper back
    // with its crate, wares, prices and labels, with no re-populate anywhere.
    let now = Instant::now();
    let mut home = world();
    let _gm = enter_gm(&mut home, now);
    home.queue(Command::SpawnMobile {
        body: 0x0190,
        hue: 0,
        hits: 50,
        notoriety: 7,
        damage: 0,
        resistance: 0,
        swing: 0,
        sight: 0,
        aggression: 0,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: Point::new(START.0 + 1, START.1, 0),
        facet: 0,
        name: Some("Mirabel".to_owned()),
        banker: false,
        vendor: true,
        equipment: Vec::new(),
    });
    home.tick(now);
    let vendor = home
        .state
        .registry
        .query::<Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a shopkeeper");
    let vendor_serial = home.registry().serial_of(vendor).unwrap().raw();
    home.queue(Command::StockVendor {
        serial: vendor_serial,
        stock: vec![npc::StockLine {
            graphic: 0x0F7A,
            hue: 0,
            amount: 50,
            price: 4,
            name: "black pearl".to_owned(),
        }],
    });
    home.tick(now);

    home.take_snapshot();
    let snapshot = home.drain_saves().next_back().expect("a snapshot");
    let mobiles = snapshot.mobiles.clone().expect("a mobile sweep");
    assert!(
        mobiles
            .iter()
            .any(|m| m.serial == vendor_serial && m.vendor),
        "the vendor is in the mobile sweep, marked as one"
    );
    // What the store would hand back at boot: every saved item, inventories
    // and ground alike.
    let mut items: Vec<ItemRecord> = snapshot
        .inventories
        .iter()
        .flat_map(|inventory| inventory.items.clone())
        .collect();
    items.extend(snapshot.ground.clone().unwrap_or_default());

    // The restart: a fresh world restored from the records alone.
    let mut shard = world();
    shard.restore_items(items);
    shard.restore_mobiles(mobiles);

    let vendor = shard
        .registry()
        .entity_of(Serial::new(vendor_serial).unwrap())
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
        "black pearl",
        "under its label"
    );
    assert_eq!(
        shard.registry().get::<Amount>(stock_item).unwrap().0,
        50,
        "at its full amount"
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
    assert_eq!(worn.mobile.raw(), vendor_serial);
    assert_eq!(worn.layer, npc::STOCK_LAYER);
}

#[test]
fn a_wounded_spawner_creature_survives_a_restart_and_is_counted() {
    use crate::spawner::{CreatureTemplate, SpawnArea, Spawner};

    // ServUO's model exactly: a live creature is saved as it stands — wounded
    // stays wounded — and its region re-counts it on load, so a restart neither
    // heals it, loses it, nor spawns a double over it.
    let now = Instant::now();
    let mut home = world();
    let creature = CreatureTemplate {
        body: 0x0009,
        hue: 0,
        hits: 10,
        notoriety: 3,
        damage: 0,
        resistance: 0,
        swing: 0,
        sight: 0,
        aggression: 2,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
    };
    let area = SpawnArea {
        x: START.0,
        y: START.1,
        width: 2,
        height: 2,
        facet: 0,
    };
    home.queue(Command::RegisterSpawner {
        spawner: Spawner::new(0, area, vec![creature], 1, 1000),
    });
    for _ in 0..3 {
        home.tick(now);
    }
    let (spawned, _) = home
        .state
        .registry
        .query::<SpawnedBy>()
        .next()
        .expect("the region filled");
    let spawned_serial = home.registry().serial_of(spawned).unwrap().raw();
    // Wound it, as a fight would.
    home.state.registry.insert(
        spawned,
        Hitpoints {
            current: 3,
            max: 10,
        },
    );

    home.take_snapshot();
    let snapshot = home.drain_saves().next_back().expect("a snapshot");
    let mobiles = snapshot.mobiles.clone().expect("a mobile sweep");
    let spawners = snapshot.spawners.clone().expect("a spawner sweep");

    let mut shard = world();
    shard.restore_spawners(spawners);
    shard.restore_mobiles(mobiles);

    let creature = shard
        .registry()
        .entity_of(Serial::new(spawned_serial).unwrap())
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
    use crate::tick::command::DecorDoor;
    use openshard_state::components::Decoration;

    // Decoration is saved like everything else — and a door left open stays
    // open across the restart, its doorway unblocked until it swings shut.
    let now = Instant::now();
    let mut home = world();
    let _gm = enter_gm(&mut home, now);
    let shut_at = Point::new(START.0 + 2, START.1, 0);
    let open_at = Point::new(START.0 + 4, START.1, 0);
    home.queue(Command::Decorate {
        facet: 0,
        statics: vec![(0x07C1, 0, Point::new(START.0 + 6, START.1, 0))],
        doors: vec![
            DecorDoor {
                closed: 0x0675,
                open: 0x0676,
                offset_x: -1,
                offset_y: 1,
                position: shut_at,
            },
            DecorDoor {
                closed: 0x0675,
                open: 0x0676,
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
    let decorations = snapshot.decorations.clone().expect("a decoration sweep");
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
    assert_eq!(restored_open.1.open, 0x0676);
    // The shut door seals its doorway; the open one blocks nobody.
    assert!(
        shard
            .state
            .facet_state(0)
            .obstructions
            .blocker_at(shut_at.x, shut_at.y)
            .is_some(),
        "the shut door blocks its tile again"
    );
    let open_pos = shard.registry().get::<Position>(restored_open.0).unwrap().0;
    assert!(
        shard
            .state
            .facet_state(0)
            .obstructions
            .blocker_at(open_pos.x, open_pos.y)
            .is_none(),
        "the open door does not"
    );
}

#[test]
fn a_snapshot_saves_an_idle_online_character_and_the_ground() {
    use openshard_entities::SerialKind;

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
        .find(|(_, w)| w.layer == BACKPACK_LAYER)
        .unwrap();
    let backpack_serial = world.registry().serial_of(backpack).unwrap();
    // A backpack item and a loose ground item.
    let (bagged, _) = world
        .state
        .registry
        .spawn_with_serial(SerialKind::Item)
        .unwrap();
    world
        .state
        .registry
        .insert(bagged, Graphic { id: 0x0EED, hue: 0 });
    world.state.registry.insert(
        bagged,
        Contained {
            container: backpack_serial,
            x: 0,
            y: 0,
            grid: 0,
        },
    );
    items::spawn_item(
        &mut world.state,
        0x1BFB,
        0,
        1,
        false,
        Point::new(1365, 1600, 0),
        0,
    );

    // Tick once to settle, draining any snapshots the enter produced, then force
    // a fresh snapshot with no movement in between.
    world.tick(now);
    let _ = world.drain_saves().count();
    world.take_snapshot();

    let snapshot = world.drain_saves().next().expect("a snapshot was taken");
    let owner = world.registry().serial_of(entity).unwrap().raw();
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
        body: 0x0190,
        hue: 0,
        hits: 100,
        notoriety: 7, // invulnerable
        damage: 0,
        resistance: 0,
        swing: 0,
        sight: 0,
        aggression: 2,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: at,
        facet: 0,
        name: Some("the banker".to_owned()),
        banker: true,
        vendor: false,
        equipment: Vec::new(),
    });
    world.tick(now);
}

fn say(world: &mut World, connection: ConnectionId, text: &str, now: Instant) {
    world.queue(Command::Say {
        connection,
        mode: 0,
        hue: 0,
        font: 3,
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
            worn.mobile == owner
                && worn.layer == npc::BANK_LAYER
                && world.registry().has::<Container>(item)
        }),
        "a character wears a bank box on the bank layer"
    );
}

#[test]
fn saying_bank_near_a_banker_opens_the_bank_box() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    spawn_banker(&mut world, Point::new(START.0 + 1, START.1, 0), now);
    let _ = packets_for(&mut world, connection);

    say(&mut world, connection, "bank", now);
    assert!(
        packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x24),
        "the bank box gump opened"
    );
}

#[test]
fn a_banker_greets_a_nearby_player() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    // The banker two tiles off — inside the greet range. Its spawn tick also
    // runs the townsfolk beat, so it greets straight away. The line is one of
    // several, but every one names the visitor.
    spawn_banker(&mut world, Point::new(START.0 + 2, START.1, 0), now);
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

#[test]
fn single_clicking_a_named_mobile_draws_its_name() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    spawn_banker(&mut world, Point::new(START.0 + 1, START.1, 0), now);
    let banker = world
        .registry()
        .query::<Banker>()
        .next()
        .map(|(e, _)| e)
        .unwrap();
    let banker_serial = world.registry().serial_of(banker).unwrap().raw();
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

/// A terrain that knows one item's tiledata name — enough to test that a
/// single-click on an item reads the name off the map's tiledata.
struct NamedTerrain {
    graphic: u16,
    name: String,
}
impl Terrain for NamedTerrain {
    fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
        Some(to)
    }
    fn item_name(&self, graphic: u16) -> Option<&str> {
        (graphic == self.graphic).then_some(self.name.as_str())
    }
}

#[test]
fn single_clicking_an_item_draws_its_tiledata_name() {
    let now = Instant::now();
    let mut world = world();
    world.state.facet_state_mut(0).terrain = Some(Box::new(NamedTerrain {
        graphic: GOLD,
        name: "gold coins".to_owned(),
    }));
    let connection = enter(&mut world, now);
    // A stack of three on the player's tile, so it is drawn and clickable.
    let serial = spawn_gold(&mut world, Point::new(START.0, START.1, 0), 3, now);
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
    let serial = spawn_gold(&mut world, Point::new(START.0, START.1, 0), 3, now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::QueryProperties {
        connection,
        serials: vec![serial],
    });
    world.tick(now);

    let opl = packets_for(&mut world, connection)
        .into_iter()
        .find(|p| p[0] == 0xD6)
        .expect("a property list was sent");
    // The first entry's cliloc sits at offset 15; a stack uses 1050039
    // (~1_NUMBER~ ~2_ITEMNAME~), not the bare tiledata cliloc.
    let cliloc = u32::from_be_bytes([opl[15], opl[16], opl[17], opl[18]]);
    assert_eq!(
        cliloc, 1_050_039,
        "a stack labels through the amount cliloc"
    );
}

#[test]
fn a_drawn_object_carries_a_tooltip_revision() {
    // A modern client (TOL) with the default send-version tooltips gets a 0xDC
    // revision alongside the 0x78 that draws a mobile.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);
    spawn_banker(&mut world, Point::new(START.0 + 1, START.1, 0), now);

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
    spawn_banker(&mut world, Point::new(START.0 + 1, START.1, 0), now);

    let packets = packets_for(&mut world, connection);
    assert!(
        !packets.iter().any(|p| p[0] == 0xDC),
        "no tooltips means no revision packet"
    );
}

#[test]
fn an_unnamed_creature_takes_its_body_default_name() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    // A chicken (body 0xD0) with no name given.
    world.queue(Command::SpawnMobile {
        body: 0x00D0,
        hue: 0,
        hits: 10,
        notoriety: 1,
        damage: 0,
        resistance: 0,
        swing: 0,
        sight: 0,
        aggression: 0,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: Point::new(START.0 + 1, START.1, 0),
        facet: 0,
        name: None,
        banker: false,
        vendor: false,
        equipment: Vec::new(),
    });
    world.tick(now);
    let chicken = world
        .state
        .registry
        .query::<Body>()
        .filter(|(e, _)| !world.state.registry.has::<Client>(*e))
        .filter_map(|(e, _)| world.state.registry.serial_of(e).map(|s| s.raw()))
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
fn a_drawn_mobile_carries_its_health_bar() {
    // The bar is populated on sight, so it reads full before any fight — not the
    // empty frame you get when health is only sent on a blow.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);
    // A placid creature (sight 0) so nothing but the draw sends a 0xA1.
    spawn_creature(
        &mut world,
        Point::new(START.0 + 1, START.1, 0),
        0,
        false,
        now,
    );

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
    let container = spawn_container_at(&mut world, Point::new(START.0, START.1, 0), now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::ContextMenuRequest {
        connection,
        serial: container,
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
    let container = spawn_container_at(&mut world, Point::new(START.0, START.1, 0), now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::ContextMenuSelect {
        connection,
        serial: container,
        index: 0,
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
    let container = spawn_container_at(&mut world, Point::new(START.0, START.1, 0), now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::ContextMenuRequest {
        connection,
        serial: container,
    });
    world.tick(now);

    assert!(
        !packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0xBF),
        "context menus off means no popup"
    );
}

#[test]
fn saying_bank_with_no_banker_near_does_nothing() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    // A banker, but far out of the 12-tile reach.
    spawn_banker(&mut world, Point::new(START.0 + 40, START.1, 0), now);
    let _ = packets_for(&mut world, connection);

    say(&mut world, connection, "bank", now);
    assert!(
        !packets_for(&mut world, connection)
            .iter()
            .any(|p| p[0] == 0x24),
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
    world.reserve_serial(0x0000_0202);
    world.queue(Command::Enter {
        connection,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: Some(0x0000_0202),
        position: Some(Point::new(1500, 1000, -5)),
        facet: 0,
        appearance: Some(Appearance {
            body: 0x0191,
            hue: 0x83EA,
        }),
        sheet: None,
        access: AccessLevel::Player,
    });
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
fn add_empty_facet(world: &mut World, facet: u8) {
    world.state.facets.insert(
        facet,
        FacetState {
            terrain: None,
            sectors: Sectors::new(FACET_WITHOUT_A_MAP.0, FACET_WITHOUT_A_MAP.1),
            obstructions: Obstructions::default(),
        },
    );
}

fn enter_on_facet(world: &mut World, connection: ConnectionId, facet: u8, now: Instant) {
    world.queue(Command::Enter {
        connection,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "P".to_owned(),
        serial: None,
        position: None,
        facet,
        appearance: None,
        sheet: None,
        access: AccessLevel::Player,
    });
    world.tick(now);
}

#[test]
fn two_facets_do_not_see_each_other() {
    // The whole point of a per-facet interest grid: two mobiles standing on
    // the very same coordinates, one on Felucca and one on Trammel, share no
    // screen. If this ever fails, someone reached for a single global grid.
    let mut world = world();
    add_empty_facet(&mut world, 1);
    let now = Instant::now();
    let here = ConnectionId::from_raw(1);
    let there = ConnectionId::from_raw(2);
    enter_on_facet(&mut world, here, 0, now);
    enter_on_facet(&mut world, there, 1, now);

    let a = world.state.players[&here];
    let b = world.state.players[&there];
    assert!(
        !world.state.seen[&a].contains(&b),
        "a mobile on facet 0 must not have drawn one on facet 1"
    );
    assert!(
        !world.state.seen[&b].contains(&a),
        "nor the other way round"
    );
}

#[test]
fn one_facet_at_the_same_spot_does_see() {
    // The control: the isolation above is facet-specific, not a bug that
    // hides everyone. Same coordinates, same facet — they see each other.
    let mut world = world();
    let now = Instant::now();
    let here = ConnectionId::from_raw(1);
    let there = ConnectionId::from_raw(2);
    enter_on_facet(&mut world, here, 0, now);
    enter_on_facet(&mut world, there, 0, now);

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
    let mut world = world();
    let now = Instant::now();
    enter(&mut world, now);
    enter(&mut world, now);
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
    assert_eq!(position, Point::new(START.0, START.1 + 1, Z_WITHOUT_A_MAP));
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
    assert_eq!(sent, vec![vec![0x22, 0, NOTORIETY_INNOCENT]]);

    let moved: Vec<_> = world.bus().read(&mut moves).copied().collect();
    assert_eq!(moved.len(), 1);
    assert_eq!(moved[0].from, Point::new(START.0, START.1, Z_WITHOUT_A_MAP));
    assert_eq!(
        moved[0].to,
        Point::new(START.0, START.1 + 1, Z_WITHOUT_A_MAP)
    );
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

    let rejects = world
        .drain_outbound()
        .filter(|out| out.packet[0] == 0x21)
        .count();
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
        let refused = world
            .drain_outbound()
            .filter(|out| out.packet[0] == 0x21)
            .count();
        assert_eq!(refused, 0, "step {step} refused");
        sequence = if sequence == u8::MAX { 1 } else { sequence + 1 };
    }
}

#[test]
fn a_walk_from_a_connection_with_no_character_is_ignored() {
    let mut world = world();
    world.queue(Command::Walk {
        connection: connection(),
        request: walk(0, Direction::South),
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
fn a_departing_character_carries_where_it_walked_to() {
    // The re-login rewind bug: the world must hand the server the character's
    // *current* position on logout, so the server's cache tracks the move and
    // a re-login this run spawns it where it left — not where it logged in.
    let mut world = world();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let start = world.registry().get::<Position>(entity).unwrap().0;
    let walked_to = Point::new(start.x + 9, start.y + 4, start.z);
    teleport(&mut world, connection, walked_to);

    world.queue(Command::Disconnect { connection });
    world.tick(now);

    let departed: Vec<_> = world.drain_departed().collect();
    assert_eq!(departed.len(), 1, "one character left");
    assert_eq!(
        (departed[0].x, departed[0].y),
        (walked_to.x, walked_to.y),
        "the logout record carries the moved position, not the login one"
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

    world.queue(Command::Enter {
        connection: connection(),
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "a".to_owned(),
        serial: None,
        position: None,
        facet: 0,
        appearance: None,
        sheet: None,
        access: AccessLevel::Player,
    });
    assert_eq!(world.player_count(), 0);
    world.tick(now);
    assert_eq!(world.ticks(), before + 1);
    assert_eq!(world.player_count(), 1);
}

#[test]
fn an_empty_tick_is_cheap_and_harmless() {
    let mut world = world();
    let now = Instant::now();
    for _ in 0..1000 {
        world.tick(now);
    }
    assert_eq!(world.ticks(), 1000);
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
    // 20Hz is ours to change. The client neither knows nor cares; it only
    // sees acks. Worth stating because the 200ms walk interval *is* the
    // client's, and the two are easy to confuse.
    assert_eq!(TICK_INTERVAL.as_millis(), 50);
    assert!(
        TICK_INTERVAL < WALK_INTERVAL,
        "a step must not span two ticks"
    );
}

/// Decorate one door and return its entity and wire serial. The door sits at
/// `at` closed, and its open leaf swings a tile aside like the metal doors do.
fn place_door(world: &mut World, at: Point, now: Instant) -> (EntityId, u32) {
    world.queue(Command::Decorate {
        facet: 0,
        statics: Vec::new(),
        doors: vec![DecorDoor {
            closed: 0x0675,
            open: 0x0676,
            offset_x: -1,
            offset_y: 1,
            position: at,
        }],
        containers: Vec::new(),
    });
    world.tick(now);
    let door = world.registry().query::<Door>().next().unwrap().0;
    let serial = world.registry().serial_of(door).unwrap().raw();
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
    place_door(&mut world, Point::new(START.0, START.1 + 1, 0), now);

    let mut refused: Cursor<StepRefused> = world.bus().cursor();
    world.queue(Command::Walk {
        connection: gm,
        request: walk(0, Direction::South),
    });
    world.tick(now);
    assert_eq!(
        world.bus().read(&mut refused).count(),
        1,
        "the walk into the shut door is refused"
    );
    assert_eq!(
        world.registry().get::<Position>(entity).unwrap().0,
        Point::new(START.0, START.1, Z_WITHOUT_A_MAP),
        "and nobody moved"
    );
}

#[test]
fn an_opened_door_lets_a_step_through_and_blocks_again_when_it_shuts() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let entity = world.state.players[&gm];
    let serial = world.registry().serial_of(entity).unwrap().raw();
    let at = Point::new(START.0 + 1, START.1, 0);
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
        Point::new(START.0, START.1, Z_WITHOUT_A_MAP)
    );

    // Open, the doorway is a doorway again.
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: door_serial,
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
    teleport(&mut world, gm, Point::new(START.0, START.1, 0));
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
    let (_door, door_serial) = place_door(&mut world, Point::new(START.0, START.1 + 1, 0), now);
    world.queue(Command::SpawnMobile {
        body: 0x0190,
        hue: 0,
        hits: 50,
        notoriety: 5,
        damage: 5,
        resistance: 0,
        swing: 0,
        sight: 5,
        aggression: 2,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: Point::new(START.0, START.1 + 2, 0),
        facet: 0,
        name: None,
        banker: false,
        vendor: false,
        equipment: Vec::new(),
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
            .and_then(|c| c.target)
            .is_none(),
        "a shut door hides prey — no aggro through it"
    );

    // Open the door and the next beat notices.
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: door_serial,
    });
    world.tick(now);
    for _ in 0..(AI_THINK_TICKS + 1) {
        world.tick(now);
    }
    assert_eq!(
        world
            .registry()
            .get::<Combat>(creature)
            .and_then(|c| c.target),
        Some(player_serial),
        "an open doorway is a sight line"
    );
}

/// Spawn a creature with a brain, returning its entity. `body` decides whether
/// it knows door handles (0x0190 human does; 0x00D1 goat does not).
fn spawn_brained(world: &mut World, body: u16, at: Point, sight: u8, now: Instant) -> EntityId {
    world.queue(Command::SpawnMobile {
        body,
        hue: 0,
        hits: 50,
        notoriety: 5,
        damage: 5,
        resistance: 0,
        swing: 0,
        sight,
        aggression: 2,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: at,
        facet: 0,
        name: None,
        banker: false,
        vendor: false,
        equipment: Vec::new(),
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
            world.state.facet_state_mut(0).obstructions.block(
                (i32::from(center.x) + dx) as u16,
                (i32::from(center.y) + dy) as u16,
                crate_entity,
                false,
                0,
                openshard_state::DOOR_HEIGHT,
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
    fence_around(&mut world, Point::new(START.0, START.1, 0));
    let creature = spawn_brained(
        &mut world,
        0x00D1,
        Point::new(START.0, START.1 + 4, 0),
        8,
        now,
    );

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
            .and_then(|c| c.target)
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
    let player_at = Point::new(START.0, START.1, 0);
    // A five-tile wall between quarry and creature, open at both ends.
    for dx in -2i32..=2 {
        let crate_entity = world.state.registry.spawn();
        world.state.facet_state_mut(0).obstructions.block(
            (i32::from(player_at.x) + dx) as u16,
            player_at.y + 2,
            crate_entity,
            false,
            0,
            openshard_state::DOOR_HEIGHT,
        );
    }
    let creature = spawn_brained(
        &mut world,
        0x00D1,
        Point::new(START.0, START.1 + 4, 0),
        10,
        now,
    );

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

#[test]
fn a_human_chaser_opens_the_door_in_its_way() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let door_at = Point::new(START.0, START.1 + 1, 0);
    let (door, door_serial) = place_door(&mut world, door_at, now);

    // Open the door first so the creature can see and acquire its prey.
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: door_serial,
    });
    world.tick(now);
    let creature = spawn_brained(
        &mut world,
        0x0190,
        Point::new(START.0, START.1 + 3, 0),
        8,
        now,
    );
    for _ in 0..(AI_THINK_TICKS * 2) {
        world.tick(now);
    }
    assert!(
        world
            .registry()
            .get::<Combat>(creature)
            .and_then(|c| c.target)
            .is_some(),
        "through the open doorway it noticed the player"
    );

    // Slam the door in its face: a human body opens it rather than giving up.
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: door_serial,
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
        distance(creature_at, Point::new(START.0, START.1, 0)) <= openshard_combat::MELEE_RANGE,
        "and came through the doorway (ended at {creature_at:?})"
    );
}

/// Spawn a creature with an explicit aggression posture, returning its entity.
fn spawn_postured(
    world: &mut World,
    at: Point,
    sight: u8,
    aggression: u8,
    now: Instant,
) -> EntityId {
    world.queue(Command::SpawnMobile {
        body: 0x00D1,
        hue: 0,
        hits: 50,
        notoriety: 1,
        damage: 5,
        resistance: 0,
        swing: 0,
        sight,
        aggression,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: at,
        facet: 0,
        name: None,
        banker: false,
        vendor: false,
        equipment: Vec::new(),
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
    let player_serial = world
        .registry()
        .serial_of(world.state.players[&gm])
        .unwrap();
    // Defensive and blind: it hunts nothing, so only the blow can start this.
    let creature = spawn_postured(&mut world, Point::new(START.0, START.1 + 2, 0), 0, 1, now);
    assert!(
        world.registry().get::<Combat>(creature).is_none(),
        "unprovoked, it minds its own business"
    );
    let creature_serial = world.registry().serial_of(creature).unwrap().raw();
    world.queue(Command::Damage {
        serial: creature_serial,
        amount: 5,
        damage_type: 0,
        by: player_serial.raw(),
    });
    world.tick(now);
    world.tick(now);
    let combat = world.registry().get::<Combat>(creature).expect("engaged");
    assert_eq!(
        combat.target,
        Some(player_serial),
        "it turned on its attacker"
    );
    assert!(combat.warmode, "and it means it");
}

#[test]
fn a_passive_creature_runs_from_its_attacker() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player_at = Point::new(START.0, START.1, 0);
    let player_serial = world
        .registry()
        .serial_of(world.state.players[&gm])
        .unwrap();
    let start_at = Point::new(START.0, START.1 + 1, 0);
    let creature = spawn_postured(&mut world, start_at, 0, 0, now);
    let creature_serial = world.registry().serial_of(creature).unwrap().raw();
    world.queue(Command::Damage {
        serial: creature_serial,
        amount: 5,
        damage_type: 0,
        by: player_serial.raw(),
    });
    world.tick(now);
    let combat = world.registry().get::<Combat>(creature).expect("aware");
    assert!(!combat.warmode, "fauna does not fight back");
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
    let player_at = Point::new(START.0, START.1, 0);
    let player_serial = world
        .registry()
        .serial_of(world.state.players[&gm])
        .unwrap();
    let start_at = Point::new(START.0, START.1 + 1, 0);
    let creature = spawn_postured(&mut world, start_at, 8, 2, now);
    let creature_serial = world.registry().serial_of(creature).unwrap().raw();
    // Cut it to under a fifth of its hits: 50 -> 9.
    world.queue(Command::Damage {
        serial: creature_serial,
        amount: 41,
        damage_type: 0,
        by: player_serial.raw(),
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
        let gameplay = Gameplay::new(
            1,
            15000,
            1000,
            20 * 60,
            2 * 60,
            18,
            3,
            31,
            step_ms,
            openshard_state::CastStyle::Stop,
            true,
            openshard_state::TooltipMode::SendVersion,
            true,
        );
        let mut world = World::new(START).with_gameplay(gameplay);
        let _gm = enter_gm(&mut world, now);
        let player_at = Point::new(START.0, START.1, 0);
        spawn_brained(
            &mut world,
            0x00D1,
            Point::new(START.0, START.1 + 7, 0),
            10,
            now,
        );
        let creature = world
            .state
            .registry
            .query::<Brain>()
            .map(|(entity, _)| entity)
            .next()
            .unwrap();
        let mut later = now;
        for _ in 0..40 {
            later += TICK_INTERVAL;
            world.tick(later);
        }
        distance(
            world.registry().get::<Position>(creature).unwrap().0,
            player_at,
        )
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
fn spawn_horse(world: &mut World, at: Point, now: Instant) -> (EntityId, u32) {
    world.queue(Command::SpawnMobile {
        body: 0x00C8,
        hue: 0x0455,
        hits: 30,
        notoriety: 1,
        damage: 3,
        resistance: 0,
        swing: 0,
        sight: 0,
        aggression: 0,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: true,
        position: at,
        facet: 0,
        name: None,
        banker: false,
        vendor: false,
        equipment: Vec::new(),
    });
    world.tick(now);
    let horse = world
        .state
        .registry
        .query::<Body>()
        .find(|(_, body)| body.id == 0x00C8)
        .map(|(entity, _)| entity)
        .expect("a horse");
    let serial = world.registry().serial_of(horse).unwrap().raw();
    (horse, serial)
}

#[test]
fn a_horse_is_mounted_and_dismounted_by_double_click() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let (horse, horse_serial) = spawn_horse(&mut world, Point::new(START.0 + 1, START.1, 0), now);

    world.queue(Command::DoubleClick {
        connection: gm,
        serial: horse_serial,
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
        world.registry().get::<Graphic>(riding.item).unwrap().id,
        0x3E9F,
        "a bay horse draws as the bay mount item"
    );

    // A raw self-double-click (no bit 31 — that would be a paperdoll request)
    // dismounts, war mode or peace; the horse lands beside the rider.
    let saddle_serial = world.registry().serial_of(riding.item).unwrap().raw();
    let _ = packets_for(&mut world, gm); // clear the outbox before the dismount
    let player_serial = world.registry().serial_of(player).unwrap().raw();
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: player_serial,
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
        distance(horse_at, Point::new(START.0, START.1, 0)) <= 1,
        "the horse stands beside its rider"
    );
}

#[test]
fn a_paperdoll_request_leaves_the_rider_mounted() {
    // The relogin bug: ClassicUO opens the paperdoll on login with a 0x06 whose
    // serial carries bit 31 — a paperdoll *request*, not a use. ServUO's `UseReq`
    // routes it straight to the paperdoll; treating it as a raw self-double-click
    // is what used to throw the rider off a breath after logging in mounted.
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);
    let player = world.state.players[&gm];
    let (_horse, horse_serial) = spawn_horse(&mut world, Point::new(START.0 + 1, START.1, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: horse_serial,
    });
    world.tick(now);
    assert!(world.registry().get::<Riding>(player).is_some(), "mounted");
    let _ = packets_for(&mut world, gm);

    let player_serial = world.registry().serial_of(player).unwrap().raw();
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: player_serial | 0x8000_0000,
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
    let (horse, horse_serial) = spawn_horse(&mut world, Point::new(START.0 + 1, START.1, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: horse_serial,
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
    let char_serial = world.registry().serial_of(player).unwrap().raw();
    let (_horse, horse_serial) = spawn_horse(&mut world, Point::new(START.0 + 1, START.1, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: horse_serial,
    });
    world.tick(now);
    let mount_graphic = {
        let riding = world
            .registry()
            .get::<Riding>(player)
            .copied()
            .expect("mounted");
        world.registry().get::<Graphic>(riding.item).unwrap().id
    };

    // The save now carries the saddle, on the mount layer.
    world.take_snapshot();
    let snapshot = world.drain_saves().next_back().expect("a snapshot");
    assert!(
        snapshot.inventories.iter().any(|inventory| {
            inventory.items.iter().any(|item| {
                matches!(
                    item.location,
                    ItemLocation::Equipped { layer, .. } if layer == openshard_items::MOUNT_LAYER
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
    world.queue(Command::Enter {
        connection: gm,
        version: ClientVersion::TOL,
        account: "admin".to_owned(),
        name: "Lord British".to_owned(),
        serial: Some(char_serial),
        position: Some(Point::new(START.0, START.1, 0)),
        facet: 0,
        appearance: None,
        sheet: None,
        access: AccessLevel::GameMaster,
    });
    world.tick(now);
    let player = world.state.players[&gm];
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
        world.registry().get::<Graphic>(riding.item).unwrap().id,
        mount_graphic,
        "and it draws as the same mount it was"
    );

    // And dismounting the REBUILT mount draws it: the save kept only the saddle,
    // so the creature must be reconstituted whole — above all its `Heading`,
    // without which the 0x78 encoder returns nothing and the horse is invisible.
    let mount_serial = world.registry().serial_of(riding.mount).unwrap().raw();
    let _ = packets_for(&mut world, gm);
    let player_serial = world.registry().serial_of(player).unwrap().raw();
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: player_serial,
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
    let (horse, horse_serial) = spawn_horse(&mut world, Point::new(START.0 + 1, START.1, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: horse_serial,
    });
    world.tick(now);
    assert!(world.registry().get::<Riding>(player).is_some(), "mounted");

    // Ride far from the mounting spot.
    let far = Point::new(START.0 + 30, START.1, 0);
    teleport(&mut world, gm, far);

    // Dismount there, with a raw self-double-click.
    let player_serial = world.registry().serial_of(player).unwrap().raw();
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: player_serial,
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

    // A shopkeeper one tile away, stocked with black pearls by "the script".
    world.queue(Command::SpawnMobile {
        body: 0x0190,
        hue: 0,
        hits: 50,
        notoriety: 1,
        damage: 0,
        resistance: 0,
        swing: 0,
        sight: 0,
        aggression: 0,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: Point::new(START.0 + 1, START.1, 0),
        facet: 0,
        name: Some("Mirabel".to_owned()),
        banker: false,
        vendor: true,
        equipment: Vec::new(),
    });
    world.tick(now);
    let vendor = world
        .state
        .registry
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a shopkeeper");
    let vendor_serial = world.registry().serial_of(vendor).unwrap().raw();
    world.queue(Command::StockVendor {
        serial: vendor_serial,
        stock: vec![npc::StockLine {
            graphic: 0x0F7A,
            hue: 0,
            amount: 50,
            price: 4,
            name: "black pearl".to_owned(),
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
    let stock_serial = world.registry().serial_of(stock_item).unwrap().raw();

    // A hundred coins in the pack, and a double-click opens the shop: the buy
    // list rides out with the contents.
    let backpack = Serial::new(backpack_serial(&world, gm)).unwrap();
    openshard_items::give(&mut world.state, backpack, GOLD, 0, 100);
    world.drain_outbound().count();
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: vendor_serial,
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, gm)
            .iter()
            .any(|p| p.first() == Some(&0x74)),
        "the shop opened with a price list"
    );

    // Three pearls at four coins: twelve gold change hands.
    world.queue(Command::Buy {
        connection: gm,
        vendor: vendor_serial,
        purchases: vec![openshard_protocol::Purchase {
            serial: stock_serial,
            amount: 3,
        }],
    });
    world.tick(now);
    assert_eq!(
        openshard_items::count_in_container(&world.state, backpack, GOLD),
        88,
        "twelve gold paid"
    );
    assert_eq!(
        openshard_items::count_in_container(&world.state, backpack, 0x0F7A),
        3,
        "three pearls delivered"
    );

    // Sell two back at half price: four gold returns.
    let pearls = world
        .state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == backpack)
        .find(|(entity, _)| {
            world
                .registry()
                .get::<Graphic>(*entity)
                .is_some_and(|g| g.id == 0x0F7A)
        })
        .map(|(entity, _)| world.registry().serial_of(entity).unwrap().raw())
        .expect("pearls in the pack");
    world.queue(Command::Sell {
        connection: gm,
        vendor: vendor_serial,
        sales: vec![openshard_protocol::Sale {
            serial: pearls,
            amount: 2,
        }],
    });
    world.tick(now);
    assert_eq!(
        openshard_items::count_in_container(&world.state, backpack, GOLD),
        92,
        "two pearls at half price is four gold"
    );
    assert_eq!(
        openshard_items::count_in_container(&world.state, backpack, 0x0F7A),
        1,
        "one pearl kept"
    );

    // A pauper is refused: the vendor keeps its goods when gold runs short.
    world.queue(Command::Buy {
        connection: gm,
        vendor: vendor_serial,
        purchases: vec![openshard_protocol::Purchase {
            serial: stock_serial,
            amount: 47,
        }],
    });
    world.tick(now);
    assert_eq!(
        openshard_items::count_in_container(&world.state, backpack, GOLD),
        92,
        "no gold moved on the refused purchase"
    );
}

#[test]
fn saying_buy_opens_the_shop_and_an_empty_sell_answers_overhead() {
    let now = Instant::now();
    let mut world = world();
    let gm = enter_gm(&mut world, now);

    // A shopkeeper one tile off, its stock crate empty.
    world.queue(Command::SpawnMobile {
        body: 0x0190,
        hue: 0,
        hits: 50,
        notoriety: 1,
        damage: 0,
        resistance: 0,
        swing: 0,
        sight: 0,
        aggression: 0,
        beat: 0,
        ranged: 0,
        ranged_kind: 0,
        wander: false,
        position: Point::new(START.0 + 1, START.1, 0),
        facet: 0,
        name: Some("Mirabel".to_owned()),
        banker: false,
        vendor: true,
        equipment: Vec::new(),
    });
    world.tick(now);
    let vendor = world
        .state
        .registry
        .query::<openshard_state::components::Vendor>()
        .map(|(entity, _)| entity)
        .next()
        .expect("a shopkeeper");
    let vendor_serial = world.registry().serial_of(vendor).unwrap().raw();
    world.drain_outbound().count();

    // "buy" opens the price list, exactly as a double-click would.
    world.queue(Command::Say {
        connection: gm,
        mode: 0,
        hue: 0,
        font: 3,
        text: "buy".to_owned(),
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, gm)
            .iter()
            .any(|p| p.first() == Some(&0x74)),
        "saying 'buy' opened the shop"
    );

    // "sell" with nothing the vendor wants is answered over the vendor's head as
    // ordinary speech (0xAE from the vendor), not a private system line (0x1C).
    world.queue(Command::Say {
        connection: gm,
        mode: 0,
        hue: 0,
        font: 3,
        text: "sell".to_owned(),
    });
    world.tick(now);
    let packets = packets_for(&mut world, gm);
    assert!(
        packets
            .iter()
            .any(|p| p[0] == 0xAE && mentions(p, vendor_serial)),
        "the vendor spoke its refusal over its own head"
    );
    assert!(
        !packets.iter().any(|p| p[0] == 0x1C),
        "and not as a private system message"
    );
}

/// Spawn an archer-shaped creature: ranged reach 8, energy bolts.
fn spawn_archer(world: &mut World, at: Point, now: Instant) -> EntityId {
    spawn_archer_bodied(world, 0x0190, at, now)
}

/// The same archer with a chosen body — a beast body cannot open doors.
fn spawn_archer_bodied(world: &mut World, body: u16, at: Point, now: Instant) -> EntityId {
    world.queue(Command::SpawnMobile {
        body,
        hue: 0,
        hits: 50,
        notoriety: 5,
        damage: 7,
        resistance: 0,
        swing: 10,
        sight: 10,
        aggression: 2,
        beat: 0,
        ranged: 8,
        ranged_kind: 4,
        wander: false,
        position: at,
        facet: 0,
        name: None,
        banker: false,
        vendor: false,
        equipment: Vec::new(),
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
    spawn_archer(&mut world, Point::new(START.0, START.1 + 5, 0), now);

    let mut later = now;
    for _ in 0..40 {
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
    let player_at = Point::new(START.0, START.1, 0);
    let archer = spawn_archer(&mut world, Point::new(START.0, START.1 + 1, 0), now);

    let mut later = now;
    for _ in 0..40 {
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
    let den = Point::new(START.0, START.1 + 3, 0);
    for dx in -1i32..=1 {
        for dy in -1i32..=1 {
            if dx == 0 && dy == 0 || (dx == 0 && dy == -1) {
                continue; // the north gap stays open for the door
            }
            let crate_entity = world.state.registry.spawn();
            world.state.facet_state_mut(0).obstructions.block(
                (i32::from(den.x) + dx) as u16,
                (i32::from(den.y) + dy) as u16,
                crate_entity,
                false,
                0,
                openshard_state::DOOR_HEIGHT,
            );
        }
    }
    let (_door, door_serial) = place_door(&mut world, Point::new(den.x, den.y - 1, 0), now);
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: door_serial,
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
            .and_then(|c| c.target)
            .is_some(),
        "it took aim through the open door"
    );
    world.queue(Command::DoubleClick {
        connection: gm,
        serial: door_serial,
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
