//! The spellbook the client draws — which spells a book holds.
//!
//! A spellbook is opened like a container (`0x24` with the gump id `0xFFFF`,
//! which is what tells the client it is a *book*, not a bag — reuse
//! [`encode_open_container`](crate::encode_open_container) with that gump). Its
//! contents are then a `0xBF` subcommand `0x1B`: the book's serial and graphic,
//! the spell the first bit stands for (`offset`, 1 for Magery), and an eight-byte
//! little-endian mask, bit `n` set when the book holds the `offset + n`-th spell.
//! Ported from ServUO's `NewSpellbookContent`.

use crate::codec::PacketWriter;

/// `0xBF` `0x1B` — the spells a book holds, as a 64-bit mask.
///
/// `offset` is the spell the low bit stands for (`1` for Magery, so bit 0 is
/// spell 1); `content` is the mask, written little-endian byte by byte.
#[must_use]
pub fn encode_spellbook_content(serial: u32, graphic: u16, offset: u16, content: u64) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(23);
    writer.u8(0xBF);
    writer.u16(0); // length, patched below
    writer.u16(0x1B); // subcommand: spellbook content
    writer.u16(0x01); // the "new" (post-4.0) form
    writer.u32(serial);
    writer.u16(graphic);
    writer.u16(offset);
    for i in 0..8 {
        writer.u8((content >> (i * 8)) as u8);
    }

    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a spellbook packet outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_content_mask_is_little_endian() {
        // Spells 1 and 64 held: bits 0 and 63 of the mask.
        let content = 1u64 | (1u64 << 63);
        let packet = encode_spellbook_content(0x4000_0001, 0x0EFA, 1, content);
        assert_eq!(packet[0], 0xBF);
        assert_eq!(
            u16::from_be_bytes([packet[1], packet[2]]),
            packet.len() as u16
        );
        assert_eq!(&packet[3..5], &0x1Bu16.to_be_bytes(), "subcommand");
        assert_eq!(&packet[5..7], &0x01u16.to_be_bytes());
        assert_eq!(&packet[7..11], &0x4000_0001u32.to_be_bytes(), "the book");
        assert_eq!(&packet[11..13], &0x0EFAu16.to_be_bytes(), "its graphic");
        assert_eq!(&packet[13..15], &1u16.to_be_bytes(), "Magery offset");
        // The 8-byte mask, little-endian: bit 0 in the first byte, bit 63 in the last.
        assert_eq!(packet[15], 0x01, "spell 1 in the low byte");
        assert_eq!(packet[22], 0x80, "spell 64 in the high byte");
    }
}
