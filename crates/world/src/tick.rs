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

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use openshard_entities::{EntityId, Registry, SerialKind};
use openshard_events::EventBus;
use openshard_gateway::ConnectionId;
use openshard_movement::{OpenWorld, Walk, Walker};
use openshard_protocol::{
    encode_light_level, encode_login_complete, encode_map_change, encode_remove, encode_walk_ack,
    encode_walk_reject, ClientVersion, Direction, Facing, MobileIncoming, MobileMove, Notoriety,
    PlayerStart, PlayerUpdate, Point, WalkRequest, DEFAULT_MAP_HEIGHT, DEFAULT_MAP_WIDTH,
};
use tracing::{debug, info, warn};

use crate::components::{Body, Client, Heading, Movement, Name, Position};
use crate::events::{
    MobileMoved, MobileTurned, PlayerEntered, PlayerLeft, RefusedReason, StepRefused,
};
use crate::sectors::{Sectors, VIEW_RANGE};
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
/// The facet size used when there is no map. Big enough for anywhere a test
/// puts something; the grid is a `Vec` of empty buckets and costs nothing.
const FACET_WITHOUT_A_MAP: (u32, u32) = (7168, 4096);

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
    /// What is near what.
    sectors: Sectors,
    /// Which entity a connection is driving.
    players: HashMap<ConnectionId, EntityId>,
    /// What each player's client currently has on screen.
    ///
    /// The server has to remember, because the client never says. There is no
    /// "what can you see" packet — only "draw this" and "forget that" — so the
    /// only way to send a mobile exactly once is to know what was sent before.
    seen: HashMap<EntityId, HashSet<EntityId>>,
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
            sectors: Sectors::new(FACET_WITHOUT_A_MAP.0, FACET_WITHOUT_A_MAP.1),
            players: HashMap::new(),
            seen: HashMap::new(),
            start,
            inbox: Vec::new(),
            outbox: Vec::new(),
            ticks: 0,
        }
    }

    /// Give this world a map.
    pub fn with_terrain(mut self, terrain: MapTerrain) -> Self {
        self.sectors = Sectors::new(terrain.map().width(), terrain.map().height());
        self.terrain = Some(terrain);
        self
    }

    /// The spatial index.
    pub const fn sectors(&self) -> &Sectors {
        &self.sectors
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
        self.sectors.insert(entity, position);
        self.seen.insert(entity, HashSet::new());

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

        // Draw whoever is already here, and draw this one for them. Both
        // directions, because arriving is symmetric: the newcomer has an empty
        // screen and everyone nearby has a gap where it now stands.
        self.refresh_around(entity);
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
                // The index is a second copy of the position; this is the line
                // that keeps it honest.
                self.sectors.insert(entity, position);
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
                self.refresh_around(entity);
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
                // A turn moves nobody, but it changes what everyone watching
                // draws — the client animates a facing it is told about.
                self.broadcast_move(entity);
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

        // Take it off every screen *before* despawning: once the entity is gone
        // its serial is released and there is nothing left to tell anyone about.
        if let Some(serial) = serial {
            for watcher in self.watchers_of(entity) {
                self.forget(watcher, entity, serial);
            }
        }
        self.seen.remove(&entity);
        self.sectors.remove(entity);
        self.registry.despawn(entity);

        if let Some(serial) = serial {
            self.bus.send(PlayerLeft { entity, serial });
            info!(%serial, "left the world");
        }
    }

    // -- interest management ----------------------------------------------

    /// Every player who currently has `entity` on screen.
    fn watchers_of(&self, entity: EntityId) -> Vec<EntityId> {
        self.seen
            .iter()
            .filter(|(watcher, seen)| **watcher != entity && seen.contains(&entity))
            .map(|(watcher, _)| *watcher)
            .collect()
    }

    /// Bring `entity`'s neighbourhood up to date, both ways.
    ///
    /// Whoever it can see, and whoever can see it. Both, because visibility is
    /// symmetric here and doing one direction leaves the other end with a mobile
    /// that walked away and never left the screen.
    fn refresh_around(&mut self, entity: EntityId) {
        let Some(centre) = self.sectors.position_of(entity) else {
            return;
        };

        // Collect first. The lookup borrows the index and the sends borrow
        // `self` mutably, and more importantly a `Vec` here is what makes the
        // set of neighbours a snapshot rather than something that shifts while
        // it is walked.
        let neighbours: Vec<EntityId> = self
            .sectors
            .nearby(centre, VIEW_RANGE)
            .map(|(id, _)| id)
            .filter(|id| *id != entity)
            .collect();

        for other in &neighbours {
            self.show(entity, *other);
            self.show(*other, entity);
        }

        // Anything this one used to see and no longer can. `nearby` says who is
        // close; only the remembered set says who *was*.
        let gone: Vec<EntityId> = self
            .seen
            .get(&entity)
            .map(|seen| {
                seen.iter()
                    .filter(|id| !neighbours.contains(id))
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        for other in gone {
            if let Some(serial) = self.registry.serial_of(other) {
                self.forget(entity, other, serial);
            }
        }

        // And anyone who used to see this one and no longer can.
        for watcher in self.watchers_of(entity) {
            if !neighbours.contains(&watcher) {
                if let Some(serial) = self.registry.serial_of(entity) {
                    self.forget(watcher, entity, serial);
                }
            }
        }

        self.broadcast_move(entity);
    }

    /// Tell everyone already watching `entity` that it moved.
    ///
    /// Only those who already have it: someone seeing it for the first time gets
    /// a `0x78` from [`World::show`], and a `0x77` for a mobile the client has
    /// never heard of is ignored.
    fn broadcast_move(&mut self, entity: EntityId) {
        let Some(packet) = self.mobile_move(entity) else {
            return;
        };
        for watcher in self.watchers_of(entity) {
            let Some(&Client {
                connection,
                version,
            }) = self.registry.get::<Client>(watcher)
            else {
                continue;
            };
            self.outbox.push(Outbound {
                connection,
                packet: packet.encode(version),
            });
        }
    }

    /// Draw `other` for `watcher`, if it is not already on screen.
    fn show(&mut self, watcher: EntityId, other: EntityId) {
        // Only players have screens. An NPC "seeing" someone is an AI question,
        // and it does not belong in the packet path.
        let Some(&Client {
            connection,
            version,
        }) = self.registry.get::<Client>(watcher)
        else {
            return;
        };
        if self
            .seen
            .get(&watcher)
            .is_some_and(|seen| seen.contains(&other))
        {
            return;
        }
        let Some(incoming) = self.mobile_incoming(other) else {
            return;
        };
        self.seen.entry(watcher).or_default().insert(other);
        self.outbox.push(Outbound {
            connection,
            packet: incoming.encode(version),
        });
    }

    /// Take `other` off `watcher`'s screen.
    fn forget(&mut self, watcher: EntityId, other: EntityId, serial: openshard_entities::Serial) {
        if let Some(seen) = self.seen.get_mut(&watcher) {
            if !seen.remove(&other) {
                return;
            }
        } else {
            return;
        }
        if let Some(&Client { connection, .. }) = self.registry.get::<Client>(watcher) {
            self.outbox.push(Outbound {
                connection,
                packet: encode_remove(serial.raw()),
            });
        }
    }

    /// Build a 0x78 for an entity, if it is a drawable mobile.
    fn mobile_incoming(&self, entity: EntityId) -> Option<MobileIncoming> {
        let serial = self.registry.serial_of(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        let Heading(facing) = *self.registry.get::<Heading>(entity)?;
        let body = *self.registry.get::<Body>(entity)?;
        Some(MobileIncoming {
            serial: serial.raw(),
            body: body.id,
            position,
            facing,
            hue: body.hue,
            flags: 0,
            notoriety: Notoriety::Innocent,
            // Nothing wears anything yet: there is no items crate. The list is
            // empty rather than absent, which is what the client expects.
            equipment: Vec::new(),
        })
    }

    /// Build a 0x77 for an entity.
    fn mobile_move(&self, entity: EntityId) -> Option<MobileMove> {
        let serial = self.registry.serial_of(entity)?;
        let Position(position) = *self.registry.get::<Position>(entity)?;
        let Heading(facing) = *self.registry.get::<Heading>(entity)?;
        let body = *self.registry.get::<Body>(entity)?;
        Some(MobileMove {
            serial: serial.raw(),
            body: body.id,
            position,
            facing,
            hue: body.hue,
            flags: 0,
            notoriety: Notoriety::Innocent,
        })
    }

    fn send(&mut self, connection: ConnectionId, packet: Vec<u8>) {
        self.outbox.push(Outbound { connection, packet });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use openshard_events::Cursor;
    use openshard_movement::WALK_INTERVAL;

    pub(super) const START: (u16, u16) = (1363, 1600);

    pub(super) fn world() -> World {
        World::new(START)
    }

    pub(super) fn connection() -> ConnectionId {
        ConnectionId::from_raw(1)
    }

    pub(super) fn enter(world: &mut World, now: Instant) -> ConnectionId {
        enter_as(world, connection(), now)
    }

    pub(super) fn enter_as(
        world: &mut World,
        connection: ConnectionId,
        now: Instant,
    ) -> ConnectionId {
        world.queue(Command::Enter {
            connection,
            version: ClientVersion::TOL,
            name: "Lord British".to_owned(),
        });
        world.tick(now);
        connection
    }

    /// Every packet the last tick produced for one connection.
    pub(super) fn packets_for(world: &mut World, connection: ConnectionId) -> Vec<Vec<u8>> {
        world
            .drain_outbound()
            .filter(|out| out.connection == connection)
            .map(|out| out.packet)
            .collect()
    }

    /// Put an entity somewhere directly, as if it had walked there.
    pub(super) fn teleport(world: &mut World, connection: ConnectionId, point: Point) {
        let entity = world.players[&connection];
        world.registry.insert(entity, Position(point));
        if let Some(Movement(mut walker)) = world.registry.get::<Movement>(entity).copied() {
            walker.position = point;
            world.registry.insert(entity, Movement(walker));
        }
        world.sectors.insert(entity, point);
        world.refresh_around(entity);
    }

    pub(super) fn walk(sequence: u8, direction: Direction) -> WalkRequest {
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

#[cfg(test)]
mod interest_tests {
    use super::tests::*;
    use super::*;
    use openshard_movement::WALK_INTERVAL;

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

        let to_bob = packets_for(&mut world, BOB);
        let drawn = to_bob.iter().filter(|p| p[0] == 0x78).count();
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
        assert_eq!(world.seen.len(), 2);

        world.queue(Command::Disconnect { connection: BOB });
        world.tick(now);

        assert_eq!(world.seen.len(), 1, "Bob's screen outlived Bob");
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
        let entity = world.players[&alice];

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
}
