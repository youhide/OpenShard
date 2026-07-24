//! The numbers a client is told about itself: the status bar's derived fields,
//! the regeneration that moves them, what carrying too much costs, and the ack
//! that lets a player leave.
//!
//! A child module rather than more of `tests.rs`, which is long past the size a
//! file should be: this slice's tests read private world state, so they stay
//! inside the module, but they do not have to pile into the same file.

use super::tests::{enter, enter_gm, packets_for, world, START};
use super::*;
use openshard_state::components::{Amount, Contained, Equipped, Graphic, Stackable};

/// The gold graphic, and the backpack layer a character wears one on.
const GOLD: u16 = 0x0EED;

/// The serial of the container a connection's character wears on its back.
fn backpack_of(world: &World, connection: ConnectionId) -> Serial {
    worn_container_on(world, connection, BACKPACK_LAYER)
}

/// The serial of the container worn on a given layer — the backpack, or the bank.
fn worn_container_on(world: &World, connection: ConnectionId, layer: u8) -> Serial {
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let (entity, _) = world
        .state
        .registry
        .query::<Equipped>()
        .find(|(entity, worn)| {
            worn.mobile == owner
                && worn.layer == layer
                && world.state.registry.has::<Container>(*entity)
        })
        .expect("a character wears a container there");
    world.state.registry.serial_of(entity).unwrap()
}

/// Put `amount` of `graphic` inside a container, as a stack.
fn put_in(world: &mut World, container: Serial, graphic: u16, amount: u16) -> EntityId {
    let (item, _) = world
        .state
        .registry
        .spawn_with_serial(SerialKind::Item)
        .unwrap();
    world.state.registry.insert(
        item,
        Graphic {
            id: graphic,
            hue: 0,
        },
    );
    world.state.registry.insert(item, Amount(amount));
    world.state.registry.insert(item, Stackable);
    world.state.registry.insert(
        item,
        Contained {
            container,
            x: 40,
            y: 65,
            grid: 0,
        },
    );
    item
}

/// Wear an item on a layer, the way `equip` would.
fn wear(world: &mut World, connection: ConnectionId, graphic: u16, layer: u8) -> EntityId {
    let player = world.state.players[&connection];
    let mobile = world.state.registry.serial_of(player).unwrap();
    let (item, _) = world
        .state
        .registry
        .spawn_with_serial(SerialKind::Item)
        .unwrap();
    world.state.registry.insert(
        item,
        Graphic {
            id: graphic,
            hue: 0,
        },
    );
    world
        .state
        .registry
        .insert(item, Equipped { mobile, layer });
    item
}

#[test]
fn the_status_bar_counts_the_gold_in_the_pack() {
    // The bar used to send a flat zero. Gold anywhere under the character —
    // loose in the backpack or in a pouch inside it — is the player's, and the
    // number they read has to agree with what they can spend.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let pack = backpack_of(&world, connection);

    assert_eq!(items::total_gold(&world.state, player), 0, "starts broke");

    put_in(&mut world, pack, GOLD, 1_000);
    assert_eq!(items::total_gold(&world.state, player), 1_000);

    // A pouch in the pack, with more in it: still the player's gold.
    let (pouch, pouch_serial) = world
        .state
        .registry
        .spawn_with_serial(SerialKind::Item)
        .unwrap();
    world
        .state
        .registry
        .insert(pouch, Graphic { id: 0x0E79, hue: 0 });
    world
        .state
        .registry
        .insert(pouch, Container { gump: 0x003C });
    world.state.registry.insert(
        pouch,
        Contained {
            container: pack,
            x: 10,
            y: 10,
            grid: 1,
        },
    );
    put_in(&mut world, pouch_serial, GOLD, 500);

    assert_eq!(
        items::total_gold(&world.state, player),
        1_500,
        "a nested container counts too"
    );
}

