//! What the world knows about a connection, apart from the character on it.
//!
//! # Why a connection is a row and not a component
//!
//! Everything the world knew about a client used to hang off its *entity* — the
//! [`Client`](crate::components::Client) component — which quietly made "has a
//! character" the precondition for "can be spoken to". A connection sitting on
//! the character screen has no entity, so the world could not address it at all:
//! [`WorldState::send_packet`](crate::WorldState::send_packet) resolves the
//! client version through the player table and drops the packet, silently, when
//! the lookup misses.
//!
//! That is why the character screen is answered by the binary today rather than
//! out of a tick, and it is the first thing in the way of moving it in — see
//! `docs/connection_state.md`. A connection is a thing in its own right, with a
//! lifetime that starts before its character exists and ends after it is gone, so
//! it gets a row of its own keyed by
//! [`ConnectionId`](openshard_gateway::ConnectionId).

use openshard_entities::EntityId;
use openshard_protocol::access::AccessLevel;
use openshard_protocol::identity::AccountName;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::world::{
    Light,
    MusicId,
};

use crate::runtime::{
    CraftGumpContext,
    GuildGumpContext,
    HeldItem,
    HouseGumpContext,
    QuestGumpContext,
    TargetPurpose,
};

/// The derived half of a player's status bar, kept to compare against next time.
///
/// Only the fields the refresh pass computes: the stats and pools have their own
/// re-send (off a buff landing), and the name never moves.
///
/// Lives on the connection because that is what it is *about*: not what the
/// character is, but what this particular client was last told it is. Two clients
/// looking at the same numbers were told them at different moments.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StatusSnapshot {
    /// Gold counted in the pack.
    pub gold:      u32,
    /// Armour summed off what is worn.
    pub armor:     u16,
    /// What the pack and everything in it weighs.
    pub weight:    u16,
    /// Pets and the mount under the rider.
    pub followers: u8,
}

/// One connected client, as the world sees it.
///
/// Opened by `Command::Authenticated` when the login conversation hands the
/// connection over, and closed by `Command::Disconnect`. A row exists for a
/// connection that is playing nothing — that is the point of it.
///
/// # Not a session
///
/// The *session* — which character this connection is playing, and whether it is
/// in the world yet — is the binary's, and stays there: the packet router has to
/// answer "may this reach the world" synchronously, and the world answers no
/// synchronous question. This is only what the world itself has to remember about
/// a socket.
///
/// # Why the transient state is here and not in a map
///
/// Everything below the identity fields used to be a map on
/// [`WorldState`](crate::WorldState) or on `World`, keyed by connection, and
/// `disconnect` cleared each one by name. That list was hand-written, so the
/// map added without a line beside it leaked and nothing caught it — and four
/// of them, the gump tables, had already done exactly that while their own docs
/// claimed to be cleared on logout. A field on the row cannot be forgotten:
/// removing the row takes it, whether or not anybody remembered it exists.
// Deliberately not `PartialEq`: two connections are never the same connection,
// and once the row carries what the client is in the middle of doing there is no
// question a comparison would answer. `ConnectionId` is the identity.
#[derive(Clone, Debug)]
pub struct Connection {
    /// What the client claims to be. Every feature gate and every encoder reads
    /// it, and this is the only place it lives: the game socket never states its
    /// version, so this is what the login socket carried across on the auth key.
    pub version: ClientVersion,
    /// Whose account this connection authenticated as.
    ///
    /// The character screen's packets name a character but never the account it
    /// belongs to — `0x5D` echoes a name, `0x83` an index — so answering any of
    /// them out of a tick means the world knowing whose list to read. It comes
    /// off the login state machine at the hand-off and never changes: a socket
    /// authenticates once.
    pub account: AccountName,
    /// The staff authority this account's characters play with.
    ///
    /// Re-derived from the account at every login and never saved with a
    /// character, so it lives with the connection rather than the roster. Carried
    /// here because entering a character is a tick's job now, and the entity's
    /// `Access` component is written from this.
    pub access:  AccessLevel,

