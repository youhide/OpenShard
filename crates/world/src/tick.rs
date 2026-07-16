//! The simulation loop.
//!
//! # Why there is a tick at all
//!
//! Everything so far answers a packet: a client asks to walk, the server says
//! yes. That works right up until something has to happen *without* a client
//! asking — an item decaying, a wound healing, an NPC deciding to move. There is
//! nowhere to put any of it in a request/response server.
//!
//! The tick is that place. It is also what makes the simulation deterministic:
//! commands arrive from network tasks on whatever thread at whatever moment,
//! queue up, and are applied in a fixed order at a fixed rate. Replay the same
//! commands and you get the same world.
//!
//! # The boundary
//!
//! ```text
//!   network tasks          the tick               network tasks
//!   ─────────────>  [ commands ]  ─────────>  [ outbound packets ]
//!        async         drained in order            async again
//! ```
//!
//! The gateway already draws half of this line by handing events to a channel
//! rather than calling back. This is the other half: nothing inside
//! [`World::tick`] awaits, reads a clock, or touches a socket.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use openshard_entities::{EntityId, Registry, SerialKind};
use openshard_events::EventBus;
use openshard_gateway::ConnectionId;
use openshard_movement::{OpenWorld, Walk, Walker};
use openshard_protocol::{
    encode_light_level, encode_login_complete, encode_map_change, encode_walk_ack,
    encode_walk_reject, ClientVersion, Direction, Facing, PlayerStart, PlayerUpdate, Point,
    WalkRequest, DEFAULT_MAP_HEIGHT, DEFAULT_MAP_WIDTH,
};
use tracing::{debug, info, warn};

use crate::components::{Body, Client, Heading, Movement, Name, Position};
use crate::events::{
    MobileMoved, MobileTurned, PlayerEntered, PlayerLeft, RefusedReason, StepRefused,
};
use crate::terrain::MapTerrain;

/// How often the world ticks.
///
/// 20Hz. Fast enough that a 200ms walk step lands within a tick of when the
/// client expects it, and slow enough to leave room for everything a tick will
/// eventually do. Not a protocol constant — the client does not know or care.
pub const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// A human male body.
const BODY_HUMAN_MALE: u16 = 0x0190;
/// Full daylight. The scale runs backwards: 0 is brightest, 0x1F pitch dark.
const LIGHT_DAY: u8 = 0;
/// Zero is Felucca.
const MAP_FELUCCA: u8 = 0;
/// The height to use when there is no map to ask.
const Z_WITHOUT_A_MAP: i8 = 0;
/// Notoriety 0x01 is "innocent" — the blue health bar.
const NOTORIETY_INNOCENT: u8 = 0x01;

/// Something for the world to do, from outside the world.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    /// A client picked a character.
    Enter {
        /// Which connection.
        connection: ConnectionId,
        /// What the client claims to be.
        version: ClientVersion,
        /// The character's name.
        name: String,
    },
    /// A client asked to take a step.
    Walk {
        /// Which connection.
        connection: ConnectionId,
        /// The request.
        request: WalkRequest,
    },
    /// A connection went away.
    Disconnect {
        /// Which connection.
        connection: ConnectionId,
    },
}

/// Bytes for a connection, produced by a tick.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Outbound {
    /// Who to send to.
    pub connection: ConnectionId,
    /// What to send.
    pub packet: Vec<u8>,
}

/// The world.
///
/// Owns the registry, the bus and the map. A plain value: nothing here is a
/// static, and a test builds as many as it likes.
pub struct World {
    registry: Registry,
    bus: EventBus,
    terrain: Option<MapTerrain>,
    /// Which entity a connection is driving.
    players: HashMap<ConnectionId, EntityId>,
    /// Where new characters appear. The height comes from the map.
    start: (u16, u16),
    /// Commands waiting for the next tick.
    inbox: Vec<Command>,
    /// Packets the last tick produced.
    outbox: Vec<Outbound>,
    /// How many ticks have run.
    ticks: u64,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("ticks", &self.ticks)
            .field("entities", &self.registry.len())
            .field("players", &self.players.len())
            .field("map", &self.terrain.is_some())
            .finish()
    }
}

impl World {
    /// An empty world with no map, spawning at `start`.
    pub fn new(start: (u16, u16)) -> Self {
        Self {
            registry: Registry::new(),
            bus: EventBus::new(),
            terrain: None,
            players: HashMap::new(),
            start,
            inbox: Vec::new(),
            outbox: Vec::new(),
            ticks: 0,
        }
    }

