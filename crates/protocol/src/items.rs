//! Item packets: what the client is told about things on the ground, and what it
//! asks to do with them.
//!
//! A mobile and an item are drawn by different packets — `0x78` for a mobile,
//! `0x1A` for an item — but the interest machinery that decides *when* to draw
//! them is the same. This module is the item half of that, plus the two requests
//! a client makes about an item it can reach: `0x07` to pick it up and `0x08` to
//! put it down.

use crate::codec::PacketWriter;
use crate::login::{expect_id, LoginDecodeError};
use crate::world::Point;

/// The serial a `0x08` drop carries when the item is going onto the ground
/// rather than into a container or onto a mobile.
pub const DROP_TO_GROUND: u32 = 0xFFFF_FFFF;

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

/// `0x07` — the client asks to pick an item up. 7 bytes.
///
/// The item goes onto the client's cursor, dragged, until a `0x08` puts it down.
/// `amount` is how much of a stack to lift; the whole item unless the client is
/// splitting a pile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PickUpItem {
    /// The item's serial.
    pub serial: u32,
    /// How many to lift, for a stack.
    pub amount: u16,
}

impl PickUpItem {
    /// The packet id.
    pub const ID: u8 = 0x07;

    /// Decode a whole `0x07` packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        Ok(Self {
            serial: reader.u32()?,
            amount: reader.u16()?,
        })
    }
}

/// `0x08` — the client asks to put the dragged item down. 14 bytes.
///
/// # The grid byte, and why this is the short form
///
/// Where the item goes is [`container`](Self::container): a real item serial
/// drops it *into* that container, a mobile serial equips it, and
/// [`DROP_TO_GROUND`] (`0xFFFFFFFF`) drops it at [`position`](Self::position) on
/// the ground.
///
/// Newer clients (SA and up, and the enhanced client) slip a one-byte *grid
/// index* in before the container serial, making the packet fifteen bytes. The
/// game connection here defaults to the older dialect and frames `0x08` at
/// fourteen, so this decodes the no-grid form; a client that sends the grid byte
/// would need a version-gated length, which is a change to the framing table and
/// this decoder together, not one of them alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DropItem {
    /// The item being dropped.
    pub serial: u32,
    /// Where, when dropping on the ground.
    pub position: Point,
    /// Where the item is going: a container serial, a mobile serial to equip on,
    /// or [`DROP_TO_GROUND`].
    pub container: u32,
}

impl DropItem {
    /// The packet id.
    pub const ID: u8 = 0x08;

    /// Decode a whole `0x08` packet, no-grid form.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let serial = reader.u32()?;
        let x = reader.u16()?;
        let y = reader.u16()?;
        let z = reader.u8()? as i8;
        let container = reader.u32()?;
        Ok(Self {
            serial,
            position: Point::new(x, y, z),
            container,
        })
    }

    /// Whether this drop is onto the ground rather than into a container or onto
    /// a mobile.
    pub const fn to_ground(&self) -> bool {
        self.container == DROP_TO_GROUND
    }
}

/// Why the server cancelled a drag — the `code` in a `0x27`.
///
/// From Sphere's `PacketDragCancel::Reason`. The client bounces the item back to
/// where it came from whichever it is; the code only changes the message it
/// shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum DragCancelReason {
    /// The item cannot be lifted at all.
    CannotLift = 0x00,
    /// Too far away to reach.
    OutOfRange = 0x01,
    /// Out of line of sight.
    OutOfSight = 0x02,
    /// It is not yours to take.
    TryToSteal = 0x03,
    /// You are already holding something.
    AlreadyHolding = 0x04,
    /// Anything else.
    Other = 0x05,
}

/// `0x27` — cancel a drag and tell the client to bounce the item back. 2 bytes.
pub fn encode_drag_cancel(reason: DragCancelReason) -> Vec<u8> {
    vec![0x27, reason as u8]
}

/// `0x13` — the client asks to equip the dragged item onto a mobile. 10 bytes.
///
/// Dragging an item onto a paperdoll sends this: the item goes onto `mobile` at
/// `layer`, the slot the client worked out from the item's tiledata. The server
/// checks it rather than trusting it, but the layer is the client's to propose.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EquipItemRequest {
    /// The item being worn.
    pub item: u32,
    /// The layer to wear it on.
    pub layer: u8,
    /// The mobile wearing it.
    pub mobile: u32,
}

