//! What the server has shown us.
//!
//! The client's side of `World::seen`. The server remembers what is on each
//! client's screen because there is no "what can you see" packet; this is the
//! other end of that arrangement — a record of what arrived, never a guess
//! about what is there.
//!
//! It grows with the decoders. `0x1B`, `0x20`, `0x1D`, `0x77`, `0x78` and
//! `0x1A` are decoded, so the player, every other mobile and every ground item
//! this client has been shown are held here, and `0x1C` and `0xAE` are kept as
//! one journal of what has been said to it — see [`Heard`], which is why they
//! are one and not two. `0x11` (the player's paperdoll numbers) is held in
//! [`Player::status`], while its health value shares [`Player::hits`] with the
//! `0xA1` health-bar update so the two displays cannot disagree.
//!
//! Containers are here too: `0x24` opens a window over one, `0x3C` lists what
//! is inside and `0x25` adds to it. Which container a window is over and what
//! that container holds are two tables rather than one — see
//! [`WorldView::contents`] for the vendor that makes them separate.
//!
//! Two of those packets can name the client's own serial, and neither means
//! what it means about anybody else: a `0x78` about ourselves is the paperdoll
//! a shard sends exactly once at world entry, and a `0x77` about ourselves is
//! not a move at all. Both are routed by serial in [`WorldView::apply`], so
//! [`WorldView::mobiles`] holds only *other* mobiles, as its docs promise.

use std::collections::{BTreeMap, VecDeque};

use rustc_hash::FxHashMap;

use openshard_protocol::access::AccessLevel;
use openshard_protocol::chunks::WorldNotice;
use openshard_protocol::containers::ContainedItem;
use openshard_protocol::direction::Facing;
use openshard_protocol::gump::layout::{Element, parse};
use openshard_protocol::gump::{GumpId, GumpKey, GumpPoint};
use openshard_protocol::items::WorldItemPayload;
use openshard_protocol::mobile::{Equipment, Notoriety, PaperdollFlags, StatusFlags, Vitals};
use openshard_protocol::properties::PropertyEntry;
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::{Font, LocalizedMessage, SpokenMessage, TalkMode, UnicodeMessage};
use openshard_protocol::spellbook::SpellbookContent;
use openshard_protocol::target::TargetCursor;
use openshard_protocol::vendor::{BuyLine, SellLine};
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::{MapSize, PlayerStart, Point};

pub use openshard_client_model::{Skill, Status};

/// How many lines of speech the journal keeps.
///
/// A bound rather than a `Vec` that grows forever, because the thing this is
/// built for is a client that stays logged in: a virtual player standing in a
/// town square hears every line said near it, and nothing ever asks it to
/// forget. The number is what a scrollback is worth reading, not a memory
/// budget — the oldest line is dropped, silently, which is what a journal does.
pub const JOURNAL_LINES: usize = 256;

/// A vendor catalogue, keyed by the merchant's mobile serial.
///
/// The buy list itself names the stock crate while `0x24` opens a window on
/// the merchant.  Keeping both identities is what lets a client draw the
/// catalogue and later send a purchase naming the merchant.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VendorBuy {
    pub container: Serial,
    pub lines: Vec<BuyLine>,
}

/// A vendor's offer to buy items from this character.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VendorSell {
    pub lines: Vec<SellLine>,
}

/// The client's own character, as the server last described it.
///
/// Not `Copy`: it carries the equipment list, which is a `Vec`. That is the
/// price of the one packet a client hears about its own paperdoll from — see
/// [`Player::equipment`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Player {
    /// The serial everything else addresses this character by.
    pub serial: Serial,
    /// The body graphic.
    pub body: Graphic,
    /// Its hue. `0x1B` never carries one — see [`WorldView::entered`] — so this
    /// reads [`Hue::NONE`] until the first `0x20`.
    pub hue: Hue,
    /// Whether this body walks through others rather than round them.
    ///
    /// [`StatusFlags::IGNORE_MOBILES`] off the flag byte a `0x20` or our own
    /// `0x78` carries, and the whole of what this end reads that byte for. The
    /// shard sets the bit from its own body-blocking rule — staff, and the dead
    /// — so `clutter::crowd` does not have to re-derive who is exempt from a
    /// rule it can only guess at. `false` until the first packet that carries
    /// the byte, which is the same absence as `hue`.
    ///
    /// **A `bool` and not the byte**, which is what this was. Of the eight bits
    /// this one is the only one anything at this end answers from, and one of
    /// the other seven is [`WARMODE`](StatusFlags::WARMODE) — a second copy of
    /// [`war`](Self::war) that no packet updates in step with it, sitting in a
    /// field named `flags` where the next reader would find it first. A `0x72`
    /// moves the stance without a `0x20` behind it, so that copy is not merely
    /// unread: it is wrong for as long as the body stands still. Keeping a byte
    /// for the sake of the bits nobody reads is how a fact ends up with two
    /// homes; the day one of the seven is wanted it can be folded out the same
    /// way this one is.
    ///
    /// [`Mobile::flags`] does keep the byte, and the asymmetry is the point:
    /// *there* the byte is the stance's one home, because no `0x72` ever
    /// describes somebody else.
    pub walks_through_bodies: bool,
    /// Whether this character stands in war mode.
    ///
    /// **The one home for that fact.** It arrives two ways — a `0x72`, which is
    /// how the server answers the paperdoll's toggle, and the `0x88`'s own
    /// [`PaperdollFlags::WARMODE`] when a paperdoll is opened on us — and it is
    /// folded to this one field rather than left in whichever packet said it
    /// last. A second copy on [`Paperdoll`] is exactly the shape that draws a
    /// stale toggle: the window would answer from the flag byte it opened with
    /// while the stance had moved on.
    ///
    /// **Not the `0x20`'s own war bit**, which the shard does send and this end
    /// does not keep — see [`walks_through_bodies`](Self::walks_through_bodies).
    /// The same split the reference client makes: ClassicUO reads
    /// `Flags & WarMode` for [`Mobile`] and gives `PlayerMobile.InWarMode` a
    /// field of its own that only the `0x72` writes.
    pub war: bool,
    /// Whether this character is a ghost.
    ///
    /// The `0x2C` this end never had a decoder for until now — see
    /// `docs/combat.md`'s D9 and P4. It gates more than the tonemap's grey:
    /// no attack goes out from a ghost, and a ghost stands with no war stance
    /// even with [`war`](Self::war) still set, the same
    /// `!InWarMode || IsDead` the reference draws by.
    pub dead: bool,
    /// Whom the shard says this client is attacking.
    ///
    /// Not the hover and not the selection: the server answers an attack click
    /// with `0xAA`, and this is that answer kept apart from every local
    /// pointer fact.
    pub attacking: Option<Serial>,
    /// The hit-point pair the shard last stated for this character.
    ///
    /// `None` means "not said yet", not an empty bar. The server decides
    /// whether the pair is exact or scaled; this client only draws
    /// `current / max`.
    pub hits: Option<Vitals>,
    /// The paperdoll numbers the shard last stated for this character.
    ///
    /// `0x11` is only ever about the connection's own character. Its hit
    /// points deliberately stay in [`Player::hits`]: `0xA1` can refresh that
    /// one fact between status replies, and keeping a second copy here would
    /// make the health line and status window disagree.
    pub status: Option<Status>,
    /// Where it stands.
    pub position: Point,
    /// Which way it faces, and whether it is running.
    pub facing: Facing,
    /// What it is wearing, including the backpack it must be able to open.
    ///
    /// `0x1B` carries no equipment and neither does `0x20`, so this is empty
    /// until the server sends this client a `0x78` naming *its own* serial —
    /// which a shard does exactly once, at world entry, because the pass that
    /// reveals a mobile sends it to everyone except itself.
    pub equipment: Vec<Equipment>,
    /// What this character's skills stand at, by the wire's own skill id.
    ///
    /// On [`Player`] and nowhere else because a `0x3A` carries **no serial**:
    /// the shard answers on the connection, so the only body it can be about is
    /// the one at this end of it. A skill window opened over a stranger's
    /// paperdoll would draw these numbers, which is a fact about the packet and
    /// not a bug in the window.
    ///
    /// Keyed by the id rather than holding one, [`WorldView::containers`]'
    /// reason: nothing can file a line under a skill other than its own. The
    /// key is a bare `u8` because that is what the wire says and this crate has
    /// no files to check it against — the client's own `skills.mul` numbering is
    /// `openshard_uofiles::skills::SkillId`, and the join between the two
    /// happens where a window is laid out and both are in hand.
    ///
    /// Sparse on purpose: a `0x3A` delta may name a skill no whole list ever
    /// did, and a table pre-filled with zeroes would draw fifty-eight skills at
    /// zero for a shard that had sent nothing at all.
    pub skills: BTreeMap<u8, Skill>,
}

/// Another mobile, as `0x77` or `0x78` last described it.
///
/// Not the client's own character — see [`Player`] for that, and
/// [`WorldView::player`] for why the two are not the same type: `0x77`/`0x78`
/// cannot move the client's own body (Sphere's warning, kept in
/// `openshard_protocol::mobile::MobileMove`'s docs), so nothing here is ever
/// keyed by the player's serial.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Mobile {
    /// Its body graphic.
    pub body: Graphic,
    /// Where it stands.
    pub position: Point,
    /// Which way it faces, and whether it is running.
    pub facing: Facing,
    /// Its hue.
    pub hue: Hue,
    /// Poisoned, invisible, war mode.
    pub flags: StatusFlags,
    /// How to colour its health bar.
    pub notoriety: Notoriety,
    /// The hit-point pair the shard last stated for this mobile.
    ///
    /// Stored as sent: for strangers this is usually a 0-100 percentage, and
    /// no caller has to know that to draw a bar.
    pub hits: Option<Vitals>,
    /// What it is wearing.
    ///
    /// Only `0x78` carries this; a `0x77` move leaves it as it was; see
    /// [`WorldView::apply`].
    pub equipment: Vec<Equipment>,
}

impl Mobile {
    /// Whether this body stands in war mode.
    ///
    /// Read off [`flags`](Self::flags) rather than kept beside it: the byte is
    /// already here, and a `bool` folded out of it at every `0x77` would be the
    /// same fact in two shapes — with the packet that forgot to update one of
    /// them drawing a body in the wrong stance. Bit `0x40`, which is *not* the
    /// paperdoll's `0x01`; see
    /// [`StatusFlags::WARMODE`](openshard_protocol::mobile::StatusFlags::WARMODE).
    ///
    /// [`Player::war`] is the same question about our own body and cannot be
    /// this: no `0x77` or `0x78` ever describes it, so the answer there comes
    /// from the `0x72` and the `0x88` instead.
    #[must_use]
    pub fn war(&self) -> bool {
        self.flags.has(StatusFlags::WARMODE)
    }
}

/// A line said to this client — `0x1C` or `0xAE`, folded into one shape.
///
/// The two packets are one event told two ways: `0x1C` carries it in Latin-1,
/// `0xAE` in big-endian UTF-16 for the text `0x1C` cannot hold, and a client
/// that spoke `0xAD` gets its own words back as `0xAE` — see
/// `openshard_protocol::speech` module docs. A journal that held one of the
/// two packet types and hoped the other never arrived would leave a client's
/// own accented or non-Latin speech undecoded and invisible to itself, which
/// is the bug this type exists to close. Everything a renderer wants is here
/// regardless of which wire shape it came in as, so nothing downstream has to
/// match on the packet again.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Heard {
    /// The speaker, or `None` for the system.
    pub serial: Option<Serial>,
    /// The speaker's body graphic, or `None` for no mobile behind it.
    pub graphic: Option<Graphic>,
    /// How it is said.
    pub mode: TalkMode,
    /// The colour to draw it in.
    pub hue: Hue,
    /// The font to draw it in.
    pub font: Font,
    /// The speaker's name, or empty for the system.
    pub name: String,
    /// What was said.
    pub text: String,
}

impl From<&SpokenMessage> for Heard {
    /// `0x1C` — Latin-1 speech.
    fn from(message: &SpokenMessage) -> Self {
        Self {
            serial: message.serial,
            graphic: message.graphic,
            mode: message.mode,
            hue: message.hue,
            font: message.font,
            name: message.name.clone(),
            text: message.text.clone(),
        }
    }
}

impl From<&UnicodeMessage> for Heard {
    /// `0xAE` — Unicode speech.
    ///
    /// The four-byte language tag on the wire is dropped here rather than
    /// carried into the journal: nothing downstream reads it, and it names a
    /// property of the *sender* (what client locale sent this), not of the
    /// line itself — keeping it would be a field every future consumer has to
    /// notice does nothing.
    fn from(message: &UnicodeMessage) -> Self {
        Self {
            serial: message.serial,
            graphic: message.graphic,
            mode: message.mode,
            hue: message.hue,
            font: message.font,
            name: message.name.clone(),
            text: message.text.clone(),
        }
    }
}

/// How many of an item are in a stack. `openshard_protocol::items` leaves this
/// a bare `u16` — it is the wire's own currency there, one field among several
/// a codec reads in sequence — but here, sitting beside the already-typed
/// `graphic`/`position`/`hue`, an untyped count is the one field a reader
/// cannot place at a glance.
/// An item on the ground, as `0x1A` last described it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Item {
    /// Its graphic.
    pub graphic: Graphic,
    /// Stack size, or the dead body's graphic for a corpse marker.
    pub payload: WorldItemPayload,
    /// Where it lies.
    pub position: Point,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue: Hue,
}

/// A targeting cursor the shard has open, and the house drawn under it.
///
/// One value and not two `Option`s side by side, which is the shape this would
/// naturally have taken and the one `combat.md`'s D1 already learned to avoid:
/// the same state in two places is the same state one packet can forget to
/// refold. A `0x6C` arriving after a `0x99` writes `multi: None` because it
/// replaces the whole value, and there is no way to leave a house being drawn
/// under a cursor that is no longer about one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenTarget {
    /// What the shard asked for.
    pub cursor: TargetCursor,
    /// The multi the client draws under the pointer, for a `0x99`. `None` for
    /// an ordinary `0x6C`.
    pub multi: Option<openshard_protocol::wire::MultiId>,
}

