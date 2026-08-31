//! Speech: what a player says, and what everyone nearby is shown.
//!
//! Two ways in, one way out. The classic client types and sends `0x03` (ASCII);
//! a modern client sends `0xAD` (Unicode, with a language tag) — and it is the
//! modern one that matters, because that is what ClassicUO and every 7.x client
//! actually send. The server turns it into a message over the speaker's head for
//! everyone in earshot: `0x1C` (ASCII) for Latin-1 speech, `0xAE` (big-endian
//! UTF-16) for text ASCII cannot carry — an accent, a non-Latin script. A client
//! that spoke `0xAD` gets `0xAE` back, so the words it typed survive the round
//! trip intact.

use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
    frame_body,
};
use crate::serial::Serial;
use crate::version::ClientVersion;
use crate::wire::{
    ClilocId,
    Graphic,
    Hue,
    RawHue,
};

/// The name field in a `0x1C` is a fixed thirty bytes.
const NAME_LENGTH: usize = 30;

/// How a line is said: the mode byte every speech packet carries, in both
/// directions.
///
/// The client's own domain — ServUO's `MessageType`, Sphere's `TALKMODE_TYPE` —
/// and it decides three things at once: how far the words carry, what the client
/// draws them as, and whether they go in the journal. Only the modes this engine
/// has a use for are named; everything else the client can send arrives as
/// [`Other`](Self::Other) rather than as a guess about what the number means.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TalkMode {
    /// Ordinary speech, heard across the screen. The client's `0`.
    Regular,
    /// An emote — the client wraps it in asterisks. ServUO's `MessageType.Emote`.
    Emote,
    /// A name label drawn over an object rather than words spoken by it: what a
    /// single click answers with. ServUO's `MessageType.Label`.
    Label,
    /// A whisper, heard only right beside the speaker.
    Whisper,
    /// A yell, carried two screens off.
    Yell,
    /// A line to the speaker's guild. ServUO's `MessageType.Guild`.
    ///
    /// **Not a range**, unlike every mode above it: distance decides nothing,
    /// and a listener is picked by membership rather than by where they are
    /// standing. Anything that routes speech by earshot has to branch off this
    /// before it measures — see `openshard_guilds::chat`.
    Guild,
    /// The same, to every guild this one is allied with. ServUO's
    /// `MessageType.Alliance`.
    Alliance,
    /// A mode this engine has no rule for, carried through as the byte it was.
    ///
    /// As [`RawTalkMode::interpret`] produces it, never one of the named modes
    /// and never carrying the keyword bits — which is what makes `interpret`
    /// followed by [`to_wire`](Self::to_wire) the identity on the mode itself.
    Other(u8),
}

impl TalkMode {
    /// The byte to put on the wire.
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Regular => 0,
            Self::Emote => 2,
            Self::Label => 6,
            Self::Whisper => 8,
            Self::Yell => 9,
            Self::Guild => 13,
            Self::Alliance => 14,
            Self::Other(mode) => mode,
        }
    }
}

/// The two top bits of a `0xAD` mode byte, which say the text is preceded by a
/// block of recognised keyword ids rather than being plain — ServUO's
/// `MessageType.Encoded`. They are framing, not a mode, and
/// [`RawTalkMode::interpret`] drops them.
const KEYWORD_BITS: u8 = 0xC0;

/// The mode byte exactly as a client packet carried it, keyword bits and all.
///
/// Kept unfolded through decoding on purpose: a decoder that rewrites a value
/// destroys the only evidence of what the client actually sent, so a log line
/// about a nonsense mode becomes impossible to write. See
/// `docs/protocol_newtypes.md` — the same finding as `0xBF 0x1A`'s stat lock.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawTalkMode(pub u8);

impl RawTalkMode {
    /// What the byte means. Total: every one of the 256 values is a mode, and
    /// the ones this engine has no rule for stay bytes inside
    /// [`TalkMode::Other`].
    #[must_use]
    pub const fn interpret(self) -> TalkMode {
        match self.0 & !KEYWORD_BITS {
            0 => TalkMode::Regular,
            2 => TalkMode::Emote,
            6 => TalkMode::Label,
            8 => TalkMode::Whisper,
            9 => TalkMode::Yell,
            13 => TalkMode::Guild,
            14 => TalkMode::Alliance,
            other => TalkMode::Other(other),
        }
    }

    /// True if the text after the header is preceded by a keyword block. Framing,
    /// which is why `decode_body` may read this and nothing else may.
    const fn has_keywords(self) -> bool {
        self.0 & KEYWORD_BITS != 0
    }
}

/// Which of the client's built-in fonts text is drawn in.
///
/// An index into `fonts.mul`, the client's alone: the numbers name typefaces the
/// client ships, and the server only ever picks one or repeats the one a player
/// chose. A number with a name, not an enum, for [`Layer`](crate::wire::Layer)'s
/// reason — the faces this engine has never sent would be a guess.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Font(pub u16);

