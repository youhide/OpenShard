//! Audiovisual feedback: sound, animation and graphical effects.
//!
//! The packets that make a world *felt* rather than merely correct — a swing
//! whooshes, a fireball flies, a body crumples. UO drives all of it from the
//! server: there is no client-side "you swung, so animate" rule, so a state
//! change with no feedback packet is silent and still to the client, which reads
//! as broken even when the numbers are right. Every one of these is broadcast to
//! the watchers who can see the actor, through the same interest machinery as a
//! `0x78`.
//!
//! Layouts are ported from ServUO's `Server/Network/Packets.cs`
//! (`PlaySound`, `MobileAnimation`, `NewMobileAnimation`, `GraphicalEffect`,
//! `HuedEffect`) and agree with Sphere's `sphereproto.h`. The wire is
//! big-endian, like the rest of the protocol.
//!
//! # Animation numbers stay bare
//!
//! `action`, `animation_type` and the frame counts are deliberately plain `u16`
//! rather than newtypes. They are indices into the client's animation tables,
//! and — this is the trap — `0x6E`'s numbering is body-specific while `0xE2`'s is
//! a body-agnostic category. One shared `AnimationId` would let a caller hand a
//! classic action number to the new packet and be believed, which is exactly the
//! confusion a newtype is supposed to prevent. Each packet documents its own
//! numbering instead.
//!
//! On N10's allowlist for the same reason [`SkillEntry::id`](crate::skill::SkillEntry::id)
//! is: the domain type both animation packets draw their numbers from,
//! `openshard_state::Action`, lives in a server crate above `protocol` and
//! cannot be held here. `GraphicalEffect::speed`/`duration` are quantities —
//! every caller passes a per-effect literal, nothing branches on either — the
//! `mobile::Vitals` argument again. `HuedEffect::render_mode` is untouched for
//! a different reason: no non-test code in this workspace constructs a
//! `HuedEffect` today, so there is nothing to classify against; wrapping it
//! would be a guess with no caller to check it against.

use crate::access::OPENSHARD_SUBCOMMANDS;
use crate::codec::PacketWriter;
use crate::packet::{DecodePacket, EncodePacket, PacketLength};
use crate::serial::{Serial, raw_or_none};
use crate::version::ClientVersion;
use crate::wire::{CursorId, Graphic, Hue, Layer, SoundId};
use crate::world::Point;

/// The ordinary client cadence for a mobile-animation frame.
///
/// A zero `delay` in either animation packet means this value rather than an
/// instantaneous action. Shared by the server's wind-up scheduler and the
/// client's action clock so the impact window is measured from one fact.
pub const DEFAULT_ANIMATION_FRAME_MS: u64 = 80;

/// A swing interval expressed in milliseconds on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SwingDuration(pub u32);

impl SwingDuration {
    /// Duration in milliseconds.
    #[must_use]
    pub const fn millis(self) -> u32 {
        self.0
    }
}

/// `0xBF` subcommand `0xE00B` — a swing animation begins now and stays active
/// for at least the supplied duration before its authoritative impact.
///
/// The stock animation packets can only carry an eight-bit per-frame delay,
/// which cannot represent a several-second heavy swing. This OpenShard
/// extension carries the whole duration as `u32`; it is sent immediately before
/// the ordinary `0x6E`/`0xE2`, so a client that understands it loops complete
/// action cycles through that interval and a stock client simply ignores the
/// unknown extended command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SwingTiming {
    /// The mobile whose next animation this timing belongs to.
    pub serial: Serial,
    /// Minimum time from the first frame to the impact.
    pub duration: SwingDuration,
}

impl SwingTiming {
    /// The extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// The first OpenShard subcommand after the map-edit request/reply pair.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 11;
    /// Id, length, subcommand, serial and duration.
    pub const LENGTH_BYTES: u16 = 13;
}

impl EncodePacket for SwingTiming {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.serial.raw());
        out.u32(self.duration.0);
    }
}

impl DecodePacket for SwingTiming {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "0xBF subcommand for swing timing",
                value: u32::from(subcommand),
            });
        }
        let serial = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "swing timing serial",
            value: 0,
        })?;
        Ok(Self {
            serial,
            duration: SwingDuration(reader.u32()?),
        })
    }
}

