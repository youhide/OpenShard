use super::tests::{START, authenticate, delete_slot, enter, enter_as, walk, world};
use super::*;
use openshard_gateway::ConnectionId;
use openshard_movement::WALK_INTERVAL;
use openshard_protocol::wire::Graphic;

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

/// The shutdown notice reaches everyone the world considers to be in it, and
/// stops there.
///
/// The second half is the point. A connection that has authenticated and not
/// yet picked a character — or has picked one and is waiting for the tick that
/// creates it — has no entity, so there is nobody to say anything *to*: it gets
/// the hang-up and no line, which is correct and was pinned nowhere. `announce`
/// walks `players` rather than the registry, and the two differ exactly here;
/// walking anything wider would either address a connection with no body or
/// speak to `Client` components that outlived the connections playing them.
///
/// `docs/shutdown.md` D4 and its backlog entry, "a stop mid-`Entering` is
/// untested".
#[test]
fn a_shutdown_notice_reaches_the_world_and_nobody_on_the_way_into_it() {
    let mut world = world();
    let now = Instant::now();
    let inside = enter(&mut world, now);
    // Authenticated, on the character screen, no entity: the state a connection
    // is in for the whole of the login conversation after its password checks
    // out, and the one a stop can land in the middle of.
    let arriving = ConnectionId::from_raw(77);
    authenticate(&mut world, arriving, now);
    // Everything the two logins produced, so that what is left is the
    // announcement. One drain: `drain_outbound` empties the queue, so a second
    // call would be reading an already-empty world rather than a second
    // connection's mail.
    //
    // The screen's own packets are asserted on the way past, because without
    // that the second assertion below would hold for a connection the world
    // simply cannot address — it would pass where `announce` is right and
    // equally where nothing works at all.
    let before: Vec<_> = world.drain_outbound().collect();
    assert!(
        before.iter().any(|out| out.connection == arriving),
        "the character screen answered this connection, so the world can reach it"
    );

    world.announce("the shard is stopping");

    // One drain, then partitioned: `drain_outbound` empties the queue, so asking
    // it twice would answer the second question about an already-empty world.
    let sent: Vec<_> = world.drain_outbound().collect();
    assert!(
        sent.iter().any(|out| out.connection == inside
            && out.packet[0] == 0x1C
            && String::from_utf8_lossy(&out.packet).contains("the shard is stopping")),
        "a player in the world is told why it is going"
    );
    assert!(
        !sent.iter().any(|out| out.connection == arriving),
        "a connection with no character is told nothing: there is no body to speak to"
    );
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
fn deleting_a_character_forgets_its_row_on_the_next_save() {
    // The `0x83` path: once a character is deleted, the next snapshot names its
    // serial in `removed`, and the store drops the row and its inventory.
    let mut world = eager();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let serial = world
        .state
        .registry
        .serial_of(world.state.players[&connection])
        .unwrap()
        .raw();
    // Log it out first: a deleted character is one at the select screen, not in
    // the world, and the save that carries the deletion comes after the logout.
    world.queue(Command::Disconnect { connection });
    world.tick(now + WALK_INTERVAL);
    let _ = world.drain_saves().count();

    // By slot, through the command: the slot indexes the list the screen was
    // sent, which the world built out of the same roster this looks the character
    // up in. See `docs/connection_state.md`, S5.
    delete_slot(&mut world, 0, now + WALK_INTERVAL * 2);
    let snapshot = only_snapshot(&mut world).expect("a deletion is a change worth saving");
    assert!(
        snapshot.removed.contains(&serial),
        "the deleted serial is marked removed so the store drops it"
    );
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
    assert!(snapshot.removed.is_empty(), "a logout must not queue a deletion");
}

#[test]
fn a_dead_players_save_is_flagged_but_keeps_the_living_body() {
    // A ghost is saved as living-and-dead: the row carries the living body (so
    // resurrection restores it) and a `dead` flag (so the relog rises a ghost).
    let mut world = eager();
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let entity = world.state.players[&connection];
    let serial = world.registry().serial_of(entity).unwrap();
    let _ = only_snapshot(&mut world); // drop the enter's save

    world.queue(Command::Damage {
        serial,
        amount: 500,
        damage_type: 0,
        by: None,
    });
    world.tick(now);

    let snapshot = only_snapshot(&mut world).expect("death is a change worth saving");
    let record = snapshot
        .characters
        .iter()
        .find(|c| c.serial == serial)
        .expect("the ghost is in the save");
    assert!(record.dead, "saved as dead");
    assert_eq!(
        record.body, 0x0190,
        "with the living body, not the grey ghost one"
    );
}

#[test]
fn a_character_that_logged_out_dead_returns_a_ghost() {
    // The other half: a saved `dead` character re-enters a ghost, grey body and
    // all, without a fresh corpse (its own still lies where it fell).
    let mut world = eager();
    let now = Instant::now();
    let connection = ConnectionId::from_raw(7);
    world.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Revenant".to_owned()),
        access: AccessLevel::Player,
        character: Character::Fresh(FreshCharacter {
            facet: Facet(0),
            start: None,
            appearance: Some(Appearance {
                body: Graphic(0x0190),
                hue: openshard_protocol::wire::Hue::NONE,
            }),
            sheet: Some(Box::new(CharacterSheet {
                strength: 100,
                dexterity: 100,
                intelligence: 100,
                skills: Vec::new(),
                effects: Vec::new(),
                stat_locks: Default::default(),
                dead: true,
                fame: 0,
                karma: 0,
                murders: 0,
                quests: Vec::new(),
                done_quests: Vec::new(),
                guild: None,
                guild_candidate: None,
            })),
        }),
    }));
    world.tick(now);

    let entity = world.state.players[&connection];
    assert!(world.registry().has::<Ghost>(entity), "re-enters a ghost");
    assert_eq!(
        world.registry().get::<Body>(entity).map(|b| b.id.0),
        Some(0x0192),
        "in the ghost body"
    );
    assert_eq!(
        world.registry().get::<Ghost>(entity).map(|g| g.body.id.0),
        Some(0x0190),
        "remembering the living body to resurrect back to"
    );
    // No fresh corpse laid on re-entry — that would duplicate the saved one.
    assert!(
        world
            .registry()
            .query::<Drawn>()
            .all(|(_, g)| g.id != Graphic(0x2006)),
        "no corpse is laid on relog"
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
    assert_eq!(only_snapshot(&mut world).expect("a change").characters.len(), 1);
    assert_eq!(world.unsaved(), 0);
}

