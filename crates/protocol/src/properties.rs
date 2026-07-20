//! Object property lists — the AoS "cliloc" tooltips a modern client shows on
//! hover.
//!
//! Three packets make one behaviour. When the server draws a thing it also sends
//! its tooltip *revision* (`0xDC`): the object's serial and a hash of its
//! properties. The client caches that hash, and when it wants the tooltip it asks
//! for the full list (`0xD6` in, a batch of serials). The server answers with the
//! list itself (`0xD6` out): the serial, the same hash, and a run of cliloc
//! entries — a localized-string number and its optional arguments. The client
//! looks the numbers up in its own `cliloc.enu`, so a name costs a number, not a
//! string: an item is cliloc `1020000 + graphic` (the client's tiledata-name
//! range), a mobile is cliloc `1050045` with its name as an argument.
//!
//! Ported from ServUO's `ObjectPropertyList`/`OPLInfo` (`Server/ObjectPropertyList.cs`)
//! and cross-checked against Sphere's `PacketPropertyList`/`PacketPropertyListVersion`
//! (`network/send.cpp`). Two wire details are worth stating: the argument text is
//! UTF-16 **little-endian** (ServUO's `Encoding.Unicode`, Sphere's `writeCharUTF16`
//! low-byte-first) — *not* the big-endian UTF-16 the `0xAE` speech packet uses —
//! and the revision hash in the `0xDC` is the **same** value the `0xD6` body
//! carries, per Sphere, so a client that requested a list can match it to the
//! revision it was told about.

use crate::codec::PacketWriter;
use crate::login::{expect_id, LoginDecodeError};

/// Builder for a `0xD6` Object Property List (the "MegaCliloc" packet).
///
/// Entries are added in order; [`finish`](Self::finish) writes the terminator,
/// patches the length and the accumulated hash, and hands back the bytes together
/// with that hash — which the caller sends in the matching `0xDC`
/// ([`encode_opl_info`]).
#[derive(Clone, Debug)]
pub struct PropertyList {
    writer: PacketWriter,
    hash: u32,
}

impl PropertyList {
    /// The packet id, shared with the inbound batch query.
    pub const ID: u8 = 0xD6;

    /// The byte offset of the revision-hash field in the body, patched by
    /// [`finish`](Self::finish): after the id (1), length (2), the constant `1`
    /// (2), the serial (4) and the constant `0` (2).
    const HASH_OFFSET: usize = 11;

    /// Start a list for `serial`. The hash field is written as zero and patched
    /// once every entry is in.
    #[must_use]
    pub fn new(serial: u32) -> Self {
        let mut writer = PacketWriter::with_capacity(64);
        writer.u8(Self::ID);
        writer.u16(0); // length, patched in `finish`
        writer.u16(1); // constant
        writer.u32(serial);
        writer.u16(0); // constant
        writer.u32(0); // revision hash, patched in `finish`
        Self { writer, hash: 0 }
    }

    /// Fold a value into the running hash — ServUO's `AddHash`. The client never
    /// recomputes this; it only compares the revision it was told against the one
    /// it cached, so any stable-per-content function would do, but matching the
    /// reference keeps the arithmetic auditable.
    fn add_hash(&mut self, value: u32) {
        self.hash ^= value & 0x03FF_FFFF;
        self.hash ^= (value >> 26) & 0x3F;
    }

    /// A cliloc with no arguments — a bare localized string (an item's tiledata
    /// name, `1020000 + graphic`).
    pub fn add(&mut self, cliloc: u32) {
        self.add_hash(cliloc);
        self.writer.u32(cliloc);
        self.writer.u16(0); // no argument bytes
    }

    /// A cliloc with a tab-separated argument string, written UTF-16 LE. Used for
    /// the templated names — cliloc `1050045` (`~1_PREFIX~~2_NAME~~3_SUFFIX~`)
    /// with a mobile's name, cliloc `1050039` (`~1_NUMBER~ ~2_ITEMNAME~`) with a
    /// stack's amount.
    pub fn add_args(&mut self, cliloc: u32, arguments: &str) {
        self.add_hash(cliloc);
        self.add_hash(string_hash(arguments));
        self.writer.u32(cliloc);
        // UTF-16 little-endian, no terminator; the length is the byte count.
        let mut bytes = Vec::with_capacity(arguments.len() * 2);
        for unit in arguments.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let byte_count =
            u16::try_from(bytes.len()).expect("a tooltip argument outgrew its u16 len");
        self.writer.u16(byte_count);
        self.writer.bytes(&bytes);
    }

    /// Terminate the list, patch its length and hash, and return the bytes and the
    /// revision hash — the latter for the matching `0xDC`.
    #[must_use]
    pub fn finish(mut self) -> (Vec<u8>, u32) {
        self.writer.u32(0); // list terminator
        let hash = self.hash;
        let mut bytes = self.writer.into_bytes();
        let length = u16::try_from(bytes.len()).expect("a property list outgrew its u16 length");
        bytes[1..3].copy_from_slice(&length.to_be_bytes());
        bytes[Self::HASH_OFFSET..Self::HASH_OFFSET + 4].copy_from_slice(&hash.to_be_bytes());
        (bytes, hash)
    }
}

