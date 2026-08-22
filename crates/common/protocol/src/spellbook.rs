//! The spellbook the client draws — which spells a book holds.
//!
//! A spellbook is opened like a container (`0x24` with the gump id `0xFFFF`,
//! which is what tells the client it is a *book*, not a bag — reuse
//! [`encode_open_container`](crate::containers::encode_open_container) with that
//! gump). Its
//! contents are then a `0xBF` subcommand `0x1B`: the book's serial and graphic,
//! the spell the first bit stands for (`offset`, 1 for Magery), and an eight-byte
//! little-endian mask, bit `n` set when the book holds the `offset + n`-th spell.
//! Ported from ServUO's `NewSpellbookContent`.

use crate::codec::{PacketReader, PacketWriter};
use crate::error::DecodeError;
use crate::packet::{DecodePacket, EncodePacket, PacketLength};
use crate::serial::Serial;
use crate::version::ClientVersion;
use crate::wire::Graphic;

/// `0xBF` `0x1B` — the spells a book holds, as a 64-bit mask. Fixed 23 bytes.
///
/// `offset` is the spell the low bit stands for (`1` for Magery, so bit 0 is
/// spell 1); `content` is the mask, written little-endian byte by byte.
///
/// # Fixed despite living under `0xBF`, and the length field is still hand-written
///
/// Like [`crate::mobile::StatLocks`], this subcommand's body never varies, so it
/// declares `Fixed(23)`. [`crate::packet::frame_body`] only back-patches a length
/// for [`PacketLength::Variable`], so the constant `u16(23)` is still written
/// here by hand, exactly where the `0xBF` envelope always puts one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SpellbookContent {
    /// The book.
    pub serial: Serial,
    /// Its graphic. Always `openshard_state::components::SPELLBOOK_GRAPHIC`
    /// today, a server-chosen constant.
    pub graphic: Graphic,
    /// The spell the low bit of `content` stands for. Bare by decision: the
    /// only value ever sent is `1` (Magery), and N3 amendment 1's test —
    /// "does something already branch on this byte" — is not met while no
    /// second spell school is wired up. See `docs/protocol_newtypes.md`.
    pub offset: u16,
    /// Bit `n` set means the book holds the `offset + n`-th spell.
    pub content: u64,
}

impl SpellbookContent {
    /// The `0xBF` subcommand for a spellbook's contents.
    pub const SUBCOMMAND: u16 = 0x1B;
}

impl EncodePacket for SpellbookContent {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Fixed(23);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(23); // this subcommand's own, constant length
        out.u16(Self::SUBCOMMAND);
        out.u16(0x01); // the "new" (post-4.0) form
        out.u32(self.serial.raw());
        out.u16(self.graphic.0);
        out.u16(self.offset);
        for i in 0..8 {
            out.u8((self.content >> (i * 8)) as u8);
        }
    }
}

impl DecodePacket for SpellbookContent {
    const ID: u8 = 0xBF;

    /// `decode_server` has already consumed the envelope's length, so every
    /// extended packet starts here at its subcommand.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let subcommand = reader.u16()?;
        if subcommand != Self::SUBCOMMAND {
            return Err(DecodeError::UnknownValue {
                field: "0xBF subcommand for spellbook content",
                value: u32::from(subcommand),
            });
        }
        // The modern form marker.  There is no useful older form for this
        // client to guess at: accepting it would shift the serial and make a
        // plausible but unrelated book appear open.
        let form = reader.u16()?;
        if form != 1 {
            return Err(DecodeError::UnknownValue {
                field: "spellbook content form",
                value: u32::from(form),
            });
        }
        let raw_serial = reader.u32()?;
        let serial = Serial::new(raw_serial).ok_or(DecodeError::UnknownValue {
            field: "spellbook serial",
            value: raw_serial,
        })?;
        let graphic = Graphic(reader.u16()?);
        let offset = reader.u16()?;
        let mut content = 0u64;
        for byte in 0..8 {
            content |= u64::from(reader.u8()?) << (byte * 8);
        }
        Ok(Self {
            serial,
            graphic,
            offset,
            content,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::encode_packet;
    use crate::server_packet::ServerPacket;

    #[test]
    fn the_content_mask_is_little_endian() {
        // Spells 1 and 64 held: bits 0 and 63 of the mask.
        let content = 1u64 | (1u64 << 63);
        let packet = encode_packet(
            &SpellbookContent {
                serial: Serial::new(0x4000_0001).unwrap(),
                graphic: Graphic(0x0EFA),
                offset: 1,
                content,
            },
            ClientVersion::new(7, 0, 45, 65),
        );
        assert_eq!(packet[0], 0xBF);
        assert_eq!(u16::from_be_bytes([packet[1], packet[2]]), packet.len() as u16);
        assert_eq!(&packet[3..5], &0x1Bu16.to_be_bytes(), "subcommand");
        assert_eq!(&packet[5..7], &0x01u16.to_be_bytes());
        assert_eq!(&packet[7..11], &0x4000_0001u32.to_be_bytes(), "the book");
        assert_eq!(&packet[11..13], &0x0EFAu16.to_be_bytes(), "its graphic");
        assert_eq!(&packet[13..15], &1u16.to_be_bytes(), "Magery offset");
        // The 8-byte mask, little-endian: bit 0 in the first byte, bit 63 in the last.
        assert_eq!(packet[15], 0x01, "spell 1 in the low byte");
        assert_eq!(packet[22], 0x80, "spell 64 in the high byte");
    }

    #[test]
    fn a_client_reads_the_book_contents_back_out_of_the_extended_envelope() {
        let content = SpellbookContent {
            serial: Serial::new(0x4000_0001).expect("an item serial"),
            graphic: Graphic(0x0EFA),
            offset: 1,
            content: 1 | (1 << 17),
        };
        let packet = encode_packet(&content, ClientVersion::new(7, 0, 45, 65));
        assert_eq!(
            ServerPacket::decode(&packet, ClientVersion::new(7, 0, 45, 65)).expect("well-formed packet"),
            Some(ServerPacket::SpellbookContent(content))
        );
    }
}
