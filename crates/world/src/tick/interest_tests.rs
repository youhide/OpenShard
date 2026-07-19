use super::tests::*;
use super::*;
use openshard_movement::WALK_INTERVAL;
use openshard_state::sectors::VIEW_RANGE;

const ALICE: ConnectionId = ConnectionId::from_raw(1);
const BOB: ConnectionId = ConnectionId::from_raw(2);

#[test]
fn two_players_in_the_same_place_see_each_other() {
    // The thing this whole crate has been missing.
    let mut world = World::new(START);
    let now = Instant::now();

    enter_as(&mut world, ALICE, now);
    let _ = world.drain_outbound().count();

    enter_as(&mut world, BOB, now);
    let to_alice = packets_for(&mut world, ALICE);
    assert!(
        to_alice.iter().any(|p| p[0] == 0x78),
        "Alice was never told Bob arrived"
    );
}

#[test]
fn a_newcomer_is_told_about_everyone_already_here() {
    // The other direction, and the one that is easy to forget: arriving is
    // symmetric. Bob's screen starts empty and Alice is already standing
    // there.
    let mut world = World::new(START);
    let now = Instant::now();

    enter_as(&mut world, ALICE, now);
    enter_as(&mut world, BOB, now);

    // Bob is drawn his own equipment in a 0x78 about himself now, so count
    // only the ones that are about Alice.
    let alice = world
        .registry()
        .serial_of(world.state.players[&ALICE])
        .unwrap()
        .raw()
        .to_be_bytes();
    let to_bob = packets_for(&mut world, BOB);
    let drawn = to_bob
        .iter()
        .filter(|p| p[0] == 0x78 && p.windows(4).any(|w| w == alice))
        .count();
    assert_eq!(drawn, 1, "Bob should be drawn Alice, exactly once");
}

#[test]
fn a_mobile_is_drawn_once_however_much_it_walks() {
    // The reason the server remembers what it sent. Without `seen`, every
    // step would redraw the mobile from scratch and the client would flicker.
    let mut world = World::new(START);
    let now = Instant::now();
    enter_as(&mut world, ALICE, now);
    enter_as(&mut world, BOB, now);
    let _ = world.drain_outbound().count();

    let mut drawn = 0;
    let mut moved = 0;
    for step in 1..=5u32 {
        world.queue(Command::Walk {
            connection: BOB,
            request: WalkRequest {
                facing: Facing::walking(Direction::South),
                sequence: (step - 1) as u8,
                fastwalk_key: 0,
            },
        });
        world.tick(now + WALK_INTERVAL * step);
        for packet in packets_for(&mut world, ALICE) {
            match packet[0] {
                0x78 => drawn += 1,
                0x77 => moved += 1,
                _ => {}
            }
        }
    }
    assert_eq!(drawn, 0, "Bob was redrawn mid-walk");
    assert!(moved > 0, "Alice never saw Bob move");
}

#[test]
fn walking_out_of_range_removes_the_mobile() {
    let mut world = World::new(START);
    let now = Instant::now();
    enter_as(&mut world, ALICE, now);
    enter_as(&mut world, BOB, now);
    let _ = world.drain_outbound().count();

    // Well past the view range.
    teleport(
        &mut world,
        BOB,
        Point::new(START.0 + VIEW_RANGE as u16 + 5, START.1, Z_WITHOUT_A_MAP),
    );

    let to_alice = packets_for(&mut world, ALICE);
    assert!(
        to_alice.iter().any(|p| p[0] == 0x1D),
        "Bob walked away and stayed on Alice's screen forever"
    );
}

#[test]
fn walking_back_into_range_draws_it_again() {
    let mut world = World::new(START);
    let now = Instant::now();
    enter_as(&mut world, ALICE, now);
    enter_as(&mut world, BOB, now);

    let far = Point::new(START.0 + VIEW_RANGE as u16 + 5, START.1, Z_WITHOUT_A_MAP);
    teleport(&mut world, BOB, far);
    let _ = world.drain_outbound().count();

    teleport(
        &mut world,
        BOB,
        Point::new(START.0, START.1, Z_WITHOUT_A_MAP),
    );
    let to_alice = packets_for(&mut world, ALICE);
    assert!(
        to_alice.iter().any(|p| p[0] == 0x78),
        "Bob came back and was never redrawn"
    );
}

