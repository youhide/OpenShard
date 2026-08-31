//! Mining, Lumberjacking and Fishing: the swing, the vein and the tool.
//!
//! A child module of its own, like `skills_tests.rs`, and for the same reason —
//! these go through the whole path a player does, and every link in it has a way
//! of being wrong that no client will report. A tile read from the wrong table
//! yields nothing on ground that looks minable; a bank that never depletes reads
//! as working perfectly until somebody notices Britannia has infinite valorite.
//!
//! The map is a [`Scene`] rather than a client file: what these tests are about
//! is the *rules*, and a scene lets them name the tile under the player instead
//! of hunting for a real mountain. It is a real map either way — [`ground`]
//! builds one and the shard reads it through its own `MapTerrain`, so a test
//! here cannot pass against a rule the shard does not have.

use openshard_movement::scene::Scene;
use openshard_protocol::containers::GridSlot;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::item_kind::ItemKindId;
use openshard_protocol::serial::RawSerial;
use openshard_protocol::wire::{
    Graphic,
    Hue,
};
use openshard_state::Skill;
use openshard_state::components::{
    Contained,
    Harvesting,
    ItemKind,
    Material,
    Skills,
    Tool,
};
use openshard_state::harvest::{
    HarvestKind,
    LOG_GRAPHIC,
    ORE_GRAPHIC,
    SAND_GRAPHIC,
    TileSource,
    definition,
};

use super::tests::{
    START,
    WALL_FLAGS,
    enter,
    packets_for,
    world,
};
use super::*;

/// A pickaxe, ServUO's `Pickaxe`.
const PICKAXE: Graphic = Graphic(0x0E86);
/// A hatchet — a weapon that is also a lumberjack's tool, which is the point.
const HATCHET: Graphic = Graphic(0x0F43);
/// A fishing pole.
const POLE: Graphic = Graphic(0x0DC0);

/// A mountain face's land tile, the first row of ServUO's mining table.
const MOUNTAIN: u16 = 220;
/// A tree, which is only ever a static.
const TREE: u16 = 0x4CCA;
/// Open sea.
const WATER: u16 = 0x00A9;
/// Plain grass, which is nothing to anybody.
const GRASS: u16 = 3;

/// How far east of [`START`] these tests ever point. `swing_at` reaches six, and
/// one test names a tile forty away to be out of reach — so the ground has to be
/// itself that far, and no further.
const REACH: u16 = 48;

/// Lay `land` under the whole facet, with an optional static standing on the one
/// tile the target tests name.
///
/// **A real map, not a terrain that answers for one.** The land id comes out of
/// a [`Scene`]'s [`WorldMap`](openshard_map::map::WorldMap) and the static out of
/// its statics list, so `land_tile` and `statics_at` are the shard's own reads
/// rather than a fixture's opinion of them. Two things the double did that a map
/// cannot: it laid its static under *every* tile of the facet, and it allowed
/// every step regardless of what was standing there. Neither is a world, and
/// neither is what any test here asks for — the claims are all about one tile a
/// pace to the east.
///
/// The land id is set without a tiledata row on purpose. Harvesting matches the
/// *id* against ServUO's tables — mountain, sand, water — and what the tile can
/// do is a second question no test here asks; giving the sea a
/// [`TileFlags::WATER`](openshard_tiles::TileFlags::WATER) row would
/// pull the player who is already standing on it under.
fn ground(world: &mut World, land: u16, static_at: Option<(u16, i8)>) {
    let mut scene = Scene::flat_holding(START.x + REACH, START.y + REACH, 0);
    scene.land_everywhere(land);
    if let Some((graphic, z)) = static_at {
        // A tree: tall and impassable, which is what a static the client can
        // claim to be chopping actually is.
        scene.art(graphic, WALL_FLAGS, 20);
        scene.put(START.x + 1, START.y, z, graphic);
    }
    let (map, tiles) = scene.into_shard(Facet(0));
    world.state.facet_state_mut(Facet(0)).set_map(Some(map), &tiles);
    world.state.set_tiles(tiles);
}

