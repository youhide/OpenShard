//! Context menus — the pop-up an AoS client opens on an object (a right-click, or
//! a single-click when the client is set that way).
//!
//! Three `0xBF` subcommands. The client asks for an object's menu (`0x13`); the
//! server sends the entries (`0x14`) — each a cliloc the client localizes and a
//! tag it will name back; the player picks one and the client returns the tag
//! (`0x15`). The entries are the object's default actions — open a container, a
//! vendor's buy/sell, a paperdoll — which the world routes to the same handlers a
//! double-click reaches.
//!
//! Ported from ServUO's `DisplayContextMenu` (the new `0x02` format) and
//! `ContextMenuRequest`/`ContextMenuResponse` (`Server/Network/PacketHandlers.cs`,
//! `Packets.cs`), cross-checked against Sphere's `Event_AOSPopupMenuRequest` /
//! `Event_AOSPopupMenuSelect`. The tag the client sends back is the entry's
//! position in the list, so the world can map it to an action by index.

use crate::codec::PacketWriter;
use crate::login::{expect_id, LoginDecodeError};

/// `0xBF` subcommand `0x13` — the client asking for an object's context menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContextMenuRequest {
    /// The object whose menu is wanted, by serial.
    pub serial: u32,
}

impl ContextMenuRequest {
    /// The packet id — the extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// The subcommand that means "open a context menu".
    pub const SUBCOMMAND: u16 = 0x13;

    /// Decode a `0xBF`, returning the request if that is its subcommand. Any other
    /// `0xBF` reads as `None`, so the dispatcher can pass on the ones it does not
    /// handle — the same shape as [`CastSpellRequest`](crate::CastSpellRequest).
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        if reader.u16()? != Self::SUBCOMMAND {
            return Ok(None);
        }
        Ok(Some(Self {
            serial: reader.u32()?,
        }))
    }
}

/// `0xBF` subcommand `0x15` — the client reporting which entry the player picked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContextMenuSelect {
    /// The object the menu was opened on.
    pub serial: u32,
    /// Which entry, by the tag the menu gave it — its position in the list.
    pub index: u16,
}

impl ContextMenuSelect {
    /// The packet id — the extended-command envelope.
    pub const ID: u8 = 0xBF;
    /// The subcommand that means "a menu entry was chosen".
    pub const SUBCOMMAND: u16 = 0x15;

    /// Decode a `0xBF`, returning the selection if that is its subcommand.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        if reader.u16()? != Self::SUBCOMMAND {
            return Ok(None);
        }
        Ok(Some(Self {
            serial: reader.u32()?,
            index: reader.u16()?,
        }))
    }
}

/// `0xBF` subcommand `0x14` — draw a context menu on an object.
///
/// The new (`0x02`) format, which every client since 6.0.0.0
/// ([`Feature::NewContextMenu`](crate::Feature::NewContextMenu)) reads: each entry
/// is a four-byte cliloc, a two-byte tag (its position, sent back on select), and
/// two-byte flags — `0` for a plain enabled entry. Ported from ServUO's
/// `DisplayContextMenu`.
#[must_use]
pub fn encode_context_menu(serial: u32, entries: &[(u32, u16)]) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(12 + entries.len() * 8);
    writer.u8(0xBF);
    writer.u16(0); // length, patched below
    writer.u16(0x14); // subcommand: display popup
    writer.u16(0x02); // the new format
    writer.u32(serial);
    writer.u8(entries.len() as u8);
    for (index, &(cliloc, flags)) in entries.iter().enumerate() {
        writer.u32(cliloc);
        writer.u16(index as u16); // the tag the client returns on select
        writer.u16(flags);
    }

    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a context menu outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_reads_its_serial() {
        let mut bytes = vec![0xBF, 0, 0];
        bytes.extend_from_slice(&ContextMenuRequest::SUBCOMMAND.to_be_bytes());
        bytes.extend_from_slice(&0x0000_1234u32.to_be_bytes());
        let len = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&len.to_be_bytes());

        let request = ContextMenuRequest::decode(&bytes).unwrap().unwrap();
        assert_eq!(request.serial, 0x0000_1234);
    }

    #[test]
    fn a_select_reads_its_serial_and_index() {
        let mut bytes = vec![0xBF, 0, 0];
        bytes.extend_from_slice(&ContextMenuSelect::SUBCOMMAND.to_be_bytes());
        bytes.extend_from_slice(&0x0000_5678u32.to_be_bytes());
        bytes.extend_from_slice(&2u16.to_be_bytes());
        let len = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&len.to_be_bytes());

        let select = ContextMenuSelect::decode(&bytes).unwrap().unwrap();
        assert_eq!((select.serial, select.index), (0x0000_5678, 2));
    }

    #[test]
    fn another_extended_command_is_not_a_context_menu() {
        // A 0xBF that is not one of the context subcommands reads as None.
        let packet = vec![0xBF, 0x00, 0x07, 0x00, 0x1C, 0x00, 0x00];
        assert_eq!(ContextMenuRequest::decode(&packet).unwrap(), None);
        assert_eq!(ContextMenuSelect::decode(&packet).unwrap(), None);
    }

    #[test]
    fn a_menu_tags_each_entry_with_its_position() {
        let packet = encode_context_menu(0x0000_00AB, &[(3_000_362, 0), (6_103, 0)]);
        assert_eq!(packet[0], 0xBF);
        assert_eq!(
            u16::from_be_bytes([packet[1], packet[2]]),
            packet.len() as u16
        );
        assert_eq!(
            &packet[3..5],
            &0x14u16.to_be_bytes(),
            "display-popup subcommand"
        );
        assert_eq!(&packet[5..7], &0x02u16.to_be_bytes(), "the new format");
        assert_eq!(&packet[7..11], &0x0000_00ABu32.to_be_bytes());
        assert_eq!(packet[11], 2, "two entries");
        // First entry: cliloc, tag 0, flags 0.
        assert_eq!(&packet[12..16], &3_000_362u32.to_be_bytes());
        assert_eq!(&packet[16..18], &0u16.to_be_bytes(), "tag is the position");
        assert_eq!(&packet[18..20], &0u16.to_be_bytes(), "enabled");
        // Second entry's tag is 1.
        assert_eq!(&packet[24..26], &1u16.to_be_bytes());
    }
}
