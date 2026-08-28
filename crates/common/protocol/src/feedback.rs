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

/// What a combat action's impact does — the axis `docs/combat_actions.md` calls
/// *kind*.
///
/// It is on the wire because a watcher draws a drawn bow differently from a
/// raised axe, and because the two end differently: a shot spends a round. The
/// three are the whole list; a fourth is a protocol change, deliberately.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CombatActionKind {
    /// A blow committed to a reach.
    Swing,
    /// A shot from a wielded ranged weapon, its round already out of the pack.
    Shot,
    /// An innate ranged attack — a breath weapon. It carries no ammunition.
    Breath,
}

impl CombatActionKind {
    /// The byte this kind is written as.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Swing => 0,
            Self::Shot => 1,
            Self::Breath => 2,
        }
    }

    /// Read a kind byte, or `None` for one this build does not know.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Swing),
            1 => Some(Self::Shot),
            2 => Some(Self::Breath),
            _ => None,
        }
    }
}

/// Which phase of an action's life the actor just entered.
///
/// The duration means a different thing in each, which is why they are one enum
/// and not a flag beside a number: an arming action is preparing, an armed
/// action is waiting, and a releasing one is landing. A zero-length timed action would be the lie
/// `docs/combat_actions.md`'s wire section refuses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActionPhase {
    /// Preparing the action before it can be held: a bow is being raised and
    /// drawn, for this long.
    Arming { ready_in: SwingDuration },
    /// Ready and waiting on the world, for at most this long.
    Armed { endurance: SwingDuration },
    /// Released: the impact lands after this long.
    Releasing { impact_in: SwingDuration },
}

impl ActionPhase {
    /// The byte this phase is written as.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Arming { .. } => 2,
            Self::Armed { .. } => 0,
            Self::Releasing { .. } => 1,
        }
    }

    /// The interval it carries, whichever of the two it is.
    #[must_use]
    pub const fn duration(self) -> SwingDuration {
        match self {
            Self::Arming { ready_in } => ready_in,
            Self::Armed { endurance } => endurance,
            Self::Releasing { impact_in } => impact_in,
        }
    }

    /// Rebuild a phase from its byte and its interval.
    #[must_use]
    pub const fn from_bits(bits: u8, duration: SwingDuration) -> Option<Self> {
        match bits {
            0 => Some(Self::Armed { endurance: duration }),
            1 => Some(Self::Releasing { impact_in: duration }),
            2 => Some(Self::Arming { ready_in: duration }),
            _ => None,
        }
    }
}

/// Why an action stopped without landing.
///
/// A closed list, and each entry is a fact the server tests at a seam it already
/// runs. The player is owed the reason: a swing that vanished with no word is
/// the defect `docs/combat_actions.md` opens with.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum InterruptReason {
    /// The committed target died, logged out, or left the facet.
    TargetGone,
    /// The committed target is no longer within the action's committed reach.
    OutOfReach,
    /// The line to the committed target was cut — a door, a wall.
    NoLineOfSight,
    /// A bard calmed the actor mid-action.
    Pacified,
    /// The actor stopped: it disengaged, aimed at somebody else, died, or left.
    Abandoned,
    /// The round the shot was committed to was gone from the pack by the loose —
    /// dropped, traded or given away while the bow was being drawn. The refusal
    /// for an empty quiver comes at the nock instead, before there is an action
    /// to interrupt at all.
    NoAmmo,
    /// The actor moved, and the shard's rules table says this action does not
    /// survive that. Walking, running and riding all end under this one name:
    /// what a watcher is being told is that the fighter moved, not which of the
    /// three it was doing.
    Moved,
    /// A wound spoiled it. The condition rule, not the damage itself — a shard
    /// whose table lets a fighter swing through a blow never sends this.
    Struck,
    /// The weapon is drawn and nothing is aimed at. A [`BalkState`] alone: it
    /// can never end an action, because an action carries the target it
    /// committed to and cannot outlive it. It is here rather than in a list of
    /// its own because a watcher asks one question — *why is that fighter
    /// standing there* — and must not need two vocabularies to hear the answer.
    NoTarget,
}

