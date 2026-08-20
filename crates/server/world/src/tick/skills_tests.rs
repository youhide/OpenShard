//! The skills the window's button runs: the ones that ask about something (Arms
//! Lore, Item Identification, Forensic Evaluation) and the ones a mobile turns on
//! itself (Meditation, Spirit Speak).
//!
//! A child module rather than more of `tests.rs`, which is long past the size a
//! file should be. These go through the whole path a player does — press the
//! button, get a cursor, click a thing, read the line that comes back — because
//! every one of them has been wrong at a different link in that chain before:
//! a cliloc block chosen by the wrong arithmetic reads as a plausible sentence
//! about the wrong object, which no client will report.

use super::tests::{START, enter, enter_as, packets_for, spawn_mobile_at, world};
use super::*;
use openshard_protocol::containers::GridSlot;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::serial::RawSerial;
use openshard_protocol::wire::{Graphic, Hue, RawSkillId};
use openshard_protocol::world::Aggression;
use openshard_skills::DEFAULT_SKILL_DELAY_TICKS;
use openshard_state::Skill;
use openshard_state::components::{
    Contained, Corpse, CorpseBody, Drawn, Equipped, HearsGhosts, Hidden, Hitpoints, Mana, Meditating, Name,
    POISON_POTION_GRAPHIC, PoisonCharges, Poisoned, Stealthing,
};

/// Give the player a skill outright, so a roll is a sure thing.
fn train(world: &mut World, connection: ConnectionId, skill: Skill, value: u16) {
    let entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(entity).unwrap();
    world.queue(Command::SetSkill {
        serial,
        skill: skill.id(),
        value,
    });
}

/// Put an item on the ground next to the player and return its serial.
fn item_beside(world: &mut World, graphic: Graphic, now: Instant) -> Serial {
    world.queue(Command::SpawnItem {
        graphic,
        hue: Hue(0),
        amount: 1,
        stackable: false,
        position: Point::new(START.0 + 1, START.1, 0),
        facet: Facet(0),
    });
    world.tick(now);
    let (entity, _) = world
        .state
        .registry
        .query::<Drawn>()
        .find(|(entity, g)| g.id == graphic && !world.state.registry.has::<Body>(*entity))
        .expect("the item was spawned");
    world.state.registry.serial_of(entity).unwrap()
}

/// Press a skill's button, answer its cursor with `target`, and return every
/// cliloc the player was sent by the answer.
fn use_skill_on(
    world: &mut World,
    connection: ConnectionId,
    skill: Skill,
    target: Serial,
    now: Instant,
) -> Vec<u32> {
    let cursor_id = {
        let entity = world.state.players[&connection];
        world.state.registry.serial_of(entity).unwrap()
    };
    world.queue(Command::UseSkillButton {
        connection,
        skill: RawSkillId(skill.id()),
    });
    world.tick(now);
    let _ = packets_for(world, connection);
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(cursor_id.raw()),
            object: Some(target),
            location: Point::new(START.0 + 1, START.1, 0),
            graphic: None,
            cancelled: false,
        },
    });
    world.tick(now);
    clilocs(world, connection)
}

/// Every cliloc number the connection was sent this tick, in order.
fn clilocs(world: &mut World, connection: ConnectionId) -> Vec<u32> {
    packets_for(world, connection)
        .into_iter()
        .filter(|p| p[0] == 0xC1)
        .map(|p| u32::from_be_bytes([p[14], p[15], p[16], p[17]]))
        .collect()
}

#[test]
fn arms_lore_reads_a_weapon_off_the_core_table() {
    // A katana: `BaseSword`, so slashing, and one-handed — the block at 1038220,
    // no hand offset. Pre-AoS damage 5..26 averages 15, which is band 3, three
    // strides of nine along the block. Getting the base or the stride wrong shows
    // a sentence about a different weapon entirely, which is why the whole number
    // is pinned rather than a range.
    let now = Instant::now();
    let mut world = world();
    let looker = enter(&mut world, now);
    train(&mut world, looker, Skill::ArmsLore, 1000);
    world.tick(now);
    let katana = item_beside(&mut world, openshard_protocol::wire::Graphic(0x13FF), now);
    let _ = packets_for(&mut world, looker);

    let said = use_skill_on(&mut world, looker, Skill::ArmsLore, katana, now);
    assert!(
        said.contains(&(1_038_220 + 3 * 9)),
        "the slashing block, band 3: {said:?}"
    );
}

#[test]
fn arms_lore_knows_a_two_handed_weapon_from_a_one_handed_one() {
    // The hand comes from tiledata's quality byte, which a test world has no
    // client files for — so both read one-handed here, and the six classes that
    // *insist* in code are the ones that can differ without a client. A bow is
    // one of them, and its own block carries no hand offset at all, so this
    // asserts the two facts that do not depend on a file being present: a katana
    // lands on the one-handed slashing line, and a bow lands on the ranged block.
    let now = Instant::now();
    let mut world = world();
    let looker = enter(&mut world, now);
    train(&mut world, looker, Skill::ArmsLore, 1000);
    world.tick(now);
    let bow = item_beside(&mut world, openshard_protocol::wire::Graphic(0x13B2), now);
    let _ = packets_for(&mut world, looker);

    // Bow: pre-AoS 9..41 averages 25, band 5.
    let said = use_skill_on(&mut world, looker, Skill::ArmsLore, bow, now);
    assert!(
        said.contains(&(1_038_224 + 5 * 9)),
        "the ranged block, band 5, no hand offset: {said:?}"
    );
}

#[test]
fn arms_lore_reads_armour_by_its_rating() {
    // A plate chest rates 40, capped at 35, which is band 7 — the top line,
    // "superbly crafted to provide maximum protection".
    let now = Instant::now();
    let mut world = world();
    let looker = enter(&mut world, now);
    train(&mut world, looker, Skill::ArmsLore, 1000);
    world.tick(now);
    let plate = item_beside(&mut world, openshard_protocol::wire::Graphic(0x1415), now);
    let _ = packets_for(&mut world, looker);

    let said = use_skill_on(&mut world, looker, Skill::ArmsLore, plate, now);
    assert!(said.contains(&(1_038_295 + 7)), "the top armour line: {said:?}");
}

#[test]
fn arms_lore_refuses_something_that_is_neither() {
    // 500352, "This is neither weapon nor armor." A gold coin is in no table, and
    // the honest answer is the client's own line for that rather than silence.
    let now = Instant::now();
    let mut world = world();
    let looker = enter(&mut world, now);
    train(&mut world, looker, Skill::ArmsLore, 1000);
    world.tick(now);
    let gold = item_beside(&mut world, openshard_protocol::wire::Graphic(0x0EED), now);
    let _ = packets_for(&mut world, looker);

    let said = use_skill_on(&mut world, looker, Skill::ArmsLore, gold, now);
    assert!(said.contains(&500_352), "neither weapon nor armour: {said:?}");
}

