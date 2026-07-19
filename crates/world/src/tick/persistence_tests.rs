use super::tests::{enter, enter_as, walk, START};
use super::*;
use openshard_gateway::ConnectionId;
use openshard_movement::WALK_INTERVAL;

/// A world that saves every tick, so a test does not have to run four
/// hundred of them to see one row.
fn eager() -> World {
    World::new(START).with_save_every(1)
}

/// Take `count` steps, and return the tick time afterwards.
///
/// The extra request is not a typo: a character spawns facing south, and the
/// first request in any other direction turns rather than steps. A test that
/// sends one request per step is a test that is off by one.
fn steps(
    world: &mut World,
    connection: ConnectionId,
    direction: Direction,
    count: u32,
    start: Instant,
) -> Instant {
    let mut now = start;
    for request in 0..=count {
        now += WALK_INTERVAL;
        world.queue(Command::Walk {
            connection,
            request: walk(request as u8, direction),
        });
        world.tick(now);
    }
    now
}

fn only_snapshot(world: &mut World) -> Option<Snapshot> {
    let mut saves: Vec<_> = world.drain_saves().collect();
    assert!(saves.len() <= 1, "one tick, one snapshot");
    saves.pop()
}

#[test]
fn entering_the_world_is_worth_saving() {
    let mut world = eager();
    let now = Instant::now();
    enter(&mut world, now);

    let snapshot = only_snapshot(&mut world).expect("a new character is a change");
    assert_eq!(snapshot.characters.len(), 1);
    assert_eq!(snapshot.characters[0].name, "Lord British");
    assert_eq!(snapshot.characters[0].x, START.0);
}

#[test]
fn an_empty_world_offers_nothing() {
    // No transaction just to say a shard is idle. With nobody online and
    // nothing loose on the ground, a save writes nothing and so is skipped.
    //
    // Note the deliberate change from earlier: an *online* character is now
    // saved every cadence whether or not it moved — picking an item up takes no
    // step, so the dirty set is not a safe basis for saving what someone holds.
    // That safety is worth a small, periodic write per online player; this test
    // guards the other side, that an empty shard still writes nothing.
    let mut world = eager();
    let now = Instant::now();
    for tick in 1..10 {
        world.tick(now + WALK_INTERVAL * tick);
    }
    assert_eq!(world.drain_saves().count(), 0);
}

#[test]
fn an_online_character_is_saved_every_cadence_even_when_idle() {
    // The safety the change above buys: a character that logs in and stands
    // still is still written, so an item it picked up without moving is not lost
    // at the next restart.
    let mut world = eager();
    let now = Instant::now();
    enter(&mut world, now);
    let _ = world.drain_saves().count();
    world.tick(now + WALK_INTERVAL);
    assert!(
        world.drain_saves().next().is_some(),
        "an idle online character is still saved"
    );
}

#[test]
fn walking_marks_the_character_without_anyone_remembering_to() {
    // The point of reading the bus. Nothing in `walk` mentions the journal:
    // the step is saved because the step was announced.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let connection = enter(&mut world, now);

    let _ = steps(&mut world, connection, Direction::North, 1, now);
    world.take_snapshot();

    let snapshot = only_snapshot(&mut world).expect("a step is a change");
    assert_eq!(snapshot.characters.len(), 1);
    assert_eq!(
        snapshot.characters[0].y,
        START.1 - 1,
        "the snapshot must hold where the step went, not where it started"
    );
}

#[test]
fn turning_is_worth_saving_too() {
    // A turn moves nobody, and a character that logs in facing the wrong way
    // is a small bug that is invisible until someone looks for it.
    let mut world = eager();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let _ = world.drain_saves();

    // One request, one tick: a character spawns facing south, so the first
    // request east turns and goes nowhere.
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::East),
    });
    world.tick(now + WALK_INTERVAL);

    let snapshot = only_snapshot(&mut world).expect("a turn is a change");
    assert_eq!(snapshot.characters[0].x, START.0, "a turn moves nobody");
    assert_eq!(
        snapshot.characters[0].facing,
        Facing::walking(Direction::East).to_bits()
    );
}

#[test]
fn logging_out_saves_where_the_player_actually_stopped() {
    // The test `keep` exists for, and the one a `touch` cannot pass: by the
    // next save the entity is despawned and there is nothing left to read.
    // Getting this wrong loses the whole session and looks like a disk fault.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let connection = enter(&mut world, now);

    let now = steps(&mut world, connection, Direction::North, 2, now);

    world.queue(Command::Disconnect { connection });
    world.tick(now + WALK_INTERVAL);
    assert_eq!(world.player_count(), 0, "and the entity is gone");

    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a session is worth saving");
    assert_eq!(snapshot.characters.len(), 1);
    assert_eq!(
        snapshot.characters[0].y,
        START.1 - 2,
        "two steps north is where the player stopped"
    );
}