    /// Give this world a map.
    pub fn with_terrain(mut self, terrain: MapTerrain) -> Self {
        self.terrain = Some(terrain);
        self
    }

    /// The event bus, for reading what happened.
    pub const fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// Everything in the world.
    pub const fn registry(&self) -> &Registry {
        &self.registry
    }

    /// How many ticks have run.
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }

    /// How many people are in the world.
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// Queue a command for the next tick.
    ///
    /// Never acts immediately. That is the whole point: a command that took
    /// effect the moment it arrived would run world code on a network thread at
    /// an arbitrary point in the tick, and two clients racing would produce a
    /// different world depending on which packet won.
    pub fn queue(&mut self, command: Command) {
        self.inbox.push(command);
    }

    /// Take the packets the last tick produced.
    pub fn drain_outbound(&mut self) -> std::vec::Drain<'_, Outbound> {
        self.outbox.drain(..)
    }

    /// Run one tick.
    ///
    /// `now` is a parameter, like everywhere else on this path: a tick that read
    /// the clock could not be replayed, and a simulation that cannot be replayed
    /// cannot be debugged from a log.
    pub fn tick(&mut self, now: Instant) {
        self.ticks += 1;

        // Take the whole inbox. A command queued *during* a tick belongs to the
        // next one — otherwise a system that queues work could starve the loop,
        // and the tick's length would depend on what happened in it.
        let commands = std::mem::take(&mut self.inbox);
        for command in commands {
            self.apply(command, now);
        }

        // Retire the oldest events. Once per tick, after every system, so that
        // "one tick" means the same thing for every event type.
        self.bus.update();
    }

    fn apply(&mut self, command: Command, now: Instant) {
        match command {
            Command::Enter {
                connection,
                version,
                name,
            } => self.enter(connection, version, name),
            Command::Walk {
                connection,
                request,
            } => self.walk(connection, request, now),
            Command::Disconnect { connection } => self.disconnect(connection),
        }
    }

    /// Where a character appears: the configured x and y, at the map's height.
    ///
    /// The `z` is read from the map rather than configured. A second source of
    /// truth that disagrees by three units leaves a character unable to take a
    /// single step — every one is more than a two-unit climb — with nothing in
    /// the log to explain it.
    fn start_position(&self) -> Point {
        let (x, y) = self.start;
        let z = self
            .terrain
            .as_ref()
            .and_then(|terrain| terrain.map().land(x, y))
            .map_or(Z_WITHOUT_A_MAP, |cell| cell.z);
        Point::new(x, y, z)
    }

    fn enter(&mut self, connection: ConnectionId, version: ClientVersion, name: String) {
        if self.players.contains_key(&connection) {
            warn!(%connection, "already in the world");
            return;
        }
        let Ok((entity, serial)) = self.registry.spawn_with_serial(SerialKind::Mobile) else {
            warn!(%connection, "the mobile serial pool is exhausted");
            return;
        };

        let position = self.start_position();
        let facing = Facing::walking(Direction::South);
        let body = Body {
            id: BODY_HUMAN_MALE,
            hue: 0x83EA,
        };

        self.registry.insert(entity, Position(position));
        self.registry.insert(entity, Heading(facing));
        self.registry.insert(entity, body);
        self.registry.insert(entity, Name(name.clone()));
        self.registry
            .insert(entity, Movement(Walker::new(position, facing)));
        self.registry.insert(
            entity,
            Client {
                connection,
                version,
            },
        );
        self.players.insert(connection, entity);

        // The order is the client's, not ours. 0x1B must come first — until it
        // lands there is no body to attach anything to — and 0x55 must come
        // last, because it is what tells the client to start drawing. What is
        // between can be reordered; the two ends cannot.
        self.send(
            connection,
            PlayerStart {
                serial: serial.raw(),
                body: body.id,
                position,
                facing,
                map_width: DEFAULT_MAP_WIDTH,
                map_height: DEFAULT_MAP_HEIGHT,
            }
            .encode(),
        );
        self.send(connection, encode_map_change(MAP_FELUCCA));
        self.send(
            connection,
            PlayerUpdate {
                serial: serial.raw(),
                body: body.id,
                hue: body.hue,
                flags: 0,
                position,
                facing,
            }
            .encode(),
        );
        self.send(connection, encode_light_level(LIGHT_DAY));
        self.send(connection, encode_login_complete());

        self.bus.send(PlayerEntered {
            entity,
            serial,
            position,
        });
        info!(%serial, name, position = %position, "in world");
    }

    fn walk(&mut self, connection: ConnectionId, request: WalkRequest, now: Instant) {
        let Some(&entity) = self.players.get(&connection) else {
            // A walk before a character. Not fatal — a stray packet from a
            // client that reconnected — but nothing to act on either.
            debug!(%connection, "0x02 from a connection with no character");
            return;
        };
        let Some(serial) = self.registry.serial_of(entity) else {
            return;
        };
        let Some(Movement(mut walker)) = self.registry.get::<Movement>(entity).copied() else {
            return;
        };

        let was = walker.position;
        let out_of_sequence = walker.sequence.is_fresh() && request.sequence != 0;
        let outcome = match &self.terrain {
            Some(terrain) => walker.request(request, terrain, now),
            None => walker.request(request, &OpenWorld, now),
        };
        self.registry.insert(entity, Movement(walker));

        match outcome {
            Walk::Moved { position, facing } => {
                self.registry.insert(entity, Position(position));
                self.registry.insert(entity, Heading(facing));
                self.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.bus.send(MobileMoved {
                    entity,
                    serial,
                    from: was,
                    to: position,
                    facing,
                });
            }
            Walk::Turned { facing } => {
                self.registry.insert(entity, Heading(facing));
                self.send(
                    connection,
                    encode_walk_ack(request.sequence, NOTORIETY_INNOCENT),
                );
                self.bus.send(MobileTurned {
                    entity,
                    serial,
                    facing,
                });
            }
            Walk::Refused => {
                // Which of the three it was is not something `Walk` says, and
                // teaching it to would put the reasons in the wrong crate. The
                // sequence is checked before anything else, so a fresh walker
                // with a non-zero sequence can only have failed that; past it,
                // the pace and the terrain are the two left and this cannot yet
                // tell them apart. Better a coarse reason than a wrong one.
                let reason = if out_of_sequence {
                    RefusedReason::OutOfSequence
                } else {
                    RefusedReason::Blocked
                };
                self.send(
                    connection,
                    encode_walk_reject(request.sequence, walker.position, walker.facing),
                );
                self.bus.send(StepRefused {
                    entity,
                    serial,
                    reason,
                });
                debug!(%serial, ?reason, "step refused");
            }
        }
    }

    fn disconnect(&mut self, connection: ConnectionId) {
        let Some(entity) = self.players.remove(&connection) else {
            return;
        };
        let serial = self.registry.serial_of(entity);
        self.registry.despawn(entity);
        if let Some(serial) = serial {
            self.bus.send(PlayerLeft { entity, serial });
            info!(%serial, "left the world");
        }
    }

    fn send(&mut self, connection: ConnectionId, packet: Vec<u8>) {
        self.outbox.push(Outbound { connection, packet });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_events::Cursor;
    use openshard_movement::WALK_INTERVAL;

    const START: (u16, u16) = (1363, 1600);

    fn world() -> World {
        World::new(START)
    }

    fn connection() -> ConnectionId {
        ConnectionId::from_raw(1)
    }

    fn enter(world: &mut World, now: Instant) -> ConnectionId {
        let connection = connection();
        world.queue(Command::Enter {
            connection,
            version: ClientVersion::TOL,
            name: "Lord British".to_owned(),
        });
        world.tick(now);
        connection
    }

    fn walk(sequence: u8, direction: Direction) -> WalkRequest {
        WalkRequest {
            facing: Facing::walking(direction),
            sequence,
            fastwalk_key: 0,
        }
    }

    #[test]
    fn a_command_does_nothing_until_the_tick() {
        // The whole boundary. If queueing acted immediately, world code would run
        // on a network thread at an arbitrary point, and two clients racing would
        // produce a different world depending on which packet won.
        let mut world = world();
        world.queue(Command::Enter {
            connection: connection(),
            version: ClientVersion::TOL,
            name: "Lord British".to_owned(),
        });

        assert_eq!(world.player_count(), 0, "queued, not applied");
        assert_eq!(world.drain_outbound().count(), 0, "and nothing sent");

        world.tick(Instant::now());
        assert_eq!(world.player_count(), 1);
    }

    #[test]
    fn entering_sends_the_sequence_the_client_needs() {
        let mut world = world();
        enter(&mut world, Instant::now());

        let ids: Vec<u8> = world.drain_outbound().map(|out| out.packet[0]).collect();
        assert_eq!(
            ids,
            vec![0x1B, 0xBF, 0x20, 0x4F, 0x55],
            "0x1B first or there is no body; 0x55 last or the client draws early"
        );
    }

    #[test]
    fn entering_builds_an_entity_out_of_components() {
        let mut world = world();
        enter(&mut world, Instant::now());

        let entity = *world.players.values().next().unwrap();
        assert!(world.registry().has::<Position>(entity));
        assert!(world.registry().has::<Body>(entity));
        assert!(world.registry().has::<Name>(entity));
        assert!(world.registry().has::<Movement>(entity), "a player walks");
        assert!(
            world.registry().has::<Client>(entity),
            "and has a connection"
        );
        assert!(world.registry().serial_of(entity).is_some());
    }

    #[test]
    fn entering_twice_on_one_connection_is_ignored() {
        let mut world = world();
        let now = Instant::now();
        enter(&mut world, now);
        enter(&mut world, now);
        assert_eq!(world.player_count(), 1);
    }

    #[test]
    fn walking_moves_the_position_component_too() {
        // Two places hold a position — `Position` and the `Movement`'s walker —
        // and a system that reads one while the other has moved is a rubber-band
        // bug. The tick is what keeps them in step.
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let _ = world.drain_outbound().count();

        world.queue(Command::Walk {
            connection,
            request: walk(0, Direction::South),
        });
        world.tick(now);

        let entity = world.players[&connection];
        let Position(position) = *world.registry().get::<Position>(entity).unwrap();
        let Movement(walker) = *world.registry().get::<Movement>(entity).unwrap();
        assert_eq!(position, walker.position, "the two must not drift apart");
        assert_eq!(position, Point::new(START.0, START.1 + 1, Z_WITHOUT_A_MAP));
    }

    #[test]
    fn walking_emits_an_event_and_acks() {
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let _ = world.drain_outbound().count();
        let mut moves: Cursor<MobileMoved> = world.bus().cursor();

        world.queue(Command::Walk {
            connection,
            request: walk(0, Direction::South),
        });
        world.tick(now);

        let sent: Vec<Vec<u8>> = world.drain_outbound().map(|out| out.packet).collect();
        assert_eq!(sent, vec![vec![0x22, 0, NOTORIETY_INNOCENT]]);

        let moved: Vec<_> = world.bus().read(&mut moves).copied().collect();
        assert_eq!(moved.len(), 1);
        assert_eq!(moved[0].from, Point::new(START.0, START.1, Z_WITHOUT_A_MAP));
        assert_eq!(
            moved[0].to,
            Point::new(START.0, START.1 + 1, Z_WITHOUT_A_MAP)
        );
    }

    #[test]
    fn turning_emits_a_turn_not_a_move() {
        // A listener that cares where things are should not have to filter out
        // events where nothing went anywhere.
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let mut moves: Cursor<MobileMoved> = world.bus().cursor();
        let mut turns: Cursor<MobileTurned> = world.bus().cursor();

        // Spawned facing south; ask for north.
        world.queue(Command::Walk {
            connection,
            request: walk(0, Direction::North),
        });
        world.tick(now);

        assert_eq!(world.bus().read(&mut moves).count(), 0, "nothing moved");
        assert_eq!(world.bus().read(&mut turns).count(), 1);
    }

    #[test]
    fn an_out_of_sequence_step_says_so() {
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let mut refused: Cursor<StepRefused> = world.bus().cursor();

        world.queue(Command::Walk {
            connection,
            request: walk(9, Direction::South),
        });
        world.tick(now);

        let events: Vec<_> = world.bus().read(&mut refused).copied().collect();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].reason, RefusedReason::OutOfSequence);
    }

    #[test]
    fn a_flood_is_refused_and_says_so() {
        // The pace, through the tick. Every step in one instant is a speedhack.
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let _ = world.drain_outbound().count();

        for sequence in 0..200u8 {
            world.queue(Command::Walk {
                connection,
                request: walk(sequence, Direction::South),
            });
        }
        world.tick(now);

        let rejects = world
            .drain_outbound()
            .filter(|out| out.packet[0] == 0x21)
            .count();
        assert!(rejects > 150, "only {rejects} of 200 instant steps refused");
    }

    #[test]
    fn an_honest_walker_is_never_refused_across_ticks() {
        let mut world = world();
        let start = Instant::now();
        let connection = enter(&mut world, start);
        let _ = world.drain_outbound().count();

        let mut sequence = 0u8;
        for step in 0..200u32 {
            let now = start + WALK_INTERVAL * step;
            world.queue(Command::Walk {
                connection,
                request: walk(sequence, Direction::South),
            });
            world.tick(now);
            let refused = world
                .drain_outbound()
                .filter(|out| out.packet[0] == 0x21)
                .count();
            assert_eq!(refused, 0, "step {step} refused");
            sequence = if sequence == u8::MAX { 1 } else { sequence + 1 };
        }
    }

    #[test]
    fn a_walk_from_a_connection_with_no_character_is_ignored() {
        let mut world = world();
        world.queue(Command::Walk {
            connection: connection(),
            request: walk(0, Direction::South),
        });
        world.tick(Instant::now());
        assert_eq!(world.drain_outbound().count(), 0);
    }

    #[test]
    fn disconnecting_releases_the_entity_and_its_serial() {
        let mut world = world();
        let now = Instant::now();
        let connection = enter(&mut world, now);
        let entity = world.players[&connection];
        let serial = world.registry().serial_of(entity).unwrap();

        let mut left: Cursor<PlayerLeft> = world.bus().cursor();
        world.queue(Command::Disconnect { connection });
        world.tick(now);

        assert_eq!(world.player_count(), 0);
        assert!(!world.registry().contains(entity));
        assert_eq!(
            world.registry().entity_of(serial),
            None,
            "a dead serial resolves to nothing"
        );
        assert_eq!(world.bus().read(&mut left).count(), 1);
    }

    #[test]
    fn disconnecting_a_connection_that_never_entered_is_harmless() {
        let mut world = world();
        world.queue(Command::Disconnect {
            connection: connection(),
        });
        world.tick(Instant::now());
    }

    #[test]
    fn a_command_queued_during_a_tick_waits_for_the_next_one() {
        // The inbox is taken whole. Otherwise a system that queues work could
        // starve the loop, and a tick's length would depend on what happened in
        // it — which is the end of a fixed timestep.
        let mut world = world();
        let now = Instant::now();
        world.tick(now);
        let before = world.ticks();

        world.queue(Command::Enter {
            connection: connection(),
            version: ClientVersion::TOL,
            name: "a".to_owned(),
        });
        assert_eq!(world.player_count(), 0);
        world.tick(now);
        assert_eq!(world.ticks(), before + 1);
        assert_eq!(world.player_count(), 1);
    }

    #[test]
    fn an_empty_tick_is_cheap_and_harmless() {
        let mut world = world();
        let now = Instant::now();
        for _ in 0..1000 {
            world.tick(now);
        }
        assert_eq!(world.ticks(), 1000);
        assert!(world.registry().is_empty());
    }

    #[test]
    fn a_reader_that_polls_once_a_tick_never_misses_an_event() {
        // The property that matters, and the reason the bus is double-buffered.
        // A system reading once per tick sees everything, whatever order the
        // systems ran in — including one that polled *before* the emitter within
        // the same tick, which is what this simulates: the cursor is taken before
        // the tick that emits.
        let mut world = world();
        let now = Instant::now();
        let mut entered: Cursor<PlayerEntered> = world.bus().cursor();

        enter(&mut world, now);
        assert_eq!(world.bus().read(&mut entered).count(), 1);
    }

    #[test]
    fn an_event_is_gone_a_tick_after_the_one_that_emitted_it() {
        // The lifetime, stated as it actually is. `tick` calls `bus.update()` at
        // its end, so the emitting tick already spends one of the event's two
        // buffers: it is readable after that tick, and gone after the next.
        //
        // That is not a bug, and the guarantee still holds — a reader polling
        // once per tick has a full tick to see it. But "events live two ticks"
        // is off by one if you count from outside, and this is where you find
        // that out.
        let mut world = world();
        let now = Instant::now();
        enter(&mut world, now);

        let mut after_emit: Cursor<PlayerEntered> = world.bus().cursor();
        assert_eq!(
            world.bus().read(&mut after_emit).count(),
            1,
            "readable after the tick that emitted it"
        );

        world.tick(now);
        let mut a_tick_later: Cursor<PlayerEntered> = world.bus().cursor();
        assert_eq!(
            world.bus().read(&mut a_tick_later).count(),
            0,
            "and gone after the next"
        );
    }

    #[test]
    fn the_tick_interval_is_not_a_protocol_constant() {
        // 20Hz is ours to change. The client neither knows nor cares; it only
        // sees acks. Worth stating because the 200ms walk interval *is* the
        // client's, and the two are easy to confuse.
        assert_eq!(TICK_INTERVAL.as_millis(), 50);
        assert!(
            TICK_INTERVAL < WALK_INTERVAL,
            "a step must not span two ticks"
        );
    }
}