    /// The item on this client's cursor, and where it was so a cancelled drag can
    /// put it back.
    ///
    /// An item here is off the ground and out of everyone's
    /// [`seen`](crate::WorldState::seen) — in limbo until a `0x08` lands it, which
    /// is why a connection that goes while dragging has to be noticed at all. An
    /// `Option` and not a map entry because a cursor holds one thing: that was
    /// always what the old map's `get` returning `None` meant.
    pub held:        Option<HeldItem>,
    /// The derived status numbers last sent, so the refresh pass sends only what
    /// changed. `None` before the first pass has run for this connection.
    pub last_status: Option<StatusSnapshot>,
    /// The light level last sent, the remembered half of the ambient diff.
    ///
    /// Forgotten with the row, and that matters: a connection id can be reused,
    /// and a reconnect inheriting the last one's remembered light would be told
    /// nothing — it would sit in daylight inside a cave.
    pub last_light:  Option<Light>,
    /// The music track this client is hearing, so a region crossing that does not
    /// change it does not restart it. Re-sending `0x6D` with the same id starts
    /// the track over.
    pub last_music:  Option<MusicId>,

    /// The targeting cursor this client has up, and what the click is for.
    ///
    /// A `.tele` raises one, a skill raises one; the `0x6C` answer reads it to
    /// know what to do with the spot. On the connection rather than the mobile
    /// because a cursor is a thing on a screen — every site that raises one
    /// already refused to do so without a `Client`, which is that invariant
    /// written out longhand at six call sites instead of held by the type.
    pub pending_target:          Option<TargetPurpose>,
    /// The quest dialog this client has open, and on which page.
    ///
    /// A gump exists only while somebody is looking at it, and a reply naming a
    /// window this side never drew is a reply to nothing — which is the whole
    /// reason the context is kept rather than trusted off the packet.
    pub quest_gump:              Option<QuestGumpContext>,
    /// The guild window this client has open, on which page, and what its rows
    /// meant. See [`GuildGumpContext`] for why the rows are remembered here.
    pub guild_gump:              Option<GuildGumpContext>,
    /// The craft window this client has open, on which category and material.
    ///
    /// Carries more weight than the quest log's: the selected category, the
    /// chosen metal and the tool in hand all live here and never in the packet,
    /// so a reply cannot name a material the player did not pick.
    pub craft_gump:              Option<CraftGumpContext>,
    /// Most recent compact catalogue context requested on this connection.
    pub craft_catalogue_request: u32,
    /// The runebook this client has open.
    pub runebook_gump:           Option<EntityId>,
    /// The gate this client has a destination list open for. The `craft_gump`
    /// shape, and for the same reason: the reply carries a button and a switch,
    /// never *which* gate asked.
    pub gate_gump:               Option<EntityId>,
    /// The healer this client has a "wouldst thou like to be resurrected?"
    /// confirm open for. The `gate_gump` shape: a reply carries only a button, so
    /// *which* healer asked has to live here, not in the packet.
    pub healer_gump:             Option<EntityId>,
    /// The house sign this client has open, and what its rows meant.
    ///
    /// The `guild_gump` shape rather than the `gate_gump` one, because a house
    /// window has rows: three lists of people, each row offering to drop whoever
    /// is on it. Which serial row four named is what this side remembers
    /// drawing, and never the number in the packet.
    pub house_gump:              Option<HouseGumpContext>,
}

impl Connection {
    /// A connection that has just been handed over by the login conversation:
    /// known identity, and nothing done yet.
    pub fn new(version: ClientVersion, account: AccountName, access: AccessLevel) -> Self {
        Self {
            version,
            account,
            access,
            held: None,
            last_status: None,
            last_light: None,
            last_music: None,
            pending_target: None,
            quest_gump: None,
            guild_gump: None,
            craft_gump: None,
            craft_catalogue_request: 0,
            runebook_gump: None,
            gate_gump: None,
            healer_gump: None,
            house_gump: None,
        }
    }

    /// Record who this connection is, keeping whatever it is in the middle of.
    ///
    /// The hand-off happens twice — once when the login conversation ends, once
    /// when a character enters, because a test may queue an `Enter` without ever
    /// having authenticated. Both write the same identity, read off the same auth
    /// key, so re-writing it is harmless; *replacing the row* would not be, and
    /// once the transient state moved here that stopped being a distinction
    /// without a difference.
    pub fn identify(&mut self, version: ClientVersion, account: AccountName, access: AccessLevel) {
        self.version = version;
        self.account = account;
        self.access = access;
    }
}
