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
use crate::combat::{
    AttackTarget,
    HealthBar,
    WarMode,
};
use crate::containers::{
    AddToContainer,
    ContainerContents,
    OpenContainer,
    add_to_container_length,
    open_container_length,
};
use crate::context::ContextMenu;
use crate::craft::{
    CraftCatalogue,
    CraftWorkbench,
};
use crate::error::{
    DecodeError,
    expect_id,
};
use crate::feature::Feature;
use crate::feedback::{
    Animation,
    CombatActionBalked,
    CombatActionEnded,
    CombatActionPhase,
    CombatActionStage,
    GraphicalEffect,
    HarvestCompleted,
    HarvestPreview,
    HarvestRefused,
    HarvestToolVisual,
    HuedEffect,
    NewAnimation,
    PlaySound,
    SwingTiming,
};
use crate::gump::{
    CloseGump,
    GumpDisplay,
};
use crate::items::{
    CorpseEquipment,
    DragCancel,
    EquipUpdate,
    WorldItem,
};
use crate::login::{
    CharacterList,
    CharacterListUpdate,
    DeleteReject,
    LoginDenied,
    Relay,
    ShardList,
    supported_features_length,
};
use crate::mobile::{
    MobileIncoming,
    MobileMove,
    MobileStatus,
    OpenPaperdoll,
    Remove,
    StatLocks,
};
use crate::packet::{
    DecodePacket,
    EncodePacket,
    Frame,
    FrameError,
    MAX_SERVER_PACKET_SIZE,
    PacketLength,
    frame_body,
    frame_packet,
};
use crate::party::{
    PartyInvitation,
    PartyMemberList,
    PartyRemoveMember,
    PartyTextMessage,
};
use crate::properties::{
    PropertyListReply,
    TooltipRevision,
};
use crate::skill::{
    SkillUpdate,
    SkillsFull,
    SkillsPacket,
};
use crate::speech::{
    LocalizedMessage,
    SpokenMessage,
    UnicodeMessage,
};
use crate::spellbook::SpellbookContent;
use crate::target::{
    MultiTargetRequest,
    TargetCursor,
};
use crate::vendor::{
    BuyList,
    SellList,
};
use crate::version::ClientVersion;
use crate::world::{
    DeathAnimation,
    DeathStatus,
    LightLevel,
    LoginComplete,
    LogoutAck,
    MapChange,
    PlayMusic,
    PlayerStart,
    PlayerUpdate,
    SERVER_CHANGE_LENGTH,
    SeasonChange,
    WalkAck,
    WalkReject,
    WeatherChange,
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
    /// `0x99` — a targeting cursor with a house drawn under it.
    MultiTarget(MultiTargetRequest),
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
    /// `0xBF 0xE00B` — how long the immediately following swing occupies.
    SwingTiming(SwingTiming),
    /// `0xBF 0xE00C` — the carried tool to show for the next harvest action.
    HarvestToolVisual(HarvestToolVisual),
    /// `0xBF 0xE00D` — start this harvest locally when its cursor is answered.
    HarvestPreview(HarvestPreview),
    /// `0xBF 0xE00E` — this optimistic harvest did not start on the shard.
    HarvestRefused(HarvestRefused),
    /// `0xBF 0xE00F` — the shard finished this harvest.
    HarvestCompleted(HarvestCompleted),
    /// `0xBF 0xE010` — a mobile entered a phase of a combat action.
    CombatActionPhase(CombatActionPhase),
    /// `0xBF 0xE011` — a mobile's combat action ended, and how.
    CombatActionEnded(CombatActionEnded),
    /// `0xBF 0xE012` — a fighter cannot begin an action, or can again.
    CombatActionBalked(CombatActionBalked),
    /// `0xBF 0xE013` — a running action entered a new stage.
    CombatActionStage(CombatActionStage),
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
    /// `0xAF` — somebody else died, and this is the corpse they leave.
    DeathAnimation(DeathAnimation),
    /// `0x22` — a walk request is allowed.
    WalkAck(WalkAck),
    /// `0x21` — a walk request is refused.
    WalkReject(WalkReject),
    /// `0x55` — the client may start drawing.
    LoginComplete(LoginComplete),
    /// `0x4F` — overall light level.
    LightLevel(LightLevel),
    /// `0x65` — precipitation, intensity and temperature.
    WeatherChange(WeatherChange),
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
    /// `0x89` — the layers the items in a corpse container occupied on its body.
    CorpseEquipment(CorpseEquipment),
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
    /// `0xBF` subcommand `0x1D` — which revision a designed house's picture is
    /// at, so a client knows whether what it has cached is stale.
    ///
    /// Its `0xD8` answer is deliberately **not** a variant here: decoding one
    /// needs the house's width and height, which no field on the wire carries,
    /// and this enum's `decode(bytes, version)` has no way to supply them. It
    /// reaches a client as `Event::Undecoded` with the bytes intact — see
    /// [`crate::design`].
    DesignRevision(crate::design::DesignRevision),
    /// `0xBF` subcommand `0xE001` — the authority this shard holds the
    /// connection's character at.
    ///
    /// This engine's own, not the reference protocol's: see
    /// [`crate::access::AuthorityNotice`] for what the client does with it and
    /// why inventing a subcommand is safe here.
    AuthorityNotice(crate::access::AuthorityNotice),
    /// `0xBF` subcommand `0xE003` — one fragment of one chunk of the world.
    ///
    /// This engine's own, like the three below and above it: see
    /// [`crate::chunks`], which is the whole of how a client of ours draws the
    /// shard's ground rather than its own disk's.
    ChunkData(crate::chunks::ChunkData),
    /// `0xBF` subcommand `0xE004` — which facet the connection is standing in,
    /// how big it is and which revision of it the shard holds.
    WorldNotice(crate::chunks::WorldNotice),
    /// `0xBF` subcommand `0xE005` — the ground moved under a connection that is
    /// already standing on it, and these are the chunks that moved.
    PublishNotice(crate::chunks::PublishNotice),
    /// `0xBF` subcommand `0xE006` — a chunk that was asked for is not coming.
    ChunkRefused(crate::chunks::ChunkRefused),
    /// `0xBF` subcommand `0xE008` — what moved since the revision a client says
    /// it already holds, so that a cache asks only for the difference.
    ChangesReply(crate::chunks::ChangesReply),
    /// `0xBF` subcommand `0xE00A` — acceptance or a typed refusal of one live
    /// map-editor commit.
    MapEditReply(crate::mapedit::MapEditReply),
    /// `0xDC` — the tooltip revision for one object.
    TooltipRevision(TooltipRevision),
    /// `0xD6` — the property list itself, answering a client's batch query.
    PropertyListReply(PropertyListReply),
    /// `0xBF` subcommand `0x06` type `0x01` — the whole party roster.
    PartyMemberList(PartyMemberList),
    /// `0xBF` subcommand `0x06` type `0x02` — a member left, and who is left.
    PartyRemoveMember(PartyRemoveMember),
    /// `0xBF` subcommand `0x06` type `0x03`/`0x04` — a line of party chat.
    PartyTextMessage(PartyTextMessage),
    /// `0xBF` subcommand `0x06` type `0x07` — you are invited to a party.
    PartyInvitation(PartyInvitation),
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
    /// `0xBF 0xE016` — compact data for the client-owned craft catalogue table.
    CraftCatalogue(CraftCatalogue),
    /// `0xBF 0xE017` — compact data for a tool-specific craft workbench.
    CraftWorkbench(CraftWorkbench),
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
            Self::TargetCursor(_) => <TargetCursor as EncodePacket>::ID,
            Self::MultiTarget(_) => <MultiTargetRequest as DecodePacket>::ID,
            Self::WarMode(_) => <WarMode as EncodePacket>::ID,
            Self::AttackTarget(_) => <AttackTarget as EncodePacket>::ID,
            Self::Health(_) => <HealthBar as EncodePacket>::ID,
            Self::PlaySound(_) => <PlaySound as EncodePacket>::ID,
            Self::Animation(_) => <Animation as EncodePacket>::ID,
            Self::NewAnimation(_) => <NewAnimation as EncodePacket>::ID,
            Self::SwingTiming(_) => <SwingTiming as EncodePacket>::ID,
            Self::HarvestToolVisual(_) => <HarvestToolVisual as EncodePacket>::ID,
            Self::HarvestPreview(_) => <HarvestPreview as EncodePacket>::ID,
            Self::HarvestRefused(_) => <HarvestRefused as EncodePacket>::ID,
            Self::HarvestCompleted(_) => <HarvestCompleted as EncodePacket>::ID,
            Self::CombatActionPhase(_) => <CombatActionPhase as EncodePacket>::ID,
            Self::CombatActionEnded(_) => <CombatActionEnded as EncodePacket>::ID,
            Self::CombatActionBalked(_) => <CombatActionBalked as EncodePacket>::ID,
            Self::CombatActionStage(_) => <CombatActionStage as EncodePacket>::ID,
            Self::Effect(_) => <GraphicalEffect as EncodePacket>::ID,
            Self::HuedEffect(_) => HuedEffect::ID,
            Self::LoginDenied(_) => <LoginDenied as EncodePacket>::ID,
            Self::ShardList(_) => <ShardList as EncodePacket>::ID,
            Self::Relay(_) => <Relay as EncodePacket>::ID,
            Self::CharacterList(_) => <CharacterList as EncodePacket>::ID,
            Self::DeleteReject(_) => DeleteReject::ID,
            Self::CharacterListUpdate(_) => CharacterListUpdate::ID,
            Self::PlayerStart(_) => <PlayerStart as EncodePacket>::ID,
            Self::PlayerUpdate(_) => <PlayerUpdate as EncodePacket>::ID,
            Self::DeathStatus(_) => <DeathStatus as EncodePacket>::ID,
            Self::DeathAnimation(_) => <DeathAnimation as EncodePacket>::ID,
            Self::WalkAck(_) => <WalkAck as EncodePacket>::ID,
            Self::WalkReject(_) => <WalkReject as EncodePacket>::ID,
            Self::LoginComplete(_) => <LoginComplete as EncodePacket>::ID,
            Self::LightLevel(_) => <LightLevel as EncodePacket>::ID,
            Self::WeatherChange(_) => <WeatherChange as EncodePacket>::ID,
            Self::PlayMusic(_) => <PlayMusic as EncodePacket>::ID,
            Self::SeasonChange(_) => SeasonChange::ID,
            Self::LogoutAck(_) => <LogoutAck as EncodePacket>::ID,
            Self::MapChange(_) => MapChange::ID,
            Self::Remove(_) => <Remove as EncodePacket>::ID,
            Self::OpenPaperdoll(_) => <OpenPaperdoll as EncodePacket>::ID,
            Self::CorpseEquipment(_) => <CorpseEquipment as EncodePacket>::ID,
            Self::MobileStatus(_) => <MobileStatus as EncodePacket>::ID,
            Self::MobileMove(_) => <MobileMove as EncodePacket>::ID,
            Self::MobileIncoming(_) => <MobileIncoming as EncodePacket>::ID,
            Self::StatLocks(_) => StatLocks::ID,
            Self::WorldItem(_) => <WorldItem as EncodePacket>::ID,
            Self::DragCancel(_) => <DragCancel as EncodePacket>::ID,
            Self::EquipUpdate(_) => <EquipUpdate as EncodePacket>::ID,
            Self::OpenContainer(_) => <OpenContainer as DecodePacket>::ID,
            Self::AddToContainer(_) => <AddToContainer as DecodePacket>::ID,
            Self::ContainerContents(_) => <ContainerContents as EncodePacket>::ID,
            Self::BuyList(_) => <BuyList as EncodePacket>::ID,
            Self::SellList(_) => <SellList as EncodePacket>::ID,
            Self::TooltipRevision(_) => <TooltipRevision as EncodePacket>::ID,
            Self::DesignRevision(_) => <crate::design::DesignRevision as EncodePacket>::ID,
            Self::AuthorityNotice(_) => <crate::access::AuthorityNotice as EncodePacket>::ID,
            Self::ChunkData(_) => <crate::chunks::ChunkData as EncodePacket>::ID,
            Self::WorldNotice(_) => <crate::chunks::WorldNotice as EncodePacket>::ID,
            Self::PublishNotice(_) => <crate::chunks::PublishNotice as EncodePacket>::ID,
            Self::ChunkRefused(_) => <crate::chunks::ChunkRefused as EncodePacket>::ID,
            Self::ChangesReply(_) => <crate::chunks::ChangesReply as EncodePacket>::ID,
            Self::MapEditReply(_) => <crate::mapedit::MapEditReply as EncodePacket>::ID,
            Self::PropertyListReply(_) => <PropertyListReply as EncodePacket>::ID,
            Self::PartyMemberList(_) => <PartyMemberList as EncodePacket>::ID,
            Self::PartyRemoveMember(_) => <PartyRemoveMember as EncodePacket>::ID,
            Self::PartyTextMessage(_) => <PartyTextMessage as EncodePacket>::ID,
            Self::PartyInvitation(_) => <PartyInvitation as EncodePacket>::ID,
            Self::SkillsFull(_) => SkillsFull::ID,
            Self::SkillUpdate(_) => SkillUpdate::ID,
            Self::SpokenMessage(_) => <SpokenMessage as EncodePacket>::ID,
            Self::LocalizedMessage(_) => <LocalizedMessage as EncodePacket>::ID,
            Self::UnicodeMessage(_) => <UnicodeMessage as EncodePacket>::ID,
            Self::ContextMenu(_) => ContextMenu::ID,
            Self::SpellbookContent(_) => <SpellbookContent as EncodePacket>::ID,
            Self::CloseGump(_) => <CloseGump as EncodePacket>::ID,
            Self::CraftCatalogue(_) => CraftCatalogue::ID,
            Self::CraftWorkbench(_) => CraftWorkbench::ID,
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
            // Two lengths, and the version decides — see `MultiTargetRequest`,
            // which for that reason is not an `EncodePacket` at all.
            Self::MultiTarget(_) => crate::target::multi_target_length(version.supports(Feature::HsPackets)),
            Self::WarMode(_) => <WarMode as EncodePacket>::LENGTH,
            Self::AttackTarget(_) => AttackTarget::LENGTH,
            Self::Health(_) => HealthBar::LENGTH,
            Self::PlaySound(_) => <PlaySound as EncodePacket>::LENGTH,
            Self::Animation(_) => Animation::LENGTH,
            Self::NewAnimation(_) => NewAnimation::LENGTH,
            Self::SwingTiming(_) => SwingTiming::LENGTH,
            Self::HarvestToolVisual(_) => HarvestToolVisual::LENGTH,
            Self::HarvestPreview(_) => HarvestPreview::LENGTH,
            Self::HarvestRefused(_) => HarvestRefused::LENGTH,
            Self::HarvestCompleted(_) => HarvestCompleted::LENGTH,
            Self::CombatActionPhase(_) => CombatActionPhase::LENGTH,
            Self::CombatActionEnded(_) => CombatActionEnded::LENGTH,
            Self::CombatActionBalked(_) => CombatActionBalked::LENGTH,
            Self::CombatActionStage(_) => CombatActionStage::LENGTH,
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
            Self::DeathAnimation(_) => DeathAnimation::LENGTH,
            Self::WalkAck(_) => WalkAck::LENGTH,
            Self::WalkReject(_) => WalkReject::LENGTH,
            Self::LoginComplete(_) => <LoginComplete as EncodePacket>::LENGTH,
            Self::LightLevel(_) => LightLevel::LENGTH,
            Self::WeatherChange(_) => WeatherChange::LENGTH,
            Self::PlayMusic(_) => <PlayMusic as EncodePacket>::LENGTH,
            Self::SeasonChange(_) => SeasonChange::LENGTH,
            Self::LogoutAck(_) => LogoutAck::LENGTH,
            Self::MapChange(_) => MapChange::LENGTH,
            Self::Remove(_) => Remove::LENGTH,
            Self::OpenPaperdoll(_) => <OpenPaperdoll as EncodePacket>::LENGTH,
            Self::CorpseEquipment(_) => <CorpseEquipment as EncodePacket>::LENGTH,
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
            Self::DesignRevision(_) => <crate::design::DesignRevision as EncodePacket>::LENGTH,
            Self::AuthorityNotice(_) => <crate::access::AuthorityNotice as EncodePacket>::LENGTH,
            Self::ChunkData(_) => <crate::chunks::ChunkData as EncodePacket>::LENGTH,
            Self::WorldNotice(_) => <crate::chunks::WorldNotice as EncodePacket>::LENGTH,
            Self::PublishNotice(_) => <crate::chunks::PublishNotice as EncodePacket>::LENGTH,
            Self::ChunkRefused(_) => <crate::chunks::ChunkRefused as EncodePacket>::LENGTH,
            Self::ChangesReply(_) => <crate::chunks::ChangesReply as EncodePacket>::LENGTH,
            Self::MapEditReply(_) => <crate::mapedit::MapEditReply as EncodePacket>::LENGTH,
            Self::PropertyListReply(_) => PropertyListReply::LENGTH,
            Self::PartyMemberList(_) => PartyMemberList::LENGTH,
            Self::PartyRemoveMember(_) => PartyRemoveMember::LENGTH,
            Self::PartyTextMessage(_) => PartyTextMessage::LENGTH,
            Self::PartyInvitation(_) => PartyInvitation::LENGTH,
            Self::SkillsFull(_) => SkillsFull::LENGTH,
            Self::SkillUpdate(_) => SkillUpdate::LENGTH,
            Self::SpokenMessage(_) => SpokenMessage::LENGTH,
            Self::LocalizedMessage(_) => LocalizedMessage::LENGTH,
            Self::UnicodeMessage(_) => UnicodeMessage::LENGTH,
            Self::ContextMenu(_) => ContextMenu::LENGTH,
            Self::SpellbookContent(_) => SpellbookContent::LENGTH,
            Self::CloseGump(_) => CloseGump::LENGTH,
            Self::CraftCatalogue(_) => CraftCatalogue::LENGTH,
            Self::CraftWorkbench(_) => CraftWorkbench::LENGTH,
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
            Self::MultiTarget(packet) => packet.write_body(out, version),
            Self::WarMode(packet) => packet.encode_body(out, version),
            Self::AttackTarget(packet) => packet.encode_body(out, version),
            Self::Health(packet) => packet.encode_body(out, version),
            Self::PlaySound(packet) => packet.encode_body(out, version),
            Self::Animation(packet) => packet.encode_body(out, version),
            Self::NewAnimation(packet) => packet.encode_body(out, version),
            Self::SwingTiming(packet) => packet.encode_body(out, version),
            Self::HarvestToolVisual(packet) => packet.encode_body(out, version),
            Self::HarvestPreview(packet) => packet.encode_body(out, version),
            Self::HarvestRefused(packet) => packet.encode_body(out, version),
            Self::HarvestCompleted(packet) => packet.encode_body(out, version),
            Self::CombatActionPhase(packet) => packet.encode_body(out, version),
            Self::CombatActionEnded(packet) => packet.encode_body(out, version),
            Self::CombatActionBalked(packet) => packet.encode_body(out, version),
            Self::CombatActionStage(packet) => packet.encode_body(out, version),
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
            Self::DeathAnimation(packet) => packet.encode_body(out, version),
            Self::WalkAck(packet) => packet.encode_body(out, version),
            Self::WalkReject(packet) => packet.encode_body(out, version),
            Self::LoginComplete(packet) => packet.encode_body(out, version),
            Self::LightLevel(packet) => packet.encode_body(out, version),
            Self::WeatherChange(packet) => packet.encode_body(out, version),
            Self::PlayMusic(packet) => packet.encode_body(out, version),
            Self::SeasonChange(packet) => packet.encode_body(out, version),
            Self::LogoutAck(packet) => packet.encode_body(out, version),
            Self::MapChange(packet) => packet.encode_body(out, version),
            Self::Remove(packet) => packet.encode_body(out, version),
            Self::OpenPaperdoll(packet) => packet.encode_body(out, version),
            Self::CorpseEquipment(packet) => packet.encode_body(out, version),
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
            Self::DesignRevision(packet) => packet.encode_body(out, version),
            Self::AuthorityNotice(packet) => packet.encode_body(out, version),
            Self::ChunkData(packet) => packet.encode_body(out, version),
            Self::WorldNotice(packet) => packet.encode_body(out, version),
            Self::PublishNotice(packet) => packet.encode_body(out, version),
            Self::ChunkRefused(packet) => packet.encode_body(out, version),
            Self::ChangesReply(packet) => packet.encode_body(out, version),
            Self::MapEditReply(packet) => packet.encode_body(out, version),
            Self::PropertyListReply(packet) => packet.encode_body(out, version),
            Self::PartyMemberList(packet) => packet.encode_body(out, version),
            Self::PartyRemoveMember(packet) => packet.encode_body(out, version),
            Self::PartyTextMessage(packet) => packet.encode_body(out, version),
            Self::PartyInvitation(packet) => packet.encode_body(out, version),
            Self::SkillsFull(packet) => packet.encode_body(out, version),
            Self::SkillUpdate(packet) => packet.encode_body(out, version),
            Self::SpokenMessage(packet) => packet.encode_body(out, version),
            Self::LocalizedMessage(packet) => packet.encode_body(out, version),
            Self::UnicodeMessage(packet) => packet.encode_body(out, version),
            Self::ContextMenu(packet) => packet.encode_body(out, version),
            Self::SpellbookContent(packet) => packet.encode_body(out, version),
            Self::CloseGump(packet) => packet.encode_body(out, version),
            Self::CraftCatalogue(packet) => packet.encode_body(out, version),
            Self::CraftWorkbench(packet) => packet.encode_body(out, version),
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
/// Decode a `0xBF`, whose subcommand is what says which packet it is.
///
/// # Why this is a second dispatch and not nine more arms
///
/// The id byte is the whole key everywhere else in [`ServerPacket::decode`], and
/// for `0xBF` it is not a key at all: nine of this enum's variants share it, and
/// which one a packet is lives two bytes further in. Every decoder below reads
/// that subcommand and refuses a body that is not its own, so the check is
/// written twice on purpose — here to pick, and there to be sure.
///
/// # Most of the family is still undecoded, and that is not an oversight
///
/// The context menu (`0x14`), the spellbook's contents (`0x1B`), the stat-lock
/// arrows (`0x19`) and the map change (`0x08`) all have `ServerPacket` variants,
/// encoders, and no arm here. Each needs a reader on the client that wants it,
/// and adding a decoder before there is one only moves the packet from
/// "undecoded" to "decoded and dropped". They are listed so the next person has
/// the subcommands rather than a search.
fn decode_extended(packet: &[u8], version: ClientVersion) -> Result<Option<ServerPacket>, ServerDecodeError> {
    // The id, the length, then the subcommand. A `0xBF` too short to hold one is
    // not this function's to refuse — the framer sized it, so it is a shard
    // writing nonsense, and reading it as no packet keeps the stream.
    let Some(subcommand) = packet
        .get(3..5)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
    else {
        return Ok(None);
    };
    Ok(Some(match subcommand {
        crate::gump::CloseGump::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::CloseGump)
                .map_err(ServerDecodeError::CloseGump)?
        }
        CraftCatalogue::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::CraftCatalogue)
                .map_err(ServerDecodeError::CraftCatalogue)?
        }
        CraftWorkbench::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::CraftWorkbench)
                .map_err(ServerDecodeError::CraftWorkbench)?
        }
        crate::spellbook::SpellbookContent::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::SpellbookContent)
                .map_err(ServerDecodeError::SpellbookContent)?
        }
        crate::design::DesignRevision::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::DesignRevision)
                .map_err(ServerDecodeError::DesignRevision)?
        }
        crate::access::AuthorityNotice::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::AuthorityNotice)
                .map_err(ServerDecodeError::AuthorityNotice)?
        }
        crate::chunks::ChunkData::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::ChunkData)
                .map_err(ServerDecodeError::ChunkData)?
        }
        crate::chunks::WorldNotice::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::WorldNotice)
                .map_err(ServerDecodeError::WorldNotice)?
        }
        crate::chunks::PublishNotice::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::PublishNotice)
                .map_err(ServerDecodeError::PublishNotice)?
        }
        crate::chunks::ChunkRefused::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::ChunkRefused)
                .map_err(ServerDecodeError::ChunkRefused)?
        }
        crate::chunks::ChangesReply::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::ChangesReply)
                .map_err(ServerDecodeError::ChangesReply)?
        }
        crate::mapedit::MapEditReply::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::MapEditReply)
                .map_err(ServerDecodeError::MapEditReply)?
        }
        SwingTiming::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::SwingTiming)
                .map_err(ServerDecodeError::SwingTiming)?
        }
        HarvestToolVisual::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::HarvestToolVisual)
                .map_err(ServerDecodeError::HarvestToolVisual)?
        }
        HarvestPreview::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::HarvestPreview)
                .map_err(ServerDecodeError::HarvestPreview)?
        }
        HarvestRefused::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::HarvestRefused)
                .map_err(ServerDecodeError::HarvestRefused)?
        }
        HarvestCompleted::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::HarvestCompleted)
                .map_err(ServerDecodeError::HarvestCompleted)?
        }
        CombatActionPhase::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::CombatActionPhase)
                .map_err(ServerDecodeError::CombatActionPhase)?
        }
        CombatActionEnded::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::CombatActionEnded)
                .map_err(ServerDecodeError::CombatActionEnded)?
        }
        CombatActionBalked::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::CombatActionBalked)
                .map_err(ServerDecodeError::CombatActionBalked)?
        }
        CombatActionStage::SUBCOMMAND => {
            decode_server(packet, version)
                .map(ServerPacket::CombatActionStage)
                .map_err(ServerDecodeError::CombatActionStage)?
        }
        crate::party::SUBCOMMAND => return decode_party(packet, version),
        _ => return Ok(None),
    }))
}