/// Put a tool in the player's pack and return its entity.
///
/// Through `spawn_with_serial` and `apply_core_defaults`, the same door a vendor's
/// shelf uses — so the uses on it are the ones a bought tool would have.
fn give_tool(world: &mut World, connection: ConnectionId, graphic: Graphic) -> EntityId {
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).unwrap();
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
    let (item, _) = world.state.registry.spawn_with_serial(SerialKind::Item).unwrap();
    world.state.registry.insert(
        item,
        Drawn {
            id:  graphic,
            hue: Hue(0),
        },
    );
    openshard_state::establish_item_location(
        &mut world.state,
        item,
        openshard_state::ItemLocation::contained(Contained {
            container: backpack,
            position:  GumpPoint::new(20, 20),
            grid:      GridSlot(0),
        }),
    )
    .unwrap();
    items::apply_core_defaults(&mut world.state, item, graphic);
    item
}

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

/// Every cliloc number the connection was sent this tick, in order.
fn clilocs(world: &mut World, connection: ConnectionId) -> Vec<u32> {
    packets_for(world, connection)
        .into_iter()
        .filter(|p| p[0] == 0xC1)
        .map(|p| u32::from_be_bytes([p[14], p[15], p[16], p[17]]))
        .collect()
}

/// Double-click a tool and answer its cursor with a spot `dx` tiles away.
fn swing_at(
    world: &mut World,
    connection: ConnectionId,
    tool: EntityId,
    dx: u16,
    graphic: u16,
    now: Instant,
) {
    let tool_serial = world.state.registry.serial_of(tool).unwrap();
    world.queue(Command::DoubleClick {
        connection,
        request: UseRequest::Use(RawSerial(tool_serial.raw())),
    });
    world.tick(now);
    let cursor_id = {
        let entity = world.state.players[&connection];
        world.state.registry.serial_of(entity).unwrap().raw()
    };
    world.queue(Command::TargetResponse {
        connection,
        response: openshard_protocol::target::TargetResponse {
            cursor_id: openshard_protocol::wire::CursorId(cursor_id),
            object:    openshard_protocol::serial::Serial::new(0),
            location:  Point::new(START.x + dx, START.y, 0),
            graphic:   (graphic != 0).then_some(Graphic(graphic)),
            cancelled: false,
        },
    });
    world.tick(now);
}

/// How many of `graphic` are in the player's backpack, across every pile.
fn carried(world: &World, connection: ConnectionId, graphic: Graphic) -> u32 {
    let player = world.state.players[&connection];
    let Some(owner) = world.state.registry.serial_of(player) else {
        return 0;
    };
    let Some(backpack) = items::backpack_of(&world.state, owner) else {
        return 0;
    };
    world
        .state
        .registry
        .query::<Contained>()
        .filter(|(_, held)| held.container == backpack)
        .filter(|(item, _)| {
            world
                .state
                .registry
                .get::<Drawn>(*item)
                .is_some_and(|g| g.id == graphic)
        })
        .map(|(item, _)| {
            u32::from(
                world
                    .state
                    .registry
                    .get::<Amount>(item)
                    .map_or(1, |amount| amount.0),
            )
        })
        .sum()
}

/// Tick until the harvest in flight finishes, or give up.
fn finish_swing(world: &mut World, connection: ConnectionId, mut now: Instant) -> Instant {
    let player = world.state.players[&connection];
    for _ in 0..400 {
        if !world.state.registry.has::<Harvesting>(player) {
            return now;
        }
        now += TICK_INTERVAL;
        world.tick(now);
    }
    panic!("the harvest never finished");
}

