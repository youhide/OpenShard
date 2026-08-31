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

use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::feature::Feature;
use crate::gump::GumpPoint;
use crate::items::ItemAmount;
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
    frame_body,
};
use crate::serial::{
    RawSerial,
    Serial,
};
use crate::version::ClientVersion;
use crate::wire::{
    Graphic,
    Hue,
};

/// The container-type byte a High Seas client expects in `0x24` for a normal
/// container (a vendor's is `0x00`, which is not this).
const CONTAINER_TYPE: u16 = 0x7D;

/// The gump id that makes a `0x24` draw a *book* rather than a container.
///
/// A spellbook, a runebook and a book of gate travel are all containers on the
/// wire, and this is the one value that tells the client to open the page view
/// instead of a bag — see [`crate::spellbook`].
pub const BOOK_GUMP: Graphic = Graphic(0xFFFF);

/// Bit 31 of a `0x06`'s serial: the client is asking for a *paperdoll*, not a
/// use.
///
/// Nothing addressable ever has this bit — the item pool stops at
/// `0x7FFF_FFFF` — so it is free for the client to flag with, and it flags the
/// paperdoll macro and the paperdoll the client opens for itself at login.
const PAPERDOLL_REQUEST: u32 = 0x8000_0000;

/// `0x06` — the client double-clicked an object. 5 bytes.
///
/// Double-click is "use this": a container opens, a door swings, a food is
/// eaten. The server decides what the object does; this only says which.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DoubleClick {
    /// The object's serial, with the paperdoll bit still on it —
    /// [`interpret`](Self::interpret) is what takes the two apart.
    pub serial: RawSerial,
}

impl DecodePacket for DoubleClick {
    const ID: u8 = 0x06;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            serial: RawSerial(reader.u32()?),
        })
    }
}

/// What a `0x06` is actually asking for.
///
/// The two are *not* the same request and answering one with the other is a
/// bug this engine has already had: ServUO's `UseReq` routes a paperdoll
/// request straight to `OnPaperdollRequest` and never to `Use`, so treating
/// the login-time paperdoll open as a self-double-click dismounted a rider a
/// breath after they logged in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UseRequest {
    /// Open this mobile's paperdoll, and do nothing else.
    Paperdoll(RawSerial),
    /// Use the object: open the container, swing the door, eat the food.
    Use(RawSerial),
}

impl DoubleClick {
    /// Build the distinct `0x06` form that asks for a mobile's paperdoll.
    ///
    /// The high bit is protocol meaning, not part of the serial.  Keeping the
    /// operation beside [`interpret`](Self::interpret) prevents clients from
    /// accidentally sending a normal double-click when they mean the paperdoll
    /// request that ServUO routes around ordinary `Use` handling.
    #[must_use]
    pub const fn paperdoll(serial: RawSerial) -> Self {
        Self {
            serial: RawSerial(serial.0 | PAPERDOLL_REQUEST),
        }
    }

    /// Encode a whole `0x06`. What `crates/client/net` sends when the player
    /// double-clicks something; this *server* never sends one, only ever decodes
    /// it — the same split as
    /// [`UnicodeTalkRequest::encode`](crate::speech::UnicodeTalkRequest::encode).
    ///
    /// The serial goes out exactly as it is held, paperdoll bit and all: the bit
    /// is part of what the request *says*, and stripping it here would leave a
    /// caller no way to ask for a paperdoll at all.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        frame_body(
            <Self as DecodePacket>::ID,
            PacketLength::Fixed(5),
            |out: &mut PacketWriter| out.u32(self.serial.0),
        )
    }

    /// Split the paperdoll bit off the serial.
    ///
    /// Total, and deliberately so: both arms carry a [`RawSerial`], because
    /// stripping a flag bit does not make what is left address anything. The
    /// check that it does is [`RawSerial::validate`], at whichever seam acts on
    /// the request — see `docs/protocol_newtypes.md` N2.
    #[must_use]
    pub const fn interpret(self) -> UseRequest {
        if self.serial.0 & PAPERDOLL_REQUEST == 0 {
            UseRequest::Use(self.serial)
        } else {
            UseRequest::Paperdoll(RawSerial(self.serial.0 & !PAPERDOLL_REQUEST))
        }
    }
}

/// Which cell of the enhanced client's grid view an item sits in.
///
/// The server picks it — [`ContainedItem`] goes out only — and the classic 2D
/// client ignores it entirely, positioning by `x`/`y` instead. A named byte
/// rather than an index type with a range: the grid's size is the client's, and
/// this engine has never had a reason to learn it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Default)]
pub struct GridSlot(pub u8);