#[test]
fn gold_weighs_a_fiftieth_of_a_stone() {
    // The one item weight that is not the tile's: ServUO's `Gold.DefaultWeight`
    // of 0.02. Without it a bank run would pin a character to the floor — 5,000
    // coins at a stone each is ten times any carry cap in the game.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let pack = backpack_of(&world, connection);

    let empty = items::total_weight(&world.state, player, BODY_WEIGHT);
    assert_eq!(empty, BODY_WEIGHT, "an empty character is its own weight");

    put_in(&mut world, pack, GOLD, 1_000);
    assert_eq!(
        items::total_weight(&world.state, player, BODY_WEIGHT),
        BODY_WEIGHT + 20,
        "a thousand coins is twenty stones"
    );
}

#[test]
fn the_bank_box_is_not_carried() {
    // What is in the bank is *there*, not on you. ServUO marks the box
    // `IsVirtualItem` and `UpdateTotals` skips a virtual item outright — weight
    // and gold both — which is why the banker has to tell you your balance
    // instead of the status bar showing it. Moving a purse from pack to bank has
    // to make the character lighter, or the bank is just a second pocket.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let pack = backpack_of(&world, connection);
    let bank = worn_container_on(&world, connection, items::BANK_LAYER);

    let empty = items::total_weight(&world.state, player, BODY_WEIGHT);
    let purse = put_in(&mut world, pack, GOLD, 1_000);
    assert_eq!(
        items::total_weight(&world.state, player, BODY_WEIGHT),
        empty + 20,
        "in the pack it weighs"
    );

    // The same coins, banked.
    world.state.registry.insert(
        purse,
        Contained {
            container: bank,
            x: 40,
            y: 65,
            grid: 0,
        },
    );
    assert_eq!(
        items::total_weight(&world.state, player, BODY_WEIGHT),
        empty,
        "in the bank it does not"
    );
    assert_eq!(
        items::total_gold(&world.state, player),
        0,
        "and the status bar counts only what is on you"
    );
}

#[test]
fn a_lifted_pile_is_still_carried() {
    // A held item is in limbo — off the grid, off every screen but the picker's —
    // but it is still in the character's hand. ServUO's `UpdateTotals` adds
    // `m_Holding` explicitly; without it, lifting the anvil is how you carry it
    // home.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let pack = backpack_of(&world, connection);

    let purse = put_in(&mut world, pack, GOLD, 1_000);
    let packed = items::total_weight(&world.state, player, BODY_WEIGHT);

    // Onto the cursor: out of the container, into the drag.
    world.state.registry.remove::<Contained>(purse);
    world.state.held.insert(
        connection,
        openshard_state::HeldItem {
            entity: purse,
            origin: openshard_state::Origin::Container(Contained {
                container: pack,
                x: 40,
                y: 65,
                grid: 0,
            }),
        },
    );

    assert_eq!(
        items::total_weight(&world.state, player, BODY_WEIGHT),
        packed,
        "the same coins weigh the same in the hand"
    );
    assert_eq!(
        items::total_gold(&world.state, player),
        1_000,
        "and are still the player's gold"
    );
}

#[test]
fn worn_plate_shows_an_armour_rating() {
    // Armour was a flat zero on the bar however dressed the character was. The
    // rating is ServUO's: each piece's class rating scaled by how much of the
    // body it covers — a plate chest (40) covering 35% of one is 14.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];

    assert_eq!(
        openshard_combat::armor::worn_armor_rating(&world.state, player),
        0,
        "a character in a shirt rates nothing"
    );

    wear(&mut world, connection, 0x1415, 0x0D); // plate chest, InnerTorso
    assert_eq!(
        openshard_combat::armor::worn_armor_rating(&world.state, player),
        14,
        "40 rating over 35% of a body"
    );

    wear(&mut world, connection, 0x1412, 0x06); // plate helm, Helm
    assert_eq!(
        openshard_combat::armor::worn_armor_rating(&world.state, player),
        19,
        "and 40 more over 14% of it"
    );
}