#[test]
fn a_pickaxe_swung_at_a_mountain_yields_ore_and_empties_the_vein() {
    // The whole chain, because every link has somewhere to go wrong: a
    // double-click that has to reach the tool table rather than a content
    // `ItemUsed`, a *location* cursor (an object cursor would refuse bare rock),
    // a land tile the client never sent, a band roll, and a bank that goes down.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, MOUNTAIN, None);
    train(&mut world, player, Skill::Mining, 1000);
    let pick = give_tool(&mut world, player, PICKAXE);
    world.tick(now);

    let before = definition(HarvestKind::Ore, true);
    swing_at(&mut world, player, pick, 1, 0, now);
    // The cursor was answered, so the swing is under way.
    let entity = world.state.players[&player];
    assert!(
        world.state.registry.has::<Harvesting>(entity),
        "the pick should be swinging"
    );
    finish_swing(&mut world, player, now);

    assert!(
        carried(&world, player, ORE_GRAPHIC) >= u32::from(before.consumed),
        "no ore in the pack"
    );
    let ore = world
        .state
        .registry
        .query::<ItemKind>()
        .find(|(_, kind)| **kind == ItemKind(ItemKindId(2)))
        .map(|(entity, _)| entity)
        .expect("mining pays an item with an ore kind");
    assert!(
        world.state.registry.has::<Material>(ore),
        "the ore carries a material id"
    );
    // Felucca pays double, so the vein is two down, not one.
    let bank_left = world.state.facet_state_mut(Facet(0)).banks.get(
        before,
        START.x + 1,
        START.y,
        Facet(0),
        openshard_state::WorldTick::ZERO,
        &mut Rng::new(1),
    );
    assert_eq!(
        bank_left.current,
        bank_left.maximum - before.consumed_felucca,
        "the vein did not go down by a Felucca harvest"
    );
}

#[test]
fn the_pick_rings_inside_the_beat_and_not_at_the_head_of_it() {
    // ServUO gives the swing and the noise it makes two different delays
    // (`EffectDelay` 1.6s against `EffectSoundDelay` 0.9s): the pick is raised,
    // and the chink comes most of a second later. Collapsing them into one call
    // is a change nothing fails on and every miner hears — it turns a swing into
    // a metronome.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, MOUNTAIN, None);
    train(&mut world, player, Skill::Mining, 1000);
    let pick = give_tool(&mut world, player, PICKAXE);
    world.tick(now);

    swing_at(&mut world, player, pick, 1, 0, now);
    // The swing has started and the pick has not rung yet.
    assert!(
        !packets_for(&mut world, player).iter().any(|p| p[0] == 0x54),
        "the sound should not be at the head of the beat"
    );
    let def = definition(HarvestKind::Ore, true);
    let mut now = now;
    let mut rang = false;
    for _ in 0..=def.sound_ticks {
        now += TICK_INTERVAL;
        world.tick(now);
        rang |= packets_for(&mut world, player).iter().any(|p| p[0] == 0x54);
    }
    assert!(rang, "the pick never rang inside its beat");
}

/// `HarvestTimer` in the reference starts an effect immediately, then each
/// effect owns its own delayed sound timer.  A long chop therefore makes one
/// sound per complete swing — not one sound for the whole action.
#[test]
fn a_long_chop_repeats_its_sound_once_per_beat() {
    let mut now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, GRASS, Some((TREE, 0)));
    let axe = give_tool(&mut world, player, HATCHET);
    world.tick(now);
    let _ = packets_for(&mut world, player);

    swing_at(&mut world, player, axe, 1, TREE, now);
    let _ = packets_for(&mut world, player);
    let def = definition(HarvestKind::Lumber, true);
    let mut heard_at = Vec::new();
    for _ in 0..=def.beat_ticks.saturating_mul(u64::from(def.beats)) {
        now += TICK_INTERVAL;
        world.tick(now);
        if packets_for(&mut world, player)
            .iter()
            .any(|packet| packet[0] == 0x54)
        {
            heard_at.push(world.state.ticks.raw());
        }
    }

    assert_eq!(
        heard_at.len(),
        usize::from(def.beats),
        "every one of the three visible chops needs its own impact sound"
    );
    assert_eq!(
        heard_at
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .collect::<Vec<_>>(),
        vec![def.beat_ticks; usize::from(def.beats.saturating_sub(1))],
        "the sound repeats at the same 1.6-second beat as the animation"
    );
}

