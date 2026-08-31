//! Who is allowed to run privileged commands — and, since [`AuthorityNotice`],
//! how the shard tells a client what it holds *them* at.

use std::fmt;
use std::str::FromStr;

use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
};
use crate::version::ClientVersion;

/// A mobile's authority: what staff commands, if any, it may run.
///
/// Ordered, so a gate is a comparison — `level >= AccessLevel::GameMaster`. It
/// lives here, beside [`crate::DenyReason`] and the other account-shaped types,
/// because it is the one crate the login server, the world and the binary all
/// already share, and so the one place all three can name the same level without
/// a new dependency between them.
///
/// The **decision** is always the shard's: every gate is checked where the
/// command runs (`World::say`, before `gm::run` is entered at all), and nothing
/// a client says about its own level is read. [`AuthorityNotice`] tells a client
/// what that decision *was*, so it can stop offering words the shard will
/// refuse; it is not the decision travelling.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum AccessLevel {
    /// An ordinary player. The default, and what a missing or unparseable
    /// configuration falls back to — authority is never granted by accident.
    #[default]
    Player,
    /// May run the world-shaping commands: spawn, teleport, set a stat.
    GameMaster,
    /// Everything a game master may do. A seam for account or shard commands that
    /// a game master should not — kept distinct now so adding them later is not a
    /// migration.
    Administrator,
}

impl AccessLevel {
    /// Whether this level clears `required` — the whole of the gate.
    pub fn allows(self, required: AccessLevel) -> bool {
        self >= required
    }

    /// The byte this level rides as in [`AuthorityNotice`].
    ///
    /// Written out rather than derived from the discriminant: the order of the
    /// variants is a *gate*, and a wire format that followed it would turn
    /// inserting a level between two into a protocol change nobody noticed.
    #[must_use]
    pub const fn wire(self) -> u8 {
        match self {
            Self::Player => 0,
            Self::GameMaster => 1,
            Self::Administrator => 2,
        }
    }

    /// The level a byte names, or `None` for one this build has never heard of.
    ///
    /// `None` and not [`Player`](Self::Player): a client that silently read an
    /// unknown level as the lowest would quietly lose a shard's new tier, and
    /// this comes off the wire, where nothing is an invariant.
    #[must_use]
    pub const fn from_wire(byte: u8) -> Option<Self> {
        Some(match byte {
            0 => Self::Player,
            1 => Self::GameMaster,
            2 => Self::Administrator,
            _ => return None,
        })
    }
}

/// The first `0xBF` subcommand this engine invented, and where every other one
/// it invents will live.
///
/// Every subcommand a real client version has ever spoken is at or below
/// `0x2B`, and ClassicUO's own private one is `0xBEEF` — so a range up here is
/// out of reach of both, and a stock client, which reads `0xBF`'s length out of
/// the envelope and dispatches on the subcommand, skips what it does not know
/// rather than losing the stream. That is what makes an extension safe to send
/// to *every* client instead of only to ours.
pub const OPENSHARD_SUBCOMMANDS: u16 = 0xE000;

/// `0xBF` subcommand `0xE001` — "this is the authority I hold you at".
///
/// # Why a packet exists at all
///
/// Authority is the world's and is checked where a command runs, so nothing
/// about *enforcement* needs this. What needs it is the client's completer: it
/// offers `openshard_commands::StaffCommand`'s whole vocabulary as a `.` line is
/// typed, and with no idea who is typing it offered all twenty-five to
/// everybody. A word the shard refuses reads exactly like a mistyped one, which
/// is a client teaching a player verbs that do nothing.
///
/// # Why it is ours and not the reference's
///
/// No client version carries this: in the reference protocol a player learns
/// their authority by trying something. So it is an invention, in the range
/// [`OPENSHARD_SUBCOMMANDS`] — see there for why sending it to a stock client is
/// safe — and it is the *only* kind of invention worth making, one where a
/// client that ignores it is simply a client with a less helpful completer.
///
/// Sent once, on world entry, because the level is the account's and does not
/// move while a character is in the world: `.gm` toggles the staff *mode*
/// (`WorldState::is_staff`, the exemptions), never the authority
/// (`WorldState::staff_authority`, which is what this reports).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AuthorityNotice {
    /// What the shard holds this connection's character at.
    pub level: AccessLevel,
}

impl AuthorityNotice {
    /// The packet id — the extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// Which `0xBF` this is.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 1;
    /// The whole framed packet, id and length included: id, length, subcommand,
    /// one byte of level.
    pub const LENGTH_BYTES: u8 = 6;

    /// Encode the whole packet.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        crate::packet::encode_packet(&self, ClientVersion::new(4, 0, 0, 0))
    }
}

/// Fixed despite living under `0xBF`, [`crate::design::DesignRevision`]'s reason
/// exactly: the body never varies, so the constant is written by hand because
/// `frame_body` only back-patches a length for [`PacketLength::Variable`].
impl EncodePacket for AuthorityNotice {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Fixed(Self::LENGTH_BYTES as u16);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(u16::from(Self::LENGTH_BYTES));
        out.u16(Self::SUBCOMMAND);
        out.u8(self.level.wire());
    }
}

impl DecodePacket for AuthorityNotice {
    const ID: u8 = 0xBF;