#[test]
fn armour_blunts_a_blow_and_bare_skin_does_not() {
    // The rating is not decorative: pre-AoS a swing gives up a share of it
    // (ServUO's `AbsorbDamage`). Rolled, so this compares totals over many
    // blows rather than asserting one — the roll is seeded, so it is stable.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];

    let through = |world: &mut World| -> u32 {
        (0..200)
            .map(|_| {
                u32::from(openshard_combat::armor::absorb_physical(
                    &mut world.state,
                    player,
                    30,
                ))
            })
            .sum()
    };

    let naked = through(&mut world);
    assert_eq!(naked, 200 * 30, "nothing worn absorbs nothing");

    // A full plate suit, every layer.
    for (graphic, layer) in [
        (0x1415, 0x0D), // chest
        (0x1412, 0x06), // helm
        (0x1410, 0x13), // arms
        (0x1411, 0x04), // legs
        (0x1414, 0x07), // gloves
        (0x1413, 0x0A), // gorget
    ] {
        wear(&mut world, connection, graphic, layer);
    }
    let armoured = through(&mut world);
    assert!(
        armoured < naked / 2,
        "plate turns most of a blow: {armoured} through vs {naked} bare"
    );
}

#[test]
fn the_bar_is_resent_only_when_a_number_moves() {
    // The alternative — a re-send beside every item mutation — is the pattern
    // that decays: the first system to move an item without knowing about the
    // status bar drops the update silently. So the pass diffs instead, and a
    // player who does nothing costs nothing.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let pack = backpack_of(&world, connection);

    // Past entry, and past the first refresh that records the baseline.
    for _ in 0..=status::STATUS_REFRESH_TICKS {
        world.tick(now);
    }
    let _ = packets_for(&mut world, connection);

    for _ in 0..=status::STATUS_REFRESH_TICKS {
        world.tick(now);
    }
    let quiet = packets_for(&mut world, connection);
    assert!(
        !quiet.iter().any(|p| p.first() == Some(&MobileStatus::ID)),
        "a still player is sent no status"
    );

    put_in(&mut world, pack, GOLD, 100);
    for _ in 0..=status::STATUS_REFRESH_TICKS {
        world.tick(now);
    }
    let after = packets_for(&mut world, connection);
    assert!(
        after.iter().any(|p| p.first() == Some(&MobileStatus::ID)),
        "gold arriving redraws the bar"
    );
}

#[test]
fn a_wound_closes_on_its_own_and_poison_stops_it() {
    // Mana and stamina trickled back and hit points never did, so a wounded
    // character could only ever be healed by someone else. ServUO's pre-AoS
    // rate is a point every eleven seconds — and none at all while poisoned.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let max = world.state.registry.get::<Hitpoints>(player).unwrap().max;
    world.state.registry.insert(
        player,
        Hitpoints {
            current: max / 2,
            max,
        },
    );

    for _ in 0..=openshard_combat::HITS_REGEN_TICKS {
        world.tick(now);
    }
    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(player)
            .unwrap()
            .current,
        max / 2 + 1,
        "a point back after the regen interval"
    );

    world.state.registry.insert(
        player,
        openshard_state::components::Poisoned {
            level: 1,
            next_pulse: u64::MAX,
            pulses_left: 10,
        },
    );
    let poisoned_at = world
        .state
        .registry
        .get::<Hitpoints>(player)
        .unwrap()
        .current;
    for _ in 0..=openshard_combat::HITS_REGEN_TICKS {
        world.tick(now);
    }
    assert_eq!(
        world
            .state
            .registry
            .get::<Hitpoints>(player)
            .unwrap()
            .current,
        poisoned_at,
        "the poisoned do not mend"
    );
}

