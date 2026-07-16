//! Everything after the character list: entering the world, and walking in it.
//!
//! # This is a placeholder for the rest of `openshard-world`
//!
//! There is no tick, no spatial index and no other mobiles. A player enters,
//! walks around alone, and none of it is saved.
//!
//! There *is* a map: `MapTerrain` from `openshard-world`, when the client files
//! are configured. Without them the shard falls back to `OpenWorld`, where every
//! step is allowed — which is a development mode, not a feature.
//!
//! When the world crate grows a tick this file goes away. Do not grow it.

use openshard_entities::{Registry, Serial, SerialKind};
use openshard_movement::{OpenWorld, Walk, Walker};
use openshard_protocol::{
    encode_light_level, encode_login_complete, encode_map_change, encode_walk_ack,
    encode_walk_reject, CharacterPlay, Direction, Facing, PlayerStart, PlayerUpdate, Point,
    WalkRequest, DEFAULT_MAP_HEIGHT, DEFAULT_MAP_WIDTH,
};
use openshard_world::MapTerrain;
use tracing::{debug, info, warn};

/// Where a new character appears: open ground north-west of Britain.
///
/// # This is a guess and it belongs in config
///
/// Facets differ. The classic (1475, 1774) — Britain's centre — is open water on
/// some maps, and a hard-coded coordinate is only right for the files it was
/// picked against.
///
/// The `z` matters more than it looks: `Game::start_position` corrects it from
/// the map at spawn, because a character standing more than two units off the
/// ground has every first step refused and cannot move, with nothing in the log
/// to say why.
const START_POSITION: Point = Point::new(1363, 1600, 30);

/// A human male body.
const BODY_HUMAN_MALE: u16 = 0x0190;

/// Full daylight. The scale runs backwards: 0 is brightest, 0x1F is pitch dark.
const LIGHT_DAY: u8 = 0;

/// Which map. Zero is Felucca.
const MAP_FELUCCA: u8 = 0;

/// One player in the world.
#[derive(Debug)]
pub struct Player {
    /// The wire identity the client addresses.
    pub serial: Serial,
    /// Where it is and which way it faces.
    pub walker: Walker,
}

/// The world, such as it is.
#[derive(Debug, Default)]
pub struct Game {
    /// Every entity. Real for its size, and the seam the world crate grows from.
    registry: Registry,
    /// The map, if the client files were configured.
    terrain: Option<MapTerrain>,
}

/// What a game packet produced.
#[derive(Debug, Default)]
pub struct Reply {
    /// Packets to send, in order.
    pub packets: Vec<Vec<u8>>,
}

impl Reply {
    fn push(&mut self, packet: Vec<u8>) {
        self.packets.push(packet);
    }
}

impl Game {
    /// An empty world with no map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Give this world a map.
    pub fn with_terrain(mut self, terrain: MapTerrain) -> Self {
        self.terrain = Some(terrain);
        self
    }

    /// Where a character should appear.
    ///
    /// The configured spot, with its height corrected to whatever the map says
    /// the ground actually is. A hard-coded z is a guess, and a guess that is
    /// more than two units out means the player's first step is refused — they
    /// spawn unable to move, with nothing in the log to explain it.
    fn start_position(&self) -> Point {
        let Some(terrain) = &self.terrain else {
            return START_POSITION;
        };
        match terrain.map().land(START_POSITION.x, START_POSITION.y) {
            Some(cell) => Point::new(START_POSITION.x, START_POSITION.y, cell.z),
            None => START_POSITION,
        }
    }