    /// The reader is past the length, so the body starts at the subcommand.
    /// Refuses a different one rather than reading its body as this one.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for an authority notice",
                value: u32::from(subcommand),
            });
        }
        let byte = reader.u8()?;
        let Some(level) = AccessLevel::from_wire(byte) else {
            return Err(DecodeError::UnknownValue {
                field: "access level",
                value: u32::from(byte),
            });
        };
        Ok(Self { level })
    }
}

impl fmt::Display for AccessLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Player => "player",
            Self::GameMaster => "gamemaster",
            Self::Administrator => "administrator",
        };
        f.write_str(name)
    }
}

/// A configured access level that names no level this build knows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnknownAccessLevel(pub String);

impl fmt::Display for UnknownAccessLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown access level {:?}", self.0)
    }
}

impl std::error::Error for UnknownAccessLevel {
}

impl FromStr for AccessLevel {
    type Err = UnknownAccessLevel;

    /// Parse a configured name, case-insensitively, with the abbreviations a
    /// human actually types. Unknown is an error the caller reports rather than a
    /// silent grant — the safe direction to be wrong in.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_lowercase().as_str() {
            "player" | "" => Ok(Self::Player),
            "gamemaster" | "gm" | "game master" => Ok(Self::GameMaster),
            "administrator" | "admin" => Ok(Self::Administrator),
            other => Err(UnknownAccessLevel(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packet, both ways, through the same framing every other one rides.
    #[test]
    fn an_authority_notice_survives_the_wire() {
        for level in [
            AccessLevel::Player,
            AccessLevel::GameMaster,
            AccessLevel::Administrator,
        ] {
            let bytes = AuthorityNotice { level }.encode();
            assert_eq!(bytes.len(), usize::from(AuthorityNotice::LENGTH_BYTES));
            assert_eq!(bytes[0], 0xBF, "the extended-command envelope");
            assert_eq!(
                u16::from_be_bytes([bytes[3], bytes[4]]),
                AuthorityNotice::SUBCOMMAND
            );

            let decoded = crate::server_packet::ServerPacket::decode(&bytes, ClientVersion::new(4, 0, 0, 0))
                .expect("our own bytes decode");
            assert_eq!(
                decoded,
                Some(crate::server_packet::ServerPacket::AuthorityNotice(
                    AuthorityNotice { level }
                ))
            );
        }
    }

    /// A level this build has never heard of is an error and not a quiet
    /// demotion: the byte came off the wire, where nothing is an invariant, and
    /// a client that read an unknown tier as `player` would silently lose it.
    #[test]
    fn an_unknown_level_byte_is_refused() {
        assert_eq!(AccessLevel::from_wire(3), None);
        let mut bytes = AuthorityNotice {
            level: AccessLevel::Administrator,
        }
        .encode();
        *bytes.last_mut().expect("a body") = 3;
        assert!(crate::server_packet::ServerPacket::decode(&bytes, ClientVersion::new(4, 0, 0, 0)).is_err());
    }

    /// The subcommand is out of reach of every client version's own — all of
    /// which are at or below `0x2B` — and of ClassicUO's private `0xBEEF`.
    /// A collision would be this engine answering for somebody else's packet.
    #[test]
    fn the_invented_subcommand_is_nobody_elses() {
        const { assert!(AuthorityNotice::SUBCOMMAND > 0x2B) };
        const { assert!(AuthorityNotice::SUBCOMMAND != 0xBEEF) };
        const { assert!(AuthorityNotice::SUBCOMMAND >= OPENSHARD_SUBCOMMANDS) };
    }

    #[test]
    fn the_levels_are_ordered_so_a_gate_is_a_comparison() {
        assert!(AccessLevel::GameMaster > AccessLevel::Player);
        assert!(AccessLevel::Administrator > AccessLevel::GameMaster);
        assert!(AccessLevel::Administrator.allows(AccessLevel::GameMaster));
        assert!(!AccessLevel::Player.allows(AccessLevel::GameMaster));
        assert!(AccessLevel::GameMaster.allows(AccessLevel::GameMaster));
    }

    #[test]
    fn the_default_is_no_authority() {
        assert_eq!(AccessLevel::default(), AccessLevel::Player);
    }

    #[test]
    fn names_parse_case_insensitively_with_abbreviations() {
        assert_eq!("player".parse(), Ok(AccessLevel::Player));
        assert_eq!("".parse(), Ok(AccessLevel::Player));
        assert_eq!("GM".parse(), Ok(AccessLevel::GameMaster));
        assert_eq!("GameMaster".parse(), Ok(AccessLevel::GameMaster));
        assert_eq!("  admin ".parse(), Ok(AccessLevel::Administrator));
    }

    #[test]
    fn an_unknown_name_is_an_error_not_a_grant() {
        assert_eq!(
            "wizard".parse::<AccessLevel>(),
            Err(UnknownAccessLevel("wizard".to_owned()))
        );
    }

    #[test]
    fn display_round_trips_through_parse() {
        for level in [
            AccessLevel::Player,
            AccessLevel::GameMaster,
            AccessLevel::Administrator,
        ] {
            assert_eq!(level.to_string().parse(), Ok(level));
        }
    }
}
