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

use std::fmt;

use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::packet::{
    EncodePacket,
    PacketLength,
};
use crate::serial::{
    RawSerial,
    Serial,
};
use crate::version::ClientVersion;
use crate::wire::ClilocId;

/// Which entry of a context menu was picked: its position in the list the
/// server sent, counted from zero.
///
/// The client never invents one — it echoes the tag the `0x14` gave the row —
/// so an entry index only ever exists because a menu offered it. That is what
/// [`RawContextMenuIndex::validate`] establishes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ContextMenuIndex(pub u16);

/// A menu entry's tag exactly as the client's `0xBF 0x15` echoed it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct RawContextMenuIndex(pub u16);

impl RawContextMenuIndex {
    /// The entry this names, when the menu that was drawn had `offered` of
    /// them.
    ///
    /// This is the check `docs/protocol_newtypes.md` N5 calls "is this one I
    /// offered": the tags are the rows numbered from zero, so their count is
    /// the whole domain. A tag past the end is a menu this side never drew —
    /// a stale click on a window whose object has changed, or a crafted
    /// packet — and is refused rather than clamped, since clamping would run
    /// *some* entry's action instead of none.
    ///
    /// A `Result` and not an `Option`, unlike `RawSerial::validate`: there is
    /// no wire value here that means "no entry", so every rejection is worth a
    /// log line and the error carries what to log.
    pub const fn validate(self, offered: usize) -> Result<ContextMenuIndex, InvalidContextMenuIndex> {
        if (self.0 as usize) < offered {
            Ok(ContextMenuIndex(self.0))
        } else {
            Err(InvalidContextMenuIndex { tag: self.0, offered })
        }
    }
}

/// A `0xBF 0x15` naming an entry the menu it answers does not have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct InvalidContextMenuIndex {
    /// The tag the client sent.
    pub tag:     u16,
    /// How many entries the menu actually offered.
    pub offered: usize,
}

impl fmt::Display for InvalidContextMenuIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "context menu entry {} was picked from a menu of {}",
            self.tag, self.offered
        )
    }
}

impl std::error::Error for InvalidContextMenuIndex {
}

/// What a context-menu entry may be beyond a plain, enabled line: greyed out,
/// coloured, drawn in the AoS style.
///
/// A named byte pair rather than an enum, for [`crate::wire::Layer`]'s reason:
/// ServUO's `CMEFlags` has a dozen bits this engine has never set, and naming
/// the ones it does not send would be a guess. `0` is every entry this shard
/// draws today.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct ContextMenuFlags(pub u16);

impl ContextMenuFlags {
    /// A plain, enabled entry — what ServUO's `DisplayContextMenu` sends for
    /// every row this engine offers.
    pub const NONE: Self = Self(0);
}

/// `0xBF` subcommand `0x13` — the client asking for an object's context menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContextMenuRequest {
    /// The object whose menu is wanted, as the client named it.
    pub serial: RawSerial,
}

impl ContextMenuRequest {
    /// The subcommand that means "open a context menu". See
    /// [`ExtendedRequest`](crate::extended::ExtendedRequest), the single place
    /// that reads a `0xBF` envelope and picks a subcommand's body decoder by it.
    pub const SUBCOMMAND: u16 = 0x13;

    /// Read the body, `reader` already past the id, length and subcommand.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            serial: RawSerial(reader.u32()?),
        })
    }
}

/// `0xBF` subcommand `0x15` — the client reporting which entry the player picked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContextMenuSelect {
    /// The object the menu was opened on, as the client named it.
    pub serial: RawSerial,
    /// Which entry, by the tag the menu gave it — its position in the list.
    pub index:  RawContextMenuIndex,
}

impl ContextMenuSelect {
    /// The subcommand that means "a menu entry was chosen". See
    /// [`ExtendedRequest`](crate::extended::ExtendedRequest).
    pub const SUBCOMMAND: u16 = 0x15;

    /// Read the body, `reader` already past the id, length and subcommand.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            serial: RawSerial(reader.u32()?),
            index:  RawContextMenuIndex(reader.u16()?),
        })
    }
}

/// One entry in a [`ContextMenu`]: the cliloc the client localizes and shows,
/// and flags — `0` for a plain enabled entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContextMenuEntry {
    /// The cliloc the client looks up and shows.
    pub cliloc: ClilocId,
    /// [`ContextMenuFlags::NONE`] for a plain enabled entry.
    pub flags:  ContextMenuFlags,
}

