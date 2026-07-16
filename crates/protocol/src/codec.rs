//! Reading and writing packet bodies.
//!
//! # Everything here is fallible
//!
//! These bytes came off a socket. There is no such thing as a malformed packet
//! that "cannot happen" — a hostile client will send a 0x02 movement request
//! truncated to three bytes to find out what the server does. So no read panics
//! and no read is infallible: [`PacketReader`] returns [`CodecError`] and the
//! connection is dropped.
//!
//! # Endianness
//!
//! UO is big-endian on the wire, throughout, including its UTF-16 strings.

use std::fmt;

/// A packet could not be read.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum CodecError {
    /// The packet ended before the field did.
    UnexpectedEnd {
        /// How many bytes the read needed.
        needed: usize,
        /// How many were left.
        available: usize,
    },
    /// A string field held a byte sequence that is not valid text.
    InvalidText,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd { needed, available } => {
                write!(f, "packet ended: needed {needed} bytes, had {available}")
            }
            Self::InvalidText => f.write_str("packet held invalid text"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Result of a read.
pub type CodecResult<T> = Result<T, CodecError>;

/// A cursor over a packet body.
///
/// Reads advance the cursor. Every one is bounds-checked; a short packet is an
/// error, never a panic and never a partial value.
///
/// ```
/// use openshard_protocol::PacketReader;
///
/// // 0x02 movement request: direction, sequence, fastwalk key.
/// let mut reader = PacketReader::new(&[0x01, 0x2A, 0xDE, 0xAD, 0xBE, 0xEF]);
/// assert_eq!(reader.u8().unwrap(), 0x01);
/// assert_eq!(reader.u8().unwrap(), 0x2A);
/// assert_eq!(reader.u32().unwrap(), 0xDEAD_BEEF);
/// assert!(reader.is_empty());
///
/// // Reading past the end is an error, not a panic.
/// assert!(reader.u8().is_err());
/// ```
#[derive(Clone, Debug)]
pub struct PacketReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> PacketReader<'a> {
    /// A reader over `bytes`, positioned at the start.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    /// How many bytes have been read.
    pub const fn position(&self) -> usize {
        self.position
    }

    /// How many bytes are left.
    pub const fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    /// Whether every byte has been read.
    pub const fn is_empty(&self) -> bool {
        self.position >= self.bytes.len()
    }

    /// The bytes not yet read.
    pub fn rest(&self) -> &'a [u8] {
        &self.bytes[self.position..]
    }

    fn take(&mut self, count: usize) -> CodecResult<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(CodecError::UnexpectedEnd {
                needed: count,
                available: self.remaining(),
            })?;
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    /// Read one byte.
    pub fn u8(&mut self) -> CodecResult<u8> {
        Ok(self.take(1)?[0])
    }

    /// Read one byte as a bool. Any non-zero value is true, as the client sends
    /// both 1 and 0xFF for "yes" depending on the packet.
    pub fn bool(&mut self) -> CodecResult<bool> {
        Ok(self.u8()? != 0)
    }

    /// Read a big-endian `u16`.
    pub fn u16(&mut self) -> CodecResult<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// Read a big-endian `u32`.
    pub fn u32(&mut self) -> CodecResult<u32> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// Read a big-endian `i32`.
    pub fn i32(&mut self) -> CodecResult<i32> {
        Ok(self.u32()? as i32)
    }

    /// Read exactly `count` raw bytes.
    pub fn bytes(&mut self, count: usize) -> CodecResult<&'a [u8]> {
        self.take(count)
    }

    /// Skip `count` bytes.
    pub fn skip(&mut self, count: usize) -> CodecResult<()> {
        self.take(count).map(|_| ())
    }

    /// Read a fixed-width ASCII string of `len` bytes.
    ///
    /// The client pads these with NULs and does not always zero the tail, so
    /// everything from the first NUL on is dropped rather than trusted.
    pub fn fixed_string(&mut self, len: usize) -> CodecResult<String> {
        let raw = self.take(len)?;
        let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
        // UO's "ASCII" is really Latin-1; decoding as UTF-8 would reject the
        // accented characters some clients send.
        Ok(raw[..end].iter().map(|b| *b as char).collect())
    }

    /// Read a fixed-width big-endian UTF-16 string of `len` *characters*.
    pub fn fixed_string_utf16(&mut self, len: usize) -> CodecResult<String> {
        let raw = self.take(len.checked_mul(2).ok_or(CodecError::InvalidText)?)?;
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect();
        String::from_utf16(&units).map_err(|_| CodecError::InvalidText)
    }

    /// Read a NUL-terminated ASCII string, consuming the terminator.
    ///
    /// Returns [`CodecError::UnexpectedEnd`] if no terminator is found, rather
    /// than treating the rest of the packet as the string.
    pub fn null_terminated_string(&mut self) -> CodecResult<String> {
        let rest = self.rest();
        let end = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or(CodecError::UnexpectedEnd {
                needed: rest.len() + 1,
                available: rest.len(),
            })?;
        let text: String = rest[..end].iter().map(|b| *b as char).collect();
        self.position += end + 1;
        Ok(text)
    }
}