#[test]
fn item_identification_names_the_thing_and_prices_it_if_it_has_one() {
    // "It appears to be:" then the name, drawn over the item itself. The value
    // line follows only for an item that carries a price, because the core knows
    // what a shopkeeper charges and nothing else — a guessed number for a rock
    // would read as authoritative.
    let now = Instant::now();
    let mut world = world();
    let looker = enter(&mut world, now);
    train(&mut world, looker, Skill::ItemId, 1000);
    world.tick(now);
    let scroll = item_beside(&mut world, openshard_protocol::wire::Graphic(0x1F2D), now);
    let entity = world.state.registry.entity_of(scroll).unwrap();
    world
        .state
        .registry
        .insert(entity, Name("a scroll of magic arrow".to_owned()));
    world
        .state
        .registry
        .insert(entity, openshard_state::components::Price(12));
    let _ = packets_for(&mut world, looker);

    let said = use_skill_on(&mut world, looker, Skill::ItemId, scroll, now);
    assert!(said.contains(&1_041_349), "it appears to be: {said:?}");
    assert!(said.contains(&1_041_351), "and it is worth: {said:?}");
}

#[test]
fn forensics_says_who_killed_a_body_and_who_has_been_through_it() {
    // Everything Forensic Evaluation reads was written by somebody else's rule at
    // the moment it happened. This is the whole chain: a mobile dies to a named
    // killer, the reap lays a corpse that remembers both, and the skill reads it
    // back — including "not desecrated", which is a different sentence from a
    // failed roll and is the one a fresh body deserves.
    let now = Instant::now();
    let mut world = world();
    let looker = enter(&mut world, now);
    let player_serial = {
        let entity = world.state.players[&looker];
        world.state.registry.serial_of(entity).unwrap()
    };
    train(&mut world, looker, Skill::Forensics, 1000);
    world.tick(now);

    let victim = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 10, now);
    world.queue(Command::Damage {
        serial: victim,
        amount: 100,
        damage_type: 0,
        by: Some(player_serial),
    });
    world.tick(now);
    // The corpse the reap laid, remembering the player as its killer.
    let (corpse, _) = world
        .state
        .registry
        .query::<Corpse>()
        .next()
        .expect("a corpse was laid");
    let story = world.state.registry.get::<Corpse>(corpse).unwrap();
    assert_eq!(
        story.killer.as_deref(),
        Some("Lord British"),
        "the killer is remembered by name, not by serial"
    );
    let corpse_serial = world.state.registry.serial_of(corpse).unwrap();
    let _ = packets_for(&mut world, looker);

    let said = use_skill_on(&mut world, looker, Skill::Forensics, corpse_serial, now);
    // A human body (the test creature wears 0x0190) reports its killer.
    assert!(said.contains(&1_042_751), "killed by ~1_KILLER_NAME~: {said:?}");
    assert!(said.contains(&501_002), "and not yet desecrated: {said:?}");
    // The first reader signs the body, so a second one is told whose work it is.
    assert_eq!(
        world
            .state
            .registry
            .get::<Corpse>(corpse)
            .unwrap()
            .examined_by
            .as_deref(),
        Some("Lord British")
    );
    // The button holds for a second after a use, so wait it out rather than
    // stripping the cooldown: the refusal is the rule, not an obstacle.
    for _ in 0..=DEFAULT_SKILL_DELAY_TICKS {
        world.tick(now);
    }
    let _ = packets_for(&mut world, looker);
    let again = use_skill_on(&mut world, looker, Skill::Forensics, corpse_serial, now);
    assert!(
        again.contains(&1_042_750),
        "the forensicist has already discovered that: {again:?}"
    );
}

#[test]
fn taking_something_off_a_corpse_makes_you_a_looter() {
    // The other half of the record Forensics reads, written where the lifting
    // happens: a corpse keeps a guest list, an ordinary chest does not.
    let now = Instant::now();
    let mut world = world();
    let looter = enter(&mut world, now);
    let player_serial = {
        let entity = world.state.players[&looter];
        world.state.registry.serial_of(entity).unwrap()
    };
    let victim = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 10, now);
    world.queue(Command::Damage {
        serial: victim,
        amount: 100,
        damage_type: 0,
        by: Some(player_serial),
    });
    world.tick(now);
    let (corpse, _) = world
        .state
        .registry
        .query::<Corpse>()
        .next()
        .expect("a corpse was laid");
    // The core drops a little gold in every creature corpse; lift it.
    let corpse_serial = world.state.registry.serial_of(corpse).unwrap();
    let (loot, _) = world
        .state
        .registry
        .query::<Contained>()
        .find(|(_, c)| c.container == corpse_serial)
        .expect("a corpse holds the baseline gold");
    let loot_serial = world.state.registry.serial_of(loot).unwrap();

    world.queue(Command::PickUpItem {
        connection: looter,
        serial: RawSerial(loot_serial.raw()),
        amount: 1,
    });
    world.tick(now);
    assert_eq!(
        world.state.registry.get::<Corpse>(corpse).unwrap().looters,
        vec!["Lord British".to_owned()],
        "the corpse remembers who went through it"
    );
}

#[test]
fn a_corpses_story_comes_back_after_a_restart() {
    // A corpse lies for seven minutes and a shard restarts inside that window, so
    // the story rides the item's saved record (schema v17). Without it the body a
    // player was investigating comes back anonymous, killed by nobody.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let player_serial = {
        let entity = world.state.players[&player];
        world.state.registry.serial_of(entity).unwrap()
    };
    let victim = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 10, now);
    world.queue(Command::Damage {
        serial: victim,
        amount: 100,
        damage_type: 0,
        by: Some(player_serial),
    });
    world.tick(now);
    world.take_snapshot();
    let snapshot = world.drain_saves().next().expect("a snapshot was taken");
    let saved = snapshot
        .ground
        .as_ref()
        .expect("the ground was swept")
        .iter()
        .find(|item| item.corpse.is_some())
        .expect("the corpse was swept into the save")
        .clone();
    assert_eq!(
        saved.corpse.as_ref().unwrap().killer.as_deref(),
        Some("Lord British")
    );

    // A fresh world restoring that save has the same body with the same story.
    let mut reborn = super::tests::world();
    // A corpse lies on the ground and owns nobody's serial, so the characters'
    // restore has nothing to bring back — it still runs first, which is what the
    // token it hands to `restore_items` is for.
    let characters = reborn.restore_characters(Vec::new());
    reborn.restore_items(vec![saved], &characters);
    let (corpse, _) = reborn
        .state
        .registry
        .query::<Corpse>()
        .next()
        .expect("the corpse came back");
    assert_eq!(
        reborn
            .state
            .registry
            .get::<Corpse>(corpse)
            .unwrap()
            .killer
            .as_deref(),
        Some("Lord British")
    );
    assert!(
        reborn.state.registry.get::<CorpseBody>(corpse).is_some(),
        "and still draws as the body it was"
    );
    assert_eq!(
        reborn.state.world_item(corpse).unwrap().payload,
        openshard_protocol::items::WorldItemPayload::Corpse {
            body: Graphic(0x0190),
            // The way it fell came back with the story it is saved beside.
            facing: openshard_protocol::direction::Direction::South,
        },
        "the restored corpse still carries a body, not a stack amount"
    );
}

