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
use std::num::NonZeroU8;

use serde::{
    Deserialize,
    Serialize,
};

use crate::access::OPENSHARD_SUBCOMMANDS;
use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::direction::Facing;
use crate::error::{
    DecodeError,
    WrongPacket,
};
use crate::identity::RawCharacterName;
use crate::login::CHARACTER_NAME_LENGTH;
use crate::mobile::{
    Notoriety,
    StatusFlags,
};
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
};
use crate::serial::Serial;
use crate::version::ClientVersion;
use crate::wire::{
    Graphic,
    Hue,
    RawCharacterSlot,
    RawClientIp,
    RawGraphic,
    RawHue,
    RawSkillId,
};

/// Where something is.
///
/// `z` is signed and one byte: UO's world is 256 units tall and the client has
/// no way to express more.
///
/// # Why its three fields stay bare integers
///
/// `Point` is itself the named type, and its components are the one thing a
/// coordinate is made of: numbers that get added to and compared. Wrapping each
/// axis would buy nothing — nothing reaches an `x` except through a `Point`, so
/// there is no call site at which it could be mistaken for a hue — and would
/// cost a `.0` on every step, every sector lookup and every line of sight in the
/// server. See the allowlist in `docs/protocol_newtypes.md`.
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

    /// UO's distance: Chebyshev, because the client draws a square.
    ///
    /// From Sphere's `GetDistSightBase`. A diagonal step covers the same
    /// distance as a straight one, which is also why diagonal movement costs no
    /// extra time. Height is not in it: two points one above the other are the
    /// same tile as far as reach, aggro and the view range are concerned.
    ///
    /// It lives on the point rather than in one of the crates that measure,
    /// because both ends of the wire measure and must agree: the shard decides a
    /// blow by it ([`RangedRange`] against this number), and the client's sight
    /// overlay draws where that decision changes.
    #[must_use]
    pub const fn distance(self, other: Self) -> u32 {
        let dx = self.x.abs_diff(other.x) as u32;
        let dy = self.y.abs_diff(other.y) as u32;
        match dx > dy {
            true => dx,
            false => dy,
        }
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {})", self.x, self.y, self.z)
    }
}

// -- 0x5D character play --------------------------------------------------

/// `0x5D` — the client picks a character from the list. 73 bytes.
///
/// Laid out here because this is where the conversation it opens is drawn, and
/// decoded on the other side of the split: it is a
/// [`LoginStagePacket::PlayCharacter`](crate::login::LoginStagePacket::PlayCharacter),
/// the last of the character screen's three, beside `0x00`/`0xF8` and `0x83`
/// whose bodies also live in this module. Nothing about a connection that sends
/// one is in the world yet.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CharacterPlay {
    /// The character's name, echoed from the 0xA9 list, not yet checked
    /// against the account's actual list — see [`RawCharacterName`]'s module
    /// docs; the lookup in the world (`tick::screen::play_character`) is the
    /// check.
    pub name:      RawCharacterName,
    /// Which slot, zero-based, into the list the server sent. Class D: the
    /// seam looks the character up by name, not by slot. See [`RawCharacterSlot`].
    pub slot:      RawCharacterSlot,
    /// The client's own claimed IPv4. Never trusted or used. See [`RawClientIp`].
    pub client_ip: RawClientIp,
}

impl DecodePacket for CharacterPlay {
    const ID: u8 = 0x5D;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        // A constant the client always sends. Sphere ignores it and so do we:
        // rejecting on it would be a compatibility risk for no gain.
        reader.skip(4)?;
        let name = RawCharacterName(reader.fixed_string(30)?);
        reader.skip(2)?; // unknown
        reader.skip(4)?; // client flags
        reader.skip(24)?; // unknown / login count
        let slot = RawCharacterSlot(reader.u32()?);
        let client_ip = RawClientIp(reader.u32()?);
        Ok(Self {
            name,
            slot,
            client_ip,
        })
    }
}

impl CharacterPlay {
    /// Encode a whole 0x5D packet. What `crates/client/net`'s login state
    /// machine sends for real — see `login`'s module docs: this server never
    /// sends one, only ever decodes it.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(73);
        writer.u8(Self::ID);
        writer.u32(0xEDED_EDED); // the constant the client sends
        writer.fixed_string(&self.name.0, 30);
        writer.zeros(2);
        writer.zeros(4);
        writer.zeros(24);
        writer.u32(self.slot.0);
        writer.u32(self.client_ip.0);
        writer.into_bytes()
    }
}

// -- 0x00 / 0xF8 create character -----------------------------------------

/// The race a player picked at character creation.
///
/// The world does not model races yet; this exists so the create packet can be
/// decoded without losing what the player chose, and so [`CreateCharacter::body`]
/// can pick the right graphic.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Race {
    /// The default, and the only one before Mondain's Legacy.
    Human,
    /// Since Mondain's Legacy.
    Elf,
    /// Since Stygian Abyss.
    Gargoyle,
}

/// A starting skill value exactly as sent — the client's own whole points,
/// not yet checked against the shard's starting-skill rule. No promotion
/// method yet — see `docs/protocol_newtypes.md`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawSkillValue(pub u8);

/// One starting skill a player chose at creation: which skill, and its value.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct SkillChoice {
    /// The skill id, as the client numbers them.
    pub skill: RawSkillId,
    /// Its starting value; the client sends whole points here. Stored raw.
    pub value: RawSkillValue,
}

/// `0x00` / `0xF8` — the client asks to create a character.
///
/// # Two ids, one packet
///
/// `0x00` is the classic 104-byte form with three starting skills. `0xF8` is
/// what ClassicUO 7.0.16 and later send — 106 bytes, with a fourth skill. The
/// two are otherwise byte-for-byte identical, so they decode through one path
/// that differs only by how many skill pairs it reads. Which id a client uses is
/// the client's business; the shard accepts both.
///
/// The sex/race byte is read with the Stygian Abyss encoding (`0x2`–`0x7`), what
/// every client that reaches character creation on a modern shard sends. A
/// genuinely pre-SA client using the old `0x0`–`0x3` encoding would have its race
/// read one off; that is a deliberate simplification while the world models no
/// races, noted here so it is a choice and not a surprise.
///
/// # Why this is not a [`DecodePacket`]
///
/// [`DecodePacket`] assumes one packet has one `const ID`. This one logically
/// decodes across *two* ids (`0x00`, `0x1F8`) with two different fixed lengths —
/// the same shape of problem the Stage 2 pilot hit with `0xB9`
/// (`docs/protocol_rewrite.md`, "Amendments forced by the Stage 2 pilot"), and
/// the Stage 3 pilot's counterpart to it. So [`Self::decode`] stays a plain
/// inherent method rather than bending the trait to fit two ids.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CreateCharacter {
    /// The new character's name.
    pub name:           RawCharacterName,
    /// Client flags reported at creation. Class D — never trusted, never read.
    pub flags:          ClientFlags,
    /// The chosen profession, or the "advanced"/custom option. See
    /// [`RawProfession::interpret`].
    pub profession:     RawProfession,
    /// The raw sex/race byte, exactly as sent. See [`RawSexRace::interpret`].
    pub sex_race:       RawSexRace,
    /// Starting strength, the client's own whole-point value.
    pub strength:       RawStatValue,
    /// Starting dexterity, the client's own whole-point value.
    pub dexterity:      RawStatValue,
    /// Starting intelligence, the client's own whole-point value.
    pub intelligence:   RawStatValue,
    /// The starting skills: three for `0x00`, four for `0xF8`.
    pub skills:         Vec<SkillChoice>,
    /// Skin hue.
    pub skin_hue:       RawHue,
    /// Hair graphic.
    pub hair:           RawGraphic,
    /// Hair hue.
    pub hair_hue:       RawHue,
    /// Facial-hair graphic.
    pub beard:          RawGraphic,
    /// Facial-hair hue.
    pub beard_hue:      RawHue,
    /// Which starting city the player picked, as an index into the list the
    /// character-list packet offered. No promotion method yet — see
    /// `docs/protocol_newtypes.md`.
    pub start_location: RawStartLocationIndex,
    /// Which character slot to fill. Class D — `create_character` fills the
    /// first free slot and does not read this. See [`RawCharacterSlot`].
    pub slot:           RawCharacterSlot,
    /// Shirt hue.
    pub shirt_hue:      RawHue,
    /// Trousers hue.
    pub pants_hue:      RawHue,
}

/// Client flags reported at character creation, exactly as sent. Never
/// trusted, never read — nothing downstream acts on them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ClientFlags(pub u32);

/// The profession byte exactly as a `0x00`/`0xF8` packet carried it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RawProfession(pub u8);

/// The profession a player picked at character creation.
///
/// The wire fixes exactly one distinction — zero means "advanced/custom" —
/// so this is as far as `protocol` interprets it. Naming the professions a
/// non-zero id refers to is Community Pack content, not this crate's business.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Profession {
    /// The player set stats and skills themselves.
    Custom,
    /// A shard-defined template, by its id.
    Predefined(u8),
}

impl RawProfession {
    /// Total: every byte value means something, including "custom".
    pub const fn interpret(self) -> Profession {
        match self.0 {
            0 => Profession::Custom,
            id => Profession::Predefined(id),
        }
    }
}

