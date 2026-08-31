//! What this client does to an object: use it (`0x06`).
//!
//! Double-click is the whole of "use" in this protocol. There is no open-door
//! packet, no open-container packet and no eat packet: the client names an
//! object and the *server* decides what using it means — `crates/server/items`'
//! `use_object` is where that decision is made, and a door swinging open is one
//! of its arms.
//!
//! So there is nothing here to get wrong except which serial is named, and one
//! thing that is not obvious: the paperdoll request rides on the same packet, as
//! bit 31 of the serial ([`DoubleClick::interpret`]). Asking for a paperdoll is
//! therefore *not* this function with a flag — it is a different request that
//! happens to share an id, and it will arrive with the UI that can show one.

use openshard_protocol::containers::DoubleClick;
use openshard_protocol::serial::{
    RawSerial,
    Serial,
};

/// Use the object with this serial: the `0x06` to write to the socket.
///
/// A [`Serial`] and not a `RawSerial`, because this end is naming something it
/// has been shown — an item out of the [`WorldView`](crate::view::WorldView) —
/// rather than repeating a number back. The paperdoll bit is never set: see the
/// module docs.
#[must_use]
pub fn use_object(serial: Serial) -> Vec<u8> {
    DoubleClick {
        serial: RawSerial(serial.raw()),
    }
    .encode()
}

/// Ask the shard to open a mobile's paperdoll.
///
/// This is still packet `0x06`, but it is not a normal use: the protocol's
/// paperdoll bit routes it to `OnPaperdollRequest` and therefore cannot trigger
/// a self-use action such as dismounting.
#[must_use]
pub fn paperdoll(serial: Serial) -> Vec<u8> {
    DoubleClick::paperdoll(RawSerial(serial.raw())).encode()
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;
    use openshard_protocol::containers::UseRequest;
    use openshard_protocol::version::ClientVersion;

    use super::*;

    /// The test this module exists for: what this client writes is what a
    /// shard's own dispatch reads, and it reads it as a *use* and not as a
    /// paperdoll request — the one way five bytes can be misunderstood.
    #[test]
    fn a_double_clicked_door_reaches_the_server_as_a_use_of_it() {
        let door = Serial::new(0x4000_002A).unwrap();
        let bytes = use_object(door);
        let ClientPacket::DoubleClick(heard) =
            ClientPacket::decode(&bytes, ClientVersion::new(7, 0, 45, 65)).expect("0x06 decodes")
        else {
            panic!("0x06 decoded as some other packet");
        };
        assert_eq!(heard.interpret(), UseRequest::Use(RawSerial(door.raw())));
    }

    #[test]
    fn a_paperdoll_request_reaches_the_server_as_a_paperdoll() {
        let mobile = Serial::new(0x0000_002A).unwrap();
        let ClientPacket::DoubleClick(heard) =
            ClientPacket::decode(&paperdoll(mobile), ClientVersion::new(7, 0, 45, 65)).expect("0x06 decodes")
        else {
            panic!("0x06 decoded as some other packet");
        };
        assert_eq!(heard.interpret(), UseRequest::Paperdoll(RawSerial(mobile.raw())));
    }
}
