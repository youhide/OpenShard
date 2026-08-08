//! Every client-to-server packet the world acts on, decoded once.
//!
//! Before this, `server/server/src/dispatch.rs` matched the raw id byte to
//! pick a handler, and each handler decoded the same bytes again — one place
//! that knew the id, another that knew the type. [`ClientPacket::decode`]
//! does both in one pass, mirroring [`crate::login::LoginStagePacket`],
//! which did the same for the login conversation first. `dispatch` matches on
//! the result and touches no raw packet buffer itself.

use crate::combat::{AttackRequest, WarMode};
use crate::containers::DoubleClick;
use crate::encoded::EncodedCommand;
use crate::error::DecodeError;
use crate::extended::ExtendedRequest;
use crate::gump::GumpResponse;
use crate::items::{DropItem, EquipItemRequest, PickUpItem};
use crate::mobile::{LookRequest, StatusQuery};
use crate::packet::{DecodePacket, decode_packet};
use crate::properties::PropertyQueryRequest;
use crate::skill::{SkillLockRequest, UseSkillRequest};
use crate::speech::{TalkRequest, UnicodeTalkRequest};
use crate::target::TargetResponse;
use crate::trade::SecureTradeAction;
use crate::vendor::{BuyReply, SellReply};
use crate::version::ClientVersion;
use crate::world::{ResyncRequest, WalkRequest};

/// `0xD1` from the client is a bare notification — "log me out" — with no
/// body to decode. Kept private: nothing outside [`ClientPacket::decode`]
/// needs the id on its own.
const LOGOUT_REQUEST_ID: u8 = 0xD1;

/// One packet the world dispatcher understood, already decoded.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ClientPacket {
    /// `0x02` — a movement request.
    Walk(WalkRequest),
    /// `0xD1` — "Log Out" on the paperdoll.
    LogoutRequest,
    /// `0x22` from the client — "tell me where I am".
    ///
    /// Sent when the walk handshake has fallen out of step: an ack the client
    /// cannot place, which it answers by asking rather than by guessing. It
    /// carries no body — the question is the whole packet — and the answer is a
    /// `0x20` with the real position, everything in view again, and both
    /// sequences back to zero. See [`RESYNC_REQUEST_ID`] for why this shares an
    /// id with the walk ack.
    ResyncRequest,
    /// `0x34` — a status-bar or skill-list query.
    StatusQuery(StatusQuery),
    /// `0xD7` — the AoS encoded command (the paperdoll's Quest and Guild
    /// buttons).
    Encoded(EncodedCommand),
    /// `0xB1` — a gump button was pressed.
    GumpResponse(GumpResponse),
    /// `0x6C` — a target cursor resolved.
    TargetResponse(TargetResponse),
    /// `0x07` — lift an item.
    PickUpItem(PickUpItem),
    /// `0x08` — put the lifted item down.
    DropItem(DropItem),
    /// `0x06` — double-click an object.
    DoubleClick(DoubleClick),
    /// `0x3B` — a vendor purchase.
    Buy(BuyReply),
    /// `0x9F` — a vendor sale.
    Sell(SellReply),
    /// `0x09` — single-click an object.
    Look(LookRequest),
    /// `0xD6` — the AoS tooltip batch query.
    PropertyQuery(PropertyQueryRequest),
    /// `0x13` — equip the lifted item onto a mobile.
    Equip(EquipItemRequest),
    /// `0x72` — toggle war mode.
    WarMode(WarMode),
    /// `0x05` — attack a target.
    Attack(AttackRequest),
    /// `0x03` — ASCII speech.
    Talk(TalkRequest),
    /// `0xAD` — Unicode speech.
    UnicodeTalk(UnicodeTalkRequest),
    /// `0xBF` — the extended-command envelope; see [`ExtendedRequest`].
    Extended(ExtendedRequest),
    /// `0x12` type `0x24` — "use skill" from the text-command envelope.
    UseSkill(UseSkillRequest),
    /// `0x3A` from the client — a skill's lock arrow moved.
    SkillLock(SkillLockRequest),
    /// `0x6F` — a secure trade window action.
    SecureTrade(SecureTradeAction),
    /// An id this crate has no handler for, or a known envelope whose
    /// content this engine does not act on (a `0x12` text command other than
    /// "use skill"). `body` is everything from the id byte on, for a caller
    /// that wants to log it. Per D5 (`docs/protocol_rewrite.md`): a logged
    /// fact, never a silently dropped connection.
    Unknown {
        /// The packet id.
        id: u8,
        /// The whole packet, id byte included.
        body: Vec<u8>,
    },
}