/// Builds a packet body.
///
/// Writes are infallible — the buffer grows. Framing (the packet id and, for
/// variable-length packets, the length field) is not this type's job.
///
/// ```
/// use openshard_protocol::PacketWriter;
///
/// let mut writer = PacketWriter::new();
/// writer.u8(0x11);
/// writer.u32(0x0000_0001);
/// assert_eq!(writer.as_bytes(), &[0x11, 0x00, 0x00, 0x00, 0x01]);
/// ```
#[derive(Clone, Default, Debug)]
pub struct PacketWriter {
    bytes: Vec<u8>,
}

impl PacketWriter {
    /// An empty writer.
    pub const fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// An empty writer with room for `capacity` bytes.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    /// How many bytes have been written.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing has been written.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// What has been written so far.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Take the written bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Write one byte.
    pub fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// Write a bool as one byte.
    pub fn bool(&mut self, value: bool) {
        self.bytes.push(u8::from(value));
    }

    /// Write a big-endian `u16`.
    pub fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a big-endian `u32`.
    pub fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a big-endian `i32`.
    pub fn i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    /// Write raw bytes.
    pub fn bytes(&mut self, value: &[u8]) {
        self.bytes.extend_from_slice(value);
    }

    /// Write `count` zero bytes.
    pub fn zeros(&mut self, count: usize) {
        self.bytes.resize(self.bytes.len() + count, 0);
    }

    /// Write an ASCII string in exactly `len` bytes, NUL-padded.
    ///
    /// Truncates if the string is longer. Truncation is correct here: these are
    /// fixed-width protocol fields, and a name that overruns its field is the
    /// caller's bug, not something to fail a live connection over. Callers that
    /// care validate length before this point.
    ///
    /// Characters above U+00FF become `?`, since the field is one byte per
    /// character.
    pub fn fixed_string(&mut self, value: &str, len: usize) {
        let start = self.bytes.len();
        for character in value.chars().take(len) {
            self.bytes.push(latin1_byte(character));
        }
        let written = self.bytes.len() - start;
        self.zeros(len - written);
    }

    /// Write a big-endian UTF-16 string in exactly `len` code units, NUL-padded.
    ///
    /// Truncates on a character boundary. A character needing a surrogate pair
    /// is dropped whole rather than split — half a pair is not valid UTF-16 and
    /// would make the field unreadable.
    pub fn fixed_string_utf16(&mut self, value: &str, len: usize) {
        let mut buffer = [0u16; 2];
        let mut written = 0;
        for character in value.chars() {
            let units = character.encode_utf16(&mut buffer);
            if written + units.len() > len {
                break;
            }
            for unit in units.iter() {
                self.u16(*unit);
            }
            written += units.len();
        }
        self.zeros((len - written) * 2);
    }

    /// Write an ASCII string followed by a NUL.
    pub fn null_terminated_string(&mut self, value: &str) {
        for character in value.chars() {
            self.bytes.push(latin1_byte(character));
        }
        self.bytes.push(0);
    }
}

