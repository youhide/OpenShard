//! Combat packets: war mode, attacking, and a mobile's health.

use crate::codec::{
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::mobile::Vitals;
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
};
use crate::serial::{
    Serial,
    raw_or_none,
};
use crate::version::ClientVersion;

/// `0x72` — enter or leave war mode. 5 bytes, the same shape both ways.
///
/// The client sends its desired stance and the server sends back the settled
/// one. The trailing `00 32 00` is fixed padding Sphere sends verbatim; only the
/// first byte, the war flag, means anything.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WarMode {
    /// True for war, false for peace.
    pub war: bool,
}

impl WarMode {
    /// The five bytes, whichever end is writing them. One body behind both
    /// doors below, because the packet is the same shape in both directions and
    /// two writers would be two chances to pad it differently.
    fn write(self, out: &mut PacketWriter) {
        out.bool(self.war);
        out.u8(0x00);
        out.u8(0x32);
        out.u8(0x00);
    }

    /// Encode a whole `0x72`. What `crates/client/net` sends when the
    /// paperdoll's peace/war toggle is pressed — the client's stance is a
    /// *request*, and what it settles to is the server's answer on the same id.
    ///
    /// No [`ClientVersion`] where [`EncodePacket`] takes one: the layout has
    /// never had a version in it, and the client half has no version to hand at
    /// the point a button is pressed.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        crate::packet::frame_body(
            <Self as EncodePacket>::ID,
            <Self as EncodePacket>::LENGTH,
            |out: &mut PacketWriter| self.write(out),
        )
    }
}

impl EncodePacket for WarMode {
    const ID: u8 = 0x72;
    const LENGTH: PacketLength = PacketLength::Fixed(5);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        self.write(out);
    }
}

impl DecodePacket for WarMode {
    const ID: u8 = 0x72;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self { war: reader.bool()? })
    }
}

/// `0x05` — the client asks to attack a mobile. 5 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AttackRequest {
    /// Whom to attack, or `None` when the serial names nothing that can be
    /// addressed.
    ///
    /// A client sends this with an out-of-range serial — a zero, a `0xFFFFFFFF` —
    /// to mean "stop attacking", and that is not a malformed packet: the answer
    /// is to clear the aim, not to drop the connection.
    pub target: Option<Serial>,
}

impl AttackRequest {
    /// Encode a whole `0x05`. What `crates/client/net` sends when a body is
    /// clicked in war mode — and, with `None`, when the aim is given up.
    ///
    /// The shape [`WarMode::encode`] has, for its reason: no [`ClientVersion`],
    /// because the layout has never had one in it and the client has no version
    /// in hand at the moment of a click.
    ///
    /// A `None` target goes out as `raw_or_none`'s zero — the sentinel every
    /// empty object field in this protocol shares — and the decoder above reads
    /// it straight back as `None`, because `Serial::new(0)` is `None`. That is
    /// the "stop attacking" the field's own docs describe, and the round trip
    /// is asserted rather than assumed.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        crate::packet::frame_body(
            <Self as EncodePacket>::ID,
            <Self as EncodePacket>::LENGTH,
            |out: &mut PacketWriter| out.u32(raw_or_none(self.target)),
        )
    }
}

impl EncodePacket for AttackRequest {
    const ID: u8 = 0x05;
    const LENGTH: PacketLength = PacketLength::Fixed(5);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(raw_or_none(self.target));
    }
}

impl DecodePacket for AttackRequest {
    const ID: u8 = 0x05;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            target: Serial::new(reader.u32()?),
        })
    }
}

/// `0xAA` — set the client's attack target, the mobile whose bar it highlights.
/// 5 bytes. `None` clears it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AttackTarget {
    /// Whose bar to highlight, or `None` to un-highlight.
    pub target: Option<Serial>,
}

impl EncodePacket for AttackTarget {
    const ID: u8 = 0xAA;
    const LENGTH: PacketLength = PacketLength::Fixed(5);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(raw_or_none(self.target));
    }
}

impl DecodePacket for AttackTarget {
    const ID: u8 = 0xAA;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            target: Serial::new(reader.u32()?),
        })
    }
}

