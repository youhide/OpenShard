//! Getting a character into the world, and walking it around.
//!
//! ```text
//!   client                                server
//!     │  0x5D character play                │
//!     │────────────────────────────────────>│
//!     │              0x1B start             │   puts the body in the world
//!     │<────────────────────────────────────│
//!     │              0xBF.0x08 map change   │
//!     │              0x20 player update     │
//!     │              0x4F light level       │
//!     │              0x55 login complete    │   the client starts drawing
//!     │<────────────────────────────────────│
//!     │  0x02 walk request                  │
//!     │────────────────────────────────────>│
//!     │              0x22 ack / 0x21 reject │
//!     │<────────────────────────────────────│
//! ```
//!
//! Layouts from SphereServer's `network/send.cpp` and `receive.cpp`.

use std::fmt;

use crate::codec::PacketWriter;
use crate::direction::Facing;
use crate::login::{expect_id, LoginDecodeError};

/// Where something is.
///
/// `z` is signed and one byte: UO's world is 256 units tall and the client has
/// no way to express more.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Point {
    /// East-west tile.
    pub x: u16,
    /// North-south tile.
    pub y: u16,
    /// Height.
    pub z: i8,
}

impl Point {
    /// A point.
    pub const fn new(x: u16, y: u16, z: i8) -> Self {
        Self { x, y, z }
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {})", self.x, self.y, self.z)
    }
}

// -- 0x5D character play --------------------------------------------------

/// `0x5D` — the client picks a character from the list. 73 bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CharacterPlay {
    /// The character's name, echoed from the 0xA9 list.
    pub name: String,
    /// Which slot, zero-based, into the list the server sent.
    pub slot: u32,
    /// The client's own claimed IPv4, as a raw dword. Not to be trusted or used.
    pub client_ip: u32,
}

impl CharacterPlay {
    /// The packet id.
    pub const ID: u8 = 0x5D;

    /// Decode a whole 0x5D packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        // A constant the client always sends. Sphere ignores it and so do we:
        // rejecting on it would be a compatibility risk for no gain.
        reader.skip(4)?;
        let name = reader.fixed_string(30)?;
        reader.skip(2)?; // unknown
        reader.skip(4)?; // client flags
        reader.skip(24)?; // unknown / login count
        let slot = reader.u32()?;
        let client_ip = reader.u32()?;
        Ok(Self {
            name,
            slot,
            client_ip,
        })
    }

    /// Encode a whole 0x5D packet.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(73);
        writer.u8(Self::ID);
        writer.u32(0xEDED_EDED); // the constant the client sends
        writer.fixed_string(&self.name, 30);
        writer.zeros(2);
        writer.zeros(4);
        writer.zeros(24);
        writer.u32(self.slot);
        writer.u32(self.client_ip);
        writer.into_bytes()
    }
}

// -- 0x1B start -----------------------------------------------------------

/// `0x1B` — put a body in the world. 37 bytes.
///
/// The first packet of the game proper. Until the client has this it has no
/// character and draws nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayerStart {
    /// The player's serial.
    pub serial: u32,
    /// The body graphic.
    pub body: u16,
    /// Where.
    pub position: Point,
    /// Which way, and whether running.
    pub facing: Facing,
    /// Map width in tiles.
    pub map_width: u16,
    /// Map height in tiles.
    pub map_height: u16,
}

/// The map size Sphere sends when it has nothing better: Britannia's.
pub const DEFAULT_MAP_WIDTH: u16 = 0x1800;
/// The map size Sphere sends when it has nothing better: Britannia's.
pub const DEFAULT_MAP_HEIGHT: u16 = 0x1000;

impl PlayerStart {
    /// The packet id.
    pub const ID: u8 = 0x1B;

    /// Encode a whole 0x1B packet.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(37);
        writer.u8(Self::ID);
        writer.u32(self.serial);
        writer.zeros(4);
        writer.u16(self.body);
        writer.u16(self.position.x);
        writer.u16(self.position.y);
        // The z field is two bytes wide but only the low one is read, as a
        // signed byte. Sphere writes a zero and then the byte; writing z as a
        // big-endian i16 would put -10 on the wire as 0xFFF6 and the client
        // would read 0xFF.
        writer.u8(0);
        writer.u8(self.position.z as u8);
        writer.u8(self.facing.to_bits());
        writer.zeros(1);
        writer.u32(0xFFFF_FFFF);
        writer.zeros(4);
        writer.u16(self.map_width);
        writer.u16(self.map_height);
        writer.zeros(6);
        writer.into_bytes()
    }
}

// -- 0x20 player update ---------------------------------------------------

/// `0x20` — move or redraw the player's own body. 19 bytes.
///
/// Also clears weather on the client, which is why Sphere's comment warns about
/// sending it casually.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayerUpdate {
    /// The player's serial.
    pub serial: u32,
    /// The body graphic.
    pub body: u16,
    /// The body hue.
    pub hue: u16,
    /// Status flags: poisoned, invisible, warmode.
    pub flags: u8,
    /// Where.
    pub position: Point,
    /// Which way, and whether running.
    pub facing: Facing,
}

