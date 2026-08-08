//! Everything the server can say: one sum type.
//!
//! # Why an enum and not forty-seven functions
//!
//! The wire format is a fixed external contract with a closed set of messages.
//! A set of free `encode_*` functions returning `Vec<u8>` cannot say that: it
//! cannot be matched over, cannot be logged uniformly, cannot tell you that an
//! id is produced by two encoders that disagree about its length, and gives a
//! test no way to name "the packet the server sent" other than by its bytes.
//!
//! So the closed set is a type. Each variant wraps a payload struct that knows
//! its id, its length and how to write its body — see
//! [`EncodePacket`](crate::packet::EncodePacket) — and this enum is the only
//! thing that turns one into bytes.
//!
//! # It grows one group at a time
//!
//! Non-exhaustive and deliberately incomplete: the packets that have been
//! rewritten are here, the rest are still free functions elsewhere in the crate.
//! `docs/protocol_rewrite.md` tracks which group lands when.

use std::fmt;

use crate::codec::PacketWriter;
use crate::combat::{AttackTarget, HealthBar, WarMode};
use crate::containers::{
    AddToContainer, ContainerContents, OpenContainer, add_to_container_length, open_container_length,
};
use crate::context::ContextMenu;
use crate::error::{DecodeError, expect_id};
use crate::feature::Feature;
use crate::feedback::{Animation, GraphicalEffect, HuedEffect, NewAnimation, PlaySound};
use crate::gump::{CloseGump, GumpDisplay};
use crate::items::{DragCancel, EquipUpdate, WorldItem};
use crate::login::{
    CharacterList, CharacterListUpdate, DeleteReject, LoginDenied, Relay, ShardList,
    supported_features_length,
};
use crate::mobile::{MobileIncoming, MobileMove, MobileStatus, OpenPaperdoll, Remove, StatLocks};
use crate::packet::{DecodePacket, EncodePacket, Frame, FrameError, PacketLength, frame_body, frame_packet};
use crate::properties::TooltipRevision;
use crate::skill::{SkillUpdate, SkillsFull};
use crate::speech::{LocalizedMessage, SpokenMessage, UnicodeMessage};
use crate::spellbook::SpellbookContent;
use crate::target::TargetCursor;
use crate::vendor::{BuyList, SellList};
use crate::version::ClientVersion;
use crate::world::{
    DeathStatus, LightLevel, LoginComplete, LogoutAck, MapChange, PlayMusic, PlayerStart, PlayerUpdate,
    SERVER_CHANGE_LENGTH, SeasonChange, WalkAck, WalkReject,
};

/// A packet the server sends to a client.
///
/// Not `Copy`: the login group carries `Vec` payloads (the shard and character
/// lists), unlike Stage 1's fixed-size ones.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ServerPacket {
    /// `0x6C` — raise a targeting cursor.
    TargetCursor(TargetCursor),
    /// `0x72` — the settled war stance.
    WarMode(WarMode),
    /// `0xAA` — which mobile's bar the client highlights.
    AttackTarget(AttackTarget),
    /// `0xA1` — a mobile's health bar.
    Health(HealthBar),
    /// `0x54` — a sound at a world location.
    PlaySound(PlaySound),
    /// `0x6E` — the classic mobile animation.
    Animation(Animation),
    /// `0xE2` — the 7.0.0.0+ mobile animation.
    NewAnimation(NewAnimation),
    /// `0x70` — an uncoloured graphical effect.
    Effect(GraphicalEffect),
    /// `0xC0` — a graphical effect with a hue and a render mode.
    HuedEffect(HuedEffect),
    /// `0x82` — refuse a login.
    LoginDenied(LoginDenied),
    /// `0xA8` — the shard list.
    ShardList(ShardList),
    /// `0x8C` — go connect to the game server.
    Relay(Relay),
    /// `0xA9` — the character list and starting cities.
    CharacterList(CharacterList),
    /// `0x85` — a character deletion was refused.
    DeleteReject(DeleteReject),
    /// `0x86` — resend the character list after a deletion.
    CharacterListUpdate(CharacterListUpdate),
    /// `0x1B` — put a body in the world.
    PlayerStart(PlayerStart),
    /// `0x20` — move or redraw the player's own body.
    PlayerUpdate(PlayerUpdate),
    /// `0x2C` — the player's own character died, or came back.
    DeathStatus(DeathStatus),
    /// `0x22` — a walk request is allowed.
    WalkAck(WalkAck),
    /// `0x21` — a walk request is refused.
    WalkReject(WalkReject),
    /// `0x55` — the client may start drawing.
    LoginComplete(LoginComplete),
    /// `0x4F` — overall light level.
    LightLevel(LightLevel),
    /// `0x6D` — play a music track.
    PlayMusic(PlayMusic),
    /// `0xBC` — which season the client draws.
    SeasonChange(SeasonChange),
    /// `0xD1` — a logout is granted.
    LogoutAck(LogoutAck),
    /// `0xBF` subcommand `0x08` — which map the client should draw.
    MapChange(MapChange),
    /// `0x1D` — take an object off the client's screen.
    Remove(Remove),
    /// `0x88` — open a mobile's paperdoll.
    OpenPaperdoll(OpenPaperdoll),
    /// `0x11` — a mobile's full status.
    MobileStatus(MobileStatus),
    /// `0x77` — move a mobile the client already knows about.
    MobileMove(MobileMove),
    /// `0x78` — draw a mobile the client has not seen.
    MobileIncoming(MobileIncoming),
    /// `0xBF` subcommand `0x19` type `2` — the three stat-training arrows.
    StatLocks(StatLocks),
    /// `0x1A` — draw an item on the ground the client has not seen.
    WorldItem(WorldItem),
    /// `0x27` — cancel a drag and bounce the item back.
    DragCancel(DragCancel),
    /// `0x2E` — a mobile is now wearing an item.
    EquipUpdate(EquipUpdate),
    /// `0x24` — open a container's gump window.
    OpenContainer(OpenContainer),
    /// `0x25` — one more item inside a container gump already open.
    AddToContainer(AddToContainer),
    /// `0x3C` — the full contents of a container, all at once.
    ContainerContents(ContainerContents),
    /// `0x74` — the prices and labels for a vendor's buy container.
    BuyList(BuyList),
    /// `0x9E` — what a vendor offers to buy from the player.
    SellList(SellList),
    /// `0xDC` — the tooltip revision for one object.
    TooltipRevision(TooltipRevision),
    /// `0x3A` — the whole skill list, to fill the window.
    SkillsFull(SkillsFull),
    /// `0x3A` — one skill's line, following a change.
    SkillUpdate(SkillUpdate),
    /// `0x1C` — speech drawn over a source and put in the journal.
    SpokenMessage(SpokenMessage),
    /// `0xC1` — a localized message: a cliloc and its substitutions.
    LocalizedMessage(LocalizedMessage),
    /// `0xAE` — Unicode speech drawn over a source.
    UnicodeMessage(UnicodeMessage),
    /// `0xBF` subcommand `0x14` — a context menu on an object.
    ContextMenu(ContextMenu),
    /// `0xBF` subcommand `0x1B` — the spells a spellbook holds.
    SpellbookContent(SpellbookContent),
    /// `0xBF` subcommand `0x04` — close an open gump on the client.
    CloseGump(CloseGump),
    /// `0xB0` — display a generic gump.
    GumpDisplay(GumpDisplay),
}