/// The sex/race byte exactly as a `0x00`/`0xF8` packet carried it, in the
/// Stygian Abyss encoding (`0x2`–`0x7`) every client that reaches character
/// creation on a modern shard sends. See [`CreateCharacter`]'s module docs for
/// the deliberate simplification this makes for a genuinely pre-SA client.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RawSexRace(pub u8);

/// The sex a player picked at character creation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Sex {
    Male,
    Female,
}

impl RawSexRace {
    /// Total: odd values are female on every client — Sphere notes this rule
    /// holds across versions — and anything the Stygian Abyss encoding does
    /// not name falls back to `(Male, Human)`, the safe default Sphere itself
    /// uses.
    pub const fn interpret(self) -> (Sex, Race) {
        let sex = if self.0.is_multiple_of(2) {
            Sex::Male
        } else {
            Sex::Female
        };
        let race = match self.0 {
            0x4 | 0x5 => Race::Elf,
            0x6 | 0x7 => Race::Gargoyle,
            _ => Race::Human,
        };
        (sex, race)
    }
}

/// A starting stat point exactly as sent: the client's own whole-point value,
/// not yet checked against the shard's starting-stat rule.
///
/// No promotion method yet — the per-stat floor/ceiling and total-points rule
/// that would produce one is gameplay balance this crate does not own; see
/// `docs/protocol_newtypes.md`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawStatValue(pub u8);

/// Which starting city index a create-character packet carried, not yet
/// checked against the list the character-list packet actually offered. No
/// promotion method yet — see `docs/protocol_newtypes.md`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawStartLocationIndex(pub u8);

impl CreateCharacter {
    /// The classic create-character id: 104 bytes, three skills.
    pub const ID_CLASSIC: u8 = 0x00;
    /// The 7.0.16+ create-character id: 106 bytes, four skills.
    pub const ID_HIGH_SEAS: u8 = 0xF8;

    /// Decode either the `0x00` or the `0xF8` create-character packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = PacketReader::new(bytes);
        let id = reader.u8()?;
        let skill_count = match id {
            Self::ID_CLASSIC => 3,
            Self::ID_HIGH_SEAS => 4,
            found => {
                return Err(DecodeError::WrongPacket(WrongPacket {
                    expected: Self::ID_HIGH_SEAS,
                    found,
                }));
            }
        };

        // pattern1 (4), pattern2 (4), a "kuoc" byte (1) — constants the client
        // sends and the server has no use for.
        reader.skip(9)?;
        let name = RawCharacterName(reader.fixed_string(CHARACTER_NAME_LENGTH)?);
        reader.skip(2)?; // 0x0000
        let flags = ClientFlags(reader.u32()?);
        reader.skip(8)?; // unknown
        let profession = RawProfession(reader.u8()?);
        reader.skip(15)?; // 0x00 * 15
        let sex_race = RawSexRace(reader.u8()?);
        let strength = RawStatValue(reader.u8()?);
        let dexterity = RawStatValue(reader.u8()?);
        let intelligence = RawStatValue(reader.u8()?);

        let mut skills = Vec::with_capacity(skill_count);
        for _ in 0..skill_count {
            let skill = RawSkillId(reader.u8()?);
            let value = RawSkillValue(reader.u8()?);
            skills.push(SkillChoice { skill, value });
        }

        let skin_hue = RawHue(reader.u16()?);
        let hair = RawGraphic(reader.u16()?);
        let hair_hue = RawHue(reader.u16()?);
        let beard = RawGraphic(reader.u16()?);
        let beard_hue = RawHue(reader.u16()?);
        reader.skip(1)?; // shard index
        let start_location = RawStartLocationIndex(reader.u8()?);
        let slot = RawCharacterSlot(reader.u32()?);
        reader.skip(4)?; // the client's claimed ip; not to be trusted
        let shirt_hue = RawHue(reader.u16()?);
        let pants_hue = RawHue(reader.u16()?);

        Ok(Self {
            name,
            flags,
            profession,
            sex_race,
            strength,
            dexterity,
            intelligence,
            skills,
            skin_hue,
            hair,
            hair_hue,
            beard,
            beard_hue,
            start_location,
            slot,
            shirt_hue,
            pants_hue,
        })
    }

    /// The body graphic for the given race and sex, as interpreted from the
    /// wire's sex/race byte. Replaces the old `is_female`/`race` methods on
    /// `Self` — see [`RawSexRace::interpret`].
    pub const fn body(sex: Sex, race: Race) -> u16 {
        match (race, sex) {
            (Race::Human, Sex::Male) => 0x0190,
            (Race::Human, Sex::Female) => 0x0191,
            (Race::Elf, Sex::Male) => 0x025D,
            (Race::Elf, Sex::Female) => 0x025E,
            (Race::Gargoyle, Sex::Male) => 0x029A,
            (Race::Gargoyle, Sex::Female) => 0x029B,
        }
    }

    /// Encode the packet. The `0xF8` (four-skill) form is written when four
    /// skills are present, the classic `0x00` form when three are present.
    /// Mostly for tests.
    ///
    /// # Panics
    ///
    /// If `skills` contains any number other than the three or four entries the
    /// two wire forms can represent.
    pub fn encode(&self) -> Vec<u8> {
        let (id, capacity) = match self.skills.len() {
            3 => (Self::ID_CLASSIC, 104),
            4 => (Self::ID_HIGH_SEAS, 106),
            count => {
                panic!("a create-character packet has {count} skills; its wire forms require exactly 3 or 4")
            }
        };
        let mut writer = PacketWriter::with_capacity(capacity);
        writer.u8(id);
        writer.zeros(9); // pattern1, pattern2, kuoc
        writer.fixed_string(&self.name.0, CHARACTER_NAME_LENGTH);
        writer.zeros(2);
        writer.u32(self.flags.0);
        writer.zeros(8);
        writer.u8(self.profession.0);
        writer.zeros(15);
        writer.u8(self.sex_race.0);
        writer.u8(self.strength.0);
        writer.u8(self.dexterity.0);
        writer.u8(self.intelligence.0);

        for choice in &self.skills {
            writer.u8(choice.skill.0);
            writer.u8(choice.value.0);
        }

        writer.u16(self.skin_hue.0);
        writer.u16(self.hair.0);
        writer.u16(self.hair_hue.0);
        writer.u16(self.beard.0);
        writer.u16(self.beard_hue.0);
        writer.zeros(1); // shard index
        writer.u8(self.start_location.0);
        writer.u32(self.slot.0);
        writer.zeros(4); // client ip
        writer.u16(self.shirt_hue.0);
        writer.u16(self.pants_hue.0);
        writer.into_bytes()
    }
}

// -- 0x1B start -----------------------------------------------------------

/// How big a facet is, in tiles.
///
/// The two numbers always travel together — both packets that carry a map size
/// carry both halves, and a client told a width without the matching height
/// draws the edge of the world in the wrong place — so they are one value rather
/// than two fields something has to keep in step. Its own components stay bare
/// integers for the same reason [`Point`]'s do.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MapSize {
    /// Width in tiles.
    pub width:  u16,
    /// Height in tiles.
    pub height: u16,
}

impl MapSize {
    /// Britannia's, which is what Sphere sends when it has nothing better.
    pub const BRITANNIA: Self = Self {
        width:  0x1800,
        height: 0x1000,
    };

    /// The size to tell `version` about `facet`, given its real width and
    /// height off the file.
    ///
    /// Every facet but two is told the truth outright: Ilshenar and Tokuno are
    /// not Britannia's shape either, and a client told the wrong one draws the
    /// world's edge wherever it likes. Felucca (`Facet(0)`) and Trammel
    /// (`Facet(1)`) are the one place a client's own version changes what
    /// "true" is — see [`ClientVersion::WIDE_MAP`]: below it, those two facets
    /// were 6144 tiles wide, not 7168, and a client told the modern number
    /// draws a world a thousand tiles wider than the one its own files hold.
    /// `width.min(6144)` rather than a flat substitution because a shard's
    /// files may already be the old 6144-wide generation, in which case there
    /// is nothing to clamp. Height never moved on either facet.
    #[must_use]
    pub fn for_client(facet: Facet, width: u32, height: u32, version: ClientVersion) -> Self {
        let width = match facet {
            Facet(0) | Facet(1) if version < ClientVersion::WIDE_MAP => width.min(6144),
            _ => width,
        };
        Self {
            width:  width as u16,
            height: height as u16,
        }
    }
}

/// `0x1B` — put a body in the world. 37 bytes.
///
/// The first packet of the game proper. Until the client has this it has no
/// character and draws nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayerStart {
    /// The player's serial.
    pub serial:   Serial,
    /// The body graphic.
    pub body:     Graphic,
    /// Where.
    pub position: Point,
    /// Which way, and whether running.
    pub facing:   Facing,
    /// How big the facet this character is on is — not Britannia's, unless it
    /// is on Britannia.
    pub map:      MapSize,
}

impl EncodePacket for PlayerStart {
    const ID: u8 = 0x1B;
    const LENGTH: PacketLength = PacketLength::Fixed(37);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.serial.raw());
        out.zeros(4);
        out.u16(self.body.0);
        out.u16(self.position.x);
        out.u16(self.position.y);
        // The z field is two bytes wide but only the low one is read, as a
        // signed byte. Sphere writes a zero and then the byte; writing z as a
        // big-endian i16 would put -10 on the wire as 0xFFF6 and the client
        // would read 0xFF.
        out.u8(0);
        out.u8(self.position.z as u8);
        out.u8(self.facing.to_bits());
        out.zeros(1);
        out.u32(0xFFFF_FFFF);
        out.zeros(4);
        out.u16(self.map.width);
        out.u16(self.map.height);
        out.zeros(6);
    }
}

