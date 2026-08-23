//! Quests: offering, accepting, progress, turn-in, and the two failures the
//! pack-side version had — a giver that stopped being one after a restart, and a
//! turn-in that took some of what it asked for and paid nothing.
//!
//! A child module rather than more of `tests.rs`, which is long past the size a
//! file should be. These read private world state, so they stay inside the
//! module.

use super::tests::{START, enter, packets_for, spawn_mobile_at, world};
use super::*;
use openshard_protocol::containers::GridSlot;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::serial::RawSerial;
use openshard_quests::{QUEST_GUMP, QUEST_RESIGN_GUMP};
use openshard_state::components::{Amount, Contained, Drawn, QuestGiver, QuestLog, Stackable};
use openshard_state::quest::{ObjectiveDef, ObjectiveKind, QuestDef, RewardDef, RewardKind};

/// The body a rat is drawn as — the slay quests' target.
const RAT: u16 = 0x00EE;
/// Spiders' silk, the obtain quests' target.
const SILK: u16 = 0x0F8D;

/// A quest asking for five rats, paying 250 gold.
fn rat_cull() -> QuestDef {
    QuestDef {
        key: "rat_cull".to_owned(),
        title: "A Plague of Rats".to_owned(),
        description: "Slay five rats.".to_owned(),
        complete: "Well done.".to_owned(),
        objectives: vec![ObjectiveDef {
            kind: ObjectiveKind::Slay {
                body: openshard_protocol::wire::Graphic(RAT),
            },
            count: 5,
            name: "sewer rat".to_owned(),
            seconds: 0,
        }],
        rewards: vec![RewardDef {
            kind: RewardKind::Gold(250),
            name: "250 gold".to_owned(),
        }],
        ..QuestDef::default()
    }
}

/// A quest asking for five skeins of silk.
fn silk_gather() -> QuestDef {
    QuestDef {
        key: "silk_gather".to_owned(),
        title: "Silk for the Spellwright".to_owned(),
        objectives: vec![ObjectiveDef {
            kind: ObjectiveKind::Obtain {
                graphic: openshard_protocol::wire::Graphic(SILK),
            },
            count: 5,
            name: "spiders' silk".to_owned(),
            seconds: 0,
        }],
        rewards: vec![RewardDef {
            kind: RewardKind::Gold(120),
            name: "120 gold".to_owned(),
        }],
        ..QuestDef::default()
    }
}

/// Put a quest giver on the map beside the start, bound to `keys`.
fn place_giver(world: &mut World, keys: &[&str], now: Instant) -> Serial {
    let at = Point::new(START.0 + 1, START.1, 0);
    let serial = spawn_mobile_at(world, at, 100, now);
    let entity = world.state.registry.entity_of(serial).unwrap();
    world.state.registry.insert(
        entity,
        QuestGiver {
            keys: keys.iter().map(|&k| k.to_owned()).collect(),
        },
    );
    serial
}

/// The player's quest log, or an empty one.
fn log_of(world: &World, connection: ConnectionId) -> QuestLog {
    let player = world.state.players[&connection];
    world
        .state
        .registry
        .get::<QuestLog>(player)
        .cloned()
        .unwrap_or_default()
}

/// Answer the open quest gump with a button.
fn press(
    world: &mut World,
    connection: ConnectionId,
    gump_id: openshard_protocol::gump::GumpId,
    button: u32,
) {
    press_with(world, connection, gump_id, button, Vec::new());
}

/// Answer with a button and a set of switches on — the resign dialog's shape.
fn press_with(
    world: &mut World,
    connection: ConnectionId,
    gump_id: openshard_protocol::gump::GumpId,
    button: u32,
    switches: Vec<u32>,
) {
    world.queue(Command::GumpResponse {
        connection,
        response: openshard_protocol::gump::GumpResponse {
            serial: openshard_protocol::gump::RawGumpKey(0),
            gump_id: openshard_protocol::gump::RawGumpId(gump_id.0),
            button: openshard_protocol::gump::RawButtonId(button),
            switches: switches
                .into_iter()
                .map(openshard_protocol::gump::RawSwitchId)
                .collect(),
            text_entries: Vec::new(),
        },
    });
}

/// Whether any packet this tick was a gump display (`0xB0`).
fn drew_a_gump(world: &mut World, connection: ConnectionId) -> bool {
    packets_for(world, connection)
        .iter()
        .any(|packet| packet.first() == Some(&0xB0))
}