impl ServerPacket {
    /// The id byte this packet goes out under.
    ///
    /// Taken from the payload's own [`EncodePacket::ID`], so there is no second
    /// table to keep in step — and it makes an id available for logging without
    /// encoding anything.
    #[must_use]
    pub const fn id(&self) -> u8 {
        match self {
            Self::TargetCursor(_) => TargetCursor::ID,
            Self::WarMode(_) => <WarMode as EncodePacket>::ID,
            Self::AttackTarget(_) => AttackTarget::ID,
            Self::Health(_) => HealthBar::ID,
            Self::PlaySound(_) => PlaySound::ID,
            Self::Animation(_) => Animation::ID,
            Self::NewAnimation(_) => NewAnimation::ID,
            Self::Effect(_) => GraphicalEffect::ID,
            Self::HuedEffect(_) => HuedEffect::ID,
            Self::LoginDenied(_) => <LoginDenied as EncodePacket>::ID,
            Self::ShardList(_) => <ShardList as EncodePacket>::ID,
            Self::Relay(_) => <Relay as EncodePacket>::ID,
            Self::CharacterList(_) => <CharacterList as EncodePacket>::ID,
            Self::DeleteReject(_) => DeleteReject::ID,
            Self::CharacterListUpdate(_) => CharacterListUpdate::ID,
            Self::PlayerStart(_) => <PlayerStart as EncodePacket>::ID,
            Self::PlayerUpdate(_) => <PlayerUpdate as EncodePacket>::ID,
            Self::DeathStatus(_) => DeathStatus::ID,
            Self::WalkAck(_) => <WalkAck as EncodePacket>::ID,
            Self::WalkReject(_) => <WalkReject as EncodePacket>::ID,
            Self::LoginComplete(_) => <LoginComplete as EncodePacket>::ID,
            Self::LightLevel(_) => LightLevel::ID,
            Self::PlayMusic(_) => PlayMusic::ID,
            Self::SeasonChange(_) => SeasonChange::ID,
            Self::LogoutAck(_) => LogoutAck::ID,
            Self::MapChange(_) => MapChange::ID,
            Self::Remove(_) => <Remove as EncodePacket>::ID,
            Self::OpenPaperdoll(_) => <OpenPaperdoll as EncodePacket>::ID,
            Self::MobileStatus(_) => <MobileStatus as EncodePacket>::ID,
            Self::MobileMove(_) => <MobileMove as EncodePacket>::ID,
            Self::MobileIncoming(_) => <MobileIncoming as EncodePacket>::ID,
            Self::StatLocks(_) => StatLocks::ID,
            Self::WorldItem(_) => <WorldItem as EncodePacket>::ID,
            Self::DragCancel(_) => DragCancel::ID,
            Self::EquipUpdate(_) => EquipUpdate::ID,
            Self::OpenContainer(_) => <OpenContainer as DecodePacket>::ID,
            Self::AddToContainer(_) => <AddToContainer as DecodePacket>::ID,
            Self::ContainerContents(_) => <ContainerContents as EncodePacket>::ID,
            Self::BuyList(_) => BuyList::ID,
            Self::SellList(_) => SellList::ID,
            Self::TooltipRevision(_) => TooltipRevision::ID,
            Self::SkillsFull(_) => SkillsFull::ID,
            Self::SkillUpdate(_) => SkillUpdate::ID,
            Self::SpokenMessage(_) => <SpokenMessage as EncodePacket>::ID,
            Self::LocalizedMessage(_) => LocalizedMessage::ID,
            Self::UnicodeMessage(_) => <UnicodeMessage as EncodePacket>::ID,
            Self::ContextMenu(_) => ContextMenu::ID,
            Self::SpellbookContent(_) => SpellbookContent::ID,
            Self::CloseGump(_) => CloseGump::ID,
            Self::GumpDisplay(_) => <GumpDisplay as EncodePacket>::ID,
        }
    }

    /// How the packet is framed: a fixed size, or a length field to patch.
    ///
    /// Takes the version because two packets cannot answer without it: `0x24`
    /// grows by the High Seas container type and `0x25` by the grid byte. That
    /// is also why neither of them is an
    /// [`EncodePacket`](crate::packet::EncodePacket) — its `LENGTH` is a `const`
    /// with nothing to ask — and why this is not a `const fn`.
    #[must_use]
    pub fn length(&self, version: ClientVersion) -> PacketLength {
        match self {
            Self::TargetCursor(_) => TargetCursor::LENGTH,
            Self::WarMode(_) => <WarMode as EncodePacket>::LENGTH,
            Self::AttackTarget(_) => AttackTarget::LENGTH,
            Self::Health(_) => HealthBar::LENGTH,
            Self::PlaySound(_) => PlaySound::LENGTH,
            Self::Animation(_) => Animation::LENGTH,
            Self::NewAnimation(_) => NewAnimation::LENGTH,
            Self::Effect(_) => GraphicalEffect::LENGTH,
            Self::HuedEffect(_) => HuedEffect::LENGTH,
            Self::LoginDenied(_) => <LoginDenied as EncodePacket>::LENGTH,
            Self::ShardList(_) => <ShardList as EncodePacket>::LENGTH,
            Self::Relay(_) => <Relay as EncodePacket>::LENGTH,
            Self::CharacterList(_) => <CharacterList as EncodePacket>::LENGTH,
            Self::DeleteReject(_) => DeleteReject::LENGTH,
            Self::CharacterListUpdate(_) => CharacterListUpdate::LENGTH,
            Self::PlayerStart(_) => <PlayerStart as EncodePacket>::LENGTH,
            Self::PlayerUpdate(_) => PlayerUpdate::LENGTH,
            Self::DeathStatus(_) => DeathStatus::LENGTH,
            Self::WalkAck(_) => WalkAck::LENGTH,
            Self::WalkReject(_) => WalkReject::LENGTH,
            Self::LoginComplete(_) => <LoginComplete as EncodePacket>::LENGTH,
            Self::LightLevel(_) => LightLevel::LENGTH,
            Self::PlayMusic(_) => PlayMusic::LENGTH,
            Self::SeasonChange(_) => SeasonChange::LENGTH,
            Self::LogoutAck(_) => LogoutAck::LENGTH,
            Self::MapChange(_) => MapChange::LENGTH,
            Self::Remove(_) => Remove::LENGTH,
            Self::OpenPaperdoll(_) => <OpenPaperdoll as EncodePacket>::LENGTH,
            Self::MobileStatus(_) => MobileStatus::LENGTH,
            Self::MobileMove(_) => MobileMove::LENGTH,
            Self::MobileIncoming(_) => MobileIncoming::LENGTH,
            Self::StatLocks(_) => StatLocks::LENGTH,
            Self::WorldItem(_) => WorldItem::LENGTH,
            Self::DragCancel(_) => DragCancel::LENGTH,
            Self::EquipUpdate(_) => EquipUpdate::LENGTH,
            Self::OpenContainer(_) => open_container_length(version),
            Self::AddToContainer(_) => add_to_container_length(version),
            Self::ContainerContents(_) => <ContainerContents as EncodePacket>::LENGTH,
            Self::BuyList(_) => BuyList::LENGTH,
            Self::SellList(_) => SellList::LENGTH,
            Self::TooltipRevision(_) => TooltipRevision::LENGTH,
            Self::SkillsFull(_) => SkillsFull::LENGTH,
            Self::SkillUpdate(_) => SkillUpdate::LENGTH,
            Self::SpokenMessage(_) => SpokenMessage::LENGTH,
            Self::LocalizedMessage(_) => LocalizedMessage::LENGTH,
            Self::UnicodeMessage(_) => UnicodeMessage::LENGTH,
            Self::ContextMenu(_) => ContextMenu::LENGTH,
            Self::SpellbookContent(_) => SpellbookContent::LENGTH,
            Self::CloseGump(_) => CloseGump::LENGTH,
            Self::GumpDisplay(_) => GumpDisplay::LENGTH,
        }
    }

    /// The bytes to put on the wire, framed for `version`.
    ///
    /// The header — id, and the length field where there is one — is written by
    /// [`frame_body`] and by nothing else, so no payload can forget it.
    #[must_use]
    pub fn encode(&self, version: ClientVersion) -> Vec<u8> {
        frame_body(self.id(), self.length(version), |out| {
            self.encode_body(out, version);
        })
    }