/// And a third dispatch, because every party packet shares a subcommand too.
///
/// The byte after it is the type — see [`crate::party`]'s table, and note that
/// it means different things inbound and outbound.
fn decode_party(packet: &[u8], version: ClientVersion) -> Result<Option<ServerPacket>, ServerDecodeError> {
    use crate::party::{
        PartyInvitation,
        PartyMemberList,
        PartyRemoveMember,
        PartyTextMessage,
    };

    let Some(&kind) = packet.get(5) else {
        return Ok(None);
    };
    Ok(Some(match kind {
        PartyMemberList::KIND => {
            decode_server(packet, version)
                .map(ServerPacket::PartyMemberList)
                .map_err(ServerDecodeError::Party)?
        }
        PartyRemoveMember::KIND => {
            decode_server(packet, version)
                .map(ServerPacket::PartyRemoveMember)
                .map_err(ServerDecodeError::Party)?
        }
        PartyTextMessage::KIND_ALL | PartyTextMessage::KIND_ONE => {
            decode_server(packet, version)
                .map(ServerPacket::PartyTextMessage)
                .map_err(ServerDecodeError::Party)?
        }
        PartyInvitation::KIND => {
            decode_server(packet, version)
                .map(ServerPacket::PartyInvitation)
                .map_err(ServerDecodeError::Party)?
        }
        _ => return Ok(None),
    }))
}

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
        // The whole `0xBF` family, by its subcommand. One arm and a second
        // dispatch rather than nine arms: the id byte does not say which
        // packet this is, and every decoder below would otherwise have to be
        // tried in turn and asked whether the body was its own.
        if id == 0xBF {
            return decode_extended(packet, version);
        }
        for decode_stage in [
            decode_session_packet,
            decode_world_packet,
            decode_interface_packet,
            decode_feedback_packet,
        ] {
            if let Some(decoded) = decode_stage(id, packet, version)? {
                return Ok(Some(decoded));
            }
        }
        Ok(None)
    }
}

