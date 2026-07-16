//! The login conversation, from account name to character list.
//!
//! ```text
//!   client                                server
//!     │  seed (see crate::SeedReader)       │
//!     │────────────────────────────────────>│
//!     │  0x80 account login                 │
//!     │────────────────────────────────────>│
//!     │              0xA8 shard list        │   or 0x82 denied
//!     │<────────────────────────────────────│
//!     │  0xA0 select shard                  │
//!     │────────────────────────────────────>│
//!     │              0x8C relay             │
//!     │<────────────────────────────────────│
//!  ── reconnect to the game server ─────────────────────────────
//!     │  seed again, then 0x91 game login   │
//!     │────────────────────────────────────>│
//!     │              0xA9 character list    │   or 0x82 denied
//!     │<────────────────────────────────────│
//! ```
//!
//! Layouts are ported from SphereServer's `network/send.cpp` and `receive.cpp`.
//!
//! # Field widths are not padding
//!
//! Names and passwords sit in fixed 30-byte fields. The client reads exactly 30
//! bytes and does not care what the server meant, so a field that is one byte
//! wrong desynchronises everything after it in the packet — usually presenting
//! as a client that silently shows an empty character list.

use std::fmt;
use std::net::Ipv4Addr;

use crate::codec::{CodecError, PacketReader, PacketWriter};
use crate::feature::Feature;
use crate::version::ClientVersion;

/// Width of an account name field. Sphere's `MAX_ACCOUNT_NAME_SIZE`.
pub const ACCOUNT_NAME_LENGTH: usize = 30;
/// Width of a password field. Sphere's `MAX_NAME_SIZE`.
pub const PASSWORD_LENGTH: usize = 30;
/// Width of a character name field.
pub const CHARACTER_NAME_LENGTH: usize = 30;
/// Width of a shard name field in the 0xA8 list.
pub const SHARD_NAME_LENGTH: usize = 32;

/// A packet did not have the id it was decoded as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WrongPacket {
    /// The id the decoder wanted.
    pub expected: u8,
    /// The id the packet actually had.
    pub found: u8,
}

impl fmt::Display for WrongPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "expected packet 0x{:02X}, found 0x{:02X}",
            self.expected, self.found
        )
    }
}

impl std::error::Error for WrongPacket {}

/// Decoding a login packet failed.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum LoginDecodeError {
    /// The packet was not the one expected.
    WrongPacket(WrongPacket),
    /// The body was malformed.
    Codec(CodecError),
}

impl From<CodecError> for LoginDecodeError {
    fn from(error: CodecError) -> Self {
        Self::Codec(error)
    }
}

impl fmt::Display for LoginDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongPacket(error) => error.fmt(f),
            Self::Codec(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for LoginDecodeError {}

/// Check and strip the id byte.
fn expect_id(bytes: &[u8], expected: u8) -> Result<PacketReader<'_>, LoginDecodeError> {
    let mut reader = PacketReader::new(bytes);
    let found = reader.u8()?;
    if found == expected {
        Ok(reader)
    } else {
        Err(LoginDecodeError::WrongPacket(WrongPacket {
            expected,
            found,
        }))
    }
}

// -- 0x80 account login ---------------------------------------------------

/// `0x80` — the client offers an account name and password. 62 bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AccountLogin {
    /// The account name, as typed.
    pub account: String,
    /// The password, in plaintext.
    ///
    /// The UO protocol has no password hashing: it is plaintext inside the
    /// login encryption, and the login encryption is trivially broken. Treat
    /// this as public, never log it, and hash it before it reaches storage.
    pub password: String,
}

impl AccountLogin {
    /// The packet id.
    pub const ID: u8 = 0x80;

    /// Decode a whole 0x80 packet, id included.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let account = reader.fixed_string(ACCOUNT_NAME_LENGTH)?;
        let password = reader.fixed_string(PASSWORD_LENGTH)?;
        // Sphere: "NextLoginKey value from uo.cfg on client machine" — the
        // server has no use for it.
        reader.skip(1)?;
        Ok(Self { account, password })
    }

    /// Encode a whole 0x80 packet. Mostly for tests and for a client.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(62);
        writer.u8(Self::ID);
        writer.fixed_string(&self.account, ACCOUNT_NAME_LENGTH);
        writer.fixed_string(&self.password, PASSWORD_LENGTH);
        writer.u8(0);
        writer.into_bytes()
    }
}