impl Font {
    /// The middling face the client renders speech in when the source names
    /// none. What every stock shard sends.
    pub const DEFAULT: Self = Self(3);
}

/// A font choice exactly as a client packet carried it — not yet checked against
/// the faces this client actually has.
///
/// Same status as [`RawHue`](crate::wire::RawHue): the set is the client's files,
/// which `protocol` does not read, so the check that turns this into a real
/// [`Font`] lives above and does not exist yet. See `docs/protocol_newtypes.md`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawFont(pub u16);

/// `0x03` — the client says something. Variable length.
///
/// `mode` is how it is said — normal, emote, whisper, yell — which the server
/// uses to decide who hears it and in what colour. `font` is the client's, kept
/// so the reply can echo it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TalkRequest {
    /// How it is said.
    pub mode: RawTalkMode,
    /// The colour the client chose.
    pub hue:  RawHue,
    /// The font the client chose.
    pub font: RawFont,
    /// What was said.
    pub text: String,
}

impl DecodePacket for TalkRequest {
    const ID: u8 = 0x03;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let mode = RawTalkMode(reader.u8()?);
        let hue = RawHue(reader.u16()?);
        let font = RawFont(reader.u16()?);
        let text = reader.null_terminated_string()?;
        Ok(Self {
            mode,
            hue,
            font,
            text,
        })
    }
}

/// `0xAD` — the client says something, in Unicode. Variable length.
///
/// What a *modern* client sends when you type — the classic client sends `0x03`,
/// this is what ClassicUO and every 7.x client use, so a shard that only reads
/// `0x03` hears nothing anyone actually says.
///
/// # Two shapes, told apart by the mode's top bits
///
/// After a header — mode, hue, font, a four-byte language tag — the text comes
/// one of two ways, and `mode & 0xC0` says which:
///
/// - **Plain** (the common case, typing): big-endian UTF-16, null-terminated.
/// - **With keywords**: the client recognised trigger words ("bank", "guards")
///   and packs their ids in a bit-field first, then plain ASCII text. Ported
///   from Sphere's `PacketSpeakReqUNICODE`: read the count, skip the packed ids,
///   read the ASCII.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnicodeTalkRequest {
    /// How it is said, keyword bits and all — [`RawTalkMode::interpret`] drops
    /// them, so the byte the client sent survives to whoever wants to log it.
    pub mode: RawTalkMode,
    /// The colour the client chose.
    pub hue:  RawHue,
    /// The font the client chose.
    pub font: RawFont,
    /// What was said.
    pub text: String,
}

impl DecodePacket for UnicodeTalkRequest {
    const ID: u8 = 0xAD;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let mode = RawTalkMode(reader.u8()?);
        let hue = RawHue(reader.u16()?);
        let font = RawFont(reader.u16()?);
        let _language = reader.bytes(4)?; // e.g. "ENU\0"

        if mode.has_keywords() {
            // Keyword-encoded: a count of trigger ids, then that many packed into
            // a bit-field, then the text as ASCII. The count word is the start of
            // the field, so the whole field is `to_skip` bytes counting from it.
            let count = usize::from((reader.u16()? & 0xFFF0) >> 4);
            let bits = (count + 1) * 12;
            let to_skip = bits.div_ceil(8);
            reader.skip(to_skip.saturating_sub(2))?; // the count word is 2 of them
            let text = reader.null_terminated_string()?;
            Ok(Self {
                mode,
                hue,
                font,
                text,
            })
        } else {
            let text = utf16_be_to_string(reader.rest());
            Ok(Self {
                mode,
                hue,
                font,
                text,
            })
        }
    }
}

impl UnicodeTalkRequest {
    /// The language tag in the header. Four bytes, null-padded, and never read by
    /// this engine — the field exists because the client fills it in and the
    /// offsets after it depend on its width, not because anything acts on it.
    const LANGUAGE: [u8; 4] = *b"ENU\0";

    /// Encode a whole `0xAD`. What `crates/client/net` sends when the player
    /// types; this *server* never sends one, only ever decodes it — the same
    /// split as [`WalkRequest::encode`](crate::world::WalkRequest::encode).
    ///
    /// Always the plain form: big-endian UTF-16, null-terminated. The keyword
    /// block is the client recognising its own trigger words ("bank", "guards")
    /// against a list that lives in the client's files, and this end has no such
    /// list — so [`mode`](Self::mode) must arrive without the keyword bits, or
    /// the header would promise a block that is not there and the server would
    /// read the text as its length. Asserted in debug rather than masked: a mode
    /// silently rewritten here is a caller that thinks it sent something else.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        debug_assert!(
            !self.mode.has_keywords(),
            "0xAD encodes the plain form; mode 0x{:02X} claims a keyword block",
            self.mode.0
        );
        frame_body(
            <Self as DecodePacket>::ID,
            PacketLength::Variable,
            |out: &mut PacketWriter| {
                out.u8(self.mode.0);
                out.u16(self.hue.0);
                out.u16(self.font.0);
                out.bytes(&Self::LANGUAGE);
                out.null_terminated_string_utf16(&self.text);
            },
        )
    }
}