impl DecodePacket for PlayerStart {
    const ID: u8 = 0x1B;

    /// The client's side of the first packet of the game proper.
    ///
    /// The z field is the trap: two bytes wide, and only the low one is read,
    /// as a *signed* byte. Reading the pair as an `i16` puts a dungeon floor at
    /// 65,526 instead of -10 — the mirror of the note on the encoder.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let serial = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x1B player serial",
            value: raw,
        })?;
        reader.skip(4)?;
        let body = Graphic(reader.u16()?);
        let x = reader.u16()?;
        let y = reader.u16()?;
        reader.skip(1)?; // the high half of z, which the client never reads
        let z = reader.u8()? as i8;
        let facing = Facing::from_bits(reader.u8()?);
        reader.skip(1)?;
        reader.skip(4)?; // 0xFFFFFFFF
        reader.skip(4)?;
        let map = MapSize {
            width:  reader.u16()?,
            height: reader.u16()?,
        };
        // The six trailing zeros are not read: nothing follows them in the
        // packet, and a frame that ended early is already a codec error.
        Ok(Self {
            serial,
            body,
            position: Point::new(x, y, z),
            facing,
            map,
        })
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
    pub serial:   Serial,
    /// The body graphic.
    pub body:     Graphic,
    /// The body hue.
    pub hue:      Hue,
    /// Status flags: poisoned, invisible, warmode.
    pub flags:    StatusFlags,
    /// Where.
    pub position: Point,
    /// Which way, and whether running.
    pub facing:   Facing,
}

impl EncodePacket for PlayerUpdate {
    const ID: u8 = 0x20;
    const LENGTH: PacketLength = PacketLength::Fixed(19);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.serial.raw());
        out.u16(self.body.0);
        out.zeros(1);
        out.u16(self.hue.0);
        out.u8(self.flags.0);
        out.u16(self.position.x);
        out.u16(self.position.y);
        out.zeros(2);
        out.u8(self.facing.to_bits());
        out.u8(self.position.z as u8);
    }
}

impl DecodePacket for PlayerUpdate {
    const ID: u8 = 0x20;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let serial = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x20 player update serial",
            value: raw,
        })?;
        let body = Graphic(reader.u16()?);
        reader.skip(1)?;
        let hue = Hue(reader.u16()?);
        let flags = StatusFlags(reader.u8()?);
        let x = reader.u16()?;
        let y = reader.u16()?;
        reader.skip(2)?;
        let facing = Facing::from_bits(reader.u8()?);
        let z = reader.u8()? as i8;
        Ok(Self {
            serial,
            body,
            hue,
            flags,
            position: Point::new(x, y, z),
            facing,
        })
    }
}

// -- 0x2C death status ----------------------------------------------------

/// `0x2C` — tell a client its own character just died, or came back. 2 bytes.
///
/// A death byte of `0` puts the client into ghost mode: it greys the world and
/// switches to the gliding ghost walk. `2` is the "alive again" answer that
/// resurrection sends to lift it. ServUO's `DeathStatus` — the one packet that
/// makes the whole screen read as death, so a ghost body drawn without it looks
/// merely like a recoloured player.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeathStatus {
    /// Whether the character is dead.
    pub dead: bool,
}

impl EncodePacket for DeathStatus {
    const ID: u8 = 0x2C;
    const LENGTH: PacketLength = PacketLength::Fixed(2);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(if self.dead { 0 } else { 2 });
    }
}

impl DecodePacket for DeathStatus {
    const ID: u8 = 0x2C;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            dead: reader.u8()? == 0,
        })
    }
}

// -- 0xAF death animation -------------------------------------------------

/// `0xAF` — a mobile died, and this is the corpse it leaves. 13 bytes.
///
/// The one packet that says which corpse was which body. Everything else about a
/// death is two unrelated facts on the wire — a mobile stops being drawn (`0x1D`)
/// and an item appears (`0x1A`) — and a client that wants to run the fall into
/// the body lying there has to pair them. Pairing them by *tile* is what a client
/// does without this packet, and two identical creatures dying on one tile in one
/// batch is enough to swap their falls.
///
/// ServUO sends it to every client in range except the dying player's own (`0x2C`
/// is what that client is told), and ClassicUO answers it by playing the death
/// group itself and holding the corpse item back until the animation is done —
/// `CorpseManager`, which is a serial pair and a direction, and nothing else.
///
/// Ours is not the mechanism that *starts* the fall: this shard sends the death
/// action as an ordinary animation (`0x6E`/`0xE2`) the moment combat announces
/// the death, and the corpse follows a tick later. What this packet adds is the
/// identity — which fall belongs to which corpse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeathAnimation {
    /// The mobile that died.
    pub killed:  Serial,
    /// The corpse it leaves.
    ///
    /// `None` for a death that leaves no body — ServUO writes a zero serial for
    /// one, and there genuinely is nothing to pair the fall with, which is not
    /// the same as a corpse whose serial we failed to learn.
    pub corpse:  Option<Serial>,
    /// Whether it fell in mid-run.
    ///
    /// A client with two death groups picks the second one for a running death
    /// (ClassicUO passes this straight into `GetDeathAction`). ServUO writes a
    /// plain zero here and never sets it; we send what the body was actually
    /// doing, which is a superset of that and costs nothing.
    pub running: bool,
}

impl EncodePacket for DeathAnimation {
    const ID: u8 = 0xAF;
    const LENGTH: PacketLength = PacketLength::Fixed(13);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.killed.raw());
        out.u32(self.corpse.map_or(0, Serial::raw));
        out.u32(u32::from(self.running));
    }
}

impl DecodePacket for DeathAnimation {
    const ID: u8 = 0xAF;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw_killed = reader.u32()?;
        let killed = Serial::new(raw_killed).ok_or(DecodeError::UnknownValue {
            field: "0xAF death animation serial",
            value: raw_killed,
        })?;
        // Zero is "no corpse" and every other value has to be a serial: a
        // corpse the sender named and this end could not read is a fall that
        // would silently pair with nothing.
        let raw_corpse = reader.u32()?;
        let corpse = match raw_corpse {
            0 => None,
            raw => {
                Some(Serial::new(raw).ok_or(DecodeError::UnknownValue {
                    field: "0xAF death animation corpse serial",
                    value: raw,
                })?)
            }
        };
        Ok(Self {
            killed,
            corpse,
            running: reader.u32()? != 0,
        })
    }
}

// -- 0x02 walk request ----------------------------------------------------

/// The sequence byte of a `0x02` walk request, exactly as the client sent it.
///
/// See [`RawStepSequence::interpret`] for why the promotion is total and why the
/// rule about sequences does not live in it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawStepSequence(pub u8);

/// The sequence byte a `0x22` ack or `0x21` reject carries back.
///
/// Always a number the client chose: the server never invents one, it echoes
/// the request's. The type exists to say that the byte reached an outbound
/// packet through the seam rather than out of nowhere.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct StepSequence(pub u8);

impl RawStepSequence {
    /// Total: every one of the 256 bytes is a legal tag.
    ///
    /// The sequence is an *echo* — the client owns the number, the server sends
    /// it back so the client can match an ack to the step that asked for it —
    /// so there is no domain to fall outside of and this promotion changes
    /// nothing but provenance. There *is* a rule about sequences (a fresh
    /// connection must open at zero, and a wrap skips it), but it refuses the
    /// **step**, not the value, and lives with the walk state machine in
    /// `openshard_movement::WalkSequence::accept`. Reflecting a byte a rule
    /// declined to accept is correct: a `0x21` names the step it is rejecting.
    pub const fn interpret(self) -> StepSequence {
        StepSequence(self.0)
    }
}

/// The fastwalk key a `0x02` carries, exactly as sent. Never read.
///
/// Dead weight on the wire. It was a 1999 attempt to stop speed hacks, was
/// broken immediately, and Sphere stopped reading it; this shard throttles by
/// pace (`openshard_movement::Pace`) instead. The type is the record of that
/// decision — it has no promotion because nothing is ever going to want one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawFastwalkKey(pub u32);

/// `0x02` — the client asks to take one step. 7 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WalkRequest {
    /// Which way, and whether running.
    pub facing:       Facing,
    /// The client's sequence number for this step. See `openshard-movement`.
    pub sequence:     RawStepSequence,
    /// The fastwalk key. Never read — see [`RawFastwalkKey`].
    pub fastwalk_key: RawFastwalkKey,
}

/// `0xBF.0xE014` — turn on the spot, never take a step.
///
/// The stock `0x02` asks for a direction and leaves the server to infer whether
/// that means a turn or a step from the mobile's facing when the packet arrives.
/// That is unsafe once combat can turn the same mobile between send and receive:
/// a request the client predicted as a turn can then be reinterpreted as a step.
/// OpenShard's client uses this typed request for the turn half of that exchange.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TurnRequest {
    /// Which way to face. The running bit is retained so the acknowledged pose
    /// is exactly the pose the client predicted, though it never makes this move.
    pub facing:   Facing,
    /// The shared walk/turn sequence number. Both request types use one ordered
    /// acknowledgement stream, so neither can overtake the other.
    pub sequence: RawStepSequence,
}

