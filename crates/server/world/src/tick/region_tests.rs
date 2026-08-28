//! Named areas, and what they change: the crossing and its music, the day/night
//! light, the guards a town answers with, and the regions that survive a
//! restart.
//!
//! A child module rather than more of `tests.rs`, for the reason `status_tests`
//! gives: these read private world state, so they stay inside the module, but
//! they need not pile into the same file.

use super::tests::{START, enter, enter_as, enter_gm, packets_for, teleport, world};
use super::*;
use openshard_state::components::{CriminalUntil, Guard, Hitpoints, Murders, Staff};
use openshard_state::{Region, RegionFlags, RegionId, RegionRect};

/// Britain's music track, as the client numbers them.
const BRITAIN_MUSIC: u16 = 11;
/// The dark of a dungeon — ServUO's `LightCycle.DungeonLevel`.
const DUNGEON_LIGHT: u8 = 26;

/// A region covering the tiles a test's characters stand on, with whatever flags
/// the case is about.
fn town(name: &str, flags: RegionFlags) -> Region {
    Region {
        id: RegionId(0),
        name: name.to_owned(),
        priority: 50,
        // A generous box around START, so a step in any direction stays inside.
        rects: vec![RegionRect::new(START.x - 20, START.y - 20, 40, 40)],
        flags,
        music: Some(BRITAIN_MUSIC),
        light: None,
    }
}

/// Register `regions` on the default facet, as `content::verb` would.
fn register(world: &mut World, regions: Vec<Region>, now: Instant) {
    world.queue(Command::RegisterRegions {
        facet: Facet(0),
        regions,
    });
    world.tick(now);
}

/// The packets of one id sent to a connection by the last tick.
fn packets_of(world: &mut World, connection: ConnectionId, id: u8) -> Vec<Vec<u8>> {
    packets_for(world, connection)
        .into_iter()
        .filter(|packet| packet[0] == id)
        .collect()
}

#[test]
fn walking_into_a_town_is_one_crossing_and_standing_still_is_none() {
    let mut world = world();
    let now = Instant::now();
    // A tiny region beside the start, so the character begins outside it.
    let inside = Point::new(START.x + 5, START.y, 0);
    let player = enter(&mut world, now);
    register(
        &mut world,
        vec![Region {
            id: RegionId(0),
            name: "Britain".to_owned(),
            priority: 50,
            rects: vec![RegionRect::new(inside.x, inside.y, 4, 4)],
            flags: RegionFlags::none(),
            music: None,
            light: None,
        }],
        now,
    );
    let mut crossings = world.state.bus.cursor::<crate::events::RegionChanged>();

    // Outside: no crossing at all, however many ticks pass.
    world.tick(now);
    assert_eq!(world.state.bus.read(&mut crossings).count(), 0);

    teleport(&mut world, player, inside);
    world.tick(now);
    let entered: Vec<_> = world.state.bus.read(&mut crossings).cloned().collect::<Vec<_>>();
    assert_eq!(entered.len(), 1, "one crossing for one boundary");
    assert_eq!(entered[0].name, "Britain");
    assert_eq!(entered[0].from, None, "it came out of unnamed land");
    assert_eq!(entered[0].to, Some(RegionId(0)));

    // Standing still inside is not a crossing, however long it lasts.
    for _ in 0..5 {
        world.tick(now);
    }
    assert_eq!(
        world.state.bus.read(&mut crossings).count(),
        0,
        "the diff must not re-fire for a mobile that has not moved"
    );
}

#[test]
fn leaving_a_town_reports_the_region_it_left() {
    let mut world = world();
    let now = Instant::now();
    let player = enter(&mut world, now);
    register(&mut world, vec![town("Britain", RegionFlags::none())], now);
    world.tick(now);
    let mut crossings = world.state.bus.cursor::<crate::events::RegionChanged>();

    teleport(&mut world, player, Point::new(START.x + 100, START.y, 0));
    world.tick(now);

    let left: Vec<_> = world.state.bus.read(&mut crossings).cloned().collect();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].from, Some(RegionId(0)), "it names the region left");
    assert_eq!(left[0].to, None, "and lands nowhere named");
    assert!(left[0].name.is_empty());
}