// -- 0x82 login denied ----------------------------------------------------

/// Why a login was refused.
///
/// # Only five of these reach the client
///
/// The client understands exactly five codes. Everything else a server might
/// want to say — bad auth id, too many characters, IP blocked, rate limited —
/// has no wire representation and must collapse into one of the five.
///
/// Sphere keeps both sets in one enum and relies on callers to translate.
/// Splitting them means the compiler does it: a [`DenyReason`] is anything the
/// server can decide, and [`DenyReason::wire_code`] is the total function that
/// maps it to what the client can hear.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum DenyReason {
    /// No such account.
    NoAccount,
    /// The account is already logged in.
    InUse,
    /// The account is blocked or banned.
    Blocked,
    /// Wrong password.
    BadPassword,
    /// Anything else: timeout, internal error.
    Other,
    /// The client version is not allowed on this shard.
    BadVersion,
    /// The selected character does not exist.
    BadCharacter,
    /// The auth id from 0x8C did not match.
    BadAuthId,
    /// The account name is malformed.
    MalformedAccount,
    /// The password is malformed.
    MalformedPassword,
    /// The character is already in the world.
    CharacterInUse,
    /// The account holds too many characters.
    TooManyCharacters,
    /// The connecting IP is blocked.
    BlockedIp,
    /// The shard is full.
    ShardFull,
    /// Too many password attempts.
    TooManyAttempts,
}

impl DenyReason {
    /// The byte the client actually understands.
    ///
    /// Reasons with no wire code of their own collapse to the nearest of the
    /// five the client knows. That collapse loses information *deliberately*:
    /// telling an attacker apart "no such account" from "wrong password" is a
    /// user-enumeration oracle, and the client has no way to show the
    /// difference anyway. Log the real reason server-side.
    pub const fn wire_code(self) -> u8 {
        match self {
            Self::NoAccount => 0x00,
            Self::InUse | Self::CharacterInUse => 0x01,
            Self::Blocked | Self::BlockedIp | Self::TooManyAttempts => 0x02,
            Self::BadPassword | Self::MalformedPassword => 0x03,
            Self::Other
            | Self::BadVersion
            | Self::BadCharacter
            | Self::BadAuthId
            | Self::MalformedAccount
            | Self::TooManyCharacters
            | Self::ShardFull => 0x04,
        }
    }
}

/// `0x82` — refuse a login. 2 bytes.
pub fn encode_login_denied(reason: DenyReason) -> Vec<u8> {
    vec![0x82, reason.wire_code()]
}

// -- 0xA8 shard list ------------------------------------------------------

/// One shard in the 0xA8 list.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ShardEntry {
    /// Shard name. Truncated to 32 bytes on the wire.
    pub name: String,
    /// How full, 0–100. The client renders anything above 100 as garbage.
    pub percent_full: u8,
    /// Timezone, as the client's own oddity: hours west of GMT.
    pub timezone: u8,
    /// Where to reach it.
    pub address: Ipv4Addr,
}

/// The client refuses to render more than this many shards, and crashes on more.
///
/// Sphere caps at the same number with the comment "too many servers in list can
/// crash the client".
pub const MAX_SHARDS: usize = 32;

/// `0xA8` — the shard list.
///
/// `version` decides the IP byte order: clients before 4.0.0 want it reversed.
/// Sphere spells this `MAXCLIVER_REVERSEIP`; here it is
/// [`Feature::ForwardShardIp`].
///
/// Entries past [`MAX_SHARDS`] are dropped rather than sent, because sending
/// them crashes the client.
pub fn encode_shard_list(shards: &[ShardEntry], version: ClientVersion) -> Vec<u8> {
    let shards = &shards[..shards.len().min(MAX_SHARDS)];
    let forward_ip = version.supports(Feature::ForwardShardIp);

    let mut writer = PacketWriter::with_capacity(6 + shards.len() * 40);
    writer.u8(0xA8);
    writer.u16(0); // length, patched below
    writer.u8(0xFF); // system info flag; Sphere sends 0xFF unconditionally
    writer.u16(shards.len() as u16);

    for (index, shard) in shards.iter().enumerate() {
        // The client indexes shards from zero in 0xA0, but Sphere numbers the
        // list from one here and subtracts on the way back.
        writer.u16((index + 1) as u16);
        writer.fixed_string(&shard.name, SHARD_NAME_LENGTH);
        writer.u8(shard.percent_full.min(100));
        writer.u8(shard.timezone);

        let octets = shard.address.octets();
        if forward_ip {
            writer.bytes(&octets);
        } else {
            writer.bytes(&[octets[3], octets[2], octets[1], octets[0]]);
        }
    }

    patch_length(writer.into_bytes())
}

