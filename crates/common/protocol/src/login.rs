//! The login conversation, from account name to character list.
//!
//! ```text
//!   client                                server
//!     │  seed (see crate::seed::SeedReader) │
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
//!
//! # Client-to-server payloads keep a plain `encode()`
//!
//! [`AccountLogin`], [`SelectShard`], [`GameServerLogin`] and
//! [`ClientVersionReport`] only ever arrive over the wire — this server never
//! sends one, so `encode()` is a plain inherent method rather than
//! [`EncodePacket`]: that trait is for the packets this server actually sends,
//! where [`crate::server_packet::ServerPacket`] is the only thing allowed to
//! call it. `AccountLogin`, `SelectShard` and `GameServerLogin` are what
//! `crates/client/net`'s login state machine (`session.rs`) sends for real now;
//! only [`ClientVersionReport::encode`] is still test-fixtures only, waiting on
//! the client announcing its own version.

use std::fmt;
use std::net::{
    Ipv4Addr,
    SocketAddrV4,
};

use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::feature::Feature;
use crate::identity::{
    CharacterName,
    RawAccountName,
    RawPlaintextPassword,
};
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
    decode_packet,
};
use crate::version::ClientVersion;
use crate::wire::{
    AuthKey,
    ClilocId,
    RawCharacterSlot,
};
use crate::world::{
    CharacterPlay,
    CreateCharacter,
    Facet,
    Point,
};

/// Width of an account name field. Sphere's `MAX_ACCOUNT_NAME_SIZE`.
pub const ACCOUNT_NAME_LENGTH: usize = 30;
/// Width of a password field. Sphere's `MAX_NAME_SIZE`.
pub const PASSWORD_LENGTH: usize = 30;
/// Width of a character name field.
pub const CHARACTER_NAME_LENGTH: usize = 30;
/// Width of a shard name field in the 0xA8 list.
pub const SHARD_NAME_LENGTH: usize = 32;

// -- 0x80 account login ---------------------------------------------------

/// `0x80` — the client offers an account name and password. 62 bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AccountLogin {
    /// The account name, as typed.
    pub account:  RawAccountName,
    /// The password, in plaintext.
    ///
    /// The UO protocol has no password hashing: it is plaintext inside the
    /// login encryption, and the login encryption is trivially broken. Treat
    /// this as public, never log it, and hash it before it reaches storage.
    pub password: RawPlaintextPassword,
}

impl DecodePacket for AccountLogin {
    const ID: u8 = 0x80;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let account = RawAccountName(reader.fixed_string(ACCOUNT_NAME_LENGTH)?);
        let password = RawPlaintextPassword(reader.fixed_string(PASSWORD_LENGTH)?);
        // Sphere: "NextLoginKey value from uo.cfg on client machine" — the
        // server has no use for it.
        reader.skip(1)?;
        Ok(Self { account, password })
    }
}

impl AccountLogin {
    /// Encode a whole 0x80 packet. What `crates/client/net`'s login state
    /// machine sends for real — see the module docs.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(62);
        writer.u8(Self::ID);
        writer.fixed_string(&self.account.0, ACCOUNT_NAME_LENGTH);
        writer.fixed_string(&self.password.0, PASSWORD_LENGTH);
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

    /// Read a wire code back, as a client must.
    ///
    /// Not the inverse of [`wire_code`](Self::wire_code) and cannot be: that
    /// function is deliberately many-to-one, so what comes back is the one
    /// reason of each group the client can actually distinguish. A client that
    /// wants the real reason has to be told it some other way — there is no
    /// wire form for it.
    #[must_use]
    pub const fn from_wire_code(code: u8) -> Option<Self> {
        match code {
            0x00 => Some(Self::NoAccount),
            0x01 => Some(Self::InUse),
            0x02 => Some(Self::Blocked),
            0x03 => Some(Self::BadPassword),
            0x04 => Some(Self::Other),
            _ => None,
        }
    }
}

/// `0x82` — refuse a login. 2 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LoginDenied {
    /// Why, collapsed to what the client can hear.
    pub reason: DenyReason,
}

impl EncodePacket for LoginDenied {
    const ID: u8 = 0x82;
    const LENGTH: PacketLength = PacketLength::Fixed(2);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(self.reason.wire_code());
    }
}

impl DecodePacket for LoginDenied {
    const ID: u8 = 0x82;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let code = reader.u8()?;
        DenyReason::from_wire_code(code)
            .map(|reason| Self { reason })
            .ok_or(DecodeError::UnknownValue {
                field: "0x82 deny code",
                value: u32::from(code),
            })
    }
}

// -- 0xA8 shard list ------------------------------------------------------

/// How full a shard is, as a percentage the client will render: 0 to 100.
///
/// The client draws anything above 100 as garbage, so 100 is a *protocol*
/// ceiling and not a matter of taste — which is why this is a type with a
/// private field rather than a `u8` the encoder repairs on its way out. The
/// encoder used to hold the byte down with `.min(100)`, applying the client's
/// rule at the last possible moment instead of where the number is chosen;
/// with the invariant on the type there is nothing left for the encoder to
/// check, and an operator reporting real fullness cannot route around it.
///
/// Clamped rather than refused, because every source of the number — an
/// operator's config, a population count over a cap that may itself have
/// changed — is a quantity that is *meant* to saturate at "full", and refusing
/// would mean a shard vanishing from the list over a rounding error. That is
/// the opposite trade from [`RawShardIndex::validate`], where a wrong number
/// names a different shard.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PercentFull(u8);

impl PercentFull {
    /// Nobody on: the value a shard advertises until it counts its players.
    pub const EMPTY: Self = Self(0);

    /// The largest value the client renders as a percentage.
    pub const FULL: Self = Self(100);

    /// This many percent, held to the client's ceiling.
    #[must_use]
    pub const fn clamped(percent: u8) -> Self {
        Self(if percent > 100 { 100 } else { percent })
    }

    /// The byte to write, already inside the client's range by construction.
    #[must_use]
    pub const fn raw(self) -> u8 {
        self.0
    }
}

/// One shard in the 0xA8 list.
///
/// # Why `timezone` stays a bare integer
///
/// It is the case N2 amendment 3 settled for the status bar's numbers: a
/// quantity, not an id, with no protocol rule about its range and only one
/// place it is ever written — a struct literal that names it. See the
/// allowlist in `docs/protocol/design_wire_types.md`. `percent_full` looked like the
/// same case and is not: 100 is a ceiling the *client* imposes, so the rule
/// lives in [`PercentFull`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ShardEntry {
    /// Shard name. Truncated to 32 bytes on the wire.
    pub name:         String,
    /// How full, 0–100.
    pub percent_full: PercentFull,
    /// Timezone, as the client's own oddity: hours west of GMT.
    pub timezone:     u8,
    /// Where to reach it.
    pub address:      Ipv4Addr,
}

/// The client refuses to render more than this many shards, and crashes on more.
///
/// Sphere caps at the same number with the comment "too many servers in list can
/// crash the client".
pub const MAX_SHARDS: usize = 32;

/// `0xA8` — the shard list.
///
/// # The address goes in backwards, and that is correct
///
/// A shard at 192.168.11.6 is sent to a modern client as `06 0B A8 C0` — the
/// octets reversed. Clients before 4.0.0 get `C0 A8 0B 06` instead.
///
/// This is the opposite of [`Relay`], which always sends the octets in order.
/// Two packets, two conventions, in the same conversation, about the same
/// address. There is no reason for it; it is simply what the client does, and
/// both SphereServer and ServUO encode it exactly this way.
///
/// **Do not "fix" this by reading Sphere's comments.** `send.cpp` labels the
/// branch that emits `C0 A8 0B 06` as sending the IP "in reverse", because it
/// reverses the *dword*. The dword is `s_addr`, which is already network byte
/// order, so reversing it un-reverses the address. The comments are the wrong
/// way round for the bytes that actually leave. The shifts are not.
///
/// Entries past [`MAX_SHARDS`] are dropped rather than sent, because sending
/// them crashes the client.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ShardList {
    /// The shards to offer. Anything past [`MAX_SHARDS`] is silently dropped.
    pub shards: Vec<ShardEntry>,
}

