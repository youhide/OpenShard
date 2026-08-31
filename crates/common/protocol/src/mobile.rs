//! Other mobiles: drawing them, moving them, taking them away.
//!
//! These are what make a shard a place rather than a single-player map viewer.
//! The client draws its own character from `0x1B`/`0x20`; everyone *else* comes
//! from here.

use std::fmt;

use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
};

use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::direction::Facing;
use crate::error::DecodeError;
use crate::feature::Feature;
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
};
use crate::serial::{
    RawSerial,
    Serial,
};
use crate::skill::SkillLock;
use crate::version::ClientVersion;
use crate::wire::{
    Graphic,
    Hue,
    Layer,
};
use crate::world::Point;

/// Status flags on a mobile: poisoned, invisible, in war mode.
///
/// A byte with named constants rather than an enum — the same shape, and for the
/// same reason, as [`PaperdollFlags`]: the bits are independent, and a mobile
/// that is poisoned *and* at war sets two of them.
///
/// # The whole table, and the one bit this engine sets
///
/// The eight are the client's, read off ClassicUO's `EntityFlags`:
///
/// | bit | meaning |
/// |---|---|
/// | `0x01` | frozen — the client refuses to walk it |
/// | `0x02` | female |
/// | `0x04` | poisoned (a green health bar), and *flying* on a gargoyle body: one bit, two readings, decided by the body |
/// | `0x08` | a yellow health bar — invulnerable |
/// | `0x10` | walk through other mobiles |
/// | `0x20` | movable, on a body that would not otherwise be |
/// | `0x40` | in war mode |
/// | `0x80` | hidden |
///
/// **Two of the eight are set by this engine.**
///
/// [`WARMODE`](Self::WARMODE), because a body's stance is the one of the eight
/// that has a picture at this end: a client draws a human at war standing in a
/// different animation group (`docs/combat.md`, D1 and D2).
///
/// [`IGNORE_MOBILES`](Self::IGNORE_MOBILES), because the shard now has a rule
/// the client would otherwise contradict. A body in the way stops a step
/// (`docs/roadmap.md`, *a mobile is not an obstacle*) and staff are exempt, and
/// the client applies its own copy of that rule to what it draws and to what it
/// predicts — so without this bit a game master walking into an NPC is allowed
/// by the shard and refused by the client they are looking at. ServUO carries
/// the same fact in the same bit, off a saved per-mobile `IgnoreMobiles`
/// (`Server/Mobile.cs:8802`).
///
/// The other six are written down so that the day one of them is wanted, its
/// bit is a fact already looked up rather than a guess — but a constant nobody
/// sets is a constant nobody has tested, so they stay in this table and out of
/// the type until somebody has a use.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct StatusFlags(pub u8);

impl StatusFlags {
    /// Nothing set: not poisoned, not hidden, not in war mode.
    pub const NONE: Self = Self(0);
    /// The mobile stands in war mode.
    ///
    /// **Not [`PaperdollFlags::WARMODE`]**, which is `0x01` of a *different*
    /// byte in a different packet. The same fact travels in both, and the two
    /// bits are not the same bit — which is exactly why each is a named constant
    /// on the flags type that carries it.
    pub const WARMODE: Self = Self(0x40);

    /// The mobile walks through other mobiles.
    ///
    /// What tells the client not to apply its own body-blocking rule to this
    /// one. The shard's answer is its `Staff` component — the flag a `.gm` puts
    /// down — and the client's has to agree, or a game master's step is allowed
    /// by one end and refused by the other.
    pub const IGNORE_MOBILES: Self = Self(0x10);

    /// Both sets of bits. A named method rather than `BitOr`, for
    /// [`PaperdollFlags::with`]'s reason.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit of `other` is set here.
    #[must_use]
    pub const fn has(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The flags for a mobile that is or is not at war — the whole of what this
    /// engine has to say about the byte, written once so that three senders
    /// cannot disagree about which bit it is.
    #[must_use]
    pub const fn of_stance(war: bool) -> Self {
        if war { Self::WARMODE } else { Self::NONE }
    }
}

/// How the client colours a mobile's health bar.
///
/// The client renders these; the meanings are its, not ours.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[non_exhaustive]
pub enum Notoriety {
    /// Blue.
    #[default]
    Innocent,
    /// Green.
    Friend,
    /// Grey: an animal, or a non-player that has not been provoked.
    Neutral,
    /// Grey: attackable.
    Criminal,
    /// Orange.
    Enemy,
    /// Red.
    Murderer,
    /// Yellow. Only rendered since 4.0.0 — see [`Feature::NotorietyInvulnerable`].
    Invulnerable,
}

impl Notoriety {
    /// The wire byte.
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Innocent => 0x01,
            Self::Friend => 0x02,
            Self::Neutral => 0x03,
            Self::Criminal => 0x04,
            Self::Enemy => 0x05,
            Self::Murderer => 0x06,
            Self::Invulnerable => 0x07,
        }
    }

    /// Read a notoriety from its wire byte. Anything unrecognised — including the
    /// `0x00` a caller uses for "unset" — reads as [`Innocent`](Self::Innocent),
    /// the safe default a blue health bar.
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            0x02 => Self::Friend,
            0x03 => Self::Neutral,
            0x04 => Self::Criminal,
            0x05 => Self::Enemy,
            0x06 => Self::Murderer,
            0x07 => Self::Invulnerable,
            _ => Self::Innocent,
        }
    }

    /// What to actually send to `version`.
    ///
    /// A client older than 4.0.0 has no yellow bar and renders `0x07` as
    /// nothing at all — the mobile gets no health bar and looks like a bug.
    /// Downgrading to blue is a small lie that draws.
    pub fn for_client(self, version: ClientVersion) -> u8 {
        if self == Self::Invulnerable && !version.supports(Feature::NotorietyInvulnerable) {
            return Self::Innocent.to_bits();
        }
        self.to_bits()
    }

    /// The hue the client draws a mobile's *name* in — the colour a single-click
    /// label comes back in, matching the health bar. ServUO's `Notoriety.Hues`,
    /// indexed by the same wire byte: blue innocent, green friend, grey
    /// neutral/criminal, orange enemy, red murderer, yellow invulnerable.
    pub const fn name_hue(self) -> Hue {
        match self {
            Self::Innocent => Hue(0x0059),
            Self::Friend => Hue(0x003F),
            Self::Neutral | Self::Criminal => Hue(0x03B2),
            Self::Enemy => Hue(0x0090),
            Self::Murderer => Hue(0x0022),
            Self::Invulnerable => Hue(0x0035),
        }
    }
}

impl Serialize for Notoriety {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.to_bits())
    }
}

impl<'de> Deserialize<'de> for Notoriety {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_bits(u8::deserialize(deserializer)?))
    }
}