/// Everything this client has been told about the world.
///
/// # There is no such thing as an empty one
///
/// A `WorldView` is built from the `0x1B` that puts a body in the world, so it
/// cannot exist before the client is in one. That is why nothing here is an
/// `Option`: "we are not in the world yet" is the absence of this whole value,
/// not a field inside it saying so.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorldView {
    /// This client's character.
    pub player: Player,
    /// How big the facet is. The client needs it to bound the map it draws.
    pub map: MapSize,
    /// Every other mobile this client has been shown, by serial.
    pub mobiles: FxHashMap<Serial, Mobile>,
    /// Every ground item this client has been shown, by serial.
    pub items: FxHashMap<Serial, Item>,
    /// What has been said to this client, oldest first, capped at
    /// [`JOURNAL_LINES`].
    ///
    /// One [`Heard`] per packet — `0x1C` or `0xAE` — so there is nothing for a
    /// second type to reconcile, and a renderer that wants the hue and the
    /// font it was told to draw in still has them.
    ///
    /// It is history, not state — which is why nothing removes from it except
    /// the cap. The first thing it holds, and the reason it exists at all, is
    /// the shard saying it is going away (`docs/shutdown.md` S3): a client that
    /// could decode that line and then dropped it was told and did not listen.
    pub journal: VecDeque<Heard>,
    /// The dialogs the server has opened here and this client has not answered,
    /// oldest first.
    ///
    /// A window is state on *both* ends: the server remembers it drew one and
    /// waits for the `0xB1`, and until that arrives the client is the only place
    /// that knows the window is up. That is why this is a list and not a
    /// snapshot replaced by each packet — a shard may have several open at once,
    /// and nothing on the wire says "these are all of them".
    ///
    /// Removed by [`gump_closed`](Self::gump_closed) when this client answers,
    /// which is the one thing here the server does not tell us — see its docs.
    pub gumps: Vec<OpenGump>,
    /// The gump art of every container the shard has opened a window for
    /// (`0x24`), by container serial.
    ///
    /// A map and not one entry: a player has a backpack open and opens a chest,
    /// and both windows stand. Removed by
    /// [`container_closed`](Self::container_closed) — closing one is a click,
    /// exactly as closing a gump is, and the wire has no packet for it either.
    pub containers: FxHashMap<Serial, Graphic>,
    /// What each container holds, as far as this client has been told
    /// (`0x3C`, then `0x25` per addition), in the order the shard listed it.
    ///
    /// # Deliberately not a field of the window
    ///
    /// The wire keys the two on different things and a vendor is where that
    /// shows: the shop window is a `0x24` naming the *vendor*, and the goods are
    /// a `0x3C` naming the crate worn on its shop layer. Fold the contents into
    /// the window and there is nowhere to put a listing whose container has no
    /// window — which is a thing the shard sends, on purpose.
    ///
    /// A `Vec` and not a map keyed by serial, because painter's order is data
    /// here: a container's icons overlap, and the shard's order is the order the
    /// reference client draws them in.
    pub contents: FxHashMap<Serial, Vec<ContainedItem>>,
    /// The one target cursor the shard currently has open for this player.
    pub target: Option<OpenTarget>,
    /// Buy catalogues currently opened by NPC vendors, keyed by vendor serial.
    pub vendor_buys: FxHashMap<Serial, VendorBuy>,
    /// Sell catalogues currently opened by NPC vendors, keyed by vendor serial.
    pub vendor_sells: FxHashMap<Serial, VendorSell>,
    /// A `0x74` arrives just before the `0x24` that identifies its vendor.
    pending_vendor_buys: FxHashMap<Serial, Vec<BuyLine>>,
    /// The stock crate each vendor most recently wore on shop layer `0x1A`.
    ///
    /// This deliberately does not live only in [`Mobile::equipment`].  A
    /// vendor may send its shop `EquipUpdate` before the ordinary mobile
    /// update reaches this client; dropping that identity made the later buy
    /// list impossible to attach to the window even though every shop packet
    /// had arrived.
    vendor_stock: FxHashMap<Serial, Serial>,
    /// Whose paperdoll the shard has opened a window for (`0x88`), by the
    /// mobile's serial.
    ///
    /// # What is deliberately not here
    ///
    /// The equipment. A `0x88` carries a serial, a title and a flag byte and
    /// says nothing about what is worn, because the client already has that: a
    /// `0x78` dressed [`mobiles`](Self::mobiles), or [`player`](Self::player)
    /// when the mobile is us. So this table is the *window*, and what it draws
    /// is read out of the mobile it names — which is also what keeps a
    /// paperdoll standing open honest, since taking a hat off is a `0x78` and
    /// reaches the window without a packet of its own.
    ///
    /// Keyed by serial, so a second `0x88` about the same mobile replaces the
    /// title rather than stacking a window. Removed by
    /// [`paperdoll_closed`](Self::paperdoll_closed): closing one is a click,
    /// exactly as it is for a container and a gump.
    pub paperdolls: FxHashMap<Serial, Paperdoll>,
    /// The books this client has opened, keyed by book serial.  The spellbook
    /// content packet is the window's source of truth, not the bag it came
    /// from: books can be carried, equipped, or opened from the ground.
    pub spellbooks: FxHashMap<Serial, Spellbook>,
    /// The party this client is in, and who has asked it into one.
    ///
    /// One value, not a table: a mobile is in at most one party, and the shard
    /// says so with a roster rather than with deltas — every change re-sends the
    /// whole list, so this is replaced rather than edited.
    pub party: Party,
    /// The AoS tooltip this client knows about each object it has been shown.
    ///
    /// Filled by the two halves of the property protocol — a `0xDC` naming a
    /// revision, a `0xD6` carrying a list — and read by whatever draws a hover.
    /// Entries go when the object does, in [`forget`](Self::forget): a tooltip
    /// is about a thing on screen, and a serial the shard has taken away has no
    /// hover to answer.
    pub tooltips: FxHashMap<Serial, Tooltip>,
    /// Which revision the shard last named for each **designed** house.
    ///
    /// [`tooltips`](Self::tooltips)' twin, one question along: a `0xBF 0x1D`
    /// names a revision, a `0xD8` carries the shape, and asking for the second
    /// is this end's move. A classic house never gets an entry — its picture is
    /// in this client's own files and has no revision.
    ///
    /// **The shape itself is not here, and that is the layering rather than an
    /// omission.** A design is a list of `Component`s, which is a client-file
    /// type, and this crate is the *wire*: it has never depended on
    /// `openshard-uofiles` and must not start. So the view holds what the
    /// packets said, and whoever holds the client's files holds what was made of
    /// it — the same split that keeps `WorldView` free of art.
    pub designs: FxHashMap<Serial, u32>,
    /// What the shard says this character's authority is — the answer to "may I
    /// run a staff command", and the only thing that reads it is the completer
    /// on the speech line (`openshard_commands::StaffCommand::matching`).
    ///
    /// [`AccessLevel::Player`] until an
    /// [`AuthorityNotice`](openshard_protocol::access::AuthorityNotice) says
    /// otherwise, which the shard sends once on world entry. That is a *default*
    /// and not an unknown: authority is never granted by accident, so the client
    /// offers nothing until it is told, and a shard that never sends the packet
    /// (an older one, or somebody else's) leaves a completer that offers no staff
    /// words — which is right, since the vocabulary it would offer is this
    /// engine's own.
    ///
    /// It is never a *permission*: nothing here is checked before a line goes
    /// out, and the shard refuses what it refuses regardless of what this says.
    pub authority: AccessLevel,
    /// Which world the shard says this connection is standing in — the facet,
    /// its size in blocks, and the revision it is published at.
    ///
    /// [`authority`](Self::authority)'s twin: another `0xBF` of this engine's
    /// own, sent once on world entry, that nothing enforces and one thing reads.
    /// What reads it is a client that means to fetch the ground rather than
    /// open it off its own disk — see
    /// [`openshard_protocol::chunks`], and `to_the_client.md` for the plan.
    ///
    /// **`None` is a real answer and not an unknown**: a shard with no ground
    /// for the facet sends no notice, and so does a shard that predates the
    /// packet or belongs to somebody else. All three mean the same thing to this
    /// end — there is no world here to ask for — so they are one state rather
    /// than three.
    ///
    /// It is *not* the same question as [`map`](Self::map), which is the size in
    /// tiles the `0x1B` carried and which every client, ours or stock, is told.
    pub world: Option<WorldNotice>,
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Party {
    /// Everyone in it, leader first. Empty when this client is in no party.
    pub members: Vec<Serial>,
    /// Who has invited this client and is waiting on an answer.
    ///
    /// Independent of [`members`](Self::members): an invitation arrives while
    /// you are in no party, and the shard clears it by sending the roster you
    /// have just joined — which is what makes this a separate field rather than
    /// a state the roster could be in.
    pub invited_by: Option<Serial>,
}

impl Party {
    /// Who leads it, or `None` for a client in no party.
    #[must_use]
    pub fn leader(&self) -> Option<Serial> {
        self.members.first().copied()
    }

    /// Whether this client is in a party at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// What this client knows about one object's tooltip.
///
/// Two facts, kept apart because the shard has three tooltip modes and each
/// sends a different one of them. In `version` mode a revision arrives with the
/// object and the list only if asked; in `full` mode the list arrives unasked
/// and no revision ever does; in `off` mode neither comes. Folding them into a
/// single "do we have it" would make the second mode look permanently stale and
/// the client would ask, every hover, for a list it already held.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Tooltip {
    /// The revision the shard last named as current for this object (`0xDC`).
    ///
    /// `None` in `full` mode, where the shard never sends one.
    pub revision: Option<u32>,
    /// The revision [`entries`](Self::entries) were built at — the hash the
    /// `0xD6` that filled them carried. `None` until one arrives.
    pub held_revision: Option<u32>,
    /// The lines, newest answer wins, in the order the shard wrote them.
    ///
    /// Kept rather than cleared when a newer revision arrives, so a hover during
    /// the round trip draws the tooltip that is one edit out of date instead of
    /// a blank. A blank would read as "this has no name".
    pub entries: Vec<PropertyEntry>,
}

impl Tooltip {
    /// Whether this client should ask the shard for the list.
    ///
    /// # Why `(None, Some(_))` is not stale
    ///
    /// That is `full` mode's steady state: a list arrived unasked and nothing
    /// has contradicted it. Calling it stale would make every hover send a
    /// request the shard answers with the same bytes, forever.
    #[must_use]
    pub fn stale(&self) -> bool {
        match (self.revision, self.held_revision) {
            (Some(revision), held) => held != Some(revision),
            (None, None) => true,
            (None, Some(_)) => false,
        }
    }
}

/// A paperdoll window the shard has opened here.
///
/// Two fields and no equipment — see [`WorldView::paperdolls`]. The serial is
/// the key rather than a field, for [`WorldView::containers`]' reason: nothing
/// can hold a paperdoll for a mobile other than the one it is filed under.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Paperdoll {
    /// The title on the paperdoll's name plate: the character name plus any
    /// honorific the shard chose to include.  It is deliberately not called a
    /// mobile name: `0x88` is the sole authority for this display string.
    pub title: String,
    /// Whether this client may lift what is worn on this doll. The shard's
    /// answer and not a guess: it is set for your own paperdoll and for a pet's,
    /// and clear for a stranger's.
    ///
    /// The flag byte's *other* bit is not kept beside it. War mode is a fact
    /// about the body, not about the window over it, and a second `0x72` moves
    /// it while this record stands still — so it is folded into
    /// [`Player::war`], and this field is the whole of what the `0x88` says
    /// about the window it opened.
    pub can_lift: bool,
}

/// The contents the shard last sent for one open spellbook.
///
/// A spellbook's `0x24` only says that it is a book.  The usable spells arrive
/// separately in `0xBF 0x1B`, so they have their own table rather than being
/// inferred from a container redraw.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Spellbook {
    /// The book's graphic, retained for a later spell school.
    pub graphic: Graphic,
    /// The one-based spell number represented by bit zero.
    pub offset: u16,
    /// Bit `n` says this book holds spell `offset + n`.
    pub content: u64,
}

impl From<SpellbookContent> for Spellbook {
    fn from(content: SpellbookContent) -> Self {
        Self {
            graphic: content.graphic,
            offset: content.offset,
            content: content.content,
        }
    }
}

/// A dialog the server has opened on this client, layout already read.
///
/// The elements are parsed once, when the packet arrives, rather than every time
/// something draws: the layout string is what the wire carries and a list of
/// elements is what anything above wants, and re-parsing per frame would be the
/// same work sixty times a second. The lines travel beside them because an
/// element names its text by index — see [`Element::Label`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OpenGump {
    /// What the reply must echo. Opaque: the server chose it — see [`GumpKey`].
    pub key: GumpKey,
    /// Which dialog, and what the reply is routed by on the way back.
    pub gump_id: GumpId,
    /// Where on the screen the server asked for it.
    pub at: GumpPoint,
    /// What to draw, in the order the window draws it.
    pub elements: Vec<Element>,
    /// The text table the elements index into.
    pub lines: Vec<String>,
}

impl OpenGump {
    /// The line an element names, or `None` when the layout indexed past the
    /// table it travelled with.
    ///
    /// Absent rather than empty on purpose: a layout naming a line that is not
    /// there is a bug on the sending side, and a blank label would hide it. The
    /// caller decides whether to draw a placeholder or nothing at all.
    #[must_use]
    pub fn line(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(String::as_str)
    }
}

impl WorldView {
    /// A serial has one location. Destination packets do not always carry the
    /// corresponding `Remove` back to the client that performed the drag, so
    /// moving it must also retire any stale source entry in our snapshot.
    fn remove_from_containers(&mut self, serial: Serial, except: Option<Serial>) -> bool {
        let mut changed = false;
        for (container, contents) in &mut self.contents {
            if Some(*container) == except {
                continue;
            }
            let before = contents.len();
            contents.retain(|item| item.serial != serial);
            changed |= contents.len() != before;
        }
        changed
    }

    /// Equipment destinations likewise omit a matching `Remove` for the
    /// acting client.  Keep a serial in exactly one projection: worn, held in
    /// a container, or on the ground.
    fn remove_from_equipment(&mut self, serial: Serial) -> bool {
        let before = self.player.equipment.len();
        self.player.equipment.retain(|item| item.serial != serial);
        let mut changed = self.player.equipment.len() != before;
        for mobile in self.mobiles.values_mut() {
            let mobile_equipment_before = mobile.equipment.len();
            mobile.equipment.retain(|item| item.serial != serial);
            changed |= mobile.equipment.len() != mobile_equipment_before;
        }
        changed
    }