impl EncodePacket for ShardList {
    const ID: u8 = 0xA8;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        let shards = &self.shards[..self.shards.len().min(MAX_SHARDS)];
        let reversed_ip = version.supports(Feature::ReversedShardIp);

        out.u8(0xFF); // system info flag; Sphere sends 0xFF unconditionally
        out.u16(shards.len() as u16);

        for (index, shard) in shards.iter().enumerate() {
            // The client indexes shards from zero in 0xA0, but Sphere numbers
            // the list from one here and subtracts on the way back.
            out.u16((index + 1) as u16);
            out.fixed_string(&shard.name, SHARD_NAME_LENGTH);
            out.u8(shard.percent_full.raw());
            out.u8(shard.timezone);

            let octets = shard.address.octets();
            if reversed_ip {
                out.bytes(&[octets[3], octets[2], octets[1], octets[0]]);
            } else {
                out.bytes(&octets);
            }
        }
    }
}

impl DecodePacket for ShardList {
    const ID: u8 = 0xA8;

    /// The reverse of the encoder, byte order included — and the byte order is
    /// the whole difficulty. A client reading these octets in the wrong order
    /// dials a plausible address and simply never arrives; see the type's docs
    /// for why the reversal is right and why Sphere's comments say otherwise.
    ///
    /// The index each entry carries is not kept: it is the position in the list
    /// plus one, and a `0xA0` answers with that position — so keeping it would
    /// be storing the index of a `Vec` inside the `Vec`.
    fn decode_body(reader: &mut PacketReader<'_>, version: ClientVersion) -> Result<Self, DecodeError> {
        let reversed_ip = version.supports(Feature::ReversedShardIp);
        reader.skip(1)?; // system info flag, 0xFF
        let count = reader.u16()? as usize;
        let mut shards = Vec::with_capacity(count.min(MAX_SHARDS));
        for _ in 0..count {
            reader.skip(2)?; // the one-based index, which is the position
            let name = reader.fixed_string(SHARD_NAME_LENGTH)?;
            // A shard list is only ever decoded by a client, and a hostile or
            // buggy server can put anything in this byte. Clamping here rather
            // than refusing keeps the entry — the shard is still reachable —
            // and is the same rule the encoder no longer has to apply.
            let percent_full = PercentFull::clamped(reader.u8()?);
            let timezone = reader.u8()?;
            let octets = reader.bytes(4)?;
            let address = if reversed_ip {
                Ipv4Addr::new(octets[3], octets[2], octets[1], octets[0])
            } else {
                Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3])
            };
            shards.push(ShardEntry {
                name,
                percent_full,
                timezone,
                address,
            });
        }
        Ok(Self { shards })
    }
}

// -- 0xA0 select shard ----------------------------------------------------

/// A shard pick exactly as a client's `0xA0` sent it: one-based, matching the
/// numbering [`ShardList`] wrote, and not yet checked against the list that
/// actually went out.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawShardIndex(pub u16);

impl RawShardIndex {
    /// The shard this names, out of the `offered` the `0xA8` listed.
    ///
    /// Two ways to be wrong and they are not the same mistake, so the error
    /// says which. Zero is the wire's own impossibility — `0xA8` numbers from
    /// one — and a naive `index - 1` on a `u16` zero wraps to 65535 and reads
    /// far past the list; past the end is a client answering a list this
    /// connection never sent.
    pub const fn validate(self, offered: usize) -> Result<ShardIndex, InvalidShardIndex> {
        if self.0 == 0 {
            return Err(InvalidShardIndex::Zero);
        }
        let index = self.0 as usize - 1;
        if index < offered {
            Ok(ShardIndex(index))
        } else {
            Err(InvalidShardIndex::PastEnd {
                index: self.0,
                offered,
            })
        }
    }
}

/// A shard the list actually offered: an index into it, counted from zero.
///
/// The one-based wire form is undone here and nowhere else, which is the point
/// of the type — a `usize` because indexing the list is all it is ever for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ShardIndex(pub usize);

/// A `0xA0` picked a shard the `0xA8` did not offer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum InvalidShardIndex {
    /// Index `0`, which the one-based wire numbering never produces.
    Zero,
    /// Past the end of the list that was sent.
    PastEnd {
        /// The one-based index the client sent.
        index:   u16,
        /// How many shards the list actually held.
        offered: usize,
    },
}

impl fmt::Display for InvalidShardIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => f.write_str("shard index 0, but the list is numbered from one"),
            Self::PastEnd { index, offered } => {
                write!(f, "shard {index} was picked from a list of {offered}")
            }
        }
    }
}

impl std::error::Error for InvalidShardIndex {
}

/// `0xA0` — the client picks a shard. 3 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SelectShard {
    /// The index the client chose, as sent — one-based, matching 0xA8.
    pub index: RawShardIndex,
}

impl DecodePacket for SelectShard {
    const ID: u8 = 0xA0;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            index: RawShardIndex(reader.u16()?),
        })
    }
}

impl SelectShard {
    /// Encode a whole 0xA0 packet. What `crates/client/net`'s login state
    /// machine sends for real — see the module docs.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(3);
        writer.u8(Self::ID);
        writer.u16(self.index.0);
        writer.into_bytes()
    }
}

// -- 0x8C relay -----------------------------------------------------------

/// `0x8C` — go connect to the game server. 11 bytes.
///
/// # The octets go in order, on every client version
///
/// A shard at 192.168.11.6 is sent as `C0 A8 0B 06`. Unconditionally: there is
/// no version gate here, unlike [`ShardList`], which reverses them for clients
/// from 4.0.0 on. The same address, two packets apart, in two different
/// orders. Both SphereServer and ServUO encode it exactly this way.
///
/// This is the single most expensive byte order in the file to get wrong, and it
/// is silent on this end. The client takes the relay, dials whatever it was
/// handed, and never comes back; the server sees a login, a clean disconnect,
/// and no second connection. Nothing here fails. The packet was well-formed and
/// pointed at 6.11.168.192.
///
/// It was wrong for exactly that reason once, from reading Sphere's
/// `PacketServerRelay` and seeing `writeByte((ip) & 0xFF)` written first. That
/// looks like a little-endian write of an address, and it is — of an `s_addr`,
/// which is *already* network byte order, so the low byte is the first octet.
/// The shifts undo an endianness the value never had. Trace it, do not read it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Relay {
    /// Where the game server is: address and port, which the wire carries
    /// adjacently and every caller already holds together — the login server's
    /// `game_address` is a `SocketAddrV4` that was being taken apart at this
    /// call site and put back together here. One field, so a relay cannot be
    /// built naming one shard's address and another's port.
    pub endpoint: SocketAddrV4,
    /// The key the client must present back on the game connection.
    pub auth_key: AuthKey,
}

impl EncodePacket for Relay {
    const ID: u8 = 0x8C;
    const LENGTH: PacketLength = PacketLength::Fixed(11);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.bytes(&self.endpoint.ip().octets());
        out.u16(self.endpoint.port());
        out.u32(self.auth_key.0);
    }
}

impl DecodePacket for Relay {
    const ID: u8 = 0x8C;

    /// Octets in order, on every version — the opposite of [`ShardList`], in
    /// the same conversation, about the same address.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let octets = reader.bytes(4)?;
        let address = Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]);
        let port = reader.u16()?;
        Ok(Self {
            endpoint: SocketAddrV4::new(address, port),
            auth_key: AuthKey(reader.u32()?),
        })
    }
}

// -- 0x91 game server login -----------------------------------------------