/// Decode the packets that establish or enter a session.
fn decode_session_packet(
    id: u8,
    packet: &[u8],
    version: ClientVersion,
) -> Result<Option<ServerPacket>, ServerDecodeError> {
    let decoded = match id {
        <LoginDenied as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::LoginDenied)
                .map_err(ServerDecodeError::LoginDenied)?
        }
        <ShardList as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::ShardList)
                .map_err(ServerDecodeError::ShardList)?
        }
        <Relay as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::Relay)
                .map_err(ServerDecodeError::Relay)?
        }
        <CharacterList as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::CharacterList)
                .map_err(ServerDecodeError::CharacterList)?
        }
        <PlayerStart as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::PlayerStart)
                .map_err(ServerDecodeError::PlayerStart)?
        }
        <LoginComplete as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::LoginComplete)
                .map_err(ServerDecodeError::LoginComplete)?
        }
        _ => return Ok(None),
    };
    Ok(Some(decoded))
}

/// Decode updates to the world and the mobiles moving through it.
fn decode_world_packet(
    id: u8,
    packet: &[u8],
    version: ClientVersion,
) -> Result<Option<ServerPacket>, ServerDecodeError> {
    let decoded = match id {
        <LightLevel as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::LightLevel)
                .map_err(ServerDecodeError::LightLevel)?
        }
        <WeatherChange as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::WeatherChange)
                .map_err(ServerDecodeError::WeatherChange)?
        }
        <Remove as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::Remove)
                .map_err(ServerDecodeError::Remove)?
        }
        <PlayerUpdate as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::PlayerUpdate)
                .map_err(ServerDecodeError::PlayerUpdate)?
        }
        <MobileStatus as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::MobileStatus)
                .map_err(ServerDecodeError::MobileStatus)?
        }
        <MobileMove as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::MobileMove)
                .map_err(ServerDecodeError::MobileMove)?
        }
        <MobileIncoming as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::MobileIncoming)
                .map_err(ServerDecodeError::MobileIncoming)?
        }
        <WorldItem as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::WorldItem)
                .map_err(ServerDecodeError::WorldItem)?
        }
        <WalkAck as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::WalkAck)
                .map_err(ServerDecodeError::WalkAck)?
        }
        <WalkReject as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::WalkReject)
                .map_err(ServerDecodeError::WalkReject)?
        }
        _ => return Ok(None),
    };
    Ok(Some(decoded))
}