#[test]
fn a_restart_continues_the_roll_stream_instead_of_dealing_it_again() {
    // The bug this is here for is not "the rolls differ across a restart" — it is
    // that without saving the generator they are *identical*. A shard re-seeded from
    // a constant at every boot replays the sequence it played last run, in order, so
    // a player who dislikes a roll has a way to ask for it again: get the shard
    // restarted. That makes it an exploit, not a cosmetic loss.
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    enter(&mut world, now);
    // Some play, so the stream is plainly not at its seed any more.
    let rolled: Vec<u32> = (0..64).map(|_| world.state.rng.below(1000)).collect();

    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a character entered");
    let saved = snapshot
        .world
        .expect("a full sweep carries the world's own scalars");
    assert_eq!(
        saved.rng_state,
        world.rng_state(),
        "the save carries where the rolls got to"
    );

    // The shard comes back up on that save.
    let mut restored = World::new(START).with_rng_state(saved.rng_state);
    let after_restart: Vec<u32> = (0..64).map(|_| restored.state.rng.below(1000)).collect();
    let kept_running: Vec<u32> = (0..64).map(|_| world.state.rng.below(1000)).collect();
    assert_eq!(
        after_restart, kept_running,
        "a restored world must roll on from the save, not from a seed"
    );
    assert_ne!(
        after_restart, rolled,
        "and must not deal the pre-save rolls a second time"
    );

    // The gate above would also pass if `with_rng_state` were a no-op and both
    // worlds simply started from the same default seed, so pin that it is not.
    let mut fresh = World::new(START);
    let from_the_default_seed: Vec<u32> = (0..64).map(|_| fresh.state.rng.below(1000)).collect();
    assert_ne!(
        after_restart, from_the_default_seed,
        "restoring a state has to actually move the generator off its default seed"
    );
}