/// One item as it sits inside a container: what `0x25` and `0x3C` write per item.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ContainedItem {
    /// The item's serial.
    pub serial:  Serial,
    /// Its graphic.
    pub graphic: Graphic,
    /// Its stack size.
    pub amount:  ItemAmount,
    /// Where its icon sits inside the container's gump art.
    ///
    /// The pair N4 left on the allowlist for N5 to name: it is a gump
    /// coordinate, not a world one, and [`GumpPoint`] is the type — measured
    /// from the art's top left here, and from the screen's for a window. Two
    /// bytes go out where a window's four do; the value is the server's, and a
    /// container's art is a few hundred pixels wide.
    pub at:      GumpPoint,
    /// Its slot in the enhanced grid view. Sent only to grid clients.
    pub grid:    GridSlot,
    /// Its hue.
    pub hue:     Hue,
}

impl ContainedItem {
    /// Write one item record: the shared body of `0x25` and `0x3C`.
    fn write(&self, writer: &mut PacketWriter, container: Serial, grid: bool) {
        writer.u32(self.serial.raw());
        writer.u16(self.graphic.0);
        writer.u8(0); // graphic offset, always zero
        writer.u16(self.amount.0);
        writer.u16(self.at.x as u16);
        writer.u16(self.at.y as u16);
        if grid {
            writer.u8(self.grid.0);
        }
        writer.u32(container.raw());
        writer.u16(self.hue.0);
    }

    /// Read one item record, and the container it names.
    ///
    /// The container comes back beside the item rather than inside it because
    /// that is how the wire carries it: **every record names its own
    /// container**, and there is no header field anywhere in `0x25` or `0x3C`
    /// that says it once. [`ContainerContents`] is where the two are put back
    /// together, and the reason its container is an `Option`.
    ///
    /// The graphic-offset byte is read and dropped, exactly as
    /// [`write`](Self::write) writes a zero for it: no reference emulator has
    /// ever sent a non-zero one, and the classic client adds it to the graphic
    /// before drawing — so a value here would change what is drawn and this
    /// engine has nothing that would produce one.
    fn read(reader: &mut PacketReader<'_>, grid: bool) -> Result<(Self, Serial), DecodeError> {
        let raw = reader.u32()?;
        let serial = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "contained item serial",
            value: raw,
        })?;
        let graphic = Graphic(reader.u16()?);
        reader.u8()?; // graphic offset, always zero
        let amount = ItemAmount(reader.u16()?);
        let at = GumpPoint::new(i32::from(reader.u16()?), i32::from(reader.u16()?));
        let slot = if grid { GridSlot(reader.u8()?) } else { GridSlot(0) };
        let raw_container = reader.u32()?;
        let container = Serial::new(raw_container).ok_or(DecodeError::UnknownValue {
            field: "contained item container",
            value: raw_container,
        })?;
        let hue = Hue(reader.u16()?);
        Ok((
            Self {
                serial,
                graphic,
                amount,
                at,
                grid: slot,
                hue,
            },
            container,
        ))
    }
}

/// `0x24` — open a container gump on the client. 7 bytes, 9 on High Seas.
///
/// # Not an `EncodePacket`
///
/// This is `0xB9`'s problem from Stage 2 (`docs/protocol_rewrite.md`) again: the
/// packet is fixed-length, but *which* fixed length depends on
/// [`Feature::HsPackets`], and [`EncodePacket::LENGTH`] is a `const` that cannot
/// ask a payload's own `version`. Neither `Fixed` nor `Variable` describes it, so
/// it stays out of that trait rather than being forced into a model it does not
/// fit.
///
/// What it *is* is a [`ServerPacket`](crate::server_packet::ServerPacket)
/// variant like any other, because that enum asks its payload for a length with
/// the version in hand. The `LENGTH` const is the only thing 0x24 cannot answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenContainer {
    /// The container whose window is opening.
    pub container: Serial,
    /// The gump art its background is drawn from, or [`BOOK_GUMP`] for a book.
    pub gump:      Graphic,
}

impl OpenContainer {
    /// Write the body, without the id byte: what the framer wraps.
    pub(crate) fn write_body(self, out: &mut PacketWriter, version: ClientVersion) {
        out.u32(self.container.raw());
        out.u16(self.gump.0);
        if version.supports(Feature::HsPackets) {
            out.u16(CONTAINER_TYPE);
        }
    }
}