impl EquipItemRequest {
    /// The packet id.
    pub const ID: u8 = 0x13;

    /// Decode a whole `0x13` packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        Ok(Self {
            item: reader.u32()?,
            layer: reader.u8()?,
            mobile: reader.u32()?,
        })
    }
}

/// `0x2E` — a mobile is now wearing an item. 15 bytes.
///
/// The single-item counterpart of the equipment list inside a `0x78`: sent when
/// one item is put on or the mobile is already drawn and only its outfit changed.
pub fn encode_equip(item: u32, graphic: u16, layer: u8, mobile: u32, hue: u16) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(15);
    writer.u8(0x2E);
    writer.u32(item);
    writer.u16(graphic);
    writer.u8(0); // graphic offset, always zero
    writer.u8(layer);
    writer.u32(mobile);
    writer.u16(hue);
    writer.into_bytes()
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

    #[test]
    fn a_pickup_is_a_serial_and_an_amount() {
        let bytes = [0x07, 0x40, 0x00, 0x00, 0x2A, 0x00, 0x05];
        let pickup = PickUpItem::decode(&bytes).unwrap();
        assert_eq!(pickup.serial, 0x4000_002A);
        assert_eq!(pickup.amount, 5);
    }

    #[test]
    fn a_ground_drop_reads_its_target_as_the_ground() {
        // serial, x=1000, y=2000, z=5, container=0xFFFFFFFF
        let mut bytes = vec![0x08];
        bytes.extend_from_slice(&0x4000_002Au32.to_be_bytes());
        bytes.extend_from_slice(&1000u16.to_be_bytes());
        bytes.extend_from_slice(&2000u16.to_be_bytes());
        bytes.push(5);
        bytes.extend_from_slice(&DROP_TO_GROUND.to_be_bytes());
        assert_eq!(bytes.len(), 14);

        let drop = DropItem::decode(&bytes).unwrap();
        assert_eq!(drop.serial, 0x4000_002A);
        assert_eq!(drop.position, Point::new(1000, 2000, 5));
        assert!(drop.to_ground());
    }

    #[test]
    fn a_drop_into_a_container_is_not_a_ground_drop() {
        let mut bytes = vec![0x08];
        bytes.extend_from_slice(&0x4000_002Au32.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.extend_from_slice(&0u16.to_be_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&0x4000_00FFu32.to_be_bytes()); // a container serial
        let drop = DropItem::decode(&bytes).unwrap();
        assert!(!drop.to_ground());
        assert_eq!(drop.container, 0x4000_00FF);
    }

    #[test]
    fn a_drag_cancel_is_two_bytes_with_the_reason() {
        assert_eq!(
            encode_drag_cancel(DragCancelReason::OutOfRange),
            vec![0x27, 0x01]
        );
        assert_eq!(
            encode_drag_cancel(DragCancelReason::AlreadyHolding),
            vec![0x27, 0x04]
        );
    }

    #[test]
    fn an_equip_request_is_item_layer_mobile() {
        let mut bytes = vec![0x13];
        bytes.extend_from_slice(&0x4000_0002u32.to_be_bytes());
        bytes.push(2); // layer 2, the left hand
        bytes.extend_from_slice(&0x0000_0001u32.to_be_bytes());
        assert_eq!(bytes.len(), 10);
        let req = EquipItemRequest::decode(&bytes).unwrap();
        assert_eq!(req.item, 0x4000_0002);
        assert_eq!(req.layer, 2);
        assert_eq!(req.mobile, 0x0000_0001);
    }

    #[test]
    fn an_equip_packet_is_fifteen_bytes() {
        let packet = encode_equip(0x4000_0002, 0x13B9, 1, 0x0000_0001, 0x0021);
        assert_eq!(packet.len(), 15);
        assert_eq!(packet[0], 0x2E);
        assert_eq!(&packet[1..5], &0x4000_0002u32.to_be_bytes());
        assert_eq!(&packet[5..7], &0x13B9u16.to_be_bytes());
        assert_eq!(packet[7], 0);
        assert_eq!(packet[8], 1); // layer
        assert_eq!(&packet[9..13], &0x0000_0001u32.to_be_bytes());
        assert_eq!(&packet[13..15], &0x0021u16.to_be_bytes());
    }
}