#[test]
fn a_pinned_seed_decides_a_fresh_worlds_rolls() {
    // What `world.seed` buys an operator: two fresh worlds built the same way roll
    // the same, and a different seed rolls differently. Only fresh ones — a world
    // with a save behind it resumes, and the test above is why.
    let rolls = |seed: u64| -> Vec<u32> {
        let mut world = World::new(START).with_seed(seed);
        (0..32).map(|_| world.state.rng.below(1000)).collect()
    };
    assert_eq!(rolls(0x1234_5678_9ABC_DEF0), rolls(0x1234_5678_9ABC_DEF0));
    assert_ne!(rolls(0x1234_5678_9ABC_DEF0), rolls(0x0FED_CBA9_8765_4321));
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
    let serials: HashSet<Serial> = snapshot.characters.iter().map(|c| c.serial).collect();
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
    assert_eq!(snapshot.characters[0].serial, serial);
}

/// A guild, its wars, its roster and its id counter all come back.
///
/// The one that matters most is the counter, and it is the one nothing about the
/// guilds themselves would reveal: a disbanded guild leaves no row, so the
/// maximum id in the table is *not* the maximum ever issued. A shard that
/// re-derived it would hand the next guild founded an id a disbanded one had
/// used, and every member record still naming it — anyone offline at the time,
/// and so never swept — would silently join the new guild.
#[test]
fn a_guild_survives_a_restart_and_its_ids_are_not_handed_out_again() {
    use openshard_state::GuildMember;

    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player).expect("a serial");

    let ours =
        openshard_guilds::found(&mut world.state, player, "The Silver Serpent", "OSS").expect("a guild");
    openshard_guilds::set_title(&mut world.state, player, player, "Warlord").expect("a title");
    // A second guild, then a war with it — and a third, disbanded, so the id
    // counter and the table plainly disagree.
    let theirs = world
        .state
        .guilds
        .found("The Black Rose".to_owned(), "TBR".to_owned(), serial);
    world.state.guilds.declare_war(ours, theirs);
    world.state.guilds.declare_war(theirs, ours);
    let doomed = world
        .state
        .guilds
        .found("The Ash".to_owned(), "ASH".to_owned(), serial);
    world.state.guilds.disband(doomed);
    let high = world.state.guilds.high_water();
    assert!(high > theirs.0, "the disbanded guild took an id with it");

    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a character entered");
    let guilds = snapshot.guilds.expect("a full sweep carries the guilds");
    assert_eq!(
        guilds.len(),
        2,
        "the disbanded one is absent, which is the delete"
    );
    let saved_world = snapshot.world.expect("and the world's own scalars");
    assert_eq!(saved_world.guild_high_water, high);

    // The membership rides with the character, not with the guild.
    let character = snapshot
        .characters
        .iter()
        .find(|record| record.serial == serial)
        .expect("the player was swept");
    assert_eq!(character.guild, Some(ours.0));
    assert_eq!(character.guild_title, "Warlord");

    // The shard comes back up on that save.
    let mut restored = World::new(START);
    restored.restore_guilds(guilds);
    let mut restored = restored.with_guild_high_water(saved_world.guild_high_water);
    assert_eq!(
        restored.state.guilds.get(ours).map(|g| g.name.as_str()),
        Some("The Silver Serpent")
    );
    assert!(
        restored.state.guilds.get(ours).unwrap().at_war_with(theirs),
        "the war did not survive the door"
    );
    assert!(restored.state.guilds.get(doomed).is_none());

    // And the counter: the next guild founded must not take the disbanded one's
    // id, which is exactly what re-deriving from the table would have done.
    let next = restored
        .state
        .guilds
        .found("The Fourth".to_owned(), "FTH".to_owned(), serial);
    assert!(next.0 > high, "{next:?} re-used an id that was already issued");
    assert_ne!(next, doomed);

    // And the member comes back a member through the ordinary login path — the
    // saved sheet, not a component put back by hand. That is the half that was
    // easy to leave out: the guild table can restore perfectly and still leave
    // every player unguilded.
    let connection = ConnectionId::from_raw(77);
    restored.queue(Command::Enter(Entering {
        connection,
        version: ClientVersion::TOL,
        account: AccountName("admin".to_owned()),
        name: CharacterName("Wilbur".to_owned()),
        access: AccessLevel::Player,
        character: Character::Fresh(FreshCharacter {
            facet: Facet(0),
            start: None,
            appearance: None,
            sheet: Some(Box::new(CharacterSheet {
                strength: 100,
                dexterity: 100,
                intelligence: 100,
                skills: Vec::new(),
                effects: Vec::new(),
                stat_locks: Default::default(),
                dead: false,
                fame: 0,
                karma: 0,
                murders: 0,
                quests: Vec::new(),
                done_quests: Vec::new(),
                guild: Some(crate::tick::command::GuildSeat {
                    guild: ours,
                    title: "Warlord".to_owned(),
                    rank: openshard_state::Rank::Emissary,
                }),
                guild_candidate: Some(theirs.0),
            })),
        }),
    }));
    restored.tick(now);
    let entity = restored.state.players[&connection];
    assert_eq!(
        restored.state.guild_of(entity).map(|g| g.abbreviation.as_str()),
        Some("OSS"),
        "a restored membership named a guild that was not there"
    );
    assert_eq!(
        restored
            .registry()
            .get::<GuildMember>(entity)
            .map(|m| m.title.as_str()),
        Some("Warlord")
    );
    // And the rank, which is the half of this the title looks like and is not:
    // the record above wears the *title* "Warlord" and holds the **Emissary**
    // rank, so a restore that read one as the other would come back looking
    // right and holding the wrong permissions.
    assert_eq!(
        restored.registry().get::<GuildMember>(entity).map(|m| m.rank),
        Some(openshard_state::Rank::Emissary)
    );
    // An invitation left for a player who was offline is exactly the invitation
    // that has to survive a restart.
    assert_eq!(
        restored
            .registry()
            .get::<openshard_state::GuildCandidate>(entity)
            .map(|asked| asked.guild),
        Some(theirs)
    );
    assert_eq!(
        restored.state.guild_label(entity).as_deref(),
        Some("[Warlord, OSS]")
    );
}