#[test]
fn removal_is_sent_once_not_every_tick() {
    // `forget` returning early when nothing was removed is what stops a
    // 0x1D per tick for a mobile that left a minute ago.
    let mut world = World::new(START);
    let now = Instant::now();
    enter_as(&mut world, ALICE, now);
    enter_as(&mut world, BOB, now);

    let far = Point::new(START.0 + VIEW_RANGE as u16 + 5, START.1, Z_WITHOUT_A_MAP);
    teleport(&mut world, BOB, far);
    let _ = world.drain_outbound().count();

    // Move again, still out of range.
    teleport(&mut world, BOB, Point::new(far.x + 1, far.y, far.z));
    let removes = packets_for(&mut world, ALICE)
        .iter()
        .filter(|p| p[0] == 0x1D)
        .count();
    assert_eq!(removes, 0, "a second removal for a mobile already gone");
}

#[test]
fn a_player_is_never_sent_itself() {
    // Sphere's own comment: 0x77 cannot move the receiving client's
    // character. Sending one is invisible and puts the two ends a tile apart.
    let mut world = World::new(START);
    let now = Instant::now();
    enter_as(&mut world, ALICE, now);
    let _ = world.drain_outbound().count();

    world.queue(Command::Walk {
        connection: ALICE,
        request: WalkRequest {
            facing: Facing::walking(Direction::South),
            sequence: 0,
            fastwalk_key: 0,
        },
    });
    world.tick(now);

    let ids: Vec<u8> = packets_for(&mut world, ALICE)
        .iter()
        .map(|p| p[0])
        .collect();
    assert!(!ids.contains(&0x78), "Alice was drawn to herself");
    assert!(
        !ids.contains(&0x77),
        "Alice was moved for herself; 0x20 does that"
    );
}

#[test]
fn leaving_takes_the_mobile_off_every_screen() {
    let mut world = World::new(START);
    let now = Instant::now();
    enter_as(&mut world, ALICE, now);
    enter_as(&mut world, BOB, now);
    let _ = world.drain_outbound().count();

    world.queue(Command::Disconnect { connection: BOB });
    world.tick(now);

    let to_alice = packets_for(&mut world, ALICE);
    assert!(
        to_alice.iter().any(|p| p[0] == 0x1D),
        "Bob logged out and stayed on Alice's screen"
    );
}

#[test]
fn leaving_removes_the_watcher_bookkeeping_too() {
    // A `seen` set that outlives its player is a slow leak: every login
    // leaves one behind and `watchers_of` walks them all forever.
    let mut world = World::new(START);
    let now = Instant::now();
    enter_as(&mut world, ALICE, now);
    enter_as(&mut world, BOB, now);
    assert_eq!(world.state.seen.len(), 2);

    world.queue(Command::Disconnect { connection: BOB });
    world.tick(now);

    assert_eq!(world.state.seen.len(), 1, "Bob's screen outlived Bob");
    assert_eq!(
        world.sectors().len(),
        1,
        "and so did his place in the index"
    );
}

#[test]
fn the_index_never_disagrees_with_the_position() {
    // Two copies of where something is. The tick is what keeps them in step,
    // and this is the assertion that says so.
    let mut world = World::new(START);
    let start = Instant::now();
    let alice = enter_as(&mut world, ALICE, start);
    let entity = world.state.players[&alice];

    for step in 1..=50u32 {
        world.queue(Command::Walk {
            connection: alice,
            request: WalkRequest {
                facing: Facing::walking(Direction::South),
                sequence: (step - 1) as u8,
                fastwalk_key: 0,
            },
        });
        world.tick(start + WALK_INTERVAL * step);

        let Position(position) = *world.registry().get::<Position>(entity).unwrap();
        assert_eq!(
            world.sectors().position_of(entity),
            Some(position),
            "the index drifted from the component at step {step}"
        );
    }
}

#[test]
fn two_hundred_players_in_one_place_do_not_stop_the_tick() {
    // Not a benchmark — a shape check. Everyone sees everyone here, so the
    // work really is quadratic in the crowd; what the index buys is that a
    // crowd in Britain costs nothing to a player in Vesper.
    let mut world = World::new(START);
    let now = Instant::now();
    for id in 0..200u64 {
        enter_as(&mut world, ConnectionId::from_raw(id + 1), now);
    }
    assert_eq!(world.player_count(), 200);
    let _ = world.drain_outbound().count();

    // One far away: its refresh must not touch the crowd at all.
    let loner = ConnectionId::from_raw(1000);
    enter_as(&mut world, loner, now);
    teleport(&mut world, loner, Point::new(6000, 3000, Z_WITHOUT_A_MAP));
    let _ = world.drain_outbound().count();

    teleport(&mut world, loner, Point::new(6001, 3000, Z_WITHOUT_A_MAP));
    assert_eq!(
        world.drain_outbound().count(),
        0,
        "a step in Vesper sent packets to a crowd in Britain"
    );
}
