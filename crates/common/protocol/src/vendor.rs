//! The shopkeeper's counter: the buy list, the sell list, and the client's
//! answers to both.
//!
//! The classic flow, agreed on by both reference emulators: a vendor's goods
//! travel as an ordinary container (`0x24`/`0x3C`) and `0x74` rides alongside
//! carrying a price and a label per item, paired with the contents *by order*.
//! The client answers a purchase with `0x3B`. Selling is one packet each way:
//! `0x9E` lists what the vendor will take from the player's pack (with offered
//! prices), `0x9F` names what the player let go.

use crate::codec::{PacketReader, PacketWriter};
use crate::error::DecodeError;
use crate::items::ItemAmount;
use crate::packet::{DecodePacket, EncodePacket, PacketLength};
use crate::serial::{RawSerial, Serial};
use crate::version::ClientVersion;
use crate::wire::{Graphic, Hue};

/// One line of a vendor's buy list: the price and label for one stock item, in
/// the same order as the `0x3C` contents it rides beside.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BuyLine {
    /// What one unit costs, in gold.
    ///
    /// A bare integer by decision, on `docs/protocol_newtypes.md`'s N10
    /// allowlist: a price is a quantity in the sense `MobileStatus`'s `gold`
    /// is — multiplied by an amount, added into a total, compared against what
    /// a purse holds — and the rules that bound it (what a vendor charges,
    /// what it pays back) live in `openshard_npc`, far above `protocol`.
    pub price: u32,
    /// The label the client shows — usually the item's name.
    pub name: String,
}

/// `0x74` — the prices and labels for a vendor's buy container.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BuyList {
    /// The stock container the lines pair with, by order.
    pub container: Serial,
    /// One line per stocked item.
    pub lines: Vec<BuyLine>,
}

impl EncodePacket for BuyList {
    const ID: u8 = 0x74;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.container.raw());
        out.u8(self.lines.len() as u8);
        for line in &self.lines {
            out.u32(line.price);
            // ServUO's `VendorBuyList`: the length counts a trailing NUL, and
            // the description is written NUL-terminated. Cap at 254 so length
            // + the NUL still fits a byte.
            let name = line.name.as_bytes();
            let take = name.len().min(u8::MAX as usize - 1);
            out.u8((take + 1) as u8);
            out.bytes(&name[..take]);
            out.u8(0);
        }
    }
}

impl DecodePacket for BuyList {
    const ID: u8 = 0x74;

    /// The label's length byte counts the trailing NUL the encoder writes, so
    /// the name is that many bytes less one — a list read as if the byte were
    /// the text length alone would eat the next line's price.
    ///
    /// A short name is truncated at the first NUL rather than kept whole: the
    /// reference client writes fixed-size buffers and pads them, and a label
    /// with a `\0` in the middle of it is not a label.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let container = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x74 stock container serial",
            value: raw,
        })?;
        let count = reader.u8()?;
        let mut lines = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            let price = reader.u32()?;
            let length = usize::from(reader.u8()?);
            let text = reader.bytes(length)?;
            let name = String::from_utf8_lossy(
                text.iter()
                    .position(|byte| *byte == 0)
                    .map_or(text, |end| &text[..end]),
            )
            .into_owned();
            lines.push(BuyLine { price, name });
        }
        Ok(Self { container, lines })
    }
}

/// A purchase the client asked for: which stock item, how many.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Purchase {
    /// The stock item, as the client read it off the `0x3C` — four bytes it
    /// chose, checked where the sale is settled.
    pub serial: RawSerial,
    /// How many units the client asked for.
    ///
    /// On N10's allowlist for `PickUpItem::amount`'s reason: a stack size is a
    /// quantity, and the check that matters — is there that much on the
    /// shelf — exists today in `openshard_npc::vendor::buy`, which takes
    /// `have.min(purchase.amount)`.
    pub amount: ItemAmount,
}

/// `0x3B` decoded — the client's answer to the buy gump.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BuyReply {
    /// The vendor mobile, as the client named it.
    pub vendor: RawSerial,
    /// What was bought; empty when the gump was closed without buying.
    pub purchases: Vec<Purchase>,
}

