//! `0xD7` — the AoS "encoded command", a family of client requests keyed by a
//! subcommand word.
//!
//! The paperdoll's Quest and Guild buttons are here, and nowhere else: they are
//! not gump replies (the paperdoll is drawn entirely client-side and has no
//! server-sent layout to answer), so a shard that does not read `0xD7` has a
//! paperdoll with two dead buttons and no way to tell. The layout is
//! `[0xD7][length u16][serial u32][subcommand u16][payload]`, from ServUO's
//! `PacketHandlers.EncodedCommand` and Sphere's `Event_ExtCmd` equivalent — the
//! two agree.

use crate::codec::PacketWriter;
use crate::error::{
    DecodeError,
    expect_id,
};
use crate::packet::{
    PacketLength,
    frame_body,
};

/// `0xD7` — a client request named by its subcommand.
///
/// Only the header is decoded. Every subcommand this engine acts on carries no
/// payload; one that does can read the rest itself rather than making this type
/// know about all of them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EncodedCommand {
    /// The entity the command is about — the player's own serial, for the
    /// paperdoll buttons. Class D: never read. Nothing here routes by it —
    /// every subcommand this engine acts on already knows whose paperdoll
    /// sent it from the connection, the same shape as [`RawGumpKey`]'s echo.
    ///
    /// [`RawGumpKey`]: crate::gump::RawGumpKey
    pub serial:     RawEncodedSerial,
    /// Which command, exactly as sent. See [`RawEncodedSubcommand::interpret`].
    pub subcommand: RawEncodedSubcommand,
}

impl EncodedCommand {
    /// The packet id.
    pub const ID: u8 = 0xD7;

    /// Decode the header of a `0xD7`.
    ///
    /// Every field is read through the bounds-checked reader, so a truncated
    /// packet is an error rather than a panic — the length on the wire is the
    /// client's word, not this end's.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        // The packet carries its own u16 length at offset 1; the framer already
        // sized the slice, so it is read past rather than trusted.
        reader.u16()?;
        let serial = RawEncodedSerial(reader.u32()?);
        let subcommand = RawEncodedSubcommand(reader.u16()?);
        Ok(Self { serial, subcommand })
    }

    /// The `0xD7` this client sends for one subcommand, with the trailing byte
    /// the reference writes after the word.
    ///
    /// Private, and reached through the two named requests below, because that
    /// trailing byte is *not* a constant: `Send_QuestMenuRequest` writes a zero
    /// and `Send_GuildMenuRequest` a `0x0A`. It is the subcommand's own payload,
    /// short enough to look like padding, and a single encoder taking whatever
    /// the caller happened to pass would let one button send the other's.
    fn encode(serial: RawEncodedSerial, subcommand: u16, payload: u8) -> Vec<u8> {
        frame_body(Self::ID, PacketLength::Variable, |out: &mut PacketWriter| {
            out.u32(serial.0);
            out.u16(subcommand);
            out.u8(payload);
        })
    }
}

/// The paperdoll's Quest button: open the quest log —
/// `GameActions.RequestQuestMenu`, whose packet is `0xD7` subcommand `0x32`
/// followed by a zero byte.
///
/// `serial` is the asking player's own, which is what the reference writes and
/// what this engine ignores on the way in (see [`EncodedCommand::serial`]).
#[must_use]
pub fn quest_log_request(serial: RawEncodedSerial) -> Vec<u8> {
    EncodedCommand::encode(serial, EncodedSubcommand::QUEST_GUMP_REQUEST, 0x00)
}

/// The paperdoll's Guild button — `0xD7` subcommand `0x28`, and the `0x0A`
/// `Send_GuildMenuRequest` writes after it.
///
/// The shard opens its own guild window on this — founding, the roster, wars and
/// alliances. The button was written before `guilds` existed, on the argument
/// that a packet which never leaves is a defect the day the system lands and one
/// nobody would look for; the system landed, and it was.
#[must_use]
pub fn guild_menu_request(serial: RawEncodedSerial) -> Vec<u8> {
    EncodedCommand::encode(serial, EncodedSubcommand::GUILD_GUMP_REQUEST, 0x0A)
}

/// The entity a `0xD7` claims to be about, exactly as sent. No promotion: see
/// [`EncodedCommand::serial`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawEncodedSerial(pub u32);

/// A `0xD7` subcommand word exactly as sent, not yet checked against the ones
/// this engine names.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct RawEncodedSubcommand(pub u16);

impl RawEncodedSubcommand {
    /// Total: every subcommand this engine has never seen is `Other`, exactly
    /// [`crate::speech::TalkMode`]'s shape — a byte with a name beats an enum
    /// with a guessed leftover arm.
    #[inline]
    #[must_use]
    pub const fn interpret(self) -> EncodedSubcommand {
        match self.0 {
            EncodedSubcommand::SET_ABILITY => EncodedSubcommand::SetAbility,
            EncodedSubcommand::END_CUSTOMISATION => EncodedSubcommand::EndCustomisation,
            EncodedSubcommand::GUILD_GUMP_REQUEST => EncodedSubcommand::GuildGumpRequest,
            EncodedSubcommand::QUEST_GUMP_REQUEST => EncodedSubcommand::QuestGumpRequest,
            other => EncodedSubcommand::Other(other),
        }
    }
}

