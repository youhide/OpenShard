//! Client-side answer to a server targeting cursor.

use openshard_protocol::packet::{
    DecodePacket,
    PacketLength,
    frame_body,
};
use openshard_protocol::target::TargetResponse;

/// Encode the `0x6C` response to an active target cursor.
#[must_use]
pub fn answer(response: TargetResponse) -> Vec<u8> {
    frame_body(TargetResponse::ID, PacketLength::Fixed(19), |out| {
        out.u8(1); // the server already knows which kind it raised
        out.u32(response.cursor_id.0);
        out.u8(if response.cancelled { 3 } else { 0 });
        out.u32(response.object.map_or(0, openshard_protocol::serial::Serial::raw));
        out.u16(if response.cancelled {
            u16::MAX
        } else {
            response.location.x
        });
        out.u16(response.location.y);
        out.u8(0);
        out.u8(response.location.z as u8);
        out.u16(response.graphic.map_or(0, |graphic| graphic.0));
    })
}

#[cfg(test)]
mod tests {
    use openshard_protocol::client_packet::ClientPacket;
    use openshard_protocol::serial::Serial;
    use openshard_protocol::version::ClientVersion;
    use openshard_protocol::wire::CursorId;
    use openshard_protocol::world::Point;

    use super::*;

    #[test]
    fn selected_object_reaches_the_shards_target_handler() {
        let object = Serial::new(0x4000_002A).unwrap();
        let response = TargetResponse {
            cursor_id: CursorId(42),
            object:    Some(object),
            location:  Point::new(123, 456, 7),
            graphic:   None,
            cancelled: false,
        };
        let ClientPacket::TargetResponse(decoded) =
            ClientPacket::decode(&answer(response), ClientVersion::new(7, 0, 45, 65)).unwrap()
        else {
            panic!("target answer had the wrong packet type");
        };
        assert_eq!(decoded, response);
    }

    #[test]
    fn cancellation_reaches_the_shards_target_handler() {
        let response = TargetResponse {
            cursor_id: CursorId(42),
            object:    None,
            location:  Point::new(0, 0, 0),
            graphic:   None,
            cancelled: true,
        };
        let ClientPacket::TargetResponse(decoded) =
            ClientPacket::decode(&answer(response), ClientVersion::new(7, 0, 45, 65)).unwrap()
        else {
            panic!("target answer had the wrong packet type");
        };
        assert_eq!(decoded.cursor_id, response.cursor_id);
        assert!(decoded.cancelled);
        assert_eq!(decoded.object, None);
        // A cancelled answer deliberately carries the protocol's `0xFFFF`
        // sentinel in `x`, not the placeholder point passed to `answer`.
        assert_eq!(decoded.location.x, u16::MAX);
    }
}
