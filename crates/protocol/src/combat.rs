//! Combat packets: war mode, attacking, and a mobile's health.

use crate::codec::PacketWriter;
use crate::login::{expect_id, LoginDecodeError};

/// `0x72` — enter or leave war mode. 5 bytes, the same shape both ways.
///
/// The client sends its desired stance and the server sends back the settled
/// one. The trailing `00 32 00` is fixed padding Sphere sends verbatim; only the
/// first byte, the war flag, means anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WarModeRequest {
    /// True for war, false for peace.
    pub war: bool,
}

impl WarModeRequest {
    /// The packet id.
    pub const ID: u8 = 0x72;

    /// Decode a whole `0x72` packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        Ok(Self {
            war: reader.u8()? != 0,
        })
    }
}

/// `0x72` — tell the client the settled war stance.
pub fn encode_war_mode(war: bool) -> Vec<u8> {
    vec![WarModeRequest::ID, u8::from(war), 0x00, 0x32, 0x00]
}

/// `0x05` — the client asks to attack a mobile. 5 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AttackRequest {
    /// Whom to attack, by serial.
    pub target: u32,
}

impl AttackRequest {
    /// The packet id.
    pub const ID: u8 = 0x05;

    /// Decode a whole `0x05` packet.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        Ok(Self {
            target: reader.u32()?,
        })
    }
}

/// `0xAA` — set the client's attack target, the mobile whose bar it highlights.
/// A serial of zero clears it.
pub fn encode_attack(serial: u32) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(5);
    writer.u8(0xAA);
    writer.u32(serial);
    writer.into_bytes()
}

/// `0xA1` — update a mobile's health bar. 9 bytes.
///
/// # Two truths, by who is looking
///
/// You see your own hit points exactly; you see everyone else's only as a bar.
/// So `full` — true when the packet goes to the mobile itself — sends the real
/// `max` and `current`, and false sends `100` and a percentage, which is how the
/// client can draw a stranger's health without ever being told the numbers.
///
/// Ported from Sphere's `PacketHealthUpdate`, which despite its `STAT_STR` name
/// is the hit-points bar — UO maps the two.
pub fn encode_health(serial: u32, max: u16, current: u16, full: bool) -> Vec<u8> {
    let (max, current) = if full {
        (max, current)
    } else {
        // Percentage, clamped by a max of at least one so a zero-max mobile does
        // not divide by zero.
        (
            100,
            (u32::from(current) * 100 / u32::from(max.max(1))) as u16,
        )
    };
    let mut writer = PacketWriter::with_capacity(9);
    writer.u8(0xA1);
    writer.u32(serial);
    writer.u16(max);
    writer.u16(current);
    writer.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_sees_real_numbers() {
        let packet = encode_health(0x0000_0001, 120, 45, true);
        assert_eq!(packet[0], 0xA1);
        assert_eq!(&packet[1..5], &0x0000_0001u32.to_be_bytes());
        assert_eq!(u16::from_be_bytes([packet[5], packet[6]]), 120);
        assert_eq!(u16::from_be_bytes([packet[7], packet[8]]), 45);
        assert_eq!(packet.len(), 9);
    }

    #[test]
    fn everyone_else_sees_a_percentage() {
        // 45 of 120 is 37%. Max goes out as 100 so the bar is cur/100.
        let packet = encode_health(0x0000_0001, 120, 45, false);
        assert_eq!(u16::from_be_bytes([packet[5], packet[6]]), 100);
        assert_eq!(u16::from_be_bytes([packet[7], packet[8]]), 37);
    }

    #[test]
    fn a_full_bar_reads_full_either_way() {
        assert_eq!(
            u16::from_be_bytes({
                let p = encode_health(1, 200, 200, false);
                [p[7], p[8]]
            }),
            100
        );
    }

    #[test]
    fn a_zero_max_does_not_divide_by_zero() {
        let packet = encode_health(1, 0, 0, false);
        assert_eq!(u16::from_be_bytes([packet[7], packet[8]]), 0);
    }

    #[test]
    fn war_mode_round_trips() {
        assert!(
            WarModeRequest::decode(&[0x72, 0x01, 0, 0x32, 0])
                .unwrap()
                .war
        );
        assert!(
            !WarModeRequest::decode(&[0x72, 0x00, 0, 0x32, 0])
                .unwrap()
                .war
        );
        assert_eq!(encode_war_mode(true), vec![0x72, 0x01, 0x00, 0x32, 0x00]);
        assert_eq!(encode_war_mode(false), vec![0x72, 0x00, 0x00, 0x32, 0x00]);
    }

    #[test]
    fn an_attack_request_is_a_serial() {
        let bytes = [0x05, 0x00, 0x00, 0x00, 0x2A];
        assert_eq!(AttackRequest::decode(&bytes).unwrap().target, 0x2A);
    }

    #[test]
    fn setting_the_attack_target_is_five_bytes() {
        let packet = encode_attack(0x0000_002A);
        assert_eq!(packet, vec![0xAA, 0x00, 0x00, 0x00, 0x2A]);
    }
}
