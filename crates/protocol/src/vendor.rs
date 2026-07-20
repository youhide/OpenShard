//! The shopkeeper's counter: the buy list, the sell list, and the client's
//! answers to both.
//!
//! The classic flow, agreed on by both reference emulators: a vendor's goods
//! travel as an ordinary container (`0x24`/`0x3C`) and `0x74` rides alongside
//! carrying a price and a label per item, paired with the contents *by order*.
//! The client answers a purchase with `0x3B`. Selling is one packet each way:
//! `0x9E` lists what the vendor will take from the player's pack (with offered
//! prices), `0x9F` names what the player let go.

use crate::codec::PacketWriter;
use crate::login::{expect_id, LoginDecodeError};

/// One line of a vendor's buy list: the price and label for one stock item, in
/// the same order as the `0x3C` contents it rides beside.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BuyLine {
    /// What one unit costs.
    pub price: u32,
    /// The label the client shows — usually the item's name.
    pub name: String,
}

/// `0x74` — the prices and labels for a vendor's buy container.
#[must_use]
pub fn encode_buy_list(container: u32, lines: &[BuyLine]) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(8 + lines.len() * 24);
    writer.u8(0x74);
    writer.u16(0); // length, patched below
    writer.u32(container);
    writer.u8(lines.len() as u8);
    for line in lines {
        writer.u32(line.price);
        // ServUO's `VendorBuyList`: the length counts a trailing NUL, and the
        // description is written NUL-terminated. Cap at 254 so length + the NUL
        // still fits a byte.
        let name = line.name.as_bytes();
        let take = name.len().min(u8::MAX as usize - 1);
        writer.u8((take + 1) as u8);
        writer.bytes(&name[..take]);
        writer.u8(0);
    }
    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a buy list outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

/// A purchase the client asked for: which stock item, how many.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Purchase {
    /// The stock item's serial, as listed in the `0x3C`.
    pub serial: u32,
    /// How many units.
    pub amount: u16,
}

/// `0x3B` decoded — the client's answer to the buy gump.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BuyReply {
    /// The vendor mobile.
    pub vendor: u32,
    /// What was bought; empty when the gump was closed without buying.
    pub purchases: Vec<Purchase>,
}

impl BuyReply {
    /// Decode a framed `0x3B`, header included.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, 0x3B)?;
        let _length = reader.u16()?;
        let vendor = reader.u32()?;
        let mut purchases = Vec::new();
        if reader.remaining() > 0 {
            let flag = reader.u8()?;
            // 0x02 is "bought"; anything else is a close with nothing taken.
            if flag == 0x02 {
                while reader.remaining() >= 7 {
                    let _layer = reader.u8()?;
                    let serial = reader.u32()?;
                    let amount = reader.u16()?;
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
    pub serial: u32,
    /// Its graphic.
    pub graphic: u16,
    /// Its hue.
    pub hue: u16,
    /// How many the player carries.
    pub amount: u16,
    /// What the vendor pays per unit.
    pub price: u16,
    /// The label the client shows.
    pub name: String,
}

/// `0x9E` — what the vendor offers to buy from the player.
#[must_use]
pub fn encode_sell_list(vendor: u32, lines: &[SellLine]) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(9 + lines.len() * 32);
    writer.u8(0x9E);
    writer.u16(0); // length, patched below
    writer.u32(vendor);
    writer.u16(lines.len() as u16);
    for line in lines {
        writer.u32(line.serial);
        writer.u16(line.graphic);
        writer.u16(line.hue);
        writer.u16(line.amount);
        writer.u16(line.price);
        let name = line.name.as_bytes();
        let take = name.len().min(u16::MAX as usize);
        writer.u16(take as u16);
        writer.bytes(&name[..take]);
    }
    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a sell list outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

/// A sale the client confirmed: which of the player's items, how many.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sale {
    /// The player's item.
    pub serial: u32,
    /// How many units.
    pub amount: u16,
}

/// `0x9F` decoded — the client's answer to the sell gump.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SellReply {
    /// The vendor mobile.
    pub vendor: u32,
    /// What was sold; empty when the gump was closed without selling.
    pub sales: Vec<Sale>,
}

impl SellReply {
    /// Decode a framed `0x9F`, header included.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, 0x9F)?;
        let _length = reader.u16()?;
        let vendor = reader.u32()?;
        let count = reader.u16()?;
        let mut sales = Vec::with_capacity(usize::from(count.min(64)));
        for _ in 0..count {
            let serial = reader.u32()?;
            let amount = reader.u16()?;
            sales.push(Sale { serial, amount });
        }
        Ok(Self { vendor, sales })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buy_list_carries_prices_and_labels_in_order() {
        let bytes = encode_buy_list(
            0x4000_0010,
            &[
                BuyLine {
                    price: 3,
                    name: "black pearl".to_owned(),
                },
                BuyLine {
                    price: 12,
                    name: "longsword".to_owned(),
                },
            ],
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

        let reply = BuyReply::decode(&bytes).unwrap();
        assert_eq!(reply.vendor, 0x0000_0AAA);
        assert_eq!(
            reply.purchases,
            vec![Purchase {
                serial: 0x4000_0020,
                amount: 5
            }]
        );
    }

    #[test]
    fn a_closed_buy_gump_buys_nothing() {
        let mut bytes = vec![0x3B, 0, 0];
        bytes.extend_from_slice(&0x0000_0AAAu32.to_be_bytes());
        let len = bytes.len() as u16;
        bytes[1..3].copy_from_slice(&len.to_be_bytes());
        assert!(BuyReply::decode(&bytes).unwrap().purchases.is_empty());
    }

    #[test]
    fn a_sell_list_round_trips_through_the_reply() {
        let list = encode_sell_list(
            0x0000_0BBB,
            &[SellLine {
                serial: 0x4000_0033,
                graphic: 0x0F7A,
                hue: 0,
                amount: 20,
                price: 2,
                name: "black pearl".to_owned(),
            }],
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
        let reply = SellReply::decode(&bytes).unwrap();
        assert_eq!(
            reply.sales,
            vec![Sale {
                serial: 0x4000_0033,
                amount: 10
            }]
        );
    }
}