/// `0xBF` subcommand `0x14` — draw a context menu on an object. Variable length.
///
/// The new (`0x02`) format, which every client since 6.0.0.0
/// ([`Feature::NewContextMenu`](crate::feature::Feature::NewContextMenu)) reads: each entry
/// is a four-byte cliloc, a two-byte tag (its position, sent back on select), and
/// two-byte flags. Ported from ServUO's `DisplayContextMenu`.
///
/// Unlike [`crate::mobile::StatLocks`] or [`crate::mobile::MapChange`](crate::world::MapChange),
/// this `0xBF` subcommand is genuinely variable — the entry count is the caller's
/// — so it needs no hand-written length literal: the envelope's own `u16` length
/// field sits at the same offset every packet's does, and [`crate::packet::frame_body`]
/// patches it exactly as it would for any other [`PacketLength::Variable`] payload.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ContextMenu {
    /// The object the menu was opened on.
    pub serial:  Serial,
    /// The entries to show, in the order the client tags them from zero.
    pub entries: Vec<ContextMenuEntry>,
}

impl EncodePacket for ContextMenu {
    const ID: u8 = 0xBF;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(0x14); // subcommand: display popup
        out.u16(0x02); // the new format
        out.u32(self.serial.raw());
        out.u8(self.entries.len() as u8);
        for (index, entry) in self.entries.iter().enumerate() {
            out.u32(entry.cliloc.0);
            out.u16(index as u16); // the tag the client returns on select
            out.u16(entry.flags.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extended::ExtendedRequest;

    #[test]
    fn a_request_reads_its_serial() {
        let mut bytes = vec![0xBF, 0, 0];
        bytes.extend_from_slice(&ContextMenuRequest::SUBCOMMAND.to_be_bytes());
        bytes.extend_from_slice(&0x0000_1234u32.to_be_bytes());
        let len = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&len.to_be_bytes());

        let request = ExtendedRequest::decode(&bytes).unwrap();
        assert_eq!(
            request,
            ExtendedRequest::ContextMenuRequest(ContextMenuRequest {
                serial: RawSerial(0x0000_1234),
            })
        );
    }

    #[test]
    fn a_select_reads_its_serial_and_index() {
        let mut bytes = vec![0xBF, 0, 0];
        bytes.extend_from_slice(&ContextMenuSelect::SUBCOMMAND.to_be_bytes());
        bytes.extend_from_slice(&0x0000_5678u32.to_be_bytes());
        bytes.extend_from_slice(&2u16.to_be_bytes());
        let len = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&len.to_be_bytes());

        let select = ExtendedRequest::decode(&bytes).unwrap();
        assert_eq!(
            select,
            ExtendedRequest::ContextMenuSelect(ContextMenuSelect {
                serial: RawSerial(0x0000_5678),
                index:  RawContextMenuIndex(2),
            })
        );
    }

    #[test]
    fn a_menu_tags_each_entry_with_its_position() {
        let packet = crate::packet::encode_packet(
            &ContextMenu {
                serial:  Serial::new(0x0000_00AB).unwrap(),
                entries: vec![
                    ContextMenuEntry {
                        cliloc: ClilocId(3_000_362),
                        flags:  ContextMenuFlags::NONE,
                    },
                    ContextMenuEntry {
                        cliloc: ClilocId(6_103),
                        flags:  ContextMenuFlags::NONE,
                    },
                ],
            },
            ClientVersion::new(7, 0, 45, 65),
        );
        assert_eq!(packet[0], 0xBF);
        assert_eq!(u16::from_be_bytes([packet[1], packet[2]]), packet.len() as u16);
        assert_eq!(&packet[3..5], &0x14u16.to_be_bytes(), "display-popup subcommand");
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

    #[test]
    fn an_entry_no_menu_offered_decodes_cleanly_and_is_refused() {
        // N9's pair. A `0x15` naming entry 7 of a two-entry menu is a window
        // this side never drew; it must not be a decode error, because that
        // would drop the connection over a value that is only a lie, and it
        // must not run entry 1 either.
        let mut bytes = vec![0xBF, 0, 0];
        bytes.extend_from_slice(&ContextMenuSelect::SUBCOMMAND.to_be_bytes());
        bytes.extend_from_slice(&0x0000_5678u32.to_be_bytes());
        bytes.extend_from_slice(&7u16.to_be_bytes());
        let len = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&len.to_be_bytes());

        let ExtendedRequest::ContextMenuSelect(select) = ExtendedRequest::decode(&bytes).unwrap() else {
            panic!("a 0x15 decodes as a select");
        };
        assert_eq!(select.index, RawContextMenuIndex(7), "the tag arrives intact");
        assert_eq!(
            select.index.validate(2),
            Err(InvalidContextMenuIndex {
                tag:     7,
                offered: 2,
            }),
            "and names nothing the menu offered"
        );
    }

    #[test]
    fn every_entry_a_menu_drew_promotes() {
        // The other half of the pair: the tags a menu of two hands out are
        // exactly 0 and 1, and both survive the check.
        assert_eq!(
            RawContextMenuIndex(0).validate(2),
            Ok(ContextMenuIndex(0)),
            "the first row"
        );
        assert_eq!(
            RawContextMenuIndex(1).validate(2),
            Ok(ContextMenuIndex(1)),
            "the last row"
        );
        assert!(
            RawContextMenuIndex(0).validate(0).is_err(),
            "an object with no menu offers no entry, not entry zero"
        );
    }
}