impl InterruptReason {
    /// The byte this reason is written as. Never `0`, which is the "no reason"
    /// filler an outcome that is not an interruption writes.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::TargetGone => 1,
            Self::OutOfReach => 2,
            Self::NoLineOfSight => 3,
            Self::Pacified => 4,
            Self::Abandoned => 5,
            Self::NoAmmo => 6,
            Self::Moved => 7,
            Self::Struck => 8,
            Self::NoTarget => 9,
        }
    }

    /// Read a reason byte, or `None` for `0` and for one this build does not know.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            1 => Some(Self::TargetGone),
            2 => Some(Self::OutOfReach),
            3 => Some(Self::NoLineOfSight),
            4 => Some(Self::Pacified),
            5 => Some(Self::Abandoned),
            6 => Some(Self::NoAmmo),
            7 => Some(Self::Moved),
            8 => Some(Self::Struck),
            9 => Some(Self::NoTarget),
            _ => None,
        }
    }
}

/// How a combat action ended.
///
/// Every action ends, and every end crosses the wire — without this a cancelled
/// telegraph keeps playing on the watcher's screen for its promised duration.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CombatActionOutcome {
    /// The impact landed and did damage.
    Hit,
    /// The impact landed and found only air.
    Miss,
    /// It never reached its impact.
    Interrupted(InterruptReason),
    /// An armed action ran out of endurance without its watch ever firing.
    Expired,
}

impl CombatActionOutcome {
    /// The byte this outcome is written as.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Hit => 0,
            Self::Miss => 1,
            Self::Interrupted(_) => 2,
            Self::Expired => 3,
        }
    }

    /// The reason byte, `0` for every outcome that is not an interruption.
    #[must_use]
    pub const fn reason_bits(self) -> u8 {
        match self {
            Self::Interrupted(reason) => reason.to_bits(),
            Self::Hit | Self::Miss | Self::Expired => 0,
        }
    }

    /// Rebuild an outcome from its two bytes. The reason byte is read only for
    /// an interruption, and an interruption with no reason is malformed.
    #[must_use]
    pub const fn from_bits(outcome: u8, reason: u8) -> Option<Self> {
        match outcome {
            0 => Some(Self::Hit),
            1 => Some(Self::Miss),
            2 => match InterruptReason::from_bits(reason) {
                Some(reason) => Some(Self::Interrupted(reason)),
                None => None,
            },
            3 => Some(Self::Expired),
            _ => None,
        }
    }
}

/// `0xBF` subcommand `0xE010` — a mobile has entered a phase of a combat action.
///
/// Sent at the commit and again at the release, because those are two different
/// pictures and the second is not implied by the first: an archer who looses is
/// not an archer holding a draw, and a watcher should be told rather than left
/// to guess from the arrival of an animation.
///
/// [`SwingTiming`] is not this packet and is not repurposed into it: it carries a
/// duration and nothing else, harvesting uses it too, and an armed action has no
/// duration to carry — a zero there already means "forget the timing you were
/// given".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CombatActionPhase {
    /// Whose action it is.
    pub actor: Serial,
    /// What it is committed to.
    pub target: Serial,
    /// What its impact will do.
    pub kind: CombatActionKind,
    /// The phase just entered, and the interval that phase measures.
    pub phase: ActionPhase,
}

impl CombatActionPhase {
    /// The extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// Follows [`HarvestCompleted::SUBCOMMAND`].
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 16;
    /// Id, length, subcommand, actor, target, kind, phase and interval.
    pub const LENGTH_BYTES: u16 = 19;
}

impl EncodePacket for CombatActionPhase {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.actor.raw());
        out.u32(self.target.raw());
        out.u8(self.kind.to_bits());
        out.u8(self.phase.to_bits());
        out.u32(self.phase.duration().0);
    }
}

impl DecodePacket for CombatActionPhase {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "0xBF subcommand for combat action phase",
                value: u32::from(subcommand),
            });
        }
        let actor = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "combat action actor",
            value: 0,
        })?;
        let target = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "combat action target",
            value: 0,
        })?;
        let kind_bits = reader.u8()?;
        let kind = CombatActionKind::from_bits(kind_bits).ok_or(crate::error::DecodeError::UnknownValue {
            field: "combat action kind",
            value: u32::from(kind_bits),
        })?;
        let phase_bits = reader.u8()?;
        let duration = SwingDuration(reader.u32()?);
        let phase =
            ActionPhase::from_bits(phase_bits, duration).ok_or(crate::error::DecodeError::UnknownValue {
                field: "combat action phase",
                value: u32::from(phase_bits),
            })?;
        Ok(Self {
            actor,
            target,
            kind,
            phase,
        })
    }
}

