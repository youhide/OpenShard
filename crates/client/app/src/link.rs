//! The shard, on a thread of its own.
//!
//! A window's event loop is not async and a socket is, so the two meet through
//! a channel in each direction: what the player does goes out as a [`Command`],
//! and what the server says comes back as a [`Update`] the event loop is woken
//! for.
//!
//! Nothing about the protocol is decided here. `client/net` owns the login
//! conversation, the walk handshake and the [`WorldView`]; this file owns the
//! thread they run on, and the rule that the renderer never sees a half-applied
//! packet — a snapshot is published after the whole of one has been folded in.
//!
//! # Why a thread and not a runtime in the event loop
//!
//! The event loop blocks on the compositor and the runtime blocks on the
//! socket, and neither can be asked to poll the other: a frame must not wait on
//! a packet, and a packet must not wait for the window to be uncovered. So the
//! socket gets a current-thread runtime of its own and the two exchange values.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use openshard_client_net::action::Outgoing;
use openshard_client_net::chunks::{Drain, Fetch, FetchError, Fetched, Restart};
use openshard_client_net::connection::Event;
use openshard_client_net::session::Plan;
use openshard_client_net::transport::{Dial, Socket, enter_world_with};
use openshard_client_net::view::WorldView;
use openshard_client_net::walk::{Moved, Walk};
use openshard_protocol::chunks::{
    Changes, ChangesReply, ChangesRequest, PublishNotice, WorldNotice, WorldRevision,
};
use openshard_protocol::feedback::{
    Animation, GraphicalEffect, HarvestCompleted, HarvestRefused, HarvestToolVisual, NewAnimation,
    SwingTiming,
};
use openshard_protocol::gump::GumpId;
use openshard_protocol::gump::GumpPoint;
use openshard_protocol::items::ItemAmount;
use openshard_protocol::packet::FramedClientPacket;
use openshard_protocol::serial::Serial;
use openshard_protocol::speech::TalkMode;
use openshard_protocol::target::TargetResponse;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::world::Point;
use openshard_protocol::world::ResyncRequest;
use openshard_protocol::world::StepSequence;

/// Where this client's own body is *drawn*, which is not where the
/// [`WorldView`] says it is.
///
/// The view is the record of what the server said, and the server says where a
/// step landed only once it has acked it — a round trip after the player asked.
/// Waiting for that is what makes a walk lag and stutter: the body stands still
/// for the latency, then crosses its tile, then stands still again.
///
/// So the picture runs on [`Walk::predicted`] instead: the tile the last `0x02`
/// asked for, which this end knows the instant it sends one. The two agree on
/// every step the server allows, and where it does not — a `0x21`, or a `0x20`
/// putting the body somewhere it did not walk to — the prediction is thrown away
/// and replaced by the server's word, which is the rollback and is flagged as
/// one.
///
/// Deliberately *not* done by moving the view: a record of what arrived that
/// contained a guess would have no way left to tell the two apart, which is the
/// argument in `client/net`'s `walk` module docs. The guess travels beside it.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    /// The tile and facing to draw, ahead of the server's confirmation.
    pub predicted: openshard_client_net::walk::Predicted,
    /// Whether it got there by a correction rather than by walking.
    ///
    /// A correction is *jumped* to and never glided: the body is not walking
    /// back the tile it mispredicted, it was never there. It also ends the pace
    /// measurement — the gap between a step and a rollback is not a walking
    /// speed. See [`crate::crowd::Crowd::snap`].
    pub corrected: bool,
}

/// The movement fact a packet carried across the app boundary.
///
/// Ordinary world packets deliberately have no value of this type.  Keeping
/// this distinct from the latest local [`Body`] prediction prevents a vendor,
/// speech line, or item update from being mistaken for a player relocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Movement {
    /// The shard accepted this locally numbered step and thereby confirmed its
    /// destination.
    Ack {
        sequence: StepSequence,
        confirmed: openshard_client_net::walk::Predicted,
    },
    /// The shard refused a step and supplied the position to use instead.
    Reject {
        /// The refused pending step. Subsequent pending steps are invalidated
        /// by the correction, but this identity records the event's source.
        sequence: StepSequence,
        confirmed: openshard_client_net::walk::Predicted,
    },
    /// A packet relocated the player independently of the walk handshake.
    Relocation {
        confirmed: openshard_client_net::walk::Predicted,
    },
    /// The server turned the character in place.  Unlike a relocation, this
    /// preserves steps already accepted locally and their visual transitions.
    Turn {
        confirmed: openshard_client_net::walk::Predicted,
    },
}

impl Movement {
    pub const fn confirmed(self) -> openshard_client_net::walk::Predicted {
        match self {
            Self::Ack { confirmed, .. }
            | Self::Reject { confirmed, .. }
            | Self::Relocation { confirmed }
            | Self::Turn { confirmed } => confirmed,
        }
    }
}

/// Where this connection's ground comes from.
///
/// Not a `bool` and not an `Option`, because both answers are a *source*: a
/// client that opened a facet on its own disk is not a client missing one. The
/// window decides it — see [`crate::WorldSource`], of which this is the half the
/// wire cares about — and it decides one thing here: whether the login is
/// followed by a fetch before anything is reported.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GroundSource {
    /// The window opened a facet before it dialled, from the install or from a
    /// base set. Nothing is asked of the shard, and a stock shard notices no
    /// difference — every client before `to_the_client.md`'s E2.
    OwnDisk,
    /// The shard's own, fetched over this connection.
    ///
    /// A [`WorldNotice`](openshard_protocol::chunks::WorldNotice) says how big
    /// the facet is, `chunks_of` it is the list to ask for, and what comes back
    /// is [`Update::Ground`]. A shard that sends no notice has no ground to
    /// give, and this client has none of its own: the connection ends and says
    /// so.
    ///
    /// `cache` is the directory the last world this shard gave us was kept in —
    /// E3, and [`openshard_client_net::cache`] is where the file's name and its
    /// rules live. A world already there at the revision the shard names costs
    /// no chunks at all; one behind costs the difference.
    Fetched {
        /// Where kept worlds live. The client's own working directory, beside
        /// `client_ui.toml`, and named by the caller rather than assumed here:
        /// this file knows about a socket, not about where a client keeps
        /// things.
        cache: std::path::PathBuf,
    },
}