/// Decode big-endian UTF-16 up to a null terminator (or the end).
fn utf16_be_to_string(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let unit = u16::from_be_bytes([pair[0], pair[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

/// [`utf16_be_to_string`]'s twin: `0xC1`'s `arguments` are little-endian —
/// see [`LocalizedMessage`]'s own doc for why that is the detail a copy from
/// `0xAE` gets wrong.
fn utf16_le_to_string(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let unit = u16::from_le_bytes([pair[0], pair[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

/// `0x1C` — draw speech over a source and put it in the journal. Variable length.
///
/// Ported from Sphere's `PacketMessageASCII`. A speaker of `None` is "the system
/// talking", not a mobile; a real speaker sends its own serial, its body graphic,
/// and its name in the fixed thirty-byte field.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SpokenMessage {
    /// The speaker, or `None` for the system.
    pub serial:  Option<Serial>,
    /// The speaker's body graphic, or `None` for no mobile behind it.
    pub graphic: Option<Graphic>,
    /// How it is said.
    pub mode:    TalkMode,
    /// The colour to draw it in.
    pub hue:     Hue,
    /// The font to draw it in.
    pub font:    Font,
    /// The speaker's name, clamped to thirty bytes.
    pub name:    String,
    /// What was said.
    pub text:    String,
}

impl EncodePacket for SpokenMessage {
    const ID: u8 = 0x1C;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(serial_or_system(self.serial));
        out.u16(graphic_or_none(self.graphic));
        out.u8(self.mode.to_wire());
        out.u16(self.hue.0);
        out.u16(self.font.0);
        out.fixed_string(&self.name, NAME_LENGTH);
        out.null_terminated_string(&self.text);
    }
}

impl DecodePacket for SpokenMessage {
    const ID: u8 = 0x1C;

    /// The client's direction of `0x1C`, written because our own client has to
    /// read what the shard says — and the first thing it has to read is the
    /// shard telling it that it is going away (`docs/shutdown.md` S3).
    ///
    /// The two sentinels are folded back into `None` here, exactly as
    /// [`serial_or_system`] and [`graphic_or_none`] fold them out: a message from
    /// the system is not a message from mobile `0xFFFFFFFF`, and losing that
    /// distinction in the decoder would mean the client had to know the sentinel
    /// too.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let serial = match reader.u32()? {
            SYSTEM_SERIAL => None,
            // A speaker outside the serial ranges is not something to guess at:
            // the sentinel is the *only* legal way to say "nobody said this".
            raw => {
                Some(Serial::new(raw).ok_or(DecodeError::UnknownValue {
                    field: "0x1C speaker serial",
                    value: raw,
                })?)
            }
        };
        let graphic = match reader.u16()? {
            NO_GRAPHIC => None,
            raw => Some(Graphic(raw)),
        };
        let mode = RawTalkMode(reader.u8()?).interpret();
        let hue = Hue(reader.u16()?);
        let font = Font(reader.u16()?);
        let name = reader.fixed_string(NAME_LENGTH)?;
        let text = reader.null_terminated_string()?;
        Ok(Self {
            serial,
            graphic,
            mode,
            hue,
            font,
            name,
            text,
        })
    }
}

/// `0xC1` — a **localized** message: the client looks the text up in its own
/// `cliloc.enu` and draws it, so nothing but a number travels.
///
/// The workhorse of every stock line the client already has a translation for —
/// "That skill cannot be used directly", "You have no free hands", the whole
/// skill and craft vocabulary. Ported from ServUO's `MessageLocalized`.
///
/// `arguments` are the `\t`-separated substitutions the cliloc's `~1_val~` slots
/// take, and they are UTF-16 **little-endian** — ServUO's `WriteLittleUniNull`.
/// That is the opposite of the `0xAE` speech packet below, which is big-endian,
/// and it is exactly the detail a copy from there gets wrong: the client draws
/// an empty line and says nothing about why.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LocalizedMessage {
    /// The speaker, or `None` for the system.
    pub serial:    Option<Serial>,
    /// The speaker's body graphic, or `None` for no mobile behind it.
    pub graphic:   Option<Graphic>,
    /// How it is said.
    pub mode:      TalkMode,
    /// The colour to draw it in.
    pub hue:       Hue,
    /// The font to draw it in.
    pub font:      Font,
    /// The cliloc the client looks up and draws.
    pub cliloc:    ClilocId,
    /// The speaker's name, clamped to thirty bytes.
    pub name:      String,
    /// The `\t`-separated substitutions for the cliloc's `~1_val~` slots.
    pub arguments: String,
}

impl EncodePacket for LocalizedMessage {
    const ID: u8 = 0xC1;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(serial_or_system(self.serial));
        out.u16(graphic_or_none(self.graphic));
        out.u8(self.mode.to_wire());
        out.u16(self.hue.0);
        out.u16(self.font.0);
        out.u32(self.cliloc.0);
        out.fixed_string(&self.name, NAME_LENGTH);
        for unit in self.arguments.encode_utf16() {
            out.u16(unit.swap_bytes()); // little-endian, unlike 0xAE below
        }
        out.u16(0);
    }
}

impl DecodePacket for LocalizedMessage {
    const ID: u8 = 0xC1;

    /// The client's direction of `0xC1` — this server never sends one to
    /// decode, but a client reading one back is exactly how
    /// `use_skill_button`'s "cannot be used directly" line, and every other
    /// gate that answers through [`WorldState::localized_message`], is
    /// observed at all. Mirrors [`SpokenMessage::decode_body`]'s sentinel
    /// folding for `serial`/`graphic`, and reads `arguments` as
    /// little-endian — see the struct's own doc for why that is not
    /// [`utf16_be_to_string`].
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let serial = match reader.u32()? {
            SYSTEM_SERIAL => None,
            raw => {
                Some(Serial::new(raw).ok_or(DecodeError::UnknownValue {
                    field: "0xC1 speaker serial",
                    value: raw,
                })?)
            }
        };
        let graphic = match reader.u16()? {
            NO_GRAPHIC => None,
            raw => Some(Graphic(raw)),
        };
        let mode = RawTalkMode(reader.u8()?).interpret();
        let hue = Hue(reader.u16()?);
        let font = Font(reader.u16()?);
        let cliloc = ClilocId(reader.u32()?);
        let name = reader.fixed_string(NAME_LENGTH)?;
        let arguments = utf16_le_to_string(reader.rest());
        Ok(Self {
            serial,
            graphic,
            mode,
            hue,
            font,
            cliloc,
            name,
            arguments,
        })
    }
}

/// The language tag in a `0xAE` is a fixed four bytes, ASCII with a NUL.
const LANGUAGE_LENGTH: usize = 4;
/// The default language a `0xAE` carries when the source did not name one.
const DEFAULT_LANGUAGE: &str = "ENU";

/// `0xAE` — draw *Unicode* speech over a source and put it in the journal.
/// Variable length.
///
/// The counterpart of [`SpokenMessage`] for text `0x1C`'s Latin-1 cannot carry:
/// an accent, a non-Latin script. Ported from Sphere's `PacketMessageUNICODE`,
/// the layout matches `0x1C` up to the name, then adds a four-byte language tag
/// before it and sends the text as big-endian UTF-16 rather than ASCII. A client
/// that spoke `0xAD` expects its own words back this way — the reason a shard
/// whose players type accented names needs it and the ASCII path is not enough.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UnicodeMessage {
    /// The speaker, or `None` for the system.
    pub serial:   Option<Serial>,
    /// The speaker's body graphic, or `None` for no mobile behind it.
    pub graphic:  Option<Graphic>,
    /// How it is said.
    pub mode:     TalkMode,
    /// The colour to draw it in.
    pub hue:      Hue,
    /// The font to draw it in.
    pub font:     Font,
    /// The four-byte language tag, e.g. `"ENU"`. See [`DEFAULT_LANGUAGE_TAG`].
    pub language: String,
    /// The speaker's name, clamped to thirty bytes.
    pub name:     String,
    /// What was said.
    pub text:     String,
}

