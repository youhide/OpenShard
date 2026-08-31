//! The secure trade window: opening it, filling it, settling it, and every way
//! it can end without anybody losing anything.
//!
//! The tests that matter most here are the *endings*. A trade holds real goods
//! in a container that is deliberately never saved, so every path out of one is
//! a path an item can vanish down, and each has a test named for it.

use openshard_items::{
    TRADE_CONTAINER_GRAPHIC,
    TRADE_LAYER,
};
use openshard_protocol::items::{
    DropDestination,
    DropItem,
};
use openshard_protocol::trade::SECURE_TRADE;
use openshard_state::components::{
    Contained,
    Drawn,
    Equipped,
};

use super::tests::{
    backpack_serial,
    connection,
    enter_as,
    serial_of,
    teleport,
    world,
};
use super::*;

const GOLD: u16 = 0x0EED;

/// The second player, opposite [`connection`].
fn other() -> ConnectionId {
    ConnectionId::from_raw(2)
}

/// Two players standing next to each other, both with a backpack.
fn two_players(world: &mut World, now: Instant) -> (ConnectionId, ConnectionId) {
    let first = enter_as(world, connection(), now);
    let second = enter_as(world, other(), now);
    // Entering puts both on the same tile; a step apart is still within
    // `TRADE_RANGE`, and standing on one another is not what a trade looks like.
    let at = world.state.registry.get::<Position>(world.state.players[&first]);
    let Some(&Position(at)) = at else {
        panic!("a player stands somewhere");
    };
    teleport(world, second, Point::new(at.x + 1, at.y, at.z));
    world.tick(now);
    (first, second)
}

/// Put one item in a connection's backpack and return its serial.
fn give(world: &mut World, connection: ConnectionId, graphic: u16, now: Instant) -> Serial {
    let owner = serial_of(world, connection);
    let pack = backpack_serial(world, connection);
    let before = contained_serials(world, pack);
    world.queue(Command::GiveItem {
        serial:    owner,
        graphic:   openshard_protocol::wire::Graphic(graphic),
        hue:       openshard_protocol::wire::Hue(0),
        amount:    1,
        stackable: false,
    });
    world.tick(now);
    *contained_serials(world, pack)
        .iter()
        .find(|serial| !before.contains(serial))
        .expect("the item landed in the pack")
}

/// The serials directly inside a container.
fn contained_serials(world: &World, container: Serial) -> Vec<Serial> {
    world
        .registry()
        .query::<Contained>()
        .filter(|(_, held)| held.container == container)
        .filter_map(|(entity, _)| world.registry().serial_of(entity))
        .collect()
}

/// Offer `item` to another player: the lift-and-drop-on-them a trade opens on.
fn offer_to(world: &mut World, from: ConnectionId, item: Serial, to: ConnectionId, now: Instant) {
    let target = serial_of(world, to);
    drop_onto(world, from, item, target, now);
}

/// Lift `item` and drop it onto `target`'s serial — the drag a trade opens on.
///
/// The destination is built by running the real `0x08` through
/// [`DropItem::destination`] rather than naming a variant here: `target` is a
/// person in some of these tests and a container in others, and which variant
/// that is is exactly the decision under test. Naming it by hand would let the
/// test agree with itself while the packet said something else.
fn drop_onto(world: &mut World, connection: ConnectionId, item: Serial, target: Serial, now: Instant) {
    world.queue(Command::PickUpItem {
        connection,
        serial: RawSerial(item.raw()),
        amount: 1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection,
        serial: RawSerial(item.raw()),
        destination: DropItem {
            serial:    RawSerial(item.raw()),
            position:  Point::default(),
            container: RawSerial(target.raw()),
        }
        .destination(),
    });
    world.tick(now);
}

/// The escrow container a connection's character is wearing, if any.
fn escrow_of(world: &World, connection: ConnectionId) -> Option<Serial> {
    let owner = world.registry().serial_of(world.state.players[&connection])?;
    world
        .registry()
        .query::<Equipped>()
        .find(|(_, worn)| worn.mobile == owner && worn.layer == TRADE_LAYER)
        .and_then(|(item, _)| world.registry().serial_of(item))
}