#[test]
fn a_region_plays_its_music_once() {
    let mut world = world();
    let now = Instant::now();
    let player = enter(&mut world, now);
    let _ = packets_for(&mut world, player);
    // Registering ticks, and that tick is the crossing: everyone already standing
    // inside the new region has just arrived in it.
    register(&mut world, vec![town("Britain", RegionFlags::none())], now);

    let music = packets_of(&mut world, player, 0x6D);
    assert_eq!(music.len(), 1, "the town's track starts on the crossing");
    assert_eq!(u16::from_be_bytes([music[0][1], music[0][2]]), BRITAIN_MUSIC);

    // Stepping about inside must not restart it: re-sending 0x6D plays the track
    // from the top, so a player pacing a town line would hear the first bar over
    // and over.
    teleport(&mut world, player, Point::new(START.x + 2, START.y + 2, 0));
    world.tick(now);
    assert!(
        packets_of(&mut world, player, 0x6D).is_empty(),
        "the same track is not re-sent"
    );
}

#[test]
fn the_light_follows_the_hour() {
    let world = world();
    // The clock is derived from the tick counter, so the world starts at
    // midnight — night. ServUO's curve: night until 04:00, full day from 06:00.
    assert_eq!(world.daylight_at(0), LIGHT_NIGHT, "midnight is night");

    let at = |hour: u64| super::tests::world().with_clock_minutes(hour * 60).daylight_at(0);
    assert_eq!(at(3), LIGHT_NIGHT, "before dawn");
    assert!(
        at(5) < LIGHT_NIGHT && at(5) > LIGHT_DAY,
        "the dawn ramp is between the two, not a switch"
    );
    assert_eq!(at(12), LIGHT_DAY, "noon is full daylight");
    assert_eq!(at(21), LIGHT_DAY, "still day before dusk");
    assert!(
        at(23) > LIGHT_DAY && at(23) <= LIGHT_NIGHT,
        "the dusk ramp falls back toward night"
    );
}

#[test]
fn a_dungeon_is_dark_at_noon_and_night_sight_beats_both() {
    let mut world = world().with_clock_minutes(12 * 60);
    let now = Instant::now();
    let player = enter(&mut world, now);
    let _ = packets_for(&mut world, player);
    register(
        &mut world,
        vec![Region {
            id: RegionId(0),
            name: "Covetous".to_owned(),
            priority: 50,
            rects: vec![RegionRect::new(START.x - 20, START.y - 20, 40, 40)],
            flags: RegionFlags::none(),
            music: None,
            light: Some(DUNGEON_LIGHT),
        }],
        now,
    );

    let light = packets_of(&mut world, player, 0x4F);
    assert_eq!(
        light.last().map(|p| p[1]),
        Some(DUNGEON_LIGHT),
        "the region's dark overrides the hour"
    );

    // Night Sight is the buff that beats the dark, so it beats the cave too.
    let entity = world.state.players[&player];
    let serial = world.state.registry.serial_of(entity).unwrap();
    let expires = world.state.ticks + 1000;
    magic::apply_behaviour_buff(
        &mut world.state,
        serial,
        openshard_state::BehaviourBuffKind::NIGHT_SIGHT,
        0,
        expires,
    );
    world.tick(now);
    assert_eq!(
        packets_of(&mut world, player, 0x4F).last().map(|p| p[1]),
        Some(LIGHT_NIGHTSIGHT.0),
        "Night Sight lights the cave"
    );
}

#[test]
fn the_light_is_sent_only_when_it_changes() {
    let mut world = world();
    let now = Instant::now();
    let player = enter(&mut world, now);
    let _ = packets_for(&mut world, player);

    // Nothing moved and no time worth a level passed: the diff sends nothing.
    for _ in 0..5 {
        world.tick(now);
    }
    assert!(
        packets_of(&mut world, player, 0x4F).is_empty(),
        "a still player at a still hour is told nothing"
    );
}