/// `0xBF` subcommand `0xE00C` — draw a harvesting tool in a mobile's hand for
/// its immediately following action.
///
/// A hatchet may be used straight from a backpack, as it is on ServUO.  The
/// ordinary animation packet can name the chopping motion but not the tool
/// that caused it, so an OpenShard client would otherwise make the lumberjack
/// swing empty hands.  This is visual-only: it neither moves the item nor
/// makes it equipped.  Stock clients skip this unknown extended command and
/// retain their ordinary harvest animation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HarvestToolVisual {
    /// The mobile about to swing.
    pub serial: Serial,
    /// The tool's item graphic.
    pub graphic: Graphic,
    /// The tool's hue.
    pub hue: Hue,
    /// The hand layer it is drawn on for this action.
    pub layer: Layer,
}

impl HarvestToolVisual {
    /// The extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// Follows [`SwingTiming::SUBCOMMAND`].
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 12;
    /// Id, length, subcommand, mobile, graphic, hue and layer.
    pub const LENGTH_BYTES: u16 = 14;
}

impl EncodePacket for HarvestToolVisual {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.serial.raw());
        out.u16(self.graphic.0);
        out.u16(self.hue.0);
        out.u8(self.layer.0);
    }
}

impl DecodePacket for HarvestToolVisual {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "0xBF subcommand for harvest tool visual",
                value: u32::from(subcommand),
            });
        }
        let serial = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "harvest tool visual serial",
            value: 0,
        })?;
        Ok(Self {
            serial,
            graphic: Graphic(reader.u16()?),
            hue: Hue(reader.u16()?),
            layer: Layer(reader.u8()?),
        })
    }
}

/// `0xBF` subcommand `0xE00D` — enough animation data to start a harvest the
/// instant its targeting click leaves the client.
///
/// This is a presentation hint, not permission to gather: the server still
/// validates the tile and is the only side that delivers a resource.  Its cursor
/// id binds the hint to exactly one open target, preventing an old hatchet click
/// from starting a later spell or house-placement cursor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HarvestPreview {
    /// The target cursor this preview belongs to.
    pub cursor_id: CursorId,
    /// The mobile that will perform the action.
    pub serial: Serial,
    /// The classic animation group to start locally.
    pub action: u16,
    /// The group length the server will later authoritatively send.
    pub frame_count: AnimationFrameCount,
    /// Server-owned work duration, before transport latency.
    pub duration: SwingDuration,
    /// Number of complete swings that make up the work interval.
    pub cycles: u16,
}

impl HarvestPreview {
    pub const ID: u8 = 0xBF;
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 13;
    /// Id, length, subcommand, cursor, mobile, group, frames, duration and cycles.
    pub const LENGTH_BYTES: u16 = 23;
}

impl EncodePacket for HarvestPreview {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.cursor_id.0);
        out.u32(self.serial.raw());
        out.u16(self.action);
        out.u16(self.frame_count.0);
        out.u32(self.duration.0);
        out.u16(self.cycles);
    }
}

impl DecodePacket for HarvestPreview {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "0xBF subcommand for harvest preview",
                value: u32::from(subcommand),
            });
        }
        let cursor_id = CursorId(reader.u32()?);
        let serial = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "harvest preview serial",
            value: 0,
        })?;
        Ok(Self {
            cursor_id,
            serial,
            action: reader.u16()?,
            frame_count: AnimationFrameCount(reader.u16()?),
            duration: SwingDuration(reader.u32()?),
            cycles: reader.u16()?,
        })
    }
}

/// `0xBF` subcommand `0xE00E` — the server refused the optimistic harvest.
///
/// The client finishes the current cycle before standing again; no resource can
/// be inferred from this packet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HarvestRefused {
    pub serial: Serial,
}

impl HarvestRefused {
    pub const ID: u8 = 0xBF;
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 14;
    pub const LENGTH_BYTES: u16 = 9;
}

impl EncodePacket for HarvestRefused {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.serial.raw());
    }
}

impl DecodePacket for HarvestRefused {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "0xBF subcommand for harvest refusal",
                value: u32::from(subcommand),
            });
        }
        let serial = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "harvest refusal serial",
            value: 0,
        })?;
        Ok(Self { serial })
    }
}

/// `0xBF` subcommand `0xE00F` — the shard has finished a harvest.
///
/// This is deliberately distinct from receiving an item update: a full
/// backpack, a failed skill roll, or a depleted vein also end the local
/// prediction, and a stack update does not reliably identify its source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HarvestCompleted {
    pub serial: Serial,
}