/// What the shard thread tells the window.
///
/// **Not `Clone`.** [`Update::Ground`] carries a whole facet, and a
/// [`MapSnapshot`](openshard_map::snapshot::MapSnapshot) has one owner per
/// process by construction — see that type's doc, which is where the rule is
/// argued. Nothing has ever cloned an update; this is what stops one from
/// starting.
#[derive(Debug)]
pub enum Update {
    /// The world as it now stands. Sent whenever a packet changed anything —
    /// whole rather than as a delta, because a renderer wants what to draw and
    /// not what moved.
    World {
        /// What the server has said, entire.
        ///
        /// No [`Body`] beside it: the body is the [`Walk`]'s answer, the walk
        /// belongs to the event-loop owner, and a world entered is exactly
        /// where that walk starts. The owner builds one from this view rather
        /// than being handed a second opinion about it.
        view: Box<WorldView>,
    },
    /// A decoded server packet, for the event-loop owner to apply.
    ///
    /// Every packet the shard sent, undivided: this thread no longer knows
    /// which of them move the player, because [`Walk`] is what decides that and
    /// [`Walk`] is the owner's. See [`fold`], which the owner calls, and this
    /// module's own docs for why the split moved.
    Mutation {
        packet: openshard_protocol::server_packet::ServerPacket,
        /// Stamped after decoding, before this packet waits for the window.
        received: Instant,
    },
    /// The server asked one mobile to play a one-shot body animation.
    Animation(Animation),
    /// The server asked one mobile to play a modern, body-agnostic animation.
    NewAnimation(NewAnimation),
    /// A graphical effect — today, always an arrow or bolt in flight.
    Effect(GraphicalEffect),
    /// The exact duration of the immediately following swing animation.
    SwingTiming(SwingTiming),
    /// A backpack harvesting tool to draw for the immediately following action.
    HarvestToolVisual(HarvestToolVisual),
    /// The shard declined a harvest the client began optimistically.
    HarvestRefused(HarvestRefused),
    /// The shard finished a harvest and has queued its result, if any.
    HarvestCompleted(HarvestCompleted),
    /// A designed house's picture, still as bytes.
    ///
    /// The one packet that crosses this seam undecoded, and it has a reason:
    /// reading a `0xD8` needs the house's width and height, which no field on
    /// the wire carries and which come out of the foundation's own multi. This
    /// thread has a socket and no client files; the window has the files. So the
    /// bytes travel and the decode happens where the box is knowable.
    Design(Vec<u8>),
    /// The facet, assembled out of every chunk of it the shard sent.
    ///
    /// Sent for a connection whose [`GroundSource`] is
    /// [`Fetched`](GroundSource::Fetched), and **after** [`Update::World`] — by
    /// however long a facet takes to arrive.
    ///
    /// Once per connection in the ordinary run, and it is no longer *only* once:
    /// a publish the shard cannot name chunk by chunk — `Changes::Everything`,
    /// which is a patch that touched more squares than a packet can list — is
    /// answered by taking the facet again, and that arrives here. Whoever folds
    /// this in has to be able to fold in a second one over a window that is
    /// already drawing; see [`Update::GroundMoved`], which is the ordinary shape
    /// of the same event and where the invalidation is argued.
    ///
    /// That gap is the whole cost of E2 and it is deliberate. The other order
    /// was available: hold the world back until the ground is here, and the
    /// window would never exist without a facet. It was refused because the
    /// packets that keep arriving during the fetch have to go *somewhere*, and
    /// the only two places are this thread's own unbounded buffer or the
    /// bounded mailbox the window drains — which the window cannot drain while
    /// it is waiting for a value this thread is holding back. So the gap is
    /// real, and it is closed by a gate rather than by an ordering: see
    /// `crate::resources::Resources::grounded`.
    ///
    /// `Box` for [`World`](Update::World)'s reason, and it is the reason the
    /// engine's style allows one at all: a `MapSnapshot` is by some way the
    /// largest thing that crosses this seam, and every other variant would be
    /// sized for it.
    Ground {
        snapshot: Box<openshard_map::snapshot::MapSnapshot>,
        /// Where this world is on disk, when it is anywhere.
        ///
        /// A world off the wire is kept as a base set of ours (see
        /// [`openshard_client_net::cache`]), and that file is the only thing a
        /// bake can be built beside: a navigation graph is stamped against the
        /// world it was built from, and a world with no file has nothing to
        /// stamp. `None` is a cache that could not be written — the ground is
        /// still perfectly good to walk on, and it is the long routes that go
        /// without.
        kept: Option<std::path::PathBuf>,
    },
    /// The squares of ground a publish moved, for the world the window is
    /// holding.
    ///
    /// `to_the_client.md`'s E4, and the reason it is chunks rather than a facet
    /// is ownership: the shard thread gave the world away with [`Ground`] and a
    /// [`MapSnapshot`](openshard_map::snapshot::MapSnapshot) has one owner per
    /// process, so the only copy there is to apply them over is the window's.
    /// `openshard_movement::ground::Ground::take_chunks` is the seam at the far
    /// end, and it rebakes the spans in the same statement for the reason that
    /// type exists.
    ///
    /// **This is the one update that invalidates what has already been drawn.**
    /// [`Ground`](Update::Ground) arrives before the first frame and can throw
    /// nothing away because nothing has been built yet; this one arrives in the
    /// middle of play, and every cache over the facet — the composited blocks,
    /// the radar's products, the route this end had planned — is a picture of the
    /// world as it was.
    ///
    /// The coarse navigation graph is *dropped* rather than rebuilt, which is the
    /// same bargain the shard takes for the same reason: a graph is eleven
    /// seconds of flood and a router planning through a wall somebody just built
    /// is worse than no router at all. Long routes go back to the bounded search
    /// until the client reconnects.
    ///
    /// [`Ground`]: Update::Ground
    GroundMoved {
        /// Every chunk that moved, each at the revision the publish named.
        chunks: Vec<openshard_map::chunk::Chunk>,
    },
    /// The coarse navigation graph, once something on this side has one.
    ///
    /// Loaded beside the world or baked from it, and either way off the frame
    /// loop: a facet's graph is ~11 s of flood, which is not a thing to do
    /// between two frames. Until it arrives, long routes are the bounded search
    /// alone — a client plans out of a building with 600 nodes or does not plan
    /// at all.
    ///
    /// `Box` for [`Ground`](Update::Ground)'s reason: it is the second largest
    /// thing that crosses this seam, and every other variant would be sized for
    /// it.
    Navigation {
        graph: Box<openshard_movement::NavigationGraph>,
        /// The artifact it was read from, or written to. What the HUD names, so
        /// that "which graph is this client running" is answerable without
        /// reading the terminal back.
        path: std::path::PathBuf,
    },
    /// The graph could not be had, and why.
    ///
    /// A refusal is an answer here: a client that asked for one and never heard
    /// back would sit at "building…" for the rest of the run, which reads as a
    /// hang and is not one.
    NavigationLost { why: String },
    /// The connection ended, and why. Nothing further will arrive.
    ///
    /// The window stays open on one of these: a client that vanished when a
    /// shard restarted would take the reason with it.
    Lost(String),
}

const MAX_ORDERED_UPDATES: usize = 256;
const COMMAND_CAPACITY: usize = 16;

/// Updates crossing from the shard thread to the application thread.
///
/// A network mutation is a fact in a sequence and is never merged with another
/// one.  Local movement events are also ordered: each has a protocol sequence
/// and starts exactly one visual transition.  Coalescing them would lose the
/// identity that an acknowledgement must retire.
///
/// The producer asks the platform loop to wake only when this mailbox changes
/// from idle to non-idle. The loop drains it as one staged batch, rather than
/// carrying one platform user event per packet or frame update.
#[derive(Clone)]
pub struct Updates {
    mailbox: Arc<UpdateMailbox>,
}

struct UpdateMailbox {
    pending: Mutex<PendingUpdates>,
    /// Wakes the shard thread once the application has made room for another
    /// ordered update.
    space: Condvar,
    capacity: usize,
}

#[derive(Default)]
struct PendingUpdates {
    /// Whether a platform wake-up is already in flight for this batch.
    notified: bool,
    /// Every update whose order must be retained, across all mutation stages.
    ordered: usize,
    /// Whether this mailbox has already reported that it reached the
    /// ordered-update limit. One line establishes backpressure without making
    /// a sustained busy connection drown its normal log in identical warnings.
    backpressure_reported: bool,
    stages: VecDeque<UpdateStage>,
}

enum UpdateStage {
    /// Facts whose order is part of their meaning.
    Ordered(VecDeque<Update>),
}

impl Updates {
    /// Start an empty staged mailbox.
    pub fn new() -> Self {
        Self::with_capacity(MAX_ORDERED_UPDATES)
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            mailbox: Arc::new(UpdateMailbox {
                pending: Mutex::new(PendingUpdates::default()),
                space: Condvar::new(),
                capacity,
            }),
        }
    }

    /// Publish an update and say whether the caller must wake the application
    /// loop. The caller owns the actual platform wake-up, so this module stays
    /// independent of `winit`.
    pub fn publish(&self, update: Update) -> bool {
        let mut pending = self
            .mailbox
            .pending
            .lock()
            .expect("the update mailbox is not poisoned");
        // Mutations cannot be merged or dropped. Stopping the socket
        // reader here applies backpressure all the way to TCP instead
        // of allowing an unfocused or GPU-blocked window to consume
        // unbounded memory while it falls behind.
        while pending.ordered == self.mailbox.capacity {
            if !pending.backpressure_reported {
                tracing::warn!(
                    capacity = self.mailbox.capacity,
                    "ordered update mailbox is full; applying socket backpressure"
                );
                pending.backpressure_reported = true;
            }
            pending = self
                .mailbox
                .space
                .wait(pending)
                .expect("the update mailbox is not poisoned");
        }
        pending.ordered += 1;
        match pending.stages.back_mut() {
            Some(UpdateStage::Ordered(updates)) => updates.push_back(update),
            _ => pending
                .stages
                .push_back(UpdateStage::Ordered(VecDeque::from([update]))),
        }
        if pending.notified {
            false
        } else {
            pending.notified = true;
            true
        }
    }

    /// Take every update staged before this call, in its original semantic
    /// order. Clearing `notified` while holding the lock closes the race with a
    /// producer that arrives between this drain and the next platform wait.
    pub fn take(&self) -> Vec<Update> {
        let mut pending = self
            .mailbox
            .pending
            .lock()
            .expect("the update mailbox is not poisoned");
        pending.notified = false;
        pending.ordered = 0;
        // A drain ends this backpressure episode. If the App falls behind
        // again later, report that distinct condition once too; keeping this
        // latched for the process lifetime would hide a renewed stall.
        pending.backpressure_reported = false;
        let stages = std::mem::take(&mut pending.stages);
        self.mailbox.space.notify_all();
        stages
            .into_iter()
            .flat_map(|stage| match stage {
                UpdateStage::Ordered(updates) => updates,
            })
            .collect()
    }
}

impl Default for Updates {
    fn default() -> Self {
        Self::new()
    }
}

pub use openshard_client_net::action::GumpReply;

/// What the window asks the shard thread to send.
///
/// One variant per thing a player can do that leaves this process. Open rather
/// than a bare `Facing` because the three are unrelated: a step is answered by
/// the walk handshake, a line of speech is answered by everyone in earshot
/// hearing it, and a dialog answer is answered by whatever the shard does about
/// it. Nothing here is a packet yet — the thread builds those, so this side
/// never touches the wire.
#[derive(Clone, Debug)]
pub enum Command {
    /// A packet to put on the wire exactly as given.
    ///
    /// The one command that arrives already encoded, and the reason is the map:
    /// a `0x02` names the tile a step is asking for, which only a terrain
    /// lookup can answer, and the terrain lives beside the owner's
    /// `MapSnapshot`. A resync request rides the same variant — it is what the
    /// owner sends when its [`Walk`] loses track, and it carries no fields at
    /// all.
    ///
    /// [`FramedClientPacket`] rather than a bare `Vec<u8>`: this thread no
    /// longer knows *which* packet it is about to write, so nothing here can
    /// notice a caller handing over half of one, two end to end, or bytes for
    /// an id nobody registered. Both of the two things that ride this variant
    /// are wrapped by their own encoder — `Walk::step` for the `0x02` and
    /// [`Link::resync`] for the `0x22` — which is what keeps that check at the
    /// one place per packet that can answer it without looking, instead of
    /// duplicated at every call site.
    Send(FramedClientPacket),
    /// An ordinary network action. Its packet mapping is owned by `client-net`.
    ///
    /// Still encoded on the thread: an action needs the player's serial and the
    /// client version, both of which the login conversation produced here, and
    /// none of them needs a map.
    Outgoing(Outgoing),
}