/// Narrow a character to one Latin-1 byte, substituting `?` when it does not fit.
fn latin1_byte(character: char) -> u8 {
    let code = character as u32;
    if code <= 0xFF {
        code as u8
    } else {
        b'?'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_big_endian() {
        let mut reader = PacketReader::new(&[0x12, 0x34, 0x56, 0x78, 0x9A]);
        assert_eq!(reader.u8().unwrap(), 0x12);
        assert_eq!(reader.u16().unwrap(), 0x3456);
        assert_eq!(reader.u16().unwrap(), 0x789A);
        assert!(reader.is_empty());
    }

    #[test]
    fn tracks_position_and_remaining() {
        let mut reader = PacketReader::new(&[1, 2, 3, 4]);
        assert_eq!((reader.position(), reader.remaining()), (0, 4));
        reader.u16().unwrap();
        assert_eq!((reader.position(), reader.remaining()), (2, 2));
        assert_eq!(reader.rest(), &[3, 4]);
    }

    #[test]
    fn a_short_packet_is_an_error_not_a_panic() {
        // A hostile client truncates packets to see what breaks.
        let mut reader = PacketReader::new(&[0x01]);
        assert_eq!(
            reader.u32(),
            Err(CodecError::UnexpectedEnd {
                needed: 4,
                available: 1
            })
        );
        assert_eq!(reader.position(), 0, "a failed read must not consume");
        assert_eq!(reader.u8().unwrap(), 0x01, "the reader is still usable");
    }

    #[test]
    fn an_empty_packet_reads_nothing() {
        let mut reader = PacketReader::new(&[]);
        assert!(reader.is_empty());
        assert!(reader.u8().is_err());
        assert_eq!(reader.rest(), &[] as &[u8]);
    }

    #[test]
    fn take_cannot_overflow_into_a_wrap() {
        // `position + count` must not wrap around and pass the bounds check.
        let mut reader = PacketReader::new(&[1, 2, 3]);
        assert!(reader.bytes(usize::MAX).is_err());
        assert_eq!(reader.position(), 0);
    }

    #[test]
    fn signed_values_round_trip() {
        let mut writer = PacketWriter::new();
        writer.i32(-1);
        writer.i32(i32::MIN);
        let bytes = writer.into_bytes();
        let mut reader = PacketReader::new(&bytes);
        assert_eq!(reader.i32().unwrap(), -1);
        assert_eq!(reader.i32().unwrap(), i32::MIN);
    }

    #[test]
    fn fixed_string_stops_at_the_first_nul() {
        // The client does not reliably zero the tail of a fixed field.
        let mut reader = PacketReader::new(b"Lord\0\0garbage!!!");
        assert_eq!(reader.fixed_string(16).unwrap(), "Lord");
        assert!(reader.is_empty(), "the whole field is consumed regardless");
    }

    #[test]
    fn fixed_string_without_a_nul_uses_the_whole_field() {
        let mut reader = PacketReader::new(b"Britannia");
        assert_eq!(reader.fixed_string(9).unwrap(), "Britannia");
    }

    #[test]
    fn fixed_string_decodes_latin1_not_utf8() {
        // 0xE9 is 'é' in Latin-1 and invalid on its own in UTF-8. Rejecting it
        // would drop connections from clients sending accented names.
        let mut reader = PacketReader::new(&[b'J', 0xE9, b'r', 0x00]);
        assert_eq!(reader.fixed_string(4).unwrap(), "J\u{E9}r");
    }

    #[test]
    fn utf16_never_writes_half_a_surrogate_pair() {
        // U+1F600 needs two code units. With one unit of room it must be
        // dropped, not split — a lone surrogate makes the field unreadable.
        let mut writer = PacketWriter::new();
        writer.fixed_string_utf16("a\u{1F600}", 2);
        assert_eq!(writer.as_bytes(), &[0x00, b'a', 0x00, 0x00]);

        let mut reader = PacketReader::new(writer.as_bytes());
        assert_eq!(reader.fixed_string_utf16(2).unwrap(), "a");
    }

    #[test]
    fn fixed_string_round_trips() {
        let mut writer = PacketWriter::new();
        writer.fixed_string("Lord British", 30);
        assert_eq!(writer.len(), 30, "the field is always its full width");

        let bytes = writer.into_bytes();
        assert_eq!(&bytes[12..], &[0u8; 18], "the tail is NUL-padded");

        let mut reader = PacketReader::new(&bytes);
        assert_eq!(reader.fixed_string(30).unwrap(), "Lord British");
    }

    #[test]
    fn fixed_string_truncates_to_the_field_width() {
        let mut writer = PacketWriter::new();
        writer.fixed_string("aaaaaaaaaa", 4);
        assert_eq!(writer.as_bytes(), b"aaaa", "no overrun of a fixed field");
    }

    #[test]
    fn fixed_string_substitutes_characters_that_do_not_fit() {
        let mut writer = PacketWriter::new();
        writer.fixed_string("a\u{4E2D}b", 3);
        assert_eq!(writer.as_bytes(), b"a?b");
    }

    #[test]
    fn utf16_round_trips() {
        let mut writer = PacketWriter::new();
        writer.fixed_string_utf16("Hail", 10);
        assert_eq!(writer.len(), 20, "two bytes per character");

        let bytes = writer.into_bytes();
        assert_eq!(&bytes[..2], &[0x00, b'H'], "big-endian on the wire");

        let mut reader = PacketReader::new(&bytes);
        assert_eq!(reader.fixed_string_utf16(10).unwrap(), "Hail");
    }

    #[test]
    fn utf16_rejects_an_unpaired_surrogate() {
        // 0xD800 alone is not valid UTF-16; it must not become a lossy '?'.
        let bytes = [0xD8, 0x00, 0x00, 0x00];
        let mut reader = PacketReader::new(&bytes);
        assert_eq!(reader.fixed_string_utf16(2), Err(CodecError::InvalidText));
    }

    #[test]
    fn utf16_length_cannot_overflow() {
        let mut reader = PacketReader::new(&[0, 0]);
        assert!(reader.fixed_string_utf16(usize::MAX).is_err());
    }

    #[test]
    fn null_terminated_round_trips() {
        let mut writer = PacketWriter::new();
        writer.null_terminated_string("go");
        writer.null_terminated_string("britain");
        let bytes = writer.into_bytes();
        assert_eq!(bytes.as_slice(), b"go\0britain\0");

        let mut reader = PacketReader::new(&bytes);
        assert_eq!(reader.null_terminated_string().unwrap(), "go");
        assert_eq!(reader.null_terminated_string().unwrap(), "britain");
        assert!(reader.is_empty());
    }

    #[test]
    fn an_unterminated_string_is_an_error() {
        // Not "take the rest of the packet" — that is how a truncated packet
        // turns into a plausible-looking command.
        let mut reader = PacketReader::new(b"no terminator here");
        assert!(reader.null_terminated_string().is_err());
    }

    #[test]
    fn an_empty_null_terminated_string_is_valid() {
        let mut reader = PacketReader::new(&[0x00, 0x41]);
        assert_eq!(reader.null_terminated_string().unwrap(), "");
        assert_eq!(reader.u8().unwrap(), 0x41);
    }

    #[test]
    fn skip_and_zeros_agree() {
        let mut writer = PacketWriter::new();
        writer.u8(1);
        writer.zeros(3);
        writer.u8(2);
        let bytes = writer.into_bytes();
        assert_eq!(bytes.as_slice(), &[1, 0, 0, 0, 2]);

        let mut reader = PacketReader::new(&bytes);
        assert_eq!(reader.u8().unwrap(), 1);
        reader.skip(3).unwrap();
        assert_eq!(reader.u8().unwrap(), 2);
    }

    #[test]
    fn bool_treats_any_nonzero_as_true() {
        // The client sends 0xFF for "yes" in some packets and 0x01 in others.
        let mut reader = PacketReader::new(&[0x00, 0x01, 0xFF]);
        assert!(!reader.bool().unwrap());
        assert!(reader.bool().unwrap());
        assert!(reader.bool().unwrap());
    }
}