impl ClientPacket {
    /// Decode `packet` by its id byte.
    ///
    /// `packet` must be non-empty — every packet reaching this point has
    /// already survived [`crate::packet::frame_client_packet`], whose
    /// shortest possible frame is one byte, so an empty slice means a caller
    /// skipped framing.
    ///
    /// `Err(_)` only for a recognised id whose body failed to decode; an
    /// unrecognised id, or a recognised envelope carrying content this engine
    /// does not act on, is `Ok(Self::Unknown)`.
    pub fn decode(packet: &[u8], version: ClientVersion) -> Result<Self, ClientDecodeError> {
        let id = *packet
            .first()
            .expect("packet is empty: caller skipped framing, which never produces one");
        match id {
            WalkRequest::ID => decode_packet(packet, version)
                .map(Self::Walk)
                .map_err(ClientDecodeError::Walk),
            LOGOUT_REQUEST_ID => Ok(Self::LogoutRequest),
            ResyncRequest::ID => Ok(Self::ResyncRequest),
            StatusQuery::ID => decode_packet(packet, version)
                .map(Self::StatusQuery)
                .map_err(ClientDecodeError::StatusQuery),
            EncodedCommand::ID => EncodedCommand::decode(packet)
                .map(Self::Encoded)
                .map_err(ClientDecodeError::Encoded),
            GumpResponse::ID => decode_packet(packet, version)
                .map(Self::GumpResponse)
                .map_err(ClientDecodeError::GumpResponse),
            TargetResponse::ID => decode_packet(packet, version)
                .map(Self::TargetResponse)
                .map_err(ClientDecodeError::TargetResponse),
            PickUpItem::ID => decode_packet(packet, version)
                .map(Self::PickUpItem)
                .map_err(ClientDecodeError::PickUpItem),
            DropItem::ID => decode_packet(packet, version)
                .map(Self::DropItem)
                .map_err(ClientDecodeError::DropItem),
            DoubleClick::ID => decode_packet(packet, version)
                .map(Self::DoubleClick)
                .map_err(ClientDecodeError::DoubleClick),
            BuyReply::ID => decode_packet(packet, version)
                .map(Self::Buy)
                .map_err(ClientDecodeError::Buy),
            SellReply::ID => decode_packet(packet, version)
                .map(Self::Sell)
                .map_err(ClientDecodeError::Sell),
            LookRequest::ID => decode_packet(packet, version)
                .map(Self::Look)
                .map_err(ClientDecodeError::Look),
            PropertyQueryRequest::ID => decode_packet(packet, version)
                .map(Self::PropertyQuery)
                .map_err(ClientDecodeError::PropertyQuery),
            EquipItemRequest::ID => decode_packet(packet, version)
                .map(Self::Equip)
                .map_err(ClientDecodeError::Equip),
            WarMode::ID => decode_packet(packet, version)
                .map(Self::WarMode)
                .map_err(ClientDecodeError::WarMode),
            AttackRequest::ID => decode_packet(packet, version)
                .map(Self::Attack)
                .map_err(ClientDecodeError::Attack),
            TalkRequest::ID => decode_packet(packet, version)
                .map(Self::Talk)
                .map_err(ClientDecodeError::Talk),
            UnicodeTalkRequest::ID => decode_packet(packet, version)
                .map(Self::UnicodeTalk)
                .map_err(ClientDecodeError::UnicodeTalk),
            ExtendedRequest::ID => ExtendedRequest::decode(packet)
                .map(Self::Extended)
                .map_err(ClientDecodeError::Extended),
            UseSkillRequest::ID => match UseSkillRequest::decode(packet) {
                Ok(Some(request)) => Ok(Self::UseSkill(request)),
                // A text command that is not "use skill" — an emote, a `go` —
                // is not this engine's business, the same as an id it has no
                // handler for at all.
                Ok(None) => Ok(Self::Unknown {
                    id,
                    body: packet.to_vec(),
                }),
                Err(error) => Err(ClientDecodeError::UseSkill(error)),
            },
            SkillLockRequest::ID => decode_packet(packet, version)
                .map(Self::SkillLock)
                .map_err(ClientDecodeError::SkillLock),
            SecureTradeAction::ID => match SecureTradeAction::decode(packet) {
                Ok(Some(action)) => Ok(Self::SecureTrade(action)),
                // An action byte this end does not know: ServUO's own handler
                // falls through the same way, so this is unhandled, not
                // malformed.
                Ok(None) => Ok(Self::Unknown {
                    id,
                    body: packet.to_vec(),
                }),
                Err(error) => Err(ClientDecodeError::SecureTrade(error)),
            },
            _ => Ok(Self::Unknown {
                id,
                body: packet.to_vec(),
            }),
        }
    }
}