/// Which of a locally-closed window's state to drop from a [`WorldView`].
///
/// One variant per kind [`WorldView`] itself distinguishes a close for — see
/// [`WorldView::paperdoll_closed`], [`WorldView::container_closed`] and
/// [`WorldView::gump_closed`]. Not [`WindowSubject`][crate::WindowSubject]:
/// that type also names a skills tree, which is this client's own state and
/// has nothing in the view to forget.
///
/// # It is not a command, despite living here
///
/// This used to be the payload of a `Command::CloseWindow` that crossed the
/// channel to the link thread. That variant is gone — S2 in
/// `docs/client_window_state.md` retired it for the `locally_closed` overlay —
/// and what is left is a plain argument to `App::apply_close_window`, which
/// writes the event-loop thread's own view. It stays in this module because it
/// names the three `WorldView` methods and nothing else does.
#[derive(Clone, Copy, Debug)]
pub enum CloseTarget {
    /// A paperdoll, named by the mobile it draws.
    Paperdoll(Serial),
    /// A container, named by its own serial.
    Container(Serial),
    /// A dialog, named by the gump id the shard opened it under.
    Gump(GumpId),
    /// A spellbook, named by its item serial.
    Spellbook(Serial),
}

/// The handle the window keeps: somewhere to send commands.
///
/// Dropping it closes the command channel, which is what ends the thread's
/// loop when the window goes away.
#[derive(Debug)]
pub struct Link {
    commands: tokio::sync::mpsc::Sender<Command>,
}

impl Link {
    /// Queue one command without making the window event loop wait for a slow
    /// socket. The walking controller already rate-limits steps and the other
    /// commands are button presses, so reaching this bound means the shard task
    /// cannot currently make progress; keeping an unbounded backlog would only
    /// replay stale input later. The server remains authoritative either way.
    fn send(&self, command: Command) {
        match self.commands.try_send(command) {
            Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("shard command queue is full; dropping stale input");
            }
        }
    }

    /// Put one already-encoded, already-checked step on the wire.
    ///
    /// The packet comes from the owner's own [`Walk`], which is where the
    /// prediction and its terrain lookup live — see [`Command::Send`]. Taking
    /// [`FramedClientPacket`] rather than raw bytes means the "is this really
    /// one whole packet" question is answered by whoever encoded it, and never
    /// again on the way here. `Walk::step` is that place, and it needs no
    /// [`ClientVersion`] to answer: `0x02` is seven bytes for every client
    /// there has ever been. This thread never learns the connection's version
    /// either, because nothing it does needs to.
    ///
    /// A closed channel is ignored rather than reported: it means the shard
    /// thread has already ended, and it has already said why. The same holds
    /// for everything below.
    pub fn step(&self, packet: FramedClientPacket) {
        self.send(Command::Send(packet));
    }

    /// Ask the shard where this character actually is.
    ///
    /// Sent when the owner's [`Walk`] has lost track of the handshake and
    /// cannot repair it by guessing. `Walk` has already stopped sending steps
    /// by then; this is the other half, and without it the walk never starts
    /// again.
    pub fn resync(&self) {
        // `0x22` (resynchronise) does not vary with the client version — see
        // `client_packet_length` — so `None` costs nothing here, and it is
        // the honest answer besides: a `Link` never learns the connection's
        // version at all, this being the one command it builds by itself.
        let bytes = ResyncRequest.encode();
        let packet = FramedClientPacket::new(bytes, None)
            .expect("ResyncRequest::encode always writes exactly one whole 0x22 packet");
        self.send(Command::Send(packet));
    }

    /// Send one action the caller has already chosen.
    ///
    /// The one entry point that does not name its own action, and it exists for
    /// [`panes::Effect::Net`](crate::panes::Effect::Net): a pane hands back an
    /// [`Outgoing`] rather than reaching for a method here, so that what a
    /// window asks the shard for is a *value* the manager can order, log or
    /// refuse. Every method below is this with its action spelled out, which is
    /// what a call site with a fixed action should still read as.
    pub fn act(&self, action: Outgoing) {
        self.send(Command::Outgoing(action));
    }

    /// Say a line, on the channel `mode` names.
    ///
    /// The mode is not decoration: `TalkMode::Guild` and `TalkMode::Alliance`
    /// are what make a line reach a roster instead of whoever is standing
    /// nearby, and the shard branches on the byte before it measures a
    /// distance. See `chat::Channel`.
    pub fn say(&self, text: String, mode: TalkMode) {
        self.send(Command::Outgoing(Outgoing::Say { text, mode }));
    }

    /// Say a line to the party — which is not speech at all, but `0xBF 0x06`.
    pub fn say_to_party(&self, text: String) {
        self.send(Command::Outgoing(Outgoing::PartySay(text)));
    }

    // No party wrappers at all any more — no `accept_party`, `decline_party`,
    // `add_to_party` or `remove_from_party`. Every one of them
    // had exactly one caller, in the shell's edge-triggered request, and the
    // party is two of this client's own gump windows now: the invitation is
    // `panes::confirm` and the roster is `panes::party`. A pane names what it
    // wants as `Effect::Net(Outgoing::PartyAccept)` and never reaches a `Link`
    // it is not allowed to hold (decision 5). The packets themselves are
    // untouched — see `openshard_client_net::party`.

    /// Answer an open dialog.
    pub fn answer_gump(&self, reply: GumpReply) {
        self.send(Command::Outgoing(Outgoing::AnswerGump(reply)));
    }

    /// Use an object — the double-click.
    pub fn use_object(&self, serial: Serial) {
        self.send(Command::Outgoing(Outgoing::Use(serial)));
    }

    /// Ask the shard to open this mobile's paperdoll, rather than using it.
    pub fn paperdoll(&self, serial: Serial) {
        self.send(Command::Outgoing(Outgoing::Paperdoll(serial)));
    }

    /// Answer a target cursor the shard raised after using a tool or skill.
    pub fn target(&self, response: TargetResponse) {
        self.send(Command::Outgoing(Outgoing::Target(response)));
    }

    /// Put an item from an open container onto the cursor.
    pub fn pick_up_item(&self, item: Serial, amount: ItemAmount) {
        self.send(Command::Outgoing(Outgoing::PickUp { item, amount }));
    }

    // No `drop_into` and no `equip`: where a held item is put down is named by
    // a [`PendingDrop`](crate::hand::PendingDrop), and that type turns
    // itself into the packet — one place, so a fourth destination is a compile
    // error rather than a `match` somebody forgot. `App::perform`'s
    // `Effect::Drop` arm sends it through [`Link::act`], the way step 1's
    // `buy`/`sell` and step 5's six doll requests went.

    /// Drop a held item onto another item, allowing the shard's normal stack rule.
    pub fn drop_onto_item(&self, item: Serial, target: Serial) {
        self.send(Command::Outgoing(Outgoing::DropInto {
            item,
            container: target,
            at: GumpPoint::new(0, 0),
        }));
    }

    /// Drop the cursor item at a world position.
    pub fn drop_on_ground(&self, item: Serial, at: Point) {
        self.send(Command::Outgoing(Outgoing::DropOnGround { item, at }));
    }

    // No `buy` and no `sell`: the shop's order is asked for by
    // `panes::vendor::VendorPane`, which names an `Outgoing` and hands it to
    // the manager rather than reaching a `Link` at all (decision 5), and
    // `App::perform` sends it through [`Link::act`]. A named method here would
    // be a second door into the same packet, open only to whoever already holds
    // the link.

    /// Ask for a stance. See [`Outgoing::WarMode`].
    pub fn war_mode(&self, war: bool) {
        self.send(Command::Outgoing(Outgoing::WarMode(war)));
    }

    /// Aim at a mobile. See [`Outgoing::Attack`].
    pub fn attack(&self, mobile: Serial) {
        self.send(Command::Outgoing(Outgoing::Attack(mobile)));
    }

    /// Give up the current combat target. See [`Outgoing::StopAttacking`].
    pub fn stop_attacking(&self) {
        self.send(Command::Outgoing(Outgoing::StopAttacking));
    }

    // No `log_out`, `status`, `skills`, `quest_log`, `guild_menu` and no
    // `virtue`: every request a paperdoll's furniture makes is asked for by
    // `panes::paperdoll::PaperdollPane` as an `Effect::Net` and sent through
    // [`Link::act`], the way the vendor's two went at step 1 of
    // `docs/window_components.md`. `war_mode` stays because it has a caller
    // that is not a window: Tab.

    // No `set_skill_lock` and no `use_skill`: the skill sheet asks for both as
    // `Effect::Net(Outgoing::SkillLock | UseSkill)` and the router sends them
    // through `Link::act`, the same way the vendor's two went at step 1. A
    // named wrapper for an `Outgoing` a pane already names is a second spelling
    // of one packet.

    /// Ask for the tooltips of these objects, in one `0xD6`.
    ///
    /// Driven by the hover rather than by everything on screen, and by
    /// [`Tooltips`](crate::tooltips::Tooltips) rather than by a caller counting
    /// frames — see that module for why both.
    pub fn query_properties(&self, serials: Vec<Serial>) {
        self.send(Command::Outgoing(Outgoing::QueryProperties(serials)));
    }

    /// Ask for a designed house's picture, in a `0xBF 0x1E`.
    ///
    /// Driven by the revision the shard named rather than by anything on
    /// screen: a house whose shape this client already holds is never asked
    /// about, which is the whole reason the revision is a packet of its own.
    pub fn query_design(&self, house: Serial) {
        self.send(Command::Outgoing(Outgoing::QueryDesign(house)));
    }
}

