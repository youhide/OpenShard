//! The secure trade window: `0x6F`, in both directions.
//!
//! Handing an item to another player by dropping it on the ground and trusting
//! them to pick it up is the oldest scam in the genre. The secure trade window
//! is the answer: two escrow containers, one per party, and a checkbox each.
//! Nothing moves until both boxes are ticked, and every other exit — a cancel, a
//! step out of range, a logout — puts each side's offering back where it came
//! from.
//!
//! One packet id carries the whole conversation, keyed by a leading action byte.
//! The server sends three of them and the client sends three, and the two sets
//! do *not* use the same numbering: `Display` is server-only and there is no
//! inbound action `0`.
//!
//! # Where the two references disagree
//!
//! Sphere writes a trailing `false` byte on every outgoing action but `Display`
//! (`send.cpp`'s `PacketTradeAction::prepareReadyChange`/`prepareClose`), making
//! them 17 bytes where ServUO's are 8 and 16. Sphere also contradicts *itself*
//! on the gold/platinum field order — `prepareUpdateGold` writes gold first
//! while `Trade_UpdateGold(dword platinum, dword gold)` reads it into platinum.
//! ServUO is self-consistent and is what a current ClassicUO is tested against,
//! so these follow ServUO. The packet carries its own length and the client
//! reads it field by field, so the extra byte is harmless either way; this note
//! exists so nobody "fixes" one reference to match the other later.

use crate::codec::PacketWriter;
use crate::error::{
    DecodeError,
    expect_id,
};
use crate::serial::{
    RawSerial,
    Serial,
};

/// The packet id both directions share.
pub const SECURE_TRADE: u8 = 0x6F;

/// How wide the partner's name is on the wire, NUL-padded.
const TRADE_NAME_LENGTH: usize = 30;

/// What the client asked the trade to do.
///
/// The action byte is the first field after the length. `Display` (`0`) is
/// server-to-client only, so it has no variant here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SecureTradeAction {
    /// The player closed the window: put everything back and end the trade.
    Cancel {
        /// The escrow container the window is drawn on, as the client names it.
        container: RawSerial,
    },
    /// The player ticked or unticked their checkbox.
    Accept {
        /// The escrow container the window is drawn on, as the client names it.
        container: RawSerial,
        /// Whether the box is now ticked.
        accepted:  bool,
    },
    /// The player moved virtual gold or platinum onto the trade.
    ///
    /// Only a client past [`Feature::NewSecureTrade`] sends this, and only where
    /// the server runs account-level currency. Gold is an item here, so this is
    /// decoded and ignored — the field order is ServUO's (`gold`, then `plat`),
    /// recorded so the day a shard grows a virtual bank the shape is already
    /// right.
    ///
    /// [`Feature::NewSecureTrade`]: crate::feature::Feature::NewSecureTrade
    UpdateGold {
        /// The escrow container the window is drawn on, as the client names it.
        container: RawSerial,
        /// Gold offered from the account balance.
        gold:      i32,
        /// Platinum offered from the account balance.
        platinum:  i32,
    },
}

impl SecureTradeAction {
    /// The packet id.
    pub const ID: u8 = SECURE_TRADE;

    /// The client cancelled.
    pub const CANCEL: u8 = 1;
    /// The client ticked or unticked its checkbox.
    pub const ACCEPT: u8 = 2;
    /// The client moved virtual currency onto the trade.
    pub const UPDATE_GOLD: u8 = 3;

    /// Decode a whole `0x6F` packet.
    ///
    /// An action byte this end does not know is not an error — ServUO's own
    /// handler falls through the same way — so it decodes to `None` and the
    /// caller does nothing.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, DecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        // The packet carries its own u16 length at offset 1; the framer already
        // sized the slice, so it is read past rather than trusted.
        reader.u16()?;
        let action = reader.u8()?;
        let container = RawSerial(reader.u32()?);
        Ok(match action {
            Self::CANCEL => Some(Self::Cancel { container }),
            Self::ACCEPT => {
                Some(Self::Accept {
                    container,
                    accepted: reader.i32()? != 0,
                })
            }
            Self::UPDATE_GOLD => {
                Some(Self::UpdateGold {
                    container,
                    gold: reader.i32()?,
                    platinum: reader.i32()?,
                })
            }
            _ => None,
        })
    }
}

/// Patch the `u16` length placeholder written at offset 1.
fn finish(writer: PacketWriter) -> Vec<u8> {
    let mut bytes = writer.into_bytes();
    let length = u16::try_from(bytes.len()).expect("a trade packet outgrew its u16 length");
    bytes[1..3].copy_from_slice(&length.to_be_bytes());
    bytes
}

/// `0x6F` action `0` — draw the trade window. 47 bytes.
///
/// `mine` is the container this client offers *into*; `theirs` is the partner's,
/// drawn on the other half of the window. Both references write these in that
/// order and byte for byte the same, which is as close to a specification as
/// this packet gets.
pub fn encode_trade_open(partner: Serial, mine: Serial, theirs: Serial, partner_name: &str) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(47);
    writer.u8(SECURE_TRADE);
    writer.u16(0); // length, patched below
    writer.u8(0); // Display
    writer.u32(partner.raw());
    writer.u32(mine.raw());
    writer.u32(theirs.raw());
    writer.u8(1); // "a name follows", always set
    writer.fixed_string(partner_name, TRADE_NAME_LENGTH);
    finish(writer)
}

