//! Client-side answers to NPC vendor lists.

use openshard_protocol::packet::{
    PacketLength,
    frame_body,
};
use openshard_protocol::serial::Serial;

/// Buy the selected quantities from `vendor` (`0x3B`).
#[must_use]
pub fn buy(vendor: Serial, purchases: &[(Serial, u16)]) -> Vec<u8> {
    frame_body(0x3B, PacketLength::Variable, |out| {
        out.u32(vendor.raw());
        if purchases.is_empty() {
            out.u8(0);
            return;
        }
        out.u8(0x02);
        for (index, (item, amount)) in purchases.iter().enumerate() {
            // The layer byte is only the row number in the classic buy reply.
            out.u8(index.min(u8::MAX as usize) as u8);
            out.u32(item.raw());
            out.u16(*amount);
        }
    })
}

/// Sell the selected quantities to `vendor` (`0x9F`).
#[must_use]
pub fn sell(vendor: Serial, sales: &[(Serial, u16)]) -> Vec<u8> {
    frame_body(0x9F, PacketLength::Variable, |out| {
        out.u32(vendor.raw());
        out.u16(sales.len().min(u16::MAX as usize) as u16);
        for (item, amount) in sales.iter().take(u16::MAX as usize) {
            out.u32(item.raw());
            out.u16(*amount);
        }
    })
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;
    use openshard_protocol::version::ClientVersion;

    use super::*;

    fn serial(raw: u32) -> Serial {
        Serial::new(raw).unwrap()
    }

    #[test]
    fn vendor_replies_round_trip_through_the_server_decoder() {
        let version = ClientVersion::new(7, 0, 45, 65);
        let vendor = serial(0x0000_002A);
        let item = serial(0x4000_0042);
        let ClientPacket::Buy(reply) = ClientPacket::decode(&buy(vendor, &[(item, 3)]), version).unwrap()
        else {
            panic!("not a buy reply");
        };
        assert_eq!(reply.vendor.validate(), Some(vendor));
        assert_eq!(reply.purchases.len(), 1);
        assert_eq!(reply.purchases[0].serial.validate(), Some(item));
        assert_eq!(
            reply.purchases[0].amount,
            openshard_protocol::items::ItemAmount(3)
        );

        let ClientPacket::Sell(reply) = ClientPacket::decode(&sell(vendor, &[(item, 2)]), version).unwrap()
        else {
            panic!("not a sell reply");
        };
        assert_eq!(reply.vendor.validate(), Some(vendor));
        assert_eq!(reply.sales.len(), 1);
        assert_eq!(reply.sales[0].serial.validate(), Some(item));
        assert_eq!(reply.sales[0].amount, openshard_protocol::items::ItemAmount(2));
    }
}