    /// The world as the entry packet described it: nobody else on screen yet.
    #[must_use]
    pub fn entered(start: PlayerStart) -> Self {
        Self {
            player: Player {
                serial: start.serial,
                body: start.body,
                hue: Hue::NONE,
                // A `0x1B` carries no flag byte. Staff learn they walk through
                // bodies from the `0x78` the shard sends them about themselves,
                // which is right behind it — see `enter.rs`.
                walks_through_bodies: false,
                // At peace until something says otherwise, which is what a
                // `0x1B` means: a session starts out of war, and a shard that
                // logs a character back in fighting says so with a `0x72`.
                war: false,
                // Same reasoning as `war`: a `0x1B` says nothing about death,
                // and a relogged ghost gets its own `0x2C` right behind it
                // (`enter.rs`'s `enter_ghost_state`).
                dead: false,
                attacking: None,
                hits: None,
                status: None,
                position: start.position,
                facing: start.facing,
                equipment: Vec::new(),
                // Nothing is trained until a `0x3A` says so — see
                // `Player::skills` for why an empty table is the honest start.
                skills: BTreeMap::new(),
            },
            map: start.map,
            mobiles: FxHashMap::default(),
            items: FxHashMap::default(),
            journal: VecDeque::new(),
            gumps: Vec::new(),
            containers: FxHashMap::default(),
            contents: FxHashMap::default(),
            target: None,
            vendor_buys: FxHashMap::default(),
            vendor_sells: FxHashMap::default(),
            pending_vendor_buys: FxHashMap::default(),
            vendor_stock: FxHashMap::default(),
            paperdolls: FxHashMap::default(),
            spellbooks: FxHashMap::default(),
            party: Party::default(),
            tooltips: FxHashMap::default(),
            designs: FxHashMap::default(),
            // Nothing until the shard says so — see the field's own doc.
            authority: AccessLevel::default(),
            // And nothing at all unless this shard has a world of its own to
            // hand over, which most do not.
            world: None,
        }
    }

    /// The shard is gone: put out everything it was the author of, and say so
    /// in the journal.
    ///
    /// # Why the world goes out rather than freezing
    ///
    /// Every table below is a statement the shard made, and the moment the
    /// connection ends none of them is a statement about anything: the bodies
    /// keep the poses they were drawn in, the bag stays open on contents that
    /// may already be somewhere else, and a window keeps offering to send a
    /// packet down a socket that is closed. A picture that goes on looking
    /// right is the expensive kind of wrong — a lost shard read as "the game
    /// got strange" for exactly as long as the last frame stayed convincing.
    ///
    /// # What is deliberately kept
    ///
    /// The **journal**, because the line this writes into it is the only thing
    /// on screen that says what happened, and clearing the log to announce
    /// something in the log is a joke the client would be playing on itself.
    /// The **player** and the **map**, because they are what the camera is
    /// anchored to and the map viewer's own state: with the body gone there is
    /// nothing to draw a frame around, and the window would go black rather
    /// than honest. What stops the body from being a lie is the caller's half —
    /// `App::walk` refuses to move it once the shard is lost, so it stands
    /// where the last packet left it.
    pub fn shard_lost(&mut self, reason: &str) {
        self.mobiles.clear();
        self.items.clear();
        self.contents.clear();
        self.containers.clear();
        self.gumps.clear();
        self.paperdolls.clear();
        self.spellbooks.clear();
        self.vendor_buys.clear();
        self.vendor_sells.clear();
        self.pending_vendor_buys.clear();
        self.vendor_stock.clear();
        // The tooltips go with the objects they described. Nothing here can be
        // hovered any more, and a name that outlived the shard that said it is
        // the same kind of convincing wrong picture as a body left mid-stride.
        self.tooltips.clear();
        // And the designs, for the same reason: a revision is about a house that
        // was on screen, and one that outlived its shard would make the next
        // shard's house at the same serial look up to date.
        self.designs.clear();
        self.target = None;
        self.heard(Heard {
            serial: None,
            graphic: None,
            // A system line has the shape the shard's own private messages
            // have — no speaker, muted grey, ordinary mode — because it is the
            // same kind of line, said by the one participant still in the room.
            mode: TalkMode::Regular,
            hue: Hue::SYSTEM,
            font: Font::DEFAULT,
            name: String::new(),
            text: format!("The shard is no longer answering: {reason}."),
        });
    }

    /// Forget a paperdoll window this client has just closed.
    ///
    /// [`container_closed`](Self::container_closed)'s twin, and the third time
    /// the same fact shows up: no packet closes a window. Nothing else is
    /// dropped with it — the equipment a paperdoll draws belongs to the mobile
    /// and outlives the window over it.
    ///
    /// Answers whether one was actually open.
    pub fn paperdoll_closed(&mut self, mobile: Serial) -> bool {
        self.paperdolls.remove(&mobile).is_some()
    }

    /// Forget a spellbook window this client has just closed.  A book does not
    /// send a close packet, so reopening it is the next `0xBF 0x1B` the shard
    /// sends after the player uses the item again.
    pub fn spellbook_closed(&mut self, book: Serial) -> bool {
        self.spellbooks.remove(&book).is_some()
    }

    /// Forget a container window this client has just closed.
    ///
    /// [`gump_closed`](Self::gump_closed)'s twin, and the same fact about the
    /// protocol: closing a window is a click and nothing on the wire carries it.
    /// The shard keeps its own list of who has what open (`WorldState`'s
    /// `open_containers`) and will keep pushing `0x25`s at a window this end has
    /// shut; dropping the *contents* as well is what makes those additions land
    /// nowhere instead of quietly rebuilding a window nobody can see.
    ///
    /// Answers whether a window was actually open, so a stale click can be told
    /// from a real close.
    pub fn container_closed(&mut self, container: Serial) -> bool {
        self.contents.remove(&container);
        self.containers.remove(&container).is_some()
    }

    /// Forget a dialog, whether the client just answered it or the server
    /// tore it down unprompted with `0xBF 0x04`.
    ///
    /// A reply button closes the window *on the client* — that is what the
    /// reference client does, and what the server assumes when it waits for
    /// one `0xB1` per window — so most closes are knowledge this end has and
    /// no packet carries, the same shape as
    /// [`player_stepped`](Self::player_stepped) learning a position from an
    /// ack that carries none. `CloseGump` is the one exception: a script or
    /// quest step dismissing its own window without a client reply and
    /// without redrawing it, so this end learns of it only from the wire.
    ///
    /// Answers whether anything was actually open under that id, so a caller can
    /// tell a real close from a stale click on a window the server already
    /// replaced.
    pub fn gump_closed(&mut self, gump_id: GumpId) -> bool {
        let before = self.gumps.len();
        self.gumps.retain(|gump| gump.gump_id != gump_id);
        self.gumps.len() != before
    }

    /// Write down a line the server said, dropping the oldest if the journal is
    /// full.
    ///
    /// Separate from [`apply`](Self::apply) only so the cap has one place to
    /// live: `0x1C` and `0xAE` both fold to [`Heard`] and land here, and
    /// whatever else learns to (a cliloc, an overhead message) lands here too
    /// rather than growing a second bound to keep in step.
    fn heard(&mut self, line: Heard) {
        if self.journal.len() == JOURNAL_LINES {
            self.journal.pop_front();
        }
        self.journal.push_back(line);
    }

    /// Record a localized system line after the application has resolved its
    /// cliloc number through the client's language table.
    ///
    /// Resolution belongs above this wire-only crate: `Cliloc.enu` is an art
    /// resource, while this view owns the bounded, ordered journal every kind
    /// of server feedback shares.
    pub fn localized_message(&mut self, message: &LocalizedMessage, text: String) {
        self.heard(Heard {
            serial: message.serial,
            graphic: message.graphic,
            mode: message.mode,
            hue: message.hue,
            font: message.font,
            name: message.name.clone(),
            text,
        });
    }

    /// Record a step of the player's own that the server has confirmed.
    ///
    /// The one thing that reaches the player from outside [`apply`](Self::apply),
    /// and it has to be: a `0x22` ack carries a sequence number and a health-bar
    /// colour and *no position*, so where the body now stands is the tile the
    /// acked step was asking for, and only [`Walk`](crate::walk::Walk) — which
    /// sent it — knows what that was.
    ///
    /// This is still a record of what the server said rather than a guess: the
    /// ack is the saying. The prediction that has not been acked yet stays where
    /// it belongs, in [`Walk::predicted`](crate::walk::Walk::predicted).
    ///
    /// Returns whether anything changed, the same as [`apply`](Self::apply).
    pub fn player_stepped(&mut self, position: Point, facing: Facing) -> bool {
        let changed = self.player.position != position || self.player.facing != facing;
        self.player.position = position;
        self.player.facing = facing;
        changed
    }

