//! Other mobiles: drawing them, moving them, taking them away.
//!
//! These are what make a shard a place rather than a single-player map viewer.
//! The client draws its own character from `0x1B`/`0x20`; everyone *else* comes
//! from here.

use crate::codec::PacketWriter;
use crate::direction::Facing;
use crate::feature::Feature;
use crate::version::ClientVersion;
use crate::world::Point;

/// Status flags on a mobile: poisoned, invisible, in war mode.
///
/// Not modelled further yet — nothing sets them. A `u8` that says so is more
/// honest than an enum with one variant.
pub type StatusFlags = u8;

/// How the client colours a mobile's health bar.
///
/// The client renders these; the meanings are its, not ours.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Notoriety {
    /// Blue.
    Innocent,
    /// Green.
    Friend,
    /// Grey: an animal, or a non-player that has not been provoked.
    Neutral,
    /// Grey: attackable.
    Criminal,
    /// Orange.
    Enemy,
    /// Red.
    Murderer,
    /// Yellow. Only rendered since 4.0.0 — see [`Feature::NotorietyInvulnerable`].
    Invulnerable,
}

impl Notoriety {
    /// The wire byte.
    pub const fn to_bits(self) -> u8 {
        match self {
            Self::Innocent => 0x01,
            Self::Friend => 0x02,
            Self::Neutral => 0x03,
            Self::Criminal => 0x04,
            Self::Enemy => 0x05,
            Self::Murderer => 0x06,
            Self::Invulnerable => 0x07,
        }
    }

    /// What to actually send to `version`.
    ///
    /// A client older than 4.0.0 has no yellow bar and renders `0x07` as
    /// nothing at all — the mobile gets no health bar and looks like a bug.
    /// Downgrading to blue is a small lie that draws.
    pub fn for_client(self, version: ClientVersion) -> u8 {
        if self == Self::Invulnerable && !version.supports(Feature::NotorietyInvulnerable) {
            return Self::Innocent.to_bits();
        }
        self.to_bits()
    }
}

/// One piece of equipment on a mobile, as the client draws it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Equipment {
    /// The item's serial.
    pub serial: u32,
    /// Its graphic.
    pub graphic: u16,
    /// Which layer: hair, weapon, robe.
    pub layer: u8,
    /// Its colour. Zero means the graphic's own.
    pub hue: u16,
}

/// `0x1D` — take an object off the client's screen. 5 bytes.
///
/// Used for mobiles walking out of range and for items being picked up. The
/// client does not distinguish; it just forgets the serial.
pub fn encode_remove(serial: u32) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(5);
    writer.u8(0x1D);
    writer.u32(serial);
    writer.into_bytes()
}

/// `0x77` — move a mobile the client already knows about. 17 bytes.
///
/// Sphere's comment is worth keeping: this cannot move the client's *own*
/// character. Sending it for the receiving player does nothing visible and the
/// two ends drift apart. Use `0x20` for that.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MobileMove {
    /// The mobile's serial.
    pub serial: u32,
    /// Its body graphic.
    pub body: u16,
    /// Where.
    pub position: Point,
    /// Which way, and whether running.
    pub facing: Facing,
    /// Its hue.
    pub hue: u16,
    /// Poisoned, invisible, war mode.
    pub flags: StatusFlags,
    /// How to colour its health bar.
    pub notoriety: Notoriety,
}

impl MobileMove {
    /// The packet id.
    pub const ID: u8 = 0x77;

    /// Encode a whole 0x77 packet.
    pub fn encode(&self, version: ClientVersion) -> Vec<u8> {
        let mut writer = PacketWriter::with_capacity(17);
        writer.u8(Self::ID);
        writer.u32(self.serial);
        writer.u16(self.body);
        writer.u16(self.position.x);
        writer.u16(self.position.y);
        writer.u8(self.position.z as u8);
        writer.u8(self.facing.to_bits());
        writer.u16(self.hue);
        writer.u8(self.flags);
        writer.u8(self.notoriety.for_client(version));
        writer.into_bytes()
    }
}

/// `0x78` — draw a mobile the client has not seen. Variable length.
///
/// # Two layouts, and the old one is the odd shape
///
/// The equipment list is where this gets interesting. Since 7.0.33.1
/// ([`Feature::NewMobileIncoming`]) every item is a fixed nine bytes with a hue
/// whether it needs one or not.
///
/// Before that, the record was *variable*, and what said which shape it was is a
/// bit inside the graphic id: `graphic | 0x8000` means "a hue follows", and
/// without it the record is seven bytes and stops at the layer. So an old client
/// parses the item list by reading a graphic, checking its top bit, and deciding
/// how much more to read. Send the new shape to an old client and it reads the
/// hue as the next item's serial and everything after is noise.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MobileIncoming {
    /// The mobile's serial.
    pub serial: u32,
    /// Its body graphic.
    pub body: u16,
    /// Where.
    pub position: Point,
    /// Which way, and whether running.
    pub facing: Facing,
    /// Its hue.
    pub hue: u16,
    /// Poisoned, invisible, war mode.
    pub flags: StatusFlags,
    /// How to colour its health bar.
    pub notoriety: Notoriety,
    /// What it is wearing.
    pub equipment: Vec<Equipment>,
}