/// Log in on a thread of its own, and report back through `proxy`.
///
/// Returns as soon as the thread is spawned: the login conversation is several
/// round trips and a window that waited for it would open blank and frozen.
///
/// **No map and no tile definitions.** The walk predicts the height of a step
/// and the server does not send one, but the terrain that answers it belongs to
/// the process's one [`MapSnapshot`](openshard_map::snapshot::MapSnapshot) — so the
/// prediction happens beside it and this thread receives a `0x02` already
/// encoded. What crosses in the other direction is decoded packets; the owner
/// folds them into its own [`Walk`].
///
/// `dial` is how the connection is opened and the only thing here that knows
/// what a socket is: `Tcp` for a shard on a network, and something else for one
/// in this process. It is moved onto the thread, so it is `Send`.
pub fn connect<D, F>(dial: D, plan: Plan, version: ClientVersion, ground: GroundSource, report: F) -> Link
where
    D: Dial + Send + 'static,
    F: Fn(Update) + Send + 'static,
{
    let (sender, commands) = tokio::sync::mpsc::channel(COMMAND_CAPACITY);
    std::thread::Builder::new()
        .name("shard".to_owned())
        .spawn(move || run(dial, plan, version, ground, &report, commands))
        // The thread is the connection; a client that could not spawn it has
        // nothing to fall back to, and the OS refusing a thread at startup is
        // not a condition worth a variant in `Update`.
        .expect("the shard thread starts");
    Link { commands: sender }
}

/// The thread body: one runtime, one login, then packets and steps until either
/// end stops.
fn run<D: Dial, F: Fn(Update) + Send>(
    dial: D,
    plan: Plan,
    version: ClientVersion,
    ground: GroundSource,
    report: &F,
    commands: tokio::sync::mpsc::Receiver<Command>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            report(Update::Lost(format!("no runtime for the shard: {error}")));
            return;
        }
    };
    runtime.block_on(async move {
        let reason = play(dial, plan, version, ground, report, commands).await;
        report(Update::Lost(reason));
    });
}

/// How often a fetch says where it has got to.
///
/// Felucca is 7,168 chunks, so one line a thousand is seven of them — enough to
/// tell a slow link from a stalled one, and few enough to sit in the same
/// terminal as `run`'s own startup checkpoints without becoming the whole of it.
const PROGRESS_EVERY: usize = 1_024;

/// Put every request the fetch is ready to make on the wire.
///
/// In a loop until it says no: at the start of a fetch that is as many requests
/// as [`IN_FLIGHT_CHUNKS`](openshard_client_net::chunks::IN_FLIGHT_CHUNKS)
/// allows, and after each chunk completes it is at most one. The pacing is the
/// fetch's; what is here is the socket.
async fn ask<D: Dial>(socket: &mut Socket<D::Stream>, fetch: &mut Fetch) -> Result<(), String> {
    while let Some(request) = fetch.next_request() {
        if let Err(error) = socket.send(&request.encode()).await {
            return Err(error.to_string());
        }
    }
    Ok(())
}

/// What this connection is still doing about the ground.
///
/// `None` of it is a connection with nothing left to do — a client that opened
/// its own facet, one whose kept world was already current, and one whose fetch
/// has finished. The first two arms are the two waits a client starts with, and
/// both are *before* the window has ground: see [`Update::Ground`]. The third is
/// the one that can happen at any time, because a publish can.
enum Pending {
    /// A world was kept and the shard is at a newer revision, so it has been
    /// asked what moved. Nothing else can be decided until the answer lands.
    ///
    /// The world is held here rather than reported early on purpose: it is a
    /// revision behind, and handing the window a world the shard has already
    /// moved past would draw ground that is knowably wrong for as long as the
    /// difference takes to arrive.
    Asking {
        /// The world as it was kept.
        held: openshard_map::snapshot::MapSnapshot,
    },
    /// Chunks are coming, whether that is the facet or only what moved.
    Fetching(Fetch),
    /// The ground moved while it was arriving, so the fetch that was running has
    /// been abandoned and its last answers are being thrown away.
    ///
    /// Nothing goes out on the wire while this lasts: the shard answers a chunk
    /// request exactly once and nothing in an answer says which request it
    /// belongs to, so asking again now would put two sets of answers on the wire
    /// with no way to tell them apart. See
    /// [`Fetch::abandon`](openshard_client_net::chunks::Fetch::abandon), which
    /// is where that is argued, and [`Restart`], which is what goes out when the
    /// drain is empty.
    Draining {
        /// What the abandoned fetch is still owed.
        drain: Drain,
        /// What to ask for once it is owed nothing.
        restart: Restart,
    },
}

/// What a connection does about the ground, once the shard has described it.
///
/// The three answers are E3 in one place: the world is already here, the world
/// is here but behind, or there is no world to start from. Each is reported as
/// it is decided, because a person watching a blank window wants to know which
/// of the three is happening.
fn decide(cache: &std::path::Path, notice: WorldNotice) -> Result<Decided, FetchError> {
    let held = match openshard_client_net::cache::read(cache, notice) {
        Ok(held) => held,
        Err(reason) => {
            // Every way a kept world is not usable ends here, and none of them
            // is fatal: what it costs is the fetch this whole mechanism exists
            // to avoid, so the reason is worth a line every time.
            eprintln!("the ground comes from the shard: {reason}");
            return Fetch::of(notice).map(Decided::Fetching);
        }
    };
    if held.revision().get() == notice.revision.0 {
        eprintln!(
            "the ground is the one we kept: facet {}, revision {}, nothing to ask for",
            notice.facet.0, notice.revision.0
        );
        return Ok(Decided::Held(held));
    }
    eprintln!(
        "the ground we kept is revision {} and the shard is at {}: asking what moved",
        held.revision().get(),
        notice.revision.0
    );
    Ok(Decided::Asking(held))
}

/// [`decide`]'s three answers, before any of them has touched the socket.
enum Decided {
    /// The kept world is the shard's world. Nothing is asked for at all.
    Held(openshard_map::snapshot::MapSnapshot),
    /// The kept world is behind: the shard is asked what moved.
    Asking(openshard_map::snapshot::MapSnapshot),
    /// There is no world to start from, so the facet is fetched whole.
    Fetching(Fetch),
}

/// What to do with the shard's answer about what moved.
///
/// Three answers again, and only two outcomes: either the world in hand is the
/// world after all, or chunks are coming. Which chunks — the difference or the
/// whole facet — is the shard's to say, and by the time this returns it is the
/// same `Fetch` either way.
fn what_moved(
    notice: WorldNotice,
    held: openshard_map::snapshot::MapSnapshot,
    reply: &ChangesReply,
) -> Result<WhatMoved, FetchError> {
    match &reply.changes {
        Changes::Everything => {
            // A revision the shard cannot place, a log it could not read, or
            // more chunks than a packet can name. All three are one thing to do.
            eprintln!(
                "the shard cannot say what moved since revision {}: taking the facet again",
                held.revision().get()
            );
            Fetch::of(notice).map(WhatMoved::These)
        }
        Changes::These(chunks) if chunks.is_empty() => {
            // A legal answer, and rarer than it looks: an empty patch moves the
            // revision without moving a tile. The world in hand is the world;
            // what it keeps is the older number, so the next connection asks
            // this same question again and is told the same thing.
            eprintln!(
                "the world is at revision {} and no chunk of it changed",
                reply.revision.0
            );
            Ok(WhatMoved::Nothing(held))
        }
        Changes::These(chunks) => {
            eprintln!(
                "{} chunk(s) moved since the world we kept: fetching those",
                chunks.len()
            );
            Fetch::over(
                notice,
                held,
                chunks.clone(),
                openshard_map::snapshot::MapRevision::decoded(reply.revision.0),
            )
            .map(WhatMoved::These)
        }
    }
}

/// [`what_moved`]'s two outcomes.
///
/// Named for the question rather than for the answer, because
/// [`Moved`](openshard_client_net::walk::Moved) is a walk's word in this same
/// file and the two are nothing to do with each other.
enum WhatMoved {
    /// Nothing did: the world already in hand is the shard's.
    Nothing(openshard_map::snapshot::MapSnapshot),
    /// These chunks are coming — the difference, or the facet.
    These(Fetch),
}