/// One of the three arrows the status bar draws, as the wire numbers them.
///
/// The client's own domain and a closed one: the `0xBF 0x19` lock byte has room
/// for exactly three, two bits apiece, and there is no fourth stat to point at.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stat {
    /// Strength — bits 4-5 of the lock byte.
    Strength,
    /// Dexterity — bits 2-3.
    Dexterity,
    /// Intelligence — bits 0-1.
    Intelligence,
}

/// Which stat a client's `0xBF 0x1A` named, exactly as sent.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RawStat(pub u8);

impl RawStat {
    /// The stat this names, or [`InvalidStat`] for a number the status bar has
    /// no arrow for.
    ///
    /// ServUO ignores the packet in that case, and so does this engine's seam —
    /// but the refusal is now a value a caller has to look at rather than a
    /// `_ => return` buried three calls deeper.
    pub const fn validate(self) -> Result<Stat, InvalidStat> {
        match self.0 {
            0 => Ok(Stat::Strength),
            1 => Ok(Stat::Dexterity),
            2 => Ok(Stat::Intelligence),
            other => Err(InvalidStat(other)),
        }
    }
}

/// A `0xBF 0x1A` naming a stat the status bar does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InvalidStat(pub u8);

impl fmt::Display for InvalidStat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "there is no stat {}; the status bar draws three arrows",
            self.0
        )
    }
}

impl std::error::Error for InvalidStat {
}

/// A stat arrow's new position, exactly as a client's `0xBF 0x1A` sent it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct RawStatLock(pub u8);

impl RawStatLock {
    /// Which way the arrow points. Total: ServUO folds anything past "locked"
    /// back to "up" rather than refusing the packet, and so does this — an arrow
    /// is a three-way choice with no room for an error.
    ///
    /// The fold lives here and not in [`StatLockRequest::decode_body`] because a
    /// decoder that rewrites a value has spent it: nothing downstream can tell
    /// the `0` a client sent from the `0x63` it did not.
    pub const fn interpret(self) -> SkillLock {
        SkillLock::from_bits(self.0)
    }
}

/// `0x09` — the client single-clicked something and wants its name.
///
/// Five bytes: the id and the clicked serial. The server answers by drawing the
/// object's name over its head, seen only by the asker — a `0x1C` label in the
/// notoriety colour. What the modern client shows as a tooltip on hover, the 2D
/// client asks for a click at a time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LookRequest {
    /// The clicked object, by serial — the client's word for what is under its
    /// cursor, and not yet anything the registry has heard of.
    pub serial: RawSerial,
}

impl DecodePacket for LookRequest {
    const ID: u8 = 0x09;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            serial: RawSerial(reader.u32()?),
        })
    }
}

/// `0x34` — a status or skill-list query. Fixed, 10 bytes.
///
/// ServUO's `MobileQuery`: a magic word, a type byte, then the queried serial.
/// Type `0x05` is the skill window opening; anything else is the status bar.
/// Answering every query with the status, as before, left the skill window
/// empty.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StatusQuery {
    /// Which window is asking.
    pub kind:   StatusQueryKind,
    /// Whom it is asking about, exactly as sent. Class D on this end: nothing
    /// here routes by it — every query this engine acts on is about the asking
    /// player's own mobile, which the connection already knows — but the client
    /// half has to *write* one, and a field only one direction reads is better
    /// than two functions that could disagree about the byte layout.
    pub serial: RawSerial,
}

/// What a [`StatusQuery`] is asking for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusQueryKind {
    /// The status bar.
    Status,
    /// The skill list.
    Skills,
}

impl StatusQueryKind {
    /// The type byte on the wire.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Status => 0x04,
            Self::Skills => 0x05,
        }
    }

    /// Which window a type byte means. Total, and everything that is not the
    /// skill list is the status bar — the reference sends `0x04` for it, and a
    /// shard that guessed at anything else would answer the wrong window.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            0x05 => Self::Skills,
            _ => Self::Status,
        }
    }
}

/// The magic word every `0x34` opens with, which nothing reads — ServUO writes
/// and ignores the same four bytes, and so does the reference client.
const STATUS_QUERY_MAGIC: u32 = 0xEDED_EDED;

impl StatusQuery {
    /// Encode a whole `0x34`. What `crates/client/net` sends when a paperdoll's
    /// Status or Skills button is pressed; this *server* never sends one, only
    /// ever decodes it — the same split as
    /// [`DoubleClick::encode`](crate::containers::DoubleClick::encode).
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        crate::packet::frame_body(
            <Self as DecodePacket>::ID,
            PacketLength::Fixed(10),
            |out: &mut PacketWriter| {
                out.u32(STATUS_QUERY_MAGIC);
                out.u8(self.kind.to_bits());
                out.u32(self.serial.0);
            },
        )
    }
}

impl DecodePacket for StatusQuery {
    const ID: u8 = 0x34;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let _magic = reader.u32()?;
        let kind = StatusQueryKind::from_bits(reader.u8()?);
        Ok(Self {
            kind,
            serial: RawSerial(reader.u32()?),
        })
    }
}

/// One piece of equipment on a mobile, as the client draws it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Equipment {
    /// The item's serial.
    pub serial:  Serial,
    /// Its graphic.
    pub graphic: Graphic,
    /// Which layer: hair, weapon, robe.
    pub layer:   Layer,
    /// Its colour. [`Hue::NONE`] means the graphic's own.
    pub hue:     Hue,
}

/// `0x1D` — take an object off the client's screen. 5 bytes.
///
/// Used for mobiles walking out of range and for items being picked up. The
/// client does not distinguish; it just forgets the serial.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Remove {
    /// The object to forget.
    pub serial: Serial,
}

impl EncodePacket for Remove {
    const ID: u8 = 0x1D;
    const LENGTH: PacketLength = PacketLength::Fixed(5);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.serial.raw());
    }
}

impl DecodePacket for Remove {
    const ID: u8 = 0x1D;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let serial = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x1D remove serial",
            value: raw,
        })?;
        Ok(Self { serial })
    }
}

/// What a `0x88` paperdoll's flag byte says about the mobile and the beholder.
///
/// Two bits are known and the rest are the client's business, so this is a byte
/// with a name and named constants rather than an enum — the same shape, and for
/// the same reason, as [`StatusFlags`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct PaperdollFlags(pub u8);

impl PaperdollFlags {
    /// Nothing set: at peace, and not liftable.
    pub const NONE: Self = Self(0);
    /// The mobile is in war mode.
    pub const WARMODE: Self = Self(0x01);
    /// The beholder may lift items off this paperdoll — set for your own, so the
    /// client lets you drag your equipment.
    pub const CAN_LIFT: Self = Self(0x02);