/// `0xBF` subcommand `0xE011` — a mobile's combat action is over, and this is
/// how it ended.
///
/// The half the wire was missing: the beginning already crossed as an animation
/// with a duration, so a telegraph that was cancelled had no way to stop, and
/// ran out its promised interval over an empty tile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CombatActionEnded {
    /// Whose action ended.
    pub actor: Serial,
    /// How it ended.
    pub outcome: CombatActionOutcome,
}

impl CombatActionEnded {
    /// The extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// Follows [`CombatActionPhase::SUBCOMMAND`].
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 17;
    /// Id, length, subcommand, actor, outcome and reason.
    pub const LENGTH_BYTES: u16 = 11;
}

impl EncodePacket for CombatActionEnded {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.actor.raw());
        out.u8(self.outcome.to_bits());
        out.u8(self.outcome.reason_bits());
    }
}

impl DecodePacket for CombatActionEnded {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "0xBF subcommand for combat action end",
                value: u32::from(subcommand),
            });
        }
        let actor = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "combat action actor",
            value: 0,
        })?;
        let outcome_bits = reader.u8()?;
        let reason_bits = reader.u8()?;
        let outcome = CombatActionOutcome::from_bits(outcome_bits, reason_bits).ok_or(
            crate::error::DecodeError::UnknownValue {
                field: "combat action outcome",
                value: u32::from(outcome_bits) << 8 | u32::from(reason_bits),
            },
        )?;
        Ok(Self { actor, outcome })
    }
}

/// Which named stretch of a released action the actor has just entered.
///
/// A bar answers *how far along* and cannot answer *how far along what* — an
/// archer who has the string at their cheek and one who has only lifted the bow
/// occupy the same rectangle. These are the four stretches every kind of impact
/// is made of, and the reason they are named neutrally rather than *draw* /
/// *swing* is that the same four fit all three kinds: what a watcher is told is
/// the shape of the effort, and the word it is drawn as is the kind's.
///
/// Where each begins is an operator setting, not a constant here: the shard owns
/// the boundaries and the wire only carries which side of them the action is on.
/// That is what keeps this a fact rather than the client's guess from a
/// percentage — the one thing a picture must never invent.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ActionStage {
    /// The weapon is coming up. Nothing is committed to a direction yet, and
    /// this is the stretch an action spends being *begun*.
    Ready,
    /// The effort: the bow bends, the arm cocks, the lungs fill.
    Load,
    /// Held on the mark. The last stretch in which a defender can still spoil
    /// what is coming.
    Aim,
    /// The stroke, the loose, the exhalation — the part that reaches the impact.
    Release,
}

impl ActionStage {
    /// The first stretch of any action, which is what a commit announces.
    pub const FIRST: Self = Self::Ready;

    /// The byte this stage is written as. Ordered, and the order is load-bearing:
    /// an action never goes backwards through them, so a shard comparing two
    /// stages is comparing progress.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Ready => 0,
            Self::Load => 1,
            Self::Aim => 2,
            Self::Release => 3,
        }
    }

    /// Read a stage byte, or `None` for one this build does not know.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Ready),
            1 => Some(Self::Load),
            2 => Some(Self::Aim),
            3 => Some(Self::Release),
            _ => None,
        }
    }
}

/// Whether a fighter can begin an action at all, and if not, why not.
///
/// The state `docs/combat_actions.md`'s D1 left with no name. An action that
/// fails at the *impact* ends with a reason and the reason crosses the wire; an
/// action that never began because the target is round a corner produced
/// nothing at all — the commit pass simply declined, every tick, in silence.
/// From a screen that is indistinguishable from a shard that has stopped
/// working, which is exactly what it was mistaken for.
///
/// Not an outcome and not a phase: an outcome is a thing that *happened* and
/// fades, and a phase belongs to an action that exists. This is a standing
/// condition of a fighter with no action, and it lasts as long as the obstacle
/// does — which is why it is sent on entering and again on leaving rather than
/// every tick it holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BalkState {
    /// Cannot begin, and this is what is in the way.
    Blocked(InterruptReason),
    /// Can begin again: whatever stood in the way is gone.
    Clear,
}