#[test]
fn meditation_enters_a_trance_and_a_step_breaks_it() {
    // The whole shape of the skill: a trance is one marker and no timer, and what
    // ends it is somebody doing something. ServUO's `DisruptiveAction` is called
    // from the move, the blow, the word and the lift; this asserts the first,
    // because a trance you can walk out of and keep is the bug that shape prevents.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    train(&mut world, player, Skill::Meditation, 1000);
    world.tick(now);
    // Spend some mana, or the answer is "you are at peace".
    world.state.registry.insert(entity, Mana { current: 1, max: 50 });

    world.queue(Command::UseSkillButton {
        connection: player,
        skill: RawSkillId(Skill::Meditation.id()),
    });
    world.tick(now);
    assert!(
        world.state.registry.has::<Meditating>(entity),
        "a grandmaster with an empty pool always focuses: {:?}",
        clilocs(&mut world, player)
    );

    world.queue(Command::Step {
        serial: world.state.registry.serial_of(entity).unwrap(),
        direction: Direction::North.to_bits(),
    });
    world.tick(now);
    // The first Step turns; the second moves, and the move is what breaks it.
    world.queue(Command::Step {
        serial: world.state.registry.serial_of(entity).unwrap(),
        direction: Direction::North.to_bits(),
    });
    world.tick(now);
    assert!(
        !world.state.registry.has::<Meditating>(entity),
        "walking out of a trance ends it"
    );
    assert!(
        clilocs(&mut world, player).contains(&500_134),
        "and says so: you stop meditating"
    );
}

#[test]
fn meditation_wants_free_hands() {
    // 502626, and the reason Meditation is a mage's skill: a shield or a sword in
    // hand refuses outright, a spellbook does not.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let owner = world.state.registry.serial_of(entity).unwrap();
    train(&mut world, player, Skill::Meditation, 1000);
    world.tick(now);
    world.state.registry.insert(entity, Mana { current: 1, max: 50 });

    // A katana on the one-handed layer.
    let (sword, _) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        sword,
        Drawn {
            id: openshard_protocol::wire::Graphic(0x13FF),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    world.state.registry.insert(
        sword,
        Equipped {
            mobile: owner,
            layer: Layer(1),
        },
    );
    let _ = packets_for(&mut world, player);

    world.queue(Command::UseSkillButton {
        connection: player,
        skill: RawSkillId(Skill::Meditation.id()),
    });
    world.tick(now);
    assert!(!world.state.registry.has::<Meditating>(entity));
    assert!(
        clilocs(&mut world, player).contains(&502_626),
        "your hands must be free"
    );
}

#[test]
fn a_trance_doubles_the_rate_mana_comes_back_at() {
    // The rate is ServUO's pre-AoS curve over Intelligence and Meditation, halved
    // while meditating — a read-site derivation, so the trance is not folded into
    // anything and taking it away needs no undoing.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    train(&mut world, player, Skill::Meditation, 1000);
    world.tick(now);

    let awake = openshard_magic::mana_regen_ticks(&world.state, entity);
    world.state.registry.insert(entity, Meditating);
    let entranced = openshard_magic::mana_regen_ticks(&world.state, entity);
    assert_eq!(entranced * 2, awake, "a trance halves the interval");
    // And a plate chest puts it back where an untrained character started: the
    // armour offset is added in seconds, which is what makes the skill's armour
    // rule bite rather than merely nag.
    let owner = world.state.registry.serial_of(entity).unwrap();
    let (plate, _) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        plate,
        Drawn {
            id: openshard_protocol::wire::Graphic(0x1415),
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    world.state.registry.insert(
        plate,
        Equipped {
            mobile: owner,
            layer: Layer(0x0D),
        },
    );
    assert!(
        openshard_magic::mana_regen_ticks(&world.state, entity) > entranced,
        "a mage in plate regenerates like a warrior"
    );
}

#[test]
fn spirit_speak_lets_the_living_hear_the_dead_without_seeing_them() {
    // The two questions are two predicates on purpose. A ghost stays invisible to
    // a listener under Spirit Speak — if `can_see_mobile` had been relaxed to
    // cover this, contacting the netherworld would make the dead walk visibly
    // among the living.
    let now = Instant::now();
    let mut world = world();
    let living = enter(&mut world, now);
    let listener = world.state.players[&living];
    train(&mut world, living, Skill::SpiritSpeak, 1000);
    world.tick(now);

    let dead = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let ghost = world.state.players[&dead];
    world.state.registry.insert(
        ghost,
        openshard_state::components::Ghost {
            body: Body {
                id: Graphic(0x0190),
                hue: openshard_protocol::wire::Hue(0),
            },
        },
    );

    assert!(!world.state.can_hear_mobile(listener, ghost), "not yet");
    world.queue(Command::UseSkillButton {
        connection: living,
        skill: RawSkillId(Skill::SpiritSpeak.id()),
    });
    world.tick(now);
    assert!(
        world.state.registry.has::<HearsGhosts>(listener),
        "a grandmaster contacts the netherworld: {:?}",
        clilocs(&mut world, living)
    );
    assert!(world.state.can_hear_mobile(listener, ghost), "and hears it");
    assert!(
        !world.state.can_see_mobile(listener, ghost),
        "but still cannot see it"
    );

    // And the contact lapses on the tick counter, with the client told.
    let until = world.state.registry.get::<HearsGhosts>(listener).unwrap().until;
    while world.state.ticks <= until {
        world.tick(now);
    }
    assert!(!world.state.registry.has::<HearsGhosts>(listener));
    assert!(
        clilocs(&mut world, living).contains(&502_445),
        "your contact with the netherworld fades"
    );
}

/// Answer a *second* cursor — the one Poisoning raises after the potion.
fn answer_cursor(world: &mut World, connection: ConnectionId, target: Serial, now: Instant) -> Vec<u32> {
    let cursor_id = {
        let entity = world.state.players[&connection];
        world.state.registry.serial_of(entity).unwrap()
    };
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(cursor_id.raw()),
            object: Some(target),
            location: Point::new(START.0 + 1, START.1, 0),
            graphic: None,
            cancelled: false,
        },
    });
    world.tick(now);
    clilocs(world, connection)
}