impl DecodePacket for BuyReply {
    const ID: u8 = 0x3B;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let vendor = RawSerial(reader.u32()?);
        let mut purchases = Vec::new();
        if reader.remaining() > 0 {
            let flag = reader.u8()?;
            // 0x02 is "bought"; anything else is a close with nothing taken.
            //
            // This byte is read and not kept, which is the *opposite* of
            // `StatLockRequest`'s and `0xAD`'s findings in
            // `docs/protocol_newtypes.md`: those two folded a value they then
            // stored, destroying the client's own byte. Here the flag is pure
            // framing — it says whether a list follows — and the two answers it
            // distinguishes ("closed" and "bought nothing") are the same empty
            // basket to everything downstream, so there is nothing to preserve.
            if flag == 0x02 {
                while reader.remaining() >= 7 {
                    let _layer = reader.u8()?;
                    let serial = RawSerial(reader.u32()?);
                    let amount = ItemAmount(reader.u16()?);
                    purchases.push(Purchase { serial, amount });
                }
            }
        }
        Ok(Self { vendor, purchases })
    }
}

/// One line of a sell list: an item from the player's pack the vendor will
/// take, and the price offered for each unit.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SellLine {
    /// The player's item.
    pub serial: Serial,
    /// Its graphic.
    pub graphic: Graphic,
    /// Its hue.
    pub hue: Hue,
    /// How many the player carries. A quantity, on N10's allowlist for
    /// [`Purchase::amount`]'s reason.
    pub amount: ItemAmount,
    /// What the vendor pays per unit. A quantity, on N10's allowlist for
    /// [`BuyLine::price`]'s reason.
    pub price: u16,
    /// The label the client shows.
    pub name: String,
}

/// `0x9E` — what the vendor offers to buy from the player.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SellList {
    /// The vendor mobile.
    pub vendor: Serial,
    /// One line per item the vendor will take.
    pub lines: Vec<SellLine>,
}

impl EncodePacket for SellList {
    const ID: u8 = 0x9E;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.vendor.raw());
        out.u16(self.lines.len() as u16);
        for line in &self.lines {
            out.u32(line.serial.raw());
            out.u16(line.graphic.0);
            out.u16(line.hue.0);
            out.u16(line.amount.0);
            out.u16(line.price);
            let name = line.name.as_bytes();
            let take = name.len().min(u16::MAX as usize);
            out.u16(take as u16);
            out.bytes(&name[..take]);
        }
    }
}

impl DecodePacket for SellList {
    const ID: u8 = 0x9E;

    /// The label here is *not* NUL-terminated — the length is the text's own —
    /// which is the one way this differs from [`BuyList`]'s lines and the one
    /// way a decoder written from the other one would be wrong.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw = reader.u32()?;
        let vendor = Serial::new(raw).ok_or(DecodeError::UnknownValue {
            field: "0x9E vendor serial",
            value: raw,
        })?;
        let count = reader.u16()?;
        // Bounded rather than trusted: the count is two bytes and the body is
        // the shard's, but a reserve of 65,535 lines on a malformed packet is a
        // cost this end need not pay before it has read one.
        let mut lines = Vec::with_capacity(usize::from(count.min(64)));
        for _ in 0..count {
            let raw_item = reader.u32()?;
            let serial = Serial::new(raw_item).ok_or(DecodeError::UnknownValue {
                field: "0x9E offered item serial",
                value: raw_item,
            })?;
            let graphic = Graphic(reader.u16()?);
            let hue = Hue(reader.u16()?);
            let amount = ItemAmount(reader.u16()?);
            let price = reader.u16()?;
            let length = usize::from(reader.u16()?);
            let name = String::from_utf8_lossy(reader.bytes(length)?).into_owned();
            lines.push(SellLine {
                serial,
                graphic,
                hue,
                amount,
                price,
                name,
            });
        }
        Ok(Self { vendor, lines })
    }
}

/// A sale the client confirmed: which of the player's items, how many.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sale {
    /// The player's item, as the client named it.
    pub serial: RawSerial,
    /// How many units the client let go. A quantity, on N10's allowlist for
    /// [`Purchase::amount`]'s reason.
    pub amount: ItemAmount,
}

/// `0x9F` decoded — the client's answer to the sell gump.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SellReply {
    /// The vendor mobile, as the client named it.
    pub vendor: RawSerial,
    /// What was sold; empty when the gump was closed without selling.
    pub sales: Vec<Sale>,
}

