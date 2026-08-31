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

use crate::codec::{
    CodecError,
    PacketReader,
    PacketWriter,
};
use crate::error::DecodeError;
use crate::packet::{
    DecodePacket,
    EncodePacket,
    PacketLength,
};
use crate::serial::{
    RawSerial,
    Serial,
};
use crate::version::ClientVersion;
use crate::wire::ClilocId;

/// Builder for a `0xD6` Object Property List (the "MegaCliloc" packet).
///
/// Entries are added in order; [`finish`](Self::finish) writes the terminator,
/// patches the length and the accumulated hash, and hands back the bytes together
/// with that hash — which the caller sends in the matching `0xDC`
/// ([`TooltipRevision`]).
#[derive(Clone, Debug)]
pub struct PropertyList {
    writer: PacketWriter,
    hash:   u32,
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
    pub fn new(serial: Serial) -> Self {
        let mut writer = PacketWriter::with_capacity(64);
        writer.u8(Self::ID);
        writer.u16(0); // length, patched in `finish`
        writer.u16(1); // constant
        writer.u32(serial.raw());
        writer.u16(0); // constant
        writer.u32(0); // revision hash, patched in `finish`
        Self { writer, hash: 0 }
    }

    /// Fold a value into the running hash — ServUO's `AddHash`. The client never
    /// recomputes this; it only compares the revision it was told against the one
    /// it cached, so any stable-per-content function would do, but matching the
    /// reference keeps the arithmetic auditable.
    const fn add_hash(&mut self, value: u32) {
        self.hash ^= value & 0x03FF_FFFF;
        self.hash ^= (value >> 26) & 0x3F;
    }

    /// A cliloc with no arguments — a bare localized string (an item's tiledata
    /// name, `1020000 + graphic`).
    pub fn add(&mut self, cliloc: ClilocId) {
        self.add_hash(cliloc.0);
        self.writer.u32(cliloc.0);
        self.writer.u16(0); // no argument bytes
    }

    /// A cliloc with a tab-separated argument string, written UTF-16 LE. Used for
    /// the templated names — cliloc `1050045` (`~1_PREFIX~~2_NAME~~3_SUFFIX~`)
    /// with a mobile's name, cliloc `1050039` (`~1_NUMBER~ ~2_ITEMNAME~`) with a
    /// stack's amount.
    pub fn add_args(&mut self, cliloc: ClilocId, arguments: &str) {
        self.add_hash(cliloc.0);
        self.add_hash(string_hash(arguments));
        self.writer.u32(cliloc.0);
        // UTF-16 little-endian, no terminator; the length is the byte count.
        let mut bytes = Vec::with_capacity(arguments.len() * 2);
        for unit in arguments.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let byte_count = u16::try_from(bytes.len()).expect("a tooltip argument outgrew its u16 len");
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TooltipRevision {
    /// The object this revision is for.
    pub serial: Serial,
    /// The same hash [`PropertyList::finish`] returned for it.
    ///
    /// Bare by decision, and not by any of N3's four classes: it is neither
    /// client input nor a value this crate names elsewhere — it is an opaque
    /// accumulator the *server* computes ([`PropertyList::add_hash`]) and only
    /// the *client* ever reads back, which is class D's shape reversed. See
    /// "Amendments forced by N7" in `docs/protocol_newtypes.md`.
    pub hash:   u32,
}

impl EncodePacket for TooltipRevision {
    const ID: u8 = 0xDC;
    const LENGTH: PacketLength = PacketLength::Fixed(9);

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u32(self.serial.raw());
        out.u32(self.hash);
    }
}

impl DecodePacket for TooltipRevision {
    const ID: u8 = 0xDC;

    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        Ok(Self {
            serial: read_serial(reader)?,
            hash:   reader.u32()?,
        })
    }
}

/// One line of a tooltip: a localized-string number and the arguments filled
/// into it.
///
/// The text itself is never on the wire. `cliloc` is looked up in the client's
/// own `cliloc.enu`, and `arguments` are the tab-separated substitutions its
/// `~1_NAME~` placeholders take — so a mobile's whole tooltip is the number
/// `1050045` and the string `" \tLord British\t "`, three fields of which two
/// are empty.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PropertyEntry {
    /// The localized string this line is.
    pub cliloc:    ClilocId,
    /// Its substitutions, tab-separated, empty when the string takes none.
    pub arguments: String,
}

/// `0xD6` inbound to a *client* — the property list itself, the answer to a
/// [`PropertyQueryRequest`].
///
/// The decoding counterpart of [`PropertyList`], which is a builder rather than
/// an [`EncodePacket`] because the server accumulates the hash as it writes. The
/// two are pinned together by a round-trip test rather than by sharing code, and
/// that test is the only thing keeping them honest — see
/// `a_built_list_decodes_and_re_encodes_to_itself`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PropertyListReply {
    /// The object this list describes.
    pub serial:  Serial,
    /// The revision the server computed for it, the same value its [`TooltipRevision`] carried.
    pub hash:    u32,
    /// The lines, in the order the server wrote them. The first is the name.
    pub entries: Vec<PropertyEntry>,
}