    /// Dispatch the body write to the payload.
    fn encode_body(&self, out: &mut PacketWriter, version: ClientVersion) {
        match self {
            Self::TargetCursor(packet) => packet.encode_body(out, version),
            Self::WarMode(packet) => packet.encode_body(out, version),
            Self::AttackTarget(packet) => packet.encode_body(out, version),
            Self::Health(packet) => packet.encode_body(out, version),
            Self::PlaySound(packet) => packet.encode_body(out, version),
            Self::Animation(packet) => packet.encode_body(out, version),
            Self::NewAnimation(packet) => packet.encode_body(out, version),
            Self::Effect(packet) => packet.encode_body(out, version),
            Self::HuedEffect(packet) => packet.encode_body(out, version),
            Self::LoginDenied(packet) => packet.encode_body(out, version),
            Self::ShardList(packet) => packet.encode_body(out, version),
            Self::Relay(packet) => packet.encode_body(out, version),
            Self::CharacterList(packet) => packet.encode_body(out, version),
            Self::DeleteReject(packet) => packet.encode_body(out, version),
            Self::CharacterListUpdate(packet) => packet.encode_body(out, version),
            Self::PlayerStart(packet) => packet.encode_body(out, version),
            Self::PlayerUpdate(packet) => packet.encode_body(out, version),
            Self::DeathStatus(packet) => packet.encode_body(out, version),
            Self::WalkAck(packet) => packet.encode_body(out, version),
            Self::WalkReject(packet) => packet.encode_body(out, version),
            Self::LoginComplete(packet) => packet.encode_body(out, version),
            Self::LightLevel(packet) => packet.encode_body(out, version),
            Self::PlayMusic(packet) => packet.encode_body(out, version),
            Self::SeasonChange(packet) => packet.encode_body(out, version),
            Self::LogoutAck(packet) => packet.encode_body(out, version),
            Self::MapChange(packet) => packet.encode_body(out, version),
            Self::Remove(packet) => packet.encode_body(out, version),
            Self::OpenPaperdoll(packet) => packet.encode_body(out, version),
            Self::MobileStatus(packet) => packet.encode_body(out, version),
            Self::MobileMove(packet) => packet.encode_body(out, version),
            Self::MobileIncoming(packet) => packet.encode_body(out, version),
            Self::StatLocks(packet) => packet.encode_body(out, version),
            Self::WorldItem(packet) => packet.encode_body(out, version),
            Self::DragCancel(packet) => packet.encode_body(out, version),
            Self::EquipUpdate(packet) => packet.encode_body(out, version),
            Self::OpenContainer(packet) => packet.write_body(out, version),
            Self::AddToContainer(packet) => packet.write_body(out, version),
            Self::ContainerContents(packet) => packet.encode_body(out, version),
            Self::BuyList(packet) => packet.encode_body(out, version),
            Self::SellList(packet) => packet.encode_body(out, version),
            Self::TooltipRevision(packet) => packet.encode_body(out, version),
            Self::SkillsFull(packet) => packet.encode_body(out, version),
            Self::SkillUpdate(packet) => packet.encode_body(out, version),
            Self::SpokenMessage(packet) => packet.encode_body(out, version),
            Self::LocalizedMessage(packet) => packet.encode_body(out, version),
            Self::UnicodeMessage(packet) => packet.encode_body(out, version),
            Self::ContextMenu(packet) => packet.encode_body(out, version),
            Self::SpellbookContent(packet) => packet.encode_body(out, version),
            Self::CloseGump(packet) => packet.encode_body(out, version),
            Self::GumpDisplay(packet) => packet.encode_body(out, version),
        }
    }
}

// -- reading, from the client's side --------------------------------------

/// Decode one server-to-client payload from a framed packet.
///
/// [`decode_packet`](crate::packet::decode_packet) asks the *client* length
/// table whether to skip a length field, which is the right question on the
/// server and the wrong one here: `0xA9` is variable in this direction and
/// unknown in that one. Same shape, other table.
fn decode_server<P: DecodePacket>(bytes: &[u8], version: ClientVersion) -> Result<P, DecodeError> {
    let mut reader = expect_id(bytes, P::ID)?;
    if server_packet_length(P::ID, version) == Some(PacketLength::Variable) {
        reader.skip(2)?;
    }
    P::decode_body(&mut reader, version)
}

impl ServerPacket {
    /// Decode `packet` by its id byte, as a client does.
    ///
    /// `packet` must be non-empty and must already have passed
    /// [`frame_server_packet`], which is what guarantees it is exactly one
    /// packet.
    ///
    /// # Three answers, not two
    ///
    /// - `Ok(Some(_))` — decoded.
    /// - `Ok(None)` — a packet this engine sends but has no decoder for yet.
    ///   The framer knew its length, so the stream is not lost and the caller
    ///   can log the id and read on. This is where the client grows: a variant
    ///   moves from here to `Some` when someone writes its `DecodePacket`.
    /// - `Err(_)` — a decoder ran and the body was not what it claimed.
    ///
    /// The middle answer is deliberately not the `Unknown { id, body }` variant
    /// [`ClientPacket`](crate::client_packet::ClientPacket) uses. That one
    /// carries the bytes because the *server* discards them otherwise; a client
    /// still holds the buffer it just framed.
    pub fn decode(packet: &[u8], version: ClientVersion) -> Result<Option<Self>, ServerDecodeError> {
        let id = *packet
            .first()
            .expect("packet is empty: caller skipped framing, which never produces one");
        let decoded = match id {
            <LoginDenied as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::LoginDenied)
                .map_err(ServerDecodeError::LoginDenied)?,
            <ShardList as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::ShardList)
                .map_err(ServerDecodeError::ShardList)?,
            <Relay as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::Relay)
                .map_err(ServerDecodeError::Relay)?,
            <CharacterList as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::CharacterList)
                .map_err(ServerDecodeError::CharacterList)?,
            <PlayerStart as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::PlayerStart)
                .map_err(ServerDecodeError::PlayerStart)?,
            <LoginComplete as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::LoginComplete)
                .map_err(ServerDecodeError::LoginComplete)?,
            <Remove as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::Remove)
                .map_err(ServerDecodeError::Remove)?,
            <PlayerUpdate as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::PlayerUpdate)
                .map_err(ServerDecodeError::PlayerUpdate)?,
            <MobileStatus as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::MobileStatus)
                .map_err(ServerDecodeError::MobileStatus)?,
            <MobileMove as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::MobileMove)
                .map_err(ServerDecodeError::MobileMove)?,
            <MobileIncoming as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::MobileIncoming)
                .map_err(ServerDecodeError::MobileIncoming)?,
            <WorldItem as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::WorldItem)
                .map_err(ServerDecodeError::WorldItem)?,
            <WalkAck as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::WalkAck)
                .map_err(ServerDecodeError::WalkAck)?,
            <WalkReject as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::WalkReject)
                .map_err(ServerDecodeError::WalkReject)?,
            <SpokenMessage as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::SpokenMessage)
                .map_err(ServerDecodeError::SpokenMessage)?,
            <UnicodeMessage as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::UnicodeMessage)
                .map_err(ServerDecodeError::UnicodeMessage)?,
            <GumpDisplay as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::GumpDisplay)
                .map_err(ServerDecodeError::GumpDisplay)?,
            <OpenContainer as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::OpenContainer)
                .map_err(ServerDecodeError::OpenContainer)?,
            <AddToContainer as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::AddToContainer)
                .map_err(ServerDecodeError::AddToContainer)?,
            <ContainerContents as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::ContainerContents)
                .map_err(ServerDecodeError::ContainerContents)?,
            <OpenPaperdoll as DecodePacket>::ID => decode_server(packet, version)
                .map(Self::OpenPaperdoll)
                .map_err(ServerDecodeError::OpenPaperdoll)?,
            _ => return Ok(None),
        };
        Ok(Some(decoded))
    }
}