/// `0x91` — login to the game server after a relay. 65 bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GameServerLogin {
    /// The key handed out in the 0x8C relay. The server must check it.
    pub auth_key: AuthKey,
    /// The account name, again.
    pub account:  RawAccountName,
    /// The password, again, still plaintext.
    pub password: RawPlaintextPassword,
}

impl DecodePacket for GameServerLogin {
    const ID: u8 = 0x91;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            auth_key: AuthKey(reader.u32()?),
            account:  RawAccountName(reader.fixed_string(ACCOUNT_NAME_LENGTH)?),
            password: RawPlaintextPassword(reader.fixed_string(PASSWORD_LENGTH)?),
        })
    }
}

impl GameServerLogin {
    /// Encode a whole 0x91 packet. What `crates/client/net`'s login state
    /// machine sends for real — see the module docs.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(65);
        writer.u8(Self::ID);
        writer.u32(self.auth_key.0);
        writer.fixed_string(&self.account.0, ACCOUNT_NAME_LENGTH);
        writer.fixed_string(&self.password.0, PASSWORD_LENGTH);
        writer.into_bytes()
    }
}

// -- 0xA9 character list --------------------------------------------------

/// One character slot in the 0xA9 list.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CharacterEntry {
    /// The character's name. Empty means an unused slot.
    pub name: CharacterName,
}

/// One starting city offered at character creation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StartLocation {
    /// The region name, e.g. "Britain".
    pub area:               String,
    /// The specific spot, e.g. "Castle Britannia".
    pub name:               String,
    /// Where the character appears. The wire widens each coordinate to a full
    /// dword here — unlike every other position on the wire, which is
    /// [`Point`]'s own `u16`/`u16`/`i8` — but the value named is the same map
    /// coordinate, so decode narrows it back down.
    pub position:           Point,
    /// Which map.
    pub map:                Facet,
    /// Cliloc id for the description. Ignored by clients before 7.0.13.0.
    pub description_cliloc: ClilocId,
}

/// The minimum number of character slots the list must contain.
///
/// Clients since 3.0.0.10 read a fixed five slots regardless of the count byte
/// and mis-render a shorter list. Sphere calls this `MINCLIVER_PADCHARLIST`.
pub const MIN_CHARACTER_SLOTS: usize = 5;

/// The `0xA9` character-list capability mask.
///
/// # Not the `0xB9` mask, and the two are one typo apart
///
/// Login sends two capability dwords a few bytes apart, they overlap in
/// subject — both are about whether the client behaves like an AoS client —
/// and until N6 both were a bare `u32` sitting in adjacent fields of
/// `openshard_login::LoginServer`. Swapping them compiled and produced a shard
/// whose clients drew no tooltips for reasons nothing logged. They are two
/// types now: this one and [`SupportedFeatures`].
///
/// This is the mask ClassicUO actually keys its `ClientFeatures` on.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct CharacterListFlags(pub u32);

impl CharacterListFlags {
    /// Advertise nothing: a modern client stays on the classic single-click
    /// name path.
    pub const NONE: Self = Self(0);

    /// The client may open **context menus** (the `0xBF` popup). ClassicUO's
    /// `CharacterListFlags.CLF_CONTEXT_MENU`; it sets
    /// `ClientFeatures.PopupEnabled` from this bit — *this* packet, not the
    /// `0xB9`.
    pub const CONTEXT_MENU: Self = Self(0x08);

    /// The client may use **AoS object tooltips** (OPL). ClassicUO's
    /// `CLF_PALADIN_NECROMANCER_TOOLTIPS`; it sets
    /// `ClientFeatures.TooltipsEnabled` from this bit (plus its own
    /// client-version check, so the server just needs to offer it). This is
    /// what makes a modern client send `0xD6` tooltip requests at all — the
    /// flag lives in the character list, not in `0xB9`.
    pub const TOOLTIPS: Self = Self(0x20);

    /// Both masks. A named method rather than a `BitOr` impl, on N2 amendment
    /// 8's argument: an operator on a newtype is the same invisible coercion
    /// `Deref` is.
    #[must_use]
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// The `0xB9` SupportedFeatures mask: which expansion feature sets the client
/// should turn on. ServUO's `FeatureFlags`.
///
/// Distinct from [`CharacterListFlags`] — see its docs for what that confusion
/// costs — and from [`Feature`], which is this crate's question about what a
/// *version* can do. This one is a claim the shard makes about itself.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct SupportedFeatures(pub u32);

impl SupportedFeatures {
    /// Advertise nothing at all: no `0xB9` is sent.
    pub const NONE: Self = Self(0);

    /// Age of Shadows: ServUO's `T2A|UOR|UOTD|LBR|AOS` (`0x1F`). The AOS bit
    /// (`0x10`) is what makes a modern client use object tooltips and context
    /// menus; the lower expansion bits ride along as the core-expansion default
    /// and a 2D client ignores the ones it does not use. Left out is
    /// `LiveAccount` (`0x8000`), which would ask for a sixth character slot the
    /// list is not sized for.
    pub const AOS: Self = Self(0x1F);

    /// Samurai Empire: AoS plus `SE` (`0x40`). ServUO's
    /// `FeatureFlags.ExpansionSE`, again without `LiveAccount`.
    pub const SE: Self = Self(Self::AOS.0 | 0x40);

    /// Mondain's Legacy: SE plus `ML` (`0x80`) and `NinthAge` (`0x200`, the
    /// custom-house tiles ML shipped). ServUO's `FeatureFlags.ExpansionML`,
    /// without `LiveAccount`.
    ///
    /// **This is what makes the client draw the paperdoll's Quest button.** A
    /// client told the shard is AoS has no quest system to show a button for,
    /// so the button is simply absent — and a server that answers `0xD7`/`0x32`
    /// perfectly will still look broken, because nothing ever sends one. The
    /// same goes for the Guild button beside it.
    pub const ML: Self = Self(Self::SE.0 | 0x80 | 0x200);
}

/// `0xB9` — the SupportedFeatures mask, sent before the character list.
///
/// # Why this stays a free function
///
/// Every other packet in this crate is [`Fixed`](PacketLength::Fixed) — a
/// constant size — or [`Variable`](PacketLength::Variable) — a self-describing
/// `u16` length field. `0xB9` is neither: it has no length field at all, and
/// its size (3 or 5 bytes) depends on the client version. [`EncodePacket::LENGTH`]
/// is a `const`, so it cannot ask `version` which shape this packet is before
/// framing runs. Forcing it into `Variable` would insert a length field the
/// wire format does not have; forcing a single `Fixed` size would be wrong for
/// half the clients. Until the framing layer can express "fixed, but the fixed
/// size depends on the version" (`0x08`'s problem too, on the decode side —
/// see [`crate::packet::client_packet_length`]), this one packet is written by
/// hand rather than bent to fit a model it does not.
///
/// It tells the client which expansion feature sets to turn on — chiefly, for
/// this engine, the AoS bit that enables object tooltips and context menus.
/// Without it (or without that bit) a modern client stays on the classic
/// single-click name path. The mask is the caller's to compose, since what a
/// shard enables is configuration rather than protocol.
///
/// `extended` picks the wire width: newer clients ([`Feature::ExtraFeatureMask`],
/// since 6.0.14.2) read a four-byte mask; older ones two. Mirrors ServUO's
/// `SupportedFeatures` / `NetState.ExtendedSupportedFeatures`.
#[must_use]
pub fn encode_supported_features(flags: SupportedFeatures, extended: bool) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(supported_features_length(extended).minimum());
    writer.u8(0xB9);
    if extended {
        writer.u32(flags.0);
    } else {
        writer.u16(flags.0 as u16);
    }
    debug_assert_eq!(writer.len(), supported_features_length(extended).minimum());
    writer.into_bytes()
}

