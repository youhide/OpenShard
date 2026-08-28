//! The tool-free craft catalogue request.
//!
//! A craft tool is still what *makes* an item, but it is a poor affordance for
//! learning the game: a player without tongs cannot even see what tongs would
//! let them make. This private `0xBF` request opens the read-only catalogue.

use crate::access::OPENSHARD_SUBCOMMANDS;
use crate::codec::PacketReader;
use crate::error::DecodeError;
use crate::packet::{PacketLength, frame_body};

/// `0xBF.0xE015` — open the craft catalogue without selecting a tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenCraftCatalogue;

impl OpenCraftCatalogue {
    /// The first private subcommand after the turn request.
    pub const SUBCOMMAND: u16 = OPENSHARD_SUBCOMMANDS + 21;

    /// Read the empty body. Extra bytes are refused so a future extension must
    /// name its versioned shape instead of silently changing this request.
    pub(crate) fn decode_body(reader: &mut PacketReader<'_>) -> Result<Self, DecodeError> {
        if reader.remaining() != 0 {
            return Err(DecodeError::UnknownValue {
                field: "craft catalogue body byte count",
                value: u32::try_from(reader.remaining()).unwrap_or(u32::MAX),
            });
        }
        Ok(Self)
    }

    /// Encode the complete extended request.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        frame_body(0xBF, PacketLength::Variable, |out| out.u16(Self::SUBCOMMAND))
    }
}

#[cfg(test)]
mod tests {
    use crate::extended::ExtendedRequest;

    use super::*;

    #[test]
    fn the_catalogue_request_round_trips_through_the_extended_envelope() {
        assert_eq!(
            ExtendedRequest::decode(&OpenCraftCatalogue.encode()).unwrap(),
            ExtendedRequest::CraftCatalogue(OpenCraftCatalogue)
        );
    }
}