/// Everything the last tick sent, to everyone.
///
/// `packets_for` drains the whole outbox and then filters, so it can only be
/// asked about one connection per tick. A trade is two-sided by definition, so
/// these tests drain once and filter twice.
fn outbound(world: &mut World) -> Vec<Outbound> {
    world.drain_outbound().collect()
}

/// Whether a `0x6F` with this action byte was sent to a connection.
fn sent_trade_action(sent: &[Outbound], to: ConnectionId, action: u8) -> bool {
    sent.iter().any(|out| {
        out.connection == to
            && out.packet.first() == Some(&SECURE_TRADE)
            && out.packet.get(3) == Some(&action)
    })
}

#[test]
fn dropping_an_item_on_another_player_opens_a_trade_window_on_both_screens() {
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    let _ = outbound(&mut world);

    offer_to(&mut world, first, sword, second, now);

    assert_eq!(world.state.trades.len(), 1, "a trade opened");
    let sent = outbound(&mut world);
    // Action 0 is `Display`: the packet that draws the window.
    assert!(sent_trade_action(&sent, first, 0), "the offerer's window opened");
    assert!(
        sent_trade_action(&sent, second, 0),
        "and so did the partner's — a window on one screen is not a trade"
    );
    // And the item went into the offerer's half, not onto the floor.
    let escrow = escrow_of(&world, first).expect("the offerer wears an escrow");
    assert_eq!(contained_serials(&world, escrow), vec![sword]);
}

#[test]
fn the_escrow_is_a_container_the_partner_can_see_into() {
    // Both parties watch both halves, or the window shows you nothing about
    // what the other person is actually offering — which is the entire point.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    offer_to(&mut world, first, sword, second, now);
    let escrow = escrow_of(&world, first).unwrap();

    let watchers = world
        .state
        .open_containers
        .get(&escrow)
        .expect("the escrow is watched");
    assert!(watchers.contains(&first) && watchers.contains(&second));
}

#[test]
fn dropping_an_item_on_a_creature_still_bounces() {
    // A creature and a shopkeeper are bodies too. Only a *player* trades, and
    // the old refusal has to survive the new arm above it.
    let now = Instant::now();
    let mut world = world();
    let player = enter_as(&mut world, connection(), now);
    let Some(&Position(at)) = world.state.registry.get::<Position>(world.state.players[&player]) else {
        panic!("a player stands somewhere");
    };
    let creature = super::tests::spawn_mobile_at(&mut world, at, 50, now);
    let sword = give(&mut world, player, 0x0F5E, now);
    let pack = backpack_serial(&world, player);

    drop_onto(&mut world, player, sword, creature, now);

    assert!(world.state.trades.is_empty(), "no trade with a creature");
    assert!(
        contained_serials(&world, pack).contains(&sword),
        "and the item bounced home rather than being lost on a body"
    );
}

#[test]
fn both_checkboxes_swap_the_goods_and_close_the_window() {
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    let shield = give(&mut world, second, 0x1B76, now);
    let first_pack = backpack_serial(&world, first);
    let second_pack = backpack_serial(&world, second);

    offer_to(&mut world, first, sword, second, now);
    offer_to(&mut world, second, shield, first, now);
    let first_escrow = escrow_of(&world, first).unwrap();
    assert_eq!(contained_serials(&world, first_escrow), vec![sword]);

    for (connection, container) in [
        (first, escrow_of(&world, first).unwrap()),
        (second, escrow_of(&world, second).unwrap()),
    ] {
        world.queue(Command::TradeAction {
            connection,
            container: RawSerial(container.raw()),
            accepted: true,
        });
        world.tick(now);
    }

    assert!(world.state.trades.is_empty(), "the trade settled and closed");
    assert!(
        contained_serials(&world, second_pack).contains(&sword),
        "the sword crossed over"
    );
    assert!(
        contained_serials(&world, first_pack).contains(&shield),
        "and the shield came back the other way"
    );
    assert!(
        escrow_of(&world, first).is_none() && escrow_of(&world, second).is_none(),
        "and neither party is still wearing an escrow"
    );
}