#[test]
fn a_vein_runs_dry_and_says_so() {
    // The bank is the whole point of a harvest system: without depletion one tile
    // is an infinite mine and nobody ever walks anywhere.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, MOUNTAIN, None);
    train(&mut world, player, Skill::Mining, 1000);
    let pick = give_tool(&mut world, player, PICKAXE);
    world.tick(now);
    // Empty the block by hand rather than swinging thirty times, which is the
    // same door `deliver` uses.
    {
        let def = definition(HarvestKind::Ore, true);
        let mut rng = Rng::new(1);
        let banks = &mut world.state.facet_state_mut(Facet(0)).banks;
        let bank = banks.get(
            def,
            START.x + 1,
            START.y,
            Facet(0),
            openshard_state::WorldTick::ZERO,
            &mut rng,
        );
        let all = bank.maximum;
        bank.consume(def, all, openshard_state::WorldTick::ZERO, &mut rng);
    }
    let _ = packets_for(&mut world, player);

    swing_at(&mut world, player, pick, 1, 0, now);
    let entity = world.state.players[&player];
    assert!(
        !world.state.registry.has::<Harvesting>(entity),
        "an empty vein should not start a swing"
    );
    assert!(
        clilocs(&mut world, player).contains(&503_040),
        "there is no metal here to mine"
    );
}

#[test]
fn walking_away_mid_swing_is_a_different_sentence_from_starting_too_far_off() {
    // ServUO keeps `OutOfRangeMessage` and `TimedOutOfRangeMessage` apart, and the
    // difference is the whole feedback: one is "you misclicked", the other is
    // "you gave up". Collapsing them is the sort of thing that reads fine in a
    // diff and is wrong in play.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, MOUNTAIN, None);
    train(&mut world, player, Skill::Mining, 1000);
    let pick = give_tool(&mut world, player, PICKAXE);
    world.tick(now);
    let _ = packets_for(&mut world, player);

    // Six tiles is well past mining's reach of two.
    swing_at(&mut world, player, pick, 6, 0, now);
    assert!(
        clilocs(&mut world, player).contains(&500_446),
        "that is too far away"
    );

    // Now start one properly and teleport out of reach mid-swing.
    swing_at(&mut world, player, pick, 1, 0, now);
    let entity = world.state.players[&player];
    assert!(world.state.registry.has::<Harvesting>(entity));
    let far = Point::new(START.x + 40, START.y, 0);
    crate::gm::teleport_to(&mut world.state, entity, far);
    let _ = packets_for(&mut world, player);
    finish_swing(&mut world, player, now);
    assert!(
        clilocs(&mut world, player).contains(&503_041),
        "you have moved too far away to continue mining"
    );
}

#[test]
fn a_tool_wears_out_and_breaks() {
    // A tool that never wears out is a tool nobody ever buys twice, and the count
    // is the reason vendors stock nineteen lockpicks and forty-six pickaxes.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, MOUNTAIN, None);
    train(&mut world, player, Skill::Mining, 1000);
    let pick = give_tool(&mut world, player, PICKAXE);
    world.state.registry.insert(pick, Tool { uses_left: 1 });
    world.tick(now);
    let _ = packets_for(&mut world, player);

    swing_at(&mut world, player, pick, 1, 0, now);
    finish_swing(&mut world, player, now);
    assert!(
        clilocs(&mut world, player).contains(&1_044_038),
        "you have worn out your tool"
    );
    assert!(
        world.state.registry.serial_of(pick).is_none(),
        "the broken pick should be gone"
    );
}