impl TurnRequest {
    /// The first OpenShard subcommand after the combat-action messages.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 20;

    /// Read the body after the extended envelope and subcommand.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            facing:   Facing::from_bits(reader.u8()?),
            sequence: RawStepSequence(reader.u8()?),
        })
    }

    /// Encode one complete typed turn request.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        crate::packet::frame_body(0xBF, PacketLength::Variable, |out| {
            out.u16(Self::SUBCOMMAND);
            out.u8(self.facing.to_bits());
            out.u8(self.sequence.0);
        })
    }
}

impl DecodePacket for WalkRequest {
    const ID: u8 = 0x02;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            facing:       Facing::from_bits(reader.u8()?),
            sequence:     RawStepSequence(reader.u8()?),
            fastwalk_key: RawFastwalkKey(reader.u32()?),
        })
    }
}

impl WalkRequest {
    /// Encode a whole 0x02 packet. What `crates/client/net`'s walk state
    /// machine (`walk.rs`) sends for real; this *server* never sends one, only
    /// ever decodes it.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(7);
        writer.u8(Self::ID);
        writer.u8(self.facing.to_bits());
        writer.u8(self.sequence.0);
        writer.u32(self.fastwalk_key.0);
        writer.into_bytes()
    }
}

/// `0x22` *from the client* — "tell me where I am". 3 bytes: the id and two of
/// nothing.
///
/// # One id, two packets
///
/// The same id going the other way is [`WalkAck`], which is also three bytes and
/// means nothing like this. No field distinguishes them — only the direction of
/// travel — and both references agree that this is how it is: ServUO registers
/// `0x22, 3, true, Resynchronize` beside the `0x22` it sends, and ClassicUO has
/// `Handler.Add(0x22, ConfirmWalk)` beside an `OutgoingPackets.Send_Resync`
/// writing the same id. They sit next to each other in this file so that nobody
/// finds one and assumes it is the other.
///
/// # What it is for
///
/// The repair leg of the walk handshake. A client that receives an ack it cannot
/// place has no way to work out where it really is — a `0x22` ack carries no
/// position — so it asks, and stops walking until it is told. The answer is a
/// `0x20`, everything in view again, and both sequences back to zero; ServUO's
/// `Resynchronize` is that list exactly. A server that ignores this leaves such a
/// client frozen for good, which is why `Walk` on our own client did not use it
/// until the shard could answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResyncRequest;

impl ResyncRequest {
    /// The id, shared with [`WalkAck`] in the other direction.
    pub const ID: u8 = 0x22;

    /// Encode the whole packet. What `crates/client/net`'s walk sends when it
    /// loses track of the server; this server only ever decodes it.
    pub fn encode(self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(3);
        writer.u8(Self::ID);
        // Two bytes the client fills with nothing. ServUO reads a fixed length
        // of three and looks at neither.
        writer.zeros(2);
        writer.into_bytes()
    }
}

/// `0x22` — the step is allowed. 3 bytes.
///
/// `notoriety` colours the player's own health bar. See [`ResyncRequest`] for
/// the unrelated packet that shares this id in the other direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WalkAck {
    /// The sequence number being acknowledged.
    pub sequence:  StepSequence,
    /// Colours the player's own health bar.
    pub notoriety: Notoriety,
}

impl EncodePacket for WalkAck {
    const ID: u8 = 0x22;
    const LENGTH: PacketLength = PacketLength::Fixed(3);

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        out.u8(self.sequence.0);
        // Through `for_client` for the same reason `0x77` and `0x78` are: a
        // client older than 4.0.0 draws no bar at all for a yellow one, and the
        // player's own bar going missing reads as the client being broken.
        out.u8(self.notoriety.for_client(version));
    }
}

impl DecodePacket for WalkAck {
    const ID: u8 = 0x22;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            // The sequence a client reads back out of an ack is a number it
            // chose itself and the server echoed, so it is a `StepSequence`
            // already — there is nothing to interpret. `RawStepSequence` is the
            // other direction, where the byte is a client's claim.
            sequence:  StepSequence(reader.u8()?),
            // Lossy in exactly one place, and knowingly: `for_client` sends a
            // yellow bar as blue to a client older than 4.0.0, so decoding what
            // such a client was sent gives `Innocent` back. That is what
            // arrived, and inventing the `Invulnerable` behind it would be a
            // guess the wire does not support.
            notoriety: Notoriety::from_bits(reader.u8()?),
        })
    }
}

/// `0x21` — the step is refused; here is where you really are. 8 bytes.
///
/// The client snaps back to this position and resets its sequence to zero.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WalkReject {
    /// The sequence number being refused.
    pub sequence: StepSequence,
    /// Where the client really is.
    pub position: Point,
    /// Which way it is really facing.
    pub facing:   Facing,
}

impl EncodePacket for WalkReject {
    const ID: u8 = 0x21;
    const LENGTH: PacketLength = PacketLength::Fixed(8);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(self.sequence.0);
        out.u16(self.position.x);
        out.u16(self.position.y);
        out.u8(self.facing.to_bits());
        out.u8(self.position.z as u8);
    }
}

impl DecodePacket for WalkReject {
    const ID: u8 = 0x21;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        // The facing sits *between* the y and the z, which is the one thing
        // about this packet worth reading twice: every other position on this
        // wire is three fields in a row.
        let sequence = StepSequence(reader.u8()?);
        let x = reader.u16()?;
        let y = reader.u16()?;
        let facing = Facing::from_bits(reader.u8()?);
        let z = reader.u8()? as i8;
        Ok(Self {
            sequence,
            position: Point::new(x, y, z),
            facing,
        })
    }
}

// -- the rest of the entry sequence ---------------------------------------

/// `0x55` — the client may start drawing. 1 byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LoginComplete;

impl EncodePacket for LoginComplete {
    const ID: u8 = 0x55;
    const LENGTH: PacketLength = PacketLength::Fixed(1);

    fn encode_body(&self, _out: &mut PacketWriter, _version: ClientVersion) {
    }
}

impl DecodePacket for LoginComplete {
    const ID: u8 = 0x55;

    /// One byte, all of it the id: the packet *is* the signal.
    fn decode_body(_reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self)
    }
}

/// How dark the world is drawn: `0` is blinding daylight and `0x1F` is pitch
/// dark.
///
/// Backwards from what the word suggests, which is exactly why it is a type: a
/// bare `u8` here reads as brightness to everyone who has not been told. Values
/// above `0x1F` are not refused — the client clamps them itself, and a shard
/// whose region data says `200` gets the dark it asked for rather than a
/// dropped packet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Light(pub u8);

/// `0x4F` — overall light level. 2 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LightLevel {
    /// How dark, in the client's own backwards scale. See [`Light`].
    pub level: Light,
}

impl EncodePacket for LightLevel {
    const ID: u8 = 0x4F;
    const LENGTH: PacketLength = PacketLength::Fixed(2);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(self.level.0);
    }
}

impl DecodePacket for LightLevel {
    const ID: u8 = 0x4F;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            level: Light(reader.u8()?),
        })
    }
}

/// The precipitation a shard asks the client to draw.
///
/// The client protocol reserves two values outside the four visible kinds:
/// `Temperature` changes only the thermometer, and `Clear` stops any existing
/// precipitation.  Keep both in the same domain rather than treating clear as
/// the absence of a packet: the latter is a state change a reconnecting client
/// must receive as explicitly as rain is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Weather {
    Rain,
    StormBrewing,
    Snow,
    Storm,
    Temperature,
    Clear,
}

impl Weather {
    /// The byte the classic weather packet uses for this condition.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Rain => 0,
            Self::StormBrewing => 1,
            Self::Snow => 2,
            Self::Storm => 3,
            Self::Temperature => 0xFE,
            Self::Clear => 0xFF,
        }
    }

    /// The conditions the classic client can name. Unknown values are refused:
    /// guessing that an unfamiliar effect is clear would leave a prior storm
    /// drawn forever, while guessing rain invents weather the shard never sent.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Rain),
            1 => Some(Self::StormBrewing),
            2 => Some(Self::Snow),
            3 => Some(Self::Storm),
            0xFE => Some(Self::Temperature),
            0xFF => Some(Self::Clear),
            _ => None,
        }
    }
}

/// `0x65` — change the weather. 4 bytes.
///
/// `intensity` is the classic client's particle-count byte. `temperature` is
/// kept even for precipitation packets because the wire carries it on every
/// change, and it makes a `Temperature` update a normal value rather than a
/// separate packet shape.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WeatherChange {
    pub weather:     Weather,
    pub intensity:   u8,
    pub temperature: u8,
}

impl EncodePacket for WeatherChange {
    const ID: u8 = 0x65;
    const LENGTH: PacketLength = PacketLength::Fixed(4);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(self.weather.to_bits());
        out.u8(self.intensity);
        out.u8(self.temperature);
    }
}

impl DecodePacket for WeatherChange {
    const ID: u8 = 0x65;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let kind = reader.u8()?;
        let weather = Weather::from_bits(kind).ok_or(DecodeError::UnknownValue {
            field: "0x65 weather kind",
            value: u32::from(kind),
        })?;
        Ok(Self {
            weather,
            intensity: reader.u8()?,
            temperature: reader.u8()?,
        })
    }
}