#[test]
fn calling_the_guards_kills_a_criminal_in_a_guarded_town() {
    let mut world = world();
    let now = Instant::now();
    let player = enter(&mut world, now);
    let thief = enter_as(&mut world, ConnectionId::from_raw(9), now);
    register(
        &mut world,
        vec![town(
            "Britain",
            RegionFlags {
                guarded: true,
                no_teleport: false,
                no_recall: false,
                no_housing: false,
                safe: false,
            },
        )],
        now,
    );
    // The thief has raised a hand against someone blue: the grey flag every
    // guarded town answers.
    let thief_entity = world.state.players[&thief];
    world.state.registry.insert(
        thief_entity,
        CriminalUntil {
            tick: world.state.ticks + 1000,
        },
    );

    world.queue(Command::Say {
        connection: player,
        mode: RawTalkMode(0),
        hue: RawHue(0),
        font: RawFont(0),
        text: "guards!".to_owned(),
    });
    world.tick(now);

    let hits = world
        .state
        .registry
        .get::<Hitpoints>(thief_entity)
        .expect("the thief has hit points");
    assert_eq!(hits.current, 0, "a guard's blow is the whole pool");
    assert_eq!(
        world.state.registry.query::<Guard>().count(),
        1,
        "one call sends one guard"
    );
}

#[test]
fn the_guards_do_not_touch_the_innocent() {
    let mut world = world();
    let now = Instant::now();
    let player = enter(&mut world, now);
    let passer_by = enter_as(&mut world, ConnectionId::from_raw(9), now);
    register(
        &mut world,
        vec![town(
            "Britain",
            RegionFlags {
                guarded: true,
                no_teleport: false,
                no_recall: false,
                no_housing: false,
                safe: false,
            },
        )],
        now,
    );

    world.queue(Command::Say {
        connection: player,
        mode: RawTalkMode(0),
        hue: RawHue(0),
        font: RawFont(0),
        text: "guards".to_owned(),
    });
    world.tick(now);

    let entity = world.state.players[&passer_by];
    let hits = world.state.registry.get::<Hitpoints>(entity).unwrap();
    assert!(hits.current > 0, "an innocent is not guard business");
    assert_eq!(
        world.state.registry.query::<Guard>().count(),
        0,
        "and no guard is spawned with nobody to punish"
    );
}

#[test]
fn the_guards_are_not_called_outside_a_guarded_region() {
    let mut world = world();
    let now = Instant::now();
    let player = enter(&mut world, now);
    let thief = enter_as(&mut world, ConnectionId::from_raw(9), now);
    // A region with no guards flag — the wilds, or a town that disabled them.
    register(&mut world, vec![town("Wilds", RegionFlags::none())], now);
    let thief_entity = world.state.players[&thief];
    world.state.registry.insert(
        thief_entity,
        CriminalUntil {
            tick: world.state.ticks + 1000,
        },
    );

    world.queue(Command::Say {
        connection: player,
        mode: RawTalkMode(0),
        hue: RawHue(0),
        font: RawFont(0),
        text: "guards".to_owned(),
    });
    world.tick(now);

    assert_eq!(world.state.registry.query::<Guard>().count(), 0);
    let hits = world.state.registry.get::<Hitpoints>(thief_entity).unwrap();
    assert!(hits.current > 0, "no authority outside a guarded region");
}

#[test]
fn staff_are_never_guard_candidates() {
    let mut world = world();
    let now = Instant::now();
    // The game master takes the default test connection; the caller needs one of
    // its own, or the second entry is refused as "already in the world".
    let gm = enter_gm(&mut world, now);
    let player = enter_as(&mut world, ConnectionId::from_raw(9), now);
    register(
        &mut world,
        vec![town(
            "Britain",
            RegionFlags {
                guarded: true,
                no_teleport: false,
                no_recall: false,
                no_housing: false,
                safe: false,
            },
        )],
        now,
    );
    // Guilty on paper, and exempt anyway.
    let gm_entity = world.state.players[&gm];
    world.state.registry.insert(
        gm_entity,
        CriminalUntil {
            tick: world.state.ticks + 1000,
        },
    );

    world.queue(Command::Say {
        connection: player,
        mode: RawTalkMode(0),
        hue: RawHue(0),
        font: RawFont(0),
        text: "guards".to_owned(),
    });
    world.tick(now);

    let hits = world.state.registry.get::<Hitpoints>(gm_entity).unwrap();
    assert!(hits.current > 0, "the staff mode exempts a game master");
}

