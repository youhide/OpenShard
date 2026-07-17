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

use crate::codec::PacketWriter;
use crate::login::{expect_id, LoginDecodeError};

/// The name field in a `0x1C` is a fixed thirty bytes.
const NAME_LENGTH: usize = 30;

/// `0x03` — the client says something. Variable length.
///
/// `mode` is how it is said — normal, emote, whisper, yell — which the server
/// uses to decide who hears it and in what colour. `font` is the client's, kept
/// so the reply can echo it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TalkRequest {
    /// How it is said (0 normal, 2 emote, 8 whisper, 9 yell, …).
    pub mode: u8,
    /// The colour the client chose.
    pub hue: u16,
    /// The font the client chose.
    pub font: u16,
    /// What was said.
    pub text: String,
}

impl TalkRequest {
    /// The packet id.
    pub const ID: u8 = 0x03;

    /// Decode a whole `0x03` packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        let mode = reader.u8()?;
        let hue = reader.u16()?;
        let font = reader.u16()?;
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
    /// How it is said, with the keyword bits already stripped.
    pub mode: u8,
    /// The colour the client chose.
    pub hue: u16,
    /// The font the client chose.
    pub font: u16,
    /// What was said.
    pub text: String,
}

impl UnicodeTalkRequest {
    /// The packet id.
    pub const ID: u8 = 0xAD;

    /// Decode a whole `0xAD` packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        let mode = reader.u8()?;
        let hue = reader.u16()?;
        let font = reader.u16()?;
        let _language = reader.bytes(4)?; // e.g. "ENU\0"

        if mode & 0xC0 != 0 {
            // Keyword-encoded: a count of trigger ids, then that many packed into
            // a bit-field, then the text as ASCII. The count word is the start of
            // the field, so the whole field is `to_skip` bytes counting from it.
            let count = usize::from((reader.u16()? & 0xFFF0) >> 4);
            let bits = (count + 1) * 12;
            let to_skip = bits.div_ceil(8);
            reader.skip(to_skip.saturating_sub(2))?; // the count word is 2 of them
            let text = reader.null_terminated_string()?;
            Ok(Self {
                mode: mode & !0xC0,
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

/// Decode big-endian UTF-16 up to a null terminator (or the end).
fn utf16_be_to_string(bytes: &[u8]) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let unit = u16::from_be_bytes([pair[0], pair[1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

/// `0x1C` — draw speech over a source and put it in the journal. Variable length.
///
/// Ported from Sphere's `PacketMessageASCII`. A `serial` of `0xFFFFFFFF` and a
/// `graphic` of `0xFFFF` are "the system talking", not a mobile; a real speaker
/// sends its own serial, its body graphic, and its name in the fixed thirty-byte
/// field.
pub fn encode_message(
    serial: u32,
    graphic: u16,
    mode: u8,
    hue: u16,
    font: u16,
    name: &str,
    text: &str,
) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(44 + text.len());
    writer.u8(0x1C);
    writer.u16(0); // length, patched below
    writer.u32(serial);
    writer.u16(graphic);
    writer.u8(mode);
    writer.u16(hue);
    writer.u16(font);
    writer.fixed_string(name, NAME_LENGTH);
    writer.null_terminated_string(text);

    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a message outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

/// The language tag in a `0xAE` is a fixed four bytes, ASCII with a NUL.
const LANGUAGE_LENGTH: usize = 4;
/// The default language a `0xAE` carries when the source did not name one.
const DEFAULT_LANGUAGE: &str = "ENU";

/// `0xAE` — draw *Unicode* speech over a source and put it in the journal.
/// Variable length.
///
/// The counterpart of [`encode_message`] for text `0x1C`'s Latin-1 cannot carry:
/// an accent, a non-Latin script. Ported from Sphere's `PacketMessageUNICODE`,
/// the layout matches `0x1C` up to the name, then adds a four-byte language tag
/// before it and sends the text as big-endian UTF-16 rather than ASCII. A client
/// that spoke `0xAD` expects its own words back this way — the reason a shard
/// whose players type accented names needs it and the ASCII path is not enough.
// One argument past clippy's limit, and every one is a distinct wire field —
// `encode_message`'s seven plus the language tag. Bundling them into a struct
// would only move the same list one call up.
#[allow(clippy::too_many_arguments)]
pub fn encode_unicode_message(
    serial: u32,
    graphic: u16,
    mode: u8,
    hue: u16,
    font: u16,
    language: &str,
    name: &str,
    text: &str,
) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(50 + text.len() * 2);
    writer.u8(0xAE);
    writer.u16(0); // length, patched below
    writer.u32(serial);
    writer.u16(graphic);
    writer.u8(mode);
    writer.u16(hue);
    writer.u16(font);
    writer.fixed_string(language, LANGUAGE_LENGTH);
    writer.fixed_string(name, NAME_LENGTH);
    writer.null_terminated_string_utf16(text);

    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a message outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

/// The default four-byte language tag (`"ENU\0"`) for a Unicode message whose
/// source named none. Exposed so the world need not restate the fallback.
pub const DEFAULT_LANGUAGE_TAG: &str = DEFAULT_LANGUAGE;

/// The serial and graphic that mark a `0x1C` as coming from the system rather
/// than a mobile.
pub const SYSTEM_SERIAL: u32 = 0xFFFF_FFFF;
/// The graphic that marks a `0x1C` as having no mobile behind it.
pub const NO_GRAPHIC: u16 = 0xFFFF;

#[cfg(test)]
mod tests {
    use super::*;

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

        let talk = TalkRequest::decode(&bytes).unwrap();
        assert_eq!(talk.mode, 0);
        assert_eq!(talk.hue, 0x0384);
        assert_eq!(talk.font, 3);
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

        let talk = UnicodeTalkRequest::decode(&bytes).unwrap();
        assert_eq!(talk.mode, 0);
        assert_eq!(talk.hue, 0x0384);
        assert_eq!(talk.font, 3);
        assert_eq!(talk.text, "hi");
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

        let talk = UnicodeTalkRequest::decode(&bytes).unwrap();
        assert_eq!(talk.mode, 0, "the keyword bits are stripped");
        assert_eq!(talk.text, "hello");
    }

    #[test]
    fn a_message_lays_out_its_header_and_pads_the_name() {
        let packet = encode_message(0x0000_0002, 0x0190, 0, 0x0384, 3, "British", "hail");
        assert_eq!(packet[0], 0x1C);
        assert_eq!(
            u16::from_be_bytes([packet[1], packet[2]]),
            packet.len() as u16
        );
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
        let packet =
            encode_unicode_message(0x0000_0002, 0x0190, 0, 0x0384, 3, "PTB", "Cidadão", "olá");
        assert_eq!(packet[0], 0xAE);
        assert_eq!(
            u16::from_be_bytes([packet[1], packet[2]]),
            packet.len() as u16
        );
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
            .chunks_exact(2)
            .map(|p| u16::from_be_bytes([p[0], p[1]]))
            .collect();
        let mut expected: Vec<u16> = "olá".encode_utf16().collect();
        expected.push(0);
        assert_eq!(text_units, expected, "big-endian UTF-16, null-terminated");
    }

    #[test]
    fn a_system_message_has_no_mobile_behind_it() {
        let packet = encode_message(SYSTEM_SERIAL, NO_GRAPHIC, 0, 0, 3, "System", "welcome");
        assert_eq!(
            u32::from_be_bytes([packet[3], packet[4], packet[5], packet[6]]),
            SYSTEM_SERIAL
        );
        assert_eq!(u16::from_be_bytes([packet[7], packet[8]]), NO_GRAPHIC);
    }
}