/// `0xA1` — a mobile's health bar. 9 bytes.
///
/// # Two truths, by who is looking
///
/// You see your own hit points exactly; you see everyone else's only as a bar.
/// So the numbers on the wire depend on the recipient, and the two constructors
/// name that choice: [`HealthBar::exact`] for the packet that goes to the mobile
/// itself, [`HealthBar::scaled`] for the watchers, which sends `100` and a
/// percentage so a client can draw a stranger's health without ever being told
/// the numbers.
///
/// Ported from Sphere's `PacketHealthUpdate`, which despite its `STAT_STR` name
/// is the hit-points bar — UO maps the two.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HealthBar {
    /// Whose bar.
    pub serial: Serial,
    /// The current/max pair, drawn as a bar — `mobile::Hitpoints`'s own shape,
    /// scaled to 0–100 for anyone but the owner.
    pub vitals: Vitals,
}

impl HealthBar {
    /// The real numbers, for the mobile's own client.
    #[must_use]
    pub const fn exact(serial: Serial, max: u16, current: u16) -> Self {
        Self {
            serial,
            vitals: Vitals { current, max },
        }
    }

    /// A 0–100 bar, for everyone else.
    #[must_use]
    pub const fn scaled(serial: Serial, max: u16, current: u16) -> Self {
        // Clamped by a max of at least one so a zero-max mobile does not divide
        // by zero. Written out rather than `max.max(1)`: `Ord` is not const.
        let divisor = if max == 0 { 1 } else { max as u32 };
        let percent = (current as u32 * 100 / divisor) as u16;
        Self {
            serial,
            vitals: Vitals {
                current: percent,
                max:     100,
            },
        }
    }
}

impl EncodePacket for HealthBar {
    const ID: u8 = 0xA1;
    const LENGTH: PacketLength = PacketLength::Fixed(9);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.serial.raw());
        out.u16(self.vitals.max);
        out.u16(self.vitals.current);
    }
}