    /// Both sets of bits. A named method rather than `BitOr`: an operator on a
    /// newtype is the same invisible coercion `Deref` is.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit of `other` is set here.
    ///
    /// [`with`](Self::with)'s counterpart, and named for the same reason: a
    /// reader that spelled `flags.0 & PaperdollFlags::WARMODE.0 != 0` would be
    /// reaching through the newtype to do it, which is exactly what the newtype
    /// is for.
    #[must_use]
    pub const fn has(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// `0x88` — open a mobile's paperdoll. 66 bytes.
///
/// The reply to double-clicking a mobile. `text` is the title shown across the
/// top (the name, plus any honorific), clamped to 60 bytes. Ported from Sphere's
/// `PacketPaperdoll`/ServUO's `DisplayPaperdoll`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OpenPaperdoll {
    /// Whose paperdoll.
    pub serial: Serial,
    /// The title across the top: the name, plus any honorific. Clamped to 60
    /// bytes.
    pub text:   String,
    /// War mode, and whether the beholder may lift what is worn.
    pub flags:  PaperdollFlags,
}

impl EncodePacket for OpenPaperdoll {
    const ID: u8 = 0x88;
    const LENGTH: PacketLength = PacketLength::Fixed(66);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.serial.raw());
        out.fixed_string(&self.text, 60);
        out.u8(self.flags.0);
    }
}

impl DecodePacket for OpenPaperdoll {
    const ID: u8 = 0x88;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let serial = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x88 paperdoll serial",
            value: raw,
        })?;
        Ok(Self {
            serial,
            text: reader.fixed_string(60)?,
            flags: PaperdollFlags(reader.u8()?),
        })
    }
}

/// One of the three bars a client draws: what there is now, and the most there
/// could be.
///
/// The halves travel together for the same reason `world::MapSize`'s do: the
/// client draws a *ratio*, so a current sent without its maximum is not a
/// smaller number, it is a bar of the wrong length. Every source of one is a
/// source of the other — `Hitpoints`, `Mana` and `Stamina` each carry both — so
/// nothing is ever in a position to know only half.
///
/// The two components stay bare integers by decision, exactly as
/// `world::Point`'s do: they are genuinely numbers, added to and clamped on
/// every regeneration tick and every blow landed, and their rules live in the
/// gameplay crates far above `protocol`. See `docs/protocol_newtypes.md` N10.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Vitals {
    /// What there is now.
    pub current: u16,
    /// The most there can be.
    pub max:     u16,
}

/// `0x11` — a mobile's full status: the paperdoll numbers. Variable length.
///
/// The one packet that carries **stamina**, and the reason it exists here at all:
/// a client reads its own stamina from this, and a mobile the client believes has
/// zero stamina *cannot run* — it falls back to a walk with no error to show for
/// it. A shard that never sends `0x11` has players who can only ever walk. Weight
/// is the same trap the other way: a client that thinks it is over its
/// `max_weight` also refuses to run, so both are sent, and honest.
///
/// The packet grew a tail with each era and clients reject the wrong length
/// outright, so the shape is chosen by [`ClientVersion::status_packet_version`],
/// not a feature flag. Ported from ServUO's `MobileStatus` for the case a mobile
/// asks about *itself* (`WriteAttr`, raw current/max, not the normalised form a
/// stranger sees). Types 3 (pre-AoS), 4 (AoS), 5 (ML) and 6 (HS+) are built here;
/// an older client is served the type-3 body, the oldest shape a modern install
/// still round-trips.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MobileStatus {
    /// Whose status.
    pub serial:        Serial,
    /// The name shown on the bar, clamped to 30 bytes.
    pub name:          String,
    /// Hit points, current and maximum.
    pub hits:          Vitals,
    /// Whether the body is female — a bit the client draws the paperdoll from.
    pub female:        bool,
    /// Strength.
    pub strength:      u16,
    /// Dexterity. In UO this is also the stamina cap.
    pub dexterity:     u16,
    /// Intelligence.
    pub intelligence:  u16,
    /// Stamina. A zero current is what stops a client running.
    pub stamina:       Vitals,
    /// Mana.
    pub mana:          Vitals,
    /// Gold in the pack, shown on the status bar.
    pub gold:          u32,
    /// Physical resistance (pre-AoS: armour rating).
    pub armor:         u16,
    /// Carried weight. Kept under `max_weight` or the client refuses to run.
    pub weight:        u16,
    /// The weight the character can carry before it is overloaded.
    pub max_weight:    u16,
    /// The sum of the three stats a character may train to.
    pub stat_cap:      u16,
    /// Pets currently following.
    pub followers:     u8,
    /// The most pets that may follow.
    pub followers_max: u8,
}

impl EncodePacket for MobileStatus {
    const ID: u8 = 0x11;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        // The version function returns 1..=6; only 3..=6 have distinct wire
        // shapes here. Anything older is served the oldest one a modern file set
        // still understands. `type` is ServUO's, and it drives the tail length.
        let kind = version.status_packet_version().clamp(3, 6);

        out.u32(self.serial.raw());
        out.fixed_string(&self.name, 30);
        out.u16(self.hits.current);
        out.u16(self.hits.max);
        out.bool(false); // the beholder may not rename us
        out.u8(kind);

        out.bool(self.female);
        out.u16(self.strength);
        out.u16(self.dexterity);
        out.u16(self.intelligence);
        out.u16(self.stamina.current);
        out.u16(self.stamina.max);
        out.u16(self.mana.current);
        out.u16(self.mana.max);
        out.u32(self.gold);
        out.u16(self.armor);
        out.u16(self.weight);

        if kind >= 5 {
            out.u16(self.max_weight);
            out.u8(1); // race id + 1; human is 0, so 1
        }

        out.u16(self.stat_cap);
        out.u8(self.followers);
        out.u8(self.followers_max);

        if kind >= 4 {
            // Resistances, luck, weapon damage, tithing — all zero until the
            // systems that set them exist. The client only needs the shape.
            for _ in 0..5 {
                out.u16(0); // fire, cold, poison, energy, luck
            }
            out.u16(0); // damage min
            out.u16(0); // damage max
            out.u32(0); // tithing points
        }

        if kind >= 6 {
            // The AoS extended-status block: 15 shorts (0..=14). Zeroed.
            for _ in 0..=14 {
                out.u16(0);
            }
        }
    }
}

impl DecodePacket for MobileStatus {
    const ID: u8 = 0x11;