/// An alliance is a body of its own, and the guild column is only a
/// back-pointer to it.
///
/// The half that is easy to leave out: the `guilds` table can restore perfectly
/// and leave every alliance gone, and the guilds would come back naming ids that
/// address nothing. That is not a crash — `guild_of`'s rule reads it as no
/// alliance — but it is silently a shard whose alliances all dissolved over a
/// restart, so the pending guild rides too, and so does the id counter.
#[test]
fn an_alliance_survives_a_restart_with_its_membership_and_its_counter() {
    let mut world = World::new(START).with_save_every(0);
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let serial = world.state.registry.serial_of(player).expect("a serial");

    let ours =
        openshard_guilds::found(&mut world.state, player, "The Silver Serpent", "OSS").expect("a guild");
    let second = world
        .state
        .guilds
        .found("The Black Rose".to_owned(), "TBR".to_owned(), serial);
    let third = world
        .state
        .guilds
        .found("The Grey Owl".to_owned(), "TGO".to_owned(), serial);

    // Two members and one still being asked, because the pending guild is the
    // part a save of "the members" would drop.
    let alliance = world
        .state
        .alliances
        .found("The Northern Compact".to_owned(), ours, second);
    world.state.alliances.accept(alliance, second);
    world.state.alliances.ask(alliance, third);
    for guild in [ours, second] {
        world.state.guilds.get_mut(guild).unwrap().alliance = Some(alliance);
    }
    // A second alliance, disbanded, so the counter and the table disagree the way
    // the guild table's already does.
    let doomed = world
        .state
        .alliances
        .found("The Ash Pact".to_owned(), third, ours);
    world.state.alliances.remove(doomed, third);
    let high = world.state.alliances.high_water();
    assert!(high > alliance.0, "the disbanded alliance took an id with it");

    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a character entered");
    let guilds = snapshot.guilds.expect("a full sweep carries the guilds");
    let alliances = snapshot.alliances.expect("and the alliances");
    assert_eq!(alliances.len(), 1, "the disbanded one is absent");
    let saved_world = snapshot.world.expect("and the world's own scalars");
    assert_eq!(saved_world.alliance_high_water, high);

    let mut restored = World::new(START);
    restored.restore_guilds(guilds);
    restored.restore_alliances(alliances);
    let mut restored = restored.with_alliance_high_water(saved_world.alliance_high_water);
    assert!(
        restored.state.allied(ours, second),
        "two guilds came back in no alliance"
    );
    assert!(
        !restored.state.allied(ours, third),
        "a guild that had only been asked came back a member"
    );
    let back = restored.state.alliances.get(alliance).expect("it was saved");
    assert_eq!(back.name, "The Northern Compact");
    assert_eq!(back.leader, ours);
    assert!(back.pending.contains(&third), "the standing question was dropped");

    // And the counter, for the guild table's reason: a reissued id would put a
    // guild into a body it never joined.
    let next = restored
        .state
        .alliances
        .found("The Fourth".to_owned(), ours, second);
    assert!(next.0 > high, "{next:?} re-used an id that was already issued");
}