#[test]
fn a_fishing_pole_will_not_mine_and_a_pickaxe_will_not_fish() {
    // The system a swing belongs to comes from the *tile*, but the tool still has
    // to agree — otherwise a pole cast at a mountain quietly mines it.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, MOUNTAIN, None);
    train(&mut world, player, Skill::Fishing, 1000);
    let pole = give_tool(&mut world, player, POLE);
    world.tick(now);
    let _ = packets_for(&mut world, player);

    swing_at(&mut world, player, pole, 1, 0, now);
    let entity = world.state.players[&player];
    assert!(!world.state.registry.has::<Harvesting>(entity));
    assert!(
        clilocs(&mut world, player).contains(&500_979),
        "you can't fish there"
    );

    // And the water case, which the same pole does handle.
    ground(&mut world, WATER, None);
    swing_at(&mut world, player, pole, 1, 0, now);
    assert!(
        world.state.registry.has::<Harvesting>(entity),
        "a pole cast at water should fish"
    );
}

#[test]
fn a_hatchet_asks_what_to_use_it_on_instead_of_where_to_dig() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let axe = give_tool(&mut world, player, HATCHET);
    world.tick(now);
    let _ = packets_for(&mut world, player);

    let serial = world.state.registry.serial_of(axe).unwrap();
    world.queue(Command::DoubleClick {
        connection: player,
        request:    UseRequest::Use(RawSerial(serial.raw())),
    });
    world.tick(now);

    let packets = packets_for(&mut world, player);
    assert!(
        packets.iter().any(|packet| {
            packet.len() == 23
                && packet[0] == 0xBF
                && packet[3..5] == (openshard_protocol::access::OPENSHARD_SUBCOMMANDS + 13).to_be_bytes()
                && packet[13..15] == 13_u16.to_be_bytes()
                && packet[17..21] == 9_600_u32.to_be_bytes()
                && packet[21..23] == 6_u16.to_be_bytes()
        }),
        "the hatchet cursor should carry an immediate, 9.6-second chop preview"
    );
    let messages: Vec<_> = packets
        .iter()
        .filter(|packet| packet[0] == 0xC1)
        .map(|packet| u32::from_be_bytes([packet[14], packet[15], packet[16], packet[17]]))
        .collect();
    assert!(
        messages.contains(&1_010_018),
        "the axe did not ask for an axe target"
    );
    assert!(!messages.contains(&503_033), "the axe asked where to dig");
}

#[test]
fn a_hatchet_chops_a_tree_static() {
    // Two things at once: an axe is a harvesting tool *derived* from the weapon
    // table's `is_axe` rather than listed again, and a tree is a static — so the
    // tile is matched with the 0x4000 bit set, which is the one arithmetic in
    // `tile_key`.
    let mut now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, GRASS, Some((TREE, 0)));
    train(&mut world, player, Skill::Lumberjacking, 1000);
    let axe = give_tool(&mut world, player, HATCHET);
    world.tick(now);

    swing_at(&mut world, player, axe, 1, TREE, now);
    let entity = world.state.players[&player];
    assert!(
        world.state.registry.has::<Harvesting>(entity),
        "the axe should be chopping"
    );
    assert_eq!(
        world
            .state
            .registry
            .get::<Harvesting>(entity)
            .expect("chopping")
            .beats_left,
        6,
        "lumberjacking lasts six full beats rather than requiring two targets"
    );
    for _ in 0..definition(HarvestKind::Lumber, true).beat_ticks * 5 {
        now += TICK_INTERVAL;
        world.tick(now);
    }
    assert!(
        world.state.registry.has::<Harvesting>(entity),
        "the sixth complete chop is still owed"
    );
    assert_eq!(
        carried(&world, player, LOG_GRAPHIC),
        0,
        "logs arrive after the final whole stroke, not while chopping is looping"
    );
    finish_swing(&mut world, player, now);
    assert_eq!(
        carried(&world, player, LOG_GRAPHIC),
        u32::from(definition(HarvestKind::Lumber, true).consumed_felucca),
        "one long Felucca chop should replace two old ten-log payouts"
    );
    assert!(
        world
            .state
            .registry
            .get::<Skills>(entity)
            .expect("a player always has a skill sheet")
            .get(Skill::Lumberjacking)
            > 0,
        "a zero-skill lumberjack should learn from the first completed tree"
    );
    assert!(
        packets_for(&mut world, player).iter().any(|packet| {
            packet.len() == 9
                && packet[0] == 0xBF
                && packet[3..5] == (openshard_protocol::access::OPENSHARD_SUBCOMMANDS + 15).to_be_bytes()
        }),
        "the logs must be accompanied by the signal that ends the local chop"
    );
}