/// How [`encode_supported_features`] is framed, for the mask width it was
/// written with.
///
/// Kept beside the encoder for the same reason as the encoder's own docs give
/// for not being an `EncodePacket`: the size is a function of the client, and a
/// framer on the other end has to reach the same answer from the same rule
/// rather than from a copy of the number.
#[must_use]
pub const fn supported_features_length(extended: bool) -> PacketLength {
    PacketLength::Fixed(if extended { 5 } else { 3 })
}

/// `0xA9` — the character list and starting cities.
///
/// `flags` is the client-capability mask; it is the caller's to compose, since
/// what a shard enables is configuration rather than protocol.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CharacterList {
    /// The account's characters, one per slot.
    pub characters: Vec<CharacterEntry>,
    /// The starting cities offered at character creation.
    pub starts:     Vec<StartLocation>,
    /// The client-capability mask; see [`CharacterListFlags`].
    pub flags:      CharacterListFlags,
}

impl EncodePacket for CharacterList {
    const ID: u8 = 0xA9;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        let slots = if version.supports(Feature::PaddedCharacterList) {
            self.characters.len().max(MIN_CHARACTER_SLOTS)
        } else {
            self.characters.len()
        };

        out.u8(slots as u8);
        for slot in 0..slots {
            let name = self
                .characters
                .get(slot)
                .map_or("", |entry| entry.name.0.as_str());
            write_character_slot(out, name);
        }

        out.u8(self.starts.len().min(u8::MAX as usize) as u8);
        let extra_info = version.supports(Feature::ExtraStartInfo);
        for (index, start) in self.starts.iter().take(u8::MAX as usize).enumerate() {
            out.u8(index as u8);
            if extra_info {
                // Since 7.0.13.0 the name fields are one byte wider *and* six
                // extra dwords follow. Getting the width wrong shifts
                // everything after it.
                out.fixed_string(&start.area, 32);
                out.fixed_string(&start.name, 32);
                out.i32(i32::from(start.position.x));
                out.i32(i32::from(start.position.y));
                out.i32(i32::from(start.position.z));
                // Both fields are dwords on the wire and the client reads them
                // signed; `u32` writes the same four big-endian bytes for every
                // value either type can hold, so the frame is unchanged.
                out.u32(u32::from(start.map.0));
                out.u32(start.description_cliloc.0);
                out.u32(0);
            } else {
                out.fixed_string(&start.area, 31);
                out.fixed_string(&start.name, 31);
            }
        }

        if version.supports(Feature::CharacterListFlags) {
            out.u32(self.flags.0);
        }
    }
}

impl DecodePacket for CharacterList {
    const ID: u8 = 0xA9;

    /// # Only the modern form
    ///
    /// Before 7.0.13.0 ([`Feature::ExtraStartInfo`]) a starting city is a name
    /// and an area and nothing else — no position, no map, no cliloc. There is
    /// no honest [`StartLocation`] to build from that, and filling the missing
    /// fields with zeros would hand a caller three coordinates that look chosen.
    /// So the old form says it is not decoded; a client this engine ships with
    /// never sees it, and one that did would want to know.
    ///
    /// # Empty slots come back as empty slots
    ///
    /// The list is padded to [`MIN_CHARACTER_SLOTS`] on the way out, so what
    /// arrives is five slots however many characters exist. Decoding gives back
    /// exactly what is on the wire, empty names included — this is a record of
    /// what the server said, and "slot three is empty" is something it said.
    fn decode_body(reader: &mut PacketReader<'_>, version: ClientVersion) -> Result<Self, DecodeError> {
        if !version.supports(Feature::ExtraStartInfo) {
            return Err(DecodeError::Unsupported {
                packet: <Self as DecodePacket>::ID,
                form:   "the pre-7.0.13.0 start list, which carries no coordinates",
            });
        }

        let slots = reader.u8()? as usize;
        let mut characters = Vec::with_capacity(slots);
        for _ in 0..slots {
            let name = reader.fixed_string(CHARACTER_NAME_LENGTH)?;
            reader.skip(PASSWORD_LENGTH)?; // the vestigial password field
            characters.push(CharacterEntry {
                name: CharacterName(name),
            });
        }

        let start_count = reader.u8()? as usize;
        let mut starts = Vec::with_capacity(start_count);
        for _ in 0..start_count {
            reader.skip(1)?; // the index, which is the position in this list
            let area = reader.fixed_string(32)?;
            let name = reader.fixed_string(32)?;
            let position = Point::new(reader.i32()? as u16, reader.i32()? as u16, reader.i32()? as i8);
            let map = Facet(reader.u32()? as u8);
            let description_cliloc = ClilocId(reader.u32()?);
            reader.skip(4)?; // the trailing zero dword
            starts.push(StartLocation {
                area,
                name,
                position,
                map,
                description_cliloc,
            });
        }

        let flags = CharacterListFlags(reader.u32()?);
        Ok(Self {
            characters,
            starts,
            flags,
        })
    }
}

/// Write one 60-byte character slot: a 30-byte name and the vestigial 30-byte
/// password field. Shared by the `0xA9` list and the `0x86` post-delete resend,
/// so the two never disagree about the slot width.
fn write_character_slot(writer: &mut PacketWriter, name: &str) {
    writer.fixed_string(name, CHARACTER_NAME_LENGTH);
    // The password field is vestigial: the client sends it back but no modern
    // server puts anything in it.
    writer.fixed_string("", PASSWORD_LENGTH);
}

// -- 0x83 delete character ------------------------------------------------

/// `0x83` — the client asks to delete a character by slot. 39 bytes.
///
/// The client sends a vestigial 30-byte password field (which no modern server
/// trusts), then the slot index, then its own IP (which we ignore). ServUO's
/// `PacketHandlers.DeleteCharacter` seeks past the 30 and reads only the index;
/// this does the same, so only the slot survives decoding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeleteCharacter {
    /// The slot to delete, as the client named it — an index into the list it
    /// was last sent, and the one place a [`RawCharacterSlot`] is actually
    /// read. [`RawCharacterSlot::validate`] is what turns it into a slot the
    /// account has.
    pub slot: RawCharacterSlot,
}

impl DecodePacket for DeleteCharacter {
    const ID: u8 = 0x83;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        reader.skip(PASSWORD_LENGTH)?; // vestigial password field
        let slot = RawCharacterSlot(reader.u32()?);
        // The trailing client IP is unused.
        Ok(Self { slot })
    }
}

// -- 0x85 delete rejected -------------------------------------------------

/// Why a character deletion was refused — the `0x85` result code.
///
/// The client renders each as its own message on the character-select screen.
/// From ServUO's `DeleteResultType`; the codes are the client's, not ours.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeleteResult {
    /// The account password did not check out.
    PasswordInvalid = 0,
    /// No character in that slot.
    CharNotExist = 1,
    /// The character is logged in and cannot be deleted.
    CharBeingPlayed = 2,
    /// The character is too young to delete (the newbie window).
    CharTooYoung = 3,
    /// The character is queued for deletion.
    CharQueued = 4,
    /// The request made no sense.
    BadRequest = 5,
}

/// `0x85` — a character deletion was refused, with the reason. 2 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DeleteReject {
    /// Why.
    pub result: DeleteResult,
}

impl EncodePacket for DeleteReject {
    const ID: u8 = 0x85;
    const LENGTH: PacketLength = PacketLength::Fixed(2);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(self.result as u8);
    }
}

// -- 0x86 character list update -------------------------------------------

/// `0x86` — resend the character list after a successful deletion.
///
/// The character block of `0xA9` on its own, no city list: a count byte then
/// `count` 60-byte slots. The count is padded to [`MIN_CHARACTER_SLOTS`] like
/// the full list, so the client redraws all five rows. Ported from ServUO's
/// `CharacterListUpdate`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CharacterListUpdate {
    /// The account's characters, one per slot, after the delete.
    pub characters: Vec<CharacterEntry>,
}