/// A server packet arrived and its body did not decode.
///
/// One variant per packet, the same shape as
/// [`ClientDecodeError`](crate::client_packet::ClientDecodeError): a caller can
/// match the failure by type the way it matches the packet.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ServerDecodeError {
    /// `0x82` did not decode.
    LoginDenied(DecodeError),
    /// `0xA8` did not decode.
    ShardList(DecodeError),
    /// `0x8C` did not decode.
    Relay(DecodeError),
    /// `0xA9` did not decode.
    CharacterList(DecodeError),
    /// `0x1B` did not decode.
    PlayerStart(DecodeError),
    /// `0x55` did not decode.
    LoginComplete(DecodeError),
    /// `0x1D` did not decode.
    Remove(DecodeError),
    /// `0x20` did not decode.
    PlayerUpdate(DecodeError),
    /// `0x11` did not decode.
    MobileStatus(DecodeError),
    /// `0x77` did not decode.
    MobileMove(DecodeError),
    /// `0x78` did not decode.
    MobileIncoming(DecodeError),
    /// `0x1A` did not decode.
    WorldItem(DecodeError),
    /// `0x22` did not decode.
    WalkAck(DecodeError),
    /// `0x21` did not decode.
    WalkReject(DecodeError),
    /// `0x1C` did not decode.
    SpokenMessage(DecodeError),
    /// `0xAE` did not decode.
    UnicodeMessage(DecodeError),
    /// `0xB0` did not decode.
    GumpDisplay(DecodeError),
    /// `0x24` did not decode.
    OpenContainer(DecodeError),
    /// `0x25` did not decode.
    AddToContainer(DecodeError),
    /// `0x3C` did not decode.
    ContainerContents(DecodeError),
    /// `0x88` did not decode.
    OpenPaperdoll(DecodeError),
}

impl fmt::Display for ServerDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (name, error) = match self {
            Self::LoginDenied(error) => ("0x82 login denied", error),
            Self::ShardList(error) => ("0xA8 shard list", error),
            Self::Relay(error) => ("0x8C relay", error),
            Self::CharacterList(error) => ("0xA9 character list", error),
            Self::PlayerStart(error) => ("0x1B player start", error),
            Self::LoginComplete(error) => ("0x55 login complete", error),
            Self::Remove(error) => ("0x1D remove", error),
            Self::PlayerUpdate(error) => ("0x20 player update", error),
            Self::MobileStatus(error) => ("0x11 mobile status", error),
            Self::MobileMove(error) => ("0x77 mobile move", error),
            Self::MobileIncoming(error) => ("0x78 mobile incoming", error),
            Self::WorldItem(error) => ("0x1A world item", error),
            Self::WalkAck(error) => ("0x22 walk ack", error),
            Self::WalkReject(error) => ("0x21 walk reject", error),
            Self::SpokenMessage(error) => ("0x1C spoken message", error),
            Self::UnicodeMessage(error) => ("0xAE unicode message", error),
            Self::GumpDisplay(error) => ("0xB0 gump display", error),
            Self::OpenContainer(error) => ("0x24 open container", error),
            Self::AddToContainer(error) => ("0x25 add to container", error),
            Self::ContainerContents(error) => ("0x3C container contents", error),
            Self::OpenPaperdoll(error) => ("0x88 paperdoll", error),
        };
        write!(f, "{name}: {error}")
    }
}

impl std::error::Error for ServerDecodeError {}

// -- framing, from the client's side --------------------------------------

/// How long the server-to-client packet with this id is, if we send it.
///
/// The mirror of [`client_packet_length`](crate::packet::client_packet_length),
/// and the table a client needs for the same reason a server needs that one:
/// the wire has no self-describing frame, so a stream cannot be split into
/// packets without knowing, per id, which kind it is.
///
/// # The numbers are not written here
///
/// Every payload already declares its own [`EncodePacket::LENGTH`], so this
/// reads those constants rather than repeating them — the same argument as
/// [`ServerPacket::id`]. What it does hold is the *ids*, and a wrong one is
/// caught by `every_packet_frames_to_its_own_length`, which frames the real
/// bytes of one of every variant.
///
/// # `None` is deliberate, not incomplete
///
/// An id this engine never sends has no entry, and framing it is fatal for the
/// connection. The alternative — guessing a length from a reference for a packet
/// no encoder here writes — would put an unverified number in the one table
/// whose entire job is to be right, and would be discovered as a desynchronised
/// stream hundreds of bytes later. When a packet is added, its length arrives
/// with it.
///
/// # Why `version` is not optional
///
/// The server takes `Option<ClientVersion>` because it frames packets before it
/// knows what is connected. A client always knows what it is, so there is no
/// unknown state to model — and three packets need the answer: `0x24`, `0x25`
/// and `0xB9` are fixed-length in a size that depends on the client, which is
/// exactly why they are hand-written free functions rather than `EncodePacket`s.
// The column alignment is load-bearing, as in the client table: this is read as
// a table, and rustfmt would reflow it into an unscannable list.
#[rustfmt::skip]
#[must_use]
pub fn server_packet_length(id: u8, version: ClientVersion) -> Option<PacketLength> {
    use PacketLength::Variable;

    // The three whose size is a function of the client. Each rule lives beside
    // the encoder that obeys it, so neither side can drift.
    match id {
        0x24 => return Some(open_container_length(version)),
        0x25 => return Some(add_to_container_length(version)),
        0xB9 => return Some(supported_features_length(
            version.supports(Feature::ExtraFeatureMask),
        )),
        _ => {}
    }

    Some(match id {
        0x11 => MobileStatus::LENGTH,
        0x1A => WorldItem::LENGTH,
        0x1B => <PlayerStart as EncodePacket>::LENGTH,
        0x1C => SpokenMessage::LENGTH,
        0x1D => Remove::LENGTH,
        0x20 => PlayerUpdate::LENGTH,
        0x21 => WalkReject::LENGTH,
        0x22 => WalkAck::LENGTH,
        0x27 => DragCancel::LENGTH,
        0x2C => DeathStatus::LENGTH,
        0x2E => EquipUpdate::LENGTH,
        0x3A => SkillsFull::LENGTH,          // and SkillUpdate: same id, both Variable
        0x3C => <ContainerContents as EncodePacket>::LENGTH,
        0x4F => LightLevel::LENGTH,
        0x54 => PlaySound::LENGTH,
        0x55 => <LoginComplete as EncodePacket>::LENGTH,
        0x6C => TargetCursor::LENGTH,
        0x6D => PlayMusic::LENGTH,
        0x6E => Animation::LENGTH,
        0x6F => Variable,                    // secure trade, hand-written in trade.rs
        0x70 => GraphicalEffect::LENGTH,
        0x72 => <WarMode as EncodePacket>::LENGTH,
        0x74 => BuyList::LENGTH,
        0x76 => SERVER_CHANGE_LENGTH,        // facet change, hand-written in world.rs
        0x77 => MobileMove::LENGTH,
        0x78 => MobileIncoming::LENGTH,
        0x82 => <LoginDenied as EncodePacket>::LENGTH,
        0x85 => DeleteReject::LENGTH,
        0x86 => CharacterListUpdate::LENGTH,
        0x88 => <OpenPaperdoll as EncodePacket>::LENGTH,
        0x8C => <Relay as EncodePacket>::LENGTH,
        0x9E => SellList::LENGTH,
        0xA1 => HealthBar::LENGTH,
        0xA8 => <ShardList as EncodePacket>::LENGTH,
        0xA9 => <CharacterList as EncodePacket>::LENGTH,
        0xAA => AttackTarget::LENGTH,
        0xAE => UnicodeMessage::LENGTH,
        0xB0 => GumpDisplay::LENGTH,
        0xBC => SeasonChange::LENGTH,
        // 0xBF is the one id whose payloads disagree with the table on purpose.
        // Each subcommand declares `Fixed(n)` and writes its own `u16` length
        // into its body — that is what the extended-command format is — so from
        // outside, every 0xBF on the wire is length-prefixed at offset 1 and is
        // framed as `Variable`. Reading MapChange's `Fixed(6)` here would frame
        // a 13-byte gump-close as six bytes and desynchronise everything after.
        0xBF => Variable,
        0xC0 => HuedEffect::LENGTH,
        0xC1 => LocalizedMessage::LENGTH,
        0xD1 => LogoutAck::LENGTH,
        0xDC => TooltipRevision::LENGTH,
        0xE2 => NewAnimation::LENGTH,
        _ => return None,
    })
}