#[test]
fn poisoning_coats_a_blade_and_the_blade_spends_its_doses() {
    // The whole chain, because every link has somewhere to go wrong: two cursors,
    // a potion that is consumed either way, `18 - level*2` doses on the blade, and
    // a landed blow that spends one into whatever it cut.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let owner = world.state.registry.serial_of(entity).unwrap();
    train(&mut world, player, Skill::Poisoning, 1000);
    world.tick(now);

    // A bottle of greater poison (level 2) and a katana, both beside the player.
    let potion = item_beside(&mut world, POISON_POTION_GRAPHIC, now);
    let potion_entity = world.state.registry.entity_of(potion).unwrap();
    world.state.registry.insert(
        potion_entity,
        PoisonCharges {
            level: openshard_protocol::world::PoisonLevel::new(2),
            charges: 1,
        },
    );
    let katana = item_beside(&mut world, openshard_protocol::wire::Graphic(0x13FF), now);
    let blade = world.state.registry.entity_of(katana).unwrap();
    let _ = packets_for(&mut world, player);

    // Press the button, pick the potion, then pick the blade.
    let asked = use_skill_on(&mut world, player, Skill::Poisoning, potion, now);
    assert!(
        asked.contains(&502_142),
        "to what do you wish to apply the poison: {asked:?}"
    );
    let applied = answer_cursor(&mut world, player, katana, now);
    assert!(applied.contains(&1_010_517), "you apply the poison: {applied:?}");
    assert_eq!(
        world.state.registry.get::<PoisonCharges>(blade).copied(),
        Some(PoisonCharges {
            level: openshard_protocol::world::PoisonLevel::new(2),
            charges: 14
        }),
        "18 - level*2 doses"
    );
    // The bottle is spent either way, and what is left is an empty one.
    assert!(!world.state.registry.has::<PoisonCharges>(potion_entity));
    assert_eq!(
        world.state.registry.get::<Drawn>(potion_entity).unwrap().id,
        openshard_state::components::EMPTY_BOTTLE_GRAPHIC
    );

    // Wield it and hit something: the target is poisoned and a dose is gone.
    world.state.registry.insert(
        blade,
        Equipped {
            mobile: owner,
            layer: Layer(1),
        },
    );
    let victim = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 200, now);
    world.queue(Command::WarMode {
        connection: player,
        war: true,
    });
    world.queue(Command::Attack {
        connection: player,
        target: Some(victim),
    });
    // Swing until one lands — the to-hit roll can miss, and a miss spends nothing.
    let victim_entity = world.state.registry.entity_of(victim).unwrap();
    for _ in 0..400 {
        world.tick(now);
        if world.state.registry.has::<Poisoned>(victim_entity) {
            break;
        }
    }
    assert!(
        world.state.registry.has::<Poisoned>(victim_entity),
        "a coated blade poisons what it cuts"
    );
    let left = world
        .state
        .registry
        .get::<PoisonCharges>(blade)
        .map_or(0, |p| p.charges);
    assert!(left < 14, "and spends doses doing it: {left} left");
}

#[test]
fn poison_only_goes_on_a_bladed_or_piercing_weapon() {
    // 502145 — pre-AoS you may coat a blade, not a mace.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    train(&mut world, player, Skill::Poisoning, 1000);
    world.tick(now);
    let potion = item_beside(&mut world, POISON_POTION_GRAPHIC, now);
    let potion_entity = world.state.registry.entity_of(potion).unwrap();
    world.state.registry.insert(
        potion_entity,
        PoisonCharges {
            level: openshard_protocol::world::PoisonLevel::new(0),
            charges: 1,
        },
    );
    let mace = item_beside(&mut world, openshard_protocol::wire::Graphic(0x0F5C), now);
    let _ = packets_for(&mut world, player);

    let _ = use_skill_on(&mut world, player, Skill::Poisoning, potion, now);
    let refused = answer_cursor(&mut world, player, mace, now);
    assert!(refused.contains(&502_145), "you cannot poison that: {refused:?}");
}

#[test]
fn taste_identification_finds_the_poison_on_a_blade() {
    // The other end of the same fact. A clean blade gets a different line, which is
    // the point — a taster who always says "poisoned" is no taster.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    train(&mut world, player, Skill::TasteId, 1000);
    world.tick(now);
    let clean = item_beside(&mut world, openshard_protocol::wire::Graphic(0x13FF), now);
    let _ = packets_for(&mut world, player);
    let said = use_skill_on(&mut world, player, Skill::TasteId, clean, now);
    assert!(said.contains(&1_010_600), "nothing unusual: {said:?}");

    let blade = world.state.registry.entity_of(clean).unwrap();
    world.state.registry.insert(
        blade,
        PoisonCharges {
            level: openshard_protocol::world::PoisonLevel::new(3),
            charges: 12,
        },
    );
    for _ in 0..=DEFAULT_SKILL_DELAY_TICKS {
        world.tick(now);
    }
    let _ = packets_for(&mut world, player);
    let said = use_skill_on(&mut world, player, Skill::TasteId, clean, now);
    assert!(said.contains(&1_038_284), "poison smeared on it: {said:?}");
}

#[test]
fn begging_takes_coin_from_a_townsperson_and_karma_from_the_beggar() {
    // The trade: a handful of gold in the pack, and up to forty karma gone. Both
    // halves matter — a beggar who lost nothing would be the best gold faucet in
    // the game, and ServUO's floor of −3000 is what stops the loss running away.
    let now = Instant::now();
    let mut world = world();
    let beggar = enter(&mut world, now);
    let entity = world.state.players[&beggar];
    train(&mut world, beggar, Skill::Begging, 1000);
    world.tick(now);
    let townsperson = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 50, now);
    let _ = packets_for(&mut world, beggar);

    let said = use_skill_on(&mut world, beggar, Skill::Begging, townsperson, now);
    assert!(said.contains(&500_405), "I feel sorry for thee: {said:?}");
    world.tick(now); // the payout is applied on the next pass of the tick
    let karma = world
        .state
        .registry
        .get::<openshard_state::components::Karma>(entity)
        .map_or(0, |k| k.0);
    assert!(karma < 0, "begging costs karma: {karma}");
    let serial = world
        .state
        .registry
        .serial_of(entity)
        .expect("the beggar has a serial");
    let backpack = openshard_items::backpack_of(&world.state, serial).expect("a backpack");
    let gold = openshard_items::count_in_container(&world.state, backpack, openshard_items::GOLD_GRAPHIC);
    assert!(
        (10..=14).contains(&gold),
        "a successful beg puts its handout in the backpack: {gold}"
    );
}