impl HarvestCompleted {
    pub const ID: u8 = 0xBF;
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 15;
    pub const LENGTH_BYTES: u16 = 9;
}

impl EncodePacket for HarvestCompleted {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.serial.raw());
    }
}

impl DecodePacket for HarvestCompleted {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "0xBF subcommand for harvest completion",
                value: u32::from(subcommand),
            });
        }
        let serial = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "harvest completion serial",
            value: 0,
        })?;
        Ok(Self { serial })
    }
}

/// `0x54` — play a sound at a world location. 12 bytes.
///
/// The point places the sound in 3D so the client attenuates it by distance; a
/// sound with no place (a UI blip) is not this packet. `volume` is left at
/// ServUO's `0` — the client scales by distance — and the flag byte is its fixed
/// `1`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlaySound {
    /// Which sound.
    pub sound: SoundId,
    /// Where it happens.
    pub at: Point,
}

impl EncodePacket for PlaySound {
    const ID: u8 = 0x54;
    const LENGTH: PacketLength = PacketLength::Fixed(12);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(0x01); // flags — ServUO's constant
        out.u16(self.sound.0);
        out.u16(0x0000); // volume; the client scales by distance
        out.u16(self.at.x);
        out.u16(self.at.y);
        // ServUO writes Z as a full `short`, so a negative height sign-extends to
        // 16 bits — not the 8-bit z the map tiles carry.
        out.u16(i16::from(self.at.z) as u16);
    }
}

impl DecodePacket for PlaySound {
    const ID: u8 = 0x54;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        // The first byte is a server-side playback flag and the next u16 is a
        // volume hint.  Neither changes which client asset this packet names;
        // the client keeps its own mixer settings, so consume both here rather
        // than inventing a second, packet-local volume policy.
        let _flags = reader.u8()?;
        let sound = SoundId(reader.u16()?);
        let _volume = reader.u16()?;
        Ok(Self {
            sound,
            at: Point::new(reader.u16()?, reader.u16()?, reader.u16()? as i16 as i8),
        })
    }
}

/// `0x6E` — animate a mobile with the classic action packet. 14 bytes.
///
/// The pre-7.0 form, and what a client without [`Feature::NewMobileAnimation`]
/// understands. `action` here is **body-specific**: the same number means
/// different frames on a human and on a dragon. A swing, a bow, a cast gesture,
/// a death throe are all one of these.
///
/// [`Feature::NewMobileAnimation`]: crate::feature::Feature::NewMobileAnimation
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Animation {
    /// Who moves.
    pub serial: Serial,
    /// Which action, in the numbering for this mobile's body.
    pub action: u16,
    /// How many frames the action runs for.
    pub frame_count: AnimationFrameCount,
    /// How many times to repeat it.
    pub repeat_count: u16,
    /// Play the frames in order. Written inverted on the wire, where the field is
    /// really "reverse"; this takes the intuitive sense and flips it, as ServUO
    /// does.
    pub forward: bool,
    /// Loop rather than play once.
    pub repeat: bool,
    /// Frame delay.
    pub delay: u8,
}

impl EncodePacket for Animation {
    const ID: u8 = 0x6E;
    const LENGTH: PacketLength = PacketLength::Fixed(14);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.serial.raw());
        out.u16(self.action);
        out.u16(self.frame_count.0);
        out.u16(self.repeat_count);
        out.bool(!self.forward); // the wire field is "reverse"
        out.bool(self.repeat);
        out.u8(self.delay);
    }
}

impl DecodePacket for Animation {
    const ID: u8 = 0x6E;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        Ok(Self {
            serial: Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
                field: "animation serial",
                value: 0,
            })?,
            action: reader.u16()?,
            frame_count: AnimationFrameCount(reader.u16()?),
            repeat_count: reader.u16()?,
            forward: !reader.bool()?,
            repeat: reader.bool()?,
            delay: reader.u8()?,
        })
    }
}

/// `0xE2` — animate a mobile with the 7.0.0.0+ action packet. 10 bytes.
///
/// Gate the choice between this and [`Animation`] on
/// [`Feature::NewMobileAnimation`], never on era. `animation_type` selects a
/// body-agnostic category the client maps to its own newer tables, so unlike
/// [`Animation::action`] it needs no body table on the server — and the two
/// numberings are not interchangeable.
///
/// [`Feature::NewMobileAnimation`]: crate::feature::Feature::NewMobileAnimation
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NewAnimation {
    /// Who moves.
    pub serial: Serial,
    /// Which category of action, in the `0xE2` numbering.
    pub animation_type: u16,
    /// Which action within the category.
    pub action: u16,
    /// Frame delay.
    pub delay: u8,
}