impl EncodePacket for UnicodeMessage {
    const ID: u8 = 0xAE;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(serial_or_system(self.serial));
        out.u16(graphic_or_none(self.graphic));
        out.u8(self.mode.to_wire());
        out.u16(self.hue.0);
        out.u16(self.font.0);
        out.fixed_string(&self.language, LANGUAGE_LENGTH);
        out.fixed_string(&self.name, NAME_LENGTH);
        out.null_terminated_string_utf16(&self.text);
    }
}

impl DecodePacket for UnicodeMessage {
    const ID: u8 = 0xAE;

    /// The client's direction of `0xAE`, written for the same reason
    /// [`SpokenMessage::decode_body`] was: our own client has to read what the
    /// shard draws over a head, and a client that spoke `0xAD` gets exactly this
    /// packet back — see the module doc. The layout matches `0x1C` up to the
    /// name, then the language tag, then big-endian UTF-16 rather than ASCII.
    ///
    /// The two sentinels fold to `None` here exactly as they do in
    /// [`SpokenMessage::decode_body`] — see that function's docs for why.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let serial = match reader.u32()? {
            SYSTEM_SERIAL => None,
            raw => {
                Some(Serial::new(raw).ok_or(DecodeError::UnknownValue {
                    field: "0xAE speaker serial",
                    value: raw,
                })?)
            }
        };
        let graphic = match reader.u16()? {
            NO_GRAPHIC => None,
            raw => Some(Graphic(raw)),
        };
        let mode = RawTalkMode(reader.u8()?).interpret();
        let hue = Hue(reader.u16()?);
        let font = Font(reader.u16()?);
        let language = reader.fixed_string(LANGUAGE_LENGTH)?;
        let name = reader.fixed_string(NAME_LENGTH)?;
        let text = utf16_be_to_string(reader.rest());
        Ok(Self {
            serial,
            graphic,
            mode,
            hue,
            font,
            language,
            name,
            text,
        })
    }
}