/// Register a set of quests on the world.
fn register(world: &mut World, quests: Vec<QuestDef>) {
    world.state.quests.set(quests);
}

#[test]
fn a_double_clicked_giver_offers_its_quest() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);
    let giver = place_giver(&mut world, &["rat_cull"], now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);

    assert!(
        drew_a_gump(&mut world, connection),
        "double-clicking a giver draws the offer"
    );
    assert!(
        log_of(&world, connection).active.is_empty(),
        "an offer is not an acceptance"
    );
}

#[test]
fn accepting_puts_the_quest_in_the_log() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);
    let giver = place_giver(&mut world, &["rat_cull"], now);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    // Button 4 is Accept — ServUO's `Buttons.AcceptQuest`.
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    let log = log_of(&world, connection);
    assert_eq!(log.active.len(), 1);
    assert_eq!(log.active[0].key, "rat_cull");
    assert_eq!(log.active[0].progress, vec![0]);
}

#[test]
fn refusing_starts_nothing() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);
    let giver = place_giver(&mut world, &["rat_cull"], now);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 2); // Refuse
    world.tick(now);

    assert!(log_of(&world, connection).active.is_empty());
}

#[test]
fn a_reply_to_a_gump_that_was_never_opened_does_nothing() {
    // The context is the server's memory of what it drew. Without it a crafted
    // `0xB1` naming the quest gump would accept whatever was pending.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);

    press(&mut world, connection, QUEST_GUMP, 4); // Accept, out of nowhere
    world.tick(now);

    assert!(log_of(&world, connection).active.is_empty());
}

#[test]
fn the_paperdoll_quest_button_opens_the_log() {
    // `0xD7` subcommand `0x32`. Nothing decoded it before, so the button did
    // nothing at all and there was no way to see an accepted quest.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::QuestLogRequest { connection });
    world.tick(now);

    assert!(
        drew_a_gump(&mut world, connection),
        "an empty log still opens — silence looks like a broken button"
    );
}

#[test]
fn a_slain_body_advances_only_the_killers_objective() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);
    let giver = place_giver(&mut world, &["rat_cull"], now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    let player = world.state.players[&connection];
    let killer = world.state.registry.serial_of(player).unwrap();
    world.state.bus.send(openshard_combat::MobileDied {
        entity: player,
        serial: killer,
        body: openshard_protocol::wire::Graphic(RAT),
        killer: Some(killer),
    });
    world.tick(now);

    assert_eq!(log_of(&world, connection).active[0].progress, vec![1]);
}

#[test]
fn an_unattributed_death_advances_nothing() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);
    let giver = place_giver(&mut world, &["rat_cull"], now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    let player = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player).unwrap();
    world.state.bus.send(openshard_combat::MobileDied {
        entity: player,
        serial,
        body: openshard_protocol::wire::Graphic(RAT),
        killer: None, // a field, a fall, a reflected blow
    });
    world.tick(now);

    assert_eq!(log_of(&world, connection).active[0].progress, vec![0]);
}

#[test]
fn obtain_progress_is_found_by_the_diffing_pass_not_announced() {
    // Nothing in the engine says "an item moved". The pass looks instead, which
    // is why picking the silk up counts without any call beside the insert — and
    // why dropping it counts down again.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![silk_gather()]);
    let giver = place_giver(&mut world, &["silk_gather"], now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let backpack = openshard_items::backpack_of(&world.state, owner).unwrap();
    let silk = put_silk(&mut world, backpack, 5);

    tick_past_the_obtain_cadence(&mut world, now);
    assert_eq!(
        log_of(&world, connection).active[0].progress,
        vec![5],
        "carrying five counts as five"
    );

    // And it falls back when they are gone.
    world.state.registry.despawn(silk);
    tick_past_the_obtain_cadence(&mut world, now);
    assert_eq!(
        log_of(&world, connection).active[0].progress,
        vec![0],
        "an objective that says 'carry five' is false once you are not"
    );
}

#[test]
fn a_turn_in_takes_the_items_and_pays() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![silk_gather()]);
    let giver = place_giver(&mut world, &["silk_gather"], now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let backpack = openshard_items::backpack_of(&world.state, owner).unwrap();
    put_silk(&mut world, backpack, 5);
    tick_past_the_obtain_cadence(&mut world, now);

    // Talk to the giver again: the complete page, then hand in, then take the
    // reward.
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 8); // Complete
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 5); // Accept reward
    world.tick(now);

    let log = log_of(&world, connection);
    assert!(log.active.is_empty(), "the quest leaves the log");
    assert_eq!(log.done.len(), 1, "and is remembered as done");
    assert_eq!(
        openshard_items::carried_amount(&world.state, owner, openshard_protocol::wire::Graphic(SILK)),
        0,
        "the silk was handed over"
    );
    assert_eq!(
        openshard_items::carried_amount(&world.state, owner, openshard_protocol::wire::Graphic(0x0EED)),
        120,
        "and the gold arrived"
    );
}