impl DecodePacket for OpenContainer {
    const ID: u8 = 0x24;

    /// The High Seas container type is left unread rather than kept. It is a
    /// constant per container kind ([`CONTAINER_TYPE`] for a bag, `0x00` for a
    /// vendor's list) and nothing this end draws is decided by it: the *gump* is
    /// what says what the window looks like.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let container = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x24 container serial",
            value: raw,
        })?;
        Ok(Self {
            container,
            gump: Graphic(reader.u16()?),
        })
    }
}

/// Encode a whole `0x24`, header and all.
///
/// Kept beside [`OpenContainer`] rather than folded into it: a shard sends this
/// one straight down a connection as bytes, and going through
/// [`ServerPacket`](crate::server_packet::ServerPacket) to do it would be a
/// match on a value that is known at the call site.
pub fn encode_open_container(serial: Serial, gump: Graphic, version: ClientVersion) -> Vec<u8> {
    let packet = OpenContainer {
        container: serial,
        gump,
    };
    let mut writer = PacketWriter::with_capacity(open_container_length(version).minimum());
    writer.u8(<OpenContainer as DecodePacket>::ID);
    packet.write_body(&mut writer, version);
    debug_assert_eq!(writer.len(), open_container_length(version).minimum());
    writer.into_bytes()
}

/// How [`encode_open_container`] is framed, for the client version it was
/// written for.
///
/// The rule — High Seas adds a two-byte container type — lives here, next to the
/// encoder that obeys it, so a framer can ask rather than carry its own copy of
/// the same `if`.
#[must_use]
pub fn open_container_length(version: ClientVersion) -> PacketLength {
    PacketLength::Fixed(if version.supports(Feature::HsPackets) {
        9
    } else {
        7
    })
}

/// `0x25` — add one item to a container gump the client already has open.
///
/// The same version-dependent-fixed-size shape as [`OpenContainer`], this time
/// gated on [`Feature::ItemGrid`], and out of `EncodePacket` for the same reason.
///
/// Two fields where the wire writes the container inside the item record: see
/// [`ContainedItem::read`] for why the container is carried beside the item and
/// not in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AddToContainer {
    /// The item that has appeared.
    pub item:      ContainedItem,
    /// The container it appeared in.
    pub container: Serial,
}

impl AddToContainer {
    /// Write the body, without the id byte: what the framer wraps.
    pub(crate) fn write_body(self, out: &mut PacketWriter, version: ClientVersion) {
        self.item
            .write(out, self.container, version.supports(Feature::ItemGrid));
    }
}

impl DecodePacket for AddToContainer {
    const ID: u8 = 0x25;

    fn decode_body(reader: &mut PacketReader<'_>, version: ClientVersion) -> Result<Self, DecodeError> {
        let (item, container) = ContainedItem::read(reader, version.supports(Feature::ItemGrid))?;
        Ok(Self { item, container })
    }
}

/// Encode a whole `0x25`, header and all — [`encode_open_container`]'s sibling,
/// and there for the same reason.
pub fn encode_add_to_container(item: ContainedItem, container: Serial, version: ClientVersion) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(add_to_container_length(version).minimum());
    writer.u8(<AddToContainer as DecodePacket>::ID);
    AddToContainer { item, container }.write_body(&mut writer, version);
    debug_assert_eq!(writer.len(), add_to_container_length(version).minimum());
    writer.into_bytes()
}

/// How [`encode_add_to_container`] is framed. The grid byte is the whole
/// difference; see [`open_container_length`] for why this lives here.
#[must_use]
pub fn add_to_container_length(version: ClientVersion) -> PacketLength {
    PacketLength::Fixed(if version.supports(Feature::ItemGrid) {
        21
    } else {
        20
    })
}

/// `0x3C` — the full contents of a container, all at once. Variable length.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ContainerContents {
    /// The container being filled, and `None` for a listing that names none.
    ///
    /// The `Option` is the wire's, not a convenience: a `0x3C` has no header
    /// field for the container — **each record carries it** (see
    /// [`ContainedItem::read`]) — so a listing with no items has said nothing
    /// about which container it was about, and there is no byte left to ask.
    /// That case is real and this engine sends it: opening an empty chest is a
    /// `0x24` naming the container and a `0x3C` with a count of zero.
    ///
    /// A writer therefore has nothing to write when this is `None`, and
    /// [`encode_body`](EncodePacket::encode_body) says so.
    pub container: Option<Serial>,
    /// Everything inside it.
    pub items:     Vec<ContainedItem>,
}