#[test]
fn a_trapped_chest_goes_off_when_it_is_opened_and_remove_trap_takes_it_off() {
    // Both halves of a trap. Without the trigger it is decoration; without the
    // disarm it is a wall — and ServUO is explicit that a *failed* disarm does not
    // set it off, which is what makes the skill worth trying at all.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    let hits_before = world
        .state
        .registry
        .get::<Hitpoints>(entity)
        .map_or(0, |h| h.current);

    world.queue(Command::SpawnContainer {
        graphic: openshard_protocol::wire::Graphic(0x0E3C),
        gump: openshard_protocol::wire::Graphic(0x003C),
        hue: openshard_protocol::wire::Hue(0),
        position: Point::new(START.0 + 1, START.1, 0),
        facet: Facet(0),
    });
    world.tick(now);
    let (chest, _) = world
        .state
        .registry
        .query::<Container>()
        .find(|(e, _)| !world.state.registry.has::<Equipped>(*e))
        .expect("a chest on the ground");
    world.state.registry.insert(
        chest,
        openshard_state::components::Trap {
            kind: openshard_state::components::TrapKind::Dart,
            power: 20,
            level: 0,
        },
    );
    let chest_serial = world.state.registry.serial_of(chest).unwrap();
    let _ = packets_for(&mut world, player);

    world.queue(Command::DoubleClick {
        connection: player,
        request: UseRequest::Use(RawSerial(chest_serial.raw())),
    });
    world.tick(now);
    let hurt = world
        .state
        .registry
        .get::<Hitpoints>(entity)
        .map_or(0, |h| h.current);
    assert!(hurt < hits_before, "the dart found flesh: {hurt}");
    assert!(
        !world
            .state
            .registry
            .has::<openshard_state::components::Trap>(chest),
        "and a sprung trap is spent"
    );

    // Now the disarm, on a fresh trap: the two prerequisite skills, then the roll.
    world.state.registry.insert(
        chest,
        openshard_state::components::Trap {
            kind: openshard_state::components::TrapKind::Dart,
            power: 0,
            level: 0,
        },
    );
    train(&mut world, player, Skill::Lockpicking, 1000);
    train(&mut world, player, Skill::DetectHidden, 1000);
    train(&mut world, player, Skill::RemoveTrap, 1000);
    world.tick(now);
    let _ = packets_for(&mut world, player);
    let said = use_skill_on(&mut world, player, Skill::RemoveTrap, chest_serial, now);
    assert!(said.contains(&502_377), "rendered harmless: {said:?}");
    assert!(
        !world
            .state
            .registry
            .has::<openshard_state::components::Trap>(chest)
    );
}

#[test]
fn remove_trap_refuses_before_it_raises_a_cursor() {
    // ServUO checks Lockpicking and Detect Hidden in `OnUse`, so somebody who could
    // not disarm anything never gets a target at all — a refusal with the client's
    // own line, not a cursor that goes nowhere.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    world.queue(Command::UseSkillButton {
        connection: player,
        skill: RawSkillId(Skill::RemoveTrap.id()),
    });
    world.tick(now);
    assert!(
        !world.state.has_target(entity),
        "no cursor for someone who knows nothing about locks"
    );
    assert!(clilocs(&mut world, player).contains(&502_366));
}

#[test]
fn hiding_takes_you_off_every_screen_and_a_word_puts_you_back() {
    // The whole subsystem in one test, because the point of it is that the state
    // lives in *one* gate and *one* break: hiding tells every watcher to forget
    // you, `can_see_mobile` keeps them from being told again, and speaking — which
    // knows nothing about hiding — gives you away through `break_cover`.
    let now = Instant::now();
    let mut world = world();
    let hider = enter(&mut world, now);
    let entity = world.state.players[&hider];
    let watcher = enter_as(&mut world, ConnectionId::from_raw(2), now);
    let onlooker = world.state.players[&watcher];
    train(&mut world, hider, Skill::Hiding, 1000);
    world.tick(now);
    assert!(world.state.can_see_mobile(onlooker, entity), "seen to start");
    let _ = packets_for(&mut world, watcher);

    world.queue(Command::UseSkillButton {
        connection: hider,
        skill: RawSkillId(Skill::Hiding.id()),
    });
    world.tick(now);
    assert!(
        world.state.registry.has::<Hidden>(entity),
        "a grandmaster hides: {:?}",
        clilocs(&mut world, hider)
    );
    assert!(!world.state.can_see_mobile(onlooker, entity));
    let serial = world.state.registry.serial_of(entity).unwrap();
    assert!(
        packets_for(&mut world, watcher)
            .iter()
            .any(|p| p[0] == 0x1D && u32::from_be_bytes([p[1], p[2], p[3], p[4]]) == serial.raw()),
        "and the watcher is told to forget them"
    );

    world.queue(Command::Say {
        connection: hider,
        mode: RawTalkMode(0),
        hue: RawHue(0),
        font: RawFont(3),
        text: "here I am".to_owned(),
    });
    world.tick(now);
    assert!(
        !world.state.registry.has::<Hidden>(entity),
        "speaking gives you away"
    );
    assert!(world.state.can_see_mobile(onlooker, entity));
}

#[test]
fn stealth_buys_a_few_quiet_steps_and_no_more() {
    // The budget is the skill: `value / 10` steps pre-AoS, spent by the movement
    // path itself, so nothing about walking has to know what Stealth is.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let entity = world.state.players[&player];
    train(&mut world, player, Skill::Hiding, 1000);
    train(&mut world, player, Skill::Stealth, 1000);
    world.tick(now);
    world.state.registry.insert(entity, Hidden);

    world.queue(Command::UseSkillButton {
        connection: player,
        skill: RawSkillId(Skill::Stealth.id()),
    });
    world.tick(now);
    assert_eq!(
        world
            .state
            .registry
            .get::<Stealthing>(entity)
            .map(|s| s.steps_left),
        Some(10),
        "a grandmaster gets ten quiet steps: {:?}",
        clilocs(&mut world, player)
    );

    // Walk the budget down to its last step, then take two more: the first spends
    // it, the second is past it.
    world.state.registry.insert(entity, Stealthing { steps_left: 1 });
    let serial = world.state.registry.serial_of(entity).unwrap();
    for _ in 0..3 {
        world.queue(Command::Step {
            serial,
            direction: Direction::North.to_bits(),
        });
        world.tick(now);
    }
    assert!(
        !world.state.registry.has::<Hidden>(entity),
        "a step past the budget gives them away"
    );
}

#[test]
fn detecting_hidden_strips_a_worse_hider_and_not_a_better_one() {
    // The contest is `detect / 1.5` against the hider's Hiding, so a searcher does
    // not simply out-roll everybody by having the skill at all.
    let now = Instant::now();
    let mut world = world();
    let seeker = enter(&mut world, now);
    train(&mut world, seeker, Skill::DetectHidden, 1000);
    world.tick(now);
    let novice = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 50, now);
    let novice_entity = world.state.registry.entity_of(novice).unwrap();
    world.state.registry.insert(novice_entity, Hidden);

    world.queue(Command::UseSkillButton {
        connection: seeker,
        skill: RawSkillId(Skill::DetectHidden.id()),
    });
    world.tick(now);
    assert!(
        !world.state.registry.has::<Hidden>(novice_entity),
        "an untrained hider standing next to a grandmaster is found"
    );
}

/// Put an instrument in the player's backpack and return its entity.
fn give_instrument(world: &mut World, connection: ConnectionId, graphic: Graphic) -> EntityId {
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let backpack = openshard_items::backpack_of(&world.state, owner).expect("a backpack");
    let (item, _) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        item,
        Drawn {
            id: graphic,
            hue: Hue(0),
        },
    );
    world.state.registry.insert(
        item,
        Contained {
            container: backpack,
            position: GumpPoint::new(20, 20),
            grid: GridSlot(0),
        },
    );
    world
        .state
        .registry
        .insert(item, openshard_state::components::Instrument { uses_left: 10 });
    item
}