/// `0x6F` action `1` — the trade is over; shut the window. 8 bytes.
pub fn encode_trade_close(container: Serial) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(8);
    writer.u8(SECURE_TRADE);
    writer.u16(0); // length, patched below
    writer.u8(1); // Close
    writer.u32(container.raw());
    finish(writer)
}

/// `0x6F` action `2` — redraw the two checkboxes. 16 bytes.
///
/// The container is always *this* client's own escrow, and `first` is the
/// checkbox belonging to whoever owns it — so the same pair of flags is sent to
/// the two clients in opposite order.
pub fn encode_trade_update(container: Serial, first: bool, second: bool) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(16);
    writer.u8(SECURE_TRADE);
    writer.u16(0); // length, patched below
    writer.u8(2); // Update
    writer.u32(container.raw());
    writer.i32(i32::from(first));
    writer.i32(i32::from(second));
    finish(writer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both references write this one identically, so it is pinned whole.
    /// ServUO's `DisplaySecureTrade`, Sphere's `prepareContainerOpen`.
    #[test]
    fn the_display_packet_is_forty_seven_bytes_of_two_containers_and_a_name() {
        let bytes = encode_trade_open(
            Serial::new(0x0000_0001).unwrap(),
            Serial::new(0x4000_0002).unwrap(),
            Serial::new(0x4000_0003).unwrap(),
            "Rowena",
        );
        assert_eq!(bytes.len(), 47);
        assert_eq!(bytes[0], 0x6F);
        assert_eq!(&bytes[1..3], &47u16.to_be_bytes());
        assert_eq!(bytes[3], 0); // Display
        assert_eq!(&bytes[4..8], &0x0000_0001u32.to_be_bytes());
        assert_eq!(&bytes[8..12], &0x4000_0002u32.to_be_bytes());
        assert_eq!(&bytes[12..16], &0x4000_0003u32.to_be_bytes());
        assert_eq!(bytes[16], 1);
        assert_eq!(&bytes[17..23], b"Rowena");
        assert!(bytes[23..].iter().all(|byte| *byte == 0), "NUL-padded");
    }

    /// ServUO's `CloseSecureTrade` writes the serial and stops. Sphere pads the
    /// same packet to 17; see the module note for why this follows ServUO.
    #[test]
    fn the_close_packet_is_eight_bytes() {
        let bytes = encode_trade_close(Serial::new(0x4000_0002).unwrap());
        assert_eq!(bytes, vec![0x6F, 0x00, 0x08, 0x01, 0x40, 0x00, 0x00, 0x02]);
    }

    /// ServUO's `UpdateSecureTrade`: the two checkboxes as full `i32`s, not bytes.
    #[test]
    fn the_update_packet_carries_both_checkboxes_as_ints() {
        let bytes = encode_trade_update(Serial::new(0x4000_0002).unwrap(), false, true);
        assert_eq!(bytes.len(), 16);
        assert_eq!(bytes[3], 2); // Update
        assert_eq!(&bytes[4..8], &0x4000_0002u32.to_be_bytes());
        assert_eq!(&bytes[8..12], &0i32.to_be_bytes());
        assert_eq!(&bytes[12..16], &1i32.to_be_bytes());
    }

    #[test]
    fn a_cancel_carries_only_the_container() {
        let packet = [0x6F, 0x00, 0x08, 0x01, 0x40, 0x00, 0x00, 0x02];
        assert_eq!(
            SecureTradeAction::decode(&packet).unwrap(),
            Some(SecureTradeAction::Cancel {
                container: RawSerial(0x4000_0002),
            })
        );
    }

    /// The checkbox arrives as a four-byte int, and any non-zero is "ticked" —
    /// ServUO reads `ReadInt32() != 0` rather than trusting a 1.
    #[test]
    fn a_checkbox_is_an_int_and_any_non_zero_ticks_it() {
        let mut packet = vec![0x6F, 0x00, 0x0C, 0x02, 0x40, 0x00, 0x00, 0x02];
        packet.extend_from_slice(&0xFFu32.to_be_bytes());
        assert_eq!(
            SecureTradeAction::decode(&packet).unwrap(),
            Some(SecureTradeAction::Accept {
                container: RawSerial(0x4000_0002),
                accepted:  true,
            })
        );
    }

    /// Gold before platinum, ServUO's order — the one Sphere's own reader and
    /// writer disagree about.
    #[test]
    fn virtual_currency_reads_gold_before_platinum() {
        let mut packet = vec![0x6F, 0x00, 0x10, 0x03, 0x40, 0x00, 0x00, 0x02];
        packet.extend_from_slice(&500i32.to_be_bytes());
        packet.extend_from_slice(&7i32.to_be_bytes());
        assert_eq!(
            SecureTradeAction::decode(&packet).unwrap(),
            Some(SecureTradeAction::UpdateGold {
                container: RawSerial(0x4000_0002),
                gold:      500,
                platinum:  7,
            })
        );
    }

    /// An action this end does not know is ignored, not an error: ServUO's
    /// handler falls through the same way, and a client is free to grow one.
    #[test]
    fn an_unknown_action_decodes_to_nothing() {
        let packet = [0x6F, 0x00, 0x08, 0x09, 0x40, 0x00, 0x00, 0x02];
        assert_eq!(SecureTradeAction::decode(&packet).unwrap(), None);
    }

    #[test]
    fn a_truncated_packet_is_an_error_not_a_panic() {
        assert!(SecureTradeAction::decode(&[0x6F, 0x00, 0x05, 0x02]).is_err());
    }
}