/// Find the first whole server-to-client packet at the front of `buffer`.
///
/// The client's [`frame_client_packet`](crate::packet::frame_client_packet):
/// same rule, other table. Does not copy and does not consume.
///
/// ```
/// use openshard_protocol::packet::Frame;
/// use openshard_protocol::server_packet::frame_server_packet;
/// use openshard_protocol::version::ClientVersion;
///
/// let modern = ClientVersion::new(7, 0, 45, 65);
///
/// // 0x55 "you may start drawing" is one byte.
/// assert_eq!(frame_server_packet(&[0x55], modern), Ok(Frame::Complete(1)));
///
/// // 0xB9 is three bytes for an old client and five for this one.
/// assert_eq!(
///     frame_server_packet(&[0xB9, 0, 0, 0], modern),
///     Ok(Frame::Incomplete { needed: 5 }),
/// );
/// ```
pub fn frame_server_packet(buffer: &[u8], version: ClientVersion) -> Result<Frame, FrameError> {
    frame_packet(buffer, |id| server_packet_length(id, version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::encode_packet;
    use crate::serial::Serial;
    use crate::target::TargetKind;
    use crate::wire::{AuthKey, CursorId};

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    /// One of every variant, so a new variant that lies about its id or length
    /// has to be added here to compile.
    fn one_of_each() -> Vec<ServerPacket> {
        let serial = Serial::new(0x0000_002A).unwrap();
        let effect = GraphicalEffect {
            kind: crate::feedback::EffectKind::Moving,
            from: Some(serial),
            to: None,
            art: crate::wire::Graphic(0x36D4),
            from_point: crate::world::Point::new(1, 2, 3),
            to_point: crate::world::Point::new(4, 5, 6),
            speed: 7,
            duration: 0,
            fixed_direction: false,
            explode: false,
        };
        vec![
            ServerPacket::TargetCursor(TargetCursor {
                cursor_id: CursorId(1),
                kind: TargetKind::Object,
            }),
            ServerPacket::WarMode(WarMode { war: true }),
            ServerPacket::AttackTarget(AttackTarget { target: Some(serial) }),
            ServerPacket::Health(HealthBar::exact(serial, 100, 50)),
            ServerPacket::PlaySound(PlaySound {
                sound: crate::wire::SoundId(0x28),
                at: crate::world::Point::new(1, 2, 3),
            }),
            ServerPacket::Animation(Animation {
                serial,
                action: 1,
                frame_count: 5,
                repeat_count: 1,
                forward: true,
                repeat: false,
                delay: 0,
            }),
            ServerPacket::NewAnimation(NewAnimation {
                serial,
                animation_type: 1,
                action: 0,
                delay: 0,
            }),
            ServerPacket::Effect(effect),
            ServerPacket::HuedEffect(HuedEffect {
                effect,
                hue: crate::wire::Hue(0x26),
                render_mode: 0,
            }),
            ServerPacket::LoginDenied(LoginDenied {
                reason: crate::login::DenyReason::BadPassword,
            }),
            ServerPacket::ShardList(ShardList {
                shards: vec![crate::login::ShardEntry {
                    name: "Britannia".to_owned(),
                    percent_full: crate::login::PercentFull::clamped(10),
                    timezone: 5,
                    address: std::net::Ipv4Addr::new(127, 0, 0, 1),
                }],
            }),
            ServerPacket::Relay(Relay {
                endpoint: std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(127, 0, 0, 1), 2593),
                auth_key: AuthKey(0xDEAD_BEEF),
            }),
            ServerPacket::CharacterList(CharacterList {
                characters: vec![crate::login::CharacterEntry {
                    name: crate::identity::CharacterName("Lord British".to_owned()),
                }],
                starts: Vec::new(),
                flags: crate::login::CharacterListFlags::NONE,
            }),
            ServerPacket::DeleteReject(DeleteReject {
                result: crate::login::DeleteResult::CharNotExist,
            }),
            ServerPacket::CharacterListUpdate(CharacterListUpdate {
                characters: vec![crate::login::CharacterEntry {
                    name: crate::identity::CharacterName("Lord British".to_owned()),
                }],
            }),
            ServerPacket::PlayerStart(PlayerStart {
                serial,
                body: crate::wire::Graphic(0x0190),
                position: crate::world::Point::new(1475, 1774, 0),
                facing: crate::direction::Facing::walking(crate::direction::Direction::South),
                map: crate::world::MapSize::BRITANNIA,
            }),
            ServerPacket::PlayerUpdate(PlayerUpdate {
                serial,
                body: crate::wire::Graphic(0x0190),
                hue: crate::wire::Hue(0x83EA),
                flags: crate::mobile::StatusFlags::NONE,
                position: crate::world::Point::new(1475, 1774, 0),
                facing: crate::direction::Facing::walking(crate::direction::Direction::South),
            }),
            ServerPacket::DeathStatus(DeathStatus { dead: true }),
            ServerPacket::WalkAck(WalkAck {
                sequence: crate::world::StepSequence(1),
                notoriety: crate::mobile::Notoriety::Innocent,
            }),
            ServerPacket::WalkReject(WalkReject {
                sequence: crate::world::StepSequence(1),
                position: crate::world::Point::new(1475, 1774, 0),
                facing: crate::direction::Facing::walking(crate::direction::Direction::South),
            }),
            ServerPacket::LoginComplete(LoginComplete),
            ServerPacket::LightLevel(LightLevel {
                level: crate::world::Light(0),
            }),
            ServerPacket::PlayMusic(PlayMusic {
                track: crate::world::MusicId(11),
            }),
            ServerPacket::SeasonChange(SeasonChange {
                season: crate::world::Season::Spring,
                play_sound: false,
            }),
            ServerPacket::LogoutAck(LogoutAck),
            ServerPacket::MapChange(MapChange {
                map: crate::world::Facet(0),
            }),
            ServerPacket::Remove(Remove { serial }),
            ServerPacket::OpenPaperdoll(OpenPaperdoll {
                serial,
                text: "Lord British".to_owned(),
                flags: crate::mobile::PaperdollFlags::NONE,
            }),
            ServerPacket::MobileStatus(MobileStatus {
                serial,
                name: "Lord British".to_owned(),
                hits: crate::mobile::Vitals {
                    current: 100,
                    max: 100,
                },
                female: false,
                strength: 100,
                dexterity: 90,
                intelligence: 80,
                stamina: crate::mobile::Vitals { current: 90, max: 90 },
                mana: crate::mobile::Vitals { current: 80, max: 80 },
                gold: 1234,
                armor: 0,
                weight: 14,
                max_weight: 390,
                stat_cap: 225,
                followers: 0,
                followers_max: 5,
            }),
            ServerPacket::MobileMove(MobileMove {
                serial,
                body: crate::wire::Graphic(0x0190),
                position: crate::world::Point::new(1475, 1774, 0),
                facing: crate::direction::Facing::walking(crate::direction::Direction::South),
                hue: crate::wire::Hue(0x83EA),
                flags: crate::mobile::StatusFlags::NONE,
                notoriety: crate::mobile::Notoriety::Innocent,
            }),
            ServerPacket::MobileIncoming(MobileIncoming {
                serial,
                body: crate::wire::Graphic(0x0190),
                position: crate::world::Point::new(1475, 1774, 0),
                facing: crate::direction::Facing::walking(crate::direction::Direction::South),
                hue: crate::wire::Hue(0x83EA),
                flags: crate::mobile::StatusFlags::NONE,
                notoriety: crate::mobile::Notoriety::Innocent,
                equipment: Vec::new(),
            }),
            ServerPacket::StatLocks(StatLocks {
                serial,
                locks: crate::mobile::StatLockBits::default(),
            }),
            ServerPacket::WorldItem(crate::items::WorldItem {
                serial: crate::serial::Serial::new(0x4000_0001).unwrap(),
                graphic: crate::wire::Graphic(0x0EED),
                amount: 1,
                position: crate::world::Point::new(1000, 2000, 5),
                hue: crate::wire::Hue::NONE,
            }),
            ServerPacket::DragCancel(crate::items::DragCancel {
                reason: crate::items::DragCancelReason::OutOfRange,
            }),
            ServerPacket::EquipUpdate(crate::items::EquipUpdate {
                item: crate::serial::Serial::new(0x4000_0002).unwrap(),
                graphic: crate::wire::Graphic(0x13B9),
                layer: crate::wire::Layer(1),
                mobile: crate::serial::Serial::new(0x0000_0001).unwrap(),
                hue: crate::wire::Hue(0x0021),
            }),
            ServerPacket::ContainerContents(crate::containers::ContainerContents {
                container: Some(crate::serial::Serial::new(0x4000_0001).unwrap()),
                items: Vec::new(),
            }),
            ServerPacket::BuyList(crate::vendor::BuyList {
                container: crate::serial::Serial::new(0x4000_0010).unwrap(),
                lines: vec![crate::vendor::BuyLine {
                    price: 3,
                    name: "black pearl".to_owned(),
                }],
            }),
            ServerPacket::SellList(crate::vendor::SellList {
                vendor: crate::serial::Serial::new(0x0000_0BBB).unwrap(),
                lines: vec![crate::vendor::SellLine {
                    serial: crate::serial::Serial::new(0x4000_0033).unwrap(),
                    graphic: crate::wire::Graphic(0x0F7A),
                    hue: crate::wire::Hue::NONE,
                    amount: 20,
                    price: 2,
                    name: "black pearl".to_owned(),
                }],
            }),
            ServerPacket::TooltipRevision(crate::properties::TooltipRevision {
                serial: crate::serial::Serial::new(0x0000_00AB).unwrap(),
                hash: 0x1234_5678,
            }),
            ServerPacket::SkillsFull(crate::skill::SkillsFull {
                entries: vec![crate::skill::SkillEntry {
                    id: 0,
                    value: 755,
                    base: 700,
                    lock: crate::skill::SkillLock::Locked,
                    cap: 1000,
                }],
            }),
            ServerPacket::SkillUpdate(crate::skill::SkillUpdate {
                entry: crate::skill::SkillEntry {
                    id: 25,
                    value: 501,
                    base: 501,
                    lock: crate::skill::SkillLock::Up,
                    cap: 1000,
                },
            }),
            ServerPacket::SpokenMessage(crate::speech::SpokenMessage {
                serial: crate::serial::Serial::new(0x0000_0002),
                graphic: Some(crate::wire::Graphic(0x0190)),
                mode: crate::speech::TalkMode::Regular,
                hue: crate::wire::Hue(0x0384),
                font: crate::speech::Font(3),
                name: "British".to_owned(),
                text: "hail".to_owned(),
            }),
            ServerPacket::LocalizedMessage(crate::speech::LocalizedMessage {
                serial: None,
                graphic: None,
                mode: crate::speech::TalkMode::Regular,
                hue: crate::wire::Hue(0x03B2),
                font: crate::speech::Font(3),
                cliloc: crate::wire::ClilocId(1_042_764),
                name: "System".to_owned(),
                arguments: "Iolo".to_owned(),
            }),
            ServerPacket::UnicodeMessage(crate::speech::UnicodeMessage {
                serial: crate::serial::Serial::new(0x0000_0002),
                graphic: Some(crate::wire::Graphic(0x0190)),
                mode: crate::speech::TalkMode::Regular,
                hue: crate::wire::Hue(0x0384),
                font: crate::speech::Font(3),
                language: "PTB".to_owned(),
                name: "Cidadão".to_owned(),
                text: "olá".to_owned(),
            }),
            ServerPacket::ContextMenu(crate::context::ContextMenu {
                serial: crate::serial::Serial::new(0x0000_00AB).unwrap(),
                entries: vec![crate::context::ContextMenuEntry {
                    cliloc: crate::wire::ClilocId(3_000_362),
                    flags: crate::context::ContextMenuFlags::NONE,
                }],
            }),
            ServerPacket::SpellbookContent(crate::spellbook::SpellbookContent {
                serial: crate::serial::Serial::new(0x4000_0001).unwrap(),
                graphic: crate::wire::Graphic(0x0EFA),
                offset: 1,
                content: 1,
            }),
            ServerPacket::CloseGump(crate::gump::CloseGump {
                gump_id: crate::gump::GumpId(0x0051_0001),
                button: crate::gump::ButtonId::CLOSE_BOX,
            }),
            ServerPacket::GumpDisplay(crate::gump::GumpDisplay {
                serial: crate::gump::GumpKey::STANDALONE,
                gump_id: crate::gump::GumpId(0x0051_0001),
                at: crate::gump::GumpPoint::new(75, 25),
                layout: "{ page 0 }".to_owned(),
                lines: Vec::new(),
            }),
        ]
    }

    #[test]
    fn every_variant_writes_the_id_it_claims() {
        for packet in one_of_each() {
            let bytes = packet.encode(version());
            assert_eq!(bytes[0], packet.id(), "{packet:?}");
        }
    }

    #[test]
    fn every_fixed_variant_writes_exactly_its_declared_length() {
        // The check `frame_body` makes in debug builds, made unconditional and
        // over every variant: a field added to a payload and forgotten in its
        // encoder shows up here.
        for packet in one_of_each() {
            let bytes = packet.encode(version());
            match packet.length(version()) {
                PacketLength::Fixed(size) => {
                    assert_eq!(bytes.len(), size as usize, "{packet:?}");
                }
                PacketLength::Variable => {
                    assert_eq!(
                        u16::from_be_bytes([bytes[1], bytes[2]]) as usize,
                        bytes.len(),
                        "{packet:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_packet_frames_to_its_own_length() {
        // The oracle for `server_packet_length`: encode one of every variant and
        // ask the framer to find it again. It catches a wrong id in the table, a
        // length that disagrees with the encoder, and — the reason this is done
        // over bytes rather than over `packet.length()` — a 0xBF subcommand whose
        // declared `Fixed(n)` must still be framed as length-prefixed.
        for packet in one_of_each() {
            let bytes = packet.encode(version());
            assert_eq!(
                frame_server_packet(&bytes, version()),
                Ok(Frame::Complete(bytes.len())),
                "{packet:?}"
            );
        }
    }

    #[test]
    fn a_packet_split_across_reads_asks_for_the_rest() {
        // What a socket does to a client: half a packet arrives, and the framer
        // has to say how much more it needs rather than guess or fail. Over every
        // variant, because a variable-length packet answers this from its length
        // field and a fixed one from the table.
        for packet in one_of_each() {
            let bytes = packet.encode(version());
            if bytes.len() < 2 {
                continue; // nothing to cut short
            }
            assert_eq!(
                frame_server_packet(&bytes[..bytes.len() - 1], version()),
                Ok(Frame::Incomplete { needed: bytes.len() }),
                "{packet:?}"
            );
        }
    }

    #[test]
    fn the_hand_written_packets_are_in_the_table() {
        // 0x24, 0x25, 0xB9 and 0x76 are not `EncodePacket`s, so `one_of_each`
        // cannot reach them — and a client that cannot frame them stops dead on
        // the first container it opens.
        let serial = Serial::new(0x0000_002A).unwrap();
        let bytes = crate::containers::encode_open_container(serial, crate::wire::Graphic(0x3C), version());
        assert_eq!(
            frame_server_packet(&bytes, version()),
            Ok(Frame::Complete(bytes.len()))
        );

        let bytes = crate::login::encode_supported_features(crate::login::SupportedFeatures::ML, true);
        assert_eq!(
            frame_server_packet(&bytes, version()),
            Ok(Frame::Complete(bytes.len()))
        );

        let bytes = crate::world::encode_server_change(
            crate::world::Point::new(1, 2, 3),
            crate::world::MapSize {
                width: 6144,
                height: 4096,
            },
        );
        assert_eq!(
            frame_server_packet(&bytes, version()),
            Ok(Frame::Complete(bytes.len()))
        );
    }

    #[test]
    fn the_old_feature_mask_is_two_bytes_narrower() {
        // The one place the table's `version` earns its place: same id, and a
        // client from before 6.0.14.2 reads two fewer bytes. Framing it with the
        // modern length swallows the first two bytes of whatever follows.
        let old = ClientVersion::new(5, 0, 9, 1);
        assert_eq!(
            server_packet_length(0xB9, old),
            Some(PacketLength::Fixed(3)),
            "an old client reads a 16-bit mask"
        );
        assert_eq!(
            server_packet_length(0xB9, version()),
            Some(PacketLength::Fixed(5))
        );
    }

    #[test]
    fn an_id_this_engine_never_sends_is_fatal() {
        // Not a silent skip: without a length there is no way to find where the
        // next packet starts, so the connection is over. 0xD6 is a real
        // server-to-client packet this engine does not send — see the table.
        assert_eq!(server_packet_length(0xD6, version()), None);
        assert_eq!(
            frame_server_packet(&[0xD6, 0x00, 0x05], version()),
            Err(FrameError::UnknownPacket(0xD6))
        );
    }

    #[test]
    fn the_login_conversation_round_trips() {
        // The packets a client has to read to reach the world, encoded by this
        // server and decoded as the client will decode them. Round-tripping is
        // the first test the encoders have ever had against a real inverse
        // rather than against hand-written bytes.
        let packets = [
            ServerPacket::LoginDenied(LoginDenied {
                reason: crate::login::DenyReason::BadPassword,
            }),
            ServerPacket::ShardList(ShardList {
                shards: vec![crate::login::ShardEntry {
                    name: "OpenShard".to_owned(),
                    percent_full: crate::login::PercentFull::clamped(12),
                    timezone: 5,
                    address: std::net::Ipv4Addr::new(192, 168, 11, 6),
                }],
            }),
            ServerPacket::Relay(Relay {
                endpoint: std::net::SocketAddrV4::new(std::net::Ipv4Addr::new(192, 168, 11, 6), 2593),
                auth_key: AuthKey(0xDEAD_BEEF),
            }),
            ServerPacket::LoginComplete(LoginComplete),
        ];

        for packet in packets {
            let bytes = packet.encode(version());
            assert_eq!(
                ServerPacket::decode(&bytes, version()),
                Ok(Some(packet.clone())),
                "{packet:?}"
            );
        }
    }

    #[test]
    fn the_packets_that_populate_a_world_view_round_trip() {
        // 0x1D, 0x20, 0x11, 0x77, 0x78 and 0x1A: what M1a needs so a client can
        // hold anyone but the player. `version()` here is TOL — a status kind
        // of 6 and the new mobile-incoming layout — so every field the struct
        // models actually rides the wire; see the separate tests below for the
        // two lossy shapes (an old status kind, the old equipment layout).
        let serial = Serial::new(0x0000_002A).unwrap();

        let packets = [
            ServerPacket::Remove(Remove { serial }),
            ServerPacket::PlayerUpdate(PlayerUpdate {
                serial,
                body: crate::wire::Graphic(0x0190),
                hue: crate::wire::Hue(0x83EA),
                flags: crate::mobile::StatusFlags::NONE,
                position: crate::world::Point::new(1475, 1774, -5),
                facing: crate::direction::Facing::running(crate::direction::Direction::SouthEast),
            }),
            ServerPacket::MobileStatus(MobileStatus {
                serial,
                name: "Lord British".to_owned(),
                hits: crate::mobile::Vitals {
                    current: 100,
                    max: 100,
                },
                female: false,
                strength: 100,
                dexterity: 90,
                intelligence: 80,
                stamina: crate::mobile::Vitals { current: 90, max: 90 },
                mana: crate::mobile::Vitals { current: 80, max: 80 },
                gold: 1234,
                armor: 0,
                weight: 14,
                max_weight: 390,
                stat_cap: 225,
                followers: 0,
                followers_max: 5,
            }),
            ServerPacket::MobileMove(MobileMove {
                serial,
                body: crate::wire::Graphic(0x0190),
                position: crate::world::Point::new(1475, 1774, -5),
                facing: crate::direction::Facing::running(crate::direction::Direction::SouthEast),
                hue: crate::wire::Hue(0x83EA),
                flags: crate::mobile::StatusFlags::NONE,
                notoriety: crate::mobile::Notoriety::Murderer,
            }),
            ServerPacket::MobileIncoming(MobileIncoming {
                serial,
                body: crate::wire::Graphic(0x0190),
                position: crate::world::Point::new(1475, 1774, -5),
                facing: crate::direction::Facing::running(crate::direction::Direction::SouthEast),
                hue: crate::wire::Hue(0x83EA),
                flags: crate::mobile::StatusFlags::NONE,
                notoriety: crate::mobile::Notoriety::Innocent,
                equipment: vec![crate::mobile::Equipment {
                    serial: Serial::new(0x4000_0001).unwrap(),
                    graphic: crate::wire::Graphic(0x1517),
                    layer: crate::wire::Layer(0x05),
                    hue: crate::wire::Hue(0x0021),
                }],
            }),
            ServerPacket::WorldItem(crate::items::WorldItem {
                serial: Serial::new(0x4000_00AB).unwrap(),
                graphic: crate::wire::Graphic(0x0EED),
                amount: 500,
                position: crate::world::Point::new(1000, 2000, -5),
                hue: crate::wire::Hue(0x0021),
            }),
        ];

        for packet in packets {
            let bytes = packet.encode(version());
            assert_eq!(
                ServerPacket::decode(&bytes, version()),
                Ok(Some(packet.clone())),
                "{packet:?}"
            );
        }
    }

    #[test]
    fn an_old_client_gets_the_hue_flagged_equipment_layout_back() {
        // Before 7.0.33.1 an item's hue rides on a stolen bit in the graphic
        // rather than a fixed field; the decoder must read the same layout it
        // was handed, not the one `version()` elsewhere in this module implies.
        let old = ClientVersion::new(7, 0, 33, 0);
        let packet = ServerPacket::MobileIncoming(MobileIncoming {
            serial: Serial::new(0x0000_0002).unwrap(),
            body: crate::wire::Graphic(0x0190),
            position: crate::world::Point::new(1475, 1774, -5),
            facing: crate::direction::Facing::walking(crate::direction::Direction::South),
            hue: crate::wire::Hue(0x83EA),
            flags: crate::mobile::StatusFlags::NONE,
            notoriety: crate::mobile::Notoriety::Innocent,
            equipment: vec![
                crate::mobile::Equipment {
                    serial: Serial::new(0x4000_0001).unwrap(),
                    graphic: crate::wire::Graphic(0x1517),
                    layer: crate::wire::Layer(0x05),
                    hue: crate::wire::Hue(0x0021),
                },
                crate::mobile::Equipment {
                    serial: Serial::new(0x4000_0002).unwrap(),
                    graphic: crate::wire::Graphic(0x1F03),
                    layer: crate::wire::Layer(0x0D),
                    hue: crate::wire::Hue::NONE,
                },
            ],
        });

        let bytes = packet.encode(old);
        assert_eq!(ServerPacket::decode(&bytes, old), Ok(Some(packet)));
    }

    #[test]
    fn a_pre_aos_status_has_no_max_weight_on_the_wire() {
        // Below status type 5 the client is never told a max weight at all —
        // decoding gets 0 back, not the value the server happened to hold, and
        // that is the honest shape of the packet rather than a decoder gap.
        let ancient = ClientVersion::new(3, 0, 8, 10);
        let sent = MobileStatus {
            serial: Serial::new(0x0001_2345).unwrap(),
            name: "Lord British".to_owned(),
            hits: crate::mobile::Vitals {
                current: 100,
                max: 100,
            },
            female: false,
            strength: 100,
            dexterity: 90,
            intelligence: 80,
            stamina: crate::mobile::Vitals { current: 90, max: 90 },
            mana: crate::mobile::Vitals { current: 80, max: 80 },
            gold: 1234,
            armor: 0,
            weight: 14,
            max_weight: 390,
            stat_cap: 225,
            followers: 0,
            followers_max: 5,
        };

        let bytes = ServerPacket::MobileStatus(sent.clone()).encode(ancient);
        let Ok(Some(ServerPacket::MobileStatus(decoded))) = ServerPacket::decode(&bytes, ancient) else {
            panic!("expected a mobile status");
        };
        assert_eq!(decoded.max_weight, 0, "type 3 never carries this field");
        assert_eq!(
            decoded,
            MobileStatus {
                max_weight: 0,
                ..sent
            }
        );
    }

    #[test]
    fn the_two_answers_to_a_walk_request_round_trip() {
        // 0x22 and 0x21: the other half of M1a. A negative z matters here —
        // `0x21`'s height is written as a byte and read back as an `i8`, and a
        // client that got the sign wrong would snap itself into the ground.
        let packets = [
            ServerPacket::WalkAck(WalkAck {
                sequence: crate::world::StepSequence(0x2A),
                notoriety: crate::mobile::Notoriety::Murderer,
            }),
            ServerPacket::WalkReject(WalkReject {
                sequence: crate::world::StepSequence(0xFF),
                position: crate::world::Point::new(1475, 1774, -5),
                facing: crate::direction::Facing::running(crate::direction::Direction::NorthWest),
            }),
        ];

        for packet in packets {
            let bytes = packet.encode(version());
            assert_eq!(
                ServerPacket::decode(&bytes, version()),
                Ok(Some(packet.clone())),
                "{packet:?}"
            );
        }
    }

    #[test]
    fn an_old_client_is_told_a_yellow_bar_is_blue_and_that_is_what_it_reads_back() {
        // `Notoriety::for_client` downgrades `Invulnerable` for a client with no
        // yellow bar, so this one shape does not survive the round trip — by
        // design, and the honest half of that bargain is that decoding reports
        // what was actually on the wire rather than what the sender meant.
        let old = ClientVersion::new(3, 0, 8, 10);
        let sent = ServerPacket::WalkAck(WalkAck {
            sequence: crate::world::StepSequence(1),
            notoriety: crate::mobile::Notoriety::Invulnerable,
        });
        assert_eq!(
            ServerPacket::decode(&sent.encode(old), old),
            Ok(Some(ServerPacket::WalkAck(WalkAck {
                sequence: crate::world::StepSequence(1),
                notoriety: crate::mobile::Notoriety::Innocent,
            }))),
        );
        // A client that does have the yellow bar gets it back intact.
        assert_eq!(
            ServerPacket::decode(&sent.encode(version()), version()),
            Ok(Some(sent))
        );
    }

    #[test]
    fn a_relayed_address_survives_both_byte_orders() {
        // 0xA8 reverses the octets for a modern client and 0x8C never does. A
        // decoder that copied one rule to the other would still round-trip
        // against itself — so the two are checked against the *same* address,
        // which is the only thing that catches it.
        let address = std::net::Ipv4Addr::new(192, 168, 11, 6);
        let list = ServerPacket::ShardList(ShardList {
            shards: vec![crate::login::ShardEntry {
                name: "OpenShard".to_owned(),
                percent_full: crate::login::PercentFull::EMPTY,
                timezone: 0,
                address,
            }],
        })
        .encode(version());
        let relay = ServerPacket::Relay(Relay {
            endpoint: std::net::SocketAddrV4::new(address, 2593),
            auth_key: AuthKey(1),
        })
        .encode(version());

        // The bytes differ...
        assert_eq!(&relay[1..5], &[192, 168, 11, 6], "0x8C sends octets in order");
        assert_eq!(
            &list[list.len() - 4..],
            &[6, 11, 168, 192],
            "0xA8 reverses them for a modern client"
        );

        // ...and both decode to the one address.
        let Ok(Some(ServerPacket::ShardList(decoded))) = ServerPacket::decode(&list, version()) else {
            panic!("the shard list did not decode");
        };
        assert_eq!(decoded.shards[0].address, address);
        let Ok(Some(ServerPacket::Relay(decoded))) = ServerPacket::decode(&relay, version()) else {
            panic!("the relay did not decode");
        };
        assert_eq!(*decoded.endpoint.ip(), address);
    }

    #[test]
    fn a_dungeon_floor_comes_back_negative() {
        // 0x1B writes z as one signed byte behind a zero. Reading the pair as a
        // big-endian i16 puts a character at z = 65,526 instead of -10, and the
        // client draws them somewhere over the map.
        let start = PlayerStart {
            serial: Serial::new(0x0000_002A).unwrap(),
            body: crate::wire::Graphic(0x0190),
            position: crate::world::Point::new(1000, 1200, -10),
            facing: crate::direction::Facing::running(crate::direction::Direction::SouthEast),
            map: crate::world::MapSize::BRITANNIA,
        };
        let bytes = ServerPacket::PlayerStart(start).encode(version());
        assert_eq!(
            ServerPacket::decode(&bytes, version()),
            Ok(Some(ServerPacket::PlayerStart(start)))
        );
    }

    #[test]
    fn the_character_list_comes_back_as_the_wire_holds_it() {
        // The list is padded to five slots on the way out, so decoding gives
        // five back however many characters exist. Re-encoding what was decoded
        // must produce the same bytes — the property that matters, since the
        // struct's own `characters` is not what the wire carries.
        let list = CharacterList {
            characters: vec![crate::login::CharacterEntry {
                name: crate::identity::CharacterName::new("Lord British"),
            }],
            starts: vec![crate::login::StartLocation {
                area: "Britain".to_owned(),
                name: "Castle Britannia".to_owned(),
                position: crate::world::Point::new(1475, 1770, 20),
                map: crate::world::Facet(0),
                description_cliloc: crate::wire::ClilocId(1075072),
            }],
            flags: crate::login::CharacterListFlags::TOOLTIPS,
        };
        let bytes = ServerPacket::CharacterList(list).encode(version());

        let Ok(Some(ServerPacket::CharacterList(decoded))) = ServerPacket::decode(&bytes, version()) else {
            panic!("the character list did not decode");
        };
        assert_eq!(decoded.characters.len(), crate::login::MIN_CHARACTER_SLOTS);
        assert_eq!(decoded.characters[0].name, "Lord British");
        assert_eq!(decoded.starts[0].name, "Castle Britannia");
        assert_eq!(
            decoded.starts[0].position,
            crate::world::Point::new(1475, 1770, 20)
        );
        assert_eq!(
            ServerPacket::CharacterList(decoded).encode(version()),
            bytes,
            "re-encoding what the wire held must reproduce it"
        );
    }

    #[test]
    fn the_old_character_list_says_it_is_not_decoded() {
        // Before 7.0.13.0 a start location has no coordinates at all. Zeros
        // would be three numbers that look chosen; this says what happened.
        let bytes = ServerPacket::CharacterList(CharacterList {
            characters: Vec::new(),
            starts: Vec::new(),
            flags: crate::login::CharacterListFlags::NONE,
        })
        .encode(version());
        let old = ClientVersion::new(5, 0, 9, 1);
        assert!(matches!(
            ServerPacket::decode(&bytes, old),
            Err(ServerDecodeError::CharacterList(DecodeError::Unsupported { .. }))
        ));
    }

    #[test]
    fn a_deny_code_the_client_does_not_know_is_an_error() {
        // Five codes reach a client. A sixth is a server this one does not
        // understand, and picking the nearest legal reason would be the decoder
        // inventing the answer.
        assert!(matches!(
            ServerPacket::decode(&[0x82, 0x09], version()),
            Err(ServerDecodeError::LoginDenied(DecodeError::UnknownValue { .. }))
        ));
    }

    #[test]
    fn a_packet_with_no_decoder_yet_is_not_a_failure() {
        // The framer knew the length, so the stream is intact and the client can
        // read on. This is the answer that shrinks as decoders are written.
        let bytes = ServerPacket::LightLevel(crate::world::LightLevel {
            level: crate::world::Light(0),
        })
        .encode(version());
        assert_eq!(ServerPacket::decode(&bytes, version()), Ok(None));
    }

    #[test]
    fn the_enum_and_the_payload_agree_byte_for_byte() {
        // Going through the enum must not add or reorder anything: the variant is
        // a wrapper, not a second encoder.
        let war = WarMode { war: true };
        assert_eq!(
            ServerPacket::WarMode(war).encode(version()),
            encode_packet(&war, version())
        );
    }
}