#[test]
fn peacemaking_calms_a_creature_and_it_stops_swinging() {
    // The bard shape end to end: an instrument in the pack, a Musicianship check
    // *before* the skill's own roll, a use spent, and a `Pacified` the fight reads
    // where it would swing rather than having anything folded into it.
    let now = Instant::now();
    let mut world = world();
    let bard = enter(&mut world, now);
    train(&mut world, bard, Skill::Peacemaking, 1000);
    train(&mut world, bard, Skill::Musicianship, 1000);
    world.tick(now);
    let lute = give_instrument(&mut world, bard, Graphic(0x0EB3));
    let creature = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 20, now);
    let creature_entity = world.state.registry.entity_of(creature).unwrap();
    let _ = packets_for(&mut world, bard);

    let said = use_skill_on(&mut world, bard, Skill::Peacemaking, creature, now);
    assert!(
        world
            .state
            .registry
            .has::<openshard_state::components::Pacified>(creature_entity),
        "a grandmaster calms a rat: {said:?}"
    );
    let uses = world
        .state
        .registry
        .get::<openshard_state::components::Instrument>(lute)
        .map(|i| i.uses_left);
    assert_eq!(uses, Some(9), "and it cost a tune");
}

#[test]
fn a_bard_with_no_instrument_gets_no_cursor() {
    // ServUO refuses in `OnUse`, before any target goes up: the instrument is the
    // whole precondition of the family.
    let now = Instant::now();
    let mut world = world();
    let bard = enter(&mut world, now);
    let entity = world.state.players[&bard];
    train(&mut world, bard, Skill::Provocation, 1000);
    world.tick(now);
    world.queue(Command::UseSkillButton {
        connection: bard,
        skill: RawSkillId(Skill::Provocation.id()),
    });
    world.tick(now);
    assert!(!world.state.has_target(entity));
    assert!(clilocs(&mut world, bard).contains(&500_617));
}

#[test]
fn discordance_makes_a_creature_worse_at_everything_at_once() {
    // The penalty is read in `skill_value` — the one question every other system
    // asks about how good somebody is — so it reaches the to-hit roll, the damage
    // scaling and a spell's casting roll without any of them knowing what a lute
    // is. This asserts the seam, not three separate consumers.
    let now = Instant::now();
    let mut world = world();
    let bard = enter(&mut world, now);
    train(&mut world, bard, Skill::Discordance, 1000);
    train(&mut world, bard, Skill::Musicianship, 1000);
    world.tick(now);
    give_instrument(&mut world, bard, Graphic(0x0EB3));
    let creature = spawn_mobile_at(&mut world, Point::new(START.0 + 1, START.1, 0), 20, now);
    let entity = world.state.registry.entity_of(creature).unwrap();
    world.queue(Command::SetSkill {
        serial: creature,
        skill: Skill::Wrestling.id(),
        value: 1000,
    });
    world.tick(now);
    let before = openshard_skills::skill_value(&world.state, entity, Skill::Wrestling);
    let _ = packets_for(&mut world, bard);

    let said = use_skill_on(&mut world, bard, Skill::Discordance, creature, now);
    assert!(
        world
            .state
            .registry
            .has::<openshard_state::components::Discorded>(entity),
        "a grandmaster puts a rat out of tune: {said:?}"
    );
    let after = openshard_skills::skill_value(&world.state, entity, Skill::Wrestling);
    assert!(
        after < before,
        "and it is worse at everything: {before} → {after}"
    );
}

/// Put a plain item in the player's backpack and return its serial.
fn give_item(world: &mut World, connection: ConnectionId, graphic: Graphic) -> Serial {
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let backpack = openshard_items::backpack_of(&world.state, owner).expect("a backpack");
    let (item, serial) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        item,
        Drawn {
            id: graphic,
            hue: Hue(0),
        },
    );
    world.state.registry.insert(
        item,
        Contained {
            container: backpack,
            position: GumpPoint::new(20, 20),
            grid: GridSlot(0),
        },
    );
    serial
}

#[test]
fn a_bandage_takes_time_and_then_mends() {
    // The one skill whose *duration* is the mechanic: the bandage is spent when the
    // work begins, the healing lands seconds later on the tick counter, and the
    // patient can be hurt again in between.
    let now = Instant::now();
    let mut world = world();
    let healer = enter(&mut world, now);
    let entity = world.state.players[&healer];
    train(&mut world, healer, Skill::Healing, 1000);
    train(&mut world, healer, Skill::Anatomy, 1000);
    world.tick(now);
    let bandage = give_item(&mut world, healer, openshard_skills::BANDAGE_GRAPHIC);
    // A wound to mend.
    world.state.registry.insert(
        entity,
        Hitpoints {
            current: 20,
            max: 100,
        },
    );
    let _ = packets_for(&mut world, healer);

    // Double-click the bandage, then point it at yourself.
    world.queue(Command::DoubleClick {
        connection: healer,
        request: UseRequest::Use(RawSerial(bandage.raw())),
    });
    world.tick(now);
    assert!(
        world.state.has_target(entity),
        "the bandage asks who it is for: {:?}",
        clilocs(&mut world, healer)
    );
    let self_serial = world.state.registry.serial_of(entity).unwrap();
    let _ = answer_cursor(&mut world, healer, self_serial, now);
    assert!(
        world
            .state
            .registry
            .has::<openshard_state::components::Bandaging>(entity),
        "and the work has begun"
    );
    assert!(
        world.state.registry.entity_of(bandage).is_none(),
        "the bandage is spent at the start, not the end"
    );

    // It is not instant: a hundred dexterity self-heals in about ten seconds.
    for _ in 0..(11 * openshard_state::TICKS_PER_SECOND) {
        world.tick(now);
    }
    let hits = world
        .state
        .registry
        .get::<Hitpoints>(entity)
        .map_or(0, |h| h.current);
    assert!(hits > 20, "the wound closed: {hits}");
}

#[test]
fn a_lockpick_opens_a_lock_it_is_good_enough_for() {
    // And refuses one it is not, which is the point of the two levels on a `Lock`:
    // without them every lock is either free or impossible.
    let now = Instant::now();
    let mut world = world();
    let thief = enter(&mut world, now);
    train(&mut world, thief, Skill::Lockpicking, 1000);
    world.tick(now);
    let pick = give_item(&mut world, thief, openshard_skills::LOCKPICK_GRAPHIC);

    world.queue(Command::SpawnContainer {
        graphic: openshard_protocol::wire::Graphic(0x0E3C),
        gump: openshard_protocol::wire::Graphic(0x003C),
        hue: openshard_protocol::wire::Hue(0),
        position: Point::new(START.0 + 1, START.1, 0),
        facet: Facet(0),
    });
    world.tick(now);
    let (chest, _) = world
        .state
        .registry
        .query::<Container>()
        .find(|(e, _)| !world.state.registry.has::<Equipped>(*e))
        .expect("a chest");
    world.state.registry.insert(
        chest,
        openshard_state::components::Lock {
            key_value: 42,
            required_skill: 0,
            max_skill: 500,
        },
    );
    let chest_serial = world.state.registry.serial_of(chest).unwrap();
    let _ = packets_for(&mut world, thief);

    world.queue(Command::DoubleClick {
        connection: thief,
        request: UseRequest::Use(RawSerial(pick.raw())),
    });
    world.tick(now);
    let said = answer_cursor(&mut world, thief, chest_serial, now);
    assert!(said.contains(&502_076), "the lock yields: {said:?}");
    assert!(
        !world
            .state
            .registry
            .has::<openshard_state::components::Lock>(chest),
        "and is open for good"
    );
}

