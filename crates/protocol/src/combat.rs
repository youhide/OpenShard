//! Combat packets: what the client is told about a mobile's health.

use crate::codec::PacketWriter;

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
}