    /// Fold in what a packet says.
    ///
    /// Returns whether anything changed, which is what a renderer wants to know
    /// and what a test can assert on.
    ///
    /// Most packets are still `false`: their decoders do not exist yet, so they
    /// never reach here as anything but
    /// [`Undecoded`](crate::connection::Event::Undecoded). The list grows one
    /// decoder at a time and this is where each one lands.
    pub fn apply(&mut self, packet: &ServerPacket) -> bool {
        match packet {
            // A second `0x1B` restarts the session on a real client — it is not
            // a move — so taking it wholesale is right: whatever the server
            // says now replaces what we thought, everyone else included.
            ServerPacket::PlayerStart(start) => {
                let mut fresh = Self::entered(*start);
                // The journal crosses the restart, alone. Everything else here
                // is what the client believes is on screen, and a `0x1B` says
                // all of it is stale; the journal is what the client was
                // *told*, and restarting a session unsays none of it. A shard
                // that announced a stop and then sent one more `0x1B` would
                // otherwise have erased the announcement. Open windows do *not*
                // cross it: a gump is keyed on a serial from the session that
                // has just been restarted, so answering one afterwards would
                // name a context the server no longer has.
                std::mem::swap(&mut fresh.journal, &mut self.journal);
                let changed = *self != fresh;
                *self = fresh;
                changed
            }
            // A window. Parsed here, once, rather than by whatever draws it —
            // see `OpenGump`. A second `0xB0` under an id already open replaces
            // that window rather than stacking a copy on it: the server sends a
            // `0xBF 0x04` before re-drawing a dialog it means to replace, and a
            // client that missed one would otherwise grow a pile of identical
            // menus nobody can dismiss.
            ServerPacket::GumpDisplay(display) => {
                let fresh = OpenGump {
                    key: display.serial,
                    gump_id: display.gump_id,
                    at: display.at,
                    elements: parse(&display.layout),
                    lines: display.lines.clone(),
                };
                match self.gumps.iter_mut().find(|open| open.gump_id == fresh.gump_id) {
                    Some(open) if *open == fresh => false,
                    Some(open) => {
                        *open = fresh;
                        true
                    }
                    None => {
                        self.gumps.push(fresh);
                        true
                    }
                }
            }
            // `0xBF 0x04`: the one case where the server *does* say a gump
            // closed, unprompted by any client reply — a script or a quest
            // step tearing down its own window rather than replacing it with
            // a fresh `0xB0`. `gump_closed` is the same removal the client
            // already runs on its own reply; this just lets the server drive
            // it too.
            ServerPacket::CloseGump(close) => self.gump_closed(close.gump_id),
            // Speech, a system line, an NPC — all one journal, whichever of the
            // two wire shapes it arrived as.
            ServerPacket::SpokenMessage(line) => {
                self.heard(Heard::from(line));
                // Always a change: the same sentence said twice is two lines in
                // a journal, unlike a position that is set twice to one tile.
                true
            }
            // `0xAE`: the shape a client that spoke `0xAD` gets its own words
            // back as — see `Heard`'s docs for why this cannot be skipped.
            ServerPacket::UnicodeMessage(line) => {
                self.heard(Heard::from(line));
                true
            }
            // The server uses `0x20` not only for relocations, but also for an
            // authoritative turn of this client's own body.  `0x77` cannot do
            // that job: its own-serial form is ignored below to preserve local
            // walk prediction.  Keep the facing from this packet even when its
            // position is unchanged.
            ServerPacket::PlayerUpdate(update) => {
                let fresh = Player {
                    serial: self.player.serial,
                    body: update.body,
                    hue: update.hue,
                    // The one bit of the byte this end answers from. The rest of
                    // it is dropped at the door rather than kept unread — see
                    // `Player::walks_through_bodies`.
                    walks_through_bodies: update.flags.has(StatusFlags::IGNORE_MOBILES),
                    // The stance is nobody's business but `0x72`'s. The byte
                    // above does carry a war bit — the shard sets it, and a
                    // stock client reads it — but it moves only when a `0x20`
                    // does, while the stance moves the moment the toggle is
                    // answered, so taking it here would be a second answer to
                    // `war`'s question and a stale one. See `Player::war`.
                    war: self.player.war,
                    // `0x20`'s reason again: this is a position and an
                    // appearance, and death is neither — it has its own
                    // packet, `0x2C`.
                    dead: self.player.dead,
                    attacking: self.player.attacking,
                    hits: self.player.hits,
                    // `0x20` says nothing about the paperdoll numbers.
                    status: self.player.status.clone(),
                    position: update.position,
                    facing: update.facing,
                    // `0x20` is a position and an appearance, never a paperdoll:
                    // keeping what the `0x78` said is the difference between a
                    // client that still knows its backpack and one that forgets
                    // it the first time the server nudges the body.
                    equipment: self.player.equipment.clone(),
                    // Kept for the same reason, and a stronger one: `0x20` says
                    // nothing about skills, and a fresh table here would empty a
                    // standing window every time the body took a step.
                    skills: self.player.skills.clone(),
                };
                let changed = self.player != fresh;
                self.player = fresh;
                changed
            }
            // A `0x77` naming this client's own serial is not a move of it: the
            // client's body is moved by `0x20` and by its own acked steps, and
            // acting on this one would fight the prediction in `Walk`. See
            // `openshard_protocol::mobile::MobileMove`.
            ServerPacket::MobileMove(step) if step.serial == self.player.serial => false,
            ServerPacket::MobileMove(step) => {
                // A move never touches what a mobile is wearing; keep whatever
                // 0x78 last said, or nothing if this is the first we have seen
                // of it — a naked arrival is exactly what an empty list means.
                let equipment = self
                    .mobiles
                    .get(&step.serial)
                    .map(|mobile| mobile.equipment.clone())
                    .unwrap_or_default();
                let fresh = Mobile {
                    body: step.body,
                    position: step.position,
                    facing: step.facing,
                    hue: step.hue,
                    flags: step.flags,
                    notoriety: step.notoriety,
                    hits: self.mobiles.get(&step.serial).and_then(|mobile| mobile.hits),
                    equipment,
                };
                let changed = self.mobiles.get(&step.serial) != Some(&fresh);
                self.mobiles.insert(step.serial, fresh);
                changed
            }
            // The one `0x78` a client is sent about itself, and the only place
            // it learns what it is wearing. It goes to the player rather than
            // into `mobiles`, which is never keyed by our own serial: a body in
            // both would be drawn twice, once at each end's idea of where it is.
            //
            // Its position is the reveal snapshot, not a player relocation.
            // `Walk` deliberately treats this packet as idle; copying the
            // coordinate here would make the authoritative world disagree with
            // that movement core whenever equipment arrives during a walk.
            ServerPacket::MobileIncoming(incoming) if incoming.serial == self.player.serial => {
                let fresh = Player {
                    serial: self.player.serial,
                    body: incoming.body,
                    hue: incoming.hue,
                    // The same bit off the same byte, for `0x20`'s reason above.
                    walks_through_bodies: incoming.flags.has(StatusFlags::IGNORE_MOBILES),
                    // Kept, for `0x20`'s reason above: a `0x78` describes a body
                    // and what hangs on it, and the stance is not one of those.
                    war: self.player.war,
                    // Kept, for the same reason: a `0x78` describes a body
                    // and its gear, and death is `0x2C`'s to say.
                    dead: self.player.dead,
                    attacking: self.player.attacking,
                    hits: self.player.hits,
                    // `0x78` dresses the player but does not restate status.
                    status: self.player.status.clone(),
                    position: self.player.position,
                    facing: self.player.facing,
                    equipment: incoming.equipment.clone(),
                    // Kept, for `0x20`'s reason above.
                    skills: self.player.skills.clone(),
                };
                let changed = self.player != fresh;
                self.player = fresh;
                changed
            }
            ServerPacket::MobileIncoming(incoming) => {
                let fresh = Mobile {
                    body: incoming.body,
                    position: incoming.position,
                    facing: incoming.facing,
                    hue: incoming.hue,
                    flags: incoming.flags,
                    notoriety: incoming.notoriety,
                    hits: self.mobiles.get(&incoming.serial).and_then(|mobile| mobile.hits),
                    equipment: incoming.equipment.clone(),
                };
                let changed = self.mobiles.get(&incoming.serial) != Some(&fresh);
                self.mobiles.insert(incoming.serial, fresh);
                changed
            }
            // A single new or changed worn item.  Shops use this immediately
            // before their buy list: the crate is equipped on layer `0x1A`,
            // then `0x74` names that crate and `0x24` names the merchant.
            // Dropping this packet therefore made a fully received shop look
            // like a packet with no owner.
            ServerPacket::EquipUpdate(update) if update.mobile == self.player.serial => {
                let was_ground = self.items.remove(&update.item).is_some();
                let was_contained = self.remove_from_containers(update.item, None);
                // A slot replacement is a separate item leaving the body, so
                // keep the incoming serial's old copy out of every other slot.
                let was_worn = self.remove_from_equipment(update.item);
                let equipment = &mut self.player.equipment;
                let fresh = Equipment {
                    serial: update.item,
                    graphic: update.graphic,
                    layer: update.layer,
                    hue: update.hue,
                };
                match equipment.iter_mut().find(|item| item.layer == update.layer) {
                    Some(item) if *item == fresh => was_ground || was_contained || was_worn,
                    Some(item) => {
                        *item = fresh;
                        true
                    }
                    None => {
                        equipment.push(fresh);
                        true
                    }
                }
            }
            ServerPacket::EquipUpdate(update) => {
                let was_ground = self.items.remove(&update.item).is_some();
                let was_contained = self.remove_from_containers(update.item, None);
                let was_worn = self.remove_from_equipment(update.item);
                let stock_changed = if update.layer.0 == 0x1A {
                    self.vendor_stock.insert(update.mobile, update.item) != Some(update.item)
                } else {
                    false
                };
                let Some(mobile) = self.mobiles.get_mut(&update.mobile) else {
                    return stock_changed || was_ground || was_contained || was_worn;
                };
                let fresh = Equipment {
                    serial: update.item,
                    graphic: update.graphic,
                    layer: update.layer,
                    hue: update.hue,
                };
                match mobile
                    .equipment
                    .iter_mut()
                    .find(|item| item.layer == update.layer)
                {
                    Some(item) if *item == fresh => stock_changed || was_ground || was_contained || was_worn,
                    Some(item) => {
                        *item = fresh;
                        true
                    }
                    None => {
                        mobile.equipment.push(fresh);
                        true
                    }
                }
            }
            ServerPacket::WorldItem(item) => {
                let fresh = Item {
                    graphic: item.graphic,
                    payload: item.payload,
                    position: item.position,
                    hue: item.hue,
                };
                let changed = self.items.get(&item.serial) != Some(&fresh);
                self.items.insert(item.serial, fresh);
                self.remove_from_containers(item.serial, None) || changed
            }
            ServerPacket::TargetCursor(cursor) => {
                // No multi, and that has to be *written* rather than left alone:
                // a plain cursor after a house cursor must stop drawing the
                // house, and the two live in one value so it cannot be forgotten.
                let opened = OpenTarget {
                    cursor: *cursor,
                    multi: None,
                };
                let changed = self.target != Some(opened);
                self.target = Some(opened);
                changed
            }
            ServerPacket::MultiTarget(request) => {
                let opened = OpenTarget {
                    cursor: TargetCursor {
                        cursor_id: request.cursor_id,
                        kind: request.kind,
                    },
                    multi: Some(request.multi),
                };
                let changed = self.target != Some(opened);
                self.target = Some(opened);
                changed
            }
            // A window over a container. The contents are a separate packet and
            // arrive in the same breath, so this only records the art: see
            // `WorldView::contents` for why the two are not one field.
            //
            // A `0x24` for a container already open is the shard re-drawing it —
            // the same shape as a second `0xB0` — and the `0x3C` behind it is
            // authoritative, so what is held now is dropped rather than merged.
            // Merging would leave an item the shard has since removed on screen
            // until something else happened to that container.
            ServerPacket::OpenContainer(open) => {
                self.contents.remove(&open.container);
                let changed = self.containers.insert(open.container, open.gump) != Some(open.gump);
                // A shop gump opens on a mobile, but the buy list named the
                // crate worn on its stock layer.  `EquipUpdate` precedes both,
                // so join the two packets through that crate here.
                if open.gump == Graphic(0x0030) {
                    let stock = self.vendor_stock.get(&open.container).copied().or_else(|| {
                        self.mobiles.get(&open.container).and_then(|mobile| {
                            mobile
                                .equipment
                                .iter()
                                .find(|item| item.layer.0 == 0x1A)
                                .map(|item| item.serial)
                        })
                    });
                    if let Some(stock) = stock {
                        if let Some(lines) = self.pending_vendor_buys.remove(&stock) {
                            self.vendor_sells.remove(&open.container);
                            self.vendor_buys.insert(
                                open.container,
                                VendorBuy {
                                    container: stock,
                                    lines,
                                },
                            );
                        }
                    }
                }
                changed
            }
            // An empty listing names no container at all — the wire has no field
            // for it — so there is nothing this can be about. See
            // `ContainerContents::container`.
            ServerPacket::ContainerContents(listing) => match listing.container {
                None => false,
                Some(container) => {
                    let changed = self.contents.get(&container) != Some(&listing.items);
                    self.contents.insert(container, listing.items.clone());
                    changed
                }
            },
            ServerPacket::BuyList(list) => {
                self.pending_vendor_buys
                    .insert(list.container, list.lines.clone());
                true
            }
            ServerPacket::SellList(list) => {
                let fresh = VendorSell {
                    lines: list.lines.clone(),
                };
                let changed = self.vendor_sells.get(&list.vendor) != Some(&fresh);
                self.vendor_sells.insert(list.vendor, fresh);
                // A sell offer replaces the buy catalogue on this merchant.
                // The protocol has no explicit close packet, so retaining it
                // would leave the client submitting purchases while showing a
                // resale list.
                self.vendor_buys.remove(&list.vendor);
                changed
            }
            // One more item in a container, which may be one this client has no
            // window for: a shard pushes a `0x25` to everyone it thinks has the
            // container open, and its list and ours part company the moment the
            // player closes a window (see `container_closed`). Recorded anyway —
            // the packet is a statement about the world, and dropping it because
            // no window is up would be this end deciding what it was told.
            //
            // Keyed by serial rather than appended blindly: the shard sends a
            // `0x25` for an item whose stack merely grew, and the reference
            // client replaces the record it already has.
            ServerPacket::AddToContainer(added) => {
                let was_ground = self.items.remove(&added.item.serial).is_some();
                let was_elsewhere = self.remove_from_containers(added.item.serial, Some(added.container));
                let was_worn = self.remove_from_equipment(added.item.serial);
                let held = self.contents.entry(added.container).or_default();
                match held.iter_mut().find(|item| item.serial == added.item.serial) {
                    Some(item) if *item == added.item => was_ground || was_elsewhere || was_worn,
                    Some(item) => {
                        *item = added.item;
                        true
                    }
                    None => {
                        held.push(added.item);
                        true
                    }
                }
            }
            // A paperdoll. Only the window: the equipment it draws is already
            // held on the mobile this names — see `WorldView::paperdolls`.
            //
            // Recorded even for a mobile this client has never been shown. The
            // packet is the shard saying "open a window on this serial", and a
            // client that dropped it because the body is not on screen would be
            // deciding what it was told; the window can draw a title and no
            // body, which is honest, where nothing at all is not.
            ServerPacket::OpenPaperdoll(paperdoll) => {
                let fresh = Paperdoll {
                    title: paperdoll.text.clone(),
                    can_lift: paperdoll.flags.has(PaperdollFlags::CAN_LIFT),
                };
                let mut changed = self.paperdolls.get(&paperdoll.serial) != Some(&fresh);
                self.paperdolls.insert(paperdoll.serial, fresh);
                // The flag byte's other bit, filed where the stance lives rather
                // than beside the window — and only when the doll is our own:
                // a `0x88` about somebody else states *their* stance, which
                // nothing here draws, and folding it into the player would be
                // this client learning it was at war from a stranger's frame.
                if paperdoll.serial == self.player.serial {
                    let war = paperdoll.flags.has(PaperdollFlags::WARMODE);
                    changed |= self.player.war != war;
                    self.player.war = war;
                }
                changed
            }
            // A spellbook's ordinary `0x24` has already described the book
            // itself.  This companion packet decides which spell rows are
            // available and may refresh after a scroll is scribed.
            ServerPacket::SpellbookContent(content) => {
                let fresh = Spellbook::from(*content);
                let changed = self.spellbooks.get(&content.serial) != Some(&fresh);
                self.spellbooks.insert(content.serial, fresh);
                changed
            }
            // The stance settled. Sent unprompted as well as in answer to the
            // client's own `0x72` — a shard puts a player into war mode when
            // something attacks them — so this is folded rather than assumed
            // from what was asked for. It is the toggle's picture on the
            // paperdoll and, later, the stance a body is drawn standing in.
            ServerPacket::WarMode(mode) => {
                let changed = self.player.war != mode.war;
                self.player.war = mode.war;
                changed
            }
            ServerPacket::AttackTarget(target) => {
                let changed = self.player.attacking != target.target;
                self.player.attacking = target.target;
                changed
            }
            ServerPacket::Health(bar) if bar.serial == self.player.serial => {
                let changed = self.player.hits != Some(bar.vitals);
                self.player.hits = Some(bar.vitals);
                changed
            }
            ServerPacket::Health(bar) => match self.mobiles.get_mut(&bar.serial) {
                Some(mobile) => {
                    let changed = mobile.hits != Some(bar.vitals);
                    mobile.hits = Some(bar.vitals);
                    changed
                }
                None => false,
            },
            // `0x11` is status-bar data, not a position or an appearance. It
            // belongs on the one player the connection is about, and its hits
            // join `0xA1` in Player::hits so the two pictures have one value.
            ServerPacket::MobileStatus(status) if status.serial == self.player.serial => {
                let fresh = Status::from(status);
                let changed =
                    self.player.status.as_ref() != Some(&fresh) || self.player.hits != Some(status.hits);
                self.player.status = Some(fresh);
                self.player.hits = Some(status.hits);
                changed
            }
            // `0x2C`: this end just died, or came back. `docs/combat.md`'s
            // D9 — the one packet that greys the whole screen, gates an
            // attack off a ghost's own click, and drops the war stance even
            // if `war` is still set.
            ServerPacket::DeathStatus(status) => {
                let changed = self.player.dead != status.dead;
                self.player.dead = status.dead;
                changed
            }
            // The whole skill list. It **replaces** rather than merges: this is
            // the shard stating every skill it knows of, so a row that is not in
            // it is a row this character no longer has — and a client that
            // merged would keep drawing a skill a shard had dropped from its
            // table, at the value it last had.
            ServerPacket::SkillsFull(list) => {
                let fresh: BTreeMap<u8, Skill> = list
                    .entries
                    .iter()
                    .map(|entry| (entry.id, Skill::from(entry)))
                    .collect();
                let changed = self.player.skills != fresh;
                self.player.skills = fresh;
                changed
            }
            // One line, after a gain — or after a loss, since a skill set to
            // train down gives ground the moment another needs the room. Folded
            // into the table rather than replacing it, which is the whole of the
            // difference between the two packets on this side of the wire.
            ServerPacket::SkillUpdate(update) => {
                let fresh = Skill::from(&update.entry);
                let changed = self.player.skills.get(&update.entry.id) != Some(&fresh);
                self.player.skills.insert(update.entry.id, fresh);
                changed
            }
            // Mobiles walking out of range and items being picked up arrive the
            // same way — the client does not distinguish, it just forgets the
            // serial. Only one of the three places can ever hold it: a serial is
            // a mobile, or a ground item, or inside exactly one container.
            //
            // The container arm is what makes an item picked *out* of a bag
            // leave the window it was drawn in — there is no "removed from
            // container" packet, a `0x1D` is the whole of it.
            ServerPacket::Remove(remove) => {
                let had_mobile = self.mobiles.remove(&remove.serial).is_some();
                let had_item = self.items.remove(&remove.serial).is_some();
                let mut was_held = false;
                for held in self.contents.values_mut() {
                    let before = held.len();
                    held.retain(|item| item.serial != remove.serial);
                    was_held |= held.len() != before;
                }
                // A container that is itself removed takes its window with it.
                let had_window = self.containers.remove(&remove.serial).is_some();
                self.contents.remove(&remove.serial);
                // A spellbook may be held by that container or lie on the
                // ground; either way a removed book cannot keep its spell
                // page open, and a reused serial must not inherit its mask.
                let had_spellbook = self.spellbooks.remove(&remove.serial).is_some();
                let had_vendor = self.vendor_buys.remove(&remove.serial).is_some()
                    || self.vendor_sells.remove(&remove.serial).is_some()
                    || self.pending_vendor_buys.remove(&remove.serial).is_some()
                    || self.vendor_stock.remove(&remove.serial).is_some();
                // And so does a mobile: a body that walked out of range cannot
                // be looked at any more, and the window over it would keep
                // drawing the equipment as it stood when the body left. The
                // reference does exactly this, in `Mobile.Destroy` — and only
                // for a mobile that is not the player, which needs no guard
                // here because a `0x1D` never names our own serial.
                let had_paperdoll = self.paperdolls.remove(&remove.serial).is_some();
                // And its tooltip. A hover cannot land on something that is not
                // drawn, so the entry has no reader left — and keeping it would
                // hand a stale name back if the serial were reused.
                let had_tooltip = self.tooltips.remove(&remove.serial).is_some();
                // And its design revision. A house that has come down cannot be
                // asked about, and a reused serial must not inherit its picture.
                let had_design = self.designs.remove(&remove.serial).is_some();
                had_mobile
                    || had_item
                    || was_held
                    || had_window
                    || had_spellbook
                    || had_vendor
                    || had_paperdoll
                    || had_tooltip
                    || had_design
            }
            // The roster, whole. Not merged: the shard re-sends the entire list
            // on every change rather than sending deltas, so replacing is the
            // faithful reading — and an accumulated roster would keep anybody
            // whose removal packet this client happened to miss.
            ServerPacket::PartyMemberList(list) => {
                let changed = self.party.members != list.members || self.party.invited_by.is_some();
                self.party.members.clone_from(&list.members);
                // Joining answers the question that was asked, so the
                // invitation goes with it. Nothing on the wire says so — the
                // shard sends a roster and considers the matter closed.
                self.party.invited_by = None;
                changed
            }
            // Somebody left. The list is who is *left*, and an empty one is this
            // client being told it is in no party — the packet has no other way
            // to say that. See `PartyRemoveMember`.
            ServerPacket::PartyRemoveMember(removal) => {
                let changed = self.party.members != removal.members;
                self.party.members.clone_from(&removal.members);
                changed
            }
            ServerPacket::PartyInvitation(invitation) => {
                let changed = self.party.invited_by != Some(invitation.leader);
                self.party.invited_by = Some(invitation.leader);
                changed
            }
            // Party chat goes in the journal beside everything else said to this
            // client — see `Heard`. It is not speech and draws over nobody's
            // head, but it is a line somebody said, and a journal that held only
            // the lines with a position would be missing half a conversation.
            //
            // # The channel is in the text, not in the mode
            //
            // Party chat has no `TalkMode`: it is not `0xAE` and the wire never
            // names one. Writing `TalkMode::Other(0x04)` would be putting a
            // party *packet type* in a field whose whole doc is "the mode byte
            // the client sent", and the next reader to trust that would be
            // reading a 4 as a talk mode. So the channel is prefixed the way
            // ServUO itself formats these for a listener (`"[Party]: {0}"`),
            // which is honest, needs no type to change, and reads correctly in a
            // journal that has no column for it.
            ServerPacket::PartyTextMessage(message) => {
                let channel = match message.to_all {
                    true => "[Party]",
                    false => "[Party tell]",
                };
                self.heard(Heard {
                    serial: Some(message.from),
                    graphic: None,
                    mode: TalkMode::Regular,
                    // The wire carries no name here, only a serial: a client is
                    // expected to know who its own party is. Left empty rather
                    // than guessed at — whatever draws this has the roster and
                    // the mobiles, and this layer has only the number.
                    name: channel.to_owned(),
                    font: Font::DEFAULT,
                    hue: Hue::NONE,
                    text: message.text.clone(),
                });
                true
            }
            // The shard says this object's tooltip has a new revision. It does
            // not send the list — asking for it is this end's move, and only
            // when something wants to draw it.
            ServerPacket::TooltipRevision(revision) => {
                let entry = self.tooltips.entry(revision.serial).or_default();
                let changed = entry.revision != Some(revision.hash);
                entry.revision = Some(revision.hash);
                changed
            }
            // The shard says this house's *picture* has a new revision. Same
            // move as the tooltip above: it does not send the shape, and asking
            // for it is this end's.
            ServerPacket::DesignRevision(revision) => {
                let Some(serial) = revision.serial.validate() else {
                    return false;
                };
                let changed = self.designs.get(&serial) != Some(&revision.revision.0);
                self.designs.insert(serial, revision.revision.0);
                changed
            }
            // What this shard holds us at. This engine's own subcommand, sent
            // once on world entry — see `AuthorityNotice`. Nothing is enforced
            // here: it decides what the speech line *offers*, and the shard
            // still refuses whatever it refuses.
            ServerPacket::AuthorityNotice(notice) => {
                let changed = self.authority != notice.level;
                self.authority = notice.level;
                changed
            }
            // And which world it is. Recorded and nothing more: fetching the
            // ground is a later phase of `to_the_client.md`, and this is the
            // fact that phase starts from.
            ServerPacket::WorldNotice(notice) => {
                let changed = self.world != Some(*notice);
                self.world = Some(*notice);
                changed
            }
            // The list. Arrives either as the answer to our `0xD6` or, in the
            // shard's `full` tooltip mode, unasked and with no revision ever
            // named — which is why this does not touch `revision`. See
            // [`Tooltip::stale`].
            ServerPacket::PropertyListReply(reply) => {
                let entry = self.tooltips.entry(reply.serial).or_default();
                let changed = entry.held_revision != Some(reply.hash) || entry.entries != reply.entries;
                entry.held_revision = Some(reply.hash);
                entry.entries.clone_from(&reply.entries);
                changed
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::combat::WarMode;
    use openshard_protocol::containers::{AddToContainer, ContainerContents};
    use openshard_protocol::direction::Direction;
    use openshard_protocol::items::{ItemAmount, ItemFlags, WorldItem, WorldItemPayload};
    use openshard_protocol::mobile::{MobileIncoming, MobileMove, MobileStatus, Remove};
    use openshard_protocol::skill::SkillLock;
    use openshard_protocol::world::{DeathStatus, PlayerUpdate};

    use super::*;

    fn start() -> PlayerStart {
        PlayerStart {
            serial: Serial::new(0x0000_002A).unwrap(),
            body: Graphic(0x0190),
            position: Point::new(1475, 1770, 20),
            facing: Facing::walking(Direction::South),
            map: MapSize::BRITANNIA,
        }
    }

    fn other() -> Serial {
        Serial::new(0x0000_0002).unwrap()
    }

    fn shirt() -> Equipment {
        Equipment {
            serial: Serial::new(0x4000_0001).unwrap(),
            graphic: Graphic(0x1517),
            layer: openshard_protocol::wire::Layer(0x05),
            hue: Hue(0x0021),
        }
    }

    fn status_of(serial: Serial) -> MobileStatus {
        MobileStatus {
            serial,
            name: "Lord British".to_owned(),
            hits: Vitals {
                current: 98,
                max: 100,
            },
            female: false,
            strength: 100,
            dexterity: 50,
            intelligence: 75,
            stamina: Vitals { current: 49, max: 50 },
            mana: Vitals { current: 72, max: 75 },
            gold: 1_234,
            armor: 42,
            weight: 12,
            max_weight: 450,
            stat_cap: 225,
            followers: 1,
            followers_max: 5,
        }
    }

    fn said(text: &str) -> SpokenMessage {
        // What `WorldState::system_message` builds: no speaker, no body behind
        // it. The shutdown notice is exactly this line.
        SpokenMessage {
            serial: None,
            graphic: None,
            mode: openshard_protocol::speech::TalkMode::Regular,
            hue: Hue(0x0035),
            font: openshard_protocol::speech::Font::DEFAULT,
            name: "System".to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn what_the_server_says_is_written_down_in_the_order_it_was_said() {
        // The client could decode `0x1C` and kept nothing, so a shard that
        // announced its stop was talking to something that heard and forgot.
        let mut view = WorldView::entered(start());
        assert!(view.apply(&ServerPacket::SpokenMessage(said("the shard is stopping"))));
        assert!(
            view.apply(&ServerPacket::SpokenMessage(said("the shard is stopping"))),
            "the same sentence twice is two lines: a journal is not a state to settle on"
        );
        assert!(view.apply(&ServerPacket::SpokenMessage(said("goodbye"))));

        let lines: Vec<&str> = view.journal.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(
            lines,
            ["the shard is stopping", "the shard is stopping", "goodbye"],
            "oldest first, and nothing merged"
        );
    }

    #[test]
    fn the_journal_forgets_its_oldest_line_and_nothing_else() {
        // A virtual player in a town square hears speech for as long as it
        // stands there, and nothing ever asks it to forget — so the bound is
        // what keeps a long session from being a leak.
        let mut view = WorldView::entered(start());
        for line in 0..JOURNAL_LINES + 2 {
            view.apply(&ServerPacket::SpokenMessage(said(&line.to_string())));
        }

        assert_eq!(view.journal.len(), JOURNAL_LINES, "the cap holds");
        assert_eq!(
            view.journal
                .front()
                .expect("a full journal has a first line")
                .text,
            "2",
            "the two oldest lines went, and in that order"
        );
        assert_eq!(
            view.journal.back().expect("a full journal has a last line").text,
            (JOURNAL_LINES + 1).to_string(),
            "and the newest is still the newest"
        );
    }

    /// `0x1C` and `0xAE` are the same event in two encodings — see [`Heard`]'s
    /// docs — so a client that spoke `0xAD` and gets its own accented words
    /// back as `0xAE` must see that line in the same place as everything said
    /// to it in plain ASCII: one journal, one order, one cap. Two journals, or
    /// a `0xAE` that decoded but nowhere to put it, would leave a player's own
    /// speech invisible even though the packet was read correctly.
    #[test]
    fn ascii_and_unicode_speech_share_one_journal_in_arrival_order_and_one_cap() {
        let mut view = WorldView::entered(start());
        let unicode = |text: &str| {
            ServerPacket::UnicodeMessage(openshard_protocol::speech::UnicodeMessage {
                serial: None,
                graphic: None,
                mode: openshard_protocol::speech::TalkMode::Regular,
                hue: Hue(0x0035),
                font: openshard_protocol::speech::Font::DEFAULT,
                language: "ENU".to_owned(),
                name: "System".to_owned(),
                text: text.to_owned(),
            })
        };

        assert!(view.apply(&ServerPacket::SpokenMessage(said("hello"))));
        assert!(view.apply(&unicode("привет")));
        assert!(view.apply(&ServerPacket::SpokenMessage(said("goodbye"))));

        let lines: Vec<&str> = view.journal.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(
            lines,
            ["hello", "привет", "goodbye"],
            "both encodings land in one journal, in the order they arrived"
        );

        // The cap is shared, not one budget per encoding: filling it with the
        // *other* wire shape must still evict the oldest line.
        for line in 0..JOURNAL_LINES {
            view.apply(&unicode(&line.to_string()));
        }
        assert_eq!(view.journal.len(), JOURNAL_LINES, "one cap for both encodings");
        assert_eq!(
            view.journal.back().expect("a full journal has a last line").text,
            (JOURNAL_LINES - 1).to_string()
        );
    }

    /// The admin menu, arriving and being answered — the whole life of a window
    /// as this end sees it. What it protects is the two halves nothing on the
    /// wire says: that a second copy of a dialog *replaces* the open one, and
    /// that answering closes it here, since no packet ever comes back to say so.
    #[test]
    fn a_dialog_replaces_its_own_open_copy_and_closes_when_it_is_answered() {
        let mut view = WorldView::entered(start());
        let menu = |title: &str| {
            ServerPacket::GumpDisplay(openshard_protocol::gump::GumpDisplay {
                serial: GumpKey::on(start().serial),
                gump_id: GumpId(0x00AD_0001),
                at: GumpPoint::new(100, 100),
                layout: "{ resizepic 0 0 5054 300 270 }{ text 105 14 2100 0 }".to_owned(),
                lines: vec![title.to_owned()],
            })
        };

        assert!(view.apply(&menu("Admin")), "a window this client did not have");
        assert_eq!(view.gumps.len(), 1);
        assert_eq!(view.gumps[0].line(0), Some("Admin"));
        assert_eq!(
            view.gumps[0].elements.first(),
            Some(&Element::Background {
                x: 0,
                y: 0,
                width: 300,
                height: 270,
                gump: 5054,
            }),
            "the layout is read when it arrives, not when it is drawn"
        );

        assert!(!view.apply(&menu("Admin")), "the same window twice is no change");
        assert!(view.apply(&menu("Admin II")), "a redrawn window is a change");
        assert_eq!(view.gumps.len(), 1, "and it replaces rather than stacks");

        assert!(view.gump_closed(GumpId(0x00AD_0001)), "it was open");
        assert!(view.gumps.is_empty());
        assert!(
            !view.gump_closed(GumpId(0x00AD_0001)),
            "answering twice closes nothing the second time"
        );
    }

    /// `0xBF 0x04`: a quest step or script dismissing its own window with no
    /// client reply and no redraw to replace it — the one case the server
    /// does say a gump closed, unlike the reply-driven close above.
    #[test]
    fn the_server_can_close_a_gump_the_client_never_answered() {
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::GumpDisplay(
            openshard_protocol::gump::GumpDisplay {
                serial: GumpKey::on(start().serial),
                gump_id: GumpId(0x00AD_0001),
                at: GumpPoint::new(100, 100),
                layout: "{ resizepic 0 0 5054 300 270 }".to_owned(),
                lines: Vec::new(),
            },
        ));
        assert_eq!(view.gumps.len(), 1);

        assert!(
            view.apply(&ServerPacket::CloseGump(openshard_protocol::gump::CloseGump {
                gump_id: GumpId(0x00AD_0001),
                button: openshard_protocol::gump::ButtonId(0),
            }))
        );
        assert!(view.gumps.is_empty());
        assert!(
            !view.apply(&ServerPacket::CloseGump(openshard_protocol::gump::CloseGump {
                gump_id: GumpId(0x00AD_0001),
                button: openshard_protocol::gump::ButtonId(0),
            })),
            "closing what is already closed is no change"
        );
    }

    #[test]
    fn a_restart_replaces_the_world_and_unsays_nothing() {
        // A `0x1B` says everything on screen is stale, and it says nothing
        // about what the client was told: the journal is history, not state.
        // The case this is for is the announcement of `docs/shutdown.md` S3
        // followed by one more entry packet.
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::MobileIncoming(MobileIncoming {
            serial: other(),
            body: Graphic(0x0190),
            position: Point::new(1476, 1770, 20),
            facing: start().facing,
            hue: Hue::NONE,
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
            equipment: Vec::new(),
        }));
        view.apply(&ServerPacket::SpokenMessage(said("the shard is stopping")));

        let elsewhere = PlayerStart {
            position: Point::new(1000, 1000, -10),
            ..start()
        };
        assert!(view.apply(&ServerPacket::PlayerStart(elsewhere)));

        assert!(view.mobiles.is_empty(), "what was on screen is stale and gone");
        let lines: Vec<&str> = view.journal.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(lines, ["the shard is stopping"], "what was said still stands");
    }

    #[test]
    fn entering_records_what_the_server_said() {
        let view = WorldView::entered(start());
        assert_eq!(view.player.position, Point::new(1475, 1770, 20));
        assert_eq!(view.map, MapSize::BRITANNIA);
        assert!(view.mobiles.is_empty());
        assert!(view.items.is_empty());
    }

    #[test]
    fn a_repeated_entry_packet_replaces_the_view() {
        // The server sends 0x1B to *restart* a session, not to nudge a
        // position. Merging it field by field would leave a client half in the
        // old world.
        let mut view = WorldView::entered(start());
        let moved = PlayerStart {
            position: Point::new(1000, 1000, -10),
            ..start()
        };
        assert!(view.apply(&ServerPacket::PlayerStart(moved)));
        assert_eq!(view.player.position, Point::new(1000, 1000, -10));
        assert!(
            !view.apply(&ServerPacket::PlayerStart(moved)),
            "the same packet twice changes nothing"
        );
    }

    #[test]
    fn a_player_update_moves_the_players_own_body() {
        let mut view = WorldView::entered(start());
        let update = PlayerUpdate {
            serial: view.player.serial,
            body: Graphic(0x0191),
            hue: Hue(0x0021),
            flags: StatusFlags::NONE,
            position: Point::new(1480, 1770, 20),
            facing: Facing::running(Direction::East),
        };
        assert!(view.apply(&ServerPacket::PlayerUpdate(update)));
        assert_eq!(view.player.body, Graphic(0x0191));
        assert_eq!(view.player.hue, Hue(0x0021));
        assert_eq!(view.player.position, Point::new(1480, 1770, 20));
        assert_eq!(view.player.facing, Facing::running(Direction::East));
        assert!(
            !view.apply(&ServerPacket::PlayerUpdate(update)),
            "the same packet twice changes nothing"
        );
    }

    #[test]
    fn a_player_update_turns_the_players_own_body_in_place() {
        let mut view = WorldView::entered(start());
        let update = PlayerUpdate {
            serial: view.player.serial,
            body: view.player.body,
            hue: view.player.hue,
            flags: StatusFlags::NONE,
            position: view.player.position,
            facing: Facing::walking(Direction::East),
        };

        assert!(view.apply(&ServerPacket::PlayerUpdate(update)));
        assert_eq!(
            view.player.position,
            start().position,
            "a turn does not move the body"
        );
        assert_eq!(view.player.facing, Facing::walking(Direction::East));
    }

    #[test]
    fn a_confirmed_step_moves_the_player_and_says_when_it_did_not() {
        // What `Walk` hands back on a `0x22`. Turning is a step in UO and its
        // ack looks exactly like a move's, so "the position did not change"
        // must not read as "nothing happened": the facing did.
        let mut view = WorldView::entered(start());
        assert!(view.player_stepped(Point::new(1475, 1769, 20), Facing::walking(Direction::North)));
        assert_eq!(view.player.position, Point::new(1475, 1769, 20));
        assert!(
            !view.player_stepped(Point::new(1475, 1769, 20), Facing::walking(Direction::North)),
            "the same place, facing the same way, is not a change"
        );
        assert!(
            view.player_stepped(Point::new(1475, 1769, 20), Facing::walking(Direction::East)),
            "a turn moves nobody and is still a change"
        );
    }

    #[test]
    fn our_own_0x78_dresses_the_player_instead_of_adding_a_mobile() {
        // The shard sends this once, at world entry, and it is the only packet
        // that tells a client what it is wearing — the reveal pass shows a
        // mobile to everyone but itself. Filed under `mobiles` it would be a
        // second body at the player's own serial, drawn twice.
        let mut view = WorldView::entered(start());
        let mine = MobileIncoming {
            serial: view.player.serial,
            body: Graphic(0x0190),
            // Deliberately different: an appearance snapshot must never
            // relocate the predicted/authoritative player movement anchor.
            position: Point::new(2000, 2000, 0),
            facing: Facing::running(Direction::South),
            hue: Hue(0x83EA),
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
            equipment: vec![shirt()],
        };
        assert!(view.apply(&ServerPacket::MobileIncoming(mine.clone())));
        assert!(view.mobiles.is_empty(), "we are not one of the others");
        assert_eq!(view.player.equipment, vec![shirt()]);
        assert_eq!(view.player.hue, Hue(0x83EA));
        assert_eq!(view.player.position, start().position);
        assert_eq!(view.player.facing, start().facing);
        assert!(
            !view.apply(&ServerPacket::MobileIncoming(mine)),
            "the same packet twice changes nothing"
        );

        // And a 0x20 afterwards must not undress us: it carries a body and a
        // position, and no paperdoll at all.
        view.apply(&ServerPacket::PlayerUpdate(PlayerUpdate {
            serial: view.player.serial,
            body: Graphic(0x0190),
            hue: Hue(0x83EA),
            flags: StatusFlags::NONE,
            position: Point::new(1476, 1770, 20),
            facing: Facing::walking(Direction::North),
        }));
        assert_eq!(view.player.equipment, vec![shirt()]);
    }

    #[test]
    fn our_own_0x77_moves_nothing() {
        // Sphere's warning, from the client's side: 0x77 cannot move the body
        // the client is predicting for. Acting on one would fight `Walk`.
        let mut view = WorldView::entered(start());
        assert!(!view.apply(&ServerPacket::MobileMove(MobileMove {
            serial: view.player.serial,
            body: Graphic(0x0190),
            position: Point::new(1000, 1000, 0),
            facing: Facing::walking(Direction::North),
            hue: Hue::NONE,
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
        })));
        assert_eq!(view.player.position, start().position);
        assert!(view.mobiles.is_empty());
    }

    #[test]
    fn a_mobile_incoming_is_recorded_with_its_equipment() {
        let mut view = WorldView::entered(start());
        let incoming = MobileIncoming {
            serial: other(),
            body: Graphic(0x0190),
            position: Point::new(1476, 1770, 20),
            facing: Facing::walking(Direction::South),
            hue: Hue(0x83EA),
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
            equipment: vec![shirt()],
        };
        assert!(view.apply(&ServerPacket::MobileIncoming(incoming.clone())));
        let mobile = view.mobiles.get(&other()).expect("the mobile was recorded");
        assert_eq!(mobile.position, Point::new(1476, 1770, 20));
        assert_eq!(mobile.equipment, vec![shirt()]);
        assert!(
            !view.apply(&ServerPacket::MobileIncoming(incoming)),
            "the same packet twice changes nothing"
        );
    }

    #[test]
    fn a_mobile_move_keeps_the_equipment_a_0x78_already_set() {
        // 0x77 never carries an equipment list — it is a move, not a redraw —
        // so a mobile already on screen must not be stripped naked by one.
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::MobileIncoming(MobileIncoming {
            serial: other(),
            body: Graphic(0x0190),
            position: Point::new(1476, 1770, 20),
            facing: Facing::walking(Direction::South),
            hue: Hue(0x83EA),
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
            equipment: vec![shirt()],
        }));

        assert!(view.apply(&ServerPacket::MobileMove(MobileMove {
            serial: other(),
            body: Graphic(0x0190),
            position: Point::new(1477, 1770, 20),
            facing: Facing::walking(Direction::South),
            hue: Hue(0x83EA),
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
        })));

        let mobile = view.mobiles.get(&other()).unwrap();
        assert_eq!(mobile.position, Point::new(1477, 1770, 20));
        assert_eq!(mobile.equipment, vec![shirt()], "the move must not undress it");
    }

    #[test]
    fn a_world_item_is_recorded_by_serial() {
        let mut view = WorldView::entered(start());
        let item = WorldItem {
            serial: Serial::new(0x4000_00AB).unwrap(),
            graphic: Graphic(0x0EED),
            payload: WorldItemPayload::Stack(ItemAmount(500)),
            position: Point::new(1000, 2000, 5),
            hue: Hue(0x0021),
            light: None,
            flags: ItemFlags::NONE,
        };
        assert!(view.apply(&ServerPacket::WorldItem(item)));
        assert_eq!(
            view.items.get(&item.serial).unwrap().payload,
            WorldItemPayload::Stack(ItemAmount(500))
        );
    }

    #[test]
    fn a_ground_destination_retires_the_lifters_stale_container_source() {
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::AddToContainer(AddToContainer {
            item: candle(),
            container: chest(),
        }));
        assert!(view.apply(&ServerPacket::WorldItem(WorldItem {
            serial: candle().serial,
            graphic: candle().graphic,
            payload: WorldItemPayload::Stack(candle().amount),
            position: Point::new(1000, 2000, 5),
            hue: candle().hue,
            light: None,
            flags: ItemFlags::NONE,
        })));
        assert!(view.contents.get(&chest()).is_none_or(Vec::is_empty));
        assert!(view.items.contains_key(&candle().serial));
    }

    #[test]
    fn a_remove_forgets_whichever_map_actually_holds_the_serial() {
        // The client does not distinguish a mobile walking out of range from an
        // item being picked up; neither does Remove — it just tries both maps.
        let mut view = WorldView::entered(start());
        let item = WorldItem {
            serial: Serial::new(0x4000_00AB).unwrap(),
            graphic: Graphic(0x0EED),
            payload: WorldItemPayload::Stack(ItemAmount(1)),
            position: Point::new(1000, 2000, 5),
            hue: Hue::NONE,
            light: None,
            flags: ItemFlags::NONE,
        };
        view.apply(&ServerPacket::WorldItem(item));

        assert!(view.apply(&ServerPacket::Remove(Remove { serial: item.serial })));
        assert!(!view.items.contains_key(&item.serial));
        assert!(
            !view.apply(&ServerPacket::Remove(Remove { serial: item.serial })),
            "forgetting something already gone changes nothing"
        );
    }

    fn paperdoll_of(serial: Serial) -> ServerPacket {
        ServerPacket::OpenPaperdoll(openshard_protocol::mobile::OpenPaperdoll {
            serial,
            text: "Lord British".to_owned(),
            flags: PaperdollFlags::CAN_LIFT,
        })
    }

    fn spellbook_of(serial: Serial, content: u64) -> ServerPacket {
        ServerPacket::SpellbookContent(SpellbookContent {
            serial,
            graphic: Graphic(0x0EFA),
            offset: 1,
            content,
        })
    }

    /// A `0x88` opens a window and dresses nobody: the equipment it draws came
    /// in a `0x78` and stays on the mobile. Asserted together because the
    /// tempting shape — a paperdoll that carries its own copy of the equipment —
    /// is exactly what would go stale the next time a hat came off.
    #[test]
    fn a_paperdoll_is_a_window_over_equipment_the_mobile_already_has() {
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::MobileIncoming(MobileIncoming {
            serial: other(),
            body: Graphic(0x0190),
            position: Point::new(1476, 1770, 20),
            facing: Facing::walking(Direction::South),
            hue: Hue(0x83EA),
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
            equipment: vec![shirt()],
        }));

        assert!(view.apply(&paperdoll_of(other())));
        let open = view.paperdolls.get(&other()).expect("the window was recorded");
        assert_eq!(open.title, "Lord British");
        assert!(open.can_lift);
        assert_eq!(
            view.mobiles.get(&other()).unwrap().equipment,
            vec![shirt()],
            "and the equipment is still the mobile's"
        );
        assert!(
            !view.apply(&paperdoll_of(other())),
            "the same window twice settles"
        );
    }