#[test]
fn taming_makes_a_creature_yours_and_it_follows() {
    // The whole pillar in one path: the skill decides, `npc::tame` makes the pet,
    // the pet's own beat walks it after you through the same `step` a wild creature
    // uses, and the status bar counts it as a follower.
    let now = Instant::now();
    let mut world = world();
    let tamer = enter(&mut world, now);
    let player = world.state.players[&tamer];
    train(&mut world, tamer, Skill::AnimalTaming, 1000);
    world.tick(now);
    // A horse, two tiles off: rideable, so tamable, and inside the cursor's reach.
    let horse = spawn_mobile_body(&mut world, 0x00C8, Point::new(START.0 + 2, START.1, 0), now);
    let entity = world.state.registry.entity_of(horse).unwrap();
    let _ = packets_for(&mut world, tamer);

    let said = use_skill_on(&mut world, tamer, Skill::AnimalTaming, horse, now);
    // The anger roll can turn it instead; retry until one of the two lands, since
    // both are real outcomes and only one makes a pet.
    let mut tries = 0;
    while !world
        .state
        .registry
        .has::<openshard_state::components::Pet>(entity)
        && tries < 40
    {
        for _ in 0..=(40 * openshard_state::TICKS_PER_SECOND) {
            world.tick(now);
        }
        let _ = packets_for(&mut world, tamer);
        let _ = use_skill_on(&mut world, tamer, Skill::AnimalTaming, horse, now);
        tries += 1;
    }
    assert!(
        world
            .state
            .registry
            .has::<openshard_state::components::Pet>(entity),
        "a grandmaster tames a horse eventually: {said:?}"
    );
    assert_eq!(
        openshard_skills::followers_of(&world.state, player),
        1,
        "and it counts against the follower cap"
    );

    // It follows: put it well out of arm's reach and it walks back. Two tiles is
    // *already* close enough (the follow gap), which is why it is moved first.
    let far = Point::new(START.0 + 7, START.1, 0);
    world.state.registry.insert(entity, Position(far));
    world.state.facet_state_mut(Facet(0)).sectors.insert(entity, far);
    let before = world.state.registry.get::<Position>(entity).unwrap().0;
    for _ in 0..200 {
        world.tick(now);
    }
    let after = world.state.registry.get::<Position>(entity).unwrap().0;
    assert!(
        openshard_state::distance(after, Point::new(START.0, START.1, 0))
            < openshard_state::distance(before, Point::new(START.0, START.1, 0)),
        "the pet walked toward its owner: {before:?} → {after:?}"
    );
}

#[test]
fn a_pet_hears_all_stay_and_stops() {
    // The order surface is speech: "all <order>" for everything you own in earshot,
    // "<name> <order>" for one. ServUO matches the client's keyword ids; this
    // matches the words, because the parser skips the keyword block.
    let now = Instant::now();
    let mut world = world();
    let owner = enter(&mut world, now);
    let player = world.state.players[&owner];
    let serial = world.state.registry.serial_of(player).unwrap();
    let horse = spawn_mobile_body(&mut world, 0x00C8, Point::new(START.0 + 3, START.1, 0), now);
    let pet = world.state.registry.entity_of(horse).unwrap();
    world.state.registry.insert(
        pet,
        openshard_state::components::Pet {
            owner: serial,
            slots: openshard_protocol::world::FollowerSlots::ONE,
            order: openshard_state::components::PetOrder::Follow,
            order_target: None,
        },
    );

    world.queue(Command::Say {
        connection: owner,
        mode: RawTalkMode(0),
        hue: RawHue(0),
        font: RawFont(3),
        text: "all stay".to_owned(),
    });
    world.tick(now);
    assert_eq!(
        world
            .state
            .registry
            .get::<openshard_state::components::Pet>(pet)
            .map(|p| p.order),
        Some(openshard_state::components::PetOrder::Stay)
    );
    // And it stops: a staying pet takes no step however far its owner walks.
    let before = world.state.registry.get::<Position>(pet).unwrap().0;
    for _ in 0..100 {
        world.tick(now);
    }
    assert_eq!(world.state.registry.get::<Position>(pet).unwrap().0, before);
}

/// Spawn a creature with a chosen body — what the tamable table is keyed by.
fn spawn_mobile_body(world: &mut World, body: u16, at: Point, now: Instant) -> Serial {
    world.queue(Command::SpawnMobile {
        body: openshard_protocol::wire::Graphic(body),
        hue: openshard_protocol::wire::Hue(0),
        hits: 50,
        notoriety: Notoriety::from_bits(5),
        damage: 5,
        resistance: openshard_protocol::world::PhysicalResistance::new(0),
        swing: 0,
        sight: Sight(0),
        aggression: Aggression::from_bits(0),
        beat: 0,
        ranged: None,
        ranged_kind: DamageType::Physical,
        wander: false,
        position: at,
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
        .filter(|(entity, b)| b.id.0 == body && !world.state.registry.has::<Client>(*entity))
        .filter_map(|(entity, _)| world.state.registry.serial_of(entity))
        .next_back()
        .expect("the creature was spawned")
}

#[test]
fn a_shop_bottle_holds_the_poison_its_label_names() {
    // The four strengths share one graphic, so a bought bottle would be inert
    // glass without this — and the shipped alchemists already stock all four, since
    // the converter reads ServUO's own shop tables. The label is what says which.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let serial = item_beside(&mut world, POISON_POTION_GRAPHIC, now);
    let bottle = world.state.registry.entity_of(serial).unwrap();
    // Spawned bare, it is the middling poison.
    assert_eq!(
        world.state.registry.get::<PoisonCharges>(bottle).map(|p| p.level),
        Some(openshard_protocol::world::PoisonLevel::new(1))
    );
    let _ = player;

    // Named as a shop names it, it is what the label says.
    let (labelled, _) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        labelled,
        Drawn {
            id: POISON_POTION_GRAPHIC,
            hue: openshard_protocol::wire::Hue(0),
        },
    );
    world
        .state
        .registry
        .insert(labelled, Name("a greater poison potion".to_owned()));
    openshard_items::apply_core_defaults(&mut world.state, labelled, POISON_POTION_GRAPHIC);
    assert_eq!(
        world
            .state
            .registry
            .get::<PoisonCharges>(labelled)
            .map(|p| p.level),
        Some(openshard_protocol::world::PoisonLevel::new(2)),
        "greater is level two"
    );
}

#[test]
fn animal_lore_reads_a_pet_and_refuses_a_wild_thing_to_a_novice() {
    // The gates *are* the skill, and they only mean anything now that pets exist —
    // which is why it waited for them. Under 100.0 you may read what somebody has
    // tamed and nothing else.
    let now = Instant::now();
    let mut world = world();
    let looker = enter(&mut world, now);
    let player = world.state.players[&looker];
    let serial = world.state.registry.serial_of(player).unwrap();
    train(&mut world, looker, Skill::AnimalLore, 500);
    world.tick(now);
    let horse = spawn_mobile_body(&mut world, 0x00C8, Point::new(START.0 + 1, START.1, 0), now);
    let entity = world.state.registry.entity_of(horse).unwrap();
    let _ = packets_for(&mut world, looker);

    let said = use_skill_on(&mut world, looker, Skill::AnimalLore, horse, now);
    assert!(
        said.contains(&1_049_674),
        "at your skill level, only tamed creatures: {said:?}"
    );

    // Tame it, and the same look opens the window.
    world.state.registry.insert(
        entity,
        openshard_state::components::Pet {
            owner: serial,
            slots: openshard_protocol::world::FollowerSlots::ONE,
            order: openshard_state::components::PetOrder::Follow,
            order_target: None,
        },
    );
    for _ in 0..=DEFAULT_SKILL_DELAY_TICKS {
        world.tick(now);
    }
    let _ = packets_for(&mut world, looker);
    world.queue(Command::UseSkillButton {
        connection: looker,
        skill: RawSkillId(Skill::AnimalLore.id()),
    });
    world.tick(now);
    let _ = packets_for(&mut world, looker);
    world.queue(Command::TargetResponse {
        connection: looker,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(serial.raw()),
            object: Some(horse),
            location: Point::new(START.0 + 1, START.1, 0),
            graphic: None,
            cancelled: false,
        },
    });
    world.tick(now);
    assert!(
        packets_for(&mut world, looker).iter().any(|p| p[0] == 0xB0),
        "the window opened"
    );
}

