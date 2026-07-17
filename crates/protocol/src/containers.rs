//! Container packets: opening a container and listing what is inside it.
//!
//! A container is an item that holds other items. Three server packets draw it:
//! `0x24` opens the gump window, `0x3C` fills it with everything inside at once,
//! and `0x25` adds one more item to a gump already open. The client asks to open
//! one by double-clicking it — `0x06`.
//!
//! # Two client-version seams
//!
//! - The `0x24` open packet gained a one-word *container type* on High Seas
//!   clients ([`Feature::HsPackets`]). Older clients stop after the gump id.
//! - Every item record inside a container gained a one-byte *grid index* on
//!   6.0.1.7 ([`Feature::ItemGrid`]) — the slot in the enhanced grid view. The
//!   classic 2D client positions items by their `x`/`y` and ignores it; a grid
//!   client reads it and desynchronises if it is missing.

use crate::codec::PacketWriter;
use crate::feature::Feature;
use crate::login::{expect_id, LoginDecodeError};
use crate::version::ClientVersion;

/// The container-type byte a High Seas client expects in `0x24` for a normal
/// container (a vendor's is `0x00`, which is not this).
const CONTAINER_TYPE: u16 = 0x7D;

/// `0x06` — the client double-clicked an object. 5 bytes.
///
/// Double-click is "use this": a container opens, a door swings, a food is
/// eaten. The server decides what the object does; this only says which.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DoubleClick {
    /// The object's serial.
    pub serial: u32,
}

impl DoubleClick {
    /// The packet id.
    pub const ID: u8 = 0x06;

    /// Decode a whole `0x06` packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        Ok(Self {
            serial: reader.u32()?,
        })
    }
}

/// One item as it sits inside a container: what `0x25` and `0x3C` write per item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContainedItem {
    /// The item's serial.
    pub serial: u32,
    /// Its graphic.
    pub graphic: u16,
    /// Its stack size.
    pub amount: u16,
    /// Its column in the container gump.
    pub x: u16,
    /// Its row in the container gump.
    pub y: u16,
    /// Its slot in the enhanced grid view. Sent only to grid clients.
    pub grid: u8,
    /// Its hue.
    pub hue: u16,
}

impl ContainedItem {
    /// Write one item record: the shared body of `0x25` and `0x3C`.
    fn write(&self, writer: &mut PacketWriter, container: u32, grid: bool) {
        writer.u32(self.serial);
        writer.u16(self.graphic);
        writer.u8(0); // graphic offset, always zero
        writer.u16(self.amount);
        writer.u16(self.x);
        writer.u16(self.y);
        if grid {
            writer.u8(self.grid);
        }
        writer.u32(container);
        writer.u16(self.hue);
    }
}

/// `0x24` — open a container gump on the client. 7 bytes, 9 on High Seas.
pub fn encode_open_container(serial: u32, gump: u16, version: ClientVersion) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(9);
    writer.u8(0x24);
    writer.u32(serial);
    writer.u16(gump);
    if version.supports(Feature::HsPackets) {
        writer.u16(CONTAINER_TYPE);
    }
    writer.into_bytes()
}

/// `0x25` — add one item to a container gump the client already has open.
pub fn encode_add_to_container(
    item: ContainedItem,
    container: u32,
    version: ClientVersion,
) -> Vec<u8> {
    let grid = version.supports(Feature::ItemGrid);
    let mut writer = PacketWriter::with_capacity(21);
    writer.u8(0x25);
    item.write(&mut writer, container, grid);
    writer.into_bytes()
}

