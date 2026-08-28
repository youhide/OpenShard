//! The targeting cursor: `0x6C`, sent to raise a crosshair and read back where it
//! was clicked.
//!
//! The server sends a `0x6C` to ask the client to target something; the client
//! answers with a `0x6C` of the same shape carrying what was clicked — a mobile,
//! an item, or a spot on the ground. It is one packet id in both directions,
//! nineteen bytes each way.

use crate::codec::{PacketReader, PacketWriter};
use crate::error::DecodeError;
use crate::feature::Feature;
use crate::packet::{DecodePacket, EncodePacket, PacketLength};
use crate::serial::Serial;
use crate::version::ClientVersion;
use crate::wire::{CursorId, Graphic, MultiId};
use crate::world::Point;

/// The `cursorType` byte a cancelled (right-clicked) target comes back as.
const CURSOR_CANCEL: u8 = 3;

/// The `x` a cancelled target may come back as instead of, or as well as, a
/// cancelling cursor type.
const CANCEL_X: u16 = 0xFFFF;

/// What a raised cursor is allowed to pick.
///
/// The client enforces this itself, which is the point: "whom shall I examine?"
/// has no answer in a patch of grass, and refusing the click in the client saves
/// the player a wasted one. The server still checks what comes back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum TargetKind {
    /// An object — a mobile or an item. A click on bare ground is refused.
    Object = 0,
    /// Either an object or a spot on the ground; a tile is reported when that is
    /// what was clicked.
    Location = 1,
}

/// A multi's displacement from the tile a targeting cursor is over.
///
/// This is deliberately not [`Point`]: a point is an absolute, unsigned map
/// coordinate, while these three signed words shift the preview the client
/// draws from that coordinate.  Keeping the offset named prevents the three
/// same-shaped wire fields from being passed or written in a different order.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MultiOffset {
    /// East-west displacement, in tiles.
    pub x: i16,
    /// North-south displacement, in tiles.
    pub y: i16,
    /// Vertical displacement.
    pub z: i16,
}

impl MultiOffset {
    /// A displacement from a multi-target cursor.
    #[must_use]
    pub const fn new(x: i16, y: i16, z: i16) -> Self {
        Self { x, y, z }
    }
}

/// `0x6C` — raise a targeting cursor. 19 bytes.
///
/// `cursor_id` is echoed back in the response so the server can match a click to
/// the request that asked for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TargetCursor {
    /// Echoed back by the client, opaque to it.
    pub cursor_id: CursorId,
    /// What the cursor may pick.
    pub kind: TargetKind,
}

/// `0x99` — raise a targeting cursor with a **house drawn under it**.
///
/// ServUO's `MultiTargetReq`. It is a `0x6C` with four more fields on the end:
/// the multi to draw and an offset to draw it at, so the player sees the villa
/// following their pointer and picks a spot with the walls in front of them
/// rather than guessing. The answer comes back as an ordinary
/// [`TargetResponse`] — the client has nothing extra to say, because where the
/// house goes is where the cursor was.
///
/// # Two lengths, and the bytes are the same
///
/// 26 for a classic client and **30** from High Seas, and the difference is four
/// zero bytes on the end: every field is written at the same offset in both. The
/// reference has two whole packet classes for that (`MultiTargetReq` and
/// `MultiTargetReqHS`), which is one class per trailing pad; here it is one
/// encoder and a length that reads the version, the way `0x24` and `0x25`
/// already do.
///
/// # The gap in the middle is not padding to be tidied away
///
/// Bytes 7–17 are zero and mean nothing — the reference `Fill()`s to 18 and then
/// seeks there to write the multi. It is the shape of a packet that grew a tail
/// without moving its head, and shortening it would be a different packet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MultiTargetRequest {
    /// The cursor this is, echoed back in the response.
    pub cursor_id: CursorId,
    /// What may be picked. A house goes on the ground, so this is
    /// [`TargetKind::Location`] in every use there is — carried anyway, because
    /// the byte is on the wire and inventing its value here would be this engine
    /// deciding something the caller is entitled to.
    pub kind: TargetKind,
    /// Which multi the client draws under the pointer. The bare id, which is
    /// what ServUO writes and `0x4000` below the graphic a placed one carries —
    /// see [`MultiId`], whose whole reason to exist is that those two `u16`s
    /// mean different things.
    pub multi: MultiId,
    /// Where the drawing sits relative to the cursor, in tiles and z.
    ///
    /// Zero in every ordinary placement: a multi is drawn from its own origin and
    /// the origin is the tile clicked. The field exists because the reference's
    /// boats use it, and a value this engine never sets is still a value it must
    /// not corrupt.
    pub offset: MultiOffset,
}