impl EncodePacket for ContainerContents {
    const ID: u8 = 0x3C;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        let Some(container) = self.container else {
            // Nowhere to put the records: every one of them is addressed by the
            // container it names, and this listing names none. An empty count
            // is the only thing these bytes can honestly say.
            debug_assert!(
                self.items.is_empty(),
                "a 0x3C with items but no container: the records have no address to go out under"
            );
            out.u16(0);
            return;
        };
        let grid = version.supports(Feature::ItemGrid);
        out.u16(self.items.len() as u16);
        for item in &self.items {
            item.write(out, container, grid);
        }
    }
}

impl DecodePacket for ContainerContents {
    const ID: u8 = 0x3C;

    /// Every record must name the same container.
    ///
    /// Nothing in the protocol forbids a mixed listing and no reference emulator
    /// has ever sent one — RunUO's `ContainerContent` is built from one
    /// container's items and ServUO's is the same code. Refused rather than
    /// split into several, because a client that opened two windows out of one
    /// `0x3C` would be inventing a shape the packet does not have.
    fn decode_body(reader: &mut PacketReader<'_>, version: ClientVersion) -> Result<Self, DecodeError> {
        let grid = version.supports(Feature::ItemGrid);
        let count = reader.u16()?;
        let mut container = None;
        let mut items = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            let (item, named) = ContainedItem::read(reader, grid)?;
            match container {
                None => container = Some(named),
                Some(first) if first == named => {}
                Some(_) => {
                    return Err(DecodeError::Unsupported {
                        packet: <Self as DecodePacket>::ID,
                        form:   "one listing naming two containers",
                    });
                }
            }
            items.push(item);
        }
        Ok(Self { container, items })
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

    /// A version with the grid index and the High Seas container type.
    fn modern() -> ClientVersion {
        ClientVersion::new(7, 0, 9, 0)
    }

    /// A version with neither.
    fn classic() -> ClientVersion {
        ClientVersion::new(5, 0, 0, 0)
    }

    /// The one container every test in here fills.
    fn container() -> Serial {
        Serial::new(0x4000_0001).unwrap()
    }

    #[test]
    fn a_double_click_is_a_serial() {
        let bytes = [0x06, 0x40, 0x00, 0x00, 0x2A];
        let click: DoubleClick = decode_packet(&bytes, classic()).unwrap();
        assert_eq!(click.serial, RawSerial(0x4000_002A));
        assert_eq!(click.interpret(), UseRequest::Use(RawSerial(0x4000_002A)));
    }

    /// The other direction of the same packet: what a client writes is what this
    /// decoder reads. Both bits of the request survive the trip — the serial and
    /// the paperdoll flag on top of it — because the encoder is deliberately not
    /// allowed to normalise either.
    #[test]
    fn a_double_click_this_client_writes_is_one_this_server_reads() {
        for raw in [0x4000_002A, 0x8000_002A] {
            let bytes = DoubleClick {
                serial: RawSerial(raw),
            }
            .encode();
            assert_eq!(bytes.len(), 5, "a 0x06 is five bytes, header included");
            let heard: DoubleClick = decode_packet(&bytes, classic()).unwrap();
            assert_eq!(heard.serial, RawSerial(raw));
        }
    }

    #[test]
    fn the_top_bit_of_a_double_click_asks_for_a_paperdoll() {
        // The same object, asked for the other way: bit 31 set, and what is
        // left is the mobile whose paperdoll is wanted.
        let bytes = [0x06, 0x80, 0x00, 0x00, 0x2A];
        let click: DoubleClick = decode_packet(&bytes, classic()).unwrap();
        assert_eq!(
            click.serial,
            RawSerial(0x8000_002A),
            "the bit survives decoding — the packet is not normalised on the way in"
        );
        assert_eq!(click.interpret(), UseRequest::Paperdoll(RawSerial(0x2A)));
    }