#[test]
fn a_second_tree_swing_does_not_claim_the_lumberjack_is_fishing() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, GRASS, Some((TREE, 0)));
    let axe = give_tool(&mut world, player, HATCHET);
    world.tick(now);
    let _ = packets_for(&mut world, player);

    swing_at(&mut world, player, axe, 1, TREE, now);
    assert!(
        world
            .state
            .registry
            .has::<Harvesting>(world.state.players[&player]),
        "the first axe swing should still be in progress"
    );
    let _ = packets_for(&mut world, player);

    swing_at(&mut world, player, axe, 1, TREE, now);
    let text: Vec<_> = packets_for(&mut world, player)
        .into_iter()
        .filter(|packet| packet[0] == 0x1C)
        .map(|packet| String::from_utf8_lossy(&packet).into_owned())
        .collect();
    assert!(
        text.iter()
            .any(|message| message.contains("You are already harvesting.")),
        "the repeated axe swing should describe harvesting: {text:?}"
    );
    assert!(
        text.iter().all(|message| !message.contains("fishing")),
        "a lumberjack must never be told they are fishing: {text:?}"
    );
}

#[test]
fn a_backpack_hatchet_is_drawn_for_its_chop() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, GRASS, Some((TREE, 0)));
    let axe = give_tool(&mut world, player, HATCHET);
    world.tick(now);
    let _ = packets_for(&mut world, player);

    swing_at(&mut world, player, axe, 1, TREE, now);

    assert!(
        packets_for(&mut world, player).iter().any(|packet| {
            packet.len() == 14
                && packet[0] == 0xBF
                && packet[3..5] == (openshard_protocol::access::OPENSHARD_SUBCOMMANDS + 12).to_be_bytes()
                && packet[9..11] == HATCHET.0.to_be_bytes()
        }),
        "the chop should lend the backpack hatchet to its animation picture"
    );
}

#[test]
fn a_bad_tree_target_refuses_the_clients_optimistic_chop() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    // Grass with no static: the client may optimistically swing at its click,
    // but only the map can decide this is not a tree.
    ground(&mut world, GRASS, None);
    let axe = give_tool(&mut world, player, HATCHET);
    world.tick(now);
    let _ = packets_for(&mut world, player);

    swing_at(&mut world, player, axe, 1, 0, now);
    assert!(
        packets_for(&mut world, player).iter().any(|packet| {
            packet.len() == 9
                && packet[0] == 0xBF
                && packet[3..5] == (openshard_protocol::access::OPENSHARD_SUBCOMMANDS + 14).to_be_bytes()
        }),
        "the shard must explicitly stop a locally predicted chop it rejected"
    );
}