impl BalkState {
    /// The byte this state is written as. `0` is *clear* — which is free,
    /// because [`InterruptReason::to_bits`] never writes a zero.
    #[must_use]
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Blocked(reason) => reason.to_bits(),
            Self::Clear => 0,
        }
    }

    /// Read a balk byte, or `None` for a reason this build does not know.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Clear),
            other => match InterruptReason::from_bits(other) {
                Some(reason) => Some(Self::Blocked(reason)),
                None => None,
            },
        }
    }
}

/// `0xBF` subcommand `0xE012` — a fighter cannot begin an action, or can again.
///
/// The third thing a watcher can see a fighter doing, after *preparing* and
/// *having just finished*: standing there unable to start. Sent on the edge in
/// both directions and never in between, so a bowman held off by a wall costs
/// two packets and not one per tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CombatActionBalked {
    /// Whose commit is being refused, or is refused no longer.
    pub actor: Serial,
    /// What is in the way, or that nothing is.
    pub balk: BalkState,
}

impl CombatActionBalked {
    /// The extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// Follows [`CombatActionEnded::SUBCOMMAND`].
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 18;
    /// Id, length, subcommand, actor and the balk byte.
    pub const LENGTH_BYTES: u16 = 10;
}

impl EncodePacket for CombatActionBalked {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.actor.raw());
        out.u8(self.balk.to_bits());
    }
}

impl DecodePacket for CombatActionBalked {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "0xBF subcommand for combat action balk",
                value: u32::from(subcommand),
            });
        }
        let actor = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "combat action actor",
            value: 0,
        })?;
        let balk_bits = reader.u8()?;
        let balk = BalkState::from_bits(balk_bits).ok_or(crate::error::DecodeError::UnknownValue {
            field: "combat action balk",
            value: u32::from(balk_bits),
        })?;
        Ok(Self { actor, balk })
    }
}

/// `0xBF` subcommand `0xE013` — a running action has entered a new stage.
///
/// Deliberately not folded into [`CombatActionPhase`], which carries the
/// interval a picture is measured against: a stage changes *within* that
/// interval, and re-sending the phase would restart the client's clock and reset
/// the very bar the stage is meant to annotate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CombatActionStage {
    /// Whose action it is.
    pub actor: Serial,
    /// The stretch just entered.
    pub stage: ActionStage,
}

impl CombatActionStage {
    /// The extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// Follows [`CombatActionBalked::SUBCOMMAND`].
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 19;
    /// Id, length, subcommand, actor and the stage byte.
    pub const LENGTH_BYTES: u16 = 10;
}

impl EncodePacket for CombatActionStage {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(Self::LENGTH_BYTES);
        out.u16(Self::SUBCOMMAND);
        out.u32(self.actor.raw());
        out.u8(self.stage.to_bits());
    }
}

impl DecodePacket for CombatActionStage {
    const ID: u8 = Self::ID;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "0xBF subcommand for combat action stage",
                value: u32::from(subcommand),
            });
        }
        let actor = Serial::new(reader.u32()?).ok_or(crate::error::DecodeError::UnknownValue {
            field: "combat action actor",
            value: 0,
        })?;
        let stage_bits = reader.u8()?;
        let stage = ActionStage::from_bits(stage_bits).ok_or(crate::error::DecodeError::UnknownValue {
            field: "combat action stage",
            value: u32::from(stage_bits),
        })?;
        Ok(Self { actor, stage })
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

impl EffectKind {
    /// Decode the wire byte, or `None` for a value ServUO never sends.
    #[must_use]
    pub const fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0x00 => Some(Self::Moving),
            0x01 => Some(Self::Lightning),
            0x02 => Some(Self::FixedXyz),
            0x03 => Some(Self::FixedFrom),
            _ => None,
        }
    }
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

impl DecodePacket for GraphicalEffect {
    const ID: u8 = 0x70;