impl DecodePacket for SellReply {
    const ID: u8 = 0x9F;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let vendor = RawSerial(reader.u32()?);
        let count = reader.u16()?;
        let mut sales = Vec::with_capacity(usize::from(count.min(64)));
        for _ in 0..count {
            let serial = RawSerial(reader.u32()?);
            let amount = ItemAmount(reader.u16()?);
            sales.push(Sale { serial, amount });
        }
        Ok(Self { vendor, sales })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{decode_packet, encode_packet};

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    #[test]
    fn a_buy_list_carries_prices_and_labels_in_order() {
        let bytes = encode_packet(
            &BuyList {
                container: Serial::new(0x4000_0010).unwrap(),
                lines: vec![
                    BuyLine {
                        price: 3,
                        name: "black pearl".to_owned(),
                    },
                    BuyLine {
                        price: 12,
                        name: "longsword".to_owned(),
                    },
                ],
            },
            version(),
        );
        assert_eq!(bytes[0], 0x74);
        let length = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        assert_eq!(length, bytes.len(), "the patched length matches the frame");
        assert_eq!(&bytes[3..7], &0x4000_0010u32.to_be_bytes());
        assert_eq!(bytes[7], 2, "two lines");
        assert_eq!(&bytes[8..12], &3u32.to_be_bytes());
        // The length counts the trailing NUL, and the name is NUL-terminated.
        assert_eq!(bytes[12] as usize, "black pearl".len() + 1);
        assert_eq!(&bytes[13..13 + "black pearl".len()], b"black pearl");
        assert_eq!(bytes[13 + "black pearl".len()], 0, "NUL-terminated");
    }

    #[test]
    fn a_buy_reply_lists_the_purchases() {
        let mut bytes = vec![0x3B, 0, 0];
        bytes.extend_from_slice(&0x0000_0AAAu32.to_be_bytes());
        bytes.push(0x02);
        bytes.push(0x1A);
        bytes.extend_from_slice(&0x4000_0020u32.to_be_bytes());
        bytes.extend_from_slice(&5u16.to_be_bytes());
        let len = bytes.len() as u16;
        bytes[1..3].copy_from_slice(&len.to_be_bytes());

        let reply: BuyReply = decode_packet(&bytes, version()).unwrap();
        assert_eq!(reply.vendor, RawSerial(0x0000_0AAA));
        assert_eq!(
            reply.purchases,
            vec![Purchase {
                serial: RawSerial(0x4000_0020),
                amount: ItemAmount(5)
            }]
        );
    }

    #[test]
    fn a_closed_buy_gump_buys_nothing() {
        let mut bytes = vec![0x3B, 0, 0];
        bytes.extend_from_slice(&0x0000_0AAAu32.to_be_bytes());
        let len = bytes.len() as u16;
        bytes[1..3].copy_from_slice(&len.to_be_bytes());
        let reply: BuyReply = decode_packet(&bytes, version()).unwrap();
        assert!(reply.purchases.is_empty());
    }

    #[test]
    fn a_sell_list_round_trips_through_the_reply() {
        let list = encode_packet(
            &SellList {
                vendor: Serial::new(0x0000_0BBB).unwrap(),
                lines: vec![SellLine {
                    serial: Serial::new(0x4000_0033).unwrap(),
                    graphic: Graphic(0x0F7A),
                    hue: Hue::NONE,
                    amount: ItemAmount(20),
                    price: 2,
                    name: "black pearl".to_owned(),
                }],
            },
            version(),
        );
        assert_eq!(list[0], 0x9E);
        let length = u16::from_be_bytes([list[1], list[2]]) as usize;
        assert_eq!(length, list.len());

        let mut bytes = vec![0x9F, 0, 0];
        bytes.extend_from_slice(&0x0000_0BBBu32.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&0x4000_0033u32.to_be_bytes());
        bytes.extend_from_slice(&10u16.to_be_bytes());
        let len = bytes.len() as u16;
        bytes[1..3].copy_from_slice(&len.to_be_bytes());
        let reply: SellReply = decode_packet(&bytes, version()).unwrap();
        assert_eq!(
            reply.sales,
            vec![Sale {
                serial: RawSerial(0x4000_0033),
                amount: ItemAmount(10)
            }]
        );
    }

    #[test]
    fn a_shelf_serial_no_client_could_own_survives_decoding_and_is_refused() {
        // N9's pair for `Purchase::serial`: `0xFFFF_FFFF` is past the item pool
        // and addresses nothing, and the split
        // `docs/protocol_newtypes.md` N2 draws says so at the *seam* — the
        // packet still decodes, because a framing error would drop the
        // connection over a value that is merely a lie.
        let mut bytes = vec![0x3B, 0, 0];
        bytes.extend_from_slice(&0x0000_0AAAu32.to_be_bytes());
        bytes.push(0x02);
        bytes.push(0x1A);
        bytes.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        bytes.extend_from_slice(&1u16.to_be_bytes());
        let len = bytes.len() as u16;
        bytes[1..3].copy_from_slice(&len.to_be_bytes());

        let reply: BuyReply = decode_packet(&bytes, version()).unwrap();
        assert_eq!(
            reply.purchases[0].serial,
            RawSerial(0xFFFF_FFFF),
            "the byte the client sent arrives intact"
        );
        assert_eq!(
            reply.purchases[0].serial.validate(),
            None,
            "and buys nothing when the sale is settled"
        );
    }
}