/// How long a `0x99` is for a given client — see [`MultiTargetRequest`].
#[must_use]
pub const fn multi_target_length(high_seas: bool) -> PacketLength {
    PacketLength::Fixed(if high_seas { 30 } else { 26 })
}

impl MultiTargetRequest {
    /// Write the body, header excluded.
    ///
    /// Not an [`EncodePacket`] impl, for [`OpenContainer`](crate::containers::OpenContainer)'s
    /// reason: that trait's `LENGTH` is a `const` and this packet has two, so
    /// declaring either would be a lie the framer's own assertion catches. The
    /// length lives in [`multi_target_length`], which can see a version.
    pub fn write_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        out.u8(self.kind as u8);
        out.u32(self.cursor_id.0);
        out.u8(0); // cursor type: neutral, as `0x6C` writes it
        // To byte 18. Eleven bytes the client fills in on the way back, and the
        // reference writes as a `Fill()` before seeking past them.
        out.zeros(11);
        out.u16(self.multi.0);
        out.u16(self.offset.x as u16);
        out.u16(self.offset.y as u16);
        out.u16(self.offset.z as u16);
        if version.supports(Feature::HsPackets) {
            out.zeros(4);
        }
    }
}

impl DecodePacket for MultiTargetRequest {
    const ID: u8 = 0x99;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw_kind = reader.u8()?;
        let kind = match raw_kind {
            0 => TargetKind::Object,
            1 => TargetKind::Location,
            // `0x6C`'s rule, and for its reason: what a cursor may pick decides
            // whether a click on grass is an answer or a misfire, and guessing
            // makes a whole placement silently wrong.
            _ => {
                return Err(DecodeError::UnknownValue {
                    field: "0x99 target kind",
                    value: u32::from(raw_kind),
                });
            }
        };
        let cursor_id = CursorId(reader.u32()?);
        let _cursor_type = reader.u8()?;
        reader.skip(11)?;
        let multi = MultiId(reader.u16()?);
        let offset = MultiOffset::new(reader.u16()? as i16, reader.u16()? as i16, reader.u16()? as i16);
        Ok(Self {
            cursor_id,
            kind,
            multi,
            offset,
        })
    }
}

impl EncodePacket for TargetCursor {
    const ID: u8 = 0x6C;
    const LENGTH: PacketLength = PacketLength::Fixed(19);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u8(self.kind as u8);
        out.u32(self.cursor_id.0);
        out.u8(0); // cursor type: neutral
        // The rest the client fills in on the way back: object serial(4),
        // x(2), y(2), a pad byte, z(1), tile graphic(2) — twelve bytes.
        out.zeros(12);
    }
}

impl DecodePacket for TargetCursor {
    const ID: u8 = 0x6C;

    /// The *request* half of the id, read by the client. Nineteen bytes in both
    /// directions and the same first six mean different things in each — see
    /// [`TargetResponse`], which decodes the other twelve.
    ///
    /// A `kind` byte that is neither 0 nor 1 is refused rather than guessed at:
    /// what a raised cursor is allowed to pick decides whether a click on grass
    /// is answered at all, and defaulting it would put a cursor on the screen
    /// that refuses everything or accepts what the shard will not take.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let raw_kind = reader.u8()?;
        let kind = match raw_kind {
            0 => TargetKind::Object,
            1 => TargetKind::Location,
            _ => {
                return Err(DecodeError::UnknownValue {
                    field: "0x6C target kind",
                    value: u32::from(raw_kind),
                });
            }
        };
        Ok(Self {
            cursor_id: CursorId(reader.u32()?),
            kind,
        })
    }
}