/// What to do about a publish, told to a client that is already drawing.
///
/// [`what_moved`] one phase later and with one thing different: there is no
/// world in hand here to fill in. The facet went to the window with
/// [`Update::Ground`] a whole fetch ago, so what comes back is chunks — unless
/// the shard could not name them, in which case the answer is the same one E2
/// starts with, and the facet arrives whole a second time.
///
/// `None` is a notice with nothing in it, which no shard of ours sends: an
/// [`Option`] and not a two-armed enum beside [`WhatMoved`] because both of that
/// one's arms carry a value and only one of these does.
fn published(notice: WorldNotice, published: &PublishNotice) -> Result<Option<Fetch>, FetchError> {
    match &published.changes {
        // A shard of ours does not send one: an empty patch moves no square, so
        // there is nothing to announce and `mapedit::commit` says nothing. A
        // client that is told anyway has nothing to fetch and nothing to redraw.
        Changes::These(chunks) if chunks.is_empty() => {
            eprintln!(
                "the ground is at revision {} and no chunk of it changed",
                published.revision.0
            );
            Ok(None)
        }
        Changes::Everything => {
            // One patch that moved more squares than a packet can list. Rare
            // enough that no shipped command can make one — an operator's edit
            // is one op and one chunk — and the honest answer is the one the
            // client already knows how to carry out.
            eprintln!(
                "the ground moved to revision {} in more chunks than can be named: taking the \
                 facet again",
                published.revision.0
            );
            Fetch::of(notice).map(Some)
        }
        Changes::These(chunks) => {
            eprintln!(
                "the ground moved to revision {}: fetching {} chunk(s)",
                published.revision.0,
                chunks.len()
            );
            Fetch::moved(
                notice,
                chunks.clone(),
                openshard_map::snapshot::MapRevision::decoded(published.revision.0),
            )
            .map(Some)
        }
    }
}

/// Where an abandoned fetch leaves this connection: draining, or already asking
/// again because there was nothing left to drain.
///
/// The second is not a rare case. A fetch asks in whole requests and the window
/// only empties, so a publish that lands in the gap between the last answer and
/// the next request finds nothing outstanding at all — and waiting for a packet
/// that is not coming would leave the ground one revision behind for the rest of
/// the connection.
async fn resume<D: Dial>(
    socket: &mut Socket<D::Stream>,
    drain: Drain,
    restart: Restart,
    notice: WorldNotice,
) -> Result<Pending, String> {
    if drain.is_empty() {
        return begin::<D>(socket, restart, notice).await;
    }
    Ok(Pending::Draining { drain, restart })
}

/// Put a restart's first requests on the wire.
///
/// The one place a fetch starts that is not a decision about what the shard
/// said: [`Restart`] has already decided, and what is left is the socket.
async fn begin<D: Dial>(
    socket: &mut Socket<D::Stream>,
    restart: Restart,
    notice: WorldNotice,
) -> Result<Pending, String> {
    let mut fetch = match restart.begin(notice) {
        Ok(fetch) => fetch,
        Err(error) => {
            return Err(format!(
                "the ground the shard published cannot be fetched: {error}"
            ));
        }
    };
    eprintln!("the ground is asked for again: {} chunk(s)", fetch.wanted());
    ask::<D>(socket, &mut fetch).await?;
    Ok(Pending::Fetching(fetch))
}