#[test]
fn a_murderer_walking_into_town_is_hunted_without_a_call() {
    let mut world = world();
    let now = Instant::now();
    let outside = Point::new(START.x + 100, START.y, 0);
    let killer = enter(&mut world, now);
    register(
        &mut world,
        vec![town(
            "Britain",
            RegionFlags {
                guarded: true,
                no_teleport: false,
                no_recall: false,
                no_housing: false,
                safe: false,
            },
        )],
        now,
    );
    let entity = world.state.players[&killer];
    world.state.registry.insert(entity, Murders(5));
    teleport(&mut world, killer, outside);
    world.tick(now);
    assert_eq!(
        world.state.registry.query::<Guard>().count(),
        0,
        "the woods are nobody's jurisdiction"
    );

    teleport(&mut world, killer, Point::new(START.x, START.y, 0));
    world.tick(now);

    assert_eq!(
        world.state.registry.query::<Guard>().count(),
        1,
        "crossing into a guarded town is itself the call"
    );
    let hits = world.state.registry.get::<Hitpoints>(entity).unwrap();
    assert_eq!(hits.current, 0);
}

#[test]
fn a_guard_earns_no_murder_count_and_leaves_when_it_is_done() {
    let mut world = world();
    let now = Instant::now();
    let player = enter(&mut world, now);
    let thief = enter_as(&mut world, ConnectionId::from_raw(9), now);
    register(
        &mut world,
        vec![town(
            "Britain",
            RegionFlags {
                guarded: true,
                no_teleport: false,
                no_recall: false,
                no_housing: false,
                safe: false,
            },
        )],
        now,
    );
    let thief_entity = world.state.players[&thief];
    world.state.registry.insert(
        thief_entity,
        CriminalUntil {
            tick: world.state.ticks + 1000,
        },
    );
    world.queue(Command::Say {
        connection: player,
        mode: RawTalkMode(0),
        hue: RawHue(0),
        font: RawFont(0),
        text: "guards".to_owned(),
    });
    world.tick(now);

    let (guard, _) = world
        .state
        .registry
        .query::<Guard>()
        .next()
        .expect("a guard answered");
    assert_eq!(
        world
            .state
            .registry
            .get::<Murders>(guard)
            .map_or(0, |count| count.0),
        0,
        "killing the guilty is the guard's purpose, not a murder"
    );

    // It stands about for a while, then vanishes on its own tick counter.
    let until = world.state.registry.get::<Guard>(guard).unwrap().until;
    world.state.ticks = until;
    world.tick(now);
    assert_eq!(
        world.state.registry.query::<Guard>().count(),
        0,
        "a guard with nothing to do goes home"
    );
}

#[test]
fn a_no_teleport_region_refuses_both_ways() {
    let mut world = world();
    let now = Instant::now();
    let player = enter(&mut world, now);
    let barred = Point::new(START.x + 60, START.y, 0);
    register(
        &mut world,
        vec![Region {
            id: RegionId(0),
            name: "The Jail".to_owned(),
            priority: 50,
            rects: vec![RegionRect::new(barred.x - 2, barred.y - 2, 8, 8)],
            flags: RegionFlags {
                no_teleport: true,
                guarded: false,
                no_recall: false,
                no_housing: false,
                safe: false,
            },
            music: None,
            light: None,
        }],
        now,
    );
    let entity = world.state.players[&player];

    // In is refused.
    assert!(!world.state.may_teleport(entity, barred));
    // Out is refused too — a jail one can cast out of is not a jail.
    teleport(&mut world, player, barred);
    assert!(!world.state.may_teleport(entity, Point::new(START.x, START.y, 0)));
    // And ordinary ground is open in both directions.
    teleport(&mut world, player, Point::new(START.x, START.y, 0));
    assert!(
        world
            .state
            .may_teleport(entity, Point::new(START.x + 1, START.y, 0))
    );
}