/// A house survives a restart, and its walls come back with it.
///
/// The half that is easy to leave out is the walls: the entity can restore
/// perfectly and stop nobody, because the footprint is *not* saved — a multi's
/// shape lives in the client's files and saving a copy would go stale the day
/// the operator updates their install. So the record is the id and the position,
/// and the obstruction index is rebuilt from them. A test that only checked the
/// `House` component came back would pass on a shard you can walk through.
#[test]
fn a_house_survives_a_restart_with_its_walls() {
    use openshard_uofiles::multi::Component;

    const COTTAGE: u16 = 0x64;
    const WALL: u16 = 0x0006;
    const FLOOR: u16 = 0x0007;

    /// One multi: two walls and a floor. The floor is here so the restore is
    /// asserted to keep dropping it, not merely to add walls — and it is the
    /// tiledata below, not this list, that says which of the three is a wall.
    fn cottage() -> Vec<Component> {
        [(WALL, -1), (WALL, 1), (FLOOR, 0)]
            .into_iter()
            .map(|(graphic, dx)| Component {
                graphic: openshard_protocol::wire::Graphic(graphic),
                dx,
                dy: 0,
                dz: 0,
                flags: 1,
            })
            .collect()
    }

    let mut world = World::new(START).with_save_every(0);
    world
        .state
        .set_tiles(super::tests::tiles_with(&[(WALL, super::tests::WALL_FLAGS, 20)]));
    world.state.multis = super::tests::multis_with(COTTAGE, cottage());
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).expect("a serial");

    let at = Point::new(START.0 + 5, START.1 + 5, 0);
    let house = openshard_housing::place(
        &mut world.state,
        player,
        at,
        Facet(0),
        openshard_protocol::wire::MultiId(COTTAGE),
        owner,
    )
    .expect("a legal spot");
    let serial = world.state.registry.serial_of(house).expect("a house serial");

    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a character entered");
    let houses = snapshot.houses.expect("a full sweep carries the houses");
    assert_eq!(houses.len(), 1);
    assert_eq!(houses[0].serial, serial);
    assert_eq!(houses[0].multi, COTTAGE);
    assert_eq!((houses[0].x, houses[0].y, houses[0].z), (at.x, at.y, at.z));
    assert_eq!(houses[0].owner, owner);

    // The shard comes back up on that save, with the same terrain.
    let mut restored = World::new(START);
    restored
        .state
        .set_tiles(super::tests::tiles_with(&[(WALL, super::tests::WALL_FLAGS, 20)]));
    restored.state.multis = super::tests::multis_with(COTTAGE, cottage());
    restored.restore_houses(houses, Vec::new());

    let back = restored
        .state
        .registry
        .entity_of(serial)
        .expect("the house came back under its own serial");
    assert_eq!(
        restored
            .state
            .registry
            .get::<openshard_state::components::House>(back)
            .map(|h| h.multi),
        Some(openshard_protocol::wire::MultiId(COTTAGE))
    );
    let obstructions = &restored.state.facet_state(Facet(0)).obstructions();
    assert!(
        obstructions.blocker_at_z(at.x - 1, at.y, 0).is_some(),
        "a restored house has no walls, so it is a picture and not a building"
    );
    assert!(
        obstructions.blocker_at_z(at.x + 1, at.y, 0).is_some(),
        "the second wall did not come back"
    );
    assert!(
        obstructions.blocker_at_z(at.x, at.y, 0).is_none(),
        "the floor was folded in on the way back, sealing the house shut"
    );
}