/// The default four-byte language tag (`"ENU\0"`) for a Unicode message whose
/// source named none. Exposed so the world need not restate the fallback.
pub const DEFAULT_LANGUAGE_TAG: &str = DEFAULT_LANGUAGE;

/// The serial that marks a message as coming from the system rather than a
/// mobile — ServUO's `Serial.MinusOne`, Sphere's `UID_CLEAR`.
///
/// Not [`NO_SERIAL`](crate::serial::NO_SERIAL)'s zero, which is what every
/// *object reference* field writes for "nothing": these packets have their own
/// sentinel and the client reads them differently — a `0` here names an object
/// the client does not have, and the words are drawn nowhere.
const SYSTEM_SERIAL: u32 = 0xFFFF_FFFF;

/// The graphic that marks a message as having no mobile behind it.
const NO_GRAPHIC: u16 = 0xFFFF;

/// The wire value of `serial`, or [`SYSTEM_SERIAL`] for the system talking.
const fn serial_or_system(serial: Option<Serial>) -> u32 {
    match serial {
        Some(serial) => serial.raw(),
        None => SYSTEM_SERIAL,
    }
}

/// The wire value of `graphic`, or [`NO_GRAPHIC`] for no mobile behind it.
const fn graphic_or_none(graphic: Option<Graphic>) -> u16 {
    match graphic {
        Some(graphic) => graphic.0,
        None => NO_GRAPHIC,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{
        decode_packet,
        encode_packet,
    };
    use crate::server_packet::ServerPacket;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    #[test]
    fn a_localized_message_carries_a_number_and_little_endian_arguments() {
        // 1042764 is "~1_NAME~ seems to belong to someone else." — one slot.
        let packet = encode_packet(
            &LocalizedMessage {
                serial:    None,
                graphic:   None,
                mode:      TalkMode::Regular,
                hue:       Hue(0x03B2),
                font:      Font(3),
                cliloc:    ClilocId(1_042_764),
                name:      "System".to_owned(),
                arguments: "Iolo".to_owned(),
            },
            version(),
        );
        assert_eq!(packet[0], 0xC1);
        assert_eq!(
            u16::from_be_bytes([packet[1], packet[2]]) as usize,
            packet.len(),
            "the length field matches the packet"
        );
        // id(1) length(2) serial(4) graphic(2) mode(1) hue(2) font(2) → cliloc at 14.
        assert_eq!(
            u32::from_be_bytes([packet[14], packet[15], packet[16], packet[17]]),
            1_042_764
        );
        // Then thirty bytes of name, and the arguments after them.
        let args = &packet[48..];
        let expected: Vec<u8> = "Iolo"
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            args, expected,
            "the arguments are UTF-16 little-endian, not the big-endian 0xAE uses"
        );
    }

    /// The gap this crate had until now: `LocalizedMessage` had
    /// `EncodePacket` but no `DecodePacket`, and no arm in
    /// `ServerPacket::decode` — so a client asking for this cliloc, the one
    /// `use_skill_button`'s "cannot be used directly" line and every other
    /// gate through `WorldState::localized_message` answers through, read it
    /// as `Unknown` and dropped it. Routed through the real dispatcher, the
    /// same way `a_lock_click_reaches_the_server_as_the_request_it_means`
    /// proves its own encoder, so a missing arm here fails this test and not
    /// just a decode-only one.
    #[test]
    fn a_localized_message_decodes_back_through_the_dispatcher() {
        let sent = LocalizedMessage {
            serial:    None,
            graphic:   None,
            mode:      TalkMode::Regular,
            hue:       Hue(0x03B2),
            font:      Font(3),
            cliloc:    ClilocId(500_014),
            name:      "System".to_owned(),
            arguments: "Iolo".to_owned(),
        };
        let bytes = encode_packet(&sent, version());
        let decoded = ServerPacket::decode(&bytes, version())
            .expect("it decodes")
            .expect("0xC1 has an arm now");
        assert!(matches!(
            decoded,
            ServerPacket::LocalizedMessage(message) if message == sent
        ));
    }

    /// The client's side of `0x1C`, round-tripped against our own encoder.
    ///
    /// Circular on its own, which is why the two sentinels are what it is really
    /// about: the system's `0xFFFFFFFF` speaker and `0xFFFF` graphic go out as
    /// `None` and must come back as `None`. A decoder that read them literally
    /// would produce a message from mobile 4294967295, and every caller would
    /// then have to know the sentinel — which is the knowledge the `Option`
    /// exists to hold in one place.
    #[test]
    fn a_spoken_message_decodes_back_to_what_was_said() {
        let said = SpokenMessage {
            serial:  None,
            graphic: None,
            mode:    TalkMode::Regular,
            hue:     Hue(0x03B2),
            font:    Font(3),
            name:    "System".to_owned(),
            text:    "The shard is shutting down.".to_owned(),
        };
        let packet = encode_packet(&said, version());
        assert_eq!(packet[0], 0x1C);

        // Through `ServerPacket::decode` and not the bare `decode_packet`: this
        // is a variable-length packet, so the two length bytes after the id have
        // to be skipped, and that skip lives in the server-packet path a client
        // actually uses.
        let Some(ServerPacket::SpokenMessage(read)) = ServerPacket::decode(&packet, version()).unwrap()
        else {
            panic!("0x1C decoded as something else");
        };
        assert_eq!(read, said);
        assert_eq!(read.serial, None, "the system talking is not mobile 0xFFFFFFFF");
        assert_eq!(read.graphic, None, "and no body stands behind it");
    }

    /// A real speaker survives too — the branch the test above does not take.
    #[test]
    fn a_spoken_message_keeps_a_real_speaker() {
        let said = SpokenMessage {
            serial:  Serial::new(0x0000_0123),
            graphic: Some(Graphic(0x0190)),
            mode:    TalkMode::Emote,
            hue:     Hue(0x0022),
            font:    Font(3),
            name:    "Iolo".to_owned(),
            text:    "hail".to_owned(),
        };
        let packet = encode_packet(&said, version());
        let Some(ServerPacket::SpokenMessage(read)) = ServerPacket::decode(&packet, version()).unwrap()
        else {
            panic!("0x1C decoded as something else");
        };
        assert_eq!(read, said);
    }

    #[test]
    fn a_talk_request_reads_its_parts() {
        // 0x03, length, mode=0, hue=0x0384, font=3, "hi\0"
        let mut bytes = vec![0x03];
        let body_len = 8 + 3; // header 8 + "hi\0"
        bytes.extend_from_slice(&(body_len as u16).to_be_bytes());
        bytes.push(0x00); // mode
        bytes.extend_from_slice(&0x0384u16.to_be_bytes()); // hue
        bytes.extend_from_slice(&3u16.to_be_bytes()); // font
        bytes.extend_from_slice(b"hi\0");

        let talk: TalkRequest = decode_packet(&bytes, version()).unwrap();
        assert_eq!(talk.mode, RawTalkMode(0));
        assert_eq!(talk.hue, RawHue(0x0384));
        assert_eq!(talk.font, RawFont(3));
        assert_eq!(talk.text, "hi");
    }

    #[test]
    fn a_unicode_talk_request_reads_plain_text() {
        // What a modern client sends typing "hi": header, then UTF-16 BE "hi\0".
        let mut bytes = vec![0xAD];
        // header: mode 0, hue 0x0384, font 3, language "ENU\0", then "hi\0" in UTF-16
        let mut body = Vec::new();
        body.push(0x00); // mode (no keyword bits)
        body.extend_from_slice(&0x0384u16.to_be_bytes()); // hue
        body.extend_from_slice(&3u16.to_be_bytes()); // font
        body.extend_from_slice(b"ENU\0"); // language
        body.extend_from_slice(&[0x00, b'h', 0x00, b'i', 0x00, 0x00]); // "hi\0" UTF-16 BE
        let total = 1 + 2 + body.len();
        bytes.extend_from_slice(&(total as u16).to_be_bytes());
        bytes.extend_from_slice(&body);

        let talk: UnicodeTalkRequest = decode_packet(&bytes, version()).unwrap();
        assert_eq!(talk.mode, RawTalkMode(0));
        assert_eq!(talk.hue, RawHue(0x0384));
        assert_eq!(talk.font, RawFont(3));
        assert_eq!(talk.text, "hi");
    }

    #[test]
    fn what_the_client_types_comes_back_out_of_the_decoder() {
        // The one thing worth asserting about the encoder: it is the server's own
        // decoder that has to read it, and a header field written in the wrong
        // width desynchronises the text rather than failing. Non-ASCII on purpose
        // — UTF-16 is the whole reason a modern client sends 0xAD at all.
        let said = UnicodeTalkRequest {
            mode: RawTalkMode(TalkMode::Regular.to_wire()),
            hue:  RawHue(0x0384),
            font: RawFont(3),
            text: "привет .admin".to_owned(),
        };
        let bytes = said.encode();
        assert_eq!(bytes[0], 0xAD);
        assert_eq!(
            u16::from_be_bytes([bytes[1], bytes[2]]) as usize,
            bytes.len(),
            "the length field counts the whole packet, itself and the id included"
        );
        let heard: UnicodeTalkRequest = decode_packet(&bytes, version()).unwrap();
        assert_eq!(heard, said);
    }

    #[test]
    fn a_unicode_talk_request_skips_the_keyword_block() {
        // mode with the keyword bit set: a count word, packed ids, then ASCII.
        let mut bytes = vec![0xAD];
        let mut body = Vec::new();
        body.push(0xC0); // keyword bit set, base mode 0
        body.extend_from_slice(&0u16.to_be_bytes()); // hue
        body.extend_from_slice(&3u16.to_be_bytes()); // font
        body.extend_from_slice(b"ENU\0"); // language
        // one keyword: count=1 -> (1<<4) in the top 12 bits -> bytes to skip = ceil((1+1)*12/8)=3
        body.extend_from_slice(&(1u16 << 4).to_be_bytes()); // count word (2 bytes)
        body.push(0x00); // the third keyword byte
        body.extend_from_slice(b"hello\0"); // ASCII text
        let total = 1 + 2 + body.len();
        bytes.extend_from_slice(&(total as u16).to_be_bytes());
        bytes.extend_from_slice(&body);

        let talk: UnicodeTalkRequest = decode_packet(&bytes, version()).unwrap();
        assert_eq!(
            talk.mode,
            RawTalkMode(0xC0),
            "decoding reads the keyword bits for the framing and leaves the byte alone"
        );
        assert_eq!(
            talk.mode.interpret(),
            TalkMode::Regular,
            "interpretation is where they are dropped"
        );
        assert_eq!(talk.text, "hello");
    }

    #[test]
    fn a_message_lays_out_its_header_and_pads_the_name() {
        let packet = encode_packet(
            &SpokenMessage {
                serial:  Serial::new(0x0000_0002),
                graphic: Some(Graphic(0x0190)),
                mode:    TalkMode::Regular,
                hue:     Hue(0x0384),
                font:    Font(3),
                name:    "British".to_owned(),
                text:    "hail".to_owned(),
            },
            version(),
        );
        assert_eq!(packet[0], 0x1C);
        assert_eq!(u16::from_be_bytes([packet[1], packet[2]]), packet.len() as u16);
        assert_eq!(&packet[3..7], &0x0000_0002u32.to_be_bytes());
        assert_eq!(&packet[7..9], &0x0190u16.to_be_bytes());
        assert_eq!(packet[9], 0); // mode
        assert_eq!(&packet[10..12], &0x0384u16.to_be_bytes()); // hue
        assert_eq!(&packet[12..14], &3u16.to_be_bytes()); // font
        // 30-byte name, "British" then zeros
        assert_eq!(&packet[14..21], b"British");
        assert_eq!(packet[21], 0);
        assert_eq!(&packet[44..48], b"hail");
        assert_eq!(packet[48], 0, "the text is null-terminated");
    }

    #[test]
    fn a_unicode_message_carries_its_text_as_big_endian_utf16() {
        // The whole reason `0xAE` exists: text `0x1C`'s Latin-1 cannot hold. A
        // Portuguese "olá" comes back with the accented letter intact.
        let packet = encode_packet(
            &UnicodeMessage {
                serial:   Serial::new(0x0000_0002),
                graphic:  Some(Graphic(0x0190)),
                mode:     TalkMode::Regular,
                hue:      Hue(0x0384),
                font:     Font(3),
                language: "PTB".to_owned(),
                name:     "Cidadão".to_owned(),
                text:     "olá".to_owned(),
            },
            version(),
        );
        assert_eq!(packet[0], 0xAE);
        assert_eq!(u16::from_be_bytes([packet[1], packet[2]]), packet.len() as u16);
        assert_eq!(&packet[3..7], &0x0000_0002u32.to_be_bytes());
        assert_eq!(&packet[7..9], &0x0190u16.to_be_bytes());
        assert_eq!(packet[9], 0); // mode
        assert_eq!(&packet[10..12], &0x0384u16.to_be_bytes()); // hue
        assert_eq!(&packet[12..14], &3u16.to_be_bytes()); // font
        assert_eq!(&packet[14..18], b"PTB\0", "the four-byte language tag");
        // 30-byte name field begins at 18; "Cidadão" narrows to Latin-1 here.
        assert_eq!(&packet[18..25], b"Cidad\xE3o");
        // The text runs from offset 48, one big-endian UTF-16 unit per char.
        let text_units: Vec<u16> = packet[48..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u16::from_be_bytes([p[0], p[1]]))
            .collect();
        let mut expected: Vec<u16> = "olá".encode_utf16().collect();
        expected.push(0);
        assert_eq!(text_units, expected, "big-endian UTF-16, null-terminated");
    }

    /// The client's side of `0xAE`, round-tripped against our own encoder, with
    /// non-ASCII text — the whole reason `0xAE` exists rather than `0x1C`
    /// carrying everything.
    ///
    /// This is the packet a client that spoke `0xAD` gets back for its *own*
    /// words (see the module doc), so a client that could not decode it would
    /// never see what it just typed — the bug this test exists to close.
    #[test]
    fn a_unicode_message_decodes_back_to_what_was_said() {
        let said = UnicodeMessage {
            serial:   Serial::new(0x0000_0123),
            graphic:  Some(Graphic(0x0190)),
            mode:     TalkMode::Regular,
            hue:      Hue(0x0022),
            font:     Font(3),
            language: "RUS".to_owned(),
            name:     "Иванов".to_owned(),
            text:     "привет .admin".to_owned(),
        };
        let packet = encode_packet(&said, version());
        assert_eq!(packet[0], 0xAE);

        let Some(ServerPacket::UnicodeMessage(read)) = ServerPacket::decode(&packet, version()).unwrap()
        else {
            panic!("0xAE decoded as something else");
        };
        assert_eq!(read.serial, said.serial);
        assert_eq!(read.graphic, said.graphic);
        assert_eq!(read.mode, said.mode);
        assert_eq!(read.hue, said.hue);
        assert_eq!(read.font, said.font);
        assert_eq!(read.language, "RUS", "the language tag round-trips");
        // The name field narrows to Latin-1 on the wire (fixed_string), so a
        // Cyrillic name does not survive it — only the text, sent as UTF-16,
        // does. That is the whole point of comparing text and not name here.
        assert_eq!(read.text, said.text, "UTF-16 text survives what Latin-1 cannot");
    }

    /// The system talking through `0xAE` folds the same sentinels `0x1C` does —
    /// see [`SpokenMessage::decode_body`]'s docs for why that fold matters.
    #[test]
    fn a_system_unicode_message_has_no_mobile_behind_it() {
        let packet = encode_packet(
            &UnicodeMessage {
                serial:   None,
                graphic:  None,
                mode:     TalkMode::Regular,
                hue:      Hue::NONE,
                font:     Font::DEFAULT,
                language: DEFAULT_LANGUAGE_TAG.to_owned(),
                name:     "System".to_owned(),
                text:     "welcome".to_owned(),
            },
            version(),
        );
        let Some(ServerPacket::UnicodeMessage(read)) = ServerPacket::decode(&packet, version()).unwrap()
        else {
            panic!("0xAE decoded as something else");
        };
        assert_eq!(read.serial, None, "the system talking is not mobile 0xFFFFFFFF");
        assert_eq!(read.graphic, None, "and no body stands behind it");
        assert_eq!(read.text, "welcome");
    }

    #[test]
    fn a_system_message_has_no_mobile_behind_it() {
        let packet = encode_packet(
            &SpokenMessage {
                serial:  None,
                graphic: None,
                mode:    TalkMode::Regular,
                hue:     Hue::NONE,
                font:    Font::DEFAULT,
                name:    "System".to_owned(),
                text:    "welcome".to_owned(),
            },
            version(),
        );
        assert_eq!(
            u32::from_be_bytes([packet[3], packet[4], packet[5], packet[6]]),
            SYSTEM_SERIAL
        );
        assert_eq!(u16::from_be_bytes([packet[7], packet[8]]), NO_GRAPHIC);
    }

    #[test]
    fn every_mode_byte_interprets_and_survives_the_round_trip() {
        // Class B is total by construction — there is no byte a client can send
        // that has no meaning here — and the leftover arm keeps the byte, so a
        // mode this engine has no rule for still goes back out as itself.
        for byte in 0..=u8::MAX {
            let mode = RawTalkMode(byte).interpret();
            assert_eq!(
                mode.to_wire(),
                byte & !KEYWORD_BITS,
                "0x{byte:02X} round-trips, less the framing bits"
            );
        }
    }

    #[test]
    fn the_keyword_bits_are_framing_and_not_part_of_the_mode() {
        // `0xC8` is a whisper with the keyword block set: the same mode as `0x08`,
        // told apart only by what follows the header.
        assert_eq!(RawTalkMode(0xC8).interpret(), TalkMode::Whisper);
        assert_eq!(RawTalkMode(0x08).interpret(), TalkMode::Whisper);
        assert!(RawTalkMode(0xC8).has_keywords());
        assert!(!RawTalkMode(0x08).has_keywords());
    }

    #[test]
    fn an_unnamed_mode_stays_the_byte_it_was() {
        // ServUO's `MessageType.Spell` is 10 and this engine has no rule for it;
        // the point of the leftover arm is that it neither becomes `Regular` nor
        // disappears.
        assert_eq!(RawTalkMode(10).interpret(), TalkMode::Other(10));
        assert_eq!(TalkMode::Other(10).to_wire(), 10);
    }
}