/// Number of frames in a body-specific mobile animation.
///
/// Zero means that the client has no frames to show and therefore keeps the
/// mobile on frame zero. The wrapper keeps this count distinct from an
/// animation frame's position.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct AnimationFrameCount(pub u16);

impl AnimationFrameCount {
    /// A frame count from a wire or table value.
    #[must_use]
    pub const fn new(raw: u16) -> Self {
        Self(raw)
    }

    /// The number of frames.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

impl EncodePacket for NewAnimation {
    const ID: u8 = 0xE2;
    const LENGTH: PacketLength = PacketLength::Fixed(10);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.serial.raw());
        out.u16(self.animation_type);
        out.u16(self.action);
        out.u8(self.delay);
    }
}

impl DecodePacket for NewAnimation {
    const ID: u8 = 0xE2;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        Ok(Self {
            serial: Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
                field: "new animation serial",
                value: 0,
            })?,
            animation_type: reader.u16()?,
            action: reader.u16()?,
            delay: reader.u8()?,
        })
    }
}

/// How a graphical effect moves, ServUO's `EffectType`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum EffectKind {
    /// A projectile from one point/mobile to another — a bolt, an arrow.
    Moving = 0x00,
    /// A lightning strike on the source.
    Lightning = 0x01,
    /// A fixed animation at a world point.
    FixedXyz = 0x02,
    /// A fixed animation on the source mobile.
    FixedFrom = 0x03,
}

/// `0x70` — a graphical effect: a projectile, a strike, a fixed animation. 28 bytes.
///
/// `art` is the effect's sprite (a fireball graphic, a bolt). `from`/`to` are the
/// mobiles it links, `None` when a point is used instead. The uncoloured form; for
/// a tinted or particle effect wrap it in a [`HuedEffect`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GraphicalEffect {
    /// How it moves.
    pub kind: EffectKind,
    /// The mobile it comes from, if any.
    pub from: Option<Serial>,
    /// The mobile it goes to, if any.
    pub to: Option<Serial>,
    /// The effect's sprite.
    pub art: Graphic,
    /// Where it starts.
    pub from_point: Point,
    /// Where it ends. The same as `from_point` for a fixed effect.
    pub to_point: Point,
    /// How fast a moving effect travels.
    pub speed: u8,
    /// How long a fixed effect lasts.
    pub duration: u8,
    /// Keep the sprite's orientation rather than turning it along its path.
    pub fixed_direction: bool,
    /// Play the client's explosion at the end.
    pub explode: bool,
}

impl EncodePacket for GraphicalEffect {
    const ID: u8 = 0x70;
    const LENGTH: PacketLength = PacketLength::Fixed(28);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(self.kind as u8);
        out.u32(raw_or_none(self.from));
        out.u32(raw_or_none(self.to));
        out.u16(self.art.0);
        out.u16(self.from_point.x);
        out.u16(self.from_point.y);
        out.u8(self.from_point.z as u8);
        out.u16(self.to_point.x);
        out.u16(self.to_point.y);
        out.u8(self.to_point.z as u8);
        out.u8(self.speed);
        out.u8(self.duration);
        out.u16(0x0000); // two reserved bytes ServUO zeroes
        out.bool(self.fixed_direction);
        out.bool(self.explode);
    }
}

/// `0xC0` — a hued graphical effect: a [`GraphicalEffect`] plus a colour and a
/// render mode. 36 bytes.
///
/// The body is the `0x70` body with two fields appended, and it is written by
/// [`GraphicalEffect`]'s own encoder rather than a copy of it — the two packets
/// cannot drift apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HuedEffect {
    /// Everything the uncoloured form carries.
    pub effect: GraphicalEffect,
    /// Tints the effect art: a green poison bolt, a blue frost.
    pub hue: Hue,
    /// The client's blend: 0 normal, higher values additive or translucent.
    pub render_mode: u32,
}