/// `0x6D` — play a music track. 3 bytes.
///
/// The id indexes the client's own music list (ServUO's `MusicName` enum order,
/// `Server/Region.cs`), so no filename travels — the client owns the tracks. Both
/// references agree byte for byte: Sphere's `PacketPlayMusic`, ServUO's
/// `PlayMusic`. Sent when a mobile crosses into a region whose track differs from
/// the one it was hearing; re-sending the same id restarts the track, which is
/// why the crossing pass compares before it sends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayMusic {
    /// Indexes the client's own music list.
    pub track: MusicId,
}

/// An index into the client's own music list — ServUO's `MusicName` order.
///
/// Not a filename and not a graphic: the tracks live in the client's files and
/// the server only ever names one by number.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct MusicId(pub u16);

impl EncodePacket for PlayMusic {
    const ID: u8 = 0x6D;
    const LENGTH: PacketLength = PacketLength::Fixed(3);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(self.track.0);
    }
}

impl DecodePacket for PlayMusic {
    const ID: u8 = 0x6D;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        Ok(Self {
            track: MusicId(reader.u16()?),
        })
    }
}

/// Which season the client draws its trees and ground in.
///
/// Five, and the client knows no others — a sixth byte draws nothing at all,
/// which is why `openshard_config` refuses one at startup rather than letting a
/// shard find out at world entry.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Season {
    Spring,
    Summer,
    Fall,
    Winter,
    /// The blighted look Felucca's dungeons use.
    Desolation,
}

impl Season {
    /// The wire byte.
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Spring => 0,
            Self::Summer => 1,
            Self::Fall => 2,
            Self::Winter => 3,
            Self::Desolation => 4,
        }
    }

    /// Read a season from its byte. Total, the way [`Notoriety::from_bits`] is:
    /// anything the client cannot draw falls back to spring rather than leaving
    /// the world in whatever it was last told.
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Summer,
            2 => Self::Fall,
            3 => Self::Winter,
            4 => Self::Desolation,
            _ => Self::Spring,
        }
    }

    /// Read a season from its byte, refusing anything the client cannot draw
    /// instead of falling back to spring — for a caller like config validation
    /// that needs to reject a bad byte rather than silently accept one.
    pub const fn try_from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Spring),
            1 => Some(Self::Summer),
            2 => Some(Self::Fall),
            3 => Some(Self::Winter),
            4 => Some(Self::Desolation),
            _ => None,
        }
    }
}

/// `0xBC` — which season the client draws. 3 bytes.
///
/// The second byte asks the client to play the season's own sound as it
/// changes; sending it on world entry with the sound off avoids announcing a
/// change that is really just a login. Ported from ServUO's `SeasonChange`,
/// whose name this takes so that [`Season`] can be the season itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SeasonChange {
    /// Which season.
    pub season:     Season,
    /// Whether to play the season's own change sound.
    pub play_sound: bool,
}

impl EncodePacket for SeasonChange {
    const ID: u8 = 0xBC;
    const LENGTH: PacketLength = PacketLength::Fixed(3);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(self.season.to_bits());
        out.bool(self.play_sound);
    }
}

/// `0xD1` — the logout the client asked for is granted. 2 bytes.
///
/// The client's own `0xD1` is a *notification*: it announces that the player
/// pressed "Log Out" on the paperdoll and then waits to be told it may go. Both
/// references answer with this same two-byte packet and nothing else — Sphere's
/// `PacketLogout::onReceive` constructs a `PacketLogoutAck`, ServUO's `LogoutReq`
/// sends a `LogoutAck` — and a server that stays silent leaves the client sitting
/// on the "logging out" screen until it times out, with nothing in any log to say
/// why.
///
/// The `0x01` is the accept. Refusing (a `0x00`, "you are in combat") is a rule
/// this shard does not have: the disconnect path already saves whatever state the
/// character is in, so there is nothing to protect by holding a player hostage.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LogoutAck;

/// `0xD1` *from the client* — "Log Out" was pressed. 2 bytes, no body.
///
/// The other half of [`LogoutAck`], and the same shape as [`ResyncRequest`]
/// beside [`WalkAck`]: one id, two packets, told apart only by which way they
/// travel. Nothing in it means anything — the reference client writes the id and
/// lets the length field zero-fill the rest — so this exists to give the client
/// half a name and one place that knows the id, rather than a literal in the
/// window's own code.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct LogoutRequest;

impl LogoutRequest {
    /// The id, shared with [`LogoutAck`] in the other direction.
    pub const ID: u8 = 0xD1;

    /// Encode the whole packet. What `crates/client/net` sends when the
    /// paperdoll's Log Out button is pressed; this server only ever decodes it,
    /// and decodes it as nothing but its id.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(2);
        writer.u8(Self::ID);
        // The one byte behind the id, which both references write and neither
        // reads — the client's table says the packet is two bytes long.
        writer.zeros(1);
        writer.into_bytes()
    }
}

impl EncodePacket for LogoutAck {
    const ID: u8 = 0xD1;
    const LENGTH: PacketLength = PacketLength::Fixed(2);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(0x01);
    }
}

impl DecodePacket for LogoutAck {
    const ID: u8 = 0xD1;

    /// The accept byte is read past and not kept. Refusing is a rule neither
    /// this shard nor either reference has — see the type's own docs — so a
    /// field carrying "it was a `0x01`" would be one nothing could ever branch
    /// on. What the packet means is that it arrived.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        reader.u8()?;
        Ok(Self)
    }
}

/// `0xBF` subcommand 0x08 — which map the client should draw. 6 bytes.
///
/// Without this the client draws Felucca whatever the server thinks.
///
/// # Fixed despite living under `0xBF`, and the length field is still hand-written
///
/// Every other `0xBF` packet this crate has seen so far is either genuinely
/// variable, or — like the `0xBF 0x19` stat-lock packet — fixed at a size the
/// `0xBF` envelope itself does not describe. This subcommand never carries a
/// list or a version-conditional tail, so its total size never moves: id,
/// length, subcommand, one map byte, six bytes always. `Fixed(6)` says that
/// directly, and is simpler than `Variable` for a body that never varies.
///
/// One consequence of choosing `Fixed`: [`crate::packet::frame_body`] only
/// back-patches a length field for [`PacketLength::Variable`], so this body
/// still writes its own `u16(6)` literal, exactly where `0xBF`'s general
/// envelope always puts one. It is a fixed constant here, not a length
/// [`frame_body`] computes — the two must simply agree, and a debug assert on
/// the body's total size (built into every `Fixed` payload) is what would
/// catch them drifting apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MapChange {
    /// Which map (facet) to draw.
    pub map: Facet,
}

/// Which facet the client draws, and which a mobile is on: `0` Felucca, `1`
/// Trammel, `2` Ilshenar, and so on up through whatever the shard's own files
/// hold.
///
/// The number indexes the client's `map*.mul`/`map*.uop` files, so its meaning
/// is fixed by what the player installed and not by anything the server
/// decides. One type for both the wire's own idea of a facet and the world's —
/// they used to be two (`world::MapId` here, `state::components::Facet`
/// separately), converted at every seam; see "Two types for one facet byte" in
/// `docs/protocol_newtypes.md`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Facet(pub u8);

/// Which *world* a facet's ground is — the base set it was imported from, named
/// by its own bytes.
///
/// Beside [`Facet`] and not inside the chunk wire, for [`Facet`]'s reason: it is
/// a fact everybody shares rather than a field one packet carries.
/// `openshard_basemap::identity_of` is what produces one, `WorldHome` is where a
/// shard keeps it, and [`WorldNotice`](crate::chunks::WorldNotice) is what tells
/// a client.
///
/// **A facet number is not an identity.** Two shards both serving facet 0 serve
/// two different Feluccas, and both call the first revision of it 1 — so a
/// client that kept a copy of one and compared revisions with the other would
/// draw a world nobody built. That is what this number separates, and it is the
/// whole of why it exists: a cache is filed under it.
///
/// It says nothing about how far a world has been *edited* — that is
/// [`WorldRevision`](crate::chunks::WorldRevision), and the two are asked
/// together. The base set never changes, so this does not either; the log beside
/// it is what moves.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct WorldId(pub u64);

/// How far, in tiles, a creature notices a foe.
///
/// Zero means the creature never initiates a fight. This is game data rather
/// than a client-packet field, but it crosses the scripting and persistence
/// seams, so the shared protocol crate owns its unambiguous representation.
/// The transparent serde form deliberately keeps existing script JSON and
/// saved records as numbers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default, Deserialize, Serialize)]
#[serde(transparent)]
pub struct Sight(pub u8);

/// Physical damage absorbed by a mobile, as a percentage.
///
/// The value crosses scripting and persistence seams as the historic numeric
/// percentage. Values above 100 have always been capped when a mobile enters
/// the world; doing that at this boundary keeps every caller canonical.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct PhysicalResistance(u8);

impl PhysicalResistance {
    /// The highest meaningful physical resistance percentage.
    pub const MAX: u8 = 100;

    /// Make a physical-resistance percentage, capping legacy out-of-range input.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(if value > Self::MAX { Self::MAX } else { value })
    }

    /// The percentage used by combat.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Serialize for PhysicalResistance {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.get())
    }
}

impl<'de> Deserialize<'de> for PhysicalResistance {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::new(u8::deserialize(deserializer)?))
    }
}

