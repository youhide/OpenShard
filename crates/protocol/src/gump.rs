//! Generic gumps: the server-driven dialog (`0xB0`) and the client's reply
//! (`0xB1`).
//!
//! A gump is UO's windowing primitive — a paperdoll, a shopkeeper, a book, and
//! the staff menus this shard needs are all gumps. The server sends a *layout*: a
//! little command language (`{ resizepic ... }`, `{ button ... }`, `{ page N }`)
//! plus a list of text strings the layout refers to by index. The client renders
//! it and, when the player clicks a button, sends back the button id in a `0xB1`.
//!
//! This is the uncompressed `0xB0` form. Clients from 5.0 also accept a zlib-packed
//! `0xDD`, and ClassicUO and the modern 2D client both still render `0xB0`, so the
//! compression is a later optimisation, not a requirement.

use crate::codec::PacketWriter;
use crate::login::{expect_id, LoginDecodeError};

/// `0xB0` — display a generic gump. Variable length.
///
/// `serial` is the context the gump belongs to (a mobile, an item, or `0` for a
/// standalone dialog); `gump_id` names *which* dialog, and the client echoes both
/// back in its `0xB1` so the server knows what was answered. `layout` is the gump
/// command string; `lines` are the strings it references by index (`{ text X Y
/// LINE }` picks `lines[LINE]`), sent as big-endian UTF-16.
pub fn encode_gump_display(
    serial: u32,
    gump_id: u32,
    x: i32,
    y: i32,
    layout: &str,
    lines: &[String],
) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(64 + layout.len());
    writer.u8(0xB0);
    writer.u16(0); // length, patched below
    writer.u32(serial);
    writer.u32(gump_id);
    writer.u32(x as u32);
    writer.u32(y as u32);

    // The layout is ASCII with a byte-count prefix; the client reads exactly that
    // many bytes, so no terminator is needed.
    let layout = layout.as_bytes();
    writer.u16(u16::try_from(layout.len()).unwrap_or(u16::MAX));
    writer.bytes(layout);

    // The text table: a count, then each line as its own UTF-16 run.
    writer.u16(u16::try_from(lines.len()).unwrap_or(u16::MAX));
    for line in lines {
        let units: Vec<u16> = line.encode_utf16().collect();
        writer.u16(u16::try_from(units.len()).unwrap_or(u16::MAX));
        for unit in units {
            writer.u16(unit); // big-endian, like every u16 on the wire
        }
    }

    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a gump outgrew its u16 length field");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

/// `0xB1` — the client's answer to a gump: which button was pressed, plus the
/// state of any switches (checkboxes, radios) and text fields.
///
/// A button of `0` is the close box — the player dismissed the gump without
/// choosing. Otherwise it is the id the layout gave the button that was clicked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GumpResponse {
    /// The context serial the gump was opened on.
    pub serial: u32,
    /// Which dialog answered — the `gump_id` the server sent.
    pub gump_id: u32,
    /// The button pressed; `0` means the gump was closed without a choice.
    pub button: u32,
    /// The ids of the switches (checkboxes, radio buttons) left *on*.
    pub switches: Vec<u32>,
    /// Text fields, as `(field id, contents)`.
    pub text_entries: Vec<(u16, String)>,
}

impl GumpResponse {
    /// The packet id.
    pub const ID: u8 = 0xB1;

    /// Decode a whole `0xB1` packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        // The 0xB1 carries its own u16 length at offset 1; skip it — the framer
        // already sized the slice.
        reader.u16()?;
        let serial = reader.u32()?;
        let gump_id = reader.u32()?;
        let button = reader.u32()?;

        let switch_count = reader.u32()? as usize;
        let mut switches = Vec::with_capacity(switch_count.min(64));
        for _ in 0..switch_count {
            switches.push(reader.u32()?);
        }