#[test]
fn a_player_one_item_short_loses_nothing_and_is_paid_nothing() {
    // The pack's version took each objective independently, so a player short on
    // the second lost what they brought for the first — invisibly.
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let two_part = QuestDef {
        key: "two_part".to_owned(),
        title: "Two Things".to_owned(),
        objectives: vec![
            ObjectiveDef {
                kind: ObjectiveKind::Obtain {
                    graphic: openshard_protocol::wire::Graphic(SILK),
                },
                count: 2,
                name: "silk".to_owned(),
                seconds: 0,
            },
            ObjectiveDef {
                kind: ObjectiveKind::Obtain {
                    graphic: openshard_protocol::wire::Graphic(0x0F7A),
                },
                count: 2,
                name: "garlic".to_owned(),
                seconds: 0,
            },
        ],
        ..QuestDef::default()
    };
    register(&mut world, vec![two_part]);
    let giver = place_giver(&mut world, &["two_part"], now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let backpack = openshard_items::backpack_of(&world.state, owner).unwrap();
    put_silk(&mut world, backpack, 2); // the first objective only

    // Force the quest to *look* complete, so the turn-in is reached at all: the
    // point under test is what happens when the items are not really there.
    {
        let mut log = log_of(&world, connection);
        log.active[0].progress = vec![2, 2];
        world.state.registry.insert(player, log);
    }
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 8);
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 5);
    world.tick(now);

    assert_eq!(
        openshard_items::carried_amount(&world.state, owner, openshard_protocol::wire::Graphic(SILK)),
        2,
        "nothing was taken, because not everything could be"
    );
    assert!(
        !log_of(&world, connection).active.is_empty(),
        "and the quest is still open"
    );
}

#[test]
fn resigning_needs_the_yes_radio() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);
    let giver = place_giver(&mut world, &["rat_cull"], now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    // Open the log, the quest's page, then Resign.
    world.queue(Command::QuestLogRequest { connection });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 11); // the first row
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 3); // Resign
    world.tick(now);

    // "No" keeps it.
    press_with(&mut world, connection, QUEST_RESIGN_GUMP, 1, vec![0]);
    world.tick(now);
    assert_eq!(
        log_of(&world, connection).active.len(),
        1,
        "answering no keeps the quest"
    );

    press(&mut world, connection, QUEST_GUMP, 3);
    world.tick(now);
    press_with(&mut world, connection, QUEST_RESIGN_GUMP, 1, vec![1]);
    world.tick(now);
    assert!(
        log_of(&world, connection).active.is_empty(),
        "answering yes gives it up"
    );
}

#[test]
fn a_done_once_quest_is_never_offered_again() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let mut quest = rat_cull();
    quest.done_once = true;
    quest.objectives[0].count = 1;
    register(&mut world, vec![quest]);
    let giver = place_giver(&mut world, &["rat_cull"], now);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    // Finish it.
    let player = world.state.players[&connection];
    let killer = world.state.registry.serial_of(player).unwrap();
    world.state.bus.send(openshard_combat::MobileDied {
        entity: player,
        serial: killer,
        body: openshard_protocol::wire::Graphic(RAT),
        killer: Some(killer),
    });
    world.tick(now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 8);
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 5);
    world.tick(now);
    assert!(log_of(&world, connection).active.is_empty());

    // And it may not be taken again.
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);
    assert!(
        log_of(&world, connection).active.is_empty(),
        "a once-only quest stays done"
    );
}

#[test]
fn a_completed_quest_reaches_the_pack() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let mut quest = rat_cull();
    quest.objectives[0].count = 1;
    register(&mut world, vec![quest]);
    let giver = place_giver(&mut world, &["rat_cull"], now);
    let mut done: Cursor<openshard_quests::QuestCompleted> = world.bus().cursor();

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);
    let player = world.state.players[&connection];
    let killer = world.state.registry.serial_of(player).unwrap();
    world.state.bus.send(openshard_combat::MobileDied {
        entity: player,
        serial: killer,
        body: openshard_protocol::wire::Graphic(RAT),
        killer: Some(killer),
    });
    world.tick(now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 8);
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 5);
    world.tick(now);

    let events: Vec<_> = world.bus().read(&mut done).cloned().collect();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].key, "rat_cull");
}