/// Write the final length into a variable-length packet's `u16` at offset 1.
fn patch_length(mut bytes: Vec<u8>) -> Vec<u8> {
    let length = u16::try_from(bytes.len()).expect("packet outgrew its u16 length field");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

// -- 0xA0 select shard ----------------------------------------------------

/// `0xA0` — the client picks a shard. 3 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SelectShard {
    /// The index the client chose, as sent — one-based, matching 0xA8.
    pub index: u16,
}

impl SelectShard {
    /// The packet id.
    pub const ID: u8 = 0xA0;

    /// Decode a whole 0xA0 packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        Ok(Self {
            index: reader.u16()?,
        })
    }

    /// The zero-based index into the list that was sent.
    ///
    /// Returns `None` for index 0, which is out of range: this is untrusted
    /// input and a naive `index - 1` underflows.
    pub const fn slot(self) -> Option<usize> {
        if self.index == 0 {
            None
        } else {
            Some(self.index as usize - 1)
        }
    }

    /// Encode a whole 0xA0 packet.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(3);
        writer.u8(Self::ID);
        writer.u16(self.index);
        writer.into_bytes()
    }
}

// -- 0x8C relay -----------------------------------------------------------

/// `0x8C` — go connect to the game server. 11 bytes.
///
/// The IP here is **always** reversed, on every client version — unlike 0xA8,
/// which only reverses below 4.0.0. Sphere writes it unconditionally in
/// `PacketServerRelay`. There is no reason for the asymmetry; it is simply what
/// the client does.
pub fn encode_relay(address: Ipv4Addr, port: u16, auth_key: u32) -> Vec<u8> {
    let octets = address.octets();
    let mut writer = PacketWriter::with_capacity(11);
    writer.u8(0x8C);
    writer.bytes(&[octets[3], octets[2], octets[1], octets[0]]);
    writer.u16(port);
    writer.u32(auth_key);
    writer.into_bytes()
}

// -- 0x91 game server login -----------------------------------------------

/// `0x91` — login to the game server after a relay. 65 bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GameServerLogin {
    /// The key handed out in the 0x8C relay. The server must check it.
    pub auth_key: u32,
    /// The account name, again.
    pub account: String,
    /// The password, again, still plaintext.
    pub password: String,
}

impl GameServerLogin {
    /// The packet id.
    pub const ID: u8 = 0x91;

    /// Decode a whole 0x91 packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        Ok(Self {
            auth_key: reader.u32()?,
            account: reader.fixed_string(ACCOUNT_NAME_LENGTH)?,
            password: reader.fixed_string(PASSWORD_LENGTH)?,
        })
    }

    /// Encode a whole 0x91 packet.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(65);
        writer.u8(Self::ID);
        writer.u32(self.auth_key);
        writer.fixed_string(&self.account, ACCOUNT_NAME_LENGTH);
        writer.fixed_string(&self.password, PASSWORD_LENGTH);
        writer.into_bytes()
    }
}

// -- 0xA9 character list --------------------------------------------------

/// One character slot in the 0xA9 list.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CharacterEntry {
    /// The character's name. Empty means an unused slot.
    pub name: String,
}

/// One starting city offered at character creation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StartLocation {
    /// The region name, e.g. "Britain".
    pub area: String,
    /// The specific spot, e.g. "Castle Britannia".
    pub name: String,
    /// Where the character appears.
    pub position: (i32, i32, i32),
    /// Which map.
    pub map: i32,
    /// Cliloc id for the description. Ignored by clients before 7.0.13.0.
    pub description_cliloc: i32,
}