impl EncodePacket for CharacterListUpdate {
    const ID: u8 = 0x86;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        let slots = self.characters.len().max(MIN_CHARACTER_SLOTS);
        out.u8(slots as u8);
        for slot in 0..slots {
            let name = self
                .characters
                .get(slot)
                .map_or("", |entry| entry.name.0.as_str());
            write_character_slot(out, name);
        }
    }
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

impl DecodePacket for ClientVersionReport {
    const ID: u8 = 0xBD;

    /// `decode_packet` has already consumed the length field before calling
    /// this — see its doc comment — so `reader.rest()` here is exactly the
    /// declared body: whatever the client wrote, already bounded by the frame
    /// that got us here. No terminator is required: a client that omits the
    /// NUL still gets a version out of whatever is left, which is the same
    /// leniency `raw` documents for junk content.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let body = reader.rest();
        let body = &body[..body.len().min(Self::MAX_LENGTH)];
        let end = body.iter().position(|byte| *byte == 0).unwrap_or(body.len());
        Ok(Self {
            raw: body[..end].iter().map(|byte| *byte as char).collect(),
        })
    }
}

impl ClientVersionReport {
    /// Sphere clamps the version string to this before reading it.
    pub const MAX_LENGTH: usize = 20;

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

    /// Encode a whole 0xBD packet. Test fixtures only — see the module docs.
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(4 + self.raw.len());
        writer.u8(Self::ID);
        writer.u16(0);
        writer.null_terminated_string(&self.raw);
        patch_length(writer.into_bytes())
    }
}

/// Write the final length into a variable-length packet's `u16` at offset 1.
///
/// Used only by [`ClientVersionReport::encode`]'s test fixtures: every
/// server-to-client variable packet in this module goes through
/// [`crate::packet::frame_body`] instead, which is the one place production
/// code patches a length.
fn patch_length(mut bytes: Vec<u8>) -> Vec<u8> {
    let length = u16::try_from(bytes.len()).expect("packet outgrew its u16 length field");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

// -- the decoded client packet ---------------------------------------------

/// One packet the login conversation understood, already decoded.
///
/// Without this, `LoginServer::handle` matched the raw id byte to pick a
/// handler, and each handler decoded the same bytes again to get a typed
/// value — one place that knew the id, another that knew the type, and the
/// two could in principle disagree. [`LoginStagePacket::decode`] does both
/// in one pass, so `handle` matches on the result and nothing else in the
/// login crate touches a raw packet buffer.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum LoginStagePacket {
    /// `0xBD`, successfully framed. Whether the version *string* itself
    /// parsed is a separate question — see [`ClientVersionReport::version`].
    VersionReport(ClientVersionReport),
    /// `0xBD` arrived too mangled to reach a body decoder at all. Not fatal:
    /// the seed usually already carried a version, and a client sending junk
    /// here is the normal case [`ClientVersionReport`] tolerates, just one
    /// layer further in.
    MalformedVersionReport,
    /// `0x80` — account name and password.
    AccountLogin(AccountLogin),
    /// `0xA0` — the shard the client picked.
    SelectShard(SelectShard),
    /// `0x91` — login to the game server after a relay.
    GameServerLogin(GameServerLogin),
    /// Any id the login conversation does not act on. Real clients send
    /// several of these during login (`0xBE` assist version, `0xA4` system
    /// info), and dropping the connection over them would break every one of
    /// them for no reason.
    Unknown(u8),
    /// `0x00`/`0xF8` — character creation. Not part of the login state
    /// machine below: the `server` crate intercepts it before it ever reaches
    /// [`crate::login`]'s own [`LoginStagePacket::decode`] call, because
    /// acting on it needs both the account (here) and the world (which this
    /// crate never sees). Decoded here anyway, next to [`Self::DeleteCharacter`],
    /// so this is still the one place that knows every id the wire can carry.
    CreateCharacter(CreateCharacter),
    /// `0x83` — character deletion. Crosses the same login/world line as
    /// [`Self::CreateCharacter`], for the same reason.
    DeleteCharacter(DeleteCharacter),
    /// `0x5D` — the character the client picked off the list, and the packet
    /// that starts the world.
    ///
    /// The third of the character screen's, beside [`Self::CreateCharacter`] and
    /// [`Self::DeleteCharacter`], and it is here for the reason the seam is drawn
    /// where it is: everything before a character is in the world belongs to the
    /// screen, whoever ends up acting on it. It was a [`ClientPacket`] until the
    /// backlog of `docs/connection_state.md` caught up with it, which left the
    /// world's dispatcher with one arm it could never legitimately reach and an
    /// `unreachable!` standing in for the invariant. On this side of the split
    /// that arm cannot be written.
    ///
    /// [`ClientPacket`]: crate::client_packet::ClientPacket
    PlayCharacter(CharacterPlay),
}

impl LoginStagePacket {
    /// Decode `packet` by its id byte.
    ///
    /// `packet` must be non-empty. That is an invariant, not a checked
    /// precondition: every packet reaching this point has already survived
    /// [`crate::packet::frame_client_packet`], whose shortest possible frame is
    /// one byte (the id), so an empty slice here means a caller skipped
    /// framing — a bug worth panicking on rather than laundering through an
    /// `Option` every caller would immediately unwrap.
    ///
    /// `Err(_)` only for a known id whose body failed to decode; an unknown id
    /// is `Ok(Self::Unknown)`, not an error, per [`Self::Unknown`].
    pub fn decode(packet: &[u8], version: ClientVersion) -> Result<Self, ClientLoginDecodeError> {
        let id = *packet
            .first()
            .expect("packet is empty: caller skipped framing, which never produces one");
        match id {
            ClientVersionReport::ID => {
                Ok(decode_packet(packet, version).map_or(Self::MalformedVersionReport, Self::VersionReport))
            }
            AccountLogin::ID => {
                decode_packet(packet, version)
                    .map(Self::AccountLogin)
                    .map_err(ClientLoginDecodeError::AccountLogin)
            }
            SelectShard::ID => {
                decode_packet(packet, version)
                    .map(Self::SelectShard)
                    .map_err(ClientLoginDecodeError::SelectShard)
            }
            GameServerLogin::ID => {
                decode_packet(packet, version)
                    .map(Self::GameServerLogin)
                    .map_err(ClientLoginDecodeError::GameServerLogin)
            }
            CreateCharacter::ID_CLASSIC | CreateCharacter::ID_HIGH_SEAS => {
                CreateCharacter::decode(packet)
                    .map(Self::CreateCharacter)
                    .map_err(ClientLoginDecodeError::CreateCharacter)
            }
            DeleteCharacter::ID => {
                decode_packet(packet, version)
                    .map(Self::DeleteCharacter)
                    .map_err(ClientLoginDecodeError::DeleteCharacter)
            }
            CharacterPlay::ID => {
                decode_packet(packet, version)
                    .map(Self::PlayCharacter)
                    .map_err(ClientLoginDecodeError::PlayCharacter)
            }
            _ => Ok(Self::Unknown(id)),
        }
    }
}