    #[test]
    fn every_double_click_interprets() {
        // Class B is total: the bit is either set or it is not, and both arms
        // hand back a serial nobody has checked yet.
        for high in [0u32, 1] {
            for low in [0u32, 1, 0x4000_002A, 0x7FFF_FFFF] {
                let raw = (high << 31) | low;
                let click = DoubleClick {
                    serial: RawSerial(raw),
                };
                let expected = RawSerial(low);
                match click.interpret() {
                    UseRequest::Use(serial) => {
                        assert_eq!(high, 0);
                        assert_eq!(serial, expected);
                    }
                    UseRequest::Paperdoll(serial) => {
                        assert_eq!(high, 1);
                        assert_eq!(serial, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn a_double_click_on_nothing_decodes_and_is_refused_at_promotion() {
        // `docs/protocol_newtypes.md` N9: the hostile value gets all the way
        // through the framer — dropping the connection over it would be wrong —
        // and dies where it would have addressed something.
        let bytes = [0x06, 0x00, 0x00, 0x00, 0x00];
        let click: DoubleClick = decode_packet(&bytes, classic()).unwrap();
        let UseRequest::Use(serial) = click.interpret() else {
            panic!("bit 31 is clear");
        };
        assert_eq!(serial.validate(), None, "zero addresses nothing");
    }

    #[test]
    fn opening_a_container_is_seven_bytes_on_a_classic_client() {
        let packet = encode_open_container(container(), Graphic(0x003C), classic());
        assert_eq!(packet[0], 0x24);
        assert_eq!(&packet[1..5], &0x4000_0001u32.to_be_bytes());
        assert_eq!(&packet[5..7], &0x003Cu16.to_be_bytes());
        assert_eq!(packet.len(), 7, "no container-type word before High Seas");
    }

    #[test]
    fn opening_a_container_gains_the_type_word_on_high_seas() {
        let packet = encode_open_container(container(), Graphic(0x003C), modern());
        assert_eq!(packet.len(), 9);
        assert_eq!(u16::from_be_bytes([packet[7], packet[8]]), CONTAINER_TYPE);
    }

    #[test]
    fn a_classic_container_item_record_has_no_grid_byte() {
        let item = ContainedItem {
            serial:  Serial::new(0x4000_0002).unwrap(),
            graphic: Graphic(0x0EED),
            amount:  ItemAmount(3),
            at:      GumpPoint::new(44, 65),
            grid:    GridSlot(7),
            hue:     Hue::NONE,
        };
        let packet = encode_add_to_container(item, container(), classic());
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
            serial:  Serial::new(0x4000_0002).unwrap(),
            graphic: Graphic(0x0EED),
            amount:  ItemAmount(3),
            at:      GumpPoint::new(44, 65),
            grid:    GridSlot(7),
            hue:     Hue::NONE,
        };
        let packet = encode_add_to_container(item, container(), modern());
        assert_eq!(packet.len(), 21);
        assert_eq!(packet[14], 7, "the grid index sits before the container serial");
        assert_eq!(&packet[15..19], &0x4000_0001u32.to_be_bytes());
    }

    #[test]
    fn container_contents_counts_its_items_and_patches_its_length() {
        let items = [
            ContainedItem {
                serial:  Serial::new(0x4000_0002).unwrap(),
                graphic: Graphic(0x0EED),
                amount:  ItemAmount(1),
                at:      GumpPoint::new(10, 10),
                grid:    GridSlot(0),
                hue:     Hue::NONE,
            },
            ContainedItem {
                serial:  Serial::new(0x4000_0003).unwrap(),
                graphic: Graphic(0x0F0E),
                amount:  ItemAmount(5),
                at:      GumpPoint::new(20, 20),
                grid:    GridSlot(1),
                hue:     Hue(0x21),
            },
        ];
        let packet = encode_packet(
            &ContainerContents {
                container: Some(container()),
                items:     items.to_vec(),
            },
            classic(),
        );
        assert_eq!(packet[0], 0x3C);
        assert_eq!(u16::from_be_bytes([packet[1], packet[2]]), packet.len() as u16);
        assert_eq!(u16::from_be_bytes([packet[3], packet[4]]), 2, "two items");
        // header 5 + two classic records of 19 each = 43
        assert_eq!(packet.len(), 5 + 2 * 19);
    }

    /// One item, for the reading tests to send round.
    fn an_item() -> ContainedItem {
        ContainedItem {
            serial:  Serial::new(0x4000_0002).unwrap(),
            graphic: Graphic(0x0EED),
            amount:  ItemAmount(3),
            at:      GumpPoint::new(44, 65),
            grid:    GridSlot(7),
            hue:     Hue(0x21),
        }
    }

    /// What this shard writes is what its own client reads — on both sides of
    /// the High Seas seam, which is the whole risk in a `0x24`: the two-byte
    /// container type is trailing, so reading it as a modern packet on a classic
    /// stream would take two bytes belonging to the next packet.
    #[test]
    fn an_open_container_this_shard_writes_is_one_this_client_reads() {
        for version in [classic(), modern()] {
            let bytes = encode_open_container(container(), Graphic(0x003C), version);
            let Some(ServerPacket::OpenContainer(heard)) = ServerPacket::decode(&bytes, version).unwrap()
            else {
                panic!("0x24 did not decode as an open container");
            };
            assert_eq!(heard.container, container());
            assert_eq!(heard.gump, Graphic(0x003C));
        }
    }

    /// A spellbook is a container whose gump id is the one value that means
    /// "not a bag" — it has to survive the trip untouched, because the window it
    /// opens is decided by nothing else.
    #[test]
    fn a_book_gump_reaches_the_client_as_itself() {
        let bytes = encode_open_container(container(), BOOK_GUMP, modern());
        let Some(ServerPacket::OpenContainer(heard)) = ServerPacket::decode(&bytes, modern()).unwrap() else {
            panic!("0x24 did not decode as an open container");
        };
        assert_eq!(heard.gump, BOOK_GUMP);
    }

    /// The grid byte is the seam here, and it sits *inside* the record rather
    /// than after it — read with the wrong rule and the container serial and the
    /// hue both come out shifted by one byte, which is a plausible-looking wrong
    /// answer rather than an error.
    #[test]
    fn an_added_item_survives_the_trip_on_both_sides_of_the_grid_seam() {
        for (version, grid) in [(classic(), GridSlot(0)), (modern(), GridSlot(7))] {
            let bytes = encode_add_to_container(an_item(), container(), version);
            let Some(ServerPacket::AddToContainer(heard)) = ServerPacket::decode(&bytes, version).unwrap()
            else {
                panic!("0x25 did not decode as an addition");
            };
            assert_eq!(heard.container, container());
            assert_eq!(
                heard.item,
                ContainedItem { grid, ..an_item() },
                "a classic client is not sent a grid index, so it reads none"
            );
        }
    }

    #[test]
    fn a_listing_this_shard_writes_is_one_this_client_reads() {
        let items = vec![
            an_item(),
            ContainedItem {
                serial: Serial::new(0x4000_0003).unwrap(),
                at: GumpPoint::new(20, 20),
                ..an_item()
            },
        ];
        let sent = ContainerContents {
            container: Some(container()),
            items,
        };
        let bytes = encode_packet(&sent, modern());
        let Some(ServerPacket::ContainerContents(heard)) = ServerPacket::decode(&bytes, modern()).unwrap()
        else {
            panic!("0x3C did not decode as a listing");
        };
        assert_eq!(heard, sent);
    }

    /// The one thing a `0x3C` cannot say. Every record names its container and
    /// there is no header field, so a listing with no records has named nothing
    /// — and the client learns which container it was from the `0x24` that came
    /// before it.
    #[test]
    fn an_empty_listing_names_no_container() {
        let bytes = encode_packet(
            &ContainerContents {
                container: Some(container()),
                items:     Vec::new(),
            },
            modern(),
        );
        let Some(ServerPacket::ContainerContents(heard)) = ServerPacket::decode(&bytes, modern()).unwrap()
        else {
            panic!("0x3C did not decode as a listing");
        };
        assert_eq!(heard.container, None);
        assert!(heard.items.is_empty());
    }

    /// A hand-built listing whose two records disagree about their container is
    /// refused rather than silently attributed to the first one.
    #[test]
    fn a_listing_naming_two_containers_is_refused() {
        let other = Serial::new(0x4000_00FF).unwrap();
        let mut body = PacketWriter::with_capacity(64);
        body.u16(2);
        an_item().write(&mut body, container(), true);
        an_item().write(&mut body, other, true);
        let inner = body.into_bytes();

        let mut packet = PacketWriter::with_capacity(inner.len() + 3);
        packet.u8(<ContainerContents as EncodePacket>::ID);
        packet.u16((inner.len() + 3) as u16);
        packet.bytes(&inner);

        let error = ServerPacket::decode(&packet.into_bytes(), modern()).unwrap_err();
        assert!(
            matches!(
                error,
                crate::server_packet::ServerDecodeError::ContainerContents(DecodeError::Unsupported { .. })
            ),
            "{error:?}"
        );
    }

    #[test]
    fn an_empty_container_is_just_a_header() {
        let packet = encode_packet(
            &ContainerContents {
                container: Some(container()),
                items:     Vec::new(),
            },
            classic(),
        );
        assert_eq!(u16::from_be_bytes([packet[3], packet[4]]), 0);
        assert_eq!(packet.len(), 5);
    }
}