impl PlayerUpdate {
    /// The packet id.
    pub const ID: u8 = 0x20;

    /// Encode a whole 0x20 packet.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(19);
        writer.u8(Self::ID);
        writer.u32(self.serial);
        writer.u16(self.body);
        writer.zeros(1);
        writer.u16(self.hue);
        writer.u8(self.flags);
        writer.u16(self.position.x);
        writer.u16(self.position.y);
        writer.zeros(2);
        writer.u8(self.facing.to_bits());
        writer.u8(self.position.z as u8);
        writer.into_bytes()
    }
}

// -- 0x02 walk request ----------------------------------------------------

/// `0x02` — the client asks to take one step. 7 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WalkRequest {
    /// Which way, and whether running.
    pub facing: Facing,
    /// The client's sequence number for this step. See `openshard-movement`.
    pub sequence: u8,
    /// The fastwalk key.
    ///
    /// Dead weight. It was a 1999 attempt to stop speed hacks, was broken
    /// immediately, and Sphere stopped reading it. Kept here only because the
    /// four bytes are on the wire.
    pub fastwalk_key: u32,
}

impl WalkRequest {
    /// The packet id.
    pub const ID: u8 = 0x02;

    /// Decode a whole 0x02 packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        Ok(Self {
            facing: Facing::from_bits(reader.u8()?),
            sequence: reader.u8()?,
            fastwalk_key: reader.u32()?,
        })
    }

    /// Encode a whole 0x02 packet.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(7);
        writer.u8(Self::ID);
        writer.u8(self.facing.to_bits());
        writer.u8(self.sequence);
        writer.u32(self.fastwalk_key);
        writer.into_bytes()
    }
}

/// `0x22` — the step is allowed. 3 bytes.
///
/// `notoriety` colours the player's own health bar.
pub fn encode_walk_ack(sequence: u8, notoriety: u8) -> Vec<u8> {
    vec![0x22, sequence, notoriety]
}

/// `0x21` — the step is refused; here is where you really are. 8 bytes.
///
/// The client snaps back to this position and resets its sequence to zero.
pub fn encode_walk_reject(sequence: u8, position: Point, facing: Facing) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(8);
    writer.u8(0x21);
    writer.u8(sequence);
    writer.u16(position.x);
    writer.u16(position.y);
    writer.u8(facing.to_bits());
    writer.u8(position.z as u8);
    writer.into_bytes()
}

// -- the rest of the entry sequence ---------------------------------------

/// `0x55` — the client may start drawing. 1 byte.
pub fn encode_login_complete() -> Vec<u8> {
    vec![0x55]
}

/// `0x4F` — overall light level. 2 bytes.
///
/// 0 is blinding daylight and 0x1F is pitch dark. Backwards from what the name
/// suggests, and the client clamps rather than complaining.
pub fn encode_light_level(level: u8) -> Vec<u8> {
    vec![0x4F, level]
}