#[test]
fn a_client_cannot_name_a_static_that_is_not_there() {
    // The anti-spoof half of the target reply, and it is not decoration: without
    // it a client sends "there is a tree at my feet" and mines the middle of
    // Britain. ServUO cancels the target outright, and so does this.
    let now = Instant::now();
    let mut world = world();
    let _player = enter(&mut world, now);
    // Grass, and *no* static standing on it.
    ground(&mut world, GRASS, None);
    let at = Point::new(START.x + 1, START.y, 0);
    assert!(
        skills::resolve_harvest_target(&world.state, Facet(0), at, Graphic(TREE)).is_none(),
        "a tree the map does not have should resolve to nothing"
    );
    // With the tree really there, at the z the client claims, it resolves.
    ground(&mut world, GRASS, Some((TREE, 0)));
    let resolved = skills::resolve_harvest_target(&world.state, Facet(0), at, Graphic(TREE)).expect("a tree");
    assert_eq!(resolved.source, TileSource::Static);
    assert_eq!(resolved.tile, Graphic(TREE));
    // And a claim at the wrong height is refused too — ServUO matches id *and* z.
    let wrong_z = Point::new(START.x + 1, START.y, 40);
    assert!(skills::resolve_harvest_target(&world.state, Facet(0), wrong_z, Graphic(TREE)).is_none());
    // Bare ground reads its tile from the map, because the client sends none.
    let land = skills::resolve_harvest_target(&world.state, Facet(0), at, Graphic(0)).expect("the ground");
    assert_eq!((land.source, land.tile), (TileSource::Land, Graphic(GRASS)));
}

#[test]
fn sand_is_mined_with_the_same_pick_and_takes_six_beats() {
    // The two mining definitions share a skill and a tool and differ in their
    // tiles and their pace, which is what makes "which system is this" a question
    // about the ground rather than about what is in your hand.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, 22, None); // the first row of ServUO's sand table
    train(&mut world, player, Skill::Mining, 1000);
    let pick = give_tool(&mut world, player, PICKAXE);
    world.tick(now);

    swing_at(&mut world, player, pick, 1, 0, now);
    let entity = world.state.players[&player];
    let work = *world.state.registry.get::<Harvesting>(entity).expect("digging");
    assert_eq!(work.kind, HarvestKind::Sand);
    assert_eq!(work.beats_left, definition(HarvestKind::Sand, true).beats);
    finish_swing(&mut world, player, now);
    assert!(carried(&world, player, SAND_GRAPHIC) > 0, "no sand");
}

#[test]
fn a_half_used_tool_and_a_half_played_lute_both_survive_a_restart() {
    // The v20 column, both halves. The instrument half is a bug this fixes rather
    // than a feature it adds: nothing saved the count, so a lute bought and half
    // played came back full at every reboot.
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    let pick = give_tool(&mut world, player, PICKAXE);
    world.state.registry.insert(pick, Tool { uses_left: 17 });
    let lute = give_tool(&mut world, player, Graphic(0x0EB3)); // not a tool; the branch below sets it
    world
        .state
        .registry
        .insert(lute, openshard_state::components::Instrument { uses_left: 42 });
    world.tick(now);

    world.take_snapshot();
    let snapshot = world.drain_saves().next().expect("a snapshot");
    let saved: Vec<_> = snapshot
        .inventories
        .iter()
        .flat_map(|inventory| inventory.items.iter())
        .filter_map(|item| item.uses.map(|uses| (item.graphic, uses)))
        .collect();
    assert!(
        saved.contains(&(PICKAXE.0, 17)),
        "the pickaxe's swings were not saved: {saved:?}"
    );
    assert!(
        saved.contains(&(0x0EB3, 42)),
        "the lute's tunes were not saved: {saved:?}"
    );
}