#[test]
fn logging_out_does_not_delete_the_character() {
    // Disconnecting is not deleting. The entity goes; the character stays.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let connection = enter(&mut world, now);
    world.queue(Command::Disconnect { connection });
    world.tick(now + WALK_INTERVAL);

    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a change");
    assert!(
        snapshot.removed.is_empty(),
        "a logout must not queue a deletion"
    );
}

#[test]
fn a_world_with_nowhere_to_save_keeps_no_journal_anyone_waits_on() {
    // save_every = 0 is a real mode. What it must not do is quietly grow a
    // journal forever, which is a leak that looks like a working shard.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let connection = enter(&mut world, now);
    steps(&mut world, connection, Direction::North, 4, now);
    assert_eq!(world.drain_saves().count(), 0, "nothing was offered");
    assert!(world.unsaved() > 0, "but it is still tracked, and takeable");

    // And a caller that asks explicitly gets it all.
    world.take_snapshot();
    assert_eq!(
        only_snapshot(&mut world)
            .expect("a change")
            .characters
            .len(),
        1
    );
    assert_eq!(world.unsaved(), 0);
}

#[test]
fn the_snapshot_arrives_on_the_cadence_and_not_before() {
    let mut world = World::new(START).with_save_every(4);
    let now = Instant::now();
    let connection = enter(&mut world, now);

    // enter() ran tick 1. Ticks 2 and 3 offer nothing; tick 4 does.
    world.queue(Command::Walk {
        connection,
        request: walk(0, Direction::North),
    });
    world.tick(now + WALK_INTERVAL);
    assert_eq!(world.drain_saves().count(), 0, "tick 2 is not a save tick");
    world.tick(now + WALK_INTERVAL * 2);
    assert_eq!(world.drain_saves().count(), 0, "nor tick 3");
    world.tick(now + WALK_INTERVAL * 3);
    assert_eq!(world.drain_saves().count(), 1, "tick 4 is");
}

#[test]
fn thirty_steps_in_one_save_window_are_one_row() {
    // What the dirty set buys: a save proportional to activity, not to how
    // chatty the activity was.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let connection = enter(&mut world, now);

    steps(&mut world, connection, Direction::North, 20, now);
    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a change");
    assert_eq!(snapshot.characters.len(), 1, "one character, one row");
}

#[test]
fn a_failed_save_is_retried_with_fresh_data_and_not_the_old_snapshot() {
    // Re-writing the failed snapshot would put the character back where it
    // was when the write began, which is a rollback nobody asked for. The
    // sweep re-reads instead.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let connection = enter(&mut world, now);

    world.take_snapshot();
    let first = only_snapshot(&mut world).expect("a change");
    assert_eq!(first.characters[0].y, START.1);
    assert_eq!(world.unsaved(), 0, "the journal was drained");

    // The store said no.
    world.resweep();

    // And the world kept ticking while the write was failing.
    steps(&mut world, connection, Direction::North, 1, now);

    world.take_snapshot();
    let retry = only_snapshot(&mut world).expect("swept");
    assert_eq!(
        retry.characters[0].y,
        START.1 - 1,
        "the retry must write where the character is now, not where it was"
    );
}

#[test]
fn a_sweep_finds_characters_nothing_has_touched() {
    // The escape hatch has to actually escape: a character that has done
    // nothing since the last save is not dirty, and a sweep must still find
    // it. Otherwise "always correct" is only true for people who moved.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    enter_as(&mut world, ConnectionId::from_raw(1), now);
    enter_as(&mut world, ConnectionId::from_raw(2), now);

    world.take_snapshot();
    let _ = world.drain_saves();
    assert_eq!(world.unsaved(), 0, "nobody is dirty");

    world.resweep();
    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a sweep is a change");
    assert_eq!(snapshot.characters.len(), 2, "including the idle one");
}

#[test]
fn two_players_are_two_rows_in_one_snapshot() {
    // The consistency promise: one drain, one instant, everyone in it.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    enter_as(&mut world, ConnectionId::from_raw(1), now);
    enter_as(&mut world, ConnectionId::from_raw(2), now);

    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a change");
    assert_eq!(snapshot.characters.len(), 2);
    let serials: HashSet<u32> = snapshot.characters.iter().map(|c| c.serial).collect();
    assert_eq!(serials.len(), 2, "and two distinct serials");
}

#[test]
fn a_saved_serial_is_the_one_the_client_was_told() {
    // The serial is on the wire and in every packet the client has been
    // sent. A character that comes back under a different one is a different
    // character with the same name.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = world.state.registry.serial_of(entity).expect("bound");

    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a change");
    assert_eq!(snapshot.characters[0].serial, serial.raw());
}