/// Put a stack of silk in a container.
fn put_silk(world: &mut World, container: Serial, amount: u16) -> EntityId {
    let (item, _) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        item,
        Drawn {
            id: openshard_protocol::wire::Graphic(SILK),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    world.state.registry.insert(item, Amount(amount));
    world.state.registry.insert(item, Stackable);
    world.state.registry.insert(
        item,
        Contained {
            container,
            position: GumpPoint::new(0, 0),
            grid: GridSlot(0),
        },
    );
    item
}

/// Tick until the obtain pass has certainly run.
fn tick_past_the_obtain_cadence(world: &mut World, now: Instant) {
    for _ in 0..=openshard_quests::OBTAIN_EVERY_TICKS {
        world.tick(now);
    }
}

#[test]
fn a_quest_giver_is_still_a_giver_after_a_restart() {
    // The headline failure of the pack-side version. The binding lived in a JS
    // map filled on `MobileSpawned`, and restored NPCs announce no such thing —
    // so the shard's quests worked on the boot where the world was populated and
    // were inert on every boot after, with nothing anywhere to say why.
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);
    let giver = place_giver(&mut world, &["rat_cull"], now);
    world.take_snapshot();
    let snapshot = world.drain_saves().next().expect("a snapshot");
    let mobiles = snapshot.mobiles.clone().expect("the mobile sweep");
    assert!(
        mobiles
            .iter()
            .any(|m| m.serial == giver && m.quest_giver == ["rat_cull"]),
        "the binding is in the save"
    );

    // The restart: a fresh world restored from the records alone, and a player
    // who was never here when the giver was placed.
    let mut shard = super::tests::world();
    let filed = super::tests::nothing_restored_first(&mut shard);
    shard.restore_mobiles(mobiles, &filed);
    shard.state.quests.set(vec![rat_cull()]);
    let connection = enter(&mut shard, now);
    let _ = packets_for(&mut shard, connection);

    shard.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    shard.tick(now);
    assert!(
        drew_a_gump(&mut shard, connection),
        "and the giver still offers its quest"
    );
}

#[test]
fn restoring_a_mobile_announces_it_as_restored_not_as_spawned() {
    // The two must stay different events: a handler that *creates* on a spawn (a
    // vendor's stock crate) would duplicate it every reboot if they were one.
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    let giver = place_giver(&mut world, &["rat_cull"], now);
    world.take_snapshot();
    let mobiles = world
        .drain_saves()
        .next()
        .expect("a snapshot")
        .mobiles
        .clone()
        .expect("the mobile sweep");

    let mut shard = super::tests::world();
    let filed = super::tests::nothing_restored_first(&mut shard);
    let mut restored: Cursor<crate::events::MobileRestored> = shard.bus().cursor();
    let mut spawned: Cursor<openshard_npc::MobileSpawned> = shard.bus().cursor();
    shard.restore_mobiles(mobiles, &filed);

    let restores: Vec<_> = shard.bus().read(&mut restored).cloned().collect();
    assert!(
        restores.iter().any(|e| e.serial == giver),
        "a restored NPC says so"
    );
    assert_eq!(
        shard.bus().read(&mut spawned).count(),
        0,
        "and does not claim to have spawned"
    );
}

