//! The targeting cursor: `0x6C`, sent to raise a crosshair and read back where it
//! was clicked.
//!
//! The server sends a `0x6C` to ask the client to target something; the client
//! answers with a `0x6C` of the same shape carrying what was clicked — a mobile,
//! an item, or a spot on the ground. It is one packet id in both directions,
//! nineteen bytes each way.

use crate::codec::PacketWriter;
use crate::login::{expect_id, LoginDecodeError};
use crate::world::Point;

/// The `type` byte: what kind of thing the cursor may pick.
const TARGET_LOCATION: u8 = 1;
/// The `cursorType` byte a cancelled (right-clicked) target comes back as.
const CURSOR_CANCEL: u8 = 3;

/// `0x6C` — raise a targeting cursor that picks a spot on the ground. 19 bytes.
///
/// `cursor_id` is echoed back in the response so the server can match a click to
/// the request that asked for it. This asks for a *location* (a ground target);
/// the object form is a later need.
pub fn encode_target_cursor(cursor_id: u32) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(19);
    writer.u8(0x6C);
    writer.u8(TARGET_LOCATION);
    writer.u32(cursor_id);
    writer.u8(0); // cursor type: neutral
                  // The rest the client fills in on the way back: object serial(4), x(2), y(2),
                  // a pad byte, z(1), tile graphic(2) — twelve bytes.
    writer.zeros(12);
    debug_assert_eq!(writer.len(), 19);
    writer.into_bytes()
}

/// `0x6C` — the client's answer: what the cursor picked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TargetResponse {
    /// The id the request carried, echoed back.
    pub cursor_id: u32,
    /// The object clicked, or `0` for a bare spot on the ground.
    pub serial: u32,
    /// Where — the clicked tile, meaningful for a ground target.
    pub location: Point,
    /// The tile graphic clicked, `0` for none.
    pub graphic: u16,
    /// Whether the target was cancelled — right-clicked away rather than picked.
    pub cancelled: bool,
}

impl TargetResponse {
    /// The packet id.
    pub const ID: u8 = 0x6C;

    /// Decode a whole `0x6C` response.
    ///
    /// Layout: type, cursor id, cursor type, clicked serial, x, y, a pad byte, z,
    /// tile graphic. A cursor type of 3 — or an `x` of `0xFFFF` — is a cancel.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _type = reader.u8()?;
        let cursor_id = reader.u32()?;
        let cursor_type = reader.u8()?;
        let serial = reader.u32()?;
        let x = reader.u16()?;
        let y = reader.u16()?;
        let _pad = reader.u8()?;
        let z = reader.u8()? as i8;
        let graphic = reader.u16()?;

        let cancelled = cursor_type == CURSOR_CANCEL || x == 0xFFFF;
        Ok(Self {
            cursor_id,
            serial,
            location: Point::new(x, y, z),
            graphic,
            cancelled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_request_is_nineteen_bytes() {
        let bytes = encode_target_cursor(0x0000_002A);
        assert_eq!(bytes.len(), 19);
        assert_eq!(bytes[0], 0x6C);
        assert_eq!(bytes[1], TARGET_LOCATION);
        assert_eq!(
            u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]),
            0x0000_002A
        );
    }

    #[test]
    fn a_ground_click_decodes_to_a_location() {
        let mut p = vec![0x6C, TARGET_LOCATION];
        p.extend_from_slice(&0x2Au32.to_be_bytes()); // cursor id
        p.push(0); // cursor type: neutral
        p.extend_from_slice(&0u32.to_be_bytes()); // serial: ground, none
        p.extend_from_slice(&1436u16.to_be_bytes()); // x
        p.extend_from_slice(&1559u16.to_be_bytes()); // y
        p.push(0); // pad
        p.push(30i8 as u8); // z
        p.extend_from_slice(&0x07C1u16.to_be_bytes()); // tile graphic
        assert_eq!(p.len(), 19);

        let got = TargetResponse::decode(&p).unwrap();
        assert_eq!(got.cursor_id, 0x2A);
        assert_eq!(got.serial, 0);
        assert_eq!(got.location, Point::new(1436, 1559, 30));
        assert_eq!(got.graphic, 0x07C1);
        assert!(!got.cancelled);
    }

    #[test]
    fn a_right_click_is_a_cancel() {
        let mut p = vec![0x6C, TARGET_LOCATION];
        p.extend_from_slice(&0x2Au32.to_be_bytes());
        p.push(CURSOR_CANCEL);
        p.extend_from_slice(&[0u8; 12]);
        assert_eq!(p.len(), 19);
        assert!(TargetResponse::decode(&p).unwrap().cancelled);
    }
}