    #[test]
    fn a_spellbook_content_packet_keeps_the_book_and_its_mask_together() {
        let mut view = WorldView::entered(start());
        let book = Serial::new(0x4000_0001).expect("an item serial");

        assert!(view.apply(&spellbook_of(book, 1 | (1 << 17))));
        assert_eq!(
            view.spellbooks.get(&book),
            Some(&Spellbook {
                graphic: Graphic(0x0EFA),
                offset: 1,
                content: 1 | (1 << 17),
            })
        );
        assert!(view.spellbook_closed(book));
        assert!(view.spellbooks.is_empty());
    }

    /// The one home for the stance, and the two doors into it: the `0x88` this
    /// client's own paperdoll opens on, and every `0x72` after it. The second
    /// must win — that is the whole reason the flag byte is not kept beside the
    /// window it arrived with.
    #[test]
    fn war_mode_is_the_players_and_the_last_packet_about_it_wins() {
        let mut view = WorldView::entered(start());
        assert!(!view.player.war, "a session starts at peace");

        assert!(view.apply(&ServerPacket::OpenPaperdoll(
            openshard_protocol::mobile::OpenPaperdoll {
                serial: view.player.serial,
                text: "Lord British".to_owned(),
                flags: PaperdollFlags::WARMODE.with(PaperdollFlags::CAN_LIFT),
            }
        )));
        assert!(view.player.war, "our own `0x88` states the stance we are in");

        assert!(view.apply(&ServerPacket::WarMode(WarMode { war: false })));
        assert!(!view.player.war, "and a `0x72` moves it");
        assert!(
            !view.apply(&ServerPacket::WarMode(WarMode { war: false })),
            "the same stance twice is not a change"
        );
    }