    /// `kind` is read from the wire, not re-derived from `version`: it is what
    /// says how much of the body follows, and a decoder that recomputed it from
    /// the connection's own version would read the wrong number of trailing
    /// bytes the moment the two disagree.
    ///
    /// Below type 5 the wire never carries `max_weight` at all — a client that
    /// old is simply never told it, so `0` here is not a guess at a real value,
    /// it is what "the server didn't say" looks like for this one field.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let serial = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x11 mobile status serial",
            value: raw,
        })?;
        let name = reader.fixed_string(30)?;
        let hits = Vitals {
            current: reader.u16()?,
            max:     reader.u16()?,
        };
        let _renamable = reader.bool()?;
        let kind = reader.u8()?;

        let female = reader.bool()?;
        let strength = reader.u16()?;
        let dexterity = reader.u16()?;
        let intelligence = reader.u16()?;
        let stamina = Vitals {
            current: reader.u16()?,
            max:     reader.u16()?,
        };
        let mana = Vitals {
            current: reader.u16()?,
            max:     reader.u16()?,
        };
        let gold = reader.u32()?;
        let armor = reader.u16()?;
        let weight = reader.u16()?;

        let max_weight = if kind >= 5 {
            let max_weight = reader.u16()?;
            reader.skip(1)?; // race id + 1, not modelled
            max_weight
        } else {
            0
        };

        let stat_cap = reader.u16()?;
        let followers = reader.u8()?;
        let followers_max = reader.u8()?;

        if kind >= 4 {
            // Resistances, luck, weapon damage, tithing: zeroed placeholders on
            // the way out, so there is nothing here worth keeping on the way in.
            reader.skip(5 * 2 + 2 + 2 + 4)?;
        }
        if kind >= 6 {
            // The AoS extended-status block: 15 zeroed shorts.
            reader.skip(15 * 2)?;
        }

        Ok(Self {
            serial,
            name,
            hits,
            female,
            strength,
            dexterity,
            intelligence,
            stamina,
            mana,
            gold,
            armor,
            weight,
            max_weight,
            stat_cap,
            followers,
            followers_max,
        })
    }
}

/// `0x77` — move a mobile the client already knows about. 17 bytes.
///
/// Sphere's comment is worth keeping: this cannot move the client's *own*
/// character. Sending it for the receiving player does nothing visible and the
/// two ends drift apart. Use `0x20` for that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileMove {
    /// The mobile's serial.
    pub serial:    Serial,
    /// Its body graphic.
    pub body:      Graphic,
    /// Where.
    pub position:  Point,
    /// Which way, and whether running.
    pub facing:    Facing,
    /// Its hue.
    pub hue:       Hue,
    /// Poisoned, invisible, war mode.
    pub flags:     StatusFlags,
    /// How to colour its health bar.
    pub notoriety: Notoriety,
}

impl EncodePacket for MobileMove {
    const ID: u8 = 0x77;
    const LENGTH: PacketLength = PacketLength::Fixed(17);

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        out.u32(self.serial.raw());
        out.u16(self.body.0);
        out.u16(self.position.x);
        out.u16(self.position.y);
        out.u8(self.position.z as u8);
        out.u8(self.facing.to_bits());
        out.u16(self.hue.0);
        out.u8(self.flags.0);
        out.u8(self.notoriety.for_client(version));
    }
}

impl DecodePacket for MobileMove {
    const ID: u8 = 0x77;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let serial = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x77 mobile move serial",
            value: raw,
        })?;
        let body = Graphic(reader.u16()?);
        let x = reader.u16()?;
        let y = reader.u16()?;
        let z = reader.u8()? as i8;
        let facing = Facing::from_bits(reader.u8()?);
        let hue = Hue(reader.u16()?);
        let flags = StatusFlags(reader.u8()?);
        let notoriety = Notoriety::from_bits(reader.u8()?);
        Ok(Self {
            serial,
            body,
            position: Point::new(x, y, z),
            facing,
            hue,
            flags,
            notoriety,
        })
    }
}

/// `0x78` — draw a mobile the client has not seen. Variable length.
///
/// # Two layouts, and the old one is the odd shape
///
/// The equipment list is where this gets interesting. Since 7.0.33.1
/// ([`Feature::NewMobileIncoming`]) every item is a fixed nine bytes with a hue
/// whether it needs one or not.
///
/// Before that, the record was *variable*, and what said which shape it was is a
/// bit inside the graphic id: `graphic | 0x8000` means "a hue follows", and
/// without it the record is seven bytes and stops at the layer. So an old client
/// parses the item list by reading a graphic, checking its top bit, and deciding
/// how much more to read. Send the new shape to an old client and it reads the
/// hue as the next item's serial and everything after is noise.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MobileIncoming {
    /// The mobile's serial.
    pub serial:    Serial,
    /// Its body graphic.
    pub body:      Graphic,
    /// Where.
    pub position:  Point,
    /// Which way, and whether running.
    pub facing:    Facing,
    /// Its hue.
    pub hue:       Hue,
    /// Poisoned, invisible, war mode.
    pub flags:     StatusFlags,
    /// How to colour its health bar.
    pub notoriety: Notoriety,
    /// What it is wearing.
    pub equipment: Vec<Equipment>,
}

impl EncodePacket for MobileIncoming {
    const ID: u8 = 0x78;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        let new_layout = version.supports(Feature::NewMobileIncoming);

        out.u32(self.serial.raw());
        out.u16(self.body.0);
        out.u16(self.position.x);
        out.u16(self.position.y);
        out.u8(self.position.z as u8);
        out.u8(self.facing.to_bits());
        out.u16(self.hue.0);
        out.u8(self.flags.0);
        out.u8(self.notoriety.for_client(version));

        for item in &self.equipment {
            out.u32(item.serial.raw());
            if new_layout {
                out.u16(item.graphic.0);
                out.u8(item.layer.0);
                out.u16(item.hue.0);
            } else if item.hue != Hue::NONE {
                // The top bit is not part of the graphic. It is the flag that
                // says the next two bytes are a hue.
                out.u16(item.graphic.0 | 0x8000);
                out.u8(item.layer.0);
                out.u16(item.hue.0);
            } else {
                out.u16(item.graphic.0);
                out.u8(item.layer.0);
            }
        }

        // A zero serial ends the list. Not a length — the client reads items
        // until it sees this.
        out.u32(0);
    }
}

impl DecodePacket for MobileIncoming {
    const ID: u8 = 0x78;

    fn decode_body(reader: &mut PacketReader<'_>, version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let serial = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x78 mobile incoming serial",
            value: raw,
        })?;
        let body = Graphic(reader.u16()?);
        let x = reader.u16()?;
        let y = reader.u16()?;
        let z = reader.u8()? as i8;
        let facing = Facing::from_bits(reader.u8()?);
        let hue = Hue(reader.u16()?);
        let flags = StatusFlags(reader.u8()?);
        let notoriety = Notoriety::from_bits(reader.u8()?);

        let new_layout = version.supports(Feature::NewMobileIncoming);
        let mut equipment = Vec::new();
        loop {
            let item_raw = reader.u32()?;
            if item_raw == 0 {
                break;
            }
            let item_serial = Serial::new(item_raw).ok_or(DecodeError::UnknownValue {
                field: "0x78 mobile incoming equipment serial",
                value: item_raw,
            })?;
            let (graphic, layer, item_hue) = if new_layout {
                (Graphic(reader.u16()?), Layer(reader.u8()?), Hue(reader.u16()?))
            } else {
                let raw_graphic = reader.u16()?;
                let hued = raw_graphic & 0x8000 != 0;
                let layer = Layer(reader.u8()?);
                let item_hue = if hued { Hue(reader.u16()?) } else { Hue::NONE };
                (Graphic(raw_graphic & 0x7FFF), layer, item_hue)
            };
            equipment.push(Equipment {
                serial: item_serial,
                graphic,
                layer,
                hue: item_hue,
            });
        }

        Ok(Self {
            serial,
            body,
            position: Point::new(x, y, z),
            facing,
            hue,
            flags,
            notoriety,
            equipment,
        })
    }
}

