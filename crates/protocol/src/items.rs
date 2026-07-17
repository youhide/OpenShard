//! Item packets: what the client is told about things on the ground.
//!
//! A mobile and an item are drawn by different packets — `0x78` for a mobile,
//! `0x1A` for an item — but the interest machinery that decides *when* to draw
//! them is the same. This module is the item half of that.

use crate::codec::PacketWriter;
use crate::world::Point;

/// `0x1A` — draw an item on the ground the client has not seen. Variable length.
///
/// # The shape is a nest of optional fields
///
/// Ported from Sphere's `PacketItemWorld`, and it is the classic UO packet in
/// full awkwardness: which fields are present is encoded in flag bits stolen
/// from other fields, because in 1997 every byte counted.
///
/// - The top bit of the **serial** (`0x8000_0000`) means "a stack amount
///   follows the graphic". A single item does not set it and sends no amount.
/// - The top bit of **x** (`0x8000`) means "a direction or light byte follows".
///   We send neither, so it stays clear. `x` itself is 15 bits.
/// - The top bit of **y** (`0x8000`) means "a hue word follows"; the next bit
///   (`0x4000`) means "a flags byte follows". `y` itself is 14 bits.
///
/// So a plain grey item is serial, graphic, x, y, z — and a hued stack of gold
/// is serial (with the amount bit), graphic, amount, x, y (with the hue bit), z,
/// hue. Sending a field whose flag bit is clear, or omitting one whose bit is
/// set, desynchronises the client mid-packet and every byte after is read as
/// something else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WorldItem {
    /// The item's wire serial.
    pub serial: u32,
    /// Its graphic (tiledata id).
    pub graphic: u16,
    /// How many are in the stack. 0 or 1 is a single item and sends no amount.
    pub amount: u16,
    /// Where it lies.
    pub position: Point,
    /// Its hue, or 0 for none.
    pub hue: u16,
}

impl WorldItem {
    /// The packet id.
    pub const ID: u8 = 0x1A;

    /// Encode a whole `0x1A` packet.
    pub fn encode(&self) -> Vec<u8> {
        // A stack amount is only sent when there is more than one; the client
        // reads a lone item as a stack of one on its own.
        let stacked = self.amount > 1;
        let hued = self.hue != 0;

        let mut writer = PacketWriter::with_capacity(20);
        writer.u8(Self::ID);
        writer.u16(0); // length, patched below

        let serial = if stacked {
            self.serial | 0x8000_0000
        } else {
            self.serial & 0x7FFF_FFFF
        };
        writer.u32(serial);
        writer.u16(self.graphic);
        if stacked {
            writer.u16(self.amount);
        }

        // x keeps its low 15 bits; its top bit would mean a direction/light byte
        // follows, and we send neither.
        writer.u16(self.position.x & 0x7FFF);
        // y keeps its low 14 bits; the top bit flags a hue word.
        let mut y = self.position.y & 0x3FFF;
        if hued {
            y |= 0x8000;
        }
        writer.u16(y);
        writer.u8(self.position.z as u8);
        if hued {
            writer.u16(self.hue);
        }

        let mut bytes = writer.into_bytes();
        let length = u16::try_from(bytes.len()).expect("an item packet outgrew its u16 length");
        bytes[1..3].copy_from_slice(&length.to_be_bytes());
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_item_is_the_short_form() {
        // No amount, no hue: serial, graphic, x, y, z and nothing optional.
        let packet = WorldItem {
            serial: 0x4000_0001,
            graphic: 0x0EED, // a gold coin graphic
            amount: 1,
            position: Point::new(1000, 2000, 5),
            hue: 0,
        }
        .encode();

        assert_eq!(packet[0], 0x1A);
        assert_eq!(
            u16::from_be_bytes([packet[1], packet[2]]),
            packet.len() as u16
        );
        // serial, unchanged (top bit clear because it is a single item)
        assert_eq!(&packet[3..7], &[0x40, 0x00, 0x00, 0x01]);
        // graphic
        assert_eq!(&packet[7..9], &0x0EEDu16.to_be_bytes());
        // x, y, z — no amount squeezed in between
        assert_eq!(&packet[9..11], &1000u16.to_be_bytes());
        assert_eq!(&packet[11..13], &2000u16.to_be_bytes());
        assert_eq!(packet[13], 5);
        assert_eq!(packet.len(), 14);
    }

    #[test]
    fn a_hued_stack_carries_the_amount_and_hue_with_their_flags() {
        let packet = WorldItem {
            serial: 0x4000_00AB,
            graphic: 0x0EED,
            amount: 500,
            position: Point::new(1000, 2000, 5),
            hue: 0x0021,
        }
        .encode();

        // The amount bit is set on top of the serial's own bits.
        assert_eq!(
            u32::from_be_bytes([packet[3], packet[4], packet[5], packet[6]]),
            0xC000_00AB
        );
        assert_eq!(&packet[7..9], &0x0EEDu16.to_be_bytes());
        // amount follows the graphic
        assert_eq!(&packet[9..11], &500u16.to_be_bytes());
        // x plain, y with the hue flag
        assert_eq!(&packet[11..13], &1000u16.to_be_bytes());
        assert_eq!(u16::from_be_bytes([packet[13], packet[14]]), 2000 | 0x8000);
        assert_eq!(packet[15], 5);
        // hue last
        assert_eq!(&packet[16..18], &0x0021u16.to_be_bytes());
    }

    #[test]
    fn a_high_z_survives_as_a_signed_byte() {
        // Underground and underwater are negative z; the client reads the byte
        // as signed, so -5 has to go out as 0xFB, not clamp to 0.
        let packet = WorldItem {
            serial: 0x4000_0001,
            graphic: 0x0001,
            amount: 1,
            position: Point::new(0, 0, -5),
            hue: 0,
        }
        .encode();
        assert_eq!(packet[13], 0xFB);
    }
}