#[test]
fn a_restore_announces_the_post_an_npc_belongs_to_not_where_it_wandered() {
    // A pack binds its NPCs by tile: the tile a quest giver was placed on is the
    // key its quest is looked up by, and `MobileRestored` is what lets it re-bind
    // on every boot. A townsperson does not stand still, and with a daily routine
    // it is somewhere else entirely for a third of the day — so a save taken while
    // one had wandered would hand its quest to whoever was standing nearest its
    // post instead, permanently, because the binding is itself persisted. The
    // event carries the post, which does not move.
    let now = Instant::now();
    let mut world = world();
    let _ = enter(&mut world, now);
    // A townsperson, not a bare mobile: in ServUO a `MondainQuester` *is* a
    // `BaseVendor`, and it is the townsfolk — the ones with an `Npc` beat and a
    // post to keep to — that a routine walks off at dusk.
    let entity =
        super::tests::spawn_townsperson(&mut world, "the healer", Point::new(START.0 + 1, START.1, 0), now);
    world.state.registry.insert(
        entity,
        QuestGiver {
            keys: vec!["rat_cull".to_owned()],
        },
    );
    let giver = world.registry().serial_of(entity).unwrap();
    let post = world.registry().get::<Position>(entity).unwrap().0;

    // Walk it off its post, the way a night routine would, and save it there.
    let wandered = Point::new(post.x + 5, post.y + 4, post.z);
    world.state.registry.insert(entity, Position(wandered));
    world.take_snapshot();
    let mobiles = world
        .drain_saves()
        .next()
        .expect("a snapshot")
        .mobiles
        .clone()
        .expect("the mobile sweep");

    let mut shard = super::tests::world();
    let filed = super::tests::nothing_restored_first(&mut shard);
    let mut restored: Cursor<crate::events::MobileRestored> = shard.bus().cursor();
    shard.restore_mobiles(mobiles, &filed);
    let event = shard
        .bus()
        .read(&mut restored)
        .find(|e| e.serial.raw() == giver.raw())
        .copied()
        .expect("the giver announced its restore");
    assert_eq!(event.at, wandered, "it is standing where it was saved");
    assert_eq!(
        event.home, post,
        "and it announces the post it belongs to, which is what a pack binds by"
    );
}

#[test]
fn a_quest_log_survives_a_restart_with_its_progress_and_cooldowns() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![rat_cull()]);
    let giver = place_giver(&mut world, &["rat_cull"], now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    let player = world.state.players[&connection];
    let killer = world.state.registry.serial_of(player).unwrap();
    world.state.bus.send(openshard_combat::MobileDied {
        entity: player,
        serial: killer,
        body: openshard_protocol::wire::Graphic(RAT),
        killer: Some(killer),
    });
    world.tick(now);
    assert_eq!(log_of(&world, connection).active[0].progress, vec![1]);

    world.take_snapshot();
    let record = world
        .drain_saves()
        .next()
        .expect("a snapshot")
        .characters
        .into_iter()
        .find(|c| c.serial == killer)
        .expect("the character");
    assert_eq!(record.quests.len(), 1);
    assert_eq!(record.quests[0].progress, vec![1]);

    // And it comes back on login.
    let mut shard = super::tests::world();
    shard.state.quests.set(vec![rat_cull()]);
    // The boot path: the row goes into the world's roster, and the entry names
    // the character rather than carrying it. See `docs/connection_state.md`, S4.
    shard.restore_characters(vec![record]);
    shard.queue(Command::Enter(Entering {
        connection: connection_two(),
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Lord British".to_owned()),
        access: AccessLevel::Player,
        // Nothing but the name: the row went in through the boot path above,
        // and the world unpacks it itself.
        character: Character::Saved,
    }));
    shard.tick(now);

    let log = log_of(&shard, connection_two());
    assert_eq!(log.active.len(), 1, "the quest came back");
    assert_eq!(log.active[0].progress, vec![1], "with its progress");
}

/// A second connection id, for the relog half of the persistence tests.
fn connection_two() -> ConnectionId {
    ConnectionId::from_raw(2)
}

#[test]
fn double_clicking_an_escortable_offers_rather_than_starts_following() {
    // The bug this pins: the escort used to begin on the click, so an NPC walked
    // off after anyone who so much as looked at it — no offer, no log entry, and
    // no way to say no. ServUO starts the follow in `BaseQuest.OnAccept`, and so
    // does this.
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![escort_quest()]);
    let giver = place_giver(&mut world, &["escort"], now);
    make_escortable(&mut world, giver);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);

    assert!(drew_a_gump(&mut world, connection), "the click offers the quest");
    assert!(
        escorter_of(&world, giver).is_none(),
        "and nobody is being followed yet"
    );

    press(&mut world, connection, QUEST_GUMP, 4); // Accept
    world.tick(now);

    let player = world.state.players[&connection];
    let expected = world.state.registry.serial_of(player);
    assert_eq!(
        escorter_of(&world, giver),
        expected,
        "accepting is what starts the escort"
    );
}

#[test]
fn resigning_an_escort_stops_it_following() {
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![escort_quest()]);
    let giver = place_giver(&mut world, &["escort"], now);
    make_escortable(&mut world, giver);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);
    assert!(escorter_of(&world, giver).is_some());

    world.queue(Command::QuestLogRequest { connection });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 11); // the row
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 3); // Resign
    world.tick(now);
    press_with(&mut world, connection, QUEST_RESIGN_GUMP, 1, vec![1]); // yes
    world.tick(now);

    assert!(
        escorter_of(&world, giver).is_none(),
        "a resigned escort does not keep trailing the player"
    );
}