/// Who may come in survives the restart too.
///
/// Its own test rather than a line in the one above, because the three lists are
/// the reason schema v28 exists: a build that knew about houses and not about
/// their lists would read a house, drop them, and write it back — which is not a
/// shard with no lists, it is a shard that deletes them on the first save.
#[test]
fn a_houses_access_lists_survive_a_restart() {
    use openshard_persistence::record::HouseRecord;

    let mut world = World::new(START);
    let serial = Serial::new(0x4000_00AA).expect("an item serial");
    let owner = Serial::new(0x0000_0001).expect("a mobile serial");
    let co_owner = Serial::new(0x0000_0002).expect("a mobile serial");
    let friend = Serial::new(0x0000_0003).expect("a mobile serial");
    let banned = Serial::new(0x0000_0004).expect("a mobile serial");

    world.restore_houses(
        vec![HouseRecord {
            serial,
            multi: 0x64,
            x: START.0 + 5,
            y: START.1 + 5,
            z: 0,
            facet: 0,
            owner,
            co_owners: vec![co_owner.raw()],
            friends: vec![friend.raw()],
            // A serial no pool can produce, to prove the filter is a filter and not
            // a silent zero: a name this engine cannot read is one it cannot act on.
            bans: vec![banned.raw(), 0],
            lockdowns: 208,
            age: 0,
        }],
        Vec::new(),
    );

    let entity = world.state.registry.entity_of(serial).expect("the house");
    let house = world
        .state
        .registry
        .get::<openshard_state::components::House>(entity)
        .expect("its component");
    assert_eq!(
        house.standing_of(co_owner, false),
        openshard_state::Standing::CoOwner
    );
    assert_eq!(
        house.standing_of(friend, false),
        openshard_state::Standing::Friend
    );
    assert_eq!(
        house.standing_of(banned, false),
        openshard_state::Standing::Banned
    );
    assert_eq!(
        house.bans.len(),
        1,
        "the unreadable serial became a ban on nobody"
    );

    // And back out again, so the sweep carries what the restore read.
    let records = world.house_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].co_owners, vec![co_owner.raw()]);
    assert_eq!(records[0].friends, vec![friend.raw()]);
    assert_eq!(records[0].bans, vec![banned.raw()]);
}

/// A shard booted without client files keeps its houses and gives them no walls.
///
/// Not a crash and not a silent demolition: the entity is there and owned, and
/// the only thing missing is the half that came from a file the shard cannot
/// read. Said out loud here because the alternative — dropping the house — would
/// lose somebody's property over a misconfigured `world.client_files`.
#[test]
fn a_house_restored_without_client_files_stands_but_stops_nobody() {
    use openshard_persistence::record::HouseRecord;

    let mut world = World::new(START);
    let serial = Serial::new(0x4000_0099).expect("an item serial");
    let owner = Serial::new(0x0000_0001).expect("a mobile serial");
    world.restore_houses(
        vec![HouseRecord {
            serial,
            multi: 0x64,
            x: START.0 + 5,
            y: START.1 + 5,
            z: 0,
            facet: 0,
            owner,
            co_owners: Vec::new(),
            friends: Vec::new(),
            bans: Vec::new(),
            lockdowns: 0,
            age: 0,
        }],
        Vec::new(),
    );

    let back = world
        .state
        .registry
        .entity_of(serial)
        .expect("the house is still a house");
    assert_eq!(
        world
            .state
            .registry
            .get::<openshard_state::components::House>(back)
            .map(|h| h.owner),
        Some(owner),
        "the owner was lost with the walls"
    );
    assert!(
        !world
            .state
            .facet_state(Facet(0))
            .obstructions()
            .holds_anything(START.0 + 4, START.1 + 5),
        "a shard with no multi table invented a wall"
    );
}

/// A house is one record, not two.
///
/// The house entity is a drawable thing standing on the ground with a graphic
/// and a position, which is exactly what `ground_items` collects — so it was
/// swept up as an item *as well as* written as a [`HouseRecord`], and the
/// restore, which puts the houses back before the items, then found its own
/// serial already spoken for.
///
/// The sign is out on the same terms and a different reason: it is *derived*
/// from the house, rebuilt by `restore_houses` from the record, and an item copy
/// of it would come back as a graphic that no longer opens a window.
#[test]
fn a_house_and_its_sign_are_not_also_ground_items() {
    use openshard_persistence::record::HouseRecord;

    let mut world = World::new(START);
    let serial = Serial::new(0x4000_00BB).expect("an item serial");
    let owner = Serial::new(0x0000_0001).expect("a mobile serial");
    world.restore_houses(
        vec![HouseRecord {
            serial,
            multi: 0x64,
            x: START.0 + 5,
            y: START.1 + 5,
            z: 0,
            facet: 0,
            owner,
            co_owners: Vec::new(),
            friends: Vec::new(),
            bans: Vec::new(),
            lockdowns: 0,
            age: 0,
        }],
        Vec::new(),
    );

    let ground = world.ground_items();
    assert!(
        !ground.iter().any(|record| record.serial == serial),
        "the house was saved as an item as well as a house"
    );
    let signs = world
        .state
        .registry
        .query::<openshard_state::components::HouseSign>()
        .filter_map(|(sign, _)| world.state.registry.serial_of(sign))
        .collect::<Vec<_>>();
    for sign in signs {
        assert!(
            !ground.iter().any(|record| record.serial == sign),
            "the sign was saved as an item"
        );
    }
}

