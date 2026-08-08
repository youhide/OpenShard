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
//! are one and not two. `0x11` (a mobile's paperdoll numbers) decodes too,
//! but is not folded in below: it is status-bar data, not a position or an
//! appearance, and belongs with whatever eventually models the status bar rather
//! than with a record of what is on screen.
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

use std::collections::{HashMap, VecDeque};

use openshard_protocol::containers::ContainedItem;
use openshard_protocol::direction::Facing;
use openshard_protocol::gump::layout::{Element, parse};
use openshard_protocol::gump::{GumpId, GumpKey, GumpPoint};
use openshard_protocol::mobile::{Equipment, Notoriety, PaperdollFlags, StatusFlags};
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::speech::{Font, SpokenMessage, TalkMode, UnicodeMessage};
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::{MapSize, PlayerStart, Point};

/// How many lines of speech the journal keeps.
///
/// A bound rather than a `Vec` that grows forever, because the thing this is
/// built for is a client that stays logged in: a virtual player standing in a
/// town square hears every line said near it, and nothing ever asks it to
/// forget. The number is what a scrollback is worth reading, not a memory
/// budget — the oldest line is dropped, silently, which is what a journal does.
pub const JOURNAL_LINES: usize = 256;

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
    /// Poisoned, invisible, war mode. Same absence as `hue` until the first
    /// `0x20`.
    pub flags: StatusFlags,
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
    /// What it is wearing.
    ///
    /// Only `0x78` carries this; a `0x77` move leaves it as it was; see
    /// [`WorldView::apply`].
    pub equipment: Vec<Equipment>,
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