    fn decode_body(
        reader: &mut crate::codec::PacketReader<'_>,
        _version: ClientVersion,
    ) -> Result<Self, crate::error::DecodeError> {
        let byte = reader.u8()?;
        let Some(kind) = EffectKind::from_wire(byte) else {
            return Err(crate::error::DecodeError::UnknownValue {
                field: "graphical effect kind",
                value: u32::from(byte),
            });
        };
        let from = Serial::new(reader.u32()?);
        let to = Serial::new(reader.u32()?);
        let art = Graphic(reader.u16()?);
        let from_point = Point {
            x: reader.u16()?,
            y: reader.u16()?,
            z: reader.u8()? as i8,
        };
        let to_point = Point {
            x: reader.u16()?,
            y: reader.u16()?,
            z: reader.u8()? as i8,
        };
        let speed = reader.u8()?;
        let duration = reader.u8()?;
        let _reserved = reader.u16()?;
        let fixed_direction = reader.bool()?;
        let explode = reader.bool()?;
        Ok(Self {
            kind,
            from,
            to,
            art,
            from_point,
            to_point,
            speed,
            duration,
            fixed_direction,
            explode,
        })
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
    fn a_released_action_carries_its_target_and_the_time_to_impact() {
        let phase = CombatActionPhase {
            actor: mobile(0x0000_1234),
            target: mobile(0x0000_5678),
            kind: CombatActionKind::Swing,
            phase: ActionPhase::Releasing {
                impact_in: SwingDuration(1_500),
            },
        };
        let packet = encode_packet(&phase, version());
        assert_eq!(packet.len(), usize::from(CombatActionPhase::LENGTH_BYTES));
        assert_eq!(packet[0], CombatActionPhase::ID);
        assert_eq!(&packet[1..3], &CombatActionPhase::LENGTH_BYTES.to_be_bytes());
        assert_eq!(&packet[3..5], &CombatActionPhase::SUBCOMMAND.to_be_bytes());
        assert_eq!(&packet[5..9], &0x0000_1234_u32.to_be_bytes());
        assert_eq!(&packet[9..13], &0x0000_5678_u32.to_be_bytes());
        assert_eq!(packet[13], 0, "a swing");
        assert_eq!(packet[14], 1, "releasing");
        assert_eq!(&packet[15..19], &1_500_u32.to_be_bytes());
        assert_eq!(decode_packet::<CombatActionPhase>(&packet, version()), Ok(phase));
    }

    /// An armed action has no duration to the impact, and saying so as a
    /// zero-length timed one is the lie the second packet exists to avoid.
    #[test]
    fn an_armed_action_carries_its_endurance_rather_than_an_impact() {
        let phase = CombatActionPhase {
            actor: mobile(0x0000_1234),
            target: mobile(0x0000_5678),
            kind: CombatActionKind::Shot,
            phase: ActionPhase::Armed {
                endurance: SwingDuration(8_000),
            },
        };
        let packet = encode_packet(&phase, version());
        assert_eq!(packet[13], 1, "a shot");
        assert_eq!(packet[14], 0, "armed");
        assert_eq!(&packet[15..19], &8_000_u32.to_be_bytes());
        assert_eq!(decode_packet::<CombatActionPhase>(&packet, version()), Ok(phase));
    }

    #[test]
    fn an_arming_action_carries_the_time_until_it_can_be_held() {
        let phase = CombatActionPhase {
            actor: mobile(0x0000_1234),
            target: mobile(0x0000_5678),
            kind: CombatActionKind::Shot,
            phase: ActionPhase::Arming {
                ready_in: SwingDuration(1_250),
            },
        };
        let packet = encode_packet(&phase, version());
        assert_eq!(packet[14], 2, "arming is distinct from held and released");
        assert_eq!(&packet[15..19], &1_250_u32.to_be_bytes());
        assert_eq!(decode_packet::<CombatActionPhase>(&packet, version()), Ok(phase));
    }

    #[test]
    fn an_interruption_names_its_reason_and_a_hit_does_not() {
        let interrupted = CombatActionEnded {
            actor: mobile(0x0000_1234),
            outcome: CombatActionOutcome::Interrupted(InterruptReason::OutOfReach),
        };
        let packet = encode_packet(&interrupted, version());
        assert_eq!(packet.len(), usize::from(CombatActionEnded::LENGTH_BYTES));
        assert_eq!(&packet[3..5], &CombatActionEnded::SUBCOMMAND.to_be_bytes());
        assert_eq!(&packet[5..9], &0x0000_1234_u32.to_be_bytes());
        assert_eq!(packet[9], 2, "interrupted");
        assert_eq!(packet[10], 2, "out of reach");
        assert_eq!(
            decode_packet::<CombatActionEnded>(&packet, version()),
            Ok(interrupted)
        );

        let hit = CombatActionEnded {
            actor: mobile(0x0000_1234),
            outcome: CombatActionOutcome::Hit,
        };
        let packet = encode_packet(&hit, version());
        assert_eq!(packet[9], 0, "hit");
        assert_eq!(
            packet[10], 0,
            "an outcome that is not an interruption has no reason"
        );
        assert_eq!(decode_packet::<CombatActionEnded>(&packet, version()), Ok(hit));
    }

    /// A reason of `0` is the filler every other outcome writes, so it cannot
    /// also stand for an interruption — reading one back is malformed, not a
    /// nameless interruption.
    #[test]
    fn an_interruption_without_a_reason_does_not_decode() {
        assert_eq!(CombatActionOutcome::from_bits(2, 0), None);
        assert_eq!(
            CombatActionOutcome::from_bits(2, InterruptReason::Pacified.to_bits()),
            Some(CombatActionOutcome::Interrupted(InterruptReason::Pacified))
        );
    }

    /// The balk byte shares its numbering with an interruption's reason, which
    /// is only sound because a reason is never `0` — that zero is what *clear*
    /// is written as.
    #[test]
    fn a_balk_names_what_is_in_the_way_and_clears_with_a_zero() {
        let blocked = CombatActionBalked {
            actor: mobile(0x0000_1234),
            balk: BalkState::Blocked(InterruptReason::NoLineOfSight),
        };
        let packet = encode_packet(&blocked, version());
        assert_eq!(packet.len(), usize::from(CombatActionBalked::LENGTH_BYTES));
        assert_eq!(packet[0], CombatActionBalked::ID);
        assert_eq!(&packet[1..3], &CombatActionBalked::LENGTH_BYTES.to_be_bytes());
        assert_eq!(&packet[3..5], &CombatActionBalked::SUBCOMMAND.to_be_bytes());
        assert_eq!(&packet[5..9], &0x0000_1234_u32.to_be_bytes());
        assert_eq!(packet[9], 3, "no line of sight");
        assert_eq!(
            decode_packet::<CombatActionBalked>(&packet, version()),
            Ok(blocked)
        );

        let clear = CombatActionBalked {
            actor: mobile(0x0000_1234),
            balk: BalkState::Clear,
        };
        let packet = encode_packet(&clear, version());
        assert_eq!(packet[9], 0, "clear");
        assert_eq!(decode_packet::<CombatActionBalked>(&packet, version()), Ok(clear));
    }

    #[test]
    fn a_stage_carries_which_stretch_of_an_action_began() {
        let stage = CombatActionStage {
            actor: mobile(0x0000_1234),
            stage: ActionStage::Aim,
        };
        let packet = encode_packet(&stage, version());
        assert_eq!(packet.len(), usize::from(CombatActionStage::LENGTH_BYTES));
        assert_eq!(&packet[3..5], &CombatActionStage::SUBCOMMAND.to_be_bytes());
        assert_eq!(&packet[5..9], &0x0000_1234_u32.to_be_bytes());
        assert_eq!(packet[9], 2, "aim");
        assert_eq!(decode_packet::<CombatActionStage>(&packet, version()), Ok(stage));
    }

    /// The four stretches are ordered, and the sustain pass compares them to
    /// decide whether an action has moved on — so the bytes have to sort the
    /// same way the stages do.
    #[test]
    fn the_stages_are_written_in_the_order_they_happen() {
        let stages = [
            ActionStage::Ready,
            ActionStage::Load,
            ActionStage::Aim,
            ActionStage::Release,
        ];
        for pair in stages.windows(2) {
            assert!(pair[0] < pair[1], "{:?} comes before {:?}", pair[0], pair[1]);
            assert!(pair[0].to_bits() < pair[1].to_bits());
        }
        assert_eq!(ActionStage::FIRST, ActionStage::Ready);
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