/// One of Ultima Online's five poison strengths, from lesser (`0`) through
/// lethal (`4`).
///
/// Scripts and saves keep their established numeric representation. Legacy
/// values above lethal are normalised at the boundary, matching the previous
/// runtime clamp in `combat::apply_poison`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct PoisonLevel(u8);

impl PoisonLevel {
    /// The strongest poison on the game's five-level scale.
    pub const LETHAL: Self = Self(4);

    /// Make a poison level, capping legacy out-of-range input at lethal.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(if value > Self::LETHAL.0 {
            Self::LETHAL.0
        } else {
            value
        })
    }

    /// The numeric level used by the historic script/save format.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Serialize for PoisonLevel {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.get())
    }
}

impl<'de> Deserialize<'de> for PoisonLevel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::new(u8::deserialize(deserializer)?))
    }
}

/// How much of a tamer's follower allowance one creature occupies.
///
/// A creature always occupies at least one slot. The persisted and scripting
/// representation remains numeric; decoding a legacy zero keeps the old
/// `npc::tame` behaviour, which already normalised it to one at creation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct FollowerSlots(u8);

impl FollowerSlots {
    /// One follower slot.
    pub const ONE: Self = Self(1);

    /// Make a follower-slot cost, normalising the invalid zero cost.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(if value == 0 { Self::ONE.0 } else { value })
    }

    /// The numeric value used by the historic save format and follower counter.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Serialize for FollowerSlots {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.get())
    }
}

impl<'de> Deserialize<'de> for FollowerSlots {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::new(u8::deserialize(deserializer)?))
    }
}

/// Whether a creature starts fights, only answers them, or runs from them.
///
/// The numeric representation is part of the script and saved-world contract:
/// `0` is passive, `1` defensive, and any other value is aggressive. Like
/// [`Sight`], this crosses the scripting and persistence seams while its serde
/// form deliberately remains the original numeric value.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Aggression {
    /// Never fights; runs from whoever hurts it.
    Passive,
    /// Fights only whoever attacked it first.
    Defensive,
    /// Attacks what it sees first.
    #[default]
    Aggressive,
}

impl Aggression {
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            0 => Self::Passive,
            1 => Self::Defensive,
            _ => Self::Aggressive,
        }
    }

    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Passive => 0,
            Self::Defensive => 1,
            Self::Aggressive => 2,
        }
    }
}

impl Serialize for Aggression {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.to_bits())
    }
}

impl<'de> Deserialize<'de> for Aggression {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_bits(u8::deserialize(deserializer)?))
    }
}

/// What kind of harm a blow does. Melee is [`Physical`](Self::Physical); a
/// spell, trap, or ranged creature picks its element.
///
/// Its numeric representation crosses the scripting and persistence seams:
/// physical is `0`, fire `1`, cold `2`, poison `3`, and energy `4`. Unknown
/// values retain the historic physical fallback.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum DamageType {
    /// A weapon or a fist.
    #[default]
    Physical,
    /// Fire.
    Fire,
    /// Cold.
    Cold,
    /// Poison.
    Poison,
    /// Energy.
    Energy,
}

impl DamageType {
    /// Read a damage type from a wire byte; anything unknown is physical.
    #[must_use]
    pub const fn from_u8(byte: u8) -> Self {
        match byte {
            1 => Self::Fire,
            2 => Self::Cold,
            3 => Self::Poison,
            4 => Self::Energy,
            _ => Self::Physical,
        }
    }

    /// The persisted/script byte for this damage type.
    #[must_use]
    pub const fn to_u8(self) -> u8 {
        match self {
            Self::Physical => 0,
            Self::Fire => 1,
            Self::Cold => 2,
            Self::Poison => 3,
            Self::Energy => 4,
        }
    }
}

impl Serialize for DamageType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.to_u8())
    }
}

impl<'de> Deserialize<'de> for DamageType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_u8(u8::deserialize(deserializer)?))
    }
}

/// A non-zero ranged attack reach, in tiles.
///
/// A creature without a ranged attack is represented by `None` at scripting
/// and persistence seams. The [`ranged`] serde helper preserves that existing
/// numeric contract as `0`, while every present reach is non-zero by type.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RangedRange(NonZeroU8);

impl RangedRange {
    /// Make a ranged reach. `0` means there is no ranged attack.
    #[must_use]
    pub const fn new(range: u8) -> Option<Self> {
        match NonZeroU8::new(range) {
            Some(range) => Some(Self(range)),
            None => None,
        }
    }

    /// The distance in tiles.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0.get()
    }
}

/// Serde support for an optional [`RangedRange`] at script and save seams.
///
/// `None` stays the legacy numeric `0`, rather than becoming JSON `null`.
pub mod ranged {
    use serde::{
        Deserialize,
        Deserializer,
        Serializer,
    };

    use super::RangedRange;

    /// Write `None` as `0` and a ranged reach as its numeric tile count.
    pub fn serialize<S: Serializer>(value: &Option<RangedRange>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(value.map_or(0, RangedRange::get))
    }

    /// Read `0` as no ranged attack and every other byte as a reach.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<RangedRange>, D::Error> {
        Ok(RangedRange::new(u8::deserialize(deserializer)?))
    }
}

impl fmt::Display for Facet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl EncodePacket for MapChange {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Fixed(6);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(6); // this subcommand's own, constant length
        out.u16(0x08);
        out.u8(self.map.0);
    }
}

/// `0x76` — the client has changed facet: where it now stands, and how big the
/// new world is. 16 bytes.
///
/// This is the packet a *facet change* needs and login does not. `0x1B` carries
/// the map size too, but it is the "you are entering the world" packet and
/// re-sending it mid-session restarts the session; ServUO's `Mobile.Map` setter
/// sends this instead, after the `0xBF 0x08` that says which map to draw.
///
/// Both references define it identically — ServUO's `ServerChange` and Sphere's
/// `PacketZoneChange` are the same sixteen bytes in the same order. They differ
/// only in that Sphere never sends it, its resync being `0xBF 0x08` and a
/// redraw; ServUO's is the one that actually changes maps at runtime, so this
/// follows ServUO.
///
/// The three zeroed fields after `z` are unused in every client that reads it.
#[must_use]
pub fn encode_server_change(at: Point, size: MapSize) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(SERVER_CHANGE_LENGTH.minimum());
    writer.u8(0x76);
    writer.u16(at.x);
    writer.u16(at.y);
    // Sign-extended, as ServUO's `(short)m.Z` is: a dungeon floor is negative,
    // and a zero-extended one puts the player 65,000 tiles in the air.
    writer.u16(i16::from(at.z) as u16);
    writer.zeros(5);
    writer.u16(size.width);
    writer.u16(size.height);
    debug_assert_eq!(writer.len(), SERVER_CHANGE_LENGTH.minimum());
    writer.into_bytes()
}