/// The minimum number of character slots the list must contain.
///
/// Clients since 3.0.0.10 read a fixed five slots regardless of the count byte
/// and mis-render a shorter list. Sphere calls this `MINCLIVER_PADCHARLIST`.
pub const MIN_CHARACTER_SLOTS: usize = 5;

/// `0xA9` — the character list and starting cities.
///
/// `flags` is the client-capability mask; it is the caller's to compose, since
/// what a shard enables is configuration rather than protocol.
pub fn encode_character_list(
    characters: &[CharacterEntry],
    starts: &[StartLocation],
    flags: u32,
    version: ClientVersion,
) -> Vec<u8> {
    let slots = if version.supports(Feature::PaddedCharacterList) {
        characters.len().max(MIN_CHARACTER_SLOTS)
    } else {
        characters.len()
    };

    let mut writer = PacketWriter::with_capacity(11 + slots * 60);
    writer.u8(0xA9);
    writer.u16(0); // length, patched below
    writer.u8(slots as u8);

    for slot in 0..slots {
        let name = characters.get(slot).map_or("", |entry| entry.name.as_str());
        writer.fixed_string(name, CHARACTER_NAME_LENGTH);
        // The password field is vestigial: the client sends it back but no
        // modern server puts anything in it.
        writer.fixed_string("", PASSWORD_LENGTH);
    }

    writer.u8(starts.len().min(u8::MAX as usize) as u8);
    let extra_info = version.supports(Feature::ExtraStartInfo);
    for (index, start) in starts.iter().take(u8::MAX as usize).enumerate() {
        writer.u8(index as u8);
        if extra_info {
            // Since 7.0.13.0 the name fields are one byte wider *and* six extra
            // dwords follow. Getting the width wrong shifts everything after it.
            writer.fixed_string(&start.area, 32);
            writer.fixed_string(&start.name, 32);
            writer.i32(start.position.0);
            writer.i32(start.position.1);
            writer.i32(start.position.2);
            writer.i32(start.map);
            writer.i32(start.description_cliloc);
            writer.u32(0);
        } else {
            writer.fixed_string(&start.area, 31);
            writer.fixed_string(&start.name, 31);
        }
    }

    if version.supports(Feature::CharacterListFlags) {
        writer.u32(flags);
    }

    patch_length(writer.into_bytes())
}

// -- 0xBD client version --------------------------------------------------

/// `0xBD` — the client reports its version as a string.
///
/// Variable length: id, `u16` length, then a NUL-terminated ASCII version.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClientVersionReport {
    /// Exactly what the client sent, before parsing.
    ///
    /// Kept raw because it carries more than the version: Sphere sniffs `UO:3D`
    /// out of this string to tell the 3D client apart, and a shard may want to
    /// log or fingerprint the rest.
    pub raw: String,
}

impl ClientVersionReport {
    /// The packet id.
    pub const ID: u8 = 0xBD;

    /// Sphere clamps the version string to this before reading it.
    pub const MAX_LENGTH: usize = 20;

    /// Decode a whole 0xBD packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let declared = reader.u16()? as usize;
        // The declared length covers the id and the length field. Trusting it
        // over the real buffer would read past the packet.
        if declared < 3 || declared > bytes.len() {
            return Err(CodecError::UnexpectedEnd {
                needed: declared,
                available: bytes.len(),
            }
            .into());
        }

