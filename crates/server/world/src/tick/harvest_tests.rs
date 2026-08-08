//! Mining, Lumberjacking and Fishing: the swing, the vein and the tool.
//!
//! A child module of its own, like `skills_tests.rs`, and for the same reason —
//! these go through the whole path a player does, and every link in it has a way
//! of being wrong that no client will report. A tile read from the wrong table
//! yields nothing on ground that looks minable; a bank that never depletes reads
//! as working perfectly until somebody notices Britannia has infinite valorite.
//!
//! The map is a stub [`Ground`] rather than a client file: what these tests are
//! about is the *rules*, and a fake terrain lets them name the tile under the
//! player instead of hunting for a real mountain. `resolve_harvest_target`, the
//! half that reads a real map, is exercised here too — against a terrain that
//! knows one static, which is what the anti-spoof check needs to have an opinion.

use super::tests::{START, enter, packets_for, world};
use super::*;
use openshard_movement::{Terrain, Tile};
use openshard_protocol::containers::GridSlot;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::serial::RawSerial;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_state::Skill;
use openshard_state::components::{Contained, Harvesting, Tool};
use openshard_state::harvest::{HarvestKind, LOG_GRAPHIC, ORE_GRAPHIC, SAND_GRAPHIC, TileSource, definition};

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

/// A terrain that answers one land tile everywhere and holds one static.
///
/// Enough to be a mountain, a beach or a lake for the length of a test, and — with
/// `static_at` set — enough for `resolve_harvest_target` to have something to
/// verify a claimed static against.
struct Ground {
    land: u16,
    static_at: Option<(u16, i8)>,
}

impl Terrain for Ground {
    fn can_step(&self, _from: Point, to: Point) -> Option<Point> {
        Some(to)
    }

    fn land_tile(&self, _tile: Tile) -> Option<u16> {
        Some(self.land)
    }

    fn statics_at(&self, _tile: Tile, out: &mut Vec<(u16, i8)>) {
        out.extend(self.static_at);
    }
}

/// Lay `land` under the whole facet, with an optional static standing on it.
fn ground(world: &mut World, land: u16, static_at: Option<(u16, i8)>) {
    world.state.facet_state_mut(Facet(0)).terrain = Some(Box::new(Ground { land, static_at }));
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
            object: openshard_protocol::serial::Serial::new(0),
            location: Point::new(START.0 + dx, START.1, 0),
            graphic: (graphic != 0).then_some(Graphic(graphic)),
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
    // double-click that has to reach the tool table rather than the pack's
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
    // Felucca pays double, so the vein is two down, not one.
    let bank_left =
        world
            .state
            .facet_state_mut(Facet(0))
            .banks
            .get(before, START.0 + 1, START.1, 0, 0, &mut Rng::new(1));
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
        let bank = banks.get(def, START.0 + 1, START.1, 0, 0, &mut rng);
        let all = bank.maximum;
        bank.consume(def, all, 0, &mut rng);
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
    let far = Point::new(START.0 + 40, START.1, 0);
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
fn a_hatchet_chops_a_tree_static() {
    // Two things at once: an axe is a harvesting tool *derived* from the weapon
    // table's `is_axe` rather than listed again, and a tree is a static — so the
    // tile is matched with the 0x4000 bit set, which is the one arithmetic in
    // `tile_key`.
    let now = Instant::now();
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
    finish_swing(&mut world, player, now);
    assert!(carried(&world, player, LOG_GRAPHIC) > 0, "no logs in the pack");
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
    let at = Point::new(START.0 + 1, START.1, 0);
    assert!(
        skills::resolve_harvest_target(&world.state, 0, at, TREE).is_none(),
        "a tree the map does not have should resolve to nothing"
    );
    // With the tree really there, at the z the client claims, it resolves.
    ground(&mut world, GRASS, Some((TREE, 0)));
    let resolved = skills::resolve_harvest_target(&world.state, 0, at, TREE).expect("a tree");
    assert_eq!(resolved.source, TileSource::Static);
    assert_eq!(resolved.tile, TREE);
    // And a claim at the wrong height is refused too — ServUO matches id *and* z.
    let wrong_z = Point::new(START.0 + 1, START.1, 40);
    assert!(skills::resolve_harvest_target(&world.state, 0, wrong_z, TREE).is_none());
    // Bare ground reads its tile from the map, because the client sends none.
    let land = skills::resolve_harvest_target(&world.state, 0, at, 0).expect("the ground");
    assert_eq!((land.source, land.tile), (TileSource::Land, GRASS));
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