impl PropertyListReply {
    /// The packet id, shared with the outbound query.
    pub const ID: u8 = 0xD6;
}

impl DecodePacket for PropertyListReply {
    const ID: u8 = Self::ID;

    /// # Two constants read and dropped
    ///
    /// The body carries a `1` before the serial and a `0` after it, which every
    /// reference writes and none explains. They are skipped rather than checked:
    /// this decoder's job is to read what a shard sends, and refusing a list
    /// because a field nobody documents held a different number would lose a
    /// tooltip to defend nothing.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        reader.skip(2)?; // the constant 1
        let serial = read_serial(reader)?;
        reader.skip(2)?; // the constant 0
        let hash = reader.u32()?;

        let mut entries = Vec::new();
        loop {
            let cliloc = reader.u32()?;
            if cliloc == 0 {
                break; // the terminator
            }
            let byte_count = usize::from(reader.u16()?);
            entries.push(PropertyEntry {
                cliloc:    ClilocId(cliloc),
                arguments: utf16_le(reader.bytes(byte_count)?)?,
            });
        }
        Ok(Self {
            serial,
            hash,
            entries,
        })
    }
}

impl EncodePacket for PropertyListReply {
    const ID: u8 = Self::ID;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        out.u16(1);
        out.u32(self.serial.raw());
        out.u16(0);
        out.u32(self.hash);
        for entry in &self.entries {
            out.u32(entry.cliloc.0);
            let mut bytes = Vec::with_capacity(entry.arguments.len() * 2);
            for unit in entry.arguments.encode_utf16() {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            let byte_count = u16::try_from(bytes.len()).expect("a tooltip argument outgrew its u16 len");
            out.u16(byte_count);
            out.bytes(&bytes);
        }
        out.u32(0);
    }
}

/// Read a serial that names a real object, refusing the reserved range.
///
/// Both packets here are *about* one object, so a serial outside the mobile and
/// item ranges is not a tooltip for something odd — it is a misread body, and
/// the next field would be read at the wrong offset.
fn read_serial(reader: &mut PacketReader<'_>) -> Result<Serial, DecodeError> {
    let raw = reader.u32()?;
    Serial::new(raw).ok_or(DecodeError::UnknownValue {
        field: "property list serial",
        value: raw,
    })
}

/// Read an argument run as UTF-16 **little-endian**, dropping a trailing NUL.
///
/// Local to this module because the LE-ness is: `0xAE` speech is big-endian
/// UTF-16 and `PacketReader::fixed_string_utf16` reads that one. The trailing
/// NUL is tolerated but not required — this engine's encoder writes none, and
/// some servers count a terminator inside the byte length.
fn utf16_le(bytes: &[u8]) -> Result<String, DecodeError> {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16(&units).map_err(|_| DecodeError::Codec(CodecError::InvalidText))
}

/// `0xD6` inbound — the client asking for the full property list of one or more
/// objects (ServUO's `BatchQueryProperties`). Variable length: after the header,
/// a run of four-byte serials.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PropertyQueryRequest {
    /// The objects whose tooltips are wanted, by serial, exactly as sent — one
    /// the client cannot see, or that names nothing, is simply skipped at the
    /// seam (`World::query_properties`), never refused.
    pub serials: Vec<RawSerial>,
}

impl DecodePacket for PropertyQueryRequest {
    /// The packet id, shared with the outbound list.
    const ID: u8 = 0xD6;

    /// Decode a whole inbound `0xD6`. Trailing bytes that do not make a full
    /// serial are ignored rather than an error — the client pads sometimes.
    fn decode_body(reader: &mut PacketReader<'_>, _version: ClientVersion) -> Result<Self, DecodeError> {
        let mut serials = Vec::new();
        while reader.rest().len() >= 4 {
            serials.push(RawSerial(reader.u32()?));
        }
        Ok(Self { serials })
    }
}

impl EncodePacket for PropertyQueryRequest {
    const ID: u8 = 0xD6;
    const LENGTH: PacketLength = PacketLength::Variable;