/// Everything after the runtime exists, up to the reason it ended.
async fn play<D: Dial, F: Fn(Update) + Send>(
    dial: D,
    plan: Plan,
    version: ClientVersion,
    ground: GroundSource,
    report: &F,
    mut commands: tokio::sync::mpsc::Receiver<Command>,
) -> String {
    let (mut socket, view) = match enter_world_with(dial, plan, version).await {
        Ok(entered) => entered,
        Err(error) => return error.to_string(),
    };
    let player_serial = view.player.serial;
    // The ground, if this client was told to take the shard's. The first
    // requests go out *here*, before the world is reported, so that the fetch is
    // already on the wire while the window is folding in its first view — but it
    // finishes later, and `Update::Ground`'s own doc is where that gap is
    // argued.
    //
    // `view.world` is a copy taken before the view moves, and it is there because
    // the shard sends the notice *before* the `0x55` that ends the login
    // conversation — `enter_world_with` folds everything up to that packet in,
    // so a notice is in hand by the time this runs. `None` is a shard that has
    // no ground for this facet at all — see `World::world_notice` — and this
    // client has none of its own, so the connection ends and says which of the
    // two it was.
    let mut pending = match &ground {
        GroundSource::OwnDisk => None,
        GroundSource::Fetched { cache } => {
            let Some(notice) = view.world else {
                return "this shard has no ground for the facet, and this client opened none of its \
                        own: start it with --base-set or with the install's map files"
                    .to_owned();
            };
            match decide(cache, notice) {
                Ok(Decided::Held(held)) => {
                    // The one path that costs nothing: the world is reported
                    // before the first packet the window sees, so a cache hit
                    // looks to everything above like a client that opened a
                    // facet on its own disk.
                    report(Update::Ground {
                        snapshot: Box::new(held),
                        // The file it was just read out of, which is where a
                        // graph baked over it lives too.
                        kept: openshard_client_net::cache::path_for(cache, notice).ok(),
                    });
                    None
                }
                Ok(Decided::Asking(held)) => {
                    let asking = ChangesRequest {
                        facet: notice.facet,
                        revision: WorldRevision(held.revision().get()),
                    };
                    if let Err(error) = socket.send(&asking.encode()).await {
                        return error.to_string();
                    }
                    Some(Pending::Asking { held })
                }
                Ok(Decided::Fetching(mut fetch)) => {
                    eprintln!(
                        "the ground comes from the shard: facet {}, revision {}, {} chunks",
                        notice.facet.0,
                        notice.revision.0,
                        fetch.wanted(),
                    );
                    if let Err(reason) = ask::<D>(&mut socket, &mut fetch).await {
                        return reason;
                    }
                    Some(Pending::Fetching(fetch))
                }
                Err(error) => return format!("the world the shard described cannot be fetched: {error}"),
            }
        }
    };
    // The notice, kept for as long as there is ground on the way: it is what a
    // kept world is filed under when the fetch lands. `Copy`, so this is the
    // value and not a borrow of the view that is about to move.
    let world_notice = view.world;
    // The newest revision the shard has said this facet is at — the notice's,
    // and then every publish's. `None` is a shard that described no world at
    // all, which is the same absence `world_notice` carries and the case where
    // nothing below ever reads this.
    //
    // What it is for is the answer to `ChangesRequest`: a reply written before a
    // publish names what moved to a revision the world has already left, and the
    // pair of numbers is the only thing that tells the two apart.
    let mut latest = world_notice.map(|notice| notice.revision);
    // Where the server put us. The owner starts its `Walk` from this view, and
    // every `0x02` after it is computed there.
    report(Update::World { view: Box::new(view) });

    loop {
        tokio::select! {
            // Cancel-safe on both arms: `read` loses no bytes when the other
            // branch wins. The bounded command receiver applies backpressure
            // at the window boundary instead of growing without limit.
            event = socket.next_event() => {
                let packet = match event {
                    Ok(Some(Event::Packet(packet))) => packet,
                    // A designed house's picture. It cannot be decoded here —
                    // see `Update::Design` — so it crosses whole.
                    Ok(Some(Event::Undecoded { id, body }))
                        if id.0 == openshard_protocol::design::DesignDetail::ID =>
                    {
                        report(Update::Design(body));
                        continue;
                    }
                    // A packet with no decoder yet, or one added since this was
                    // written: framing already said where the next one starts.
                    Ok(Some(_)) => continue,
                    Ok(None) => return "the shard closed the connection".to_owned(),
                    Err(error) => return error.to_string(),
                };
                // "You may go." The shard answers the paperdoll's Log Out button
                // with this and then leaves the character standing until the
                // socket closes — closing it is the client's half, and both
                // references do it here. Nothing after this packet is worth
                // reading, so the loop ends and the window is told why.
                if matches!(packet, openshard_protocol::server_packet::ServerPacket::LogoutAck(_)) {
                    return "logged out".to_owned();
                }
                // The answer to "what moved since the world we kept". It arrives
                // once, before any chunk of this connection, and what it decides
                // is which of the two fetches happens — or neither.
                if let Some(Pending::Asking { .. }) = &pending {
                    if let openshard_protocol::server_packet::ServerPacket::ChangesReply(reply) = &packet {
                        let Some(notice) = world_notice else {
                            return "the shard answered about a world it never described".to_owned();
                        };
                        if reply.facet != notice.facet {
                            return format!(
                                "we asked what moved on facet {} and the shard answered about facet {}",
                                notice.facet.0, reply.facet.0
                            );
                        }
                        // A publish landed between the question and this answer.
                        // What the reply names is the difference to a revision
                        // the shard has already moved past, and every chunk of
                        // it would arrive at the new one — `WrongRevision`, one
                        // fetch later. There is no list here to add the publish
                        // to the way an abandoned fetch's is, so the honest
                        // answer is to ask the question again: one round trip,
                        // one request in flight at a time, and no state.
                        //
                        // `!=` rather than "older than", because the two numbers
                        // have to *agree* and not merely be ordered: a reply
                        // from ahead of every notice this connection has seen is
                        // as unusable as one from behind.
                        let now = latest.expect("the shard described the world this asked about");
                        if reply.revision != now {
                            let Some(Pending::Asking { held }) = &pending else {
                                unreachable!("the arm matched a line ago");
                            };
                            eprintln!(
                                "the shard answered about revision {} and it is at {} now: asking again",
                                reply.revision.0, now.0,
                            );
                            let asking = ChangesRequest {
                                facet: notice.facet,
                                revision: WorldRevision(held.revision().get()),
                            };
                            if let Err(error) = socket.send(&asking.encode()).await {
                                return error.to_string();
                            }
                            continue;
                        }
                        let Some(Pending::Asking { held }) = pending.take() else {
                            unreachable!("the arm matched a line ago");
                        };
                        match what_moved(notice, held, reply) {
                            Ok(WhatMoved::Nothing(held)) => {
                                report(Update::Ground {
                                    snapshot: Box::new(held),
                                    // Unchanged, so the kept file still is this
                                    // world — and a graph beside it still is a
                                    // graph of it.
                                    kept: match &ground {
                                        GroundSource::Fetched { cache } => {
                                            openshard_client_net::cache::path_for(cache, notice).ok()
                                        }
                                        GroundSource::OwnDisk => None,
                                    },
                                });
                            }
                            Ok(WhatMoved::These(mut fetch)) => {
                                if let Err(reason) = ask::<D>(&mut socket, &mut fetch).await {
                                    return reason;
                                }
                                pending = Some(Pending::Fetching(fetch));
                            }
                            Err(error) => {
                                return format!("the world the shard described cannot be fetched: {error}");
                            }
                        }
                        continue;
                    }
                }
                // The shard moved its own ground. It arrives unasked, at any
                // point after world entry, and what it costs to act on is a
                // handful of chunks — see `published`, which is the decision,
                // and `Update::GroundMoved`, which is what the window is given.
                if let openshard_protocol::server_packet::ServerPacket::PublishNotice(publish) = &packet {
                    let GroundSource::Fetched { .. } = &ground else {
                        // A client drawing a facet off its own disk. The shard's
                        // ground is not the ground on this screen, and chunks of
                        // it would not belong to the world in hand.
                        eprintln!(
                            "the shard's facet {} is at revision {} now; this client is drawing its \
                             own ground and does not follow it",
                            publish.facet.0, publish.revision.0
                        );
                        continue;
                    };
                    let Some(notice) = world_notice else {
                        return "the shard published a world it never described".to_owned();
                    };
                    if publish.facet != notice.facet {
                        // Nothing here holds another facet's ground: this client
                        // fetched the one it entered on, and a `0x76` is not a
                        // thing it has ever been sent.
                        eprintln!(
                            "facet {} moved to revision {}, and we are standing on facet {}",
                            publish.facet.0, publish.revision.0, notice.facet.0
                        );
                        continue;
                    }
                    // Whatever this connection is in the middle of, the shard is
                    // at this revision now.
                    latest = Some(publish.revision);
                    let moved_to = openshard_map::snapshot::MapRevision::decoded(publish.revision.0);
                    match pending.take() {
                        // Nothing is on the way, so the publish is answered as it
                        // stands: fetch what it names, for the world the window
                        // is holding.
                        None => match published(notice, publish) {
                            Ok(None) => {}
                            Ok(Some(mut fetch)) => {
                                if let Err(reason) = ask::<D>(&mut socket, &mut fetch).await {
                                    return reason;
                                }
                                pending = Some(Pending::Fetching(fetch));
                            }
                            Err(error) => {
                                return format!("the ground the shard published cannot be fetched: {error}");
                            }
                        },
                        // The question is out and its answer is not back yet, so
                        // there is no list here to add this one to. Nothing is
                        // done about it now: the reply names the revision it was
                        // written at, and a stale one asks again where it lands.
                        Some(Pending::Asking { held }) => {
                            eprintln!(
                                "the ground moved to revision {} while we were asking what moved",
                                publish.revision.0
                            );
                            pending = Some(Pending::Asking { held });
                        }
                        // Ground is on the wire at a revision the shard has just
                        // moved past, and the answers still coming cannot be told
                        // apart from the ones a second fetch would ask for. So
                        // the fetch stops, what it is owed is eaten rather than
                        // decoded, and what to ask for again is the union of what
                        // it was asking about and what this publish named — see
                        // `Fetch::abandon`, which is where all three are argued.
                        Some(Pending::Fetching(fetch)) => {
                            let (drain, restart) = fetch.abandon(&publish.changes, moved_to);
                            eprintln!(
                                "the ground moved to revision {} while it was still arriving: \
                                 {} chunk(s) of the fetch that was abandoned are still owed",
                                publish.revision.0,
                                drain.owed()
                            );
                            pending = Some(match resume::<D>(&mut socket, drain, restart, notice).await {
                                Ok(pending) => pending,
                                Err(reason) => return reason,
                            });
                        }
                        // A second edit while the first one's answers are still
                        // draining. The list grows and the revision moves; there
                        // is no second abandonment, because nothing has gone out
                        // on the wire since the first.
                        Some(Pending::Draining { drain, mut restart }) => {
                            restart.and(&publish.changes, moved_to);
                            eprintln!(
                                "the ground moved to revision {} while {} chunk(s) were still \
                                 draining",
                                publish.revision.0,
                                drain.owed()
                            );
                            pending = Some(Pending::Draining { drain, restart });
                        }
                    }
                    continue;
                }
                // The last answers to a fetch the ground moved out from under.
                // They are counted and thrown away — nothing here is decoded,
                // and nothing is reported — and when the wire is finally owed
                // nothing, the restart goes out. Until then this connection asks
                // for no ground at all, which is the whole reason a drain is a
                // state and not a filter.
                if let Some(Pending::Draining { drain, .. }) = pending.as_mut() {
                    if drain.on_packet(&packet) {
                        if drain.is_empty() {
                            let Some(Pending::Draining { restart, .. }) = pending.take() else {
                                unreachable!("the drain borrowed a line ago");
                            };
                            let notice = world_notice
                                .expect("a fetch is only abandoned for a world the shard described");
                            pending = Some(match begin::<D>(&mut socket, restart, notice).await {
                                Ok(pending) => pending,
                                Err(reason) => return reason,
                            });
                        }
                        continue;
                    }
                }
                // The ground, while it is still arriving. A chunk packet is
                // consumed here and never reported: what the window is given is
                // the facet, once, and not the seven thousand fragments it
                // came in. Every failure ends the connection, because a client
                // that was told to take the shard's ground and did not get it
                // has nothing to draw and no second source to fall back to.
                if let Some(Pending::Fetching(active)) = pending.as_mut() {
                    let before = active.held();
                    let mine = match active.on_packet(&packet) {
                        Ok(mine) => mine,
                        Err(error) => return format!("fetching the ground: {error}"),
                    };
                    if mine {
                        let held = active.held();
                        if held != before && held % PROGRESS_EVERY == 0 {
                            eprintln!("the ground: {held} of {} chunks", active.wanted());
                        }
                        if !active.is_complete() {
                            // A chunk out is room for a chunk in, and `ask` is
                            // what decides whether that is yet a request.
                            if let Err(reason) = ask::<D>(&mut socket, active).await {
                                return reason;
                            }
                            continue;
                        }
                        // Whole. The fetch is over and its one value is the
                        // facet, so it is taken rather than left behind: what
                        // follows on this connection is ordinary traffic, and a
                        // `Fetch` still sitting here would refuse the next
                        // `ChunkData` E4 sends as one nobody asked for.
                        let Some(Pending::Fetching(done)) = pending.take() else {
                            unreachable!("the fetch borrowed a line ago");
                        };
                        eprintln!(
                            "{}: {} chunks",
                            if done.is_over_a_world() {
                                "the ground moved"
                            } else {
                                "the ground arrived"
                            },
                            done.wanted()
                        );
                        match done.finish() {
                            Ok(Fetched::World(snapshot)) => {
                                // Kept before it is handed over, and on this
                                // thread: the window has no ground yet either
                                // way, and the write is half a second against a
                                // fetch that was seconds — 578 ms on Felucca,
                                // measured by `openshard-uofiles`'s
                                // `base_set_cost` example after version 2 of the
                                // file made the write deflate every chunk. That
                                // number is what chose the deflate level; see
                                // `openshard_protocol::chunks::DeflateLevel`,
                                // whose level six would have made this 4.2 s and
                                // this comment a lie. A cache that
                                // will not be written costs the next connection
                                // the same fetch and nothing else, so it is a
                                // line rather than a lost connection.
                                let mut kept = None;
                                if let (GroundSource::Fetched { cache }, Some(notice)) =
                                    (&ground, world_notice)
                                {
                                    match openshard_client_net::cache::write(cache, notice, &snapshot) {
                                        Ok(written) => {
                                            eprintln!(
                                                "the ground is kept at {}",
                                                written.path.display()
                                            );
                                            // A world of this facet that nobody
                                            // will ask for again — a shard that
                                            // re-imported, or a third shard on
                                            // one facet. Worth a line: it is a
                                            // hundred megabytes leaving the
                                            // working directory without anyone
                                            // asking for that either.
                                            for gone in &written.swept {
                                                eprintln!(
                                                    "a world this client had kept was let go of to \
                                                     make room: {}",
                                                    gone.display()
                                                );
                                            }
                                            kept = Some(written.path);
                                        }
                                        Err(error) => eprintln!("the ground was not kept: {error}"),
                                    }
                                }
                                report(Update::Ground {
                                    snapshot: Box::new(snapshot),
                                    // The write above, and not the path it would
                                    // have used: a bake belongs beside a world
                                    // that is really on the disk, and a cache
                                    // that failed to write left none there.
                                    kept,
                                });
                            }
                            // E4's arm: the world these belong to is the
                            // window's, so they cross the seam as chunks.
                            //
                            // **The kept file is left at the revision it was
                            // written at**, and that is a decision rather than an
                            // omission: rewriting it would mean the world coming
                            // back across this seam to be written from, and what
                            // it saves is one small fetch on the next connection
                            // — which is exactly the mechanism E3 built and this
                            // client will run anyway. The next start asks what
                            // moved, is told these same chunks, and writes the
                            // file then.
                            Ok(Fetched::Chunks(chunks)) => report(Update::GroundMoved { chunks }),
                            Err(error) => {
                                return format!("the ground the shard sent is not a facet: {error}");
                            }
                        }
                        continue;
                    }
                }
                if let openshard_protocol::server_packet::ServerPacket::Animation(animation) = packet {
                    report(Update::Animation(animation));
                }
                if let openshard_protocol::server_packet::ServerPacket::NewAnimation(animation) = packet {
                    report(Update::NewAnimation(animation));
                }
                if let openshard_protocol::server_packet::ServerPacket::Effect(effect) = packet {
                    report(Update::Effect(effect));
                }
                if let openshard_protocol::server_packet::ServerPacket::SwingTiming(timing) = packet {
                    report(Update::SwingTiming(timing));
                }
                if let openshard_protocol::server_packet::ServerPacket::HarvestToolVisual(visual) = packet {
                    report(Update::HarvestToolVisual(visual));
                }
                if let openshard_protocol::server_packet::ServerPacket::HarvestRefused(refusal) = packet {
                    report(Update::HarvestRefused(refusal));
                }
                if let openshard_protocol::server_packet::ServerPacket::HarvestCompleted(completion) = packet {
                    report(Update::HarvestCompleted(completion));
                }
                // Undivided: which packets move the player is [`Walk`]'s answer
                // and `Walk` belongs to the owner. The desync a fold can find,
                // and the resync it owes the shard, are the owner's too — see
                // [`Link::resync`].
                report(Update::Mutation {
                    packet,
                    received: Instant::now(),
                });
            }
            command = commands.recv() => {
                // `None` is the window closing: the `Link` was dropped.
                let Some(command) = command else {
                    return "the window closed".to_owned();
                };
                // An action becomes bytes here; a step is already a checked
                // packet and is only unwrapped back to bytes at this one
                // point, immediately before the socket write. What this
                // thread will not do is decide *which* tile a step asks for —
                // that needs the terrain, and the terrain is the owner's.
                let bytes = match command {
                    Command::Send(packet) => packet.into_bytes(),
                    Command::Outgoing(action) => action.encode(player_serial, version),
                };
                if let Err(error) = socket.send(&bytes).await {
                    return error.to_string();
                }
            }
        }
    }
}