#[test]
fn one_checkbox_alone_moves_nothing() {
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    offer_to(&mut world, first, sword, second, now);
    let escrow = escrow_of(&world, first).unwrap();

    world.queue(Command::TradeAction {
        connection: first,
        container:  RawSerial(escrow.raw()),
        accepted:   true,
    });
    world.tick(now);

    assert_eq!(world.state.trades.len(), 1, "the trade is still open");
    assert_eq!(
        contained_serials(&world, escrow),
        vec![sword],
        "and the sword has not moved"
    );
}

#[test]
fn adding_an_item_after_a_checkbox_unticks_both() {
    // ServUO's `ClearChecks`, found by diffing rather than announced from the
    // container's own `OnItemAdded`. Both boxes clear, not only the mover's:
    // agreeing to one pile is not agreeing to another.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    let gold = give(&mut world, first, GOLD, now);
    offer_to(&mut world, first, sword, second, now);
    let escrow = escrow_of(&world, first).unwrap();

    // Only one side agrees, so the trade stays open — and what is on the table
    // at that moment is what the tick then watches.
    world.queue(Command::TradeAction {
        connection: first,
        container:  RawSerial(escrow.raw()),
        accepted:   true,
    });
    world.tick(now);
    assert!(world.state.trades[0].from.accepted);
    let _ = second;

    // Now sweeten the offer.
    offer_to(&mut world, first, gold, second, now);
    world.tick(now);

    let trade = &world.state.trades[0];
    assert!(
        !trade.from.accepted && !trade.to.accepted,
        "changing the goods retracts every agreement to them"
    );
}

#[test]
fn a_cancel_returns_both_sides_offerings() {
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    let shield = give(&mut world, second, 0x1B76, now);
    let first_pack = backpack_serial(&world, first);
    let second_pack = backpack_serial(&world, second);
    offer_to(&mut world, first, sword, second, now);
    offer_to(&mut world, second, shield, first, now);
    let escrow = escrow_of(&world, first).unwrap();
    let _ = outbound(&mut world);

    world.queue(Command::TradeCancel {
        connection: first,
        container:  RawSerial(escrow.raw()),
    });
    world.tick(now);

    assert!(world.state.trades.is_empty());
    assert!(contained_serials(&world, first_pack).contains(&sword));
    assert!(contained_serials(&world, second_pack).contains(&shield));
    // Action 1 is `Close`. The party who did *not* cancel must be told too, or
    // they keep a window over a trade that no longer exists.
    assert!(sent_trade_action(&outbound(&mut world), second, 1));
}

#[test]
fn walking_out_of_range_cancels_the_trade_and_returns_the_goods() {
    // ServUO revalidates from the `Location` setter — a call beside every mover.
    // This engine finds it in the tick instead, so what is tested is that simply
    // *being* apart at the next tick is enough.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    let first_pack = backpack_serial(&world, first);
    offer_to(&mut world, first, sword, second, now);
    assert_eq!(world.state.trades.len(), 1);

    let Some(&Position(at)) = world.state.registry.get::<Position>(world.state.players[&second]) else {
        panic!("a player stands somewhere");
    };
    teleport(&mut world, second, Point::new(at.x + 20, at.y, at.z));
    world.tick(now);

    assert!(world.state.trades.is_empty(), "walking away ends it");
    assert!(
        contained_serials(&world, first_pack).contains(&sword),
        "and the sword is back in its owner's pack, not in a container nobody owns"
    );
}

#[test]
fn logging_out_mid_trade_returns_the_goods_before_the_character_is_saved() {
    // The ordering is the whole test: an escrow is never saved, so if the
    // disconnect took the character's record before the trade was cancelled the
    // item would be in neither the save nor the world.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, second, 0x0F5E, now);
    let second_pack = backpack_serial(&world, second);
    offer_to(&mut world, second, sword, first, now);
    let escrow = escrow_of(&world, second).expect("the offerer wears an escrow");
    assert_eq!(contained_serials(&world, escrow), vec![sword]);

    world.queue(Command::Disconnect { connection: first });
    world.tick(now);

    assert!(world.state.trades.is_empty(), "one party leaving ends it");
    assert!(
        contained_serials(&world, second_pack).contains(&sword),
        "and the player still here has their sword back"
    );
}