        let body = &bytes[3..declared];
        let end = body
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(body.len())
            .min(Self::MAX_LENGTH);
        Ok(Self {
            raw: body[..end].iter().map(|byte| *byte as char).collect(),
        })
    }

    /// Parse the reported version, if it is a version at all.
    ///
    /// `None` for a string the client made up. That is not fatal on its own —
    /// the seed usually carried a version already.
    pub fn version(&self) -> Option<ClientVersion> {
        self.raw.parse().ok()
    }

    /// Whether the client identified itself as the 3D client.
    ///
    /// Sphere looks for this substring; there is no cleaner signal.
    pub fn is_3d_client(&self) -> bool {
        self.raw.contains("UO:3D")
    }

    /// Encode a whole 0xBD packet.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(4 + self.raw.len());
        writer.u8(Self::ID);
        writer.u16(0);
        writer.null_terminated_string(&self.raw);
        patch_length(writer.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{client_packet_length, PacketLength};

    /// The `u16` a variable-length packet declares at offset 1.
    ///
    /// `frame_client_packet` is no use here: these are server-to-client packets
    /// and are deliberately absent from the client length table, because the
    /// server already knows the length of what it writes.
    fn declared_length(bytes: &[u8]) -> usize {
        u16::from_be_bytes([bytes[1], bytes[2]]) as usize
    }

    /// The shard count, which sits after the id, the length and the 0xFF flag.
    fn shard_count(bytes: &[u8]) -> usize {
        u16::from_be_bytes([bytes[4], bytes[5]]) as usize
    }

    fn shard(name: &str, address: [u8; 4]) -> ShardEntry {
        ShardEntry {
            name: name.to_owned(),
            percent_full: 10,
            timezone: 5,
            address: Ipv4Addr::from(address),
        }
    }

    #[test]
    fn account_login_round_trips_at_the_declared_length() {
        let login = AccountLogin {
            account: "youri".to_owned(),
            password: "hunter2".to_owned(),
        };
        let bytes = login.encode();

        assert_eq!(
            client_packet_length(AccountLogin::ID),
            Some(PacketLength::Fixed(62))
        );
        assert_eq!(bytes.len(), 62, "the table and the encoder must agree");
        assert_eq!(AccountLogin::decode(&bytes).unwrap(), login);
    }

    #[test]
    fn account_login_rejects_the_wrong_packet() {
        let mut bytes = AccountLogin {
            account: "a".to_owned(),
            password: "b".to_owned(),
        }
        .encode();
        bytes[0] = 0x91;
        assert_eq!(
            AccountLogin::decode(&bytes),
            Err(LoginDecodeError::WrongPacket(WrongPacket {
                expected: 0x80,
                found: 0x91,
            }))
        );
    }

    #[test]
    fn account_login_rejects_a_truncated_packet() {
        let bytes = [0x80u8, b'a', b'b'];
        assert!(matches!(
            AccountLogin::decode(&bytes),
            Err(LoginDecodeError::Codec(_))
        ));
    }

    #[test]
    fn account_login_truncates_an_overlong_name_to_its_field() {
        let login = AccountLogin {
            account: "x".repeat(50),
            password: String::new(),
        };
        assert_eq!(login.encode().len(), 62, "a long name must not overrun");
        assert_eq!(
            AccountLogin::decode(&login.encode()).unwrap().account.len(),
            30
        );
    }

    #[test]
    fn deny_reasons_collapse_onto_the_five_the_client_knows() {
        for reason in [
            DenyReason::NoAccount,
            DenyReason::InUse,
            DenyReason::Blocked,
            DenyReason::BadPassword,
            DenyReason::Other,
            DenyReason::BadVersion,
            DenyReason::BadCharacter,
            DenyReason::BadAuthId,
            DenyReason::MalformedAccount,
            DenyReason::MalformedPassword,
            DenyReason::CharacterInUse,
            DenyReason::TooManyCharacters,
            DenyReason::BlockedIp,
            DenyReason::ShardFull,
            DenyReason::TooManyAttempts,
        ] {
            let code = reason.wire_code();
            assert!(
                code <= 0x04,
                "{reason:?} sends 0x{code:02X}, which the client cannot read"
            );
        }

        // Spot-check the collapse, which is the part that loses information.
        assert_eq!(
            DenyReason::BlockedIp.wire_code(),
            0x02,
            "reads as 'blocked'"
        );
        assert_eq!(DenyReason::ShardFull.wire_code(), 0x04, "reads as 'other'");
    }

    #[test]
    fn login_denied_matches_the_declared_length() {
        let bytes = encode_login_denied(DenyReason::BadPassword);
        assert_eq!(bytes, vec![0x82, 0x03]);
        assert_eq!(bytes.len(), 2);
    }

    #[test]
    fn shard_list_frames_and_declares_its_own_length() {
        let shards = [shard("Britannia", [10, 0, 0, 1])];
        let bytes = encode_shard_list(&shards, ClientVersion::TOL);

        assert_eq!(bytes.len(), 46, "Sphere's PacketServerList base length");
        assert_eq!(
            declared_length(&bytes),
            46,
            "the declared length must match reality"
        );
        assert_eq!(bytes[3], 0xFF, "system info flag");
        assert_eq!(shard_count(&bytes), 1);
        assert_eq!(bytes[6..8], [0x00, 0x01], "list is numbered from one");
    }

    #[test]
    fn shard_list_ip_order_flips_at_4_0_0() {
        // MAXCLIVER_REVERSEIP: the one boundary Sphere states backwards.
        let shards = [shard("Britannia", [10, 0, 0, 1])];

        let modern = encode_shard_list(&shards, ClientVersion::new(4, 0, 0, 0));
        assert_eq!(&modern[42..46], &[10, 0, 0, 1], "4.0.0 sends wire order");

        let ancient = encode_shard_list(&shards, ClientVersion::new(3, 255, 255, 255));
        assert_eq!(
            &ancient[42..46],
            &[1, 0, 0, 10],
            "one patch below, reversed"
        );
    }

    #[test]
    fn shard_list_drops_entries_past_the_client_crash_point() {
        let shards: Vec<_> = (0..40)
            .map(|i| shard(&format!("s{i}"), [1, 2, 3, 4]))
            .collect();
        let bytes = encode_shard_list(&shards, ClientVersion::TOL);
        assert_eq!(
            shard_count(&bytes),
            MAX_SHARDS,
            "more than 32 crashes the client, so they are dropped not sent"
        );
    }

    #[test]
    fn shard_list_clamps_a_nonsense_fullness() {
        let mut entry = shard("Britannia", [10, 0, 0, 1]);
        entry.percent_full = 250;
        let bytes = encode_shard_list(&[entry], ClientVersion::TOL);
        assert_eq!(bytes[40], 100, "the client renders >100 as garbage");
    }

    #[test]
    fn select_shard_round_trips() {
        let select = SelectShard { index: 1 };
        let bytes = select.encode();
        assert_eq!(bytes.len(), 3);
        assert_eq!(
            client_packet_length(SelectShard::ID),
            Some(PacketLength::Fixed(3))
        );
        assert_eq!(SelectShard::decode(&bytes).unwrap(), select);
        assert_eq!(select.slot(), Some(0), "the wire is one-based");
    }

    #[test]
    fn select_shard_zero_does_not_underflow() {
        // Untrusted input: `index - 1` on a u16 zero would wrap to 65535 and
        // index far out of the shard list.
        assert_eq!(SelectShard { index: 0 }.slot(), None);
    }

    #[test]
    fn relay_always_reverses_the_ip() {
        // Unlike 0xA8, this one does not depend on the client version at all.
        let bytes = encode_relay(Ipv4Addr::new(10, 0, 0, 1), 2593, 0xDEAD_BEEF);
        assert_eq!(bytes.len(), 11);
        assert_eq!(&bytes[1..5], &[1, 0, 0, 10]);
        assert_eq!(
            &bytes[5..7],
            &2593u16.to_be_bytes(),
            "the port is not reversed"
        );
        assert_eq!(&bytes[7..11], &0xDEAD_BEEFu32.to_be_bytes());
    }

    #[test]
    fn game_server_login_round_trips_at_the_declared_length() {
        let login = GameServerLogin {
            auth_key: 0x1234_5678,
            account: "youri".to_owned(),
            password: "hunter2".to_owned(),
        };
        let bytes = login.encode();
        assert_eq!(
            client_packet_length(GameServerLogin::ID),
            Some(PacketLength::Fixed(65))
        );
        assert_eq!(bytes.len(), 65);
        assert_eq!(GameServerLogin::decode(&bytes).unwrap(), login);
    }

    #[test]
    fn character_list_pads_to_five_slots() {
        // Clients since 3.0.0.10 read five slots whatever the count byte says.
        let characters = [CharacterEntry {
            name: "Lord British".to_owned(),
        }];
        let bytes = encode_character_list(&characters, &[], 0, ClientVersion::TOL);

        assert_eq!(bytes[3], 5, "one character still means five slots");
        assert_eq!(&bytes[4..16], b"Lord British");
        assert_eq!(&bytes[64..76], &[0u8; 12], "slot two is blank, not absent");
        assert_eq!(declared_length(&bytes), bytes.len());
    }

    #[test]
    fn character_list_does_not_pad_for_clients_that_predate_the_rule() {
        let characters = [CharacterEntry {
            name: "Lord British".to_owned(),
        }];
        let old = ClientVersion::new(3, 0, 0, 9);
        assert!(!old.supports(Feature::PaddedCharacterList));

        let bytes = encode_character_list(&characters, &[], 0, old);
        assert_eq!(bytes[3], 1);
    }

    #[test]
    fn character_list_start_locations_widen_at_7_0_13_0() {
        let starts = [StartLocation {
            area: "Britain".to_owned(),
            name: "Castle Britannia".to_owned(),
            position: (1475, 1774, 0),
            map: 0,
            description_cliloc: 1075072,
        }];

        let modern = encode_character_list(&[], &starts, 0, ClientVersion::new(7, 0, 13, 0));
        let ancient = encode_character_list(&[], &starts, 0, ClientVersion::new(7, 0, 12, 255));
        assert_eq!(
            modern.len() - ancient.len(),
            (1 + 32 + 32 + 24) - (1 + 31 + 31),
            "extra start info is two wider fields plus six dwords"
        );
    }

    #[test]
    fn character_list_omits_flags_for_the_oldest_clients() {
        // Straddle the boundary exactly. A wider gap would also move the
        // character-slot padding, which is a different gate entirely.
        let with_flags =
            encode_character_list(&[], &[], 0xAABB_CCDD, ClientVersion::new(1, 26, 0, 1));
        let without = encode_character_list(&[], &[], 0xAABB_CCDD, ClientVersion::new(1, 26, 0, 0));
        assert_eq!(
            with_flags.len() - without.len(),
            4,
            "send.cpp gates the flags dword on version > 1.26.0.0"
        );
        assert_eq!(
            &with_flags[with_flags.len() - 4..],
            &0xAABB_CCDDu32.to_be_bytes()
        );
    }

    #[test]
    fn client_version_report_round_trips() {
        let report = ClientVersionReport {
            raw: "7.0.45.65".to_owned(),
        };
        let bytes = report.encode();
        assert_eq!(
            client_packet_length(ClientVersionReport::ID),
            Some(PacketLength::Variable)
        );
        assert_eq!(declared_length(&bytes), bytes.len());

        let decoded = ClientVersionReport::decode(&bytes).unwrap();
        assert_eq!(decoded, report);
        assert_eq!(decoded.version(), Some(ClientVersion::new(7, 0, 45, 65)));
        assert!(!decoded.is_3d_client());
    }

    #[test]
    fn client_version_report_spots_the_3d_client() {
        let report = ClientVersionReport {
            raw: "4.0.0a, UO:3D".to_owned(),
        };
        let decoded = ClientVersionReport::decode(&report.encode()).unwrap();
        assert!(decoded.is_3d_client());
    }

    #[test]
    fn client_version_report_survives_junk() {
        // The version is a claim from the network; garbage must not be fatal.
        let report = ClientVersionReport {
            raw: "not a version".to_owned(),
        };
        let decoded = ClientVersionReport::decode(&report.encode()).unwrap();
        assert_eq!(decoded.version(), None);
    }

    #[test]
    fn client_version_report_will_not_read_past_its_packet() {
        // A declared length longer than the buffer is the classic overread.
        let bytes = [0xBD, 0xFF, 0xFF, b'7', 0x00];
        assert!(matches!(
            ClientVersionReport::decode(&bytes),
            Err(LoginDecodeError::Codec(_))
        ));

        let too_short = [0xBD, 0x00, 0x00];
        assert!(ClientVersionReport::decode(&too_short).is_err());
    }

    #[test]
    fn client_version_report_clamps_a_long_string() {
        let report = ClientVersionReport {
            raw: "9".repeat(80),
        };
        let decoded = ClientVersionReport::decode(&report.encode()).unwrap();
        assert_eq!(
            decoded.raw.len(),
            ClientVersionReport::MAX_LENGTH,
            "Sphere clamps to 20 before reading"
        );
    }
}