/// A known client-login packet id arrived but its body did not decode.
///
/// Fatal in every case: the login conversation closes the connection rather
/// than act on half-read credentials or a forged relay key. Kept as one
/// variant per packet, rather than collapsing to `(u8, DecodeError)`, so a
/// caller can match the packet by type the same way it would match
/// [`LoginStagePacket`] itself.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ClientLoginDecodeError {
    /// `0x80` did not decode.
    AccountLogin(DecodeError),
    /// `0xA0` did not decode.
    SelectShard(DecodeError),
    /// `0x91` did not decode.
    GameServerLogin(DecodeError),
    /// `0x00`/`0xF8` did not decode.
    CreateCharacter(DecodeError),
    /// `0x83` did not decode.
    DeleteCharacter(DecodeError),
    /// `0x5D` did not decode.
    PlayCharacter(DecodeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::WrongPacket;
    use crate::packet::{
        client_packet_length,
        encode_packet,
    };
    use crate::wire::{
        CharacterSlot,
        InvalidCharacterSlot,
    };

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    /// The `u16` a variable-length packet declares at offset 1.
    fn declared_length(bytes: &[u8]) -> usize {
        u16::from_be_bytes([bytes[1], bytes[2]]) as usize
    }

    /// The shard count, which sits after the id, the length and the 0xFF flag.
    fn shard_count(bytes: &[u8]) -> usize {
        u16::from_be_bytes([bytes[4], bytes[5]]) as usize
    }

    fn shard(name: &str, address: [u8; 4]) -> ShardEntry {
        ShardEntry {
            name:         name.to_owned(),
            percent_full: PercentFull::clamped(10),
            timezone:     5,
            address:      Ipv4Addr::from(address),
        }
    }

    #[test]
    fn account_login_round_trips_at_the_declared_length() {
        let login = AccountLogin {
            account:  RawAccountName("admin".to_owned()),
            password: RawPlaintextPassword("hunter2".to_owned()),
        };
        let bytes = login.encode();

        assert_eq!(
            client_packet_length(AccountLogin::ID, None),
            Some(PacketLength::Fixed(62))
        );
        assert_eq!(bytes.len(), 62, "the table and the encoder must agree");
        assert_eq!(decode_packet::<AccountLogin>(&bytes, version()).unwrap(), login);
    }

    #[test]
    fn account_login_rejects_the_wrong_packet() {
        let mut bytes = AccountLogin {
            account:  RawAccountName("a".to_owned()),
            password: RawPlaintextPassword("b".to_owned()),
        }
        .encode();
        bytes[0] = 0x91;
        assert_eq!(
            decode_packet::<AccountLogin>(&bytes, version()),
            Err(DecodeError::WrongPacket(WrongPacket {
                expected: 0x80,
                found:    0x91,
            }))
        );
    }

    #[test]
    fn account_login_rejects_a_truncated_packet() {
        let bytes = [0x80u8, b'a', b'b'];
        assert!(matches!(
            decode_packet::<AccountLogin>(&bytes, version()),
            Err(DecodeError::Codec(_))
        ));
    }

    #[test]
    #[should_panic(expected = "packet is empty")]
    fn client_login_packet_decode_panics_on_an_empty_slice() {
        // Real packets never arrive empty: `frame_client_packet`'s shortest
        // frame is the one-byte id. An empty slice here means whoever called
        // `decode` skipped framing, which is a server bug worth a panic, not
        // a laundered `Option`.
        let _ = LoginStagePacket::decode(&[], version());
    }

    #[test]
    fn client_login_packet_decode_reports_which_id_failed() {
        let bytes = [AccountLogin::ID, b'a', b'b'];
        assert!(matches!(
            LoginStagePacket::decode(&bytes, version()),
            Err(ClientLoginDecodeError::AccountLogin(_))
        ));
    }

    #[test]
    fn delete_character_reads_the_slot_past_the_vestigial_fields() {
        // 39 bytes: id + 30 pad + u32 slot + 4 client IP.
        let mut bytes = vec![DeleteCharacter::ID];
        bytes.extend(std::iter::repeat_n(0u8, PASSWORD_LENGTH));
        bytes.extend_from_slice(&3u32.to_be_bytes());
        bytes.extend_from_slice(&[192, 168, 0, 1]);
        assert_eq!(
            client_packet_length(DeleteCharacter::ID, None),
            Some(PacketLength::Fixed(39))
        );
        assert_eq!(bytes.len(), 39, "the table and the wire form must agree");
        assert_eq!(
            decode_packet::<DeleteCharacter>(&bytes, version()).unwrap().slot,
            RawCharacterSlot(3)
        );
    }

    /// N9's pair for `RawCharacterSlot`: the slot a `0x83` names is three
    /// well-formed dwords whatever number it holds, so it decodes, and the
    /// refusal happens at promotion — where the list it indexes is in hand.
    /// Refused and not clamped: clamping would delete *some* character.
    #[test]
    fn a_delete_naming_a_slot_the_account_lacks_decodes_and_is_refused() {
        let mut bytes = vec![DeleteCharacter::ID];
        bytes.extend(std::iter::repeat_n(0u8, PASSWORD_LENGTH));
        bytes.extend_from_slice(&7u32.to_be_bytes());
        bytes.extend_from_slice(&[192, 168, 0, 1]);

        let decoded = decode_packet::<DeleteCharacter>(&bytes, version()).unwrap();
        assert_eq!(decoded.slot, RawCharacterSlot(7), "the dword survives decoding");
        assert_eq!(
            decoded.slot.validate(5),
            Err(InvalidCharacterSlot { slot: 7, held: 5 }),
            "five characters means slots 0..5"
        );
        assert_eq!(decoded.slot.validate(8), Ok(CharacterSlot(7)));

        // An empty account has no slot at all, zero included — the check is a
        // count, so it does not need a separate "is the list empty" branch.
        assert!(RawCharacterSlot(0).validate(0).is_err());
        assert_eq!(RawCharacterSlot(0).validate(1), Ok(CharacterSlot(0)));
    }

    /// The bits are the client's, ported from ClassicUO and ServUO, and a
    /// constant with the wrong value fails silently: the client simply never
    /// sends the request the flag was supposed to enable. Pinned here per
    /// CLAUDE.md — a flag's value belongs in a test beside the constant.
    #[test]
    fn the_two_capability_masks_keep_their_ported_bits() {
        assert_eq!(CharacterListFlags::CONTEXT_MENU.0, 0x08);
        assert_eq!(CharacterListFlags::TOOLTIPS.0, 0x20);
        assert_eq!(
            CharacterListFlags::TOOLTIPS
                .with(CharacterListFlags::CONTEXT_MENU)
                .0,
            0x28
        );
        assert_eq!(CharacterListFlags::NONE.0, 0);

        // ServUO's FeatureFlags: T2A|UOR|UOTD|LBR|AOS, then SE, then ML plus
        // NinthAge. Each expansion contains the one before it.
        assert_eq!(SupportedFeatures::AOS.0, 0x1F);
        assert_eq!(SupportedFeatures::SE.0, 0x5F);
        assert_eq!(SupportedFeatures::ML.0, 0x2DF);
        for (wider, narrower) in [
            (SupportedFeatures::SE, SupportedFeatures::AOS),
            (SupportedFeatures::ML, SupportedFeatures::SE),
        ] {
            assert_eq!(
                wider.0 & narrower.0,
                narrower.0,
                "{wider:?} must contain {narrower:?}"
            );
        }
        // `LiveAccount` asks for a sixth character slot the list is not sized
        // for, so no mask here may carry it.
        for mask in [
            SupportedFeatures::AOS,
            SupportedFeatures::SE,
            SupportedFeatures::ML,
        ] {
            assert_eq!(mask.0 & 0x8000, 0, "{mask:?} must not advertise LiveAccount");
        }
    }

    #[test]
    fn delete_character_rejects_the_wrong_packet() {
        let bytes = [0x91u8; 39];
        assert!(matches!(
            decode_packet::<DeleteCharacter>(&bytes, version()),
            Err(DecodeError::WrongPacket(_))
        ));
    }

    #[test]
    fn delete_reject_is_two_bytes_carrying_the_reason() {
        assert_eq!(
            encode_packet(
                &DeleteReject {
                    result: DeleteResult::CharBeingPlayed,
                },
                version()
            ),
            vec![0x85, 2]
        );
    }

    #[test]
    fn character_list_update_pads_to_five_slots() {
        let characters = vec![CharacterEntry {
            name: CharacterName("Dupre".to_owned()),
        }];
        let bytes = encode_packet(&CharacterListUpdate { characters }, version());
        assert_eq!(bytes[0], 0x86);
        assert_eq!(declared_length(&bytes), bytes.len(), "self-declared length");
        assert_eq!(bytes[3], MIN_CHARACTER_SLOTS as u8, "padded to five rows");
        assert_eq!(bytes.len(), 4 + MIN_CHARACTER_SLOTS * 60);
        // The first slot's name sits right after the count byte.
        assert_eq!(&bytes[4..9], b"Dupre");
        // The second slot is empty.
        assert_eq!(bytes[64], 0);
    }

    #[test]
    fn account_login_truncates_an_overlong_name_to_its_field() {
        let login = AccountLogin {
            account:  RawAccountName("x".repeat(50)),
            password: RawPlaintextPassword(String::new()),
        };
        assert_eq!(login.encode().len(), 62, "a long name must not overrun");
        assert_eq!(
            decode_packet::<AccountLogin>(&login.encode(), version())
                .unwrap()
                .account
                .0
                .len(),
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
        assert_eq!(DenyReason::BlockedIp.wire_code(), 0x02, "reads as 'blocked'");
        assert_eq!(DenyReason::ShardFull.wire_code(), 0x04, "reads as 'other'");
    }

    #[test]
    fn login_denied_matches_the_declared_length() {
        let bytes = encode_packet(
            &LoginDenied {
                reason: DenyReason::BadPassword,
            },
            version(),
        );
        assert_eq!(bytes, vec![0x82, 0x03]);
        assert_eq!(bytes.len(), 2);
    }

    #[test]
    fn shard_list_frames_and_declares_its_own_length() {
        let shards = vec![shard("Britannia", [10, 0, 0, 1])];
        let bytes = encode_packet(&ShardList { shards }, ClientVersion::TOL);

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
    fn shard_list_reverses_the_ip_for_modern_clients() {
        // The address is 192.168.11.6 because that is the one that caught this:
        // a real client, told to dial 6.11.168.192, timing out.
        //
        // Reversed for the NEWER client. That is the way round it goes, it is
        // the opposite of the relay two packets later, and both SphereServer and
        // ServUO do exactly this. Sphere's inline comments say otherwise and are
        // wrong; its shifts are right. This test is the shifts.
        let shards = vec![shard("Britannia", [192, 168, 11, 6])];

        let modern = encode_packet(
            &ShardList {
                shards: shards.clone(),
            },
            ClientVersion::new(4, 0, 0, 0),
        );
        assert_eq!(
            &modern[42..46],
            &[6, 11, 168, 192],
            "since 4.0.0 the shard list carries the address backwards"
        );

        let ancient = encode_packet(&ShardList { shards }, ClientVersion::new(3, 255, 255, 255));
        assert_eq!(
            &ancient[42..46],
            &[192, 168, 11, 6],
            "one patch below, the octets go in order"
        );
    }

    #[test]
    fn shard_list_drops_entries_past_the_client_crash_point() {
        let shards: Vec<_> = (0..40).map(|i| shard(&format!("s{i}"), [1, 2, 3, 4])).collect();
        let bytes = encode_packet(&ShardList { shards }, ClientVersion::TOL);
        assert_eq!(
            shard_count(&bytes),
            MAX_SHARDS,
            "more than 32 crashes the client, so they are dropped not sent"
        );
    }

    /// The client renders anything above 100 as garbage, so no `ShardEntry`
    /// can be built holding such a value — the ceiling is applied where the
    /// number is chosen, not repaired on the way out. `100` and everything
    /// below it survives untouched: a clamp that also moved legal values would
    /// be a rescale, and the operator's number would stop meaning what it says.
    #[test]
    fn percent_full_cannot_hold_a_value_the_client_would_draw_as_garbage() {
        assert_eq!(PercentFull::clamped(0).raw(), 0);
        assert_eq!(PercentFull::clamped(99).raw(), 99);
        assert_eq!(PercentFull::clamped(100).raw(), 100, "100 is legal, not clamped");
        assert_eq!(
            PercentFull::clamped(101).raw(),
            100,
            "one over is the first clamp"
        );
        assert_eq!(PercentFull::clamped(250).raw(), 100);
        assert_eq!(PercentFull::EMPTY.raw(), 0);
        assert_eq!(PercentFull::FULL.raw(), 100);
    }

    /// And the wire agrees: a nonsense number cannot reach the client through
    /// the encoder, and one arriving *from* a server cannot leave the decoder.
    #[test]
    fn shard_list_carries_only_a_renderable_fullness() {
        let mut entry = shard("Britannia", [10, 0, 0, 1]);
        entry.percent_full = PercentFull::clamped(250);
        let bytes = encode_packet(&ShardList { shards: vec![entry] }, ClientVersion::TOL);
        assert_eq!(bytes[40], 100, "the client renders >100 as garbage");

        // Through `ServerPacket::decode`, which is the client's own route in:
        // `decode_packet` reads the *client* length table and 0xA8 is not in it.
        let mut forged = bytes;
        forged[40] = 250;
        let Ok(Some(crate::server_packet::ServerPacket::ShardList(decoded))) =
            crate::server_packet::ServerPacket::decode(&forged, ClientVersion::TOL)
        else {
            panic!("a shard list must decode as one");
        };
        assert_eq!(
            decoded.shards[0].percent_full,
            PercentFull::FULL,
            "a server's nonsense byte is clamped, not carried into the client"
        );
    }

    #[test]
    fn select_shard_round_trips() {
        let select = SelectShard {
            index: RawShardIndex(1),
        };
        let bytes = select.encode();
        assert_eq!(bytes.len(), 3);
        assert_eq!(
            client_packet_length(SelectShard::ID, None),
            Some(PacketLength::Fixed(3))
        );
        assert_eq!(decode_packet::<SelectShard>(&bytes, version()).unwrap(), select);
        assert_eq!(
            select.index.validate(1),
            Ok(ShardIndex(0)),
            "the wire is one-based"
        );
    }

    /// N9's pair for `RawShardIndex`: an index the list never offered decodes
    /// cleanly — it is three well-formed bytes — and is refused at promotion,
    /// which is where a refusal belongs. Both ways of being wrong, because
    /// they are different bugs: zero is the wire's own impossibility and used
    /// to be checked in the packet, past-the-end used to be checked a hundred
    /// lines away in `openshard_login`. One promotion, both refusals.
    #[test]
    fn a_shard_index_the_list_never_offered_decodes_and_is_refused() {
        for index in [0u16, 2, 99, u16::MAX] {
            let bytes = SelectShard {
                index: RawShardIndex(index),
            }
            .encode();
            let decoded = decode_packet::<SelectShard>(&bytes, version())
                .unwrap_or_else(|error| panic!("{index} must decode, not {error:?}"));
            assert_eq!(decoded.index, RawShardIndex(index), "the byte survives decoding");
            assert!(
                decoded.index.validate(1).is_err(),
                "{index} is not one of the one shard that was offered"
            );
        }

        // Zero says which of the two it is, because a naive `index - 1` on a
        // u16 zero wraps to 65535 and reads far past the list.
        assert_eq!(RawShardIndex(0).validate(4), Err(InvalidShardIndex::Zero));
        assert_eq!(
            RawShardIndex(5).validate(4),
            Err(InvalidShardIndex::PastEnd {
                index:   5,
                offered: 4,
            })
        );
        assert_eq!(RawShardIndex(4).validate(4), Ok(ShardIndex(3)), "the last shard");
    }

    #[test]
    fn the_relay_sends_the_octets_in_order() {
        // This is the packet that decides whether anyone ever reaches the shard,
        // and getting it wrong is invisible from the server: the client dials
        // what it was given, finds nothing, and the log shows a clean login
        // followed by a disconnect that looks entirely normal.
        //
        // It shipped reversed once. A real ClassicUO, told 192.168.11.6, said:
        //
        //     Connecting to tcp://6.11.168.192:2593/
        //     error while connecting ... Operation timed out
        //
        // Hence the address. This test is that log line.
        let bytes = encode_packet(
            &Relay {
                endpoint: SocketAddrV4::new(Ipv4Addr::new(192, 168, 11, 6), 2593),
                auth_key: AuthKey(0xDEAD_BEEF),
            },
            version(),
        );
        assert_eq!(bytes.len(), 11);
        assert_eq!(&bytes[1..5], &[192, 168, 11, 6]);
        assert_eq!(&bytes[5..7], &2593u16.to_be_bytes(), "the port is not touched");
        assert_eq!(&bytes[7..11], &0xDEAD_BEEFu32.to_be_bytes());
    }

    #[test]
    fn the_relay_and_the_shard_list_disagree_about_the_same_address() {
        // Not a curiosity: this is the whole trap, and a change that makes these
        // two agree has broken one of them. Two packets, one conversation, one
        // address, opposite orders — because that is what the client does.
        let address = Ipv4Addr::new(192, 168, 11, 6);
        let modern = ClientVersion::new(7, 0, 45, 65);

        let list = encode_packet(
            &ShardList {
                shards: vec![shard("Britannia", address.octets())],
            },
            modern,
        );
        let relay = encode_packet(
            &Relay {
                endpoint: SocketAddrV4::new(address, 2593),
                auth_key: AuthKey(0),
            },
            modern,
        );

        assert_eq!(&list[42..46], &[6, 11, 168, 192]);
        assert_eq!(&relay[1..5], &[192, 168, 11, 6]);
    }

    #[test]
    fn the_relay_does_not_care_what_the_client_is() {
        // 0xA8 has a version gate. This one does not, and adding one would be
        // the obvious "symmetry" fix that breaks every modern client.
        for version in [
            ClientVersion::OLDEST,
            ClientVersion::new(3, 0, 0, 0),
            ClientVersion::new(4, 0, 0, 0),
            ClientVersion::TOL,
        ] {
            let bytes = encode_packet(
                &Relay {
                    endpoint: SocketAddrV4::new(Ipv4Addr::new(192, 168, 11, 6), 2593),
                    auth_key: AuthKey(0),
                },
                version,
            );
            assert_eq!(&bytes[1..5], &[192, 168, 11, 6]);
        }
    }

    #[test]
    fn game_server_login_round_trips_at_the_declared_length() {
        let login = GameServerLogin {
            auth_key: AuthKey(0x1234_5678),
            account:  RawAccountName("admin".to_owned()),
            password: RawPlaintextPassword("hunter2".to_owned()),
        };
        let bytes = login.encode();
        assert_eq!(
            client_packet_length(GameServerLogin::ID, None),
            Some(PacketLength::Fixed(65))
        );
        assert_eq!(bytes.len(), 65);
        assert_eq!(
            decode_packet::<GameServerLogin>(&bytes, version()).unwrap(),
            login
        );
    }

    #[test]
    fn character_list_pads_to_five_slots() {
        // Clients since 3.0.0.10 read five slots whatever the count byte says.
        let characters = vec![CharacterEntry {
            name: CharacterName("Lord British".to_owned()),
        }];
        let bytes = encode_packet(
            &CharacterList {
                characters,
                starts: Vec::new(),
                flags: CharacterListFlags::NONE,
            },
            ClientVersion::TOL,
        );

        assert_eq!(bytes[3], 5, "one character still means five slots");
        assert_eq!(&bytes[4..16], b"Lord British");
        assert_eq!(&bytes[64..76], &[0u8; 12], "slot two is blank, not absent");
        assert_eq!(declared_length(&bytes), bytes.len());
    }

    #[test]
    fn character_list_does_not_pad_for_clients_that_predate_the_rule() {
        let characters = vec![CharacterEntry {
            name: CharacterName("Lord British".to_owned()),
        }];
        let old = ClientVersion::new(3, 0, 0, 9);
        assert!(!old.supports(Feature::PaddedCharacterList));

        let bytes = encode_packet(
            &CharacterList {
                characters,
                starts: Vec::new(),
                flags: CharacterListFlags::NONE,
            },
            old,
        );
        assert_eq!(bytes[3], 1);
    }

    #[test]
    fn character_list_start_locations_widen_at_7_0_13_0() {
        let starts = vec![StartLocation {
            area:               "Britain".to_owned(),
            name:               "Castle Britannia".to_owned(),
            position:           Point::new(1475, 1774, 0),
            map:                Facet(0),
            description_cliloc: ClilocId(1_075_072),
        }];

        let list = CharacterList {
            characters: Vec::new(),
            starts,
            flags: CharacterListFlags::NONE,
        };
        let modern = encode_packet(&list, ClientVersion::new(7, 0, 13, 0));
        let ancient = encode_packet(&list, ClientVersion::new(7, 0, 12, 255));
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
        let list = CharacterList {
            characters: Vec::new(),
            starts:     Vec::new(),
            flags:      CharacterListFlags(0xAABB_CCDD),
        };
        let with_flags = encode_packet(&list, ClientVersion::new(1, 26, 0, 1));
        let without = encode_packet(&list, ClientVersion::new(1, 26, 0, 0));
        assert_eq!(
            with_flags.len() - without.len(),
            4,
            "send.cpp gates the flags dword on version > 1.26.0.0"
        );
        assert_eq!(&with_flags[with_flags.len() - 4..], &0xAABB_CCDDu32.to_be_bytes());
    }

    #[test]
    fn client_version_report_round_trips() {
        let report = ClientVersionReport {
            raw: "7.0.45.65".to_owned(),
        };
        let bytes = report.encode();
        assert_eq!(
            client_packet_length(ClientVersionReport::ID, None),
            Some(PacketLength::Variable)
        );
        assert_eq!(declared_length(&bytes), bytes.len());

        let decoded: ClientVersionReport = decode_packet(&bytes, version()).unwrap();
        assert_eq!(decoded, report);
        assert_eq!(decoded.version(), Some(ClientVersion::new(7, 0, 45, 65)));
        assert!(!decoded.is_3d_client());
    }

    #[test]
    fn client_version_report_spots_the_3d_client() {
        let report = ClientVersionReport {
            raw: "4.0.0a, UO:3D".to_owned(),
        };
        let decoded: ClientVersionReport = decode_packet(&report.encode(), version()).unwrap();
        assert!(decoded.is_3d_client());
    }

    #[test]
    fn client_version_report_survives_junk() {
        // The version is a claim from the network; garbage must not be fatal.
        let report = ClientVersionReport {
            raw: "not a version".to_owned(),
        };
        let decoded: ClientVersionReport = decode_packet(&report.encode(), version()).unwrap();
        assert_eq!(decoded.version(), None);
    }

    /// `decode_packet` skips the length field rather than re-checking it: by
    /// the time a body decoder runs, `frame_client_packet` has already proved
    /// `bytes` is a complete, correctly-bounded packet (see its doc comment).
    /// So a length field that lies about the packet's true size no longer
    /// fails *here* — that check now lives once, at the framing layer, not
    /// duplicated in every variable-length decoder.
    #[test]
    fn decoding_does_not_re_validate_a_length_field_framing_already_checked() {
        let bytes = [0xBD, 0xFF, 0xFF, b'7', 0x00];
        let decoded: ClientVersionReport = decode_packet(&bytes, version()).unwrap();
        assert_eq!(decoded.raw, "7");
    }

    #[test]
    fn client_version_report_clamps_a_long_string() {
        let report = ClientVersionReport { raw: "9".repeat(80) };
        let decoded: ClientVersionReport = decode_packet(&report.encode(), version()).unwrap();
        assert_eq!(
            decoded.raw.len(),
            ClientVersionReport::MAX_LENGTH,
            "Sphere clamps to 20 before reading"
        );
    }

    #[test]
    fn client_version_report_does_not_scan_past_its_clamp() {
        let report = ClientVersionReport {
            raw: format!("{}\0ignored", "9".repeat(ClientVersionReport::MAX_LENGTH)),
        };
        let decoded: ClientVersionReport = decode_packet(&report.encode(), version()).unwrap();
        assert_eq!(decoded.raw, "9".repeat(ClientVersionReport::MAX_LENGTH));
    }
}
