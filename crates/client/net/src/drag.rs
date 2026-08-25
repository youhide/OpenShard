//! Client-side encoders for lifting an item and putting it into a container.

use openshard_protocol::feature::Feature;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::items::{DROP_TO_GROUND, DropItem, EquipItemRequest, ItemAmount, PickUpItem};
use openshard_protocol::packet::{DecodePacket, PacketLength, frame_body};
use openshard_protocol::serial::Serial;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::RawLayer;
use openshard_protocol::world::Point;

/// Ask the shard to put `item` on the cursor.
#[must_use]
pub fn pick_up(item: Serial, amount: ItemAmount) -> Vec<u8> {
    frame_body(PickUpItem::ID, PacketLength::Fixed(7), |out| {
        out.u32(item.raw());
        out.u16(amount.0);
    })
}

/// Put the cursor item in a container at its gump-local location.
#[must_use]
pub fn drop_into(item: Serial, container: Serial, at: GumpPoint, version: ClientVersion) -> Vec<u8> {
    let grid = version.supports(Feature::ItemGrid);
    frame_body(
        DropItem::ID,
        PacketLength::Fixed(if grid { 15 } else { 14 }),
        |out| {
            out.u32(item.raw());
            out.u16(at.x.clamp(0, u16::MAX.into()) as u16);
            out.u16(at.y.clamp(0, u16::MAX.into()) as u16);
            out.u8(0);
            if grid {
                out.u8(0);
            }
            out.u32(container.raw());
        },
    )
}

/// Put the cursor item onto a world tile.
#[must_use]
pub fn drop_on_ground(item: Serial, at: Point, version: ClientVersion) -> Vec<u8> {
    let grid = version.supports(Feature::ItemGrid);
    frame_body(
        DropItem::ID,
        PacketLength::Fixed(if grid { 15 } else { 14 }),
        |out| {
            out.u32(item.raw());
            out.u16(at.x);
            out.u16(at.y);
            out.u8(at.z as u8);
            if grid {
                out.u8(0);
            }
            out.u32(DROP_TO_GROUND.0);
        },
    )
}

/// Put the cursor item onto a mobile's paperdoll slot.
#[must_use]
pub fn equip(item: Serial, layer: RawLayer, mobile: Serial) -> Vec<u8> {
    frame_body(EquipItemRequest::ID, PacketLength::Fixed(10), |out| {
        out.u32(item.raw());
        out.u8(layer.0);
        out.u32(mobile.raw());
    })
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;

    use super::*;

    #[test]
    fn container_drag_packets_round_trip_through_the_shards_decoder() {
        let version = ClientVersion::new(7, 0, 45, 65);
        let item = Serial::new(0x4000_002A).unwrap();
        let bag = Serial::new(0x4000_002B).unwrap();

        let ClientPacket::PickUpItem(pickup) =
            ClientPacket::decode(&pick_up(item, ItemAmount(7)), version).unwrap()
        else {
            panic!("lift was not a 0x07");
        };
        assert_eq!(pickup.serial.validate(), Some(item));
        assert_eq!(pickup.amount, ItemAmount(7));

        let ClientPacket::DropItem(drop) =
            ClientPacket::decode(&drop_into(item, bag, GumpPoint::new(42, 73), version), version).unwrap()
        else {
            panic!("drop was not a 0x08");
        };
        assert_eq!(drop.serial.validate(), Some(item));
        assert_eq!(
            drop.destination(),
            openshard_protocol::items::DropDestination::Item {
                item: bag,
                at: GumpPoint::new(42, 73),
            }
        );
        let ground = Point::new(100, 200, 7);
        let ClientPacket::DropItem(drop) =
            ClientPacket::decode(&drop_on_ground(item, ground, version), version).unwrap()
        else {
            panic!("ground drop was not a 0x08");
        };
        assert_eq!(
            drop.destination(),
            openshard_protocol::items::DropDestination::Ground(ground)
        );
    }

    #[test]
    fn paperdoll_drag_reaches_the_shard_as_an_equip_request() {
        let version = ClientVersion::new(7, 0, 45, 65);
        let item = Serial::new(0x4000_002A).unwrap();
        let wearer = Serial::new(0x0000_0007).unwrap();

        let layer = RawLayer(1);
        let ClientPacket::Equip(request) =
            ClientPacket::decode(&equip(item, layer, wearer), version).unwrap()
        else {
            panic!("paperdoll drop was not an equip request");
        };
        assert_eq!(request.item.validate(), Some(item));
        assert_eq!(request.layer, layer);
        assert_eq!(request.mobile.validate(), Some(wearer));
    }
}