        let text_count = reader.u32()? as usize;
        let mut text_entries = Vec::with_capacity(text_count.min(64));
        for _ in 0..text_count {
            let id = reader.u16()?;
            let len = reader.u16()? as usize;
            let mut units = Vec::with_capacity(len);
            for _ in 0..len {
                units.push(reader.u16()?);
            }
            text_entries.push((id, String::from_utf16_lossy(&units)));
        }

        Ok(Self {
            serial,
            gump_id,
            button,
            switches,
            text_entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gump_declares_its_own_length_and_id() {
        let bytes = encode_gump_display(
            0,
            0x00AD_0001,
            50,
            40,
            "{ resizepic 0 0 5054 200 200 }{ button 10 10 4005 4007 1 0 1 }",
            &["Spawn".to_owned()],
        );
        assert_eq!(bytes[0], 0xB0);
        let declared = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        assert_eq!(
            declared,
            bytes.len(),
            "the length word must match the bytes"
        );
        assert_eq!(
            u32::from_be_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]),
            0x00AD_0001,
            "the gump id round-trips"
        );
    }

    #[test]
    fn a_line_rides_as_big_endian_utf16() {
        // "Ok" is two code units; each is a big-endian u16.
        let bytes = encode_gump_display(0, 1, 0, 0, "{ page 0 }", &["Ok".to_owned()]);
        // Find the text table: it is the tail, `count(2) len(2) 'O'(2) 'k'(2)`.
        let tail = &bytes[bytes.len() - 8..];
        assert_eq!(u16::from_be_bytes([tail[0], tail[1]]), 1, "one line");
        assert_eq!(u16::from_be_bytes([tail[2], tail[3]]), 2, "two chars");
        assert_eq!(u16::from_be_bytes([tail[4], tail[5]]), b'O' as u16);
        assert_eq!(u16::from_be_bytes([tail[6], tail[7]]), b'k' as u16);
    }

    #[test]
    fn a_response_reads_the_button_and_its_fields() {
        // 0xB1: len, serial, gumpId, button=3, 1 switch (id 7), 1 text (id 2, "Hi").
        let mut p = vec![0xB1u8, 0, 0];
        p.extend_from_slice(&0x1234u32.to_be_bytes());
        p.extend_from_slice(&0x00AD_0001u32.to_be_bytes());
        p.extend_from_slice(&3u32.to_be_bytes());
        p.extend_from_slice(&1u32.to_be_bytes()); // switch count
        p.extend_from_slice(&7u32.to_be_bytes()); // switch id
        p.extend_from_slice(&1u32.to_be_bytes()); // text count
        p.extend_from_slice(&2u16.to_be_bytes()); // text id
        p.extend_from_slice(&2u16.to_be_bytes()); // char count
        p.extend_from_slice(&(b'H' as u16).to_be_bytes());
        p.extend_from_slice(&(b'i' as u16).to_be_bytes());
        let len = u16::try_from(p.len()).unwrap();
        p[1..3].copy_from_slice(&len.to_be_bytes());

        let got = GumpResponse::decode(&p).unwrap();
        assert_eq!(got.serial, 0x1234);
        assert_eq!(got.gump_id, 0x00AD_0001);
        assert_eq!(got.button, 3);
        assert_eq!(got.switches, vec![7]);
        assert_eq!(got.text_entries, vec![(2, "Hi".to_owned())]);
    }

    #[test]
    fn a_closed_gump_is_button_zero() {
        let mut p = vec![0xB1u8, 0, 0];
        p.extend_from_slice(&0u32.to_be_bytes()); // serial
        p.extend_from_slice(&5u32.to_be_bytes()); // gumpId
        p.extend_from_slice(&0u32.to_be_bytes()); // button 0 = closed
        p.extend_from_slice(&0u32.to_be_bytes()); // no switches
        p.extend_from_slice(&0u32.to_be_bytes()); // no text
        let len = u16::try_from(p.len()).unwrap();
        p[1..3].copy_from_slice(&len.to_be_bytes());

        let got = GumpResponse::decode(&p).unwrap();
        assert_eq!(got.button, 0, "the close box");
        assert!(got.switches.is_empty());
    }
}