#[test]
fn an_overloaded_walker_tires_and_is_finally_refused() {
    // The stamina pool existed and nothing spent it. Being over the carry cap is
    // what spends it — ServUO's `WeightOverloading` — and a pool at zero is the
    // one thing that stops a mule dead.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let pack = backpack_of(&world, connection);
    // 22,000 coins: 440 stones, a good fifty past what a 100-strength character
    // may carry. Heavy enough to tire, light enough to stagger on for a while.
    put_in(&mut world, pack, GOLD, 22_000);

    let full = world.state.registry.get::<Stamina>(player).unwrap().current;
    assert!(full > 0, "a fresh character has stamina");

    let refusal = world.spend_step_stamina(player, false);
    assert!(refusal.is_none(), "the first overloaded step is allowed");
    let after = world.state.registry.get::<Stamina>(player).unwrap().current;
    assert!(after < full, "but it costs stamina: {full} -> {after}");

    // Keep walking and the pool runs out; then nothing moves.
    let mut refusal = None;
    for _ in 0..100 {
        refusal = world.spend_step_stamina(player, false);
        if refusal.is_some() {
            break;
        }
    }
    assert!(
        // Which of the two messages depends on whether the load or the last
        // point below a tenth of the pool took it to zero — ServUO's order, and
        // both say the same thing to the player.
        refusal.is_some_and(|message| message.contains("too fatigued")),
        "an exhausted overloaded walker is told why it cannot move"
    );
    assert_eq!(
        world.state.registry.get::<Stamina>(player).unwrap().current,
        0
    );
}

#[test]
fn a_crushing_load_cannot_be_walked_off_at_all() {
    // The cost scales with how far over the line the load is (`5 + over/25`), so
    // enough of it empties a full pool in a single step and the mule simply does
    // not move. ServUO's arithmetic, and the reason a bank run is made in trips.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let pack = backpack_of(&world, connection);
    // Nearly two hundred thousand coins: four thousand stones.
    for _ in 0..3 {
        put_in(&mut world, pack, GOLD, u16::MAX);
    }

    assert!(
        world.spend_step_stamina(player, false).is_some(),
        "the very first step is refused"
    );
    assert_eq!(
        world.state.registry.get::<Stamina>(player).unwrap().current,
        0
    );
}

#[test]
fn an_unburdened_walk_is_all_but_free() {
    // Faithful, not punitive: ServUO charges a point every sixteenth step on
    // foot and nothing else. Against the regen that is very nearly a wash, which
    // is why classic running feels endless without being free.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let full = world.state.registry.get::<Stamina>(player).unwrap().current;

    for _ in 0..openshard_combat::STEPS_PER_STAMINA - 1 {
        assert!(world.spend_step_stamina(player, true).is_none());
    }
    assert_eq!(
        world.state.registry.get::<Stamina>(player).unwrap().current,
        full,
        "fifteen steps cost nothing"
    );
    assert!(world.spend_step_stamina(player, true).is_none());
    assert_eq!(
        world.state.registry.get::<Stamina>(player).unwrap().current,
        full - 1,
        "the sixteenth costs one"
    );
}

#[test]
fn staff_never_tire() {
    // A game master carrying a town's worth of gold still walks. Fatigue is a
    // player rule, as it is in ServUO (`IsStaff` returns before the weighing).
    let now = Instant::now();
    let mut world = world();
    let connection = enter_gm(&mut world, now);
    let player = world.state.players[&connection];
    let pack = backpack_of(&world, connection);
    put_in(&mut world, pack, GOLD, u16::MAX);
    put_in(&mut world, pack, GOLD, u16::MAX);

    let full = world.state.registry.get::<Stamina>(player).unwrap().current;
    for _ in 0..50 {
        assert!(world.spend_step_stamina(player, true).is_none());
    }
    assert_eq!(
        world.state.registry.get::<Stamina>(player).unwrap().current,
        full,
        "staff pay nothing"
    );
}