/// A designed house restores with **its own** walls, on a shard with no client
/// files at all.
///
/// This is the one place H1's stated bargain gets *better* rather than worse.
/// H1 accepted that a shard booted without client files restores its houses and
/// gives them no walls, because the walls come from a file. A design does not
/// come from a file — it is the original — so it needs no terrain to be read
/// back, and this world has none.
///
/// `World::new` installs no terrain, which is what makes the assertion mean
/// something: a classic house here would have no footprint at all.
#[test]
fn a_designed_house_restores_its_own_walls_with_no_client_files() {
    use openshard_persistence::record::{HouseDesignRecord, HouseRecord};

    let mut world = World::new(START);
    let serial = Serial::new(0x4000_00CC).expect("an item serial");
    let owner = Serial::new(0x0000_0001).expect("a mobile serial");
    let at = (START.0 + 5, START.1 + 5);

    // One wall, one tile east of the origin. `flags` non-zero is "drawn" — the
    // reader normalises both multi formats' opposite senses before a design is
    // ever built from one.
    let wall = HouseDesignRecord {
        house: serial,
        revision: 7,
        graphic: 0x0006,
        dx: 1,
        dy: 0,
        dz: 0,
        flags: 1,
    };
    world.restore_houses(
        vec![HouseRecord {
            serial,
            multi: 0x64,
            x: at.0,
            y: at.1,
            z: 0,
            facet: 0,
            owner,
            co_owners: Vec::new(),
            friends: Vec::new(),
            bans: Vec::new(),
            lockdowns: 0,
            age: 0,
        }],
        vec![wall],
    );

    let entity = world.state.registry.entity_of(serial).expect("the house");
    let design = world
        .state
        .registry
        .get::<openshard_state::components::HouseDesign>(entity)
        .expect("its design came back");
    assert_eq!(design.revision, 7, "the cache key did not survive the restart");
    assert_eq!(design.components.len(), 1);
    assert_eq!(design.components[0].graphic, Graphic(0x0006));
    assert_eq!(design.components[0].dx, 1);
}

/// A classic house carries no design, and writes no design rows.
///
/// The other half, and the one that keeps the common case free: every house on
/// every shard today is a classic multi, and the design table must stay empty
/// for all of them rather than filling with a copy of what the client files
/// already say.
#[test]
fn a_classic_house_writes_no_design_rows() {
    use openshard_persistence::record::HouseRecord;

    let mut world = World::new(START);
    let serial = Serial::new(0x4000_00CD).expect("an item serial");
    let owner = Serial::new(0x0000_0001).expect("a mobile serial");
    world.restore_houses(
        vec![HouseRecord {
            serial,
            multi: 0x64,
            x: START.0 + 5,
            y: START.1 + 5,
            z: 0,
            facet: 0,
            owner,
            co_owners: Vec::new(),
            friends: Vec::new(),
            bans: Vec::new(),
            lockdowns: 0,
            age: 0,
        }],
        Vec::new(),
    );

    let entity = world.state.registry.entity_of(serial).expect("the house");
    assert!(
        !world
            .state
            .registry
            .has::<openshard_state::components::HouseDesign>(entity),
        "a classic house was given a design"
    );
    assert!(
        world.house_design_records().is_empty(),
        "a classic house wrote design rows"
    );
}