impl MobileIncoming {
    /// The packet id.
    pub const ID: u8 = 0x78;

    /// Encode a whole 0x78 packet for `version`.
    pub fn encode(&self, version: ClientVersion) -> Vec<u8> {
        let new_layout = version.supports(Feature::NewMobileIncoming);

        let mut writer = PacketWriter::with_capacity(23 + self.equipment.len() * 9);
        writer.u8(Self::ID);
        writer.u16(0); // length, patched below
        writer.u32(self.serial);
        writer.u16(self.body);
        writer.u16(self.position.x);
        writer.u16(self.position.y);
        writer.u8(self.position.z as u8);
        writer.u8(self.facing.to_bits());
        writer.u16(self.hue);
        writer.u8(self.flags);
        writer.u8(self.notoriety.for_client(version));

        for item in &self.equipment {
            writer.u32(item.serial);
            if new_layout {
                writer.u16(item.graphic);
                writer.u8(item.layer);
                writer.u16(item.hue);
            } else if item.hue != 0 {
                // The top bit is not part of the graphic. It is the flag that
                // says the next two bytes are a hue.
                writer.u16(item.graphic | 0x8000);
                writer.u8(item.layer);
                writer.u16(item.hue);
            } else {
                writer.u16(item.graphic);
                writer.u8(item.layer);
            }
        }

        // A zero serial ends the list. Not a length — the client reads items
        // until it sees this.
        writer.u32(0);

        let mut bytes = writer.into_bytes();
        let length = u16::try_from(bytes.len()).expect("a mobile outgrew its u16 length field");
        bytes[1..3].copy_from_slice(&length.to_be_bytes());
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direction::Direction;

    fn facing() -> Facing {
        Facing::running(Direction::SouthEast)
    }

    fn mobile() -> MobileIncoming {
        MobileIncoming {
            serial: 0x0000_0002,
            body: 0x0190,
            position: Point::new(1475, 1774, -5),
            facing: facing(),
            hue: 0x83EA,
            flags: 0,
            notoriety: Notoriety::Innocent,
            equipment: Vec::new(),
        }
    }

    fn shirt() -> Equipment {
        Equipment {
            serial: 0x4000_0001,
            graphic: 0x1517,
            layer: 0x05,
            hue: 0x0021,
        }
    }

    #[test]
    fn remove_is_five_bytes() {
        assert_eq!(
            encode_remove(0xDEAD_BEEF),
            vec![0x1D, 0xDE, 0xAD, 0xBE, 0xEF]
        );
    }

    #[test]
    fn a_move_matches_its_declared_length() {
        let bytes = MobileMove {
            serial: 2,
            body: 0x0190,
            position: Point::new(1475, 1774, -5),
            facing: facing(),
            hue: 0x83EA,
            flags: 0,
            notoriety: Notoriety::Innocent,
        }
        .encode(ClientVersion::TOL);

        assert_eq!(bytes.len(), 17, "Sphere's PacketCharacterMove length");
        assert_eq!(bytes[0], 0x77);
        assert_eq!(&bytes[1..5], &2u32.to_be_bytes());
        assert_eq!(&bytes[5..7], &0x0190u16.to_be_bytes());
        assert_eq!(bytes[11] as i8, -5, "z is one signed byte");
        assert_eq!(bytes[12], facing().to_bits());
        assert_eq!(bytes[16], 0x01, "innocent");
    }

    #[test]
    fn a_naked_mobile_is_the_base_length() {
        let bytes = mobile().encode(ClientVersion::TOL);
        assert_eq!(bytes.len(), 23, "Sphere's PacketCharacter base length");
        assert_eq!(bytes[0], 0x78);
        assert_eq!(
            u16::from_be_bytes([bytes[1], bytes[2]]) as usize,
            bytes.len(),
            "the declared length must match reality"
        );
        assert_eq!(
            &bytes[19..23],
            &[0, 0, 0, 0],
            "the zero serial ends the list"
        );
    }

    #[test]
    fn a_modern_client_gets_nine_fixed_bytes_per_item() {
        // Since 7.0.33.1 the hue is always there, needed or not.
        let mut incoming = mobile();
        incoming.equipment = vec![shirt()];
        let bytes = incoming.encode(ClientVersion::new(7, 0, 33, 1));

        assert_eq!(bytes.len(), 23 + 9);
        assert_eq!(&bytes[19..23], &0x4000_0001u32.to_be_bytes(), "item serial");
        assert_eq!(
            &bytes[23..25],
            &0x1517u16.to_be_bytes(),
            "the graphic carries no flag bit"
        );
        assert_eq!(bytes[25], 0x05, "layer");
        assert_eq!(&bytes[26..28], &0x0021u16.to_be_bytes(), "hue, always");
    }

    #[test]
    fn an_old_client_gets_the_hue_flagged_into_the_graphic() {
        // The bit that says "a hue follows". An old client reads the graphic,
        // checks its top bit, and decides how much more to read — so the record
        // is nine bytes here and seven below.
        let mut incoming = mobile();
        incoming.equipment = vec![shirt()];
        let bytes = incoming.encode(ClientVersion::new(7, 0, 33, 0));

        assert_eq!(bytes.len(), 23 + 9);
        assert_eq!(
            u16::from_be_bytes([bytes[23], bytes[24]]),
            0x1517 | 0x8000,
            "the top bit flags the hue"
        );
        assert_eq!(&bytes[26..28], &0x0021u16.to_be_bytes());
    }

    #[test]
    fn an_old_client_gets_seven_bytes_for_an_unhued_item() {
        // The variable-length case. Sending nine bytes here would have the
        // client read the hue as the next item's serial, and everything after it
        // is noise.
        let mut incoming = mobile();
        incoming.equipment = vec![Equipment { hue: 0, ..shirt() }];
        let bytes = incoming.encode(ClientVersion::new(7, 0, 33, 0));

        assert_eq!(bytes.len(), 23 + 7, "no hue, no two bytes for one");
        assert_eq!(
            u16::from_be_bytes([bytes[23], bytes[24]]),
            0x1517,
            "and no flag bit either"
        );
        assert_eq!(bytes[25], 0x05, "the record stops at the layer");
    }

    #[test]
    fn the_two_layouts_differ_for_the_same_mobile() {
        // The whole reason this is version-gated. If these ever agree, the gate
        // has stopped doing anything.
        let mut incoming = mobile();
        incoming.equipment = vec![Equipment { hue: 0, ..shirt() }];

        let modern = incoming.encode(ClientVersion::new(7, 0, 33, 1));
        let ancient = incoming.encode(ClientVersion::new(7, 0, 33, 0));
        assert_ne!(modern, ancient);
        assert_eq!(
            modern.len(),
            ancient.len() + 2,
            "the hue an old client skips"
        );
    }

    #[test]
    fn a_mobile_wearing_a_lot_still_declares_its_length() {
        let mut incoming = mobile();
        incoming.equipment = (0..25)
            .map(|index| Equipment {
                serial: 0x4000_0000 + index,
                layer: index as u8,
                ..shirt()
            })
            .collect();

        for version in [ClientVersion::new(7, 0, 33, 0), ClientVersion::TOL] {
            let bytes = incoming.encode(version);
            assert_eq!(
                u16::from_be_bytes([bytes[1], bytes[2]]) as usize,
                bytes.len(),
                "{version}"
            );
        }
    }

    #[test]
    fn notoriety_bytes_are_the_clients_own() {
        assert_eq!(Notoriety::Innocent.to_bits(), 0x01);
        assert_eq!(Notoriety::Friend.to_bits(), 0x02);
        assert_eq!(Notoriety::Neutral.to_bits(), 0x03);
        assert_eq!(Notoriety::Criminal.to_bits(), 0x04);
        assert_eq!(Notoriety::Enemy.to_bits(), 0x05);
        assert_eq!(Notoriety::Murderer.to_bits(), 0x06);
        assert_eq!(Notoriety::Invulnerable.to_bits(), 0x07);
    }

    #[test]
    fn an_old_client_gets_blue_rather_than_no_health_bar() {
        // A client before 4.0.0 renders 0x07 as nothing — the mobile gets no
        // health bar at all, which looks like a bug rather than a GM.
        let ancient = ClientVersion::new(3, 255, 255, 255);
        assert!(!ancient.supports(Feature::NotorietyInvulnerable));
        assert_eq!(
            Notoriety::Invulnerable.for_client(ancient),
            Notoriety::Innocent.to_bits()
        );

        let modern = ClientVersion::AOS;
        assert_eq!(Notoriety::Invulnerable.for_client(modern), 0x07);
    }

    #[test]
    fn every_other_notoriety_survives_an_old_client() {
        let ancient = ClientVersion::new(3, 0, 0, 0);
        for notoriety in [
            Notoriety::Innocent,
            Notoriety::Friend,
            Notoriety::Neutral,
            Notoriety::Criminal,
            Notoriety::Enemy,
            Notoriety::Murderer,
        ] {
            assert_eq!(
                notoriety.for_client(ancient),
                notoriety.to_bits(),
                "{notoriety:?} was downgraded and should not have been"
            );
        }
    }
}
