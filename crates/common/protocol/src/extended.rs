//! `0xBF` — the "general information" extended-command envelope, decoded once.
//!
//! `[id][u16 length][u16 subcommand][body...]`. Four subcommands carry a
//! request this engine acts on — cast, the two context-menu packets, and the
//! stat-lock arrow — and each used to read the envelope for itself,
//! independently deciding whether a given `0xBF` was its own: the "three
//! different 0xBF types (context, casting, mobile) that each re-read the same
//! envelope and each decide independently whether the packet is theirs"
//! `docs/protocol_rewrite.md` calls out as the thing worth fixing. This reads
//! the subcommand once and hands the reader — already positioned past it — to
//! whichever body it names.

use crate::casting::CastSpellRequest;
use crate::chunks::{
    ChangesRequest,
    ChunkRequest,
};
use crate::context::{
    ContextMenuRequest,
    ContextMenuSelect,
};
use crate::craft::OpenCraftCatalogue;
use crate::design::DesignDetailsRequest;
use crate::error::{
    DecodeError,
    expect_id,
};
use crate::house_inventory::HouseInventoryRequest;
use crate::mapedit::MapEditRequest;
use crate::mobile::StatLockRequest;
use crate::party::PartyRequest;
use crate::world::TurnRequest;

/// A decoded `0xBF` client request.
///
/// Not `Copy` since parties: a line of party chat is a `String`, and every other
/// variant here is two or three integers. Cloning one is what a caller that used
/// to copy does instead.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ExtendedRequest {
    /// Subcommand `0x1C` — cast a spell.
    Cast(CastSpellRequest),
    /// Subcommand `0x13` — open an object's context menu.
    ContextMenuRequest(ContextMenuRequest),
    /// Subcommand `0x15` — a context-menu entry was picked.
    ContextMenuSelect(ContextMenuSelect),
    /// Subcommand `0x1A` — a stat's lock arrow moved.
    StatLock(StatLockRequest),
    /// Subcommand `0x1E` — "send me that house's design". The middle of the
    /// three-packet design conversation; see [`DesignDetailsRequest`].
    DesignDetails(DesignDetailsRequest),
    /// Subcommand `0x06` — everything a party does. Which of the seven it is
    /// lives in the body's first byte, not in the subcommand — see
    /// [`PartyRequest`].
    Party(PartyRequest),
    /// Subcommand `0xE002` — "send me these chunks of the world". This
    /// engine's own, and the only inbound one that is: no reference client has
    /// a word for it, which is exactly what makes receiving one the whole of
    /// the capability negotiation — see [`ChunkRequest`].
    Chunks(ChunkRequest),
    /// Subcommand `0xE007` — "what has moved since this revision?". This
    /// engine's own, like [`Chunks`](Self::Chunks) above, and asked only by a
    /// client that kept a copy of the ground it was given — see
    /// [`ChangesRequest`].
    Changes(ChangesRequest),
    /// Subcommand `0xE009` — commit a bounded batch of canonical map edits
    /// against an exact parent revision.
    MapEdit(MapEditRequest),
    /// Subcommand `0xE014` — turn on the spot, never step.
    Turn(TurnRequest),
    /// `0xBF.0xE015` — open the tool-free craft catalogue.
    CraftCatalogue(OpenCraftCatalogue),
    /// `0xBF.0xE018` — bounded exact-selector house inventory search/open.
    HouseInventory(HouseInventoryRequest),
    /// Any subcommand this engine does not act on — screen size, close-gump
    /// and the rest of the family `0xBF` carries. Not an error:
    /// the same "logged fact, not a dropped connection" treatment
    /// [`ClientPacket::Unknown`](crate::client_packet::ClientPacket::Unknown)
    /// gives an unhandled id.
    Unknown(u16),
}

impl ExtendedRequest {
    /// The packet id — the extended-command envelope.
    pub const ID: u8 = 0xBF;

    /// Decode a `0xBF`: the envelope once, then the one body its subcommand
    /// names.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = expect_id(bytes, Self::ID)?;
        let _length = reader.u16()?;
        let subcommand = reader.u16()?;
        Ok(match subcommand {
            CastSpellRequest::SUBCOMMAND => Self::Cast(CastSpellRequest::decode_body(&mut reader)?),
            ContextMenuRequest::SUBCOMMAND => {
                Self::ContextMenuRequest(ContextMenuRequest::decode_body(&mut reader)?)
            }
            ContextMenuSelect::SUBCOMMAND => {
                Self::ContextMenuSelect(ContextMenuSelect::decode_body(&mut reader)?)
            }
            StatLockRequest::SUBCOMMAND => Self::StatLock(StatLockRequest::decode_body(&mut reader)?),
            DesignDetailsRequest::SUBCOMMAND => {
                Self::DesignDetails(DesignDetailsRequest::decode_body(&mut reader)?)
            }
            crate::party::SUBCOMMAND => Self::Party(PartyRequest::decode_body(&mut reader)?),
            ChunkRequest::SUBCOMMAND => Self::Chunks(ChunkRequest::decode_body(&mut reader)?),
            ChangesRequest::SUBCOMMAND => Self::Changes(ChangesRequest::decode_body(&mut reader)?),
            MapEditRequest::SUBCOMMAND => Self::MapEdit(MapEditRequest::decode_body(&mut reader)?),
            TurnRequest::SUBCOMMAND => Self::Turn(TurnRequest::decode_body(&mut reader)?),
            OpenCraftCatalogue::SUBCOMMAND => {
                Self::CraftCatalogue(OpenCraftCatalogue::decode_body(&mut reader)?)
            }
            HouseInventoryRequest::SUBCOMMAND => {
                Self::HouseInventory(HouseInventoryRequest::decode_body(&mut reader)?)
            }
            other => Self::Unknown(other),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrecognised_subcommand_is_not_an_error() {
        // A close-gump (0x04) is a real 0xBF subcommand this engine does not
        // act on. It must read as Unknown, not fail the whole packet.
        let packet = vec![0xBF, 0x00, 0x07, 0x00, 0x04, 0x00, 0x00];
        assert_eq!(
            ExtendedRequest::decode(&packet).unwrap(),
            ExtendedRequest::Unknown(0x04)
        );
    }

    #[test]
    fn a_truncated_envelope_is_refused_not_panicked() {
        for cut in 1..5 {
            assert!(
                ExtendedRequest::decode(&[0xBF, 0x00, 0x07, 0x00, 0x1C][..cut]).is_err(),
                "a {cut}-byte packet must not decode"
            );
        }
    }
}