#[test]
fn a_trade_escrow_is_not_swept_into_the_save() {
    // The exclusion that fails silently: without it a restored character wears
    // an escrow container belonging to a trade that ended when the shard did,
    // and it can never be opened, closed or taken off.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    offer_to(&mut world, first, sword, second, now);
    let escrow = escrow_of(&world, first).expect("an escrow exists to be excluded");

    world.take_snapshot();
    let snapshot = world.drain_saves().next().expect("a snapshot was taken");

    let saved: Vec<Serial> = snapshot
        .inventories
        .iter()
        .flat_map(|inventory| inventory.items.iter())
        .map(|item| item.serial)
        .collect();
    assert!(!saved.contains(&escrow), "the escrow container is not saved");
    assert!(
        !saved.contains(&sword),
        "nor is what is inside it, which the walk would otherwise recurse into"
    );
}

#[test]
fn cancelling_every_trade_first_is_what_makes_a_shutdown_save_whole() {
    // The shutdown path's one line. With the trade cancelled the goods are back
    // in a pack, and a pack *is* saved — so a clean stop taken mid-trade costs
    // nobody anything.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    offer_to(&mut world, first, sword, second, now);

    world.cancel_all_trades();
    world.take_snapshot();
    let snapshot = world.drain_saves().next().expect("a snapshot was taken");

    let saved: Vec<Serial> = snapshot
        .inventories
        .iter()
        .flat_map(|inventory| inventory.items.iter())
        .map(|item| item.serial)
        .collect();
    assert!(saved.contains(&sword), "the offered sword is saved after all");
}

#[test]
fn the_escrow_container_itself_cannot_be_lifted() {
    // ServUO's `CheckLift` refusing outright. Without this a party drags the
    // window off its own paperdoll and onto the floor.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    offer_to(&mut world, first, sword, second, now);
    let escrow = escrow_of(&world, first).unwrap();

    world.queue(Command::PickUpItem {
        connection: first,
        serial:     RawSerial(escrow.raw()),
        amount:     1,
    });
    world.tick(now);

    assert!(
        super::tests::nothing_is_held(&world),
        "nothing went onto the cursor"
    );
    assert!(escrow_of(&world, first).is_some(), "and the escrow is still worn");
}

#[test]
fn an_onlooker_is_not_shown_the_trade_container_on_a_paperdoll() {
    // The other silent exclusion. An escrow is worn so that reach works; drawing
    // it hangs a mystery box off both traders on every screen in view.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    offer_to(&mut world, first, sword, second, now);
    let escrow = escrow_of(&world, first).unwrap();

    let owner = serial_of(&world, first);
    let worn = world.state.equipment_of(owner);
    assert!(
        !worn.iter().any(|item| item.serial == escrow),
        "the escrow is not in the equipment list a 0x78 draws"
    );
    assert!(
        !worn.iter().any(|item| item.layer == TRADE_LAYER),
        "and neither is anything else on its layer"
    );
}

#[test]
fn a_second_trade_is_refused_while_one_is_open() {
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let third = enter_as(&mut world, ConnectionId::from_raw(3), now);
    let Some(&Position(at)) = world.state.registry.get::<Position>(world.state.players[&first]) else {
        panic!("a player stands somewhere");
    };
    teleport(&mut world, third, Point::new(at.x, at.y + 1, at.z));
    world.tick(now);

    let sword = give(&mut world, first, 0x0F5E, now);
    let gold = give(&mut world, first, GOLD, now);
    offer_to(&mut world, first, sword, second, now);
    assert_eq!(world.state.trades.len(), 1);
    let first_pack = backpack_serial(&world, first);

    offer_to(&mut world, first, gold, third, now);

    assert_eq!(world.state.trades.len(), 1, "still only the one trade");
    assert!(
        contained_serials(&world, first_pack).contains(&gold),
        "and the refused offer bounced home"
    );
}