    fn encode_body(&self, out: &mut PacketWriter, _version: ClientVersion) {
        for serial in &self.serials {
            out.u32(serial.0);
        }
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
    use crate::packet::{
        decode_packet,
        encode_packet,
    };

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    #[test]
    fn a_property_list_lays_out_its_header_and_terminator() {
        let mut list = PropertyList::new(Serial::new(0x0000_1234).unwrap());
        list.add(ClilocId(1_020_000 + 0x0EED)); // an item's tiledata-name cliloc
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
        let mut list = PropertyList::new(Serial::new(1).unwrap());
        list.add_args(ClilocId(1_050_045), " \tHi\t ");
        let (bytes, _) = list.finish();
        // Find the arg run: header 15 bytes, then cliloc (4) + arg-len (2).
        let arg_len = u16::from_be_bytes([bytes[19], bytes[20]]) as usize;
        let args = &bytes[21..21 + arg_len];
        // " \tHi\t " as UTF-16 LE: each char one unit, low byte first.
        let expected: Vec<u8> = " \tHi\t ".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(args, expected.as_slice());
        assert_eq!(args[0], b' ', "low byte first: a space is 0x20 0x00");
        assert_eq!(args[1], 0x00);
    }

    #[test]
    fn the_opl_info_carries_the_list_hash() {
        let mut list = PropertyList::new(Serial::new(0x0000_00AB).unwrap());
        list.add_args(ClilocId(1_050_045), " \tLord British\t ");
        let (_, hash) = list.finish();
        let info = encode_packet(
            &TooltipRevision {
                serial: Serial::new(0x0000_00AB).unwrap(),
                hash,
            },
            version(),
        );
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
        let request: PropertyQueryRequest = decode_packet(&bytes, version()).unwrap();
        assert_eq!(
            request.serials,
            vec![
                RawSerial(0x1111_1111),
                RawSerial(0x2222_2222),
                RawSerial(0x3333_3333)
            ]
        );
    }

    #[test]
    fn a_built_list_decodes_and_re_encodes_to_itself() {
        // The only thing keeping the builder and the decoder in step. They share
        // no code — the builder accumulates a hash as it writes and the reply
        // carries one already computed — so without this, a change to either
        // side's layout is a silent parity break that only a real client sees.
        let mut list = PropertyList::new(Serial::new(0x4000_0001).unwrap());
        list.add(ClilocId(1_020_000 + 0x0EED));
        list.add_args(ClilocId(1_050_045), " \tLord British\t [OSS]");
        list.add_args(ClilocId(1_042_971), "Warlord, The Silver Serpent");
        let (bytes, hash) = list.finish();

        let reply: PropertyListReply = decode_packet(&bytes, version()).unwrap();
        assert_eq!(reply.serial, Serial::new(0x4000_0001).unwrap());
        assert_eq!(reply.hash, hash, "the decoder reads the hash the builder wrote");
        assert_eq!(
            reply.entries,
            vec![
                PropertyEntry {
                    cliloc:    ClilocId(1_020_000 + 0x0EED),
                    arguments: String::new(),
                },
                PropertyEntry {
                    cliloc:    ClilocId(1_050_045),
                    arguments: " \tLord British\t [OSS]".to_owned(),
                },
                PropertyEntry {
                    cliloc:    ClilocId(1_042_971),
                    arguments: "Warlord, The Silver Serpent".to_owned(),
                },
            ]
        );
        assert_eq!(encode_packet(&reply, version()), bytes, "byte-for-byte");
    }

    #[test]
    fn an_argument_terminator_is_tolerated_and_dropped() {
        // This engine writes no NUL after an argument run, but the length field
        // is a byte count and some servers count a terminator inside it. Reading
        // one as text would put a stray character at the end of every tooltip.
        let mut body = vec![0xD6u8, 0, 0];
        body.extend_from_slice(&1u16.to_be_bytes());
        body.extend_from_slice(&0x4000_0001u32.to_be_bytes());
        body.extend_from_slice(&0u16.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes()); // hash
        body.extend_from_slice(&1_050_045u32.to_be_bytes());
        let args: Vec<u8> = "Hi\0".encode_utf16().flat_map(u16::to_le_bytes).collect();
        body.extend_from_slice(&(args.len() as u16).to_be_bytes());
        body.extend_from_slice(&args);
        body.extend_from_slice(&0u32.to_be_bytes()); // terminator
        let length = u16::try_from(body.len()).unwrap();
        body[1..3].copy_from_slice(&length.to_be_bytes());

        let reply: PropertyListReply = decode_packet(&body, version()).unwrap();
        assert_eq!(reply.entries[0].arguments, "Hi");
    }

    #[test]
    fn a_revision_round_trips() {
        // 0xDC had an encoder and no decoder, so it reached the client as an
        // undecoded id — which is why nothing on that end ever knew a tooltip
        // had gone stale.
        let revision = TooltipRevision {
            serial: Serial::new(0x0000_00AB).unwrap(),
            hash:   0xDEAD_BEEF,
        };
        let bytes = encode_packet(&revision, version());
        assert_eq!(
            decode_packet::<TooltipRevision>(&bytes, version()).unwrap(),
            revision
        );
    }

    #[test]
    fn a_batch_query_round_trips() {
        let request = PropertyQueryRequest {
            serials: vec![RawSerial(0x4000_0001), RawSerial(0x4000_0002)],
        };
        let bytes = encode_packet(&request, version());
        assert_eq!(bytes[0], 0xD6);
        assert_eq!(
            decode_packet::<PropertyQueryRequest>(&bytes, version()).unwrap(),
            request
        );
    }

    #[test]
    fn the_hash_changes_when_the_name_changes() {
        let of = |name: &str| {
            let mut list = PropertyList::new(Serial::new(1).unwrap());
            list.add_args(ClilocId(1_050_045), name);
            list.finish().1
        };
        assert_ne!(of(" \tArthur\t "), of(" \tGuinevere\t "));
    }
}