/// Decode packets that update a client window, cursor, or object presentation.
fn decode_interface_packet(
    id: u8,
    packet: &[u8],
    version: ClientVersion,
) -> Result<Option<ServerPacket>, ServerDecodeError> {
    let decoded = match id {
        <SpokenMessage as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::SpokenMessage)
                .map_err(ServerDecodeError::SpokenMessage)?
        }
        // Shares `0xC1` with nothing — the id is `LocalizedMessage`'s alone
        // — but had no arm here even though `EncodePacket` for it has stood
        // since `use_skill_button`'s "cannot be used directly" line: a
        // client asking for this cliloc read it as `Unknown` and dropped it
        // silently, which no e2e test had ever sent one over a real socket
        // to catch.
        <LocalizedMessage as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::LocalizedMessage)
                .map_err(ServerDecodeError::LocalizedMessage)?
        }
        <UnicodeMessage as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::UnicodeMessage)
                .map_err(ServerDecodeError::UnicodeMessage)?
        }
        <GumpDisplay as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::GumpDisplay)
                .map_err(ServerDecodeError::GumpDisplay)?
        }
        // The two halves of a tooltip. Both had encoders and neither had an
        // arm, so every property list this engine has ever sent reached its
        // own client as an undecoded id and was dropped — the shard's side
        // has been complete for a long time and nothing on this end asked.
        <TooltipRevision as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::TooltipRevision)
                .map_err(ServerDecodeError::TooltipRevision)?
        }
        <PropertyListReply as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::PropertyListReply)
                .map_err(ServerDecodeError::PropertyListReply)?
        }
        <OpenContainer as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::OpenContainer)
                .map_err(ServerDecodeError::OpenContainer)?
        }
        <AddToContainer as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::AddToContainer)
                .map_err(ServerDecodeError::AddToContainer)?
        }
        <ContainerContents as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::ContainerContents)
                .map_err(ServerDecodeError::ContainerContents)?
        }
        <OpenPaperdoll as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::OpenPaperdoll)
                .map_err(ServerDecodeError::OpenPaperdoll)?
        }
        <CorpseEquipment as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::CorpseEquipment)
                .map_err(ServerDecodeError::CorpseEquipment)?
        }
        // What a mobile is wearing, one layer at a time. Without this arm a
        // body was dressed once, by the `0x78` that drew it, and never
        // again — and a vendor's stock crate, which arrives as nothing but
        // a `0x2E` on layer `0x1A`, had no way in at all.
        <EquipUpdate as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::EquipUpdate)
                .map_err(ServerDecodeError::EquipUpdate)?
        }
        // The lift the shard refused. Purely local state depends on it: the
        // item drawn on the cursor is this client's own projection, and
        // nothing else ever says to put it back.
        <DragCancel as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::DragCancel)
                .map_err(ServerDecodeError::DragCancel)?
        }
        // The crosshair. The client enforces what a cursor may pick, so a
        // client that cannot read the request cannot raise one.
        <TargetCursor as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::TargetCursor)
                .map_err(ServerDecodeError::TargetCursor)?
        }
        <MultiTargetRequest as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::MultiTarget)
                .map_err(ServerDecodeError::MultiTarget)?
        }
        // The shop's two halves. Each names a different serial — the buy
        // list names the stock crate, the sell list the vendor — which is
        // why the window that joins them is keyed on neither by accident;
        // see `WorldView::apply`'s `0x24` arm.
        <BuyList as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::BuyList)
                .map_err(ServerDecodeError::BuyList)?
        }
        <SellList as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::SellList)
                .map_err(ServerDecodeError::SellList)?
        }
        _ => return Ok(None),
    };
    Ok(Some(decoded))
}

/// Decode combat feedback and other immediate state changes shown to a player.
fn decode_feedback_packet(
    id: u8,
    packet: &[u8],
    version: ClientVersion,
) -> Result<Option<ServerPacket>, ServerDecodeError> {
    let decoded = match id {
        // The stance that settled — the answer to the paperdoll's toggle,
        // and the same five bytes the client asked with. Decoded through
        // the same type both directions share; there is nothing in the
        // packet to say which way it was travelling.
        <WarMode as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::WarMode)
                .map_err(ServerDecodeError::WarMode)?
        }
        <AttackTarget as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::AttackTarget)
                .map_err(ServerDecodeError::AttackTarget)?
        }
        <HealthBar as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::Health)
                .map_err(ServerDecodeError::Health)?
        }
        <PlaySound as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::PlaySound)
                .map_err(ServerDecodeError::PlaySound)?
        }
        // The arrow's flight: combat already sends it when a shot lands,
        // NPC or player, so this is the client's other half of that packet.
        <GraphicalEffect as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::Effect)
                .map_err(ServerDecodeError::Effect)?
        }
        <PlayMusic as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::PlayMusic)
                .map_err(ServerDecodeError::PlayMusic)?
        }
        <Animation as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::Animation)
                .map_err(ServerDecodeError::Animation)?
        }
        <NewAnimation as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::NewAnimation)
                .map_err(ServerDecodeError::NewAnimation)?
        }
        <DeathStatus as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::DeathStatus)
                .map_err(ServerDecodeError::DeathStatus)?
        }
        <DeathAnimation as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(ServerPacket::DeathAnimation)
                .map_err(ServerDecodeError::DeathAnimation)?
        }
        // Both `0x3A`s. The id routes them together and the type byte tells
        // them apart, which is why this is the one arm that decodes into a
        // decision rather than into a variant — see `SkillsPacket`.
        <SkillsPacket as DecodePacket>::ID => {
            match decode_server(packet, version).map_err(ServerDecodeError::Skills)? {
                SkillsPacket::WholeList(list) => ServerPacket::SkillsFull(list),
                SkillsPacket::OneLine(line) => ServerPacket::SkillUpdate(line),
            }
        }
        // "You may go." A client that could not read this would sit on the
        // paperdoll's Log Out button with nothing happening, which is
        // exactly what the packet exists to prevent.
        <LogoutAck as DecodePacket>::ID => {
            decode_server(packet, version)
                .map(|LogoutAck| ServerPacket::LogoutAck(LogoutAck))
                .map_err(ServerDecodeError::LogoutAck)?
        }
        _ => return Ok(None),
    };
    Ok(Some(decoded))
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
    /// `0x4F` did not decode.
    LightLevel(DecodeError),
    /// `0x65` did not decode.
    WeatherChange(DecodeError),
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
    /// `0x89` did not decode.
    CorpseEquipment(DecodeError),
    /// `0x72` did not decode.
    WarMode(DecodeError),
    /// `0xAA` did not decode.
    AttackTarget(DecodeError),
    /// `0xA1` did not decode.
    Health(DecodeError),
    /// `0x70` did not decode.
    Effect(DecodeError),
    /// `0x54` did not decode.
    PlaySound(DecodeError),
    /// `0x6D` did not decode.
    PlayMusic(DecodeError),
    /// `0x6E` did not decode.
    Animation(DecodeError),
    /// `0xE2` did not decode.
    NewAnimation(DecodeError),
    /// `0x2C` did not decode.
    DeathStatus(DecodeError),
    /// `0xAF` did not decode.
    DeathAnimation(DecodeError),
    /// `0xD1` did not decode.
    LogoutAck(DecodeError),
    /// `0x3A` did not decode — either of the two, since the id is shared and
    /// which one it was is a fact from inside the body that failed to be read.
    Skills(DecodeError),
    /// `0xC1` did not decode.
    LocalizedMessage(DecodeError),
    /// `0x2E` did not decode.
    EquipUpdate(DecodeError),
    /// `0x27` did not decode.
    DragCancel(DecodeError),
    /// `0x6C` did not decode.
    TargetCursor(DecodeError),
    /// `0x99` did not decode.
    MultiTarget(DecodeError),
    /// `0x74` did not decode.
    BuyList(DecodeError),
    /// `0x9E` did not decode.
    SellList(DecodeError),
    /// `0xDC` did not decode.
    TooltipRevision(DecodeError),
    /// `0xBF 0x1D` did not decode.
    DesignRevision(DecodeError),
    /// `0xBF 0xE001` did not decode.
    AuthorityNotice(DecodeError),
    /// `0xBF 0xE003` did not decode.
    ChunkData(DecodeError),
    /// `0xBF 0xE004` did not decode.
    WorldNotice(DecodeError),
    /// `0xBF 0xE005` did not decode.
    PublishNotice(DecodeError),
    /// `0xBF 0xE006` did not decode.
    ChunkRefused(DecodeError),
    /// `0xBF 0xE008` did not decode.
    ChangesReply(DecodeError),
    /// `0xBF 0xE00A` did not decode.
    MapEditReply(DecodeError),
    /// `0xBF 0xE00B` did not decode.
    SwingTiming(DecodeError),
    /// `0xBF 0xE00C` did not decode.
    HarvestToolVisual(DecodeError),
    /// `0xBF 0xE00D` did not decode.
    HarvestPreview(DecodeError),
    /// `0xBF 0xE00E` did not decode.
    HarvestRefused(DecodeError),
    /// `0xBF 0xE00F` did not decode.
    HarvestCompleted(DecodeError),
    CombatActionPhase(DecodeError),
    CombatActionEnded(DecodeError),
    CombatActionBalked(DecodeError),
    CombatActionStage(DecodeError),
    /// `0xD6` did not decode.
    PropertyListReply(DecodeError),
    /// A `0xBF` subcommand `0x06` did not decode. One variant for all four,
    /// because which of them it was is a fact from inside the body that failed
    /// to be read — the same reasoning `Skills` has for its shared `0x3A`.
    Party(DecodeError),
    /// `0xBF` subcommand `0x04` did not decode.
    CloseGump(DecodeError),
    /// `0xBF` subcommand `0xE016` did not decode.
    CraftCatalogue(DecodeError),
    /// `0xBF` subcommand `0xE017` did not decode.
    CraftWorkbench(DecodeError),
    /// `0xBF` subcommand `0x1B` did not decode.
    SpellbookContent(DecodeError),
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
            Self::LightLevel(error) => ("0x4F light level", error),
            Self::WeatherChange(error) => ("0x65 weather change", error),
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
            Self::TooltipRevision(error) => ("0xDC tooltip revision", error),
            Self::DesignRevision(error) => ("0xBF 0x1D design revision", error),
            Self::AuthorityNotice(error) => ("0xBF 0xE001 authority notice", error),
            Self::ChunkData(error) => ("0xBF 0xE003 chunk data", error),
            Self::WorldNotice(error) => ("0xBF 0xE004 world notice", error),
            Self::PublishNotice(error) => ("0xBF 0xE005 publish notice", error),
            Self::ChunkRefused(error) => ("0xBF 0xE006 chunk refused", error),
            Self::ChangesReply(error) => ("0xBF 0xE008 changes reply", error),
            Self::MapEditReply(error) => ("0xBF 0xE00A map-edit reply", error),
            Self::SwingTiming(error) => ("0xBF 0xE00B swing timing", error),
            Self::HarvestToolVisual(error) => ("0xBF 0xE00C harvest tool visual", error),
            Self::HarvestPreview(error) => ("0xBF 0xE00D harvest preview", error),
            Self::HarvestRefused(error) => ("0xBF 0xE00E harvest refusal", error),
            Self::HarvestCompleted(error) => ("0xBF 0xE00F harvest completion", error),
            Self::CombatActionPhase(error) => ("0xBF 0xE010 combat action phase", error),
            Self::CombatActionEnded(error) => ("0xBF 0xE011 combat action end", error),
            Self::CombatActionBalked(error) => ("0xBF 0xE012 combat action balk", error),
            Self::CombatActionStage(error) => ("0xBF 0xE013 combat action stage", error),
            Self::PropertyListReply(error) => ("0xD6 property list", error),
            Self::Party(error) => ("0xBF 0x06 party", error),
            Self::CloseGump(error) => ("0xBF 0x04 close gump", error),
            Self::CraftCatalogue(error) => ("0xBF 0xE016 craft catalogue", error),
            Self::CraftWorkbench(error) => ("0xBF 0xE017 craft workbench", error),
            Self::SpellbookContent(error) => ("0xBF 0x1B spellbook content", error),
            Self::OpenContainer(error) => ("0x24 open container", error),
            Self::AddToContainer(error) => ("0x25 add to container", error),
            Self::ContainerContents(error) => ("0x3C container contents", error),
            Self::OpenPaperdoll(error) => ("0x88 paperdoll", error),
            Self::CorpseEquipment(error) => ("0x89 corpse equipment", error),
            Self::WarMode(error) => ("0x72 war mode", error),
            Self::AttackTarget(error) => ("0xAA attack target", error),
            Self::Health(error) => ("0xA1 health bar", error),
            Self::Effect(error) => ("0x70 graphical effect", error),
            Self::PlaySound(error) => ("0x54 play sound", error),
            Self::PlayMusic(error) => ("0x6D play music", error),
            Self::Animation(error) => ("0x6E animation", error),
            Self::NewAnimation(error) => ("0xE2 new animation", error),
            Self::DeathStatus(error) => ("0x2C death status", error),
            Self::DeathAnimation(error) => ("0xAF death animation", error),
            Self::LogoutAck(error) => ("0xD1 logout ack", error),
            Self::Skills(error) => ("0x3A skills", error),
            Self::LocalizedMessage(error) => ("0xC1 localized message", error),
            Self::EquipUpdate(error) => ("0x2E equip update", error),
            Self::DragCancel(error) => ("0x27 drag cancel", error),
            Self::TargetCursor(error) => ("0x6C target cursor", error),
            Self::MultiTarget(error) => ("0x99 multi target", error),
            Self::BuyList(error) => ("0x74 buy list", error),
            Self::SellList(error) => ("0x9E sell list", error),
        };
        write!(f, "{name}: {error}")
    }
}