/// A pack with no room in it loses the ore, and says so.
///
/// The line existed and nothing could reach it: `give_to_backpack` failed only
/// for a mobile wearing no pack at all, so a miner mined into a backpack with no
/// bottom. The ceiling is ServUO's `Container.GlobalMaxItems`, and the vein is
/// asserted *not* to have gone down with the swing — a full pack costs the swing
/// and the tool's use, not the ore in the ground.
#[test]
fn a_full_pack_loses_the_ore_and_tells_the_miner() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, MOUNTAIN, None);
    train(&mut world, player, Skill::Mining, 1000);
    let pick = give_tool(&mut world, player, PICKAXE);
    world.tick(now);

    // Fill the pack to the brim with discrete items, which is what a slot count
    // counts. Not staff: `check_hold` waves those through, and a test run as one
    // would pass on an engine with no ceiling at all.
    let entity = world.state.players[&player];
    assert!(
        !world.state.is_staff(entity),
        "staff are never refused, so this would prove nothing"
    );
    let owner = world.state.registry.serial_of(entity).unwrap();
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
    let already = items::contents_of(&world.state, backpack).len();
    for _ in already..items::MAX_ITEMS {
        items::place_one(&mut world.state, backpack, Graphic(0x1BE3), Hue(0), 1)
            .expect("the serial pool is not dry");
    }
    let _ = packets_for(&mut world, player);

    let before = definition(HarvestKind::Ore, true);
    swing_at(&mut world, player, pick, 1, 0, now);
    finish_swing(&mut world, player, now);
    let said = clilocs(&mut world, player);

    assert!(
        said.contains(&before.messages.pack_full.0),
        "the miner was not told the ore was lost: {said:?}"
    );
    assert_eq!(carried(&world, player, ORE_GRAPHIC), 0, "the ore landed anyway");
    // The vein is untouched. ServUO takes the bank down only on a payout that
    // landed, and a swing that costs the ore in the ground *and* delivers nothing
    // is the shape of bug that empties a shard quietly.
    let bank = world.state.facet_state_mut(Facet(0)).banks.get(
        before,
        START.x + 1,
        START.y,
        Facet(0),
        openshard_state::WorldTick::ZERO,
        &mut Rng::new(1),
    );
    assert_eq!(
        bank.current, bank.maximum,
        "a lost payout took the vein down with it"
    );
}

/// One more onto a pile already in the pack is not one more *item*.
///
/// The trap in a slot count: ore stacks, so a miner at the item ceiling with a
/// pile of iron already in there is still owed the next ten. ServUO asks
/// `CheckStack` before `CheckHold` for exactly this, and a ceiling that skipped
/// the question would stop a miner mining at a hundred and twenty-five swings
/// with a pack that had room for all of it.
#[test]
fn a_pile_already_in_the_pack_takes_more_at_the_item_ceiling() {
    let now = Instant::now();
    let mut world = world();
    let player = enter(&mut world, now);
    ground(&mut world, MOUNTAIN, None);
    train(&mut world, player, Skill::Mining, 1000);
    let pick = give_tool(&mut world, player, PICKAXE);
    world.tick(now);

    // One swing to make the pile, then fill every remaining slot around it.
    swing_at(&mut world, player, pick, 1, 0, now);
    let now = finish_swing(&mut world, player, now);
    let mined = carried(&world, player, ORE_GRAPHIC);
    assert!(mined > 0, "the first swing paid nothing, so there is no pile");

    let entity = world.state.players[&player];
    let owner = world.state.registry.serial_of(entity).unwrap();
    let backpack = items::backpack_of(&world.state, owner).expect("a backpack");
    let already = items::contents_of(&world.state, backpack).len();
    for _ in already..items::MAX_ITEMS {
        items::place_one(&mut world.state, backpack, Graphic(0x1BE3), Hue(0), 1)
            .expect("the serial pool is not dry");
    }
    let _ = packets_for(&mut world, player);

    let before = definition(HarvestKind::Ore, true);
    swing_at(&mut world, player, pick, 1, 0, now);
    finish_swing(&mut world, player, now);
    let said = clilocs(&mut world, player);
    assert!(
        !said.contains(&before.messages.pack_full.0),
        "a merge onto a pile that was already there was charged a slot: {said:?}"
    );
    assert!(
        carried(&world, player, ORE_GRAPHIC) > mined,
        "the ore did not reach the pile it should have merged onto"
    );
}