#[test]
fn the_logout_request_is_answered() {
    // The client announces it is leaving and then *waits*. Both references ack
    // it; a shard that stays silent leaves the paperdoll's "Log Out" hanging
    // until the client times out, with nothing anywhere to say why.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::LogoutRequest { connection });
    world.tick(now);
    assert!(
        packets_for(&mut world, connection).contains(&vec![0xD1, 0x01]),
        "the logout is granted"
    );
}

#[test]
fn gm_mode_off_makes_a_game_master_a_player() {
    // Sphere's `.GM`, and the reason it exists: the account's authority and the
    // staff *mode* are two different things, so a game master can walk under the
    // rules they are testing. With the mode off the exemptions go — fatigue bites
    // — and the commands stay, which is what lets them switch back.
    let now = Instant::now();
    let mut world = world();
    let connection = enter_gm(&mut world, now);
    let player = world.state.players[&connection];
    let pack = backpack_of(&world, connection);
    put_in(&mut world, pack, GOLD, 22_000); // 440 stones, past the 394 cap

    assert!(
        world.state.is_staff(player),
        "a staff login starts in GM mode"
    );
    assert!(
        world.spend_step_stamina(player, false).is_none()
            && world.state.registry.get::<Stamina>(player).unwrap().current
                == world.state.registry.get::<Stamina>(player).unwrap().max,
        "and pays nothing for its load"
    );

    gm::run(&mut world.state, player, "gm off");
    assert!(!world.state.is_staff(player), "the mode is off");
    assert!(
        world.state.staff_authority(player),
        "but the authority is untouched"
    );
    world.spend_step_stamina(player, false);
    assert!(
        world.state.registry.get::<Stamina>(player).unwrap().current
            < world.state.registry.get::<Stamina>(player).unwrap().max,
        "and now the load costs stamina"
    );

    // And back: the command still runs with the mode off, or there would be no
    // way out of it.
    gm::run(&mut world.state, player, "gm");
    assert!(world.state.is_staff(player), "toggled back on");
}

#[test]
fn gm_mode_off_hides_the_dead() {
    // The other exemption on the same switch: staff see ghosts. With the mode off
    // a game master is as blind to them as any living player, which is the only
    // way to check that gate from a staff account.
    let now = Instant::now();
    let mut world = world();
    let watcher = enter_gm(&mut world, now);
    let ghost = ConnectionId::from_raw(9);
    super::tests::enter_as(&mut world, ghost, now);
    let dead = world.state.players[&ghost];
    let dead_serial = world.state.registry.serial_of(dead).unwrap();
    world.enter_ghost_state(dead, dead_serial, true);
    let staff = world.state.players[&watcher];

    assert!(
        world.state.can_see_mobile(staff, dead),
        "a game master sees the dead"
    );
    gm::run(&mut world.state, staff, "gm off");
    assert!(
        !world.state.can_see_mobile(staff, dead),
        "and stops seeing them with the mode off"
    );
}

#[test]
fn banked_gold_reaches_the_bar_only_when_the_operator_says_so() {
    // Two shards, one line of config apart. Off is UO's own answer — the box is
    // virtual, so its gold is not yours to display — and on is the convenience a
    // shard may prefer. Weight is not on the switch either way.
    let now = Instant::now();
    for (flag, expected) in [(false, 0), (true, 1_000)] {
        let mut world = world().with_gameplay(Gameplay {
            bank_gold_in_status: flag,
            ..Gameplay::default()
        });
        let connection = enter(&mut world, now);
        let player = world.state.players[&connection];
        let bank = worn_container_on(&world, connection, items::BANK_LAYER);
        let weightless = items::total_weight(&world.state, player, BODY_WEIGHT);
        put_in(&mut world, bank, GOLD, 1_000);

        assert_eq!(
            world.derived_status(player).gold,
            expected,
            "bank_gold_in_status = {flag}"
        );
        assert_eq!(
            items::total_weight(&world.state, player, BODY_WEIGHT),
            weightless,
            "banked gold is never carried, whatever the flag says"
        );
    }
}