/// The three stat arrows on the status bar, as the wire carries them.
///
/// Two bits each inside one byte of the `0xBF 0x19` extended-status packet, and
/// the mirror of [`SkillLock`] for strength, dexterity and intelligence — the
/// same three-way arrow, so it is the same type and not a second copy of it.
/// Ported from ServUO's `StatLockInfo` (out) and its `0x1A` handler (in).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct StatLockBits {
    /// Strength's arrow.
    pub strength:     SkillLock,
    /// Dexterity's arrow.
    pub dexterity:    SkillLock,
    /// Intelligence's arrow.
    pub intelligence: SkillLock,
}

/// `0xBF` subcommand `0x19` type `2` — tell a client which way its stats train,
/// so the paperdoll draws the three arrows.
///
/// ServUO's `StatLockInfo`: the subcommand, the type byte, the mobile's serial, a
/// zero, and one byte holding all three arrows two bits apiece —
/// `str << 4 | dex << 2 | int`.
///
/// # Fixed despite living under `0xBF`, and the length field is still hand-written
///
/// Like `world::MapChange`, this subcommand's body never varies — there is no
/// list and no version branch — so it declares `Fixed(12)`. [`crate::packet::frame_body`]
/// only back-patches a length for [`PacketLength::Variable`], so the constant
/// `u16(12)` is still written here by hand, exactly where the `0xBF` envelope
/// always puts one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StatLocks {
    /// Whose stats.
    pub serial: Serial,
    /// The three arrows.
    pub locks:  StatLockBits,
}

impl EncodePacket for StatLocks {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Fixed(12);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(12); // this subcommand's own, constant length
        out.u16(0x19);
        out.u8(2); // the classic client's type; the enhanced one wants 5
        out.u32(self.serial.raw());
        out.u8(0);
        out.u8((self.locks.strength.to_bits() << 4)
            | (self.locks.dexterity.to_bits() << 2)
            | self.locks.intelligence.to_bits());
    }
}

/// `0xBF` subcommand `0x1A` — the player clicked one of the status bar's stat
/// arrows.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StatLockRequest {
    /// Which stat the client named, before anyone checked there is one.
    pub stat: RawStat,
    /// Where it says the arrow now points.
    pub lock: RawStatLock,
}

impl StatLockRequest {
    /// The subcommand that means "a stat's arrow moved". See
    /// [`ExtendedRequest`](crate::extended::ExtendedRequest), the single place
    /// that reads a `0xBF` envelope and picks a subcommand's body decoder by it.
    pub const SUBCOMMAND: u16 = 0x1A;