#[test]
fn staff_teleport_where_players_may_not() {
    let mut world = world();
    let now = Instant::now();
    let gm = enter_gm(&mut world, now);
    let barred = Point::new(START.x + 60, START.y, 0);
    register(
        &mut world,
        vec![Region {
            id: RegionId(0),
            name: "The Jail".to_owned(),
            priority: 50,
            rects: vec![RegionRect::new(barred.x - 2, barred.y - 2, 8, 8)],
            flags: RegionFlags {
                no_teleport: true,
                guarded: false,
                no_recall: false,
                no_housing: false,
                safe: false,
            },
            music: None,
            light: None,
        }],
        now,
    );

    let entity = world.state.players[&gm];
    assert!(
        world.state.may_teleport(entity, barred),
        "a game master in staff mode goes where they are needed"
    );
    // With the mode off, the same account is under the rule with everyone else.
    world.state.registry.remove::<Staff>(entity);
    assert!(!world.state.may_teleport(entity, barred));
}

#[test]
fn regions_and_the_clock_survive_a_restart() {
    let mut world = world();
    let now = Instant::now();
    let _ = enter(&mut world, now);
    register(
        &mut world,
        vec![
            town(
                "Britain",
                RegionFlags {
                    guarded: true,
                    no_teleport: false,
                    no_recall: false,
                    no_housing: false,
                    safe: false,
                },
            ),
            Region {
                id: RegionId(0),
                name: "Covetous".to_owned(),
                priority: 60,
                rects: vec![RegionRect::new(100, 100, 20, 20).with_z(-128, -20)],
                flags: RegionFlags {
                    no_teleport: true,
                    guarded: false,
                    no_recall: false,
                    no_housing: false,
                    safe: false,
                },
                music: None,
                light: Some(DUNGEON_LIGHT),
            },
        ],
        now,
    );
    // Move the clock somewhere that is plainly not midnight.
    world = world.with_clock_minutes(13 * 60);
    world.take_snapshot();
    let snapshot = world.drain_saves().next().expect("regions are worth a snapshot");
    let saved = snapshot.regions.clone().expect("the sweep took them");
    assert_eq!(saved.len(), 2);
    let world_row = snapshot.world.expect("the sweep took the world's own scalars");
    assert_eq!(world_row.clock_minutes, 13 * 60);

    // A fresh world, restored from those records, is the same world.
    let mut restored = super::tests::world();
    restored.restore_regions(saved);
    let britain = restored
        .state
        .region_at(Facet(0), Point::new(START.x, START.y, 0))
        .expect("Britain came back");
    assert_eq!(britain.name, "Britain");
    assert!(britain.flags.guarded, "and it is still guarded");
    assert_eq!(britain.music, Some(BRITAIN_MUSIC));

    let dungeon = restored
        .state
        .region_at(Facet(0), Point::new(105, 105, -40))
        .expect("the dungeon came back, height band and all");
    assert_eq!(dungeon.light, Some(DUNGEON_LIGHT));
    assert!(dungeon.flags.no_teleport);
    assert!(
        restored
            .state
            .region_at(Facet(0), Point::new(105, 105, 0))
            .is_none(),
        "the surface above it is still open sky"
    );

    let restored = restored.with_clock_minutes(world_row.clock_minutes);
    assert_eq!(restored.uo_time_at(0).0, 13, "and it is one in the afternoon");
}

#[test]
fn registering_again_replaces_the_set() {
    let mut world = world();
    let now = Instant::now();
    let _ = enter(&mut world, now);
    register(&mut world, vec![town("Britain", RegionFlags::none())], now);
    register(&mut world, vec![town("Trinsic", RegionFlags::none())], now);

    let here = world
        .state
        .region_at(Facet(0), Point::new(START.x, START.y, 0))
        .expect("somewhere");
    assert_eq!(here.name, "Trinsic");
    assert_eq!(
        world.state.facet_state(Facet(0)).regions.len(),
        1,
        "a second registration replaces, it does not stack"
    );
}