#[test]
fn a_purse_inside_the_bank_is_still_banked() {
    // The bank box holds a tree like any container, so the balance has to walk it
    // — a one-level scan of the box would miss coins in a pouch inside it, which
    // is exactly where a tidy player puts them.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let bank = worn_container_on(&world, connection, items::BANK_LAYER);

    let (pouch, pouch_serial) = world
        .state
        .registry
        .spawn_with_serial(SerialKind::Item)
        .unwrap();
    world
        .state
        .registry
        .insert(pouch, Graphic { id: 0x0E79, hue: 0 });
    world
        .state
        .registry
        .insert(pouch, Container { gump: 0x003C });
    world.state.registry.insert(
        pouch,
        Contained {
            container: bank,
            x: 10,
            y: 10,
            grid: 0,
        },
    );
    put_in(&mut world, pouch_serial, GOLD, 700);
    put_in(&mut world, bank, GOLD, 300);

    assert_eq!(items::banked_gold(&world.state, player), 1_000);
}

#[test]
fn a_vendor_takes_the_bank_when_the_pack_is_short() {
    // ServUO's `BaseVendor`: the pack first, then the bank, and the vendor says
    // which paid. Without the fallback a banked fortune buys nothing, which is
    // what correcting the weight rule left behind.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let bank = worn_container_on(&world, connection, items::BANK_LAYER);
    put_in(&mut world, bank, GOLD, 500);
    let vendor =
        super::tests::spawn_stocked_vendor(&mut world, Point::new(START.0 + 1, START.1, 0), now);
    let stock = stock_line_serial(&world, vendor);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::Buy {
        connection,
        vendor,
        purchases: vec![openshard_protocol::Purchase {
            serial: stock,
            amount: 10, // 10 × 4 gold
        }],
    });
    world.tick(now);

    assert_eq!(
        items::banked_gold(&world.state, player),
        460,
        "the bank paid the forty"
    );
    assert!(
        items::carried(&world.state, player)
            .iter()
            .any(|&(graphic, amount)| graphic == 0x0F7A && amount == 10),
        "and the goods are in the pack"
    );
}

#[test]
fn with_bank_payment_off_a_banked_fortune_buys_nothing() {
    // The same purchase on a shard that keeps the money strictly in hand.
    let now = Instant::now();
    let mut world = world().with_gameplay(Gameplay {
        vendor_bank_payment: false,
        ..Gameplay::default()
    });
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let bank = worn_container_on(&world, connection, items::BANK_LAYER);
    put_in(&mut world, bank, GOLD, 500);
    let vendor =
        super::tests::spawn_stocked_vendor(&mut world, Point::new(START.0 + 1, START.1, 0), now);
    let stock = stock_line_serial(&world, vendor);

    world.queue(Command::Buy {
        connection,
        vendor,
        purchases: vec![openshard_protocol::Purchase {
            serial: stock,
            amount: 10,
        }],
    });
    world.tick(now);

    assert_eq!(
        items::banked_gold(&world.state, player),
        500,
        "nothing was withdrawn"
    );
    assert!(
        !items::carried(&world.state, player)
            .iter()
            .any(|&(graphic, _)| graphic == 0x0F7A),
        "and nothing was handed over"
    );
}

/// The serial of the one line a `spawn_stocked_vendor` vendor has in stock.
fn stock_line_serial(world: &World, vendor_serial: u32) -> u32 {
    let vendor = world
        .state
        .registry
        .entity_of(Serial::new(vendor_serial).unwrap())
        .unwrap();
    let owner = world.state.registry.serial_of(vendor).unwrap();
    let crate_serial = world
        .state
        .registry
        .query::<Equipped>()
        .find(|(_, worn)| worn.mobile == owner && worn.layer == npc::STOCK_LAYER)
        .and_then(|(entity, _)| world.state.registry.serial_of(entity))
        .expect("a vendor wears a stock crate");
    items::contents_of(&world.state, crate_serial)
        .first()
        .expect("the crate holds a line")
        .serial
}