    /// The flag byte a `0x20` carries about our own body: one bit is kept and
    /// the war bit is not.
    ///
    /// Both halves matter. `IGNORE_MOBILES` has to land, because it is the
    /// shard telling this end that its own body-blocking rule does not apply to
    /// this mover (`clutter::crowd`). And the byte's war bit has to *stay* out
    /// of the stance: it moves only when a `0x20` does, so a client that took it
    /// would put a body that toggled war while standing still back at peace on
    /// its next relocation — the reference client keeps the two apart for the
    /// same reason (`PlayerMobile.InWarMode` is its own field there).
    #[test]
    fn a_player_update_carries_the_body_blocking_exemption_and_not_the_stance() {
        let mut view = WorldView::entered(start());
        assert!(!view.player.walks_through_bodies, "nothing has said so yet");

        assert!(view.apply(&ServerPacket::WarMode(WarMode { war: true })));
        assert!(view.player.war);

        let relocated = PlayerUpdate {
            serial: view.player.serial,
            body: view.player.body,
            hue: view.player.hue,
            // A `.gm` at peace, as the shard's `stance_of` would state it — and
            // the war bit deliberately clear, which is the disagreement this
            // test is about.
            flags: StatusFlags::IGNORE_MOBILES,
            position: Point::new(1480, 1770, 20),
            facing: view.player.facing,
        };
        assert!(view.apply(&ServerPacket::PlayerUpdate(relocated)));
        assert!(
            view.player.walks_through_bodies,
            "the shard said this mover walks through bodies and the client did not hear it"
        );
        assert!(
            view.player.war,
            "a `0x20` took the stance away from the `0x72` that set it"
        );
    }

    /// `0x2C` — `docs/combat.md` D9/P4. The one packet that puts a ghost on
    /// screen; before this arm existed the byte decoded fine and `apply` had
    /// nowhere to put it.
    #[test]
    fn death_status_is_the_players_own() {
        let mut view = WorldView::entered(start());
        assert!(!view.player.dead, "a session starts alive");

        assert!(view.apply(&ServerPacket::DeathStatus(DeathStatus { dead: true })));
        assert!(view.player.dead);
        assert!(
            !view.apply(&ServerPacket::DeathStatus(DeathStatus { dead: true })),
            "the same status twice is not a change"
        );

        assert!(view.apply(&ServerPacket::DeathStatus(DeathStatus { dead: false })));
        assert!(!view.player.dead, "resurrection un-says it");
    }