/// `0xDC` — the tooltip *revision* for one object: its serial and its property
/// hash. Sent when the object is drawn (in send-version mode) so the client knows
/// whether the tooltip it holds is current; a changed hash makes it ask for the
/// full list. Fixed nine bytes.
#[must_use]
pub fn encode_opl_info(serial: u32, hash: u32) -> Vec<u8> {
    let mut writer = PacketWriter::with_capacity(9);
    writer.u8(0xDC);
    writer.u32(serial);
    writer.u32(hash);
    writer.into_bytes()
}

/// `0xD6` inbound — the client asking for the full property list of one or more
/// objects (ServUO's `BatchQueryProperties`). Variable length: after the header,
/// a run of four-byte serials.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PropertyQueryRequest {
    /// The objects whose tooltips are wanted, by serial.
    pub serials: Vec<u32>,
}

impl PropertyQueryRequest {
    /// The packet id, shared with the outbound list.
    pub const ID: u8 = 0xD6;

    /// Decode a whole inbound `0xD6`. Trailing bytes that do not make a full
    /// serial are ignored rather than an error — the client pads sometimes.
    pub fn decode(bytes: &[u8]) -> Result<Self, LoginDecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        let mut serials = Vec::new();
        while reader.rest().len() >= 4 {
            serials.push(reader.u32()?);
        }
        Ok(Self { serials })
    }
}

/// A stable 32-bit hash of a tooltip argument string — FNV-1a over its bytes.
///
/// Only stability matters: the client compares revisions, it never recomputes
/// this, so the exact algorithm is free as long as it is deterministic (no
/// std-hash randomisation, so a replay hashes identically).
fn string_hash(value: &str) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for byte in value.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_property_list_lays_out_its_header_and_terminator() {
        let mut list = PropertyList::new(0x0000_1234);
        list.add(1_020_000 + 0x0EED); // an item's tiledata-name cliloc
        let (bytes, hash) = list.finish();

        assert_eq!(bytes[0], 0xD6);
        assert_eq!(
            u16::from_be_bytes([bytes[1], bytes[2]]),
            bytes.len() as u16,
            "the length is patched to the real size"
        );
        assert_eq!(&bytes[3..5], &1u16.to_be_bytes());
        assert_eq!(&bytes[5..9], &0x0000_1234u32.to_be_bytes(), "the serial");
        assert_eq!(&bytes[9..11], &0u16.to_be_bytes());
        assert_eq!(
            u32::from_be_bytes([bytes[11], bytes[12], bytes[13], bytes[14]]),
            hash,
            "the body carries the same hash the 0xDC will"
        );
        // one entry: cliloc (4) + arg length 0 (2), then the u32 terminator.
        assert_eq!(
            &bytes[15..19],
            &(1_020_000u32 + 0x0EED).to_be_bytes(),
            "the cliloc number"
        );
        assert_eq!(&bytes[19..21], &0u16.to_be_bytes(), "no argument bytes");
        assert_eq!(&bytes[bytes.len() - 4..], &0u32.to_be_bytes(), "terminated");
        assert_ne!(hash, 0, "a named object has a non-zero revision");
    }

    #[test]
    fn arguments_are_utf16_little_endian() {
        // The reason this is not the 0xAE speech encoder: OPL args are LE.
        let mut list = PropertyList::new(1);
        list.add_args(1_050_045, " \tHi\t ");
        let (bytes, _) = list.finish();
        // Find the arg run: header 15 bytes, then cliloc (4) + arg-len (2).
        let arg_len = u16::from_be_bytes([bytes[19], bytes[20]]) as usize;
        let args = &bytes[21..21 + arg_len];
        // " \tHi\t " as UTF-16 LE: each char one unit, low byte first.
        let expected: Vec<u8> = " \tHi\t "
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(args, expected.as_slice());
        assert_eq!(args[0], b' ', "low byte first: a space is 0x20 0x00");
        assert_eq!(args[1], 0x00);
    }

    #[test]
    fn the_opl_info_carries_the_list_hash() {
        let mut list = PropertyList::new(0x0000_00AB);
        list.add_args(1_050_045, " \tLord British\t ");
        let (_, hash) = list.finish();
        let info = encode_opl_info(0x0000_00AB, hash);
        assert_eq!(info.len(), 9);
        assert_eq!(info[0], 0xDC);
        assert_eq!(&info[1..5], &0x0000_00ABu32.to_be_bytes());
        assert_eq!(&info[5..9], &hash.to_be_bytes());
    }

    #[test]
    fn a_batch_query_reads_every_serial() {
        // 0xD6, length, then three serials.
        let mut bytes = vec![0xD6];
        let body_len = 3 + 3 * 4;
        bytes.extend_from_slice(&(body_len as u16).to_be_bytes());
        for serial in [0x1111_1111u32, 0x2222_2222, 0x3333_3333] {
            bytes.extend_from_slice(&serial.to_be_bytes());
        }
        let request = PropertyQueryRequest::decode(&bytes).unwrap();
        assert_eq!(request.serials, vec![0x1111_1111, 0x2222_2222, 0x3333_3333]);
    }

    #[test]
    fn the_hash_changes_when_the_name_changes() {
        let of = |name: &str| {
            let mut list = PropertyList::new(1);
            list.add_args(1_050_045, name);
            list.finish().1
        };
        assert_ne!(of(" \tArthur\t "), of(" \tGuinevere\t "));
    }
}