/// How [`encode_server_change`] is framed.
///
/// A hand-written packet still has to be readable from the other end, and the
/// client's framer needs this length before it can find where the next packet
/// starts. Naming it here keeps the size beside the code that writes it — a
/// number copied into a framing table is a number that can disagree with the
/// encoder.
pub const SERVER_CHANGE_LENGTH: PacketLength = PacketLength::Fixed(16);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direction::Direction;
    use crate::extended::ExtendedRequest;
    use crate::packet::{
        client_packet_length,
        decode_packet,
        encode_packet,
    };

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    fn facing() -> Facing {
        Facing::running(Direction::SouthEast)
    }

    #[test]
    fn a_typed_turn_round_trips_without_becoming_a_walk_request() {
        let request = TurnRequest {
            facing:   Facing::running(Direction::East),
            sequence: RawStepSequence(42),
        };
        let bytes = request.encode();
        assert_eq!(bytes[0], 0xBF);
        assert_eq!(&bytes[3..5], &TurnRequest::SUBCOMMAND.to_be_bytes());
        assert_eq!(
            ExtendedRequest::decode(&bytes),
            Ok(ExtendedRequest::Turn(request))
        );
    }

    /// A client below [`ClientVersion::WIDE_MAP`] is told Felucca and Trammel are
    /// 6144 wide even when the shard's own files are the modern 7168 generation —
    /// the gap `MapSize::for_client` closes. A modern client, and every other
    /// facet regardless of version, gets the file's own truth unchanged.
    #[test]
    fn an_old_client_is_told_felucca_and_trammel_are_the_old_width() {
        let old = ClientVersion::new(4, 0, 11, 3);
        assert_eq!(
            MapSize::for_client(Facet(0), 7168, 4096, old),
            MapSize {
                width:  6144,
                height: 4096,
            },
            "Felucca, one patch below the boundary"
        );
        assert_eq!(
            MapSize::for_client(Facet(1), 7168, 4096, old),
            MapSize {
                width:  6144,
                height: 4096,
            },
            "Trammel, the same rule"
        );
        assert_eq!(
            MapSize::for_client(Facet(0), 7168, 4096, ClientVersion::WIDE_MAP),
            MapSize {
                width:  7168,
                height: 4096,
            },
            "the boundary itself is the new width, not the old one"
        );
        assert_eq!(
            MapSize::for_client(Facet(2), 7168, 4096, old),
            MapSize {
                width:  7168,
                height: 4096,
            },
            "Ilshenar has no old-width rule to fall under"
        );
        assert_eq!(
            MapSize::for_client(Facet(0), 6144, 4096, old),
            MapSize {
                width:  6144,
                height: 4096,
            },
            "a shard already on the old map files has nothing to clamp"
        );
    }

    #[test]
    fn character_play_round_trips_at_the_declared_length() {
        let play = CharacterPlay {
            name:      RawCharacterName("Lord British".to_owned()),
            slot:      RawCharacterSlot(0),
            client_ip: RawClientIp(0x0A00_0001),
        };
        let bytes = play.encode();
        assert_eq!(
            client_packet_length(CharacterPlay::ID, None),
            Some(PacketLength::Fixed(73))
        );
        assert_eq!(bytes.len(), 73, "the table and the encoder must agree");
        assert_eq!(decode_packet::<CharacterPlay>(&bytes, version()).unwrap(), play);
    }

    #[test]
    fn character_play_rejects_a_truncated_packet() {
        assert!(decode_packet::<CharacterPlay>(&[0x5D, 0x00], version()).is_err());
    }

    fn sample_create(high_seas: bool) -> CreateCharacter {
        let mut skills = vec![
            SkillChoice {
                skill: RawSkillId(1),
                value: RawSkillValue(50),
            },
            SkillChoice {
                skill: RawSkillId(2),
                value: RawSkillValue(30),
            },
            SkillChoice {
                skill: RawSkillId(3),
                value: RawSkillValue(20),
            },
        ];
        if high_seas {
            skills.push(SkillChoice {
                skill: RawSkillId(4),
                value: RawSkillValue(0),
            });
        }
        CreateCharacter {
            name: RawCharacterName("Lord British".to_owned()),
            flags: ClientFlags(0x0000_001F),
            profession: RawProfession(1),
            sex_race: RawSexRace(0x3), // human female
            strength: RawStatValue(60),
            dexterity: RawStatValue(20),
            intelligence: RawStatValue(20),
            skills,
            skin_hue: RawHue(0x83EA),
            hair: RawGraphic(0x203B),
            hair_hue: RawHue(0x044E),
            beard: RawGraphic(0),
            beard_hue: RawHue(0),
            start_location: RawStartLocationIndex(0),
            slot: RawCharacterSlot(0),
            shirt_hue: RawHue(0x0386),
            pants_hue: RawHue(0x01BB),
        }
    }

    #[test]
    fn create_character_high_seas_round_trips_at_its_declared_length() {
        let create = sample_create(true);
        let bytes = create.encode();
        assert_eq!(bytes[0], CreateCharacter::ID_HIGH_SEAS);
        assert_eq!(bytes.len(), 106, "the 0xF8 form is 106 bytes, four skills");
        assert_eq!(
            client_packet_length(CreateCharacter::ID_HIGH_SEAS, None),
            Some(PacketLength::Fixed(106)),
            "the table and the encoder must agree"
        );
        assert_eq!(CreateCharacter::decode(&bytes).unwrap(), create);
    }

    #[test]
    fn create_character_classic_round_trips_at_its_declared_length() {
        let create = sample_create(false);
        let bytes = create.encode();
        assert_eq!(bytes[0], CreateCharacter::ID_CLASSIC);
        assert_eq!(bytes.len(), 104, "the 0x00 form is 104 bytes, three skills");
        assert_eq!(
            client_packet_length(CreateCharacter::ID_CLASSIC, None),
            Some(PacketLength::Fixed(104))
        );
        assert_eq!(CreateCharacter::decode(&bytes).unwrap(), create);
    }

    #[test]
    fn create_character_refuses_skill_counts_its_wire_cannot_represent() {
        for count in [2, 5] {
            let mut create = sample_create(false);
            create.skills.resize(count, SkillChoice::default());
            assert!(
                std::panic::catch_unwind(|| create.encode()).is_err(),
                "{count} skills must not be padded or truncated"
            );
        }
    }

    #[test]
    fn create_character_reads_the_name_and_skills_at_the_right_offsets() {
        // The whole risk in a fixed-layout packet is a field one byte out of
        // place, which shifts everything after it. Pin the name and the skills.
        let decoded = CreateCharacter::decode(&sample_create(true).encode()).unwrap();
        assert_eq!(decoded.name, "Lord British");
        assert_eq!(decoded.skin_hue, RawHue(0x83EA));
        assert_eq!(decoded.skills.len(), 4);
        assert_eq!(
            decoded.skills[0],
            SkillChoice {
                skill: RawSkillId(1),
                value: RawSkillValue(50),
            }
        );
        assert_eq!(decoded.start_location, RawStartLocationIndex(0));
    }

    #[test]
    fn create_character_maps_race_and_sex_to_a_body() {
        let human_female = CreateCharacter {
            sex_race: RawSexRace(0x3),
            ..sample_create(true)
        };
        let (sex, race) = human_female.sex_race.interpret();
        assert!(matches!(sex, Sex::Female));
        assert_eq!(race, Race::Human);
        assert_eq!(CreateCharacter::body(sex, race), 0x0191);

        let elf_male = CreateCharacter {
            sex_race: RawSexRace(0x4),
            ..sample_create(true)
        };
        let (sex, race) = elf_male.sex_race.interpret();
        assert!(matches!(sex, Sex::Male));
        assert_eq!(race, Race::Elf);
        assert_eq!(CreateCharacter::body(sex, race), 0x025D);

        let gargoyle_female = CreateCharacter {
            sex_race: RawSexRace(0x7),
            ..sample_create(true)
        };
        let (sex, race) = gargoyle_female.sex_race.interpret();
        assert!(matches!(sex, Sex::Female));
        assert_eq!(race, Race::Gargoyle);
        assert_eq!(CreateCharacter::body(sex, race), 0x029B);
    }

    #[test]
    fn create_character_rejects_a_truncated_packet() {
        assert!(CreateCharacter::decode(&[CreateCharacter::ID_HIGH_SEAS, 0x00]).is_err());
    }

    #[test]
    fn create_character_rejects_the_wrong_id() {
        let mut bytes = sample_create(true).encode();
        bytes[0] = 0x5D;
        assert!(matches!(
            CreateCharacter::decode(&bytes),
            Err(DecodeError::WrongPacket(_))
        ));
    }

    /// A serial every test in here can use: valid, and in the mobile pool.
    fn serial() -> Serial {
        Serial::new(0x0000_0001).unwrap()
    }

    #[test]
    fn player_start_matches_its_declared_length() {
        let start = PlayerStart {
            serial:   serial(),
            body:     Graphic(0x0190),
            position: Point::new(1475, 1774, 0),
            facing:   facing(),
            map:      MapSize::BRITANNIA,
        };
        let bytes = encode_packet(&start, version());
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
            serial:   serial(),
            body:     Graphic(0x0190),
            position: Point::new(100, 100, -10),
            facing:   facing(),
            map:      MapSize::BRITANNIA,
        };
        let bytes = encode_packet(&start, version());
        assert_eq!(bytes[15], 0x00, "the high byte is padding, not sign");
        assert_eq!(bytes[16] as i8, -10, "the low byte carries the height");
    }

    #[test]
    fn player_update_matches_its_declared_length() {
        let update = PlayerUpdate {
            serial:   serial(),
            body:     Graphic(0x0190),
            hue:      Hue(0x83EA),
            flags:    StatusFlags::NONE,
            position: Point::new(1475, 1774, -5),
            facing:   facing(),
        };
        let bytes = encode_packet(&update, version());
        assert_eq!(bytes.len(), 19, "Sphere's PacketPlayerUpdate length");
        assert_eq!(bytes[0], 0x20);
        assert_eq!(&bytes[8..10], &0x83EAu16.to_be_bytes(), "hue");
        assert_eq!(bytes[17], facing().to_bits());
        assert_eq!(bytes[18] as i8, -5, "z is one signed byte here");
    }

    #[test]
    fn death_status_is_two_bytes_dead_is_zero() {
        let dead = encode_packet(&DeathStatus { dead: true }, version());
        assert_eq!(dead, vec![0x2C, 0x00], "0 puts the client in ghost mode");
        let alive = encode_packet(&DeathStatus { dead: false }, version());
        assert_eq!(alive, vec![0x2C, 0x02], "2 is the alive-again answer");
    }

    #[test]
    fn a_death_animation_names_the_body_and_the_corpse() {
        // The pairing is the whole packet: without it a client watching two
        // identical creatures fall on one tile cannot tell which corpse is which.
        let death = DeathAnimation {
            killed:  Serial::new(0x0000_02BC).unwrap(),
            corpse:  Some(Serial::new(0x4000_0001).unwrap()),
            running: true,
        };
        let bytes = encode_packet(&death, version());

        assert_eq!(bytes.len(), 13);
        assert_eq!(bytes[0], 0xAF);
        assert_eq!(&bytes[1..5], &0x0000_02BCu32.to_be_bytes(), "the body");
        assert_eq!(&bytes[5..9], &0x4000_0001u32.to_be_bytes(), "its corpse");
        assert_eq!(&bytes[9..13], &1u32.to_be_bytes(), "it fell running");

        let mut reader = PacketReader::new(&bytes[1..]);
        assert_eq!(
            DeathAnimation::decode_body(&mut reader, version()).unwrap(),
            death
        );
    }

    #[test]
    fn a_death_that_leaves_no_body_sends_a_zero_serial() {
        // ServUO's `Serial.Zero` for a mobile whose corpse never got made. It has
        // to survive as "no corpse" rather than as serial zero, which is not a
        // serial at all.
        let death = DeathAnimation {
            killed:  Serial::new(0x0000_02BC).unwrap(),
            corpse:  None,
            running: false,
        };
        let bytes = encode_packet(&death, version());

        assert_eq!(&bytes[5..9], &[0, 0, 0, 0], "no corpse");
        let mut reader = PacketReader::new(&bytes[1..]);
        assert_eq!(
            DeathAnimation::decode_body(&mut reader, version()).unwrap(),
            death
        );
    }

    #[test]
    fn walk_request_round_trips_at_the_declared_length() {
        let request = WalkRequest {
            facing:       facing(),
            sequence:     RawStepSequence(42),
            fastwalk_key: RawFastwalkKey(0xDEAD_BEEF),
        };
        let bytes = request.encode();
        assert_eq!(
            client_packet_length(WalkRequest::ID, None),
            Some(PacketLength::Fixed(7))
        );
        assert_eq!(bytes.len(), 7);
        assert_eq!(decode_packet::<WalkRequest>(&bytes, version()).unwrap(), request);
    }

    #[test]
    fn walk_request_keeps_the_running_bit_out_of_the_direction() {
        let bytes = WalkRequest {
            facing:       Facing::running(Direction::North),
            sequence:     RawStepSequence(0),
            fastwalk_key: RawFastwalkKey(0),
        }
        .encode();
        assert_eq!(bytes[1], 0x80, "north, running");

        let decoded = decode_packet::<WalkRequest>(&bytes, version()).unwrap();
        assert_eq!(decoded.facing.direction, Direction::North);
        assert!(decoded.facing.running);
    }

    #[test]
    fn walk_ack_and_reject_match_their_declared_lengths() {
        assert_eq!(
            encode_packet(
                &WalkAck {
                    sequence:  StepSequence(7),
                    notoriety: Notoriety::Innocent,
                },
                version()
            ),
            vec![0x22, 7, 0x01]
        );

        let reject = encode_packet(
            &WalkReject {
                sequence: StepSequence(7),
                position: Point::new(1475, 1774, -5),
                facing:   facing(),
            },
            version(),
        );
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
        assert_eq!(encode_packet(&LoginComplete, version()), vec![0x55]);
        assert_eq!(
            encode_packet(&LightLevel { level: Light(0) }, version()),
            vec![0x4F, 0]
        );
        // Music and season: three bytes each, the track big-endian. Both
        // references write exactly this.
        assert_eq!(
            encode_packet(&PlayMusic { track: MusicId(11) }, version()),
            vec![0x6D, 0x00, 11]
        );
        assert_eq!(
            encode_packet(
                &PlayMusic {
                    track: MusicId(0x0102),
                },
                version()
            ),
            vec![0x6D, 0x01, 0x02]
        );
        assert_eq!(
            encode_packet(
                &SeasonChange {
                    season:     Season::Winter,
                    play_sound: true,
                },
                version()
            ),
            vec![0xBC, 3, 1]
        );
        assert_eq!(
            encode_packet(
                &SeasonChange {
                    season:     Season::Spring,
                    play_sound: false,
                },
                version()
            ),
            vec![0xBC, 0, 0]
        );
        // The logout ack is the same two bytes in both references, and the same
        // length the client's own table gives the id it comes back on.
        assert_eq!(encode_packet(&LogoutAck, version()), vec![0xD1, 0x01]);
        assert_eq!(
            crate::packet::client_packet_length(0xD1, None),
            Some(crate::packet::PacketLength::Fixed(2))
        );

        // 0xBF is variable-length on the client's own table, but this
        // subcommand's own body never varies, so it declares its own length at
        // offset 1 the same way every other fixed packet does.
        let map = encode_packet(&MapChange { map: Facet(1) }, version());
        assert_eq!(map.len(), 6);
        assert_eq!(map[0], 0xBF);
        assert_eq!(u16::from_be_bytes([map[1], map[2]]), 6, "declares its length");
        assert_eq!(u16::from_be_bytes([map[3], map[4]]), 0x08, "subcommand");
        assert_eq!(map[5], 1, "Trammel");
    }

    /// The facet-change packet, byte for byte.
    ///
    /// ServUO's `ServerChange` and Sphere's `PacketZoneChange` agree exactly on
    /// this layout, which is as close to a specification as this genre gets, so
    /// it is worth pinning rather than trusting a reading of either.
    #[test]
    fn the_server_change_says_where_and_how_big() {
        let packet = encode_server_change(
            Point::new(1495, 1629, -20),
            MapSize {
                width:  2304,
                height: 1600,
            },
        );

        assert_eq!(packet.len(), 16, "fixed at sixteen bytes");
        assert_eq!(packet[0], 0x76);
        assert_eq!(u16::from_be_bytes([packet[1], packet[2]]), 1495, "x");
        assert_eq!(u16::from_be_bytes([packet[3], packet[4]]), 1629, "y");
        assert_eq!(
            i16::from_be_bytes([packet[5], packet[6]]),
            -20,
            "z is signed — a dungeon floor is below zero"
        );
        assert_eq!(&packet[7..12], &[0; 5], "three unused fields");
        assert_eq!(
            u16::from_be_bytes([packet[12], packet[13]]),
            2304,
            "Ilshenar's width, not Britannia's"
        );
        assert_eq!(u16::from_be_bytes([packet[14], packet[15]]), 1600, "height");
    }

    #[test]
    fn a_point_at_the_edges_of_its_fields_encodes() {
        // z is the one that can go negative, and the map is 24 bits wide in
        // neither axis — u16 is the whole range the client has. The serial is
        // the top of the item pool rather than `u32::MAX`, because `Serial`
        // refuses `0xFFFFFFFF`: that value means "nothing", not "the last
        // object".
        let highest = Serial::new(crate::serial::ITEM_MAX).unwrap();
        let start = PlayerStart {
            serial:   highest,
            body:     Graphic(u16::MAX),
            position: Point::new(u16::MAX, u16::MAX, i8::MIN),
            facing:   Facing::walking(Direction::NorthWest),
            map:      MapSize {
                width:  u16::MAX,
                height: u16::MAX,
            },
        };
        assert_eq!(encode_packet(&start, version()).len(), 37);

        let update = PlayerUpdate {
            serial:   highest,
            body:     Graphic(u16::MAX),
            hue:      Hue(u16::MAX),
            flags:    StatusFlags(u8::MAX),
            position: Point::new(u16::MAX, u16::MAX, i8::MAX),
            facing:   Facing::walking(Direction::NorthWest),
        };
        assert_eq!(encode_packet(&update, version()).len(), 19);
    }

    #[test]
    fn every_sequence_byte_survives_interpretation() {
        // Class B, and total: the walk sequence has no out-of-domain value to
        // refuse, so the promotion must be defined — and unchanged — for all
        // 256. A gap here would be a step the server silently renumbered, and
        // the client matches its ack by that number.
        for byte in 0..=u8::MAX {
            assert_eq!(RawStepSequence(byte).interpret(), StepSequence(byte), "{byte}");
        }
    }

    #[test]
    fn a_walk_ack_downgrades_a_yellow_bar_for_an_old_client() {
        // The player's own health bar goes through `for_client` for the same
        // reason another mobile's does: a client below 4.0.0 draws nothing at
        // all for 0x07, and a missing bar on your own character reads as the
        // client being broken rather than as a server sending a colour it
        // cannot draw.
        let ack = WalkAck {
            sequence:  StepSequence(3),
            notoriety: Notoriety::Invulnerable,
        };
        let ancient = ClientVersion::new(3, 0, 0, 0);
        assert_eq!(encode_packet(&ack, ancient), vec![0x22, 3, 0x01], "blue instead");
        assert_eq!(
            encode_packet(&ack, version()),
            vec![0x22, 3, 0x07],
            "a modern client gets the yellow it can draw"
        );
    }

    #[test]
    fn the_seasons_are_the_clients_own_five() {
        for (season, bits) in [
            (Season::Spring, 0),
            (Season::Summer, 1),
            (Season::Fall, 2),
            (Season::Winter, 3),
            (Season::Desolation, 4),
        ] {
            assert_eq!(season.to_bits(), bits, "{season:?}");
            assert_eq!(Season::from_bits(bits), season);
            assert_eq!(Season::try_from_bits(bits), Some(season));
        }
        // Total, and the fallback is the one the client can always draw. A
        // shard config cannot reach here — `openshard_config` refuses a sixth
        // season at startup — but a script or a save from another shard can.
        assert_eq!(Season::from_bits(5), Season::Spring);
        assert_eq!(Season::from_bits(u8::MAX), Season::Spring);
        // `try_from_bits` is what config validation uses instead: unlike
        // `from_bits` it refuses rather than silently drawing spring.
        assert_eq!(Season::try_from_bits(5), None);
        assert_eq!(Season::try_from_bits(u8::MAX), None);
    }

    #[test]
    fn weather_keeps_the_classic_control_values_distinct() {
        for (weather, bits) in [
            (Weather::Rain, 0),
            (Weather::StormBrewing, 1),
            (Weather::Snow, 2),
            (Weather::Storm, 3),
            (Weather::Temperature, 0xFE),
            (Weather::Clear, 0xFF),
        ] {
            assert_eq!(weather.to_bits(), bits);
            assert_eq!(Weather::from_bits(bits), Some(weather));
        }
        assert_eq!(Weather::from_bits(4), None);
    }
}
