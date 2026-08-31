//! Asking the shard what an object's tooltip says (`0xD6`).
//!
//! The client's half of the AoS property protocol, and the half that had never
//! been written: the shard has been building lists and sending revisions for a
//! long time, and nothing on this end ever asked for one.
//!
//! # Why this is not sent for everything on screen
//!
//! Because the revision packet exists. The shard's `version` tooltip mode sends
//! a `0xDC` — nine bytes — with each object it draws, and the list itself only
//! when asked; a client that turned every revision into a request would put the
//! full list of every object in view back on the wire and leave the `0xDC` doing
//! nothing but announcing it. So the request is driven by the hover, which is
//! the moment somebody actually wants to read one, and
//! [`Tooltip::stale`](crate::view::Tooltip::stale) is what decides whether it is
//! needed at all.
//!
//! # One packet, many serials
//!
//! `0xD6` is a *batch* query — ServUO's `BatchQueryProperties` reads serials
//! until the body runs out. The caller decides how many go in one; nothing here
//! caps it, because the only thing a cap could protect is the `u16` length, and
//! four thousand serials is far past any hover.

use openshard_protocol::packet::encode_packet;
use openshard_protocol::properties::PropertyQueryRequest;
use openshard_protocol::serial::{
    RawSerial,
    Serial,
};
use openshard_protocol::version::ClientVersion;

/// Ask for the property lists of `serials`: the `0xD6` to write to the socket.
///
/// [`Serial`]s and not `RawSerial`s, for [`use_object`](crate::interact::use_object)'s
/// reason — this end is naming things it has been shown, out of the
/// [`WorldView`](crate::view::WorldView), rather than repeating numbers back.
#[must_use]
pub fn query(serials: &[Serial], version: ClientVersion) -> Vec<u8> {
    encode_packet(
        &PropertyQueryRequest {
            serials: serials.iter().map(|serial| RawSerial(serial.raw())).collect(),
        },
        version,
    )
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;

    use super::*;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    /// The test this module exists for: what this client writes is what the
    /// shard's own dispatch reads, serial for serial and in order.
    #[test]
    fn a_batch_reaches_the_server_as_the_serials_it_named() {
        let wanted = [
            Serial::new(0x0000_002A).unwrap(),
            Serial::new(0x4000_0001).unwrap(),
        ];
        let ClientPacket::PropertyQuery(heard) =
            ClientPacket::decode(&query(&wanted, version()), version()).expect("0xD6 decodes")
        else {
            panic!("0xD6 decoded as some other packet");
        };
        assert_eq!(
            heard.serials,
            vec![RawSerial(0x0000_002A), RawSerial(0x4000_0001)]
        );
    }

    /// A hover over nothing must not put an empty query on the wire, but if one
    /// is built it has to be a well-formed packet rather than a truncated body.
    #[test]
    fn an_empty_batch_is_still_a_whole_packet() {
        let bytes = query(&[], version());
        assert_eq!(bytes[0], 0xD6);
        let ClientPacket::PropertyQuery(heard) =
            ClientPacket::decode(&bytes, version()).expect("0xD6 decodes")
        else {
            panic!("0xD6 decoded as some other packet");
        };
        assert!(heard.serials.is_empty());
    }
}
