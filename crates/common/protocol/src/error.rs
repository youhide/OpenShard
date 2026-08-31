//! What "this packet is not what it claims" looks like.
//!
//! Every byte handled here came off a socket, so decoding is fallible all the
//! way down: a decoder returns [`DecodeError`] and never panics, never
//! substitutes a default and never returns a partly-filled value. See
//! [`crate::codec`] for why.

use std::fmt;

use crate::codec::{
    CodecError,
    PacketReader,
};

/// A packet did not have the id it was decoded as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WrongPacket {
    /// The id the decoder wanted.
    pub expected: u8,
    /// The id the packet actually had.
    pub found:    u8,
}

impl fmt::Display for WrongPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "expected packet 0x{:02X}, found 0x{:02X}",
            self.expected, self.found
        )
    }
}

impl std::error::Error for WrongPacket {
}

/// Decoding a packet failed.
///
/// The two cases are worth telling apart: a wrong id means the dispatcher
/// routed the packet to the wrong decoder, which is a server bug, while a codec
/// error means the client sent something short or malformed, which is the
/// client's business and only ever costs it its connection.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum DecodeError {
    /// The packet was not the one expected.
    WrongPacket(WrongPacket),
    /// The body was malformed.
    Codec(CodecError),
    /// A field held a value the protocol does not define.
    ///
    /// Distinct from a codec error: the bytes were all there and the shape was
    /// right, and one of them named something that does not exist — a deny code
    /// outside the five the client knows, a direction above seven. Nothing can
    /// be salvaged from it, and substituting the nearest legal value would be a
    /// decoder inventing data.
    UnknownValue {
        /// Which field, for the log line.
        field: &'static str,
        /// What it held.
        value: u32,
    },
    /// A legitimate wire form this crate does not decode.
    ///
    /// Neither malformed nor misrouted: the bytes are fine and the decoder for
    /// the shape they are in has not been written. Says so rather than reading
    /// the bytes as the shape it does know, which would succeed and produce
    /// wrong values.
    Unsupported {
        /// The packet id.
        packet: u8,
        /// The form that is not handled, named for a human.
        form:   &'static str,
    },
}

impl From<CodecError> for DecodeError {
    fn from(error: CodecError) -> Self {
        Self::Codec(error)
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongPacket(error) => error.fmt(f),
            Self::Codec(error) => error.fmt(f),
            Self::UnknownValue { field, value } => {
                write!(f, "{field} held {value}, which names nothing")
            }
            Self::Unsupported { packet, form } => {
                write!(f, "packet 0x{packet:02X} arrived as {form}, which is not decoded")
            }
        }
    }
}

impl std::error::Error for DecodeError {
}

/// Check and strip the id byte.
pub(crate) fn expect_id(bytes: &[u8], expected: u8) -> Result<PacketReader<'_>, DecodeError> {
    let mut reader = PacketReader::new(bytes);
    let found = reader.u8()?;
    if found == expected {
        Ok(reader)
    } else {
        Err(DecodeError::WrongPacket(WrongPacket { expected, found }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expect_id_strips_the_id_it_matched() {
        let mut reader = expect_id(&[0x02, 0xAB], 0x02).unwrap();
        assert_eq!(reader.u8().unwrap(), 0xAB, "cursor sits past the id");
    }

    #[test]
    fn expect_id_names_both_sides_of_a_mismatch() {
        let error = expect_id(&[0x02], 0x03).unwrap_err();
        assert_eq!(
            error,
            DecodeError::WrongPacket(WrongPacket {
                expected: 0x03,
                found:    0x02,
            })
        );
    }

    /// An empty slice has no id to check, which is a codec error rather than a
    /// mismatch: there is nothing to compare against.
    #[test]
    fn expect_id_on_an_empty_packet_is_a_codec_error() {
        let error = expect_id(&[], 0x02).unwrap_err();
        assert!(matches!(error, DecodeError::Codec(_)), "{error:?}");
    }
}