/// What one packet did: whether the world changed, and whether the prediction
/// was thrown away.
///
/// Two answers rather than one because they are independent — a `0x21` that
/// rolls the body back to where the *view* already had it changes nothing in the
/// view and everything on screen.
pub(crate) struct Folded {
    /// The authoritative movement fact, if this packet contained one.
    pub(crate) movement: Option<Movement>,
}

/// Whether the walk handshake can answer this packet at all.
///
/// The four kinds [`Walk::on_packet`] has an arm for, and nothing else. It is a
/// question about the *kind* and not about the walk's state, so it says "could
/// move the player" rather than "did": an ack a rollback already voided is one
/// of these and moves nothing. That is the honest answer for a diagnostic
/// counting traffic before it is folded — see
/// [`App::observe_stationary_soak_update`](crate::App::observe_stationary_soak_update).
pub(crate) fn touches_the_walk(packet: &openshard_protocol::server_packet::ServerPacket) -> bool {
    use openshard_protocol::server_packet::ServerPacket;
    matches!(
        packet,
        ServerPacket::WalkAck(_)
            | ServerPacket::WalkReject(_)
            | ServerPacket::PlayerUpdate(_)
            | ServerPacket::PlayerStart(_)
    )
}

/// One packet into both records of where we are, answering whether anything
/// the window draws has changed.
///
/// The whole rule of this file, and the only part of it worth a test: a
/// [`WorldView`] does not learn its own body's position from a `0x22` or a
/// `0x21`, because neither packet carries one. A `0x22` names a sequence and
/// [`Walk`] is what knows which tile that step was asking for; a `0x21` is a
/// rollback to what the server says, and the view has no arm for either. Fold
/// only one of the two and the client's own body stands still while everyone
/// else moves around it.
pub(crate) fn fold(
    walk: &mut Walk,
    packet: &openshard_protocol::server_packet::ServerPacket,
) -> Result<Folded, openshard_client_net::walk::UnexpectedAck> {
    let movement = match walk.on_packet(packet)? {
        Moved::Stepped { position, facing, .. } => {
            let openshard_protocol::server_packet::ServerPacket::WalkAck(ack) = packet else {
                unreachable!("only a WalkAck can confirm a pending step");
            };
            Some(Movement::Ack {
                sequence: ack.sequence,
                confirmed: openshard_client_net::walk::Predicted { position, facing },
            })
        }
        Moved::Snapped { position, facing } => {
            let confirmed = openshard_client_net::walk::Predicted { position, facing };
            Some(match packet {
                openshard_protocol::server_packet::ServerPacket::WalkReject(reject) => Movement::Reject {
                    sequence: reject.sequence,
                    confirmed,
                },
                openshard_protocol::server_packet::ServerPacket::PlayerUpdate(_)
                | openshard_protocol::server_packet::ServerPacket::PlayerStart(_) => {
                    Movement::Relocation { confirmed }
                }
                _ => unreachable!("only a relocation packet can snap Walk"),
            })
        }
        Moved::Turned { confirmed } => Some(Movement::Turn { confirmed }),
        Moved::Idle => None,
    };
    Ok(Folded { movement })
}

#[cfg(test)]
mod tests {
    use openshard_protocol::containers::{
        AddToContainer, ContainedItem, ContainerContents, GridSlot, OpenContainer,
    };
    use openshard_protocol::direction::{Direction, Facing};
    use openshard_protocol::gump::GumpPoint;
    use openshard_protocol::mobile::Notoriety;
    use openshard_protocol::serial::Serial;
    use openshard_protocol::server_packet::ServerPacket;
    use openshard_protocol::vendor::{BuyList, SellList};
    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_protocol::world::{MapSize, PlayerStart, Point, StepSequence, WalkAck, WalkReject};

    use super::*;

    fn entered() -> (WorldView, Walk) {
        let start = PlayerStart {
            serial: Serial::new(0x0000_002A).unwrap(),
            body: Graphic(0x0190),
            position: Point::new(100, 100, 0),
            facing: Facing::walking(Direction::North),
            map: MapSize::BRITANNIA,
        };
        let view = WorldView::entered(start);
        let walk = Walk::new(view.player.position, view.player.facing);
        (view, walk)
    }

    /// One acknowledged step, as the mailbox sees it: an ordered fact whose
    /// sequence is its identity. That identity is the whole point of the two
    /// tests below — an ack that was merged with another, or delivered out of
    /// order, retires the wrong step.
    fn acked(sequence: u8) -> Update {
        Update::Mutation {
            packet: ServerPacket::WalkAck(WalkAck {
                sequence: StepSequence(sequence),
                notoriety: Notoriety::Innocent,
            }),
            received: Instant::now(),
        }
    }

    #[test]
    fn a_busy_frame_keeps_each_numbered_step() {
        let updates = Updates::new();
        assert!(updates.publish(acked(101)), "the idle mailbox needs one wake-up");
        assert!(
            !updates.publish(acked(102)),
            "the wake-up already covers this frame"
        );

        let staged = updates.take();
        assert!(matches!(
            staged.as_slice(),
            [
                Update::Mutation {
                    packet: ServerPacket::WalkAck(WalkAck {
                        sequence: StepSequence(101),
                        ..
                    }),
                    ..
                },
                Update::Mutation {
                    packet: ServerPacket::WalkAck(WalkAck {
                        sequence: StepSequence(102),
                        ..
                    }),
                    ..
                },
            ]
        ));
        assert!(
            updates.publish(acked(103)),
            "a drained mailbox needs a new wake-up"
        );
    }