/// The escort quest every traveller gives: no region of its own, so the
/// destination is whatever the giver asked for.
fn escort_quest() -> QuestDef {
    QuestDef {
        key: "escort".to_owned(),
        title: "An Escort Request".to_owned(),
        objectives: vec![ObjectiveDef {
            kind: ObjectiveKind::Escort {
                region: String::new(),
            },
            count: 1,
            name: "escort".to_owned(),
            seconds: 0,
        }],
        ..QuestDef::default()
    }
}

/// Make a placed NPC escortable, with a destination already fixed so the test
/// does not depend on a facet having named regions.
fn make_escortable(world: &mut World, serial: Serial) {
    let entity = world.state.registry.entity_of(serial).unwrap();
    world.state.registry.insert(
        entity,
        openshard_state::components::Escortable {
            destination: "Britain".to_owned(),
            escorter: None,
            last_seen: 0,
        },
    );
}

/// Who, if anyone, an escortable is following.
fn escorter_of(world: &World, serial: Serial) -> Option<Serial> {
    let entity = world.state.registry.entity_of(serial)?;
    world
        .state
        .registry
        .get::<openshard_state::components::Escortable>(entity)
        .and_then(|escort| escort.escorter)
}

#[test]
fn an_escort_names_its_destination_in_the_offer_and_the_log() {
    // It used to say "Escort to a destination" in both, because the town was
    // picked when the quest was *accepted* — which is no use to a player deciding
    // whether to walk across the facet. A traveller knows where it is going from
    // the moment it is placed.
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![escort_quest()]);
    let giver = place_giver(&mut world, &["escort"], now);
    register_towns(&mut world, now);
    // Bound with no destination, the way the pack binds every traveller.
    world.queue(Command::MakeEscortable {
        serial: giver,
        destination: String::new(),
    });
    world.tick(now);
    let _ = packets_for(&mut world, connection);

    // The offer's objectives page names it.
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    let _ = packets_for(&mut world, connection);
    press(&mut world, connection, QUEST_GUMP, 7); // Next -> Objectives
    world.tick(now);
    assert!(
        gump_says(&mut world, connection, "Minoc"),
        "the offer says where, before the player agrees to go"
    );

    // And so does the log, after accepting.
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);
    world.queue(Command::QuestLogRequest { connection });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 11);
    world.tick(now);
    let _ = packets_for(&mut world, connection);
    press(&mut world, connection, QUEST_GUMP, 7);
    world.tick(now);
    assert!(
        gump_says(&mut world, connection, "Minoc"),
        "and the log still says where"
    );
}

#[test]
fn a_traveller_with_nowhere_to_go_offers_nothing() {
    // A facet with no named regions cannot host an escort. Offering one anyway
    // would be offering a walk that can never be finished.
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![escort_quest()]);
    let giver = place_giver(&mut world, &["escort"], now);
    world.queue(Command::MakeEscortable {
        serial: giver,
        destination: String::new(),
    });
    world.tick(now);
    let _ = packets_for(&mut world, connection);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    assert!(!drew_a_gump(&mut world, connection), "no destination, no offer");
}

#[test]
fn re_binding_an_escortable_keeps_the_escort_it_is_on() {
    // The pack re-binds every NPC on restore, and a shard that saves mid-escort
    // must not drop it on the next boot.
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![escort_quest()]);
    let giver = place_giver(&mut world, &["escort"], now);
    register_towns(&mut world, now);
    world.queue(Command::MakeEscortable {
        serial: giver,
        destination: String::new(),
    });
    world.tick(now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);
    let leading = escorter_of(&world, giver);
    assert!(leading.is_some());

    world.queue(Command::MakeEscortable {
        serial: giver,
        destination: String::new(),
    });
    world.tick(now);
    assert_eq!(
        escorter_of(&world, giver),
        leading,
        "a re-bind does not drop an escort in progress"
    );
}

/// Whether the last gump drawn for a connection carries `text` in one of its
/// lines.
fn gump_says(world: &mut World, connection: ConnectionId, text: &str) -> bool {
    last_gump_lines(world, connection)
        .iter()
        .any(|line| line.contains(text))
}