    /// Read the body, `reader` already past the id, length and subcommand.
    ///
    /// Byte shape only. Neither field is checked here: which stat exists is
    /// [`RawStat::validate`]'s answer and where an arrow may point is
    /// [`RawStatLock::interpret`]'s, both at the seam that acts on the packet.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            stat: RawStat(reader.u8()?),
            lock: RawStatLock(reader.u8()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direction::Direction;
    use crate::extended::ExtendedRequest;
    use crate::packet::encode_packet;

    fn version() -> ClientVersion {
        ClientVersion::TOL
    }

    #[test]
    fn the_stat_locks_pack_three_arrows_into_one_byte() {
        // ServUO's `StatLockInfo`: str in bits 4-5, dex in 2-3, int in 0-1.
        let packet = encode_packet(
            &StatLocks {
                serial: Serial::new(0x0000_0007).unwrap(),
                locks:  StatLockBits {
                    strength:     SkillLock::Locked,
                    dexterity:    SkillLock::Down,
                    intelligence: SkillLock::Up,
                },
            },
            version(),
        );
        assert_eq!(packet[0], 0xBF);
        assert_eq!(u16::from_be_bytes([packet[1], packet[2]]) as usize, packet.len());
        assert_eq!(u16::from_be_bytes([packet[3], packet[4]]), 0x19);
        assert_eq!(packet[5], 2, "the classic client's extended-status type");
        assert_eq!(
            u32::from_be_bytes([packet[6], packet[7], packet[8], packet[9]]),
            7
        );
        assert_eq!(packet[10], 0);
        assert_eq!(packet[11], 0b10_01_00);
        assert_eq!(packet.len(), 12);
    }

    #[test]
    fn a_stat_lock_request_reads_its_stat_and_arrow() {
        // 0xBF, length, subcommand 0x1A, stat 1 (dexterity), lock 1 (down).
        let packet = [0xBF, 0x00, 0x07, 0x00, 0x1A, 0x01, 0x01];
        let request = ExtendedRequest::decode(&packet).unwrap();
        assert_eq!(
            request,
            ExtendedRequest::StatLock(StatLockRequest {
                stat: RawStat(1),
                lock: RawStatLock(1),
            })
        );
        let ExtendedRequest::StatLock(request) = request else {
            panic!("expected a stat-lock request");
        };
        assert_eq!(request.stat.validate(), Ok(Stat::Dexterity));
        assert_eq!(request.lock.interpret(), SkillLock::Down);
    }

    #[test]
    fn an_impossible_stat_arrow_reads_as_up() {
        // ServUO folds anything past "locked" back to "up" rather than refusing.
        // The fold is the promotion's, not the decoder's: the byte the client
        // sent survives to the seam, which is what lets a log line say what
        // actually arrived.
        let packet = [0xBF, 0x00, 0x07, 0x00, 0x1A, 0x00, 0x63];
        let ExtendedRequest::StatLock(request) = ExtendedRequest::decode(&packet).unwrap() else {
            panic!("expected a stat-lock request");
        };
        assert_eq!(request.lock, RawStatLock(0x63), "decoding rewrites nothing");
        assert_eq!(request.lock.interpret(), SkillLock::Up);
    }

    #[test]
    fn every_arrow_byte_interprets() {
        // Class B's promise: `interpret` is total. Nothing a client can put in
        // this byte needs a `Result`, so nothing downstream grows one.
        for bits in 0..=u8::MAX {
            let lock = RawStatLock(bits).interpret();
            assert_eq!(lock, SkillLock::from_bits(bits));
            if bits > 2 {
                assert_eq!(lock, SkillLock::Up, "0x{bits:02X} folds to up");
            }
        }
    }

    #[test]
    fn a_fourth_stat_decodes_cleanly_and_is_refused_at_promotion() {
        // The pair `docs/protocol_newtypes.md` N9 asks for. The packet is
        // well-formed — dropping the connection over it would be wrong — and the
        // value is refused where the arrow would have been stored.
        let packet = [0xBF, 0x00, 0x07, 0x00, 0x1A, 0x03, 0x01];
        let ExtendedRequest::StatLock(request) = ExtendedRequest::decode(&packet).unwrap() else {
            panic!("expected a stat-lock request");
        };
        assert_eq!(request.stat, RawStat(3));
        assert_eq!(request.stat.validate(), Err(InvalidStat(3)));

        assert_eq!(RawStat(0).validate(), Ok(Stat::Strength));
        assert_eq!(RawStat(1).validate(), Ok(Stat::Dexterity));
        assert_eq!(RawStat(2).validate(), Ok(Stat::Intelligence));
    }

    #[test]
    fn a_single_click_on_nothing_decodes_cleanly_and_is_refused_at_promotion() {
        // The same pair for the one serial a client chooses in this module. A
        // `0x09` naming zero is a click that hit nothing, not a broken packet.
        let bytes = [0x09, 0x00, 0x00, 0x00, 0x00];
        let look: LookRequest = crate::packet::decode_packet(&bytes, version()).unwrap();
        assert_eq!(look.serial, RawSerial(0));
        assert_eq!(look.serial.validate(), None);
    }

    #[test]
    fn a_status_query_reads_the_type_byte() {
        let skills = [0x34, 0x00, 0x00, 0x12, 0x34, 0x05, 0x40, 0x00, 0x00, 0x2A];
        let query: StatusQuery = crate::packet::decode_packet(&skills, version()).unwrap();
        assert_eq!(query.kind, StatusQueryKind::Skills);

        let status = [0x34, 0x00, 0x00, 0x12, 0x34, 0x04, 0x40, 0x00, 0x00, 0x2A];
        let query: StatusQuery = crate::packet::decode_packet(&status, version()).unwrap();
        assert_eq!(query.kind, StatusQueryKind::Status);
        assert_eq!(query.serial, RawSerial(0x4000_002A), "and whom it asks about");
    }

    /// The client's half: what a paperdoll's Status and Skills buttons write is
    /// what this crate's own decoder reads back, type byte and serial both. The
    /// magic word is not asserted on — nothing reads it, at either end.
    #[test]
    fn a_status_query_this_client_writes_is_one_this_server_reads() {
        for kind in [StatusQueryKind::Status, StatusQueryKind::Skills] {
            let bytes = StatusQuery {
                kind,
                serial: RawSerial(0x4000_002A),
            }
            .encode();
            let heard: StatusQuery = crate::packet::decode_packet(&bytes, version()).unwrap();
            assert_eq!(heard.kind, kind);
            assert_eq!(heard.serial, RawSerial(0x4000_002A));
        }
    }

    #[test]
    fn a_single_click_decodes_the_clicked_serial() {
        let bytes = [0x09, 0x40, 0x00, 0x00, 0x2A];
        let look: LookRequest = crate::packet::decode_packet(&bytes, version()).unwrap();
        assert_eq!(look.serial, RawSerial(0x4000_002A));
        assert_eq!(look.serial.validate(), Serial::new(0x4000_002A));
    }

    #[test]
    fn a_name_hue_matches_the_health_bar_colour() {
        // Blue innocent, yellow invulnerable — the same colours the bar uses, from
        // ServUO's Notoriety.Hues.
        assert_eq!(Notoriety::Innocent.name_hue(), Hue(0x0059));
        assert_eq!(Notoriety::Invulnerable.name_hue(), Hue(0x0035));
        assert_eq!(Notoriety::Murderer.name_hue(), Hue(0x0022));
    }

    fn facing() -> Facing {
        Facing::running(Direction::SouthEast)
    }

    fn mobile() -> MobileIncoming {
        MobileIncoming {
            serial:    Serial::new(0x0000_0002).unwrap(),
            body:      Graphic(0x0190),
            position:  Point::new(1475, 1774, -5),
            facing:    facing(),
            hue:       Hue(0x83EA),
            flags:     StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
            equipment: Vec::new(),
        }
    }

    fn shirt() -> Equipment {
        Equipment {
            serial:  Serial::new(0x4000_0001).unwrap(),
            graphic: Graphic(0x1517),
            layer:   Layer(0x05),
            hue:     Hue(0x0021),
        }
    }

    #[test]
    fn remove_is_five_bytes() {
        // The serial was `0xDEAD_BEEF` here until `Remove` took a `Serial`, and
        // `Serial::new` refuses it — it is past the item pool, so no client
        // could have anything under it. The shape it proves is unchanged: five
        // bytes, the serial big-endian.
        assert_eq!(
            encode_packet(
                &Remove {
                    serial: Serial::new(0x4EAD_BEEF).unwrap(),
                },
                version()
            ),
            vec![0x1D, 0x4E, 0xAD, 0xBE, 0xEF]
        );
    }

    #[test]
    fn a_move_matches_its_declared_length() {
        let bytes = encode_packet(
            &MobileMove {
                serial:    Serial::new(2).unwrap(),
                body:      Graphic(0x0190),
                position:  Point::new(1475, 1774, -5),
                facing:    facing(),
                hue:       Hue(0x83EA),
                flags:     StatusFlags::NONE,
                notoriety: Notoriety::Innocent,
            },
            ClientVersion::TOL,
        );

        assert_eq!(bytes.len(), 17, "Sphere's PacketCharacterMove length");
        assert_eq!(bytes[0], 0x77);
        assert_eq!(&bytes[1..5], &2u32.to_be_bytes());
        assert_eq!(&bytes[5..7], &0x0190u16.to_be_bytes());
        assert_eq!(bytes[11] as i8, -5, "z is one signed byte");
        assert_eq!(bytes[12], facing().to_bits());
        assert_eq!(bytes[16], 0x01, "innocent");
    }

    #[test]
    fn a_naked_mobile_is_the_base_length() {
        let bytes = encode_packet(&mobile(), ClientVersion::TOL);
        assert_eq!(bytes.len(), 23, "Sphere's PacketCharacter base length");
        assert_eq!(bytes[0], 0x78);
        assert_eq!(
            u16::from_be_bytes([bytes[1], bytes[2]]) as usize,
            bytes.len(),
            "the declared length must match reality"
        );
        assert_eq!(&bytes[19..23], &[0, 0, 0, 0], "the zero serial ends the list");
    }

    #[test]
    fn a_modern_client_gets_nine_fixed_bytes_per_item() {
        // Since 7.0.33.1 the hue is always there, needed or not.
        let mut incoming = mobile();
        incoming.equipment = vec![shirt()];
        let bytes = encode_packet(&incoming, ClientVersion::new(7, 0, 33, 1));

        assert_eq!(bytes.len(), 23 + 9);
        assert_eq!(&bytes[19..23], &0x4000_0001u32.to_be_bytes(), "item serial");
        assert_eq!(
            &bytes[23..25],
            &0x1517u16.to_be_bytes(),
            "the graphic carries no flag bit"
        );
        assert_eq!(bytes[25], 0x05, "layer");
        assert_eq!(&bytes[26..28], &0x0021u16.to_be_bytes(), "hue, always");
    }

    #[test]
    fn an_old_client_gets_the_hue_flagged_into_the_graphic() {
        // The bit that says "a hue follows". An old client reads the graphic,
        // checks its top bit, and decides how much more to read — so the record
        // is nine bytes here and seven below.
        let mut incoming = mobile();
        incoming.equipment = vec![shirt()];
        let bytes = encode_packet(&incoming, ClientVersion::new(7, 0, 33, 0));

        assert_eq!(bytes.len(), 23 + 9);
        assert_eq!(
            u16::from_be_bytes([bytes[23], bytes[24]]),
            0x1517 | 0x8000,
            "the top bit flags the hue"
        );
        assert_eq!(&bytes[26..28], &0x0021u16.to_be_bytes());
    }

    #[test]
    fn an_old_client_gets_seven_bytes_for_an_unhued_item() {
        // The variable-length case. Sending nine bytes here would have the
        // client read the hue as the next item's serial, and everything after it
        // is noise.
        let mut incoming = mobile();
        incoming.equipment = vec![Equipment {
            hue: Hue::NONE,
            ..shirt()
        }];
        let bytes = encode_packet(&incoming, ClientVersion::new(7, 0, 33, 0));

        assert_eq!(bytes.len(), 23 + 7, "no hue, no two bytes for one");
        assert_eq!(
            u16::from_be_bytes([bytes[23], bytes[24]]),
            0x1517,
            "and no flag bit either"
        );
        assert_eq!(bytes[25], 0x05, "the record stops at the layer");
    }

    #[test]
    fn the_two_layouts_differ_for_the_same_mobile() {
        // The whole reason this is version-gated. If these ever agree, the gate
        // has stopped doing anything.
        let mut incoming = mobile();
        incoming.equipment = vec![Equipment {
            hue: Hue::NONE,
            ..shirt()
        }];

        let modern = encode_packet(&incoming, ClientVersion::new(7, 0, 33, 1));
        let ancient = encode_packet(&incoming, ClientVersion::new(7, 0, 33, 0));
        assert_ne!(modern, ancient);
        assert_eq!(modern.len(), ancient.len() + 2, "the hue an old client skips");
    }

    #[test]
    fn a_mobile_wearing_a_lot_still_declares_its_length() {
        let mut incoming = mobile();
        incoming.equipment = (0..25)
            .map(|index| {
                Equipment {
                    serial: Serial::new(0x4000_0000 + index).unwrap(),
                    layer: Layer(index as u8),
                    ..shirt()
                }
            })
            .collect();

        for version in [ClientVersion::new(7, 0, 33, 0), ClientVersion::TOL] {
            let bytes = encode_packet(&incoming, version);
            assert_eq!(
                u16::from_be_bytes([bytes[1], bytes[2]]) as usize,
                bytes.len(),
                "{version}"
            );
        }
    }

    #[test]
    fn notoriety_bytes_are_the_clients_own() {
        assert_eq!(Notoriety::Innocent.to_bits(), 0x01);
        assert_eq!(Notoriety::Friend.to_bits(), 0x02);
        assert_eq!(Notoriety::Neutral.to_bits(), 0x03);
        assert_eq!(Notoriety::Criminal.to_bits(), 0x04);
        assert_eq!(Notoriety::Enemy.to_bits(), 0x05);
        assert_eq!(Notoriety::Murderer.to_bits(), 0x06);
        assert_eq!(Notoriety::Invulnerable.to_bits(), 0x07);
    }

    #[test]
    fn notoriety_reads_back_from_its_byte_and_defaults_to_blue() {
        for noto in [
            Notoriety::Innocent,
            Notoriety::Friend,
            Notoriety::Neutral,
            Notoriety::Criminal,
            Notoriety::Enemy,
            Notoriety::Murderer,
            Notoriety::Invulnerable,
        ] {
            assert_eq!(Notoriety::from_bits(noto.to_bits()), noto);
        }
        // Unset or nonsense reads as the safe blue.
        assert_eq!(Notoriety::from_bits(0), Notoriety::Innocent);
        assert_eq!(Notoriety::from_bits(0xFF), Notoriety::Innocent);
    }

    #[test]
    fn an_old_client_gets_blue_rather_than_no_health_bar() {
        // A client before 4.0.0 renders 0x07 as nothing — the mobile gets no
        // health bar at all, which looks like a bug rather than a GM.
        let ancient = ClientVersion::new(3, 255, 255, 255);
        assert!(!ancient.supports(Feature::NotorietyInvulnerable));
        assert_eq!(
            Notoriety::Invulnerable.for_client(ancient),
            Notoriety::Innocent.to_bits()
        );

        let modern = ClientVersion::AOS;
        assert_eq!(Notoriety::Invulnerable.for_client(modern), 0x07);
    }

    fn a_status() -> MobileStatus {
        MobileStatus {
            serial:        Serial::new(0x0001_2345).unwrap(),
            name:          "Lord British".to_owned(),
            hits:          Vitals {
                current: 100,
                max:     100,
            },
            female:        false,
            strength:      100,
            dexterity:     90,
            intelligence:  80,
            stamina:       Vitals {
                current: 90,
                max:     90,
            },
            mana:          Vitals {
                current: 80,
                max:     80,
            },
            gold:          1234,
            armor:         0,
            weight:        14,
            max_weight:    390,
            stat_cap:      225,
            followers:     0,
            followers_max: 5,
        }
    }

    #[test]
    fn a_paperdoll_is_sixty_six_bytes_with_its_title() {
        let bytes = encode_packet(
            &OpenPaperdoll {
                serial: Serial::new(0x0001_2345).unwrap(),
                text:   "Lord British".to_owned(),
                flags:  PaperdollFlags::CAN_LIFT,
            },
            version(),
        );
        assert_eq!(bytes.len(), 66, "id + serial + 60-byte title + flags");
        assert_eq!(bytes[0], 0x88);
        assert_eq!(
            u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]),
            0x0001_2345
        );
        assert_eq!(bytes[65], PaperdollFlags::CAN_LIFT.0);
        // The title is where the client draws the name across the top.
        assert!(bytes[5..].starts_with(b"Lord British"));
    }

    /// The client's half of `0x88`. Worth a test of its own rather than trusting
    /// the encoder above: the title is a fixed 60-byte field, so a decoder that
    /// reads it as null-terminated and a decoder that reads it as a block land on
    /// different flag bytes, and the flags are the last byte of the packet.
    #[test]
    fn a_paperdoll_reads_back_the_title_and_the_flags() {
        let sent = OpenPaperdoll {
            serial: Serial::new(0x0001_2345).unwrap(),
            text:   "Lord British".to_owned(),
            flags:  PaperdollFlags::WARMODE.with(PaperdollFlags::CAN_LIFT),
        };
        let bytes = encode_packet(&sent, version());
        let read: OpenPaperdoll = crate::packet::decode_packet(&bytes, version()).unwrap();
        assert_eq!(read, sent, "the padding past the title is not part of it");
    }

    #[test]
    fn the_paperdoll_flags_are_one_byte_of_bits() {
        // Your own paperdoll while fighting: both bits, one byte, and the values
        // are Sphere's `PacketPaperdoll`.
        let both = PaperdollFlags::WARMODE.with(PaperdollFlags::CAN_LIFT);
        assert_eq!(both.0, 0x03);
        assert_eq!(PaperdollFlags::NONE.with(PaperdollFlags::WARMODE).0, 0x01);
        assert_eq!(both.with(PaperdollFlags::WARMODE), both, "setting twice is once");
    }

    /// The war bit of a `0x77`/`0x78` is `0x40`, and it is *not* the paperdoll's
    /// `0x01`. One fact, two packets, two different bits — the mistake this test
    /// exists to catch is a reader that assumes the byte is the same byte.
    #[test]
    fn the_status_flags_war_bit_is_not_the_paperdolls() {
        assert_eq!(StatusFlags::WARMODE.0, 0x40);
        assert_eq!(PaperdollFlags::WARMODE.0, 0x01);
        assert_eq!(StatusFlags::of_stance(true), StatusFlags::WARMODE);
        assert_eq!(StatusFlags::of_stance(false), StatusFlags::NONE);
        assert!(StatusFlags::of_stance(true).has(StatusFlags::WARMODE));
        assert!(!StatusFlags::of_stance(false).has(StatusFlags::WARMODE));
    }

    /// The bit that says "walk through other mobiles" is `0x10`, and it is
    /// independent of the stance.
    ///
    /// A game master at war is both, and the reader that matters is a client
    /// deciding whether to predict a step into somebody: an engine that packed
    /// the two into one byte-wide answer would give a fighting GM the wrong one.
    #[test]
    fn ignoring_mobiles_is_its_own_bit_and_rides_beside_the_stance() {
        assert_eq!(StatusFlags::IGNORE_MOBILES.0, 0x10);
        let fighting_gm = StatusFlags::of_stance(true).with(StatusFlags::IGNORE_MOBILES);
        assert_eq!(fighting_gm.0, 0x50);
        assert!(fighting_gm.has(StatusFlags::WARMODE));
        assert!(fighting_gm.has(StatusFlags::IGNORE_MOBILES));
        assert!(!StatusFlags::of_stance(true).has(StatusFlags::IGNORE_MOBILES));
        assert!(
            !StatusFlags::IGNORE_MOBILES.has(StatusFlags::WARMODE),
            "and a peaceable game master is not drawn with a sword out"
        );
    }

    /// A mobile's flag byte survives the packet that carries it — both of them,
    /// because a stance the `0x78` states and the `0x77` drops would be a body
    /// that leaves its war stance every time it takes a step.
    #[test]
    fn a_stance_rides_both_the_move_and_the_incoming_packet() {
        let flags = StatusFlags::of_stance(true);
        let step = MobileMove {
            serial: Serial::new(1).unwrap(),
            body: Graphic(0x0190),
            position: Point { x: 5, y: 6, z: 7 },
            facing: Facing::walking(Direction::South),
            hue: Hue::NONE,
            flags,
            notoriety: Notoriety::Innocent,
        };
        let back: MobileMove =
            crate::packet::decode_packet(&encode_packet(&step, version()), version()).unwrap();
        assert_eq!(back.flags, flags);

        let incoming = MobileIncoming {
            serial: Serial::new(1).unwrap(),
            body: Graphic(0x0190),
            position: Point { x: 5, y: 6, z: 7 },
            facing: Facing::walking(Direction::South),
            hue: Hue::NONE,
            flags,
            notoriety: Notoriety::Innocent,
            equipment: Vec::new(),
        };
        // Through the *client's* door rather than `decode_packet`: `0x78` is
        // variable in this direction and unknown in the client table, so the
        // generic helper would not skip its length word. Same reason
        // `server_packet::decode_server` exists.
        let bytes = encode_packet(&incoming, version());
        let Ok(Some(crate::server_packet::ServerPacket::MobileIncoming(back))) =
            crate::server_packet::ServerPacket::decode(&bytes, version())
        else {
            panic!("a 0x78 this engine wrote is one a client reads");
        };
        assert_eq!(back.flags, flags);
    }

    #[test]
    fn a_status_packet_declares_its_own_length() {
        // The client rejects a 0x11 whose length word does not match its bytes,
        // and reads past the end of a short one — so the two must always agree.
        for version in [
            ClientVersion::new(3, 0, 8, 10), // type 3
            ClientVersion::new(4, 0, 0, 0),  // type 4
            ClientVersion::new(5, 0, 0, 0),  // type 5
            ClientVersion::TOL,              // type 6
        ] {
            let bytes = encode_packet(&a_status(), version);
            assert_eq!(bytes[0], 0x11);
            let declared = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
            assert_eq!(declared, bytes.len(), "length mismatch for {version}");
        }
    }

    #[test]
    fn the_modern_status_is_the_hundred_and_twenty_one_byte_shape() {
        // ServUO's `EnsureCapacity(121)` for a type-6 self status. Off by a byte
        // and a High Seas client desyncs on the next packet.
        assert_eq!(encode_packet(&a_status(), ClientVersion::TOL).len(), 121);
    }

    #[test]
    fn stamina_rides_in_the_status_and_is_not_zero() {
        // The whole reason this packet is sent: a zero here is a mobile the client
        // will not let run. The bytes are at a fixed offset for a self status.
        let bytes = encode_packet(&a_status(), ClientVersion::TOL);
        // id(1) len(2) serial(4) name(30) hits(2) hitsmax(2) rename(1) type(1)
        // female(1) str(2) dex(2) int(2) => stamina starts at byte 50.
        let stamina = u16::from_be_bytes([bytes[50], bytes[51]]);
        assert_eq!(stamina, 90, "the client reads run-eligibility from here");
    }

    #[test]
    fn every_other_notoriety_survives_an_old_client() {
        let ancient = ClientVersion::new(3, 0, 0, 0);
        for notoriety in [
            Notoriety::Innocent,
            Notoriety::Friend,
            Notoriety::Neutral,
            Notoriety::Criminal,
            Notoriety::Enemy,
            Notoriety::Murderer,
        ] {
            assert_eq!(
                notoriety.for_client(ancient),
                notoriety.to_bits(),
                "{notoriety:?} was downgraded and should not have been"
            );
        }
    }
}