    #[test]
    fn mutations_stay_ordered_on_both_sides_of_a_step() {
        let updates = Updates::new();
        updates.publish(Update::Lost("before".to_owned()));
        updates.publish(acked(101));
        updates.publish(Update::Lost("after".to_owned()));

        let staged = updates.take();
        assert!(matches!(&staged[0], Update::Lost(reason) if reason == "before"));
        assert!(matches!(
            &staged[1],
            Update::Mutation {
                packet: ServerPacket::WalkAck(WalkAck {
                    sequence: StepSequence(101),
                    ..
                }),
                ..
            }
        ));
        assert!(matches!(&staged[2], Update::Lost(reason) if reason == "after"));
    }

    #[test]
    fn ordered_delivery_waits_for_the_application_instead_of_growing() {
        let updates = Updates::with_capacity(1);
        updates.publish(Update::Lost("first".to_owned()));
        let producer = updates.clone();
        let (sent, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            sent.send(producer.publish(Update::Lost("second".to_owned())))
                .expect("the test is listening");
        });

        assert!(
            received
                .recv_timeout(std::time::Duration::from_millis(20))
                .is_err(),
            "the second ordered update must wait for capacity"
        );
        assert!(matches!(&updates.take()[0], Update::Lost(reason) if reason == "first"));
        assert!(
            received.recv_timeout(std::time::Duration::from_secs(1)).is_ok(),
            "draining must release the shard thread"
        );
        worker.join().expect("the shard-side publisher exits");
    }

    #[test]
    fn draining_rearms_backpressure_reporting_for_a_later_stall() {
        let updates = Updates::with_capacity(1);
        updates.publish(Update::Lost("first".to_owned()));
        let producer = updates.clone();
        let (started_by_producer, started) = std::sync::mpsc::channel();
        let (finished_by_producer, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_by_producer
                .send(())
                .expect("the test waits for the network publisher");
            finished_by_producer
                .send(producer.publish(Update::Lost("second".to_owned())))
                .expect("the test waits for the network publisher");
        });

        started
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("the network publisher starts");
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_millis(20))
                .is_err(),
            "the second ordered update is blocked at the mailbox limit"
        );
        assert!(
            updates
                .mailbox
                .pending
                .lock()
                .expect("the update mailbox is not poisoned")
                .backpressure_reported,
            "a full mailbox records that this stall has been reported"
        );

        updates.take();
        assert!(
            !updates
                .mailbox
                .pending
                .lock()
                .expect("the update mailbox is not poisoned")
                .backpressure_reported,
            "draining ends the reporting episode"
        );
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("draining releases the network publisher"),
            "the newly non-idle mailbox asks for a platform wake-up"
        );
        worker.join().expect("the shard-side publisher exits");
    }

    /// A slow or occluded window must not turn an active socket into an
    /// unbounded queue.  Numbered movement events retain their order and are
    /// consequently covered by the same backpressure as packets.
    #[test]
    fn a_stalled_window_bounds_numbered_walk_acknowledgements() {
        let updates = Updates::new();
        for packet in 0..MAX_ORDERED_UPDATES - 1 {
            updates.publish(Update::Lost(format!("packet {packet}")));
        }
        updates.publish(acked(101));

        let producer = updates.clone();
        let (started_by_producer, started) = std::sync::mpsc::channel();
        let (finished_by_producer, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_by_producer
                .send(())
                .expect("the test waits for the network publisher");
            finished_by_producer
                .send(producer.publish(Update::Lost("after stall".to_owned())))
                .expect("the test waits for the network publisher");
        });

        started
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("the network publisher starts");
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_millis(20))
                .is_err(),
            "a full ordered mailbox applies backpressure while the window is stalled"
        );

        let staged = updates.take();
        assert_eq!(staged.len(), MAX_ORDERED_UPDATES);
        for (packet, update) in staged.iter().take(MAX_ORDERED_UPDATES - 1).enumerate() {
            assert!(matches!(update, Update::Lost(reason) if reason == &format!("packet {packet}")));
        }
        let Some(Update::Mutation {
            packet: ServerPacket::WalkAck(ack),
            ..
        }) = staged.last()
        else {
            panic!("the numbered walk remains ordered with packets");
        };
        assert_eq!(ack.sequence, StepSequence(101));

        assert!(
            finished
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("draining releases the socket reader"),
            "the newly non-idle mailbox asks the event loop for one wake-up"
        );
        assert!(matches!(
            updates.take().as_slice(),
            [Update::Lost(reason)] if reason == "after stall"
        ));
        worker.join().expect("the shard-side publisher exits");
    }

    #[test]
    fn command_delivery_has_a_fixed_bound() {
        let (commands, mut received) = tokio::sync::mpsc::channel(1);
        let link = Link { commands };
        link.stop_attacking();
        link.stop_attacking();

        assert!(matches!(
            received.try_recv(),
            Ok(Command::Outgoing(Outgoing::StopAttacking))
        ));
        assert!(
            received.try_recv().is_err(),
            "the second command was not queued without limit"
        );
    }

    #[test]
    fn an_ack_moves_the_body_the_window_draws() {
        // The 0x22 carries a sequence and a health-bar colour and no position
        // at all, so the tile is the one `Walk` asked for. A client that only
        // fed packets to the view would walk on the server and stand still on
        // the screen.
        let (mut view, mut walk) = entered();
        walk.step(Facing::walking(Direction::North), |_, _| None).unwrap();

        let ack = ServerPacket::WalkAck(WalkAck {
            sequence: StepSequence(0),
            notoriety: Notoriety::Innocent,
        });
        let folded = fold(&mut walk, &ack).unwrap();
        view.apply(&ack);
        let confirmed = folded
            .movement
            .expect("an acknowledgement is movement")
            .confirmed();
        view.player_stepped(confirmed.position, confirmed.facing);
        assert!(
            matches!(folded.movement, Some(Movement::Ack { .. })),
            "an allowed step is not a rollback"
        );
        assert_eq!(view.player.position, Point::new(100, 99, 0));
    }

    #[test]
    fn a_rejection_puts_the_body_back_where_the_server_says() {
        // And the other direction: a 0x21 is the server disagreeing, and the
        // view has no arm for it — only `Walk` knows the step it undoes.
        let (mut view, mut walk) = entered();
        walk.step(Facing::walking(Direction::North), |_, _| None).unwrap();

        let reject = ServerPacket::WalkReject(WalkReject {
            sequence: StepSequence(0),
            position: Point::new(100, 100, 0),
            facing: Facing::walking(Direction::North),
        });
        let folded = fold(&mut walk, &reject).unwrap();
        view.apply(&reject);
        assert!(matches!(
            folded.movement,
            Some(Movement::Reject {
                sequence: StepSequence(0),
                ..
            })
        ));
        assert_eq!(view.player.position, Point::new(100, 100, 0));
        assert_eq!(
            walk.predicted().position,
            Point::new(100, 100, 0),
            "and the prediction is thrown away with it"
        );
    }

    #[test]
    fn vendor_and_container_packets_are_not_movement_events() {
        let (_, mut walk) = entered();
        walk.step(Facing::walking(Direction::East), |_, _| None).unwrap();
        let predicted_before = walk.predicted();
        let container = Serial::new(0x4000_0001).unwrap();
        let vendor = Serial::new(0x0000_002A).unwrap();
        let item = ContainedItem {
            serial: Serial::new(0x4000_0002).unwrap(),
            graphic: Graphic(0x0EED),
            amount: openshard_protocol::items::ItemAmount(1),
            at: GumpPoint { x: 20, y: 30 },
            grid: GridSlot(0),
            hue: Hue::NONE,
        };
        let packets = [
            ServerPacket::OpenContainer(OpenContainer {
                container,
                gump: Graphic(0x003C),
            }),
            ServerPacket::AddToContainer(AddToContainer { item, container }),
            ServerPacket::ContainerContents(ContainerContents {
                container: Some(container),
                items: vec![item],
            }),
            ServerPacket::BuyList(BuyList {
                container,
                lines: Vec::new(),
            }),
            ServerPacket::SellList(SellList {
                vendor,
                lines: Vec::new(),
            }),
        ];

        for packet in packets {
            let folded = fold(&mut walk, &packet).expect("vendor traffic cannot desync walking");
            assert!(folded.movement.is_none(), "{packet:?} is not movement");
            assert_eq!(walk.predicted(), predicted_before);
        }
    }

    /// The whole of the lag compensation, stated once: what is drawn is the
    /// prediction, and it is a tile ahead of the view for as long as the ack
    /// takes to arrive.
    #[test]
    fn a_step_is_predicted_before_the_server_has_answered() {
        let (view, mut walk) = entered();
        walk.step(Facing::walking(Direction::North), |_, _| None).unwrap();

        assert_eq!(
            view.player.position,
            Point::new(100, 100, 0),
            "the view is still what the server said"
        );
        let body = Body {
            predicted: walk.predicted(),
            corrected: false,
        };
        assert_eq!(
            body.predicted.position,
            Point::new(100, 99, 0),
            "and the body is drawn where the step asked to be"
        );
        assert!(!body.corrected);
    }

    #[test]
    fn an_ack_for_a_step_nobody_took_is_an_error() {
        // The two ends have lost track of each other. Nothing local repairs
        // that, so the thread reports it rather than guessing.
        let (_, mut walk) = entered();
        let ack = ServerPacket::WalkAck(WalkAck {
            sequence: StepSequence(3),
            notoriety: Notoriety::Innocent,
        });
        assert!(fold(&mut walk, &ack).is_err());
    }
}