/// The text lines of the last gump drawn for a connection.
fn last_gump_lines(world: &mut World, connection: ConnectionId) -> Vec<String> {
    let packet = packets_for(world, connection)
        .into_iter()
        .rfind(|p| p.first() == Some(&0xB0))
        .expect("a gump");
    let layout_len = u16::from_be_bytes([packet[19], packet[20]]) as usize;
    let mut at = 21 + layout_len;
    let count = u16::from_be_bytes([packet[at], packet[at + 1]]) as usize;
    at += 2;
    let mut lines = Vec::new();
    for _ in 0..count {
        let chars = u16::from_be_bytes([packet[at], packet[at + 1]]) as usize;
        at += 2;
        let units: Vec<u16> = (0..chars)
            .map(|i| u16::from_be_bytes([packet[at + i * 2], packet[at + i * 2 + 1]]))
            .collect();
        at += chars * 2;
        lines.push(String::from_utf16_lossy(&units));
    }
    lines
}

/// Two named regions on the default facet: the one the travellers stand in, and
/// somewhere for them to want to go.
fn register_towns(world: &mut World, now: Instant) {
    use openshard_state::{Region, RegionFlags, RegionId, RegionRect};
    let here = Region {
        id: RegionId(0),
        name: "Britain".to_owned(),
        priority: 50,
        rects: vec![RegionRect::new(START.0 - 20, START.1 - 20, 40, 40)],
        flags: RegionFlags::default(),
        music: None,
        light: None,
    };
    let away = Region {
        id: RegionId(0),
        name: "Minoc".to_owned(),
        priority: 50,
        rects: vec![RegionRect::new(START.0 + 200, START.1 + 200, 40, 40)],
        flags: RegionFlags::default(),
        music: None,
        light: None,
    };
    world.queue(Command::RegisterRegions {
        facet: Facet(0),
        regions: vec![here, away],
    });
    world.tick(now);
}

#[test]
fn an_escort_pays_on_reaching_its_destination() {
    // The arrival match only ever looked for an objective naming the region
    // literally, and the one shipped escort quest names none — so not one of its
    // sixty travellers could ever complete.
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![escort_quest()]);
    let giver = place_giver(&mut world, &["escort"], now);
    register_towns(&mut world, now);
    world.queue(Command::MakeEscortable {
        serial: giver,
        destination: String::new(),
    });
    world.tick(now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4); // Accept
    world.tick(now);
    assert_eq!(log_of(&world, connection).active[0].progress, vec![0]);

    // Put both of them inside the destination region and let the pass look.
    let player = world.state.players[&connection];
    let inside = Point::new(START.0 + 210, START.1 + 210, 0);
    teleport_to(&mut world, player, inside);
    let npc = world.state.registry.entity_of(giver).unwrap();
    teleport_to(&mut world, npc, inside);
    world.tick(now);

    assert_eq!(
        log_of(&world, connection).active[0].progress,
        vec![1],
        "arriving completes the escort"
    );
}

#[test]
fn a_delivery_completes_on_talking_to_its_destination() {
    // Deliver objectives never advanced at all: nothing outside the turn-in even
    // looked at the kind, so a delivery quest could be accepted and never
    // finished.
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![deliver_quest()]);
    let giver = place_giver(&mut world, &["deliver_silk"], now);
    let destination = place_named(&mut world, "Mirabel", now);

    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    // Talking to the destination without the goods does nothing.
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(destination.raw())),
    });
    world.tick(now);
    assert_eq!(
        log_of(&world, connection).active[0].progress,
        vec![0],
        "an empty-handed conversation is not a delivery"
    );

    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let backpack = openshard_items::backpack_of(&world.state, owner).unwrap();
    put_silk(&mut world, backpack, 2);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(destination.raw())),
    });
    world.tick(now);

    assert_eq!(
        log_of(&world, connection).active[0].progress,
        vec![2],
        "carrying them completes the objective"
    );
}

/// A quest asking for two skeins of silk taken to Mirabel.
fn deliver_quest() -> QuestDef {
    QuestDef {
        key: "deliver_silk".to_owned(),
        title: "A Parcel for Mirabel".to_owned(),
        objectives: vec![ObjectiveDef {
            kind: ObjectiveKind::Deliver {
                graphic: openshard_protocol::wire::Graphic(SILK),
                to: "Mirabel".to_owned(),
            },
            count: 2,
            name: "spiders' silk".to_owned(),
            seconds: 0,
        }],
        ..QuestDef::default()
    }
}