/// `0x6C` — the client's answer: what the cursor picked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TargetResponse {
    /// The id the request carried, echoed back.
    pub cursor_id: CursorId,
    /// The object clicked, or `None` for a bare spot on the ground.
    ///
    /// Absence is the real thing here, not a zero to be checked for later: a
    /// ground target *has* no object, and a location spell wants exactly that.
    pub object: Option<Serial>,
    /// Where — the clicked tile, meaningful for a ground target.
    pub location: Point,
    /// The tile graphic clicked, `None` for none.
    pub graphic: Option<Graphic>,
    /// Whether the target was cancelled — right-clicked away rather than picked.
    pub cancelled: bool,
}

impl DecodePacket for TargetResponse {
    const ID: u8 = 0x6C;

    /// Layout: type, cursor id, cursor type, clicked serial, x, y, a pad byte, z,
    /// tile graphic. A cursor type of 3 — or an `x` of `0xFFFF` — is a cancel.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let _kind = reader.u8()?;
        let cursor_id = CursorId(reader.u32()?);
        let cursor_type = reader.u8()?;
        let object = Serial::new(reader.u32()?);
        let x = reader.u16()?;
        let y = reader.u16()?;
        let _pad = reader.u8()?;
        let z = reader.u8()? as i8;
        let graphic = match reader.u16()? {
            0 => None,
            id => Some(Graphic(id)),
        };