impl DecodePacket for HealthBar {
    const ID: u8 = 0xA1;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            serial: {
                let raw = reader.u32()?;
                Serial::new(raw).ok_or(DecodeError::UnknownValue {
                    field: "health bar serial",
                    value: raw,
                })?
            },
            vitals: Vitals {
                max:     reader.u16()?,
                current: reader.u16()?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{
        decode_packet,
        encode_packet,
    };

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    fn mobile(raw: u32) -> Serial {
        Serial::new(raw).unwrap()
    }

    #[test]
    fn the_owner_sees_real_numbers() {
        let packet = encode_packet(&HealthBar::exact(mobile(1), 120, 45), version());
        assert_eq!(packet[0], 0xA1);
        assert_eq!(&packet[1..5], &0x0000_0001u32.to_be_bytes());
        assert_eq!(u16::from_be_bytes([packet[5], packet[6]]), 120);
        assert_eq!(u16::from_be_bytes([packet[7], packet[8]]), 45);
        assert_eq!(packet.len(), 9);
    }

    #[test]
    fn everyone_else_sees_a_percentage() {
        // 45 of 120 is 37%. Max goes out as 100 so the bar is cur/100.
        let packet = encode_packet(&HealthBar::scaled(mobile(1), 120, 45), version());
        assert_eq!(u16::from_be_bytes([packet[5], packet[6]]), 100);
        assert_eq!(u16::from_be_bytes([packet[7], packet[8]]), 37);
    }

    #[test]
    fn a_full_bar_reads_full_either_way() {
        assert_eq!(HealthBar::scaled(mobile(1), 200, 200).vitals.current, 100);
    }

    #[test]
    fn a_zero_max_does_not_divide_by_zero() {
        assert_eq!(HealthBar::scaled(mobile(1), 0, 0).vitals.current, 0);
    }

    #[test]
    fn war_mode_round_trips() {
        assert!(
            decode_packet::<WarMode>(&[0x72, 0x01, 0, 0x32, 0], version())
                .unwrap()
                .war
        );
        assert!(
            !decode_packet::<WarMode>(&[0x72, 0x00, 0, 0x32, 0], version())
                .unwrap()
                .war
        );
        assert_eq!(
            encode_packet(&WarMode { war: true }, version()),
            vec![0x72, 0x01, 0x00, 0x32, 0x00]
        );
        assert_eq!(
            encode_packet(&WarMode { war: false }, version()),
            vec![0x72, 0x00, 0x00, 0x32, 0x00]
        );
    }

    /// The client's door and the server's write the same five bytes. Asserted
    /// rather than assumed: they are two public functions over one body, and the
    /// day one of them grows a version this is what says so.
    #[test]
    fn both_ends_write_the_same_war_mode_packet() {
        for war in [true, false] {
            assert_eq!(
                WarMode { war }.encode(),
                encode_packet(&WarMode { war }, version())
            );
        }
    }

    #[test]
    fn an_attack_request_is_a_serial() {
        let request: AttackRequest = decode_packet(&[0x05, 0x00, 0x00, 0x00, 0x2A], version()).unwrap();
        assert_eq!(request.target, Serial::new(0x2A));
    }

    #[test]
    fn an_attack_request_for_nothing_is_not_malformed() {
        // The client's way of saying "stop": a serial no object can have. It
        // clears the aim, and it must not cost the connection.
        let request: AttackRequest = decode_packet(&[0x05, 0x00, 0x00, 0x00, 0x00], version()).unwrap();
        assert_eq!(request.target, None);
    }

    /// The client's own door, and the round trip through the server's decoder.
    /// Both directions of one packet in one test, because the two halves are
    /// what a click on a body has to survive.
    #[test]
    fn an_attack_request_this_client_writes_is_one_this_server_reads() {
        let target = Serial::new(0x2A);
        let bytes = AttackRequest { target }.encode();
        assert_eq!(bytes, vec![0x05, 0x00, 0x00, 0x00, 0x2A]);
        assert_eq!(
            bytes,
            encode_packet(&AttackRequest { target }, version()),
            "the client's door and the trait write the same five bytes"
        );
        let back: AttackRequest = decode_packet(&bytes, version()).unwrap();
        assert_eq!(back.target, target);
    }

    /// "Stop attacking": no serial out, no serial back, and not an error at
    /// either end.
    #[test]
    fn giving_up_the_aim_round_trips_as_nothing() {
        let bytes = AttackRequest { target: None }.encode();
        assert_eq!(bytes, vec![0x05, 0x00, 0x00, 0x00, 0x00]);
        let back: AttackRequest = decode_packet(&bytes, version()).unwrap();
        assert_eq!(back.target, None);
    }

    #[test]
    fn setting_the_attack_target_is_five_bytes() {
        assert_eq!(
            encode_packet(
                &AttackTarget {
                    target: Serial::new(0x2A),
                },
                version()
            ),
            vec![0xAA, 0x00, 0x00, 0x00, 0x2A]
        );
    }

    #[test]
    fn an_attack_target_decodes_as_the_servers_aim() {
        let target: AttackTarget = decode_packet(&[0xAA, 0x00, 0x00, 0x00, 0x2A], version()).unwrap();
        assert_eq!(target.target, Serial::new(0x2A));

        let cleared: AttackTarget = decode_packet(&[0xAA, 0x00, 0x00, 0x00, 0x00], version()).unwrap();
        assert_eq!(cleared.target, None);
    }

    #[test]
    fn a_health_bar_decodes_the_pair_the_server_sent() {
        let bar: HealthBar = decode_packet(&[0xA1, 0, 0, 0, 0x2A, 0, 100, 0, 37], version()).unwrap();
        assert_eq!(bar.serial, mobile(0x2A));
        assert_eq!(bar.vitals.max, 100);
        assert_eq!(bar.vitals.current, 37);
    }

    #[test]
    fn clearing_the_attack_target_writes_a_zero() {
        assert_eq!(
            encode_packet(&AttackTarget { target: None }, version()),
            vec![0xAA, 0x00, 0x00, 0x00, 0x00]
        );
    }
}