/// Place a plain named NPC — a delivery destination, which gives no quests.
fn place_named(world: &mut World, name: &str, now: Instant) -> Serial {
    let at = Point::new(START.0 + 2, START.1, 0);
    let serial = spawn_mobile_at(world, at, 100, now);
    let entity = world.state.registry.entity_of(serial).unwrap();
    world
        .state
        .registry
        .insert(entity, openshard_state::components::Name(name.to_owned()));
    serial
}

/// Move a mobile to a tile, sector index included.
fn teleport_to(world: &mut World, entity: EntityId, at: Point) {
    world
        .state
        .registry
        .insert(entity, openshard_state::components::Position(at));
    let facet = world.state.facet_of(entity);
    world
        .state
        .facet_state_mut(facet)
        .sectors
        .insert(entity, at, openshard_state::Occupant::Mobile);
}

#[test]
fn an_escorted_traveller_walks_after_its_escorter() {
    // The one behaviour a player sees immediately and the one nothing covered:
    // that the follow step is actually taken, not merely decided.
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    register(&mut world, vec![escort_quest()]);
    let giver = place_giver(&mut world, &["escort"], now);
    register_towns(&mut world, now);
    world.queue(Command::MakeEscortable {
        serial: giver,
        destination: String::new(),
    });
    world.tick(now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    // Walk the player a few tiles off and let the escort beat run.
    let player = world.state.players[&connection];
    let away = Point::new(START.0 + 6, START.1, 0);
    teleport_to(&mut world, player, away);
    let npc = world.state.registry.entity_of(giver).unwrap();
    let before = position_of(&world, npc);
    for _ in 0..20 {
        world.tick(now);
    }
    let after = position_of(&world, npc);

    assert_ne!(before, after, "the traveller followed");
    assert!(
        openshard_state::distance(after, away) < openshard_state::distance(before, away),
        "and got closer, rather than wandering"
    );
}

/// Where a mobile stands.
fn position_of(world: &World, entity: EntityId) -> Point {
    world
        .state
        .registry
        .get::<openshard_state::components::Position>(entity)
        .expect("a placed mobile")
        .0
}

#[test]
fn a_timed_objective_fails_when_its_seconds_run_out() {
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    let mut quest = rat_cull();
    quest.objectives[0].seconds = 2;
    register(&mut world, vec![quest]);
    let giver = place_giver(&mut world, &["rat_cull"], now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);
    assert!(!log_of(&world, connection).active[0].failed);

    // Two seconds of ticks, and a little over.
    for _ in 0..(3 * openshard_state::TICKS_PER_SECOND) {
        world.tick(now);
    }

    let log = log_of(&world, connection);
    assert!(log.active[0].failed, "the clock ran out");
    // A failed quest stays in the log, in red, until it is resigned — ServUO
    // shows it rather than removing it, so the player finds out why it stopped
    // counting.
    assert_eq!(log.active.len(), 1);
}

#[test]
fn an_any_of_these_quest_completes_on_one_objective() {
    // `all_objectives: false` is rendered by the gump either way (cliloc
    // 1072209, "Only one of the following"), so getting it wrong would make the
    // window lie.
    let now = Instant::now();
    let mut world = super::tests::world();
    let connection = enter(&mut world, now);
    let either = QuestDef {
        key: "either".to_owned(),
        title: "One or the Other".to_owned(),
        all_objectives: false,
        objectives: vec![
            ObjectiveDef {
                kind: ObjectiveKind::Slay {
                    body: openshard_protocol::wire::Graphic(RAT),
                },
                count: 1,
                name: "rat".to_owned(),
                seconds: 0,
            },
            ObjectiveDef {
                kind: ObjectiveKind::Obtain {
                    graphic: openshard_protocol::wire::Graphic(SILK),
                },
                count: 5,
                name: "silk".to_owned(),
                seconds: 0,
            },
        ],
        ..QuestDef::default()
    };
    register(&mut world, vec![either]);
    let giver = place_giver(&mut world, &["either"], now);
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 4);
    world.tick(now);

    let player = world.state.players[&connection];
    let killer = world.state.registry.serial_of(player).unwrap();
    world.state.bus.send(openshard_combat::MobileDied {
        entity: player,
        serial: killer,
        body: openshard_protocol::wire::Graphic(RAT),
        killer: Some(killer),
    });
    world.tick(now);

    // The rat alone is enough; the silk was never touched.
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(giver.raw())),
    });
    world.tick(now);
    press(&mut world, connection, QUEST_GUMP, 8); // Complete
    world.tick(now);
    assert!(
        log_of(&world, connection).active.is_empty(),
        "one of the two was the whole of it"
    );
}