impl EncodePacket for HuedEffect {
    const ID: u8 = 0xC0;
    const LENGTH: PacketLength = PacketLength::Fixed(36);

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        self.effect.encode_body(out, version);
        // The hue field is a full dword here, not the u16 a hue is elsewhere.
        out.u32(u32::from(self.hue.0));
        out.u32(self.render_mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{decode_packet, encode_packet};

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    fn mobile(raw: u32) -> Serial {
        Serial::new(raw).unwrap()
    }

    #[test]
    fn play_sound_is_twelve_bytes_ported_from_servuo() {
        // 0x54, flag 1, sound, volume 0, x, y, z-as-short. A negative z must
        // sign-extend, not be truncated — a sound underground is otherwise placed
        // at z 65408 and silent.
        let packet = encode_packet(
            &PlaySound {
                sound: SoundId(0x0028),
                at: Point::new(0x0568, 0x0640, -5),
            },
            version(),
        );
        assert_eq!(packet.len(), 12);
        assert_eq!(packet[0], 0x54);
        assert_eq!(packet[1], 0x01);
        assert_eq!(&packet[2..4], &[0x00, 0x28], "sound id, big-endian");
        assert_eq!(&packet[4..6], &[0x00, 0x00], "volume");
        assert_eq!(&packet[6..8], &[0x05, 0x68], "x");
        assert_eq!(&packet[8..10], &[0x06, 0x40], "y");
        assert_eq!(&packet[10..12], &[0xFF, 0xFB], "z = -5 sign-extended");
    }

    #[test]
    fn classic_animation_inverts_forward_like_servuo() {
        // 0x6E, serial, action, frameCount, repeatCount, !forward, repeat, delay.
        let packet = encode_packet(
            &Animation {
                serial: mobile(0x0000_1234),
                action: 0x000A,
                frame_count: AnimationFrameCount(0x0007),
                repeat_count: 0x0001,
                forward: true,
                repeat: false,
                delay: 0,
            },
            version(),
        );
        assert_eq!(packet.len(), 14);
        assert_eq!(packet[0], 0x6E);
        assert_eq!(&packet[1..5], &[0x00, 0x00, 0x12, 0x34]);
        assert_eq!(&packet[5..7], &[0x00, 0x0A], "action");
        assert_eq!(&packet[7..9], &[0x00, 0x07], "frame count");
        assert_eq!(&packet[9..11], &[0x00, 0x01], "repeat count");
        assert_eq!(packet[11], 0x00, "forward=true writes reverse=0");
        assert_eq!(packet[12], 0x00, "repeat=false");
        assert_eq!(packet[13], 0x00, "delay");
    }

    #[test]
    fn new_animation_is_ten_bytes() {
        let packet = encode_packet(
            &NewAnimation {
                serial: mobile(0x0000_1234),
                animation_type: 0x0005,
                action: 0x0009,
                delay: 1,
            },
            version(),
        );
        assert_eq!(packet.len(), 10);
        assert_eq!(packet[0], 0xE2);
        assert_eq!(&packet[1..5], &[0x00, 0x00, 0x12, 0x34]);
        assert_eq!(&packet[5..7], &[0x00, 0x05], "type");
        assert_eq!(&packet[7..9], &[0x00, 0x09], "action");
        assert_eq!(packet[9], 0x01, "delay");
    }

    #[test]
    fn swing_timing_carries_a_multi_second_duration() {
        let timing = SwingTiming {
            serial: mobile(0x0000_1234),
            duration: SwingDuration(5_000),
        };
        let packet = encode_packet(&timing, version());
        assert_eq!(packet.len(), usize::from(SwingTiming::LENGTH_BYTES));
        assert_eq!(packet[0], SwingTiming::ID);
        assert_eq!(&packet[1..3], &SwingTiming::LENGTH_BYTES.to_be_bytes());
        assert_eq!(&packet[3..5], &SwingTiming::SUBCOMMAND.to_be_bytes());
        assert_eq!(&packet[5..9], &0x0000_1234_u32.to_be_bytes());
        assert_eq!(&packet[9..13], &5_000_u32.to_be_bytes());
        assert_eq!(decode_packet::<SwingTiming>(&packet, version()), Ok(timing));
    }

    #[test]
    fn harvest_tool_visual_carries_the_backpack_axes_picture() {
        let visual = HarvestToolVisual {
            serial: mobile(0x0000_1234),
            graphic: Graphic(0x0F43),
            hue: Hue(0x0481),
            layer: Layer(1),
        };
        let packet = encode_packet(&visual, version());
        assert_eq!(packet.len(), usize::from(HarvestToolVisual::LENGTH_BYTES));
        assert_eq!(packet[0], HarvestToolVisual::ID);
        assert_eq!(&packet[1..3], &HarvestToolVisual::LENGTH_BYTES.to_be_bytes());
        assert_eq!(&packet[3..5], &HarvestToolVisual::SUBCOMMAND.to_be_bytes());
        assert_eq!(&packet[5..9], &0x0000_1234_u32.to_be_bytes());
        assert_eq!(&packet[9..11], &0x0F43_u16.to_be_bytes());
        assert_eq!(decode_packet::<HarvestToolVisual>(&packet, version()), Ok(visual));
    }

    #[test]
    fn graphical_effect_is_twenty_eight_bytes() {
        let packet = encode_packet(
            &GraphicalEffect {
                kind: EffectKind::Moving,
                from: Serial::new(0x0000_0001),
                to: Serial::new(0x0000_0002),
                art: Graphic(0x36D4),
                from_point: Point::new(0x0568, 0x0640, 0),
                to_point: Point::new(0x0570, 0x0640, 0),
                speed: 7,
                duration: 0,
                fixed_direction: false,
                explode: true,
            },
            version(),
        );
        assert_eq!(packet.len(), 28);
        assert_eq!(packet[0], 0x70);
        assert_eq!(packet[1], 0x00, "EffectKind::Moving");
        assert_eq!(&packet[2..6], &[0x00, 0x00, 0x00, 0x01], "from serial");
        assert_eq!(&packet[6..10], &[0x00, 0x00, 0x00, 0x02], "to serial");
        assert_eq!(&packet[10..12], &[0x36, 0xD4], "effect graphic");
        assert_eq!(packet[27], 0x01, "explode=true");
    }

    #[test]
    fn an_effect_with_no_mobiles_writes_zero_serials() {
        let packet = encode_packet(
            &GraphicalEffect {
                kind: EffectKind::FixedXyz,
                from: None,
                to: None,
                art: Graphic(0x373A),
                from_point: Point::new(1, 2, 3),
                to_point: Point::new(1, 2, 3),
                speed: 9,
                duration: 20,
                fixed_direction: true,
                explode: false,
            },
            version(),
        );
        assert_eq!(&packet[2..10], &[0u8; 8], "both serial fields are empty");
    }

    #[test]
    fn hued_effect_is_thirty_six_bytes_with_the_colour_last() {
        let effect = GraphicalEffect {
            kind: EffectKind::FixedFrom,
            from: Serial::new(0x0000_0001),
            to: None,
            art: Graphic(0x373A),
            from_point: Point::new(0x0568, 0x0640, 0),
            to_point: Point::new(0x0568, 0x0640, 0),
            speed: 9,
            duration: 20,
            fixed_direction: true,
            explode: false,
        };
        let packet = encode_packet(
            &HuedEffect {
                effect,
                hue: Hue(0x0026),
                render_mode: 0x0000_0001,
            },
            version(),
        );
        assert_eq!(packet.len(), 36);
        assert_eq!(packet[0], 0xC0);
        assert_eq!(packet[1], 0x03, "EffectKind::FixedFrom");
        assert_eq!(&packet[28..32], &[0x00, 0x00, 0x00, 0x26], "hue");
        assert_eq!(&packet[32..36], &[0x00, 0x00, 0x00, 0x01], "render mode");
    }

    #[test]
    fn a_hued_effect_body_is_the_plain_one_plus_eight_bytes() {
        // The reuse is the point: 0xC0 cannot drift away from 0x70.
        let effect = GraphicalEffect {
            kind: EffectKind::Lightning,
            from: Serial::new(0x0000_0007),
            to: None,
            art: Graphic(0x0000),
            from_point: Point::new(10, 20, -3),
            to_point: Point::new(10, 20, -3),
            speed: 0,
            duration: 0,
            fixed_direction: false,
            explode: false,
        };
        let plain = encode_packet(&effect, version());
        let hued = encode_packet(
            &HuedEffect {
                effect,
                hue: Hue::NONE,
                render_mode: 0,
            },
            version(),
        );
        assert_eq!(plain[1..], hued[1..28], "same body, different id");
        assert_eq!(&hued[28..], &[0u8; 8]);
    }
}