/// A `0xD7` subcommand this engine has a name for, or the raw word if it does
/// not.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EncodedSubcommand {
    /// Set a weapon's special ability (AoS). Not acted on: combat has no
    /// abilities yet. Named so the byte layout is not re-derived when it does.
    SetAbility,
    /// The house-design window was closed — end the customisation session.
    ///
    /// The **only** design subcommand that is a bracket rather than an edit, and
    /// the first of the family this engine speaks. Its opposite number is not
    /// here: customisation *begins* from the house's own window, server-side,
    /// exactly as the reference's `BeginCustomize` does.
    EndCustomisation,
    /// The paperdoll's Guild button — the shard answers it with the guild window.
    GuildGumpRequest,
    /// The paperdoll's Quest button — open the quest log.
    QuestGumpRequest,
    /// A subcommand this engine does not name.
    Other(u16),
}

impl EncodedSubcommand {
    const SET_ABILITY: u16 = 0x19;
    /// ServUO's `Designer_Close`, registered at `HouseFoundation.cs:815`. The
    /// hex is read out of the reference rather than guessed, per `style.md`'s
    /// "ports name their source".
    const END_CUSTOMISATION: u16 = 0x0C;
    const GUILD_GUMP_REQUEST: u16 = 0x28;
    const QUEST_GUMP_REQUEST: u16 = 0x32;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(serial: u32, subcommand: u16) -> Vec<u8> {
        let mut bytes = vec![0xD7u8, 0, 0];
        bytes.extend_from_slice(&serial.to_be_bytes());
        bytes.extend_from_slice(&subcommand.to_be_bytes());
        let length = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&length.to_be_bytes());
        bytes
    }

    #[test]
    fn an_encoded_command_reads_its_serial_and_subcommand() {
        let got =
            EncodedCommand::decode(&packet(0x0000_1234, EncodedSubcommand::QUEST_GUMP_REQUEST)).unwrap();
        assert_eq!(got.serial, RawEncodedSerial(0x0000_1234));
        assert_eq!(got.subcommand.interpret(), EncodedSubcommand::QuestGumpRequest);
    }

    #[test]
    fn a_truncated_encoded_command_is_refused_not_panicked() {
        let full = packet(1, EncodedSubcommand::GUILD_GUMP_REQUEST);
        for cut in 1..full.len() {
            assert!(
                EncodedCommand::decode(&full[..cut]).is_err(),
                "a {cut}-byte packet must not decode"
            );
        }
    }

    #[test]
    fn another_packet_id_is_not_an_encoded_command() {
        let mut bytes = packet(1, EncodedSubcommand::SET_ABILITY);
        bytes[0] = 0xD6;
        assert!(EncodedCommand::decode(&bytes).is_err());
    }

    /// The two paperdoll buttons, written by this crate and read back by it:
    /// the length field the framer patched, the subcommand each button means,
    /// and the trailing byte that differs between them.
    #[test]
    fn the_two_paperdoll_requests_decode_as_themselves() {
        let quest = quest_log_request(RawEncodedSerial(0x0000_002A));
        assert_eq!(quest.len(), 10, "id, length, serial, subcommand, payload");
        assert_eq!(
            &quest[1..3],
            &10u16.to_be_bytes(),
            "the framer patched the length"
        );
        assert_eq!(quest[9], 0x00, "the quest request's own trailing byte");
        let decoded = EncodedCommand::decode(&quest).unwrap();
        assert_eq!(decoded.serial, RawEncodedSerial(0x0000_002A));
        assert_eq!(
            decoded.subcommand.interpret(),
            EncodedSubcommand::QuestGumpRequest
        );

        let guild = guild_menu_request(RawEncodedSerial(0x0000_002A));
        assert_eq!(guild[9], 0x0A, "and the guild request's is not the same byte");
        assert_eq!(
            EncodedCommand::decode(&guild).unwrap().subcommand.interpret(),
            EncodedSubcommand::GuildGumpRequest
        );
    }

    /// The one design subcommand this engine reads, and the neighbour it must
    /// not be confused with: `0x0C` closes the editor, `0x0D` lays a stair.
    #[test]
    fn the_close_subcommand_is_the_one_the_reference_registers() {
        assert_eq!(
            EncodedCommand::decode(&packet(1, 0x0C))
                .unwrap()
                .subcommand
                .interpret(),
            EncodedSubcommand::EndCustomisation
        );
        assert_eq!(
            RawEncodedSubcommand(0x0D).interpret(),
            EncodedSubcommand::Other(0x0D),
            "the stair subcommand is not the close one"
        );
    }

    #[test]
    fn an_unnamed_subcommand_interprets_total_to_other() {
        // N1 amendment 1's shape: every one of the 65536 words this engine has
        // never named still interprets, to the raw word rather than a panic
        // or a guessed name.
        assert_eq!(
            RawEncodedSubcommand(0x7F).interpret(),
            EncodedSubcommand::Other(0x7F)
        );
    }
}