/// A recognised client packet arrived but its body did not decode.
///
/// Fatal in every case dispatch acts on: the world drops the connection
/// rather than act on a half-read command. One variant per packet, the same
/// shape as [`crate::login::ClientLoginDecodeError`], so a caller can match
/// the failure by type the same way it would match [`ClientPacket`] itself.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ClientDecodeError {
    /// `0x02` did not decode.
    Walk(DecodeError),
    /// `0x34` did not decode.
    StatusQuery(DecodeError),
    /// `0xD7` did not decode.
    Encoded(DecodeError),
    /// `0xB1` did not decode.
    GumpResponse(DecodeError),
    /// `0x6C` did not decode.
    TargetResponse(DecodeError),
    /// `0x07` did not decode.
    PickUpItem(DecodeError),
    /// `0x08` did not decode.
    DropItem(DecodeError),
    /// `0x06` did not decode.
    DoubleClick(DecodeError),
    /// `0x3B` did not decode.
    Buy(DecodeError),
    /// `0x9F` did not decode.
    Sell(DecodeError),
    /// `0x09` did not decode.
    Look(DecodeError),
    /// `0xD6` did not decode.
    PropertyQuery(DecodeError),
    /// `0x13` did not decode.
    Equip(DecodeError),
    /// `0x72` did not decode.
    WarMode(DecodeError),
    /// `0x05` did not decode.
    Attack(DecodeError),
    /// `0x03` did not decode.
    Talk(DecodeError),
    /// `0xAD` did not decode.
    UnicodeTalk(DecodeError),
    /// `0xBF` did not decode.
    Extended(DecodeError),
    /// `0x12` did not decode.
    UseSkill(DecodeError),
    /// `0x3A` did not decode.
    SkillLock(DecodeError),
    /// `0x6F` did not decode.
    SecureTrade(DecodeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::encode_packet;

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    #[test]
    fn a_walk_request_decodes_by_id() {
        let bytes = [0x02, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
        let packet = ClientPacket::decode(&bytes, version()).unwrap();
        assert!(matches!(packet, ClientPacket::Walk(_)));
    }

    #[test]
    fn logout_carries_no_body() {
        let packet = ClientPacket::decode(&[0xD1, 0x01], version()).unwrap();
        assert_eq!(packet, ClientPacket::LogoutRequest);
    }

    #[test]
    fn an_unknown_id_is_not_an_error() {
        let bytes = [0xF3, 0x00, 0x00];
        let packet = ClientPacket::decode(&bytes, version()).unwrap();
        assert_eq!(
            packet,
            ClientPacket::Unknown {
                id: 0xF3,
                body: bytes.to_vec()
            }
        );
    }

    /// The one id that means two unrelated things depending on which way it is
    /// travelling. Three bytes each way, and nothing in the body to tell them
    /// apart — this test is here so that a reader who finds `0x22` in the server
    /// packets is told, by a failing name, that they are not the same packet.
    #[test]
    fn twenty_two_from_the_client_is_a_resync_and_not_a_walk_ack() {
        let packet = ClientPacket::decode(&[0x22, 0x00, 0x00], version()).unwrap();
        assert_eq!(packet, ClientPacket::ResyncRequest);
    }

    #[test]
    fn an_unrecognised_text_command_is_unknown_not_an_error() {
        // 0x12 type 0x33 is not "use skill" (0x24) — the client's `go`, say.
        let mut bytes = vec![0x12u8, 0x00, 0x00, 0x33];
        bytes.extend_from_slice(b"north\0");
        let length = u16::try_from(bytes.len()).unwrap();
        bytes[1..3].copy_from_slice(&length.to_be_bytes());

        let packet = ClientPacket::decode(&bytes, version()).unwrap();
        assert_eq!(
            packet,
            ClientPacket::Unknown {
                id: 0x12,
                body: bytes.clone()
            }
        );
    }

    #[test]
    fn a_malformed_known_id_is_an_error() {
        // 0x02 (walk) is Fixed(7); three bytes never decodes.
        let bytes = [0x02, 0, 0];
        assert!(matches!(
            ClientPacket::decode(&bytes, version()),
            Err(ClientDecodeError::Walk(_))
        ));
    }

    #[test]
    fn character_select_is_not_a_world_packet() {
        // `0x5D` is the character screen's, and it is decoded by
        // `LoginStagePacket` — see its `PlayCharacter`. Reaching this decoder at
        // all means the split in `RawPacket::parse_packet` was bypassed, so what
        // comes back is an id this half has no handler for rather than a packet
        // the world's dispatcher could be handed. That is the property the arm
        // this replaced could only state in an `unreachable!` two crates away.
        let bytes = [0x5Du8; 73];
        assert!(matches!(
            ClientPacket::decode(&bytes, version()),
            Ok(ClientPacket::Unknown { id: 0x5D, .. })
        ));
    }

    #[test]
    fn an_extended_command_round_trips_by_subcommand() {
        let packet = encode_packet(
            &crate::context::ContextMenu {
                serial: crate::serial::Serial::new(1).unwrap(),
                entries: Vec::new(),
            },
            version(),
        );
        let decoded = ClientPacket::decode(&packet, version());
        // ContextMenu (0x14) is outbound-only; as a client packet its
        // subcommand is simply one this engine does not act on.
        assert!(matches!(
            decoded,
            Ok(ClientPacket::Extended(ExtendedRequest::Unknown(0x14)))
        ));
    }
}