/// **A ship survives a restart with its deck under it.**
///
/// The half that is easy to leave out is the same one a house's walls are: the
/// entity can come back perfectly and carry nobody, because the hull-and-deck
/// split is *not* saved — a boat's shape is a pure function of its multi id, so
/// what is saved is the id and the position and the split is recomputed. A test
/// that only checked the `Boat` component came back would pass on a shard whose
/// ships you fall straight through.
#[test]
fn a_boat_survives_a_restart_with_its_deck() {
    use openshard_movement::scene::Scene;
    use openshard_tiles::TileFlags;
    use openshard_uofiles::multi::Component;

    const SLOOP: u16 = 0x0C;
    const HULL: u16 = 0x3E4E;
    const DECK: u16 = 0x3E4A;

    /// A sloop: a hull plank and a deck plank. Which of the two is a wall and how
    /// tall each stands is the tiledata's answer, not this list's.
    fn sloop() -> Vec<Component> {
        [(HULL, -1), (DECK, 0)]
            .into_iter()
            .map(|(graphic, dx)| Component {
                graphic: openshard_protocol::wire::Graphic(graphic),
                dx,
                dy: 0,
                dz: 0,
                flags: 1,
            })
            .collect()
    }

    /// Open water with a jetty at [`START`], and the two planks a sloop is made
    /// of in the same table the ground reads.
    ///
    /// **The shore is not decoration.** The double this replaced said every tile
    /// was water *and* every step was allowed, which is not a world that can
    /// exist: water is a surface only a swimmer stands on, so a shard that is
    /// water everywhere has nowhere to put the character who launches the ship.
    /// Real ground makes that contradiction a compile-and-run answer instead of
    /// a fixture agreeing with itself.
    fn sea() -> Scene {
        // Land tile `0` is what a flat scene is paved with, so flagging the id is
        // the whole sea — no per-tile pass over a facet-sized square.
        const WATER: u16 = 0;
        const JETTY: u16 = 1;
        let mut scene = Scene::flat_holding(START.0 + 8, START.1 + 8, 0);
        scene.land_art(WATER, TileFlags::WATER);
        scene.land(START.0, START.1, JETTY);
        scene.art(HULL, super::tests::WALL_FLAGS, 10);
        scene.art(DECK, TileFlags::PLATFORM, 3);
        scene
    }

    let (map, tiles) = sea().into_shard(Facet(0));
    let mut world = World::new(START).with_save_every(0);
    world.state.facet_state_mut(Facet(0)).set_map(Some(map), &tiles);
    world.state.set_tiles(tiles);
    world.state.multis = super::tests::multis_with(SLOOP, sloop());
    let now = Instant::now();
    let connection = enter(&mut world, now);
    let player = world.state.players[&connection];
    let owner = world.state.registry.serial_of(player).expect("a serial");

    let at = Point::new(START.0 + 5, START.1 + 5, 0);
    let boat = openshard_boats::place(
        &mut world.state,
        player,
        at,
        Facet(0),
        openshard_protocol::wire::MultiId(SLOOP),
        owner,
    )
    .expect("open water");
    let serial = world.state.registry.serial_of(boat).expect("a boat serial");

    world.take_snapshot();
    let snapshot = only_snapshot(&mut world).expect("a character entered");
    let boats = snapshot.boats.expect("a full sweep carries the boats");
    assert_eq!(boats.len(), 1);
    assert_eq!(boats[0].serial, serial);
    assert_eq!(boats[0].multi, SLOOP);
    assert_eq!((boats[0].x, boats[0].y, boats[0].z), (at.x, at.y, at.z));
    assert_eq!(boats[0].owner, owner);

    // And it is not saved twice. A ship carries a `Drawn` and a `Position` like
    // any item, so the item sweep would pick it up as ground clutter and restore
    // a hull with no deck under whoever was standing on it — the bug this engine
    // has already had once, with houses.
    let ground = snapshot.ground.expect("a full sweep carries the ground");
    assert!(
        !ground.iter().any(|item| item.serial == serial),
        "the ship was saved as an item as well as a boat"
    );

    // The shard comes back up on that save, with the same sea.
    let mut restored = World::new(START);
    let (map, tiles) = sea().into_shard(Facet(0));
    restored
        .state
        .facet_state_mut(Facet(0))
        .set_map(Some(map), &tiles);
    restored.state.set_tiles(tiles);
    restored.state.multis = super::tests::multis_with(SLOOP, sloop());
    restored.restore_boats(boats);

    let back = restored
        .state
        .registry
        .entity_of(serial)
        .expect("the ship came back under its own serial");
    assert_eq!(
        restored
            .state
            .registry
            .get::<openshard_state::components::Boat>(back),
        Some(&openshard_state::components::Boat {
            multi: openshard_protocol::wire::MultiId(SLOOP),
            owner,
        }),
    );

    // And the berth is back, which is the half a `Boat` component cannot prove.
    let index = &restored.state.facet_state(Facet(0)).boats();
    assert_eq!(
        index.deck_at(at.x, at.y, 0),
        Some(3),
        "the ship came back with nothing to stand on",
    );
    assert!(
        index.blocks_at(at.x - 1, at.y, 0),
        "the ship came back with no hull",
    );
    assert_eq!(index.boat_at(at.x, at.y), Some(back));
}