/// `0x3C` — the full contents of a container, all at once. Variable length.
pub fn encode_container_contents(
    container: u32,
    items: &[ContainedItem],
    version: ClientVersion,
) -> Vec<u8> {
    let grid = version.supports(Feature::ItemGrid);
    let mut writer = PacketWriter::with_capacity(5 + items.len() * 20);
    writer.u8(0x3C);
    writer.u16(0); // length, patched below
    writer.u16(items.len() as u16);
    for item in items {
        item.write(&mut writer, container, grid);
    }

    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a container outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version with the grid index and the High Seas container type.
    fn modern() -> ClientVersion {
        ClientVersion::new(7, 0, 9, 0)
    }

    /// A version with neither.
    fn classic() -> ClientVersion {
        ClientVersion::new(5, 0, 0, 0)
    }

    #[test]
    fn a_double_click_is_a_serial() {
        let bytes = [0x06, 0x40, 0x00, 0x00, 0x2A];
        assert_eq!(DoubleClick::decode(&bytes).unwrap().serial, 0x4000_002A);
    }

    #[test]
    fn opening_a_container_is_seven_bytes_on_a_classic_client() {
        let packet = encode_open_container(0x4000_0001, 0x003C, classic());
        assert_eq!(packet[0], 0x24);
        assert_eq!(&packet[1..5], &0x4000_0001u32.to_be_bytes());
        assert_eq!(&packet[5..7], &0x003Cu16.to_be_bytes());
        assert_eq!(packet.len(), 7, "no container-type word before High Seas");
    }

    #[test]
    fn opening_a_container_gains_the_type_word_on_high_seas() {
        let packet = encode_open_container(0x4000_0001, 0x003C, modern());
        assert_eq!(packet.len(), 9);
        assert_eq!(u16::from_be_bytes([packet[7], packet[8]]), CONTAINER_TYPE);
    }

    #[test]
    fn a_classic_container_item_record_has_no_grid_byte() {
        let item = ContainedItem {
            serial: 0x4000_0002,
            graphic: 0x0EED,
            amount: 3,
            x: 44,
            y: 65,
            grid: 7,
            hue: 0,
        };
        let packet = encode_add_to_container(item, 0x4000_0001, classic());
        // 0x25 + serial + graphic + 0 + amount + x + y + container + hue = 20
        assert_eq!(packet.len(), 20);
        assert_eq!(packet[0], 0x25);
        assert_eq!(&packet[1..5], &0x4000_0002u32.to_be_bytes());
        assert_eq!(&packet[5..7], &0x0EEDu16.to_be_bytes());
        assert_eq!(packet[7], 0); // graphic offset
        assert_eq!(&packet[8..10], &3u16.to_be_bytes());
        assert_eq!(&packet[10..12], &44u16.to_be_bytes());
        assert_eq!(&packet[12..14], &65u16.to_be_bytes());
        // straight to the container serial, no grid byte
        assert_eq!(&packet[14..18], &0x4000_0001u32.to_be_bytes());
    }

    #[test]
    fn a_grid_client_item_record_carries_the_grid_byte() {
        let item = ContainedItem {
            serial: 0x4000_0002,
            graphic: 0x0EED,
            amount: 3,
            x: 44,
            y: 65,
            grid: 7,
            hue: 0,
        };
        let packet = encode_add_to_container(item, 0x4000_0001, modern());
        assert_eq!(packet.len(), 21);
        assert_eq!(
            packet[14], 7,
            "the grid index sits before the container serial"
        );
        assert_eq!(&packet[15..19], &0x4000_0001u32.to_be_bytes());
    }

    #[test]
    fn container_contents_counts_its_items_and_patches_its_length() {
        let items = [
            ContainedItem {
                serial: 0x4000_0002,
                graphic: 0x0EED,
                amount: 1,
                x: 10,
                y: 10,
                grid: 0,
                hue: 0,
            },
            ContainedItem {
                serial: 0x4000_0003,
                graphic: 0x0F0E,
                amount: 5,
                x: 20,
                y: 20,
                grid: 1,
                hue: 0x21,
            },
        ];
        let packet = encode_container_contents(0x4000_0001, &items, classic());
        assert_eq!(packet[0], 0x3C);
        assert_eq!(
            u16::from_be_bytes([packet[1], packet[2]]),
            packet.len() as u16
        );
        assert_eq!(u16::from_be_bytes([packet[3], packet[4]]), 2, "two items");
        // header 5 + two classic records of 19 each = 43
        assert_eq!(packet.len(), 5 + 2 * 19);
    }

    #[test]
    fn an_empty_container_is_just_a_header() {
        let packet = encode_container_contents(0x4000_0001, &[], classic());
        assert_eq!(u16::from_be_bytes([packet[3], packet[4]]), 0);
        assert_eq!(packet.len(), 5);
    }
}