        let cancelled = cursor_type == CURSOR_CANCEL || x == CANCEL_X;
        Ok(Self {
            cursor_id,
            object,
            location: Point::new(x, y, z),
            graphic,
            cancelled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::{decode_packet, encode_packet};
    use crate::server_packet::ServerPacket;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    #[test]
    fn a_cursor_request_is_nineteen_bytes() {
        let bytes = encode_packet(
            &TargetCursor {
                cursor_id: CursorId(0x0000_002A),
                kind: TargetKind::Location,
            },
            version(),
        );
        assert_eq!(bytes.len(), 19);
        assert_eq!(bytes[0], 0x6C);
        assert_eq!(bytes[1], TargetKind::Location as u8);
        assert_eq!(
            u32::from_be_bytes([bytes[2], bytes[3], bytes[4], bytes[5]]),
            0x0000_002A
        );
    }

    #[test]
    fn an_object_cursor_differs_by_one_byte() {
        let object = encode_packet(
            &TargetCursor {
                cursor_id: CursorId(1),
                kind: TargetKind::Object,
            },
            version(),
        );
        let location = encode_packet(
            &TargetCursor {
                cursor_id: CursorId(1),
                kind: TargetKind::Location,
            },
            version(),
        );
        assert_eq!(object[1], 0x00);
        assert_eq!(location[1], 0x01);
        assert_eq!(object[2..], location[2..], "nothing else differs");
    }

    #[test]
    fn a_ground_click_decodes_to_a_location() {
        let mut p = vec![0x6C, TargetKind::Location as u8];
        p.extend_from_slice(&0x2Au32.to_be_bytes()); // cursor id
        p.push(0); // cursor type: neutral
        p.extend_from_slice(&0u32.to_be_bytes()); // serial: ground, none
        p.extend_from_slice(&1436u16.to_be_bytes()); // x
        p.extend_from_slice(&1559u16.to_be_bytes()); // y
        p.push(0); // pad
        p.push(30i8 as u8); // z
        p.extend_from_slice(&0x07C1u16.to_be_bytes()); // tile graphic
        assert_eq!(p.len(), 19);

        let got: TargetResponse = decode_packet(&p, version()).unwrap();
        assert_eq!(got.cursor_id, CursorId(0x2A));
        assert_eq!(got.object, None, "a ground click hit no object");
        assert_eq!(got.location, Point::new(1436, 1559, 30));
        assert_eq!(got.graphic, Some(Graphic(0x07C1)));
        assert!(!got.cancelled);
    }

    #[test]
    fn an_object_click_carries_the_serial() {
        let mut p = vec![0x6C, TargetKind::Object as u8];
        p.extend_from_slice(&0x2Au32.to_be_bytes());
        p.push(0);
        p.extend_from_slice(&0x0000_1234u32.to_be_bytes());
        p.extend_from_slice(&[0u8; 8]);
        assert_eq!(p.len(), 19);

        let got: TargetResponse = decode_packet(&p, version()).unwrap();
        assert_eq!(got.object, Serial::new(0x1234));
        assert_eq!(got.graphic, None, "no tile graphic on an object click");
    }

    #[test]
    fn a_right_click_is_a_cancel() {
        let mut p = vec![0x6C, TargetKind::Location as u8];
        p.extend_from_slice(&0x2Au32.to_be_bytes());
        p.push(CURSOR_CANCEL);
        p.extend_from_slice(&[0u8; 12]);
        assert_eq!(p.len(), 19);
        let got: TargetResponse = decode_packet(&p, version()).unwrap();
        assert!(got.cancelled);
    }

    fn a_request() -> MultiTargetRequest {
        MultiTargetRequest {
            cursor_id: CursorId(0x0000_002A),
            kind: TargetKind::Location,
            multi: MultiId(0x0064),
            offset: MultiOffset::default(),
        }
    }

    /// The two lengths, and the fact that they are the *same bytes*: High Seas
    /// added four zeroes on the end and moved nothing. The reference keeps two
    /// whole packet classes for that difference.
    #[test]
    fn a_multi_target_is_twenty_six_bytes_and_thirty_after_high_seas() {
        let classic = ServerPacket::MultiTarget(a_request()).encode(ClientVersion::new(6, 0, 0, 0));
        let modern = ServerPacket::MultiTarget(a_request()).encode(ClientVersion::HS);
        assert_eq!(classic.len(), 26);
        assert_eq!(modern.len(), 30);
        assert_eq!(
            classic[..26],
            modern[..26],
            "the extra four bytes moved a field instead of padding the end"
        );
        assert_eq!(&modern[26..], &[0, 0, 0, 0]);
    }

    /// The multi id lands at byte 18, which is where the reference seeks to after
    /// filling the eleven bytes it never writes. An encoder that packed them out
    /// would produce a shorter, tidier packet no client can read.
    #[test]
    fn the_multi_sits_at_byte_eighteen_after_a_gap_of_nothing() {
        let bytes = ServerPacket::MultiTarget(a_request()).encode(version());
        assert_eq!(&bytes[7..18], &[0; 11], "the gap is not padding to be tidied");
        assert_eq!(u16::from_be_bytes([bytes[18], bytes[19]]), 0x0064);
    }

    #[test]
    fn a_multi_offset_keeps_its_axes_in_wire_order() {
        let bytes = ServerPacket::MultiTarget(MultiTargetRequest {
            offset: MultiOffset::new(-3, 4, -128),
            ..a_request()
        })
        .encode(version());

        assert_eq!(i16::from_be_bytes([bytes[20], bytes[21]]), -3, "x");
        assert_eq!(i16::from_be_bytes([bytes[22], bytes[23]]), 4, "y");
        assert_eq!(i16::from_be_bytes([bytes[24], bytes[25]]), -128, "z");
    }

    #[test]
    fn a_multi_target_round_trips() {
        for version in [ClientVersion::new(6, 0, 0, 0), ClientVersion::HS] {
            // A non-zero offset, because zero is what every placement sends and a
            // field only ever written as zero is a field whose bytes nobody has
            // checked.
            let sent = MultiTargetRequest {
                offset: MultiOffset::new(-3, 4, -128),
                ..a_request()
            };
            let bytes = ServerPacket::MultiTarget(sent).encode(version);
            let got: MultiTargetRequest = decode_packet(&bytes, version).unwrap();
            assert_eq!(got, sent, "at {version:?}");
        }
    }

    /// A multi id and the graphic a placed one draws as are two `u16`s that mean
    /// different things, which is what [`MultiId`] exists to keep apart.
    #[test]
    fn a_multi_id_is_not_the_graphic_it_draws_as() {
        let cottage = MultiId(0x0064);
        assert_eq!(cottage.graphic(), Graphic(0x4064));
        assert_eq!(MultiId::from_graphic(Graphic(0x4064)), cottage);
        assert_eq!(
            MultiId::from_graphic(Graphic(0x0064)),
            cottage,
            "a caller holding the bare id reaches the same multi"
        );
    }
}