    #[test]
    fn attack_target_is_the_players_server_set_aim() {
        let mut view = WorldView::entered(start());
        assert_eq!(view.player.attacking, None);

        assert!(view.apply(&ServerPacket::AttackTarget(
            openshard_protocol::combat::AttackTarget {
                target: Some(other())
            }
        )));
        assert_eq!(view.player.attacking, Some(other()));
        assert!(
            !view.apply(&ServerPacket::AttackTarget(
                openshard_protocol::combat::AttackTarget {
                    target: Some(other())
                }
            )),
            "the same aim twice settles"
        );
        assert!(view.apply(&ServerPacket::AttackTarget(
            openshard_protocol::combat::AttackTarget { target: None }
        )));
        assert_eq!(view.player.attacking, None);
    }

    #[test]
    fn health_bars_land_on_the_mobile_they_name() {
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::MobileIncoming(MobileIncoming {
            serial: other(),
            body: Graphic(0x00D6),
            position: Point::new(1476, 1770, 20),
            facing: Facing::walking(Direction::South),
            hue: Hue::NONE,
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Neutral,
            equipment: Vec::new(),
        }));

        assert!(view.apply(&ServerPacket::Health(
            openshard_protocol::combat::HealthBar::exact(view.player.serial, 120, 45)
        )));
        assert_eq!(
            view.player.hits,
            Some(Vitals {
                current: 45,
                max: 120
            })
        );