    /// How many entities exist.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.registry.len()
    }

    /// Handle `0x5D`: put a character in the world.
    ///
    /// The order of the reply is the client's, not ours. `0x1B` must come first
    /// — until it lands the client has no body to attach anything to — and
    /// `0x55` must come last, because it is what tells the client to start
    /// drawing. What is in between can be reordered; the two ends cannot.
    pub fn character_play(&mut self, play: &CharacterPlay) -> Option<(Player, Reply)> {
        let (entity, serial) = self.registry.spawn_with_serial(SerialKind::Mobile).ok()?;
        let facing = Facing::walking(Direction::South);
        let walker = Walker::new(self.start_position(), facing);

        // The registry is barely used yet — it holds the serial mapping and
        // nothing else. That is deliberate: the components belong to
        // `openshard-world`, and inventing them here would mean inventing them
        // twice.
        debug!(?entity, %serial, name = play.name, "entering the world");

        let mut reply = Reply::default();
        reply.push(
            PlayerStart {
                serial: serial.raw(),
                body: BODY_HUMAN_MALE,
                position: walker.position,
                facing,
                map_width: DEFAULT_MAP_WIDTH,
                map_height: DEFAULT_MAP_HEIGHT,
            }
            .encode(),
        );
        reply.push(encode_map_change(MAP_FELUCCA));
        reply.push(
            PlayerUpdate {
                serial: serial.raw(),
                body: BODY_HUMAN_MALE,
                hue: 0x83EA,
                flags: 0,
                position: walker.position,
                facing,
            }
            .encode(),
        );
        reply.push(encode_light_level(LIGHT_DAY));
        reply.push(encode_login_complete());

        info!(%serial, name = play.name, position = %walker.position, "in world");
        Some((Player { serial, walker }, reply))
    }

    /// Handle `0x02`: try to take a step.
    pub fn walk(&mut self, player: &mut Player, request: WalkRequest) -> Reply {
        let mut reply = Reply::default();
        // Without client files there is no map, and `OpenWorld` allows
        // everything. That is a lie the server tells the client, and the reason
        // startup warns about it.
        let outcome = match &self.terrain {
            Some(terrain) => player.walker.request(request, terrain),
            None => player.walker.request(request, &OpenWorld),
        };
        match outcome {
            // A turn and a step are the same answer to the client: one ack.
            // They differ only in whether the position moved, and the walker
            // already knows that.
            Walk::Moved { facing, .. } | Walk::Turned { facing } => {
                // Notoriety 0x01 is "innocent" — the blue health bar.
                reply.push(encode_walk_ack(request.sequence, 0x01));
                debug!(%player.serial, %facing, position = %player.walker.position, "step");
            }
            Walk::Refused => {
                warn!(%player.serial, sequence = request.sequence, "step refused");
                reply.push(encode_walk_reject(
                    request.sequence,
                    player.walker.position,
                    player.walker.facing,
                ));
            }
        }
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play() -> CharacterPlay {
        CharacterPlay {
            name: "Lord British".to_owned(),
            slot: 0,
            client_ip: 0,
        }
    }

    fn walk(sequence: u8, direction: Direction) -> WalkRequest {
        WalkRequest {
            facing: Facing::walking(direction),
            sequence,
            fastwalk_key: 0,
        }
    }

    #[test]
    fn entering_the_world_sends_the_sequence_the_client_needs() {
        let mut game = Game::new();
        let (player, reply) = game.character_play(&play()).unwrap();

        let ids: Vec<u8> = reply.packets.iter().map(|p| p[0]).collect();
        assert_eq!(
            ids,
            vec![0x1B, 0xBF, 0x20, 0x4F, 0x55],
            "0x1B first or there is no body; 0x55 last or the client draws early"
        );
        assert!(player.serial.is_mobile());
    }

    #[test]
    fn a_player_starts_where_it_should() {
        let mut game = Game::new();
        let (player, _) = game.character_play(&play()).unwrap();
        assert_eq!(player.walker.position, START_POSITION);
        assert_eq!(player.walker.facing.direction, Direction::South);
    }

    #[test]
    fn every_player_gets_its_own_serial() {
        let mut game = Game::new();
        let (first, _) = game.character_play(&play()).unwrap();
        let (second, _) = game.character_play(&play()).unwrap();
        assert_ne!(first.serial, second.serial);
        assert_eq!(game.len(), 2);
    }

    #[test]
    fn walking_acks_and_moves() {
        let mut game = Game::new();
        let (mut player, _) = game.character_play(&play()).unwrap();

        // Facing south already, so this one steps.
        let reply = game.walk(&mut player, walk(0, Direction::South));
        assert_eq!(reply.packets, vec![vec![0x22, 0, 0x01]], "acked");
        assert_eq!(player.walker.position, Point::new(1475, 1775, 20));
    }

    #[test]
    fn turning_acks_without_moving() {
        let mut game = Game::new();
        let (mut player, _) = game.character_play(&play()).unwrap();

        let reply = game.walk(&mut player, walk(0, Direction::North));
        assert_eq!(
            reply.packets,
            vec![vec![0x22, 0, 0x01]],
            "a turn is acked too"
        );
        assert_eq!(player.walker.position, START_POSITION, "but does not move");
        assert_eq!(player.walker.facing.direction, Direction::North);
    }

    #[test]
    fn an_out_of_sequence_walk_is_rejected_with_the_real_position() {
        // The client snaps back to whatever the 0x21 says, so it had better be
        // where the server actually thinks the player is.
        let mut game = Game::new();
        let (mut player, _) = game.character_play(&play()).unwrap();

        let reply = game.walk(&mut player, walk(9, Direction::South));
        assert_eq!(reply.packets.len(), 1);
        let reject = &reply.packets[0];
        assert_eq!(reject[0], 0x21);
        assert_eq!(reject[1], 9, "echoes the sequence it refused");
        assert_eq!(&reject[2..4], &START_POSITION.x.to_be_bytes());
        assert_eq!(&reject[4..6], &START_POSITION.y.to_be_bytes());
        assert_eq!(reject[7] as i8, START_POSITION.z);
    }

    #[test]
    fn a_player_can_walk_a_full_lap_of_sequence_numbers() {
        // 300 steps crosses the 255-to-1 wrap, which is where a naive sequence
        // would silently start rejecting.
        let mut game = Game::new();
        let (mut player, _) = game.character_play(&play()).unwrap();

        let mut sequence = 0u8;
        for step in 0..300 {
            let reply = game.walk(&mut player, walk(sequence, Direction::South));
            assert_eq!(
                reply.packets[0][0], 0x22,
                "step {step} with sequence {sequence} was refused"
            );
            sequence = if sequence == u8::MAX { 1 } else { sequence + 1 };
        }
    }
}