impl Heard {
    /// From a `0x1C` — Latin-1 speech.
    #[must_use]
    pub fn ascii(message: &SpokenMessage) -> Self {
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

    /// From a `0xAE` — Unicode speech.
    ///
    /// The four-byte language tag on the wire is dropped here rather than
    /// carried into the journal: nothing downstream reads it, and it names a
    /// property of the *sender* (what client locale sent this), not of the
    /// line itself — keeping it would be a field every future consumer has to
    /// notice does nothing.
    #[must_use]
    pub fn unicode(message: &UnicodeMessage) -> Self {
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

/// An item on the ground, as `0x1A` last described it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Item {
    /// Its graphic.
    pub graphic: Graphic,
    /// How many are in the stack.
    pub amount: u16,
    /// Where it lies.
    pub position: Point,
    /// Its hue, or [`Hue::NONE`] for none.
    pub hue: Hue,
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
    pub mobiles: HashMap<Serial, Mobile>,
    /// Every ground item this client has been shown, by serial.
    pub items: HashMap<Serial, Item>,
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
    pub containers: HashMap<Serial, Graphic>,
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
    pub contents: HashMap<Serial, Vec<ContainedItem>>,
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
    pub paperdolls: HashMap<Serial, Paperdoll>,
}

/// A paperdoll window the shard has opened here.
///
/// Two fields and no equipment — see [`WorldView::paperdolls`]. The serial is
/// the key rather than a field, for [`WorldView::containers`]' reason: nothing
/// can hold a paperdoll for a mobile other than the one it is filed under.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Paperdoll {
    /// The title across the top: the name, plus any honorific the shard chose to
    /// put in front of it. Not the mobile's name as anything else knows it — the
    /// wire's `0x88` is the only place this string exists.
    pub name: String,
    /// War mode, and whether this client may lift what is worn. The second is
    /// the shard's answer and not a guess: it is set for your own paperdoll and
    /// for a pet's, and clear for a stranger's.
    pub flags: PaperdollFlags,
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
    /// The world as the entry packet described it: nobody else on screen yet.
    #[must_use]
    pub fn entered(start: PlayerStart) -> Self {
        Self {
            player: Player {
                serial: start.serial,
                body: start.body,
                hue: Hue::NONE,
                flags: StatusFlags::NONE,
                position: start.position,
                facing: start.facing,
                equipment: Vec::new(),
            },
            map: start.map,
            mobiles: HashMap::new(),
            items: HashMap::new(),
            journal: VecDeque::new(),
            gumps: Vec::new(),
            containers: HashMap::new(),
            contents: HashMap::new(),
            paperdolls: HashMap::new(),
        }
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

    /// Forget a dialog this client has just answered.
    ///
    /// The one thing about a gump the server never says. A reply button closes
    /// the window *on the client* — that is what the reference client does, and
    /// what the server assumes when it waits for one `0xB1` per window — so the
    /// close is knowledge this end has and no packet carries, the same shape as
    /// [`player_stepped`](Self::player_stepped) learning a position from an ack
    /// that carries none.
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
            // Speech, a system line, an NPC — all one journal, whichever of the
            // two wire shapes it arrived as.
            ServerPacket::SpokenMessage(line) => {
                self.heard(Heard::ascii(line));
                // Always a change: the same sentence said twice is two lines in
                // a journal, unlike a position that is set twice to one tile.
                true
            }
            // `0xAE`: the shape a client that spoke `0xAD` gets its own words
            // back as — see `Heard`'s docs for why this cannot be skipped.
            ServerPacket::UnicodeMessage(line) => {
                self.heard(Heard::unicode(line));
                true
            }
            ServerPacket::PlayerUpdate(update) => {
                let fresh = Player {
                    serial: self.player.serial,
                    body: update.body,
                    hue: update.hue,
                    flags: update.flags,
                    position: update.position,
                    facing: update.facing,
                    // `0x20` is a position and an appearance, never a paperdoll:
                    // keeping what the `0x78` said is the difference between a
                    // client that still knows its backpack and one that forgets
                    // it the first time the server nudges the body.
                    equipment: self.player.equipment.clone(),
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
            ServerPacket::MobileIncoming(incoming) if incoming.serial == self.player.serial => {
                let fresh = Player {
                    serial: self.player.serial,
                    body: incoming.body,
                    hue: incoming.hue,
                    flags: incoming.flags,
                    position: incoming.position,
                    facing: incoming.facing,
                    equipment: incoming.equipment.clone(),
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
                    equipment: incoming.equipment.clone(),
                };
                let changed = self.mobiles.get(&incoming.serial) != Some(&fresh);
                self.mobiles.insert(incoming.serial, fresh);
                changed
            }
            ServerPacket::WorldItem(item) => {
                let fresh = Item {
                    graphic: item.graphic,
                    amount: item.amount,
                    position: item.position,
                    hue: item.hue,
                };
                let changed = self.items.get(&item.serial) != Some(&fresh);
                self.items.insert(item.serial, fresh);
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
                self.containers.insert(open.container, open.gump) != Some(open.gump)
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
                let held = self.contents.entry(added.container).or_default();
                match held.iter_mut().find(|item| item.serial == added.item.serial) {
                    Some(item) if *item == added.item => false,
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
                    name: paperdoll.text.clone(),
                    flags: paperdoll.flags,
                };
                let changed = self.paperdolls.get(&paperdoll.serial) != Some(&fresh);
                self.paperdolls.insert(paperdoll.serial, fresh);
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
                // And so does a mobile: a body that walked out of range cannot
                // be looked at any more, and the window over it would keep
                // drawing the equipment as it stood when the body left. The
                // reference does exactly this, in `Mobile.Destroy` — and only
                // for a mobile that is not the player, which needs no guard
                // here because a `0x1D` never names our own serial.
                let had_paperdoll = self.paperdolls.remove(&remove.serial).is_some();
                had_mobile || had_item || was_held || had_window || had_paperdoll
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::containers::{AddToContainer, ContainerContents};
    use openshard_protocol::direction::Direction;
    use openshard_protocol::items::WorldItem;
    use openshard_protocol::mobile::{MobileIncoming, MobileMove, Remove};
    use openshard_protocol::world::PlayerUpdate;

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
            position: start().position,
            facing: start().facing,
            hue: Hue(0x83EA),
            flags: StatusFlags::NONE,
            notoriety: Notoriety::Innocent,
            equipment: vec![shirt()],
        };
        assert!(view.apply(&ServerPacket::MobileIncoming(mine.clone())));
        assert!(view.mobiles.is_empty(), "we are not one of the others");
        assert_eq!(view.player.equipment, vec![shirt()]);
        assert_eq!(view.player.hue, Hue(0x83EA));
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
            amount: 500,
            position: Point::new(1000, 2000, 5),
            hue: Hue(0x0021),
        };
        assert!(view.apply(&ServerPacket::WorldItem(item)));
        assert_eq!(view.items.get(&item.serial).unwrap().amount, 500);
    }

    #[test]
    fn a_remove_forgets_whichever_map_actually_holds_the_serial() {
        // The client does not distinguish a mobile walking out of range from an
        // item being picked up; neither does Remove — it just tries both maps.
        let mut view = WorldView::entered(start());
        let item = WorldItem {
            serial: Serial::new(0x4000_00AB).unwrap(),
            graphic: Graphic(0x0EED),
            amount: 1,
            position: Point::new(1000, 2000, 5),
            hue: Hue::NONE,
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
        assert_eq!(open.name, "Lord British");
        assert_eq!(open.flags, PaperdollFlags::CAN_LIFT);
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
            amount: 1,
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
        assert!(view.apply(&opened(chest())));
        assert!(view.apply(&ServerPacket::ContainerContents(ContainerContents {
            container: Some(chest()),
            items: vec![candle()],
        })));

        assert_eq!(view.containers.get(&chest()), Some(&Graphic(0x003C)));
        assert_eq!(view.contents.get(&chest()).unwrap(), &[candle()]);
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
            amount: 7,
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
}