#[test]
fn a_second_drop_on_the_same_partner_adds_to_the_open_window() {
    // Both references do this rather than opening a second window.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    let gold = give(&mut world, first, GOLD, now);
    let partner = serial_of(&world, second);

    drop_onto(&mut world, first, sword, partner, now);
    drop_onto(&mut world, first, gold, partner, now);

    assert_eq!(world.state.trades.len(), 1);
    let escrow = escrow_of(&world, first).unwrap();
    let offered = contained_serials(&world, escrow);
    assert!(offered.contains(&sword) && offered.contains(&gold));
}

#[test]
fn taking_an_offer_back_out_of_the_window_is_an_ordinary_lift() {
    // No new path: the escrow is a real container, which is why it is one.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    let pack = backpack_serial(&world, first);
    offer_to(&mut world, first, sword, second, now);
    let escrow = escrow_of(&world, first).unwrap();
    assert_eq!(contained_serials(&world, escrow), vec![sword]);

    world.queue(Command::PickUpItem {
        connection: first,
        serial:     RawSerial(sword.raw()),
        amount:     1,
    });
    world.tick(now);
    world.queue(Command::DropItem {
        connection:  first,
        serial:      RawSerial(sword.raw()),
        destination: DropDestination::Item {
            item: pack,
            at:   GumpPoint::new(0, 0),
        },
    });
    world.tick(now);

    assert!(contained_serials(&world, escrow).is_empty());
    assert!(contained_serials(&world, pack).contains(&sword));
}

#[test]
fn a_partner_cannot_take_an_item_from_the_other_half_of_a_trade() {
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    offer_to(&mut world, first, sword, second, now);
    let escrow = escrow_of(&world, first).unwrap();

    world.queue(Command::PickUpItem {
        connection: second,
        serial:     RawSerial(sword.raw()),
        amount:     1,
    });
    world.tick(now);

    assert!(super::tests::nothing_is_held(&world));
    assert_eq!(contained_serials(&world, escrow), vec![sword]);
}

#[test]
fn a_partner_cannot_add_an_item_to_the_other_half_of_a_trade() {
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    let shield = give(&mut world, second, 0x1B76, now);
    let second_pack = backpack_serial(&world, second);
    offer_to(&mut world, first, sword, second, now);
    let first_escrow = escrow_of(&world, first).unwrap();

    drop_onto(&mut world, second, shield, first_escrow, now);

    assert_eq!(contained_serials(&world, first_escrow), vec![sword]);
    assert!(contained_serials(&world, second_pack).contains(&shield));
}

#[test]
fn trade_actions_must_name_the_senders_own_escrow() {
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    offer_to(&mut world, first, sword, second, now);
    let second_escrow = escrow_of(&world, second).unwrap();

    world.queue(Command::TradeAction {
        connection: first,
        container:  RawSerial(second_escrow.raw()),
        accepted:   true,
    });
    world.tick(now);
    assert!(!world.state.trades[0].from.accepted);

    world.queue(Command::TradeCancel {
        connection: first,
        container:  RawSerial(second_escrow.raw()),
    });
    world.tick(now);
    assert_eq!(world.state.trades.len(), 1);
}

#[test]
fn the_escrow_wears_servuos_own_graphic_and_layer() {
    // Pinned because both are the client's business: the graphic is what the
    // window is drawn from, and the layer is the one no `0x13` can reach.
    let now = Instant::now();
    let mut world = world();
    let (first, second) = two_players(&mut world, now);
    let sword = give(&mut world, first, 0x0F5E, now);
    offer_to(&mut world, first, sword, second, now);

    let escrow = escrow_of(&world, first).unwrap();
    let entity = world.registry().entity_of(escrow).unwrap();
    assert_eq!(
        world.registry().get::<Drawn>(entity).unwrap().id,
        TRADE_CONTAINER_GRAPHIC
    );
    assert_eq!(
        world.registry().get::<Equipped>(entity).unwrap().layer,
        TRADE_LAYER
    );
    let _ = second;
}