/// Setting a skill sends the one-line `0x3A`, so an open window follows.
///
/// `Command::SetSkill` moved the sheet and told nobody, which is only invisible
/// because every test before this one read the sheet rather than the wire.
#[test]
fn setting_a_skill_sends_the_window_its_one_line() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let _ = packets_for(&mut world, connection);

    train(&mut world, connection, Skill::Mining, 500);
    world.tick(now);
    let deltas: Vec<Vec<u8>> = packets_for(&mut world, connection)
        .into_iter()
        .filter(|packet| packet[0] == 0x3A)
        .collect();
    assert_eq!(deltas.len(), 1, "one skill moved, so one line: {deltas:?}");
    // `0xDF` is the delta-with-caps body, and the id is Mining's — raw, unlike
    // the full list's one-based ids.
    assert_eq!(deltas[0][3], 0xDF);
    assert_eq!(
        u16::from_be_bytes([deltas[0][4], deltas[0][5]]),
        u16::from(Skill::Mining.id())
    );

    // And setting it to what it already is says nothing.
    train(&mut world, connection, Skill::Mining, 500);
    world.tick(now);
    assert!(
        !packets_for(&mut world, connection)
            .iter()
            .any(|packet| packet[0] == 0x3A),
        "a set that changed nothing sent a line"
    );
}

/// A stat change moves every skill that stat lends to, and each one needs its
/// line.
///
/// The value a window draws is the trained number *plus* what the stats lend it
/// before AoS, so `.set str 100` moved dozens of numbers on the shard and none on
/// any window. Nothing about it is visible from the sheet, which is why it stood
/// for as long as it did.
#[test]
fn a_stat_change_tells_the_window_about_every_skill_it_moved() {
    let now = Instant::now();
    let mut world = world();
    // Pre-AoS, which is the era the stat influence exists in at all: ServUO
    // zeroes the three scale columns from AoS on, and this asserts the influence.
    world.state.gameplay.combat_era = openshard_config::CombatEra::from(1);
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player).unwrap();
    let before = openshard_skills::skill_value(&world.state, player, Skill::Mining);
    let stats = *world
        .state
        .registry
        .get::<openshard_state::components::Stats>(player)
        .expect("a player has stats");
    let _ = packets_for(&mut world, connection);

    // Strength alone, and *down*: Mining's bonus caps out at its `stat_total`, so
    // a fresh character already sits on the ceiling and raising strength moves
    // nothing. Dexterity and intelligence are left where they are so what is
    // announced can only have come from the one stat that changed.
    world.queue(Command::SetStats {
        serial,
        strength: 10,
        dexterity: stats.dexterity,
        intelligence: stats.intelligence,
    });
    world.tick(now);
    let moved: Vec<u8> = packets_for(&mut world, connection)
        .into_iter()
        .filter(|packet| packet[0] == 0x3A && packet[3] == 0xDF)
        .map(|packet| packet[5])
        .collect();
    assert!(
        moved.contains(&Skill::Mining.id()),
        "Mining leans on strength and was not announced: {moved:?}"
    );
    assert!(
        moved.len() > 1,
        "one stat lends to more than one skill: {moved:?}"
    );
    assert!(
        !moved.contains(&Skill::Meditation.id()),
        "Meditation leans on no stat at all and was announced anyway: {moved:?}"
    );
    assert!(
        openshard_skills::skill_value(&world.state, player, Skill::Mining) < before,
        "the value the window is being told about did not actually move"
    );
}

/// `.skill <name> <value>` — the counterpart of `.set`, in the unit a player
/// reads off their own window.
#[test]
fn the_skill_command_takes_a_name_and_whole_points() {
    let now = Instant::now();
    let mut world = world();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];

    gm::run(&mut world.state, player, "skill mining 95");
    assert_eq!(
        world
            .state
            .registry
            .get::<openshard_state::components::Skills>(player)
            .map(|s| s.get(Skill::Mining)),
        Some(950),
        "whole points are tenths on the sheet"
    );

    // One decimal, because the window draws one.
    gm::run(&mut world.state, player, "skill lumberjacking 33.5");
    assert_eq!(
        world
            .state
            .registry
            .get::<openshard_state::components::Skills>(player)
            .map(|s| s.get(Skill::Lumberjacking)),
        Some(335)
    );

    // A name nobody has, and a value that is not one. Both answer rather than
    // doing nothing, which is what a silent no-op looks like from a chat box.
    for (line, expected) in [
        ("skill mimning 95", "There is no skill called 'mimning'."),
        ("skill mining lots", "That is not a skill value. Try 95, or 95.5."),
        (
            "skill mining 95.55",
            "That is not a skill value. Try 95, or 95.5.",
        ),
    ] {
        world.tick(now);
        let _ = packets_for(&mut world, connection);
        gm::run(&mut world.state, player, line);
        world.tick(now);
        let said = packets_for(&mut world, connection);
        assert!(
            said.iter()
                .any(|packet| String::from_utf8_lossy(packet).contains(expected)),
            "{line:?} did not answer with {expected:?}"
        );
    }
    assert_eq!(
        world
            .state
            .registry
            .get::<openshard_state::components::Skills>(player)
            .map(|s| s.get(Skill::Mining)),
        Some(950),
        "a refused command moved the sheet"
    );
}