impl std::error::Error for ServerDecodeError {
}

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
        0x99 => {
            return Some(crate::target::multi_target_length(
                version.supports(Feature::HsPackets),
            ));
        }
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
        0x65 => WeatherChange::LENGTH,
        0x54 => <PlaySound as EncodePacket>::LENGTH,
        0x55 => <LoginComplete as EncodePacket>::LENGTH,
        0x6C => TargetCursor::LENGTH,
        0x6D => <PlayMusic as EncodePacket>::LENGTH,
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
        0x89 => CorpseEquipment::LENGTH,
        0x8C => <Relay as EncodePacket>::LENGTH,
        0x9E => SellList::LENGTH,
        0xA1 => HealthBar::LENGTH,
        0xA8 => <ShardList as EncodePacket>::LENGTH,
        0xA9 => <CharacterList as EncodePacket>::LENGTH,
        0xAA => AttackTarget::LENGTH,
        0xAE => UnicodeMessage::LENGTH,
        0xAF => <DeathAnimation as EncodePacket>::LENGTH,
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
        // The AoS property list, and the one id in this table with no
        // `ServerPacket` variant behind it: a shard writes it as bytes through
        // `PropertyList::finish` (`state::send_property_list`), so nothing in
        // the enum would ever have put it here. It belongs here all the same —
        // this table is what says where the *next* packet starts, and a length
        // it does not know is not a packet dropped but a connection ended
        // (`Connection::poll`'s `FrameError::UnknownPacket`). Opening a shop
        // sends one of these per stocked item, which is how a purchase used to
        // take the whole session down with it.
        0xD6 => Variable,
        // A designed house's picture, and the second id here with no
        // `ServerPacket` variant behind it: `WorldState::design_detail_packet`
        // writes it as bytes through `DesignDetail::encode`, so nothing in the
        // enum would ever have put it here either.
        //
        // It belongs here for `0xD6`'s reason exactly, and the reason is worth
        // restating because this table has now been short an id twice: a length
        // this table does not know is **not a packet skipped, it is a connection
        // ended**. The shard that sends one and the client that cannot frame one
        // are the same workspace, so the first designed house in view would have
        // dropped every one of our own clients that walked past it — while the
        // classic client, which has its own table, drew it perfectly.
        0xD8 => Variable,
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
    frame_packet(
        buffer,
        |id| server_packet_length(id, version),
        MAX_SERVER_PACKET_SIZE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::encode_packet;
    use crate::serial::Serial;
    use crate::target::{
        MultiOffset,
        TargetKind,
    };
    use crate::wire::{
        AuthKey,
        CursorId,
    };

    fn version() -> ClientVersion {
        ClientVersion::new(7, 0, 45, 65)
    }

    /// Every variant of [`ServerPacket`], named once — and the compiler is what
    /// holds this list to the enum.
    ///
    /// This exists because [`one_of_each`] used to *say* it held one of each and
    /// nothing checked it: ten of the sixty-two were missing, and each one
    /// missing is a packet outside `every_packet_frames_to_its_own_length`, the
    /// only oracle [`server_packet_length`] has. A wrong entry there is a dropped
    /// connection rather than a dropped packet, so "somebody will remember" is
    /// not a good enough guarantee.
    ///
    /// The two halves below are what make the claim checkable. `variant_name` is
    /// a `match` with no wildcard, so a variant added to the enum and not to this
    /// list does not compile; `VARIANT_NAMES` is built from the same list, so it
    /// cannot drift from it. What is left — that the fixture actually carries one
    /// of each — is `the_fixture_holds_one_of_every_variant` below.
    macro_rules! every_variant {
        ($($variant:ident,)*) => {
            /// Every variant's name, in declaration order.
            const VARIANT_NAMES: &[&str] = &[$(stringify!($variant),)*];

            /// Which variant a packet is, so a test can say which one is absent.
            fn variant_name(packet: &ServerPacket) -> &'static str {
                match packet {
                    $(ServerPacket::$variant(_) => stringify!($variant),)*
                }
            }
        };
    }

    every_variant![
        TargetCursor,
        MultiTarget,
        WarMode,
        AttackTarget,
        Health,
        PlaySound,
        Animation,
        NewAnimation,
        SwingTiming,
        HarvestToolVisual,
        HarvestPreview,
        HarvestRefused,
        HarvestCompleted,
        CombatActionPhase,
        CombatActionEnded,
        CombatActionBalked,
        CombatActionStage,
        Effect,
        HuedEffect,
        LoginDenied,
        ShardList,
        Relay,
        CharacterList,
        DeleteReject,
        CharacterListUpdate,
        PlayerStart,
        PlayerUpdate,
        DeathStatus,
        DeathAnimation,
        WalkAck,
        WalkReject,
        LoginComplete,
        LightLevel,
        WeatherChange,
        PlayMusic,
        SeasonChange,
        LogoutAck,
        MapChange,
        Remove,
        OpenPaperdoll,
        CorpseEquipment,
        MobileStatus,
        MobileMove,
        MobileIncoming,
        StatLocks,
        WorldItem,
        DragCancel,
        EquipUpdate,
        OpenContainer,
        AddToContainer,
        ContainerContents,
        BuyList,
        SellList,
        DesignRevision,
        AuthorityNotice,
        ChunkData,
        WorldNotice,
        PublishNotice,
        ChunkRefused,
        ChangesReply,
        MapEditReply,
        TooltipRevision,
        PropertyListReply,
        PartyMemberList,
        PartyRemoveMember,
        PartyTextMessage,
        PartyInvitation,
        SkillsFull,
        SkillUpdate,
        SpokenMessage,
        LocalizedMessage,
        UnicodeMessage,
        ContextMenu,
        SpellbookContent,
        CloseGump,
        CraftCatalogue,
        CraftWorkbench,
        GumpDisplay,
    ];

    /// One of every variant, which is checked rather than asserted: see
    /// [`every_variant`] and `the_fixture_holds_one_of_every_variant`.
    ///
    /// Every test below runs over this list, so a variant absent from it is a
    /// variant whose id, whose declared length and whose framing nothing looks
    /// at.
    fn one_of_each() -> Vec<ServerPacket> {
        let serial = Serial::new(0x0000_002A).unwrap();
        let effect = GraphicalEffect {
            kind:            crate::feedback::EffectKind::Moving,
            from:            Some(serial),
            to:              None,
            art:             crate::wire::Graphic(0x36D4),
            from_point:      crate::world::Point::new(1, 2, 3),
            to_point:        crate::world::Point::new(4, 5, 6),
            speed:           7,
            duration:        0,
            fixed_direction: false,
            explode:         false,
        };
        vec![
            ServerPacket::TargetCursor(TargetCursor {
                cursor_id: CursorId(1),
                kind:      TargetKind::Object,
            }),
            ServerPacket::MultiTarget(MultiTargetRequest {
                cursor_id: CursorId(2),
                kind:      TargetKind::Location,
                multi:     crate::wire::MultiId(0x0064),
                offset:    MultiOffset::default(),
            }),
            ServerPacket::WarMode(WarMode { war: true }),
            ServerPacket::AttackTarget(AttackTarget { target: Some(serial) }),
            ServerPacket::Health(HealthBar::exact(serial, 100, 50)),
            ServerPacket::PlaySound(PlaySound {
                sound: crate::wire::SoundId(0x28),
                at:    crate::world::Point::new(1, 2, 3),
            }),
            ServerPacket::Animation(Animation {
                serial,
                action: 1,
                frame_count: crate::feedback::AnimationFrameCount(5),
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
            ServerPacket::SwingTiming(SwingTiming {
                serial,
                duration: crate::feedback::SwingDuration(5_000),
            }),
            ServerPacket::HarvestToolVisual(HarvestToolVisual {
                serial,
                graphic: crate::wire::Graphic(0x0F43),
                hue: crate::wire::Hue(0x0481),
                layer: crate::wire::Layer(1),
            }),
            ServerPacket::HarvestPreview(HarvestPreview {
                cursor_id: CursorId(0x2A),
                serial,
                action: 13,
                frame_count: crate::feedback::AnimationFrameCount(6),
                duration: crate::feedback::SwingDuration(4_800),
                cycles: 3,
            }),
            ServerPacket::HarvestRefused(HarvestRefused { serial }),
            ServerPacket::HarvestCompleted(HarvestCompleted { serial }),
            ServerPacket::CombatActionPhase(CombatActionPhase {
                actor:  serial,
                target: serial,
                kind:   crate::feedback::CombatActionKind::Swing,
                phase:  crate::feedback::ActionPhase::Releasing {
                    impact_in: crate::feedback::SwingDuration(1_500),
                },
            }),
            ServerPacket::CombatActionEnded(CombatActionEnded {
                actor:   serial,
                outcome: crate::feedback::CombatActionOutcome::Interrupted(
                    crate::feedback::InterruptReason::TargetGone,
                ),
            }),
            ServerPacket::CombatActionBalked(CombatActionBalked {
                actor: serial,
                balk:  crate::feedback::BalkState::Blocked(crate::feedback::InterruptReason::OutOfReach),
            }),
            ServerPacket::CombatActionStage(CombatActionStage {
                actor: serial,
                stage: crate::feedback::ActionStage::Load,
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
                    name:         "Britannia".to_owned(),
                    percent_full: crate::login::PercentFull::clamped(10),
                    timezone:     5,
                    address:      std::net::Ipv4Addr::new(127, 0, 0, 1),
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
                starts:     Vec::new(),
                flags:      crate::login::CharacterListFlags::NONE,
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
            ServerPacket::DeathAnimation(DeathAnimation {
                killed:  serial,
                corpse:  Serial::new(0x4000_0003),
                running: true,
            }),
            ServerPacket::WalkAck(WalkAck {
                sequence:  crate::world::StepSequence(1),
                notoriety: crate::mobile::Notoriety::Innocent,
            }),
            ServerPacket::WalkReject(WalkReject {
                sequence: crate::world::StepSequence(1),
                position: crate::world::Point::new(1475, 1774, 0),
                facing:   crate::direction::Facing::walking(crate::direction::Direction::South),
            }),
            ServerPacket::LoginComplete(LoginComplete),
            ServerPacket::LightLevel(LightLevel {
                level: crate::world::Light(0),
            }),
            ServerPacket::WeatherChange(crate::world::WeatherChange {
                weather:     crate::world::Weather::Rain,
                intensity:   64,
                temperature: 12,
            }),
            ServerPacket::PlayMusic(PlayMusic {
                track: crate::world::MusicId(11),
            }),
            ServerPacket::SeasonChange(SeasonChange {
                season:     crate::world::Season::Spring,
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
            ServerPacket::CorpseEquipment(CorpseEquipment {
                corpse: serial,
                items:  vec![crate::items::CorpseEquipmentItem {
                    layer: crate::wire::Layer::TORSO,
                    item:  Serial::new(0x4000_0001).unwrap(),
                }],
            }),
            ServerPacket::MobileStatus(MobileStatus {
                serial,
                name: "Lord British".to_owned(),
                hits: crate::mobile::Vitals {
                    current: 100,
                    max:     100,
                },
                female: false,
                strength: 100,
                dexterity: 90,
                intelligence: 80,
                stamina: crate::mobile::Vitals {
                    current: 90,
                    max:     90,
                },
                mana: crate::mobile::Vitals {
                    current: 80,
                    max:     80,
                },
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
                serial:   crate::serial::Serial::new(0x4000_0001).unwrap(),
                graphic:  crate::wire::Graphic(0x0EED),
                payload:  crate::items::WorldItemPayload::Stack(crate::items::ItemAmount(1)),
                position: crate::world::Point::new(1000, 2000, 5),
                hue:      crate::wire::Hue::NONE,
                light:    None,
                flags:    crate::items::ItemFlags::NONE,
            }),
            ServerPacket::DragCancel(crate::items::DragCancel {
                reason: crate::items::DragCancelReason::OutOfRange,
            }),
            ServerPacket::EquipUpdate(crate::items::EquipUpdate {
                item:    crate::serial::Serial::new(0x4000_0002).unwrap(),
                graphic: crate::wire::Graphic(0x13B9),
                layer:   crate::wire::Layer(1),
                mobile:  crate::serial::Serial::new(0x0000_0001).unwrap(),
                hue:     crate::wire::Hue(0x0021),
            }),
            // `0x24` and `0x25` are two lengths each, and the version picks —
            // which is why neither is an `EncodePacket` and why both being
            // outside this list left `open_container_length` and
            // `add_to_container_length` with no oracle at all.
            ServerPacket::OpenContainer(OpenContainer {
                container: crate::serial::Serial::new(0x4000_0001).unwrap(),
                gump:      crate::wire::Graphic(0x003C),
            }),
            ServerPacket::AddToContainer(AddToContainer {
                item:      crate::containers::ContainedItem {
                    serial:  crate::serial::Serial::new(0x4000_0002).unwrap(),
                    graphic: crate::wire::Graphic(0x0EED),
                    amount:  crate::items::ItemAmount(3),
                    at:      crate::gump::GumpPoint::new(44, 65),
                    grid:    crate::containers::GridSlot(7),
                    hue:     crate::wire::Hue::NONE,
                },
                container: crate::serial::Serial::new(0x4000_0001).unwrap(),
            }),
            ServerPacket::ContainerContents(crate::containers::ContainerContents {
                container: Some(crate::serial::Serial::new(0x4000_0001).unwrap()),
                items:     Vec::new(),
            }),
            ServerPacket::BuyList(crate::vendor::BuyList {
                container: crate::serial::Serial::new(0x4000_0010).unwrap(),
                lines:     vec![crate::vendor::BuyLine {
                    price: 3,
                    name:  "black pearl".to_owned(),
                }],
            }),
            ServerPacket::SellList(crate::vendor::SellList {
                vendor: crate::serial::Serial::new(0x0000_0BBB).unwrap(),
                lines:  vec![crate::vendor::SellLine {
                    serial:  crate::serial::Serial::new(0x4000_0033).unwrap(),
                    graphic: crate::wire::Graphic(0x0F7A),
                    hue:     crate::wire::Hue::NONE,
                    amount:  crate::items::ItemAmount(20),
                    price:   2,
                    name:    "black pearl".to_owned(),
                }],
            }),
            ServerPacket::TooltipRevision(crate::properties::TooltipRevision {
                serial: crate::serial::Serial::new(0x0000_00AB).unwrap(),
                hash:   0x1234_5678,
            }),
            // `0xD6` and `0xD8` are both in the framing table because it was
            // short an id twice; this is the half of that pair which has a
            // variant to encode.
            ServerPacket::PropertyListReply(PropertyListReply {
                serial:  crate::serial::Serial::new(0x0000_00AB).unwrap(),
                hash:    0x1234_5678,
                entries: vec![crate::properties::PropertyEntry {
                    cliloc:    crate::wire::ClilocId(1_042_971),
                    arguments: "a katana".to_owned(),
                }],
            }),
            ServerPacket::DesignRevision(crate::design::DesignRevision {
                serial:   crate::serial::RawSerial(0x4000_0100),
                revision: crate::design::Revision(7),
            }),
            ServerPacket::PartyMemberList(PartyMemberList {
                members: vec![serial, crate::serial::Serial::new(0x0000_002B).unwrap()],
            }),
            ServerPacket::PartyRemoveMember(PartyRemoveMember {
                removed: crate::serial::Serial::new(0x0000_002B).unwrap(),
                members: vec![serial],
            }),
            ServerPacket::PartyTextMessage(PartyTextMessage {
                to_all: true,
                from:   serial,
                text:   "on my way".to_owned(),
            }),
            ServerPacket::PartyInvitation(PartyInvitation { leader: serial }),
            ServerPacket::SkillsFull(crate::skill::SkillsFull {
                entries: vec![crate::skill::SkillEntry {
                    id:    0,
                    value: 755,
                    base:  700,
                    lock:  crate::skill::SkillLock::Locked,
                    cap:   1000,
                }],
            }),
            ServerPacket::SkillUpdate(crate::skill::SkillUpdate {
                entry: crate::skill::SkillEntry {
                    id:    25,
                    value: 501,
                    base:  501,
                    lock:  crate::skill::SkillLock::Up,
                    cap:   1000,
                },
            }),
            ServerPacket::SpokenMessage(crate::speech::SpokenMessage {
                serial:  crate::serial::Serial::new(0x0000_0002),
                graphic: Some(crate::wire::Graphic(0x0190)),
                mode:    crate::speech::TalkMode::Regular,
                hue:     crate::wire::Hue(0x0384),
                font:    crate::speech::Font(3),
                name:    "British".to_owned(),
                text:    "hail".to_owned(),
            }),
            ServerPacket::LocalizedMessage(crate::speech::LocalizedMessage {
                serial:    None,
                graphic:   None,
                mode:      crate::speech::TalkMode::Regular,
                hue:       crate::wire::Hue(0x03B2),
                font:      crate::speech::Font(3),
                cliloc:    crate::wire::ClilocId(1_042_764),
                name:      "System".to_owned(),
                arguments: "Iolo".to_owned(),
            }),
            ServerPacket::UnicodeMessage(crate::speech::UnicodeMessage {
                serial:   crate::serial::Serial::new(0x0000_0002),
                graphic:  Some(crate::wire::Graphic(0x0190)),
                mode:     crate::speech::TalkMode::Regular,
                hue:      crate::wire::Hue(0x0384),
                font:     crate::speech::Font(3),
                language: "PTB".to_owned(),
                name:     "Cidadão".to_owned(),
                text:     "olá".to_owned(),
            }),
            ServerPacket::ContextMenu(crate::context::ContextMenu {
                serial:  crate::serial::Serial::new(0x0000_00AB).unwrap(),
                entries: vec![crate::context::ContextMenuEntry {
                    cliloc: crate::wire::ClilocId(3_000_362),
                    flags:  crate::context::ContextMenuFlags::NONE,
                }],
            }),
            ServerPacket::SpellbookContent(crate::spellbook::SpellbookContent {
                serial:  crate::serial::Serial::new(0x4000_0001).unwrap(),
                graphic: crate::wire::Graphic(0x0EFA),
                offset:  1,
                content: 1,
            }),
            ServerPacket::CloseGump(crate::gump::CloseGump {
                gump_id: crate::gump::GumpId(0x0051_0001),
                button:  crate::gump::ButtonId::CLOSE_BOX,
            }),
            ServerPacket::CraftCatalogue(crate::craft::CraftCatalogue {
                gump_id: crate::gump::GumpId(0x0051_0001),
                rows:    vec![crate::craft::CraftCatalogueRow {
                    button:           8,
                    result:           crate::wire::Graphic(0x13EB),
                    result_hue:       crate::wire::Hue::NONE,
                    result_item_kind: Some(crate::item_kind::ItemKindId(4)),
                    name:             crate::wire::ClilocId(1_022_036),
                    skill:            crate::wire::ClilocId(1_044_067),
                    skill_min:        300,
                    ready:            true,
                    weapon:           None,
                    components:       vec![crate::craft::CraftCatalogueComponent {
                        item_kind: Some(crate::item_kind::ItemKindId(1)),
                        material:  Some(crate::item_kind::MaterialId(1)),
                        graphic:   crate::wire::Graphic(0x1BF2),
                        hue:       crate::wire::Hue::NONE,
                        name:      crate::wire::ClilocId(1_045_000),
                        amount:    3,
                    }],
                }],
            }),
            ServerPacket::CraftWorkbench(crate::craft::CraftWorkbench {
                gump_id:             crate::gump::GumpId(0x0051_0001),
                title:               crate::craft::CraftText::Literal("Blacksmithy".to_owned()),
                groups:              vec![crate::craft::CraftWorkbenchGroup {
                    button:   1,
                    name:     crate::craft::CraftText::Cliloc(crate::wire::ClilocId(1_044_010)),
                    selected: true,
                }],
                selected_material:   None,
                tool_uses:           Some(50),
                tool_carried:        true,
                required_facilities: 3,
                present_facilities:  3,
                notice:              None,
                materials_button:    None,
                refresh_button:      14,
                cancel_button:       84,
                page:                crate::craft::CraftWorkbenchPage::Items {
                    recipes: vec![crate::craft::CraftWorkbenchRecipe {
                        make_button:       Some(2),
                        details_button:    Some(3),
                        result:            crate::craft::CraftWorkbenchComponent {
                            item_kind: Some(crate::item_kind::ItemKindId(4)),
                            graphic:   crate::wire::Graphic(0x13EB),
                            hue:       crate::wire::Hue::NONE,
                            name:      crate::craft::CraftText::Literal("Longsword".to_owned()),
                            amount:    1,
                            carried:   None,
                        },
                        skills:            vec![(
                            crate::craft::CraftText::Literal("Blacksmithy".to_owned()),
                            300,
                        )],
                        components:        Vec::new(),
                        use_all_resources: false,
                        markable:          true,
                    }],
                },
            }),
            ServerPacket::GumpDisplay(crate::gump::GumpDisplay {
                serial:  crate::gump::GumpKey::STANDALONE,
                gump_id: crate::gump::GumpId(0x0051_0001),
                at:      crate::gump::GumpPoint::new(75, 25),
                layout:  "{ page 0 }".to_owned(),
                lines:   Vec::new(),
            }),
            ServerPacket::AuthorityNotice(crate::access::AuthorityNotice {
                level: crate::access::AccessLevel::GameMaster,
            }),
            // The one variable-length 0xBF this engine invented, and the reason
            // it is here: a fragment's length is its blob's, so the framing
            // oracle below is the only thing that checks the two agree.
            ServerPacket::ChunkData(crate::chunks::ChunkData {
                facet:    crate::world::Facet(0),
                at:       crate::chunks::ChunkAt { x: 12, y: 34 },
                revision: crate::chunks::WorldRevision(2),
                fragment: crate::chunks::Fragment::new(0, 1).expect("one of one"),
                inflated: crate::chunks::InflatedLength(12_568),
                blob:     vec![0x78, 0x9C, 0x03, 0x00],
            }),
            ServerPacket::WorldNotice(crate::chunks::WorldNotice {
                facet:    crate::world::Facet(0),
                blocks:   crate::chunks::FacetBlocks { wide: 896, down: 512 },
                revision: crate::chunks::WorldRevision(1),
                world:    Some(crate::world::WorldId(0x0123_4567_89AB_CDEF)),
            }),
            ServerPacket::PublishNotice(crate::chunks::PublishNotice {
                facet:    crate::world::Facet(0),
                revision: crate::chunks::WorldRevision(2),
                changes:  crate::chunks::Changes::These(vec![crate::chunks::ChunkAt { x: 2, y: 2 }]),
            }),
            ServerPacket::ChunkRefused(crate::chunks::ChunkRefused {
                facet:  crate::world::Facet(0),
                at:     crate::chunks::ChunkAt { x: 900, y: 0 },
                reason: crate::chunks::Refusal::PastTheEdge,
            }),
            // The second variable-length 0xBF of ours, and named chunks rather
            // than `Everything` for the same reason the fragment above carries a
            // blob: the arm whose length varies is the one worth framing.
            ServerPacket::ChangesReply(crate::chunks::ChangesReply {
                facet:    crate::world::Facet(0),
                revision: crate::chunks::WorldRevision(3),
                changes:  crate::chunks::Changes::These(vec![
                    crate::chunks::ChunkAt { x: 21, y: 25 },
                    crate::chunks::ChunkAt { x: 22, y: 25 },
                ]),
            }),
            ServerPacket::MapEditReply(crate::mapedit::MapEditReply {
                facet:    crate::world::Facet(0),
                revision: crate::chunks::WorldRevision(4),
                outcome:  crate::mapedit::MapEditOutcome::Accepted,
            }),
        ]
    }

    #[test]
    fn the_fixture_holds_one_of_every_variant() {
        // The other half of `every_variant!`: that macro makes the *list* of
        // variants impossible to get wrong, and this makes the fixture cover it.
        // Failing here means a variant is encoded by nothing below — not that it
        // is broken, which is worse, because nothing would say either way.
        let present: Vec<&str> = one_of_each().iter().map(variant_name).collect();
        let missing: Vec<&&str> = VARIANT_NAMES
            .iter()
            .filter(|name| !present.contains(name))
            .collect();
        assert!(
            missing.is_empty(),
            "one_of_each is short {} of the {} variants: {missing:?}",
            missing.len(),
            VARIANT_NAMES.len()
        );
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
    fn a_large_private_packet_uses_the_wire_limit_not_the_gateway_limit() {
        // The complete craft catalogue is currently about 23 KB. That is a
        // legitimate server response even though the hostile client-input cap
        // is 18 KB; the packet's u16 field is the bound in this direction.
        let length = 23_483usize;
        let [high, low] = (length as u16).to_be_bytes();
        let mut bytes = vec![0xBF, high, low];
        bytes.resize(length, 0);

        assert_eq!(
            frame_server_packet(&bytes, version()),
            Ok(Frame::Complete(length))
        );
    }

    #[test]
    fn the_hand_written_packets_are_in_the_table() {
        // 0xB9 and 0x76 have no variant at all, so `one_of_each` cannot reach
        // them however complete it is — and a client that cannot frame them
        // stops dead on the first login. 0x24 is here for a smaller reason: it
        // *is* a variant now, but `encode_open_container` writes its own header
        // rather than going through `frame_body`, and a second writer is a
        // second chance to disagree with the table.
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
                width:  6144,
                height: 4096,
            },
        );
        assert_eq!(
            frame_server_packet(&bytes, version()),
            Ok(Frame::Complete(bytes.len()))
        );
    }

    /// Everything a shop is made of, written by the shard and read back the way
    /// the client reads it.
    ///
    /// A packet with an encoder, an id in the framing table and *no arm in
    /// `decode`* is the failure this covers, and it is the quietest one this
    /// file can have: the stream stays in step, so nothing breaks — the client
    /// simply never learns what it was told. A vendor was three packets of it
    /// (`0x2E`, `0x74`, `0x9E`), which is why the buy window opened empty while
    /// every byte of its catalogue had arrived.
    #[test]
    fn the_packets_a_shop_is_made_of_decode_as_themselves() {
        for packet in [
            ServerPacket::EquipUpdate(crate::items::EquipUpdate {
                item:    Serial::new(0x4000_0002).unwrap(),
                graphic: crate::wire::Graphic(0x0E3F),
                layer:   crate::wire::Layer(0x1A),
                mobile:  Serial::new(0x0000_002A).unwrap(),
                hue:     crate::wire::Hue(0x0021),
            }),
            ServerPacket::BuyList(crate::vendor::BuyList {
                container: Serial::new(0x4000_0010).unwrap(),
                lines:     vec![
                    crate::vendor::BuyLine {
                        price: 3,
                        name:  "black pearl".to_owned(),
                    },
                    crate::vendor::BuyLine {
                        price: 12,
                        name:  "longsword".to_owned(),
                    },
                ],
            }),
            ServerPacket::SellList(crate::vendor::SellList {
                vendor: Serial::new(0x0000_002A).unwrap(),
                lines:  vec![crate::vendor::SellLine {
                    serial:  Serial::new(0x4000_0011).unwrap(),
                    graphic: crate::wire::Graphic(0x0F7B),
                    hue:     crate::wire::Hue::NONE,
                    amount:  crate::items::ItemAmount(4),
                    price:   2,
                    name:    "black pearl".to_owned(),
                }],
            }),
            ServerPacket::DragCancel(crate::items::DragCancel {
                reason: crate::items::DragCancelReason::OutOfRange,
            }),
            ServerPacket::TargetCursor(TargetCursor {
                cursor_id: CursorId(0x0000_0007),
                kind:      TargetKind::Location,
            }),
        ] {
            let bytes = packet.encode(version());
            assert_eq!(
                ServerPacket::decode(&bytes, version()),
                Ok(Some(packet.clone())),
                "{packet:?}"
            );
        }
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

    /// **A designed house's picture, framed — and it was not, for one commit.**
    ///
    /// `0xD8` is the second packet a shard writes as bytes with no
    /// `ServerPacket` variant behind it, and it went in without this table
    /// gaining a row. The failure that would have caused is worth naming
    /// exactly, because it is not the one it looks like: the *classic* client
    /// has its own table and would have drawn the house perfectly, while every
    /// one of **our own** clients walking past the first designed house on the
    /// shard would have dropped its connection — a bug that reads as "our client
    /// is broken" and lives entirely in the shard's protocol crate.
    ///
    /// The same shape as the `0xD6` failure the roadmap records, in the same
    /// table, one id along.
    #[test]
    fn a_design_is_framed_even_though_no_variant_carries_it() {
        use crate::design::{
            DesignDetail,
            DesignTile,
            Revision,
        };
        use crate::serial::RawSerial;
        use crate::wire::Graphic;

        let tiles = [DesignTile {
            graphic: Graphic(0x0006),
            dx:      0,
            dy:      0,
            dz:      0,
        }];
        let bytes = DesignDetail {
            serial:   RawSerial(0x4000_0001),
            revision: Revision(1),
            response: true,
            tiles:    &tiles,
        }
        .encode(|_| false);

        assert_eq!(bytes[0], DesignDetail::ID);
        assert_eq!(
            server_packet_length(DesignDetail::ID, version()),
            Some(PacketLength::Variable),
            "a length this table does not know ends the connection"
        );
        assert_eq!(
            frame_server_packet(&bytes, version()),
            Ok(Frame::Complete(bytes.len()))
        );
    }

    #[test]
    fn an_id_this_engine_never_sends_is_fatal() {
        // Not a silent skip: without a length there is no way to find where the
        // next packet starts, so the connection is over.
        //
        // The example is `0x1E`, the pre-6.0 animation packet `0x6E` replaced.
        // It has been `0xD6` and then `0x99` before now, and both of those had
        // to be moved along when this engine started sending them — which is the
        // lesson: the id here should be one that *cannot* be implemented later,
        // not merely one that has not been. An obsolete packet stays obsolete.
        assert_eq!(server_packet_length(0x1E, version()), None);
        assert_eq!(
            frame_server_packet(&[0x1E, 0x00, 0x05], version()),
            Err(FrameError::UnknownPacket(0x1E))
        );
    }

    /// The half of the table that is not the enum: a shard writes some packets
    /// as bytes, and the framer has to know their length even though no
    /// `ServerPacket` variant ever will. The property list took a session down
    /// before it was in the table — opening a vendor's window sends one per
    /// stocked item.
    ///
    /// **It is no longer the whole of that set**, which is the point of the
    /// sibling test below: this comment used to say it was, and the next
    /// byte-written packet was added without anyone re-reading it.
    #[test]
    fn a_property_list_is_framed_even_though_no_variant_carries_it() {
        let mut list = crate::properties::PropertyList::new(Serial::new(0x4000_0001).unwrap());
        list.add(crate::wire::ClilocId(1_020_000));
        let (bytes, _hash) = list.finish();

        assert_eq!(bytes[0], crate::properties::PropertyList::ID);
        assert_eq!(
            server_packet_length(crate::properties::PropertyList::ID, version()),
            Some(PacketLength::Variable)
        );
        assert_eq!(
            frame_server_packet(&bytes, version()),
            Ok(Frame::Complete(bytes.len()))
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
                    name:         "OpenShard".to_owned(),
                    percent_full: crate::login::PercentFull::clamped(12),
                    timezone:     5,
                    address:      std::net::Ipv4Addr::new(192, 168, 11, 6),
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
                    max:     100,
                },
                female: false,
                strength: 100,
                dexterity: 90,
                intelligence: 80,
                stamina: crate::mobile::Vitals {
                    current: 90,
                    max:     90,
                },
                mana: crate::mobile::Vitals {
                    current: 80,
                    max:     80,
                },
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
                    serial:  Serial::new(0x4000_0001).unwrap(),
                    graphic: crate::wire::Graphic(0x1517),
                    layer:   crate::wire::Layer(0x05),
                    hue:     crate::wire::Hue(0x0021),
                }],
            }),
            ServerPacket::WorldItem(crate::items::WorldItem {
                serial:   Serial::new(0x4000_00AB).unwrap(),
                graphic:  crate::wire::Graphic(0x0EED),
                payload:  crate::items::WorldItemPayload::Stack(crate::items::ItemAmount(500)),
                position: crate::world::Point::new(1000, 2000, -5),
                hue:      crate::wire::Hue(0x0021),
                light:    None,
                flags:    crate::items::ItemFlags::NONE,
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
    fn a_flying_arrow_round_trips_through_server_packet_decode() {
        // `0x70` had an encoder and no decoder at all until this — a shard could
        // send an archer's shot but no client (including this repo's own) could
        // ever read it back. `HuedEffect`/`0xC0` deliberately stays undecoded:
        // nothing sends it yet.
        let serial = Serial::new(0x0000_002A).unwrap();
        let target = Serial::new(0x0000_002B).unwrap();
        let packet = ServerPacket::Effect(GraphicalEffect {
            kind:            crate::feedback::EffectKind::Moving,
            from:            Some(serial),
            to:              Some(target),
            art:             crate::wire::Graphic(0x0F42),
            from_point:      crate::world::Point::new(1000, 1000, 0),
            to_point:        crate::world::Point::new(1005, 1000, 0),
            speed:           18,
            duration:        1,
            fixed_direction: false,
            explode:         false,
        });
        let bytes = packet.encode(version());
        assert_eq!(ServerPacket::decode(&bytes, version()), Ok(Some(packet)));
    }

    #[test]
    fn an_old_client_gets_the_hue_flagged_equipment_layout_back() {
        // Before 7.0.33.1 an item's hue rides on a stolen bit in the graphic
        // rather than a fixed field; the decoder must read the same layout it
        // was handed, not the one `version()` elsewhere in this module implies.
        let old = ClientVersion::new(7, 0, 33, 0);
        let packet = ServerPacket::MobileIncoming(MobileIncoming {
            serial:    Serial::new(0x0000_0002).unwrap(),
            body:      crate::wire::Graphic(0x0190),
            position:  crate::world::Point::new(1475, 1774, -5),
            facing:    crate::direction::Facing::walking(crate::direction::Direction::South),
            hue:       crate::wire::Hue(0x83EA),
            flags:     crate::mobile::StatusFlags::NONE,
            notoriety: crate::mobile::Notoriety::Innocent,
            equipment: vec![
                crate::mobile::Equipment {
                    serial:  Serial::new(0x4000_0001).unwrap(),
                    graphic: crate::wire::Graphic(0x1517),
                    layer:   crate::wire::Layer(0x05),
                    hue:     crate::wire::Hue(0x0021),
                },
                crate::mobile::Equipment {
                    serial:  Serial::new(0x4000_0002).unwrap(),
                    graphic: crate::wire::Graphic(0x1F03),
                    layer:   crate::wire::Layer(0x0D),
                    hue:     crate::wire::Hue::NONE,
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
            serial:        Serial::new(0x0001_2345).unwrap(),
            name:          "Lord British".to_owned(),
            hits:          crate::mobile::Vitals {
                current: 100,
                max:     100,
            },
            female:        false,
            strength:      100,
            dexterity:     90,
            intelligence:  80,
            stamina:       crate::mobile::Vitals {
                current: 90,
                max:     90,
            },
            mana:          crate::mobile::Vitals {
                current: 80,
                max:     80,
            },
            gold:          1234,
            armor:         0,
            weight:        14,
            max_weight:    390,
            stat_cap:      225,
            followers:     0,
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
                sequence:  crate::world::StepSequence(0x2A),
                notoriety: crate::mobile::Notoriety::Murderer,
            }),
            ServerPacket::WalkReject(WalkReject {
                sequence: crate::world::StepSequence(0xFF),
                position: crate::world::Point::new(1475, 1774, -5),
                facing:   crate::direction::Facing::running(crate::direction::Direction::NorthWest),
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
            sequence:  crate::world::StepSequence(1),
            notoriety: crate::mobile::Notoriety::Invulnerable,
        });
        assert_eq!(
            ServerPacket::decode(&sent.encode(old), old),
            Ok(Some(ServerPacket::WalkAck(WalkAck {
                sequence:  crate::world::StepSequence(1),
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
            serial:   Serial::new(0x0000_002A).unwrap(),
            body:     crate::wire::Graphic(0x0190),
            position: crate::world::Point::new(1000, 1200, -10),
            facing:   crate::direction::Facing::running(crate::direction::Direction::SouthEast),
            map:      crate::world::MapSize::BRITANNIA,
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
            starts:     vec![crate::login::StartLocation {
                area:               "Britain".to_owned(),
                name:               "Castle Britannia".to_owned(),
                position:           crate::world::Point::new(1475, 1770, 20),
                map:                crate::world::Facet(0),
                description_cliloc: crate::wire::ClilocId(1075072),
            }],
            flags:      crate::login::CharacterListFlags::TOOLTIPS,
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
            starts:     Vec::new(),
            flags:      crate::login::CharacterListFlags::NONE,
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
    fn the_environment_packets_round_trip_through_the_client_reader() {
        // These are state rather than commands: a second copy simply replaces
        // the first, so decoding them at the protocol boundary is enough for a
        // view to keep one current answer.
        let bytes = ServerPacket::LightLevel(crate::world::LightLevel {
            level: crate::world::Light(0),
        })
        .encode(version());
        assert_eq!(
            ServerPacket::decode(&bytes, version()),
            Ok(Some(ServerPacket::LightLevel(crate::world::LightLevel {
                level: crate::world::Light(0),
            })))
        );
        let weather = ServerPacket::WeatherChange(crate::world::WeatherChange {
            weather:     crate::world::Weather::Snow,
            intensity:   80,
            temperature: 3,
        });
        assert_eq!(
            ServerPacket::decode(&weather.encode(version()), version()),
            Ok(Some(weather))
        );
    }

    /// The two packets the paperdoll's buttons wait for, round-tripped through
    /// the client's own reader. Both used to fall through to `Ok(None)`: the
    /// shard answered the war toggle and the client threw the answer away, so
    /// the toggle's picture could never move.
    #[test]
    fn the_answers_a_paperdoll_waits_for_are_decoded() {
        for war in [true, false] {
            let bytes = ServerPacket::WarMode(WarMode { war }).encode(version());
            assert_eq!(
                ServerPacket::decode(&bytes, version()),
                Ok(Some(ServerPacket::WarMode(WarMode { war })))
            );
        }
        let bytes = ServerPacket::LogoutAck(crate::world::LogoutAck).encode(version());
        assert_eq!(
            ServerPacket::decode(&bytes, version()),
            Ok(Some(ServerPacket::LogoutAck(crate::world::LogoutAck)))
        );
    }

    /// `docs/combat.md` P4/D9: the client never had a decoder for `0x2C`, so a
    /// death that arrived over the wire dropped silently and the world never
    /// greyed. Round-tripped like [`the_answers_a_paperdoll_waits_for_are_decoded`]
    /// above.
    #[test]
    fn death_status_is_decoded() {
        for dead in [true, false] {
            let bytes = ServerPacket::DeathStatus(DeathStatus { dead }).encode(version());
            assert_eq!(
                ServerPacket::decode(&bytes, version()),
                Ok(Some(ServerPacket::DeathStatus(DeathStatus { dead })))
            );
        }
    }

    #[test]
    fn combat_feedback_packets_are_decoded() {
        let serial = Serial::new(0x2A).unwrap();
        let target = ServerPacket::AttackTarget(AttackTarget { target: Some(serial) });
        assert_eq!(
            ServerPacket::decode(&target.encode(version()), version()),
            Ok(Some(target))
        );

        let clear = ServerPacket::AttackTarget(AttackTarget { target: None });
        assert_eq!(
            ServerPacket::decode(&clear.encode(version()), version()),
            Ok(Some(clear))
        );

        let health = ServerPacket::Health(HealthBar::scaled(serial, 120, 45));
        assert_eq!(
            ServerPacket::decode(&health.encode(version()), version()),
            Ok(Some(health))
        );

        let sound = ServerPacket::PlaySound(PlaySound {
            sound: crate::wire::SoundId(0x0137),
            at:    crate::world::Point::new(1475, 1774, -5),
        });
        assert_eq!(
            ServerPacket::decode(&sound.encode(version()), version()),
            Ok(Some(sound))
        );

        let music = ServerPacket::PlayMusic(PlayMusic {
            track: crate::world::MusicId(9),
        });
        assert_eq!(
            ServerPacket::decode(&music.encode(version()), version()),
            Ok(Some(music))
        );

        let animation = ServerPacket::NewAnimation(NewAnimation {
            serial,
            animation_type: 0,
            action: 0,
            delay: 0,
        });
        assert_eq!(
            ServerPacket::decode(&animation.encode(version()), version()),
            Ok(Some(animation))
        );
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