/// `0xBF` subcommand 0x08 — which map the client should draw. 6 bytes.
///
/// Without this the client draws Felucca whatever the server thinks.
pub fn encode_map_change(map: u8) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(6);
    writer.u8(0xBF);
    writer.u16(6);
    writer.u16(0x08);
    writer.u8(map);
    writer.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direction::Direction;
    use crate::packet::{client_packet_length, PacketLength};

    fn facing() -> Facing {
        Facing::running(Direction::SouthEast)
    }

    #[test]
    fn character_play_round_trips_at_the_declared_length() {
        let play = CharacterPlay {
            name: "Lord British".to_owned(),
            slot: 0,
            client_ip: 0x0A00_0001,
        };
        let bytes = play.encode();
        assert_eq!(
            client_packet_length(CharacterPlay::ID),
            Some(PacketLength::Fixed(73))
        );
        assert_eq!(bytes.len(), 73, "the table and the encoder must agree");
        assert_eq!(CharacterPlay::decode(&bytes).unwrap(), play);
    }

    #[test]
    fn character_play_rejects_a_truncated_packet() {
        assert!(CharacterPlay::decode(&[0x5D, 0x00]).is_err());
    }

    #[test]
    fn player_start_matches_its_declared_length() {
        let start = PlayerStart {
            serial: 0x0000_0001,
            body: 0x0190,
            position: Point::new(1475, 1774, 0),
            facing: facing(),
            map_width: DEFAULT_MAP_WIDTH,
            map_height: DEFAULT_MAP_HEIGHT,
        };
        let bytes = start.encode();
        assert_eq!(bytes.len(), 37, "Sphere's PacketPlayerStart length");
        assert_eq!(bytes[0], 0x1B);
        assert_eq!(&bytes[1..5], &1u32.to_be_bytes());
        assert_eq!(&bytes[9..11], &0x0190u16.to_be_bytes(), "body");
        assert_eq!(&bytes[11..13], &1475u16.to_be_bytes(), "x");
        assert_eq!(&bytes[13..15], &1774u16.to_be_bytes(), "y");
        assert_eq!(bytes[17], facing().to_bits());
        assert_eq!(&bytes[19..23], &[0xFF; 4], "the 0xFFFFFFFF Sphere sends");
    }

    #[test]
    fn a_negative_z_survives_the_two_byte_field() {
        // The z field is two bytes but only the low one is read, as a signed
        // byte. Writing z as a big-endian i16 would put -10 on the wire as
        // 0xFFF6, and the client would take 0xFF — a height of -1.
        let start = PlayerStart {
            serial: 1,
            body: 0x0190,
            position: Point::new(100, 100, -10),
            facing: facing(),
            map_width: DEFAULT_MAP_WIDTH,
            map_height: DEFAULT_MAP_HEIGHT,
        };
        let bytes = start.encode();
        assert_eq!(bytes[15], 0x00, "the high byte is padding, not sign");
        assert_eq!(bytes[16] as i8, -10, "the low byte carries the height");
    }

    #[test]
    fn player_update_matches_its_declared_length() {
        let update = PlayerUpdate {
            serial: 1,
            body: 0x0190,
            hue: 0x83EA,
            flags: 0,
            position: Point::new(1475, 1774, -5),
            facing: facing(),
        };
        let bytes = update.encode();
        assert_eq!(bytes.len(), 19, "Sphere's PacketPlayerUpdate length");
        assert_eq!(bytes[0], 0x20);
        assert_eq!(&bytes[8..10], &0x83EAu16.to_be_bytes(), "hue");
        assert_eq!(bytes[17], facing().to_bits());
        assert_eq!(bytes[18] as i8, -5, "z is one signed byte here");
    }

    #[test]
    fn walk_request_round_trips_at_the_declared_length() {
        let request = WalkRequest {
            facing: facing(),
            sequence: 42,
            fastwalk_key: 0xDEAD_BEEF,
        };
        let bytes = request.encode();
        assert_eq!(
            client_packet_length(WalkRequest::ID),
            Some(PacketLength::Fixed(7))
        );
        assert_eq!(bytes.len(), 7);
        assert_eq!(WalkRequest::decode(&bytes).unwrap(), request);
    }

    #[test]
    fn walk_request_keeps_the_running_bit_out_of_the_direction() {
        let bytes = WalkRequest {
            facing: Facing::running(Direction::North),
            sequence: 0,
            fastwalk_key: 0,
        }
        .encode();
        assert_eq!(bytes[1], 0x80, "north, running");

        let decoded = WalkRequest::decode(&bytes).unwrap();
        assert_eq!(decoded.facing.direction, Direction::North);
        assert!(decoded.facing.running);
    }

    #[test]
    fn walk_ack_and_reject_match_their_declared_lengths() {
        assert_eq!(encode_walk_ack(7, 0x01), vec![0x22, 7, 0x01]);

        let reject = encode_walk_reject(7, Point::new(1475, 1774, -5), facing());
        assert_eq!(reject.len(), 8, "Sphere's PacketMovementRej length");
        assert_eq!(reject[0], 0x21);
        assert_eq!(reject[1], 7, "the sequence being rejected");
        assert_eq!(&reject[2..4], &1475u16.to_be_bytes());
        assert_eq!(&reject[4..6], &1774u16.to_be_bytes());
        assert_eq!(reject[6], facing().to_bits());
        assert_eq!(reject[7] as i8, -5);
    }

    #[test]
    fn the_small_entry_packets_are_the_right_shape() {
        assert_eq!(encode_login_complete(), vec![0x55]);
        assert_eq!(encode_light_level(0), vec![0x4F, 0]);

        // 0xBF is variable-length, so it declares its own length at offset 1.
        let map = encode_map_change(1);
        assert_eq!(map.len(), 6);
        assert_eq!(map[0], 0xBF);
        assert_eq!(
            u16::from_be_bytes([map[1], map[2]]),
            6,
            "declares its length"
        );
        assert_eq!(u16::from_be_bytes([map[3], map[4]]), 0x08, "subcommand");
        assert_eq!(map[5], 1, "Trammel");
    }

    #[test]
    fn a_point_at_the_edges_of_its_fields_encodes() {
        // z is the one that can go negative, and the map is 24 bits wide in
        // neither axis — u16 is the whole range the client has.
        let start = PlayerStart {
            serial: u32::MAX,
            body: u16::MAX,
            position: Point::new(u16::MAX, u16::MAX, i8::MIN),
            facing: Facing::walking(Direction::NorthWest),
            map_width: u16::MAX,
            map_height: u16::MAX,
        };
        assert_eq!(start.encode().len(), 37);

        let update = PlayerUpdate {
            serial: u32::MAX,
            body: u16::MAX,
            hue: u16::MAX,
            flags: u8::MAX,
            position: Point::new(u16::MAX, u16::MAX, i8::MAX),
            facing: Facing::walking(Direction::NorthWest),
        };
        assert_eq!(update.encode().len(), 19);
    }
}