        assert!(view.apply(&ServerPacket::Health(
            openshard_protocol::combat::HealthBar::scaled(other(), 80, 20)
        )));
        assert_eq!(
            view.mobiles.get(&other()).and_then(|mobile| mobile.hits),
            Some(Vitals {
                current: 25,
                max: 100
            })
        );
        assert!(
            !view.apply(&ServerPacket::Health(
                openshard_protocol::combat::HealthBar::scaled(Serial::new(0x7A).unwrap(), 80, 20)
            )),
            "a bar for something not on screen creates no phantom mobile"
        );
    }

    #[test]
    fn a_status_reply_fills_only_the_players_numbers_and_refreshes_its_hits() {
        let mut view = WorldView::entered(start());
        let status = status_of(view.player.serial);

        assert!(view.apply(&ServerPacket::MobileStatus(status.clone())));
        let held = view
            .player
            .status
            .as_ref()
            .expect("the status belongs to the player");
        assert_eq!(held.name, "Lord British");
        assert_eq!(held.stamina, Vitals { current: 49, max: 50 });
        assert_eq!(held.max_weight, 450);
        assert_eq!(
            view.player.hits,
            Some(status.hits),
            "one home for health-bar hits"
        );
        assert!(
            !view.apply(&ServerPacket::MobileStatus(status)),
            "the same reply does not make a second world change"
        );

        assert!(
            !view.apply(&ServerPacket::MobileStatus(status_of(other()))),
            "a connection-only reply cannot create status for somebody else"
        );
        assert_eq!(
            view.player.status.as_ref().map(|status| status.name.as_str()),
            Some("Lord British")
        );
    }

    /// A stranger's `0x88` says whether *they* are at war, and this client is
    /// not. The bit is dropped rather than folded: nothing draws a stranger's
    /// stance — their frame has no toggle on it — and folding it would have our
    /// own doll show war because somebody else drew a sword.
    #[test]
    fn a_strangers_paperdoll_does_not_put_this_client_into_war_mode() {
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::OpenPaperdoll(
            openshard_protocol::mobile::OpenPaperdoll {
                serial: other(),
                text: "a guard".to_owned(),
                flags: PaperdollFlags::WARMODE,
            },
        ));
        assert!(!view.player.war);
    }

    /// Closing a window is a click and no packet carries it — the third place
    /// this is true. What must *not* go with it is the equipment: it belongs to
    /// the mobile and the body is still standing there.
    #[test]
    fn closing_a_paperdoll_leaves_the_mobile_dressed() {
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::MobileIncoming(MobileIncoming {
            serial: other(),
            body: Graphic(0x0190),
            position: Point::new(1476, 1770, 20),
            facing: Facing::walking(Direction::South),
            hue: Hue(0x83EA),
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
            equipment: vec![shirt()],
        }));
        view.apply(&paperdoll_of(other()));

        assert!(view.paperdoll_closed(other()));
        assert!(view.paperdolls.is_empty());
        assert_eq!(view.mobiles.get(&other()).unwrap().equipment, vec![shirt()]);
        assert!(!view.paperdoll_closed(other()), "a stale click closes nothing");
    }

    /// A body that walked out of range takes its paperdoll with it — the window
    /// would otherwise keep drawing the equipment as it stood when the mobile
    /// left. `Mobile.Destroy` in the reference does the same.
    #[test]
    fn a_mobile_leaving_closes_its_paperdoll() {
        let mut view = WorldView::entered(start());
        view.apply(&paperdoll_of(other()));
        assert!(view.apply(&ServerPacket::Remove(Remove { serial: other() })));
        assert!(view.paperdolls.is_empty());
    }

    /// The shard may open a paperdoll on a serial this client has never been
    /// shown — a `0x88` is an instruction, not a description of the screen.
    /// Recording it is what lets the window draw a title over an empty body
    /// rather than nothing at all.
    #[test]
    fn a_paperdoll_for_a_mobile_never_seen_is_still_recorded() {
        let mut view = WorldView::entered(start());
        assert!(view.apply(&paperdoll_of(other())));
        assert!(view.paperdolls.contains_key(&other()));
        assert!(!view.mobiles.contains_key(&other()));
    }

    fn chest() -> Serial {
        Serial::new(0x4000_0100).unwrap()
    }

    fn candle() -> ContainedItem {
        ContainedItem {
            serial: Serial::new(0x4000_0101).unwrap(),
            graphic: Graphic(0x0A28),
            amount: ItemAmount(1),
            at: GumpPoint::new(44, 65),
            grid: openshard_protocol::containers::GridSlot(0),
            hue: Hue::NONE,
        }
    }

    fn opened(container: Serial) -> ServerPacket {
        ServerPacket::OpenContainer(openshard_protocol::containers::OpenContainer {
            container,
            gump: Graphic(0x003C),
        })
    }

    /// What a double-clicked chest actually looks like on the wire: the window
    /// and its contents are two packets, and both have to land for anything to
    /// be drawn.
    #[test]
    fn opening_a_chest_is_a_window_and_a_listing() {
        let mut view = WorldView::entered(start());
        let movement_before = (view.player.position, view.player.facing);
        assert!(view.apply(&opened(chest())));
        assert!(view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: Some(chest()),
            items: vec![candle()],
        })));

        assert_eq!(view.containers.get(&chest()), Some(&Graphic(0x003C)));
        assert_eq!(view.contents.get(&chest()).unwrap(), &[candle()]);
        assert_eq!(
            (view.player.position, view.player.facing),
            movement_before,
            "a double-clicked container only changes the container kernel"
        );
    }

    /// The listing that cannot say what it is about. Nothing to fold in, and
    /// nothing to clear either — the window that came before it is what says
    /// the container is open.
    #[test]
    fn an_empty_listing_leaves_the_window_alone() {
        let mut view = WorldView::entered(start());
        view.apply(&opened(chest()));
        assert!(!view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: None,
            items: Vec::new(),
        })));
        assert!(view.containers.contains_key(&chest()));
    }

    /// A shard re-opening a container it has already shown is re-drawing it, and
    /// the listing behind the `0x24` is the truth. Keeping the old items would
    /// leave something the shard has since taken out on screen.
    #[test]
    fn re_opening_a_container_drops_what_was_in_it() {
        let mut view = WorldView::entered(start());
        view.apply(&opened(chest()));
        view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: Some(chest()),
            items: vec![candle()],
        }));
        view.apply(&opened(chest()));
        assert!(!view.contents.contains_key(&chest()));
    }

    /// A `0x25` for an item already listed is the shard saying its stack grew,
    /// not that there are two of it — the reference client replaces the record.
    #[test]
    fn an_addition_replaces_the_item_it_names() {
        let mut view = WorldView::entered(start());
        view.apply(&opened(chest()));
        assert!(view.apply(&ServerPacket::AddToContainer(AddToContainer {
            item: candle(),
            container: chest(),
        })));
        let grown = ContainedItem {
            amount: ItemAmount(7),
            ..candle()
        };
        assert!(view.apply(&ServerPacket::AddToContainer(AddToContainer {
            item: grown,
            container: chest(),
        })));
        assert_eq!(view.contents.get(&chest()).unwrap(), &[grown]);
        assert!(
            !view.apply(&ServerPacket::AddToContainer(AddToContainer {
                item: grown,
                container: chest(),
            })),
            "the same record twice settles"
        );
    }

    #[test]
    fn a_container_destination_retires_the_lifters_stale_ground_source() {
        let mut view = WorldView::entered(start());
        let item = WorldItem {
            serial: candle().serial,
            graphic: candle().graphic,
            payload: WorldItemPayload::Stack(candle().amount),
            position: Point::new(1000, 2000, 5),
            hue: candle().hue,
            light: None,
            flags: ItemFlags::NONE,
        };
        view.apply(&ServerPacket::WorldItem(item));
        assert!(view.apply(&ServerPacket::AddToContainer(AddToContainer {
            item: candle(),
            container: chest(),
        })));
        assert!(!view.items.contains_key(&item.serial));
        assert_eq!(view.contents.get(&chest()), Some(&vec![candle()]));
    }

    #[test]
    fn moving_an_equipped_item_into_a_bag_retires_its_paperdoll_copy() {
        let mut view = WorldView::entered(start());
        view.player.equipment.push(shirt());

        assert!(view.apply(&ServerPacket::AddToContainer(AddToContainer {
            item: ContainedItem {
                serial: shirt().serial,
                graphic: shirt().graphic,
                amount: ItemAmount(1),
                at: GumpPoint::new(20, 30),
                grid: Default::default(),
                hue: shirt().hue,
            },
            container: chest(),
        })));
        assert!(view.player.equipment.is_empty());
        assert_eq!(view.contents.get(&chest()).unwrap().len(), 1);
    }

    #[test]
    fn equipping_a_bag_item_retires_its_container_copy() {
        let mut view = WorldView::entered(start());
        view.contents.insert(chest(), vec![candle()]);

        assert!(view.apply(&ServerPacket::EquipUpdate(
            openshard_protocol::items::EquipUpdate {
                item: candle().serial,
                graphic: candle().graphic,
                layer: openshard_protocol::wire::Layer(1),
                mobile: view.player.serial,
                hue: candle().hue,
            },
        )));
        assert!(view.contents.get(&chest()).unwrap().is_empty());
        assert_eq!(view.player.equipment.len(), 1);
    }

    /// There is no "taken out of the container" packet: an item leaving a bag is
    /// a `0x1D` and nothing else, so a `0x1D` has to reach the contents or the
    /// icon stays in the window forever.
    #[test]
    fn taking_an_item_out_of_a_bag_is_a_plain_remove() {
        let mut view = WorldView::entered(start());
        view.apply(&opened(chest()));
        view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: Some(chest()),
            items: vec![candle()],
        }));
        assert!(view.apply(&ServerPacket::Remove(Remove {
            serial: candle().serial
        })));
        assert!(view.contents.get(&chest()).unwrap().is_empty());
    }

    /// Removing the container itself — dropped, stolen, decayed — takes its
    /// window with it. Nothing else would ever close it: the wire has no packet
    /// that does.
    #[test]
    fn removing_the_chest_closes_its_window() {
        let mut view = WorldView::entered(start());
        view.apply(&opened(chest()));
        view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: Some(chest()),
            items: vec![candle()],
        }));
        assert!(view.apply(&ServerPacket::Remove(Remove { serial: chest() })));
        assert!(!view.containers.contains_key(&chest()));
        assert!(!view.contents.contains_key(&chest()));
    }

    /// A vendor's goods arrive as a listing for the crate worn on its shop
    /// layer, and the window is a `0x24` naming the *vendor* — so contents with
    /// no window of their own are a shape the shard sends on purpose, and this
    /// end keeps them.
    #[test]
    fn a_listing_with_no_window_is_still_written_down() {
        let mut view = WorldView::entered(start());
        assert!(view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: Some(chest()),
            items: vec![candle()],
        })));
        assert!(view.contents.contains_key(&chest()));
        assert!(!view.containers.contains_key(&chest()));
    }

    /// What a lost shard leaves behind, and what it must not.
    ///
    /// Every table here was something the shard said, and the moment it stops
    /// answering none of them is about anything — but a picture that goes on
    /// looking right is what made a disconnect read as a game gone strange.
    /// The journal is the exception, and it has to be: the line announcing the
    /// loss is written into it.
    #[test]
    fn a_lost_shard_puts_out_the_world_it_described_and_says_so() {
        let mut view = WorldView::entered(start());
        let vendor = other();
        view.apply(&ServerPacket::MobileIncoming(MobileIncoming {
            serial: vendor,
            body: Graphic(0x0190),
            position: Point::new(1476, 1770, 20),
            facing: Facing::walking(Direction::South),
            hue: Hue::NONE,
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
            equipment: Vec::new(),
        }));
        view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: Some(chest()),
            items: vec![candle()],
        }));
        view.apply(&ServerPacket::OpenContainer(
            openshard_protocol::containers::OpenContainer {
                container: chest(),
                gump: Graphic(0x003C),
            },
        ));
        view.apply(&paperdoll_of(view.player.serial));
        let said = view.journal.len();

        view.apply(&revision_of(vendor, 0xABCD));
        // `0x99` and not `0xD6`, which this used to name: the shard does send a
        // `0xD6` and this client now reads it, so the example has to be an id
        // the framing table genuinely has no row for — the same correction
        // `connection.rs`'s own test carries.
        view.shard_lost("unknown packet 0x1E");

        assert!(view.mobiles.is_empty(), "nobody is standing there any more");
        assert!(view.containers.is_empty(), "no window offers to send a packet");
        assert!(view.contents.is_empty());
        assert!(view.paperdolls.is_empty());
        assert!(view.items.is_empty());
        assert!(view.tooltips.is_empty(), "and no name outlived what it named");
        assert!(view.target.is_none());
        assert_eq!(view.journal.len(), said + 1, "and the log gained the reason");
        let last = view.journal.back().expect("the line");
        assert!(last.serial.is_none(), "the system said it, not a mobile");
        assert!(last.text.contains("unknown packet 0x1E"));
        assert_eq!(
            view.player.serial,
            start().serial,
            "the body the camera is anchored to stays; `App::walk` is what stops moving it"
        );
    }

    #[test]
    fn a_vendor_buy_list_joins_its_stock_crate_even_before_the_vendor_body_arrives() {
        let mut view = WorldView::entered(start());
        let vendor = other();
        let stock = Serial::new(0x4000_0099).unwrap();
        let item = candle();
        // The shop's `0x2E` can beat the ordinary `0x78` for this mobile.
        // It must remain useful rather than being discarded merely because
        // the body record is still in flight.
        assert!(view.apply(&ServerPacket::EquipUpdate(
            openshard_protocol::items::EquipUpdate {
                item: stock,
                graphic: Graphic(0x0E3F),
                layer: openshard_protocol::wire::Layer(0x1A),
                mobile: vendor,
                hue: Hue::NONE,
            },
        )));
        assert!(view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: Some(stock),
            items: vec![item],
        })));
        assert!(
            view.apply(&ServerPacket::BuyList(openshard_protocol::vendor::BuyList {
                container: stock,
                lines: vec![BuyLine {
                    price: 5,
                    name: "candle".to_owned(),
                }],
            }))
        );
        assert!(view.apply(&ServerPacket::OpenContainer(
            openshard_protocol::containers::OpenContainer {
                container: vendor,
                gump: Graphic(0x0030),
            },
        )));
        assert_eq!(
            view.vendor_buys.get(&vendor).map(|buy| buy.container),
            Some(stock)
        );
        assert_eq!(view.vendor_buys[&vendor].lines[0].price, 5);
    }

    /// Closing a window is a click and no packet carries it — the same fact as
    /// `gump_closed`. The contents go with it, so the `0x25`s a shard keeps
    /// pushing at a window this end has shut land nowhere.
    #[test]
    fn closing_a_container_is_this_end_knowing_something_the_wire_never_said() {
        let mut view = WorldView::entered(start());
        view.apply(&opened(chest()));
        view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: Some(chest()),
            items: vec![candle()],
        }));
        assert!(view.container_closed(chest()));
        assert!(!view.containers.contains_key(&chest()));
        assert!(!view.contents.contains_key(&chest()));
        assert!(
            !view.container_closed(chest()),
            "closing it twice is a stale click"
        );
    }

    fn entry(id: u8, value: u16) -> openshard_protocol::skill::SkillEntry {
        openshard_protocol::skill::SkillEntry {
            id,
            value,
            base: value,
            lock: SkillLock::Up,
            cap: 1000,
        }
    }

    /// The whole list fills the table, keyed by the id the row carried.
    #[test]
    fn the_whole_skill_list_fills_the_table_by_id() {
        let mut view = WorldView::entered(start());
        assert!(view.player.skills.is_empty(), "nothing is trained before a 0x3A");
        assert!(
            view.apply(&ServerPacket::SkillsFull(openshard_protocol::skill::SkillsFull {
                entries: vec![entry(0, 755), entry(45, 500)],
            }))
        );
        assert_eq!(view.player.skills.len(), 2);
        assert_eq!(view.player.skills[&45].value, 500);
        assert_eq!(
            view.player.skills[&0].lock,
            SkillLock::Up,
            "the lock rides with the line"
        );
    }

    /// A second whole list *replaces*. The shard is stating every skill it has,
    /// so a row missing from the new one is a row that is gone — a client that
    /// merged would go on drawing it at the value it last had, for ever.
    #[test]
    fn a_second_whole_list_replaces_rather_than_merges() {
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::SkillsFull(openshard_protocol::skill::SkillsFull {
            entries: vec![entry(0, 755), entry(45, 500)],
        }));
        assert!(
            view.apply(&ServerPacket::SkillsFull(openshard_protocol::skill::SkillsFull {
                entries: vec![entry(0, 755)],
            }))
        );
        assert_eq!(view.player.skills.len(), 1);
        assert!(!view.player.skills.contains_key(&45));
    }

    /// A delta folds into the table, which is the whole of the difference
    /// between the two packets at this end.
    #[test]
    fn a_single_line_moves_one_skill_and_leaves_the_rest_standing() {
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::SkillsFull(openshard_protocol::skill::SkillsFull {
            entries: vec![entry(0, 755), entry(45, 500)],
        }));
        assert!(view.apply(&ServerPacket::SkillUpdate(
            openshard_protocol::skill::SkillUpdate {
                entry: entry(45, 501),
            }
        )));
        assert_eq!(view.player.skills[&45].value, 501);
        assert_eq!(view.player.skills[&0].value, 755, "the rest stands");
        assert!(
            !view.apply(&ServerPacket::SkillUpdate(
                openshard_protocol::skill::SkillUpdate {
                    entry: entry(45, 501),
                }
            )),
            "the same line twice moved nothing"
        );
    }

    /// A skill a whole list never named still lands: the shard may train
    /// something this client was not told about, and dropping the line would
    /// leave the window a row short with no way to notice.
    #[test]
    fn a_delta_may_name_a_skill_no_list_did() {
        let mut view = WorldView::entered(start());
        assert!(view.apply(&ServerPacket::SkillUpdate(
            openshard_protocol::skill::SkillUpdate {
                entry: entry(25, 300),
            }
        )));
        assert_eq!(view.player.skills[&25].value, 300);
    }

    /// The one thing a step must not do. `0x20` and `0x78` rebuild the whole
    /// `Player`, and a fresh table there would empty a standing window every
    /// time the body moved.
    #[test]
    fn a_step_does_not_empty_the_skill_table() {
        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::SkillsFull(openshard_protocol::skill::SkillsFull {
            entries: vec![entry(0, 755)],
        }));
        view.apply(&ServerPacket::PlayerUpdate(PlayerUpdate {
            serial: view.player.serial,
            body: view.player.body,
            hue: Hue::NONE,
            flags: StatusFlags::NONE,
            position: Point::new(1476, 1770, 20),
            facing: view.player.facing,
        }));
        assert_eq!(view.player.skills.len(), 1, "the step kept the table");
    }

    fn revision_of(serial: Serial, hash: u32) -> ServerPacket {
        ServerPacket::TooltipRevision(openshard_protocol::properties::TooltipRevision { serial, hash })
    }

    fn list_of(serial: Serial, hash: u32, name: &str) -> ServerPacket {
        ServerPacket::PropertyListReply(openshard_protocol::properties::PropertyListReply {
            serial,
            hash,
            entries: vec![openshard_protocol::properties::PropertyEntry {
                cliloc: openshard_protocol::wire::ClilocId(1_050_045),
                arguments: format!(" \t{name}\t "),
            }],
        })
    }

    /// The shard's `version` mode, which is what it ships as: a revision arrives
    /// with the object and the list only if this end asks. The point of the
    /// assertion is the *middle* state — told a revision, holding no list — as
    /// that is the only thing that makes the client ask.
    #[test]
    fn a_revision_makes_a_tooltip_stale_and_the_list_settles_it() {
        let mut view = WorldView::entered(start());
        assert!(view.apply(&revision_of(other(), 0xABCD)));
        assert!(
            view.tooltips[&other()].stale(),
            "told a revision, holding nothing"
        );

        assert!(view.apply(&list_of(other(), 0xABCD, "Lord British")));
        assert!(!view.tooltips[&other()].stale());
        assert_eq!(view.tooltips[&other()].entries[0].arguments, " \tLord British\t ");

        assert!(
            !view.apply(&list_of(other(), 0xABCD, "Lord British")),
            "the same list again changes nothing"
        );
    }

    /// The shard's `full` mode sends the list unasked and never sends a `0xDC`
    /// at all. Modelled as one "do we have it" flag, this would read as
    /// permanently stale and the client would re-ask on every single hover for
    /// a list it was already holding.
    #[test]
    fn a_list_that_arrives_unasked_is_not_stale() {
        let mut view = WorldView::entered(start());
        assert!(view.apply(&list_of(other(), 0x1234, "a dagger")));
        assert_eq!(view.tooltips[&other()].revision, None, "no 0xDC ever came");
        assert!(!view.tooltips[&other()].stale());
    }

    /// The object changed. The old lines stay put until the new ones arrive, so
    /// a hover mid-round-trip draws a name one edit out of date rather than a
    /// blank — a blank reads as "this thing has no name".
    #[test]
    fn a_newer_revision_keeps_the_lines_it_supersedes() {
        let mut view = WorldView::entered(start());
        view.apply(&revision_of(other(), 1));
        view.apply(&list_of(other(), 1, "a dagger"));
        assert!(view.apply(&revision_of(other(), 2)));

        assert!(view.tooltips[&other()].stale());
        assert_eq!(
            view.tooltips[&other()].entries[0].arguments,
            " \ta dagger\t ",
            "still drawable while the new list is in flight"
        );
    }

    #[test]
    fn a_removed_object_takes_its_tooltip_with_it() {
        let mut view = WorldView::entered(start());
        view.apply(&revision_of(other(), 1));
        assert!(view.apply(&ServerPacket::Remove(Remove { serial: other() })));
        assert!(view.tooltips.is_empty());
    }

    fn party_list(members: &[Serial]) -> ServerPacket {
        ServerPacket::PartyMemberList(openshard_protocol::party::PartyMemberList {
            members: members.to_vec(),
        })
    }

    /// The roster arrives whole on every change, so it is replaced rather than
    /// merged — and an invitation is answered by the roster that follows it,
    /// which nothing on the wire says outright.
    #[test]
    fn joining_a_party_replaces_the_roster_and_clears_the_invitation() {
        let mut view = WorldView::entered(start());
        assert!(view.apply(&ServerPacket::PartyInvitation(
            openshard_protocol::party::PartyInvitation { leader: other() }
        )));
        assert_eq!(view.party.invited_by, Some(other()));
        assert!(view.party.is_empty());

        assert!(view.apply(&party_list(&[other(), view.player.serial])));
        assert_eq!(view.party.leader(), Some(other()), "the first row leads");
        assert_eq!(view.party.invited_by, None, "joining answered the question");

        assert!(
            !view.apply(&party_list(&[other(), view.player.serial])),
            "the same roster again changes nothing"
        );
    }

    /// The empty list is how a client is told it is in no party — the packet has
    /// no other way to say it, and reading it as "a removal from a party I am
    /// still in" would leave the window up over nobody.
    #[test]
    fn an_empty_removal_is_the_end_of_the_party() {
        let mut view = WorldView::entered(start());
        view.apply(&party_list(&[other(), view.player.serial]));
        assert!(view.apply(&ServerPacket::PartyRemoveMember(
            openshard_protocol::party::PartyRemoveMember {
                removed: view.player.serial,
                members: Vec::new(),
            }
        )));
        assert!(view.party.is_empty());
        assert_eq!(view.party.leader(), None);
    }

    /// Party chat has no `TalkMode`, so the channel is prefixed into the name
    /// the way ServUO formats these for a listener. Asserted because the
    /// tempting alternative — a `TalkMode::Other` holding a party packet type —
    /// would put a 4 in a field documented as the mode byte a client sent.
    #[test]
    fn a_party_line_says_which_channel_it_came_on() {
        let mut view = WorldView::entered(start());
        assert!(view.apply(&ServerPacket::PartyTextMessage(
            openshard_protocol::party::PartyTextMessage {
                to_all: true,
                from: other(),
                text: "regroup".to_owned(),
            }
        )));
        let line = view.journal.back().expect("a line");
        assert_eq!(line.name, "[Party]");
        assert_eq!(line.text, "regroup");
        assert_eq!(line.mode, TalkMode::Regular, "it is not a talk mode");

        view.apply(&ServerPacket::PartyTextMessage(
            openshard_protocol::party::PartyTextMessage {
                to_all: false,
                from: other(),
                text: "you first".to_owned(),
            },
        ));
        assert_eq!(view.journal.back().expect("a line").name, "[Party tell]");
    }

    /// A house cursor is a cursor *and* a house, and the two travel together.
    ///
    /// The invariant worth a test is the second half: a plain `0x6C` arriving
    /// after a `0x99` must stop drawing the house. Held as two `Option`s side by
    /// side that would be a packet away from a villa following the pointer
    /// through a "whom shall I examine?" — which is `combat.md`'s D1, in a
    /// different colour.
    #[test]
    fn a_house_cursor_carries_its_house_and_a_plain_one_takes_it_away() {
        use openshard_protocol::target::{MultiTargetRequest, TargetKind};
        use openshard_protocol::wire::{CursorId, MultiId};

        let mut view = WorldView::entered(start());
        view.apply(&ServerPacket::MultiTarget(MultiTargetRequest {
            cursor_id: CursorId(7),
            kind: TargetKind::Location,
            multi: MultiId(0x64),
            offset: (0, 0, 0),
        }));
        let open = view.target.expect("a cursor is up");
        assert_eq!(open.cursor.cursor_id, CursorId(7));
        assert_eq!(open.cursor.kind, TargetKind::Location);
        assert_eq!(open.multi, Some(MultiId(0x64)));

        view.apply(&ServerPacket::TargetCursor(TargetCursor {
            cursor_id: CursorId(8),
            kind: TargetKind::Object,
        }));
        let open = view.target.expect("the new cursor is up");
        assert_eq!(open.cursor.cursor_id, CursorId(8));
        assert_eq!(
            open.multi, None,
            "a plain cursor kept drawing the house from the one before it"
        );
    }

    /// A designed house announces a revision, and the view remembers which one.
    ///
    /// The shape itself never reaches this layer — see [`WorldView::designs`] —
    /// so what is asserted is the cache key and nothing else. That is the whole
    /// of what the wire told us.
    #[test]
    fn a_design_revision_is_remembered_by_serial() {
        use openshard_protocol::design::{DesignRevision, Revision};
        use openshard_protocol::serial::RawSerial;

        let mut view = WorldView::entered(start());
        let house = Serial::new(0x4000_0001).unwrap();

        assert!(view.apply(&ServerPacket::DesignRevision(DesignRevision {
            serial: RawSerial(house.raw()),
            revision: Revision(4),
        })));
        assert_eq!(view.designs.get(&house), Some(&4));

        // The same revision again is not a change, so nothing redraws for it.
        assert!(!view.apply(&ServerPacket::DesignRevision(DesignRevision {
            serial: RawSerial(house.raw()),
            revision: Revision(4),
        })));
        // A newer one is.
        assert!(view.apply(&ServerPacket::DesignRevision(DesignRevision {
            serial: RawSerial(house.raw()),
            revision: Revision(5),
        })));
        assert_eq!(view.designs.get(&house), Some(&5));
    }

    /// A house that comes down takes its revision with it. A serial the shard
    /// reuses must not inherit the picture of what stood there before — which is
    /// the same failure a stale tooltip would be, one layer over.
    #[test]
    fn a_removed_house_forgets_its_design_revision() {
        use openshard_protocol::design::{DesignRevision, Revision};
        use openshard_protocol::serial::RawSerial;

        let mut view = WorldView::entered(start());
        let house = Serial::new(0x4000_0001).unwrap();
        view.apply(&ServerPacket::DesignRevision(DesignRevision {
            serial: RawSerial(house.raw()),
            revision: Revision(4),
        }));

        assert!(view.apply(&ServerPacket::Remove(Remove { serial: house })));
        assert!(
            view.designs.is_empty(),
            "a demolished house left its revision behind"
        );
    }
}
