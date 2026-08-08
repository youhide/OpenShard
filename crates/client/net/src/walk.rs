//! Walking, from the client's chair: `0x02` out, `0x22` or `0x21` back.
//!
//! # Why this has to predict at all
//!
//! A `0x22` ack carries two bytes — the sequence being answered and a health-bar
//! colour — and *not* a position. The server has one and does not send it. So
//! "where am I now?" is a question only this end can answer: it is the tile the
//! step the ack names was asking for. That is what [`Walk`] keeps, and why it
//! has to work out turn-versus-step by exactly the rule the server uses. It does
//! not reimplement that rule — [`openshard_movement::intend`] is the one copy,
//! and the server's `Walker::request` calls the same function.
//!
//! # What it deliberately does not do
//!
//! Move the [`WorldView`](crate::view::WorldView) before the ack arrives. A real
//! 2D client steps immediately and rubber-bands on a `0x21`, which is the right
//! trade for something drawing at 60 frames a second and the wrong one for a
//! record of what the server said. The prediction lives here, in
//! [`Walk::predicted`], and the view learns a position only once a `0x22` has
//! confirmed the step that produced it. Anything on top that wants to draw
//! ahead of the server can read the prediction and do so knowingly — and our own
//! window does: `client/app`'s `link::Body` publishes [`Walk::predicted`]
//! alongside the view every time a step is sent, so the body moves on the
//! player's input rather than a round trip later, and a [`Moved::Snapped`] is
//! the rollback. That is the lag compensation, and it is *outside* this type on
//! purpose: the prediction and the record stay two values, so nothing downstream
//! has to guess which one it is holding.
//!
//! It also holds no map. [`openshard_movement::intend`] carries `z` over
//! unchanged, because neither end of it has terrain — so the height comes in as
//! an argument to [`Walk::step`], from whoever loaded a facet. A caller with no
//! map passes `|_, _, _| None` and gets the old flat prediction, which drifts up a
//! hill until a `0x20` or a `0x21` corrects it. What is deliberately *not*
//! predicted here is whether a step is allowed at all: that is the server's
//! answer and it arrives as a `0x21`.

use std::collections::VecDeque;

use openshard_movement::{Intent, StepCounter, intend};
use openshard_protocol::direction::Facing;
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::world::{Point, RawFastwalkKey, RawStepSequence, StepSequence, WalkRequest};

/// Where this client believes it will be once every step in flight is answered.
///
/// Not where it is: nothing here has been confirmed by the server. It is the
/// state the *next* `0x02` is computed from, which is why a caller sending
/// several steps without waiting still produces a walkable line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Predicted {
    /// The tile the last step asked for.
    pub position: Point,
    /// The facing the last step asked for.
    pub facing: Facing,
}

/// One step asked for and not yet answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pending {
    /// The sequence byte its `0x02` carried, and the one its `0x22` will echo.
    sequence: StepSequence,
    /// Where this client will be if the step is allowed. For a turn, where it
    /// already was — a turn moves nobody.
    position: Point,
    /// Which way it will face.
    facing: Facing,
}

/// What a server packet meant for the walk.
///
/// Closed on purpose, unlike [`Step`](crate::session::Step): a packet either
/// says nothing about the walk, confirms a step, or says where the body really
/// is, and there is no fourth thing the handshake can do. A caller matching all
/// three should be told by the compiler if that ever stops being true.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Moved {
    /// Nothing to do — a packet the walk has no interest in.
    Idle,
    /// A step this client asked for was allowed. It is standing here now, and
    /// this is the first moment that is true.
    Stepped {
        /// Where it is.
        position: Point,
        /// Which way it faces.
        facing: Facing,
        /// The colour of its own health bar, which is the only other thing a
        /// `0x22` carries.
        ///
        /// Not folded into the [`WorldView`](crate::view::WorldView) for the
        /// same reason `0x11` is not: it is status-bar data, not a position.
        /// It is handed over rather than dropped so that whatever eventually
        /// models the status bar has it.
        notoriety: Notoriety,
    },
    /// Forget the prediction: this is where the server says it really is.
    ///
    /// A `0x21` refusing a step and a `0x20` moving the body somewhere it did
    /// not walk to are the same event from here — everything in flight is void,
    /// and both ends go back to a sequence of zero.
    Snapped {
        /// Where it really is.
        position: Point,
        /// Which way it really faces.
        facing: Facing,
    },
}

/// A `0x22` answered a step this client is not waiting for.
///
/// Means the two ends have lost track of each other: acks arrive in the order
/// the steps were sent, one apiece, and a refused step gets a `0x21` instead. A
/// caller has nothing local to repair — the server's idea of where the body is
/// no longer matches this one's, and only the server can say so.
///
/// Deliberately *not* what a step voided by a rollback produces. Those are
/// answered too — see [`Walk::draining`] — and reporting them here made a wall
/// plus a slow link look exactly like a desync.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UnexpectedAck {
    /// The sequence the server acked.
    pub got: StepSequence,
    /// The step this client was waiting on, or `None` when it was waiting on
    /// nothing at all.
    pub expected: Option<StepSequence>,
}

impl std::fmt::Display for UnexpectedAck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.expected {
            Some(expected) => write!(
                f,
                "0x22 acked step {} while waiting on step {}",
                self.got.0, expected.0
            ),
            None => write!(f, "0x22 acked step {} with nothing in flight", self.got.0),
        }
    }
}

impl std::error::Error for UnexpectedAck {}

/// A step was asked for that has no tile to land on.
///
/// The map is addressed with `u16`s, so there is no way to express a step west
/// from `x = 0`. Refused here rather than sent, because the server refuses it
/// too (`Walker::request`'s world-edge branch) and a `0x02` that can only come
/// back as a `0x21` is a round trip spent to learn something this end already
/// knew.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AtTheWorldEdge {
    /// Where the step was from.
    pub from: Point,
    /// Which way it was going.
    pub facing: Facing,
}

impl std::fmt::Display for AtTheWorldEdge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} from {} leaves the map", self.facing.direction, self.from)
    }
}

impl std::error::Error for AtTheWorldEdge {}

/// How many steps may be in flight before this client stops asking for more.
///
/// The reference's number: `ClassicUO`'s `Constants.MAX_STEP_COUNT`, checked
/// first thing in `PlayerMobile.Walk`, which refuses to queue a sixth.
///
/// It is not a second pace limit — the shard's `WalkPace` is the only judge of
/// how fast a body may walk, and `steer.rs` is what keeps this end inside it.
/// This is the answer to a *silent* shard: without it, a client whose server
/// stopped answering keeps predicting itself further and further from where it
/// really is, and the correction when the link comes back is the length of the
/// outage. Five steps is two seconds of walking, which is longer than any
/// hiccup worth predicting through.
pub const MAX_IN_FLIGHT: usize = 5;

/// Why a step was not sent.
///
/// Every variant is a refusal this end made on its own, before any bytes were
/// written — the server refuses steps too, and that arrives as a `0x21`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotSent {
    /// The step would leave the coordinate space. See [`AtTheWorldEdge`].
    AtTheWorldEdge(AtTheWorldEdge),
    /// [`MAX_IN_FLIGHT`] steps are already unanswered.
    ///
    /// The shard has gone quiet — nothing else can produce this, because
    /// `steer.rs` asks for one step per step's length and a live shard answers
    /// each of them within a round trip. A caller should draw the body where it
    /// is and try again on the next ask, which is what walking into it looks
    /// like: the character stops until the server speaks.
    Backlogged {
        /// How many are unanswered, which is [`MAX_IN_FLIGHT`].
        in_flight: usize,
    },
    /// The walk is out of step with the server and has asked to be told where it
    /// is. See [`Walk::out_of_step`].
    OutOfStep,
}

impl std::fmt::Display for NotSent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AtTheWorldEdge(edge) => edge.fmt(f),
            Self::Backlogged { in_flight } => {
                write!(f, "{in_flight} steps are unanswered: the shard has gone quiet")
            }
            Self::OutOfStep => f.write_str("the walk is out of step and waiting on a resync"),
        }
    }
}

impl std::error::Error for NotSent {}

impl From<AtTheWorldEdge> for NotSent {
    fn from(edge: AtTheWorldEdge) -> Self {
        Self::AtTheWorldEdge(edge)
    }
}

/// Drives one character's walking.
///
/// Built from wherever the character is standing — `Step::Entered`'s
/// [`PlayerStart`](openshard_protocol::world::PlayerStart) is where a login
/// hands that over — and then fed every packet the connection decodes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Walk {
    /// The sequence byte the next `0x02` will carry.
    counter: StepCounter,
    /// Where the last step asked to be.
    predicted: Predicted,
    /// Steps sent and not yet answered, oldest first.
    pending: VecDeque<Pending>,
    /// How many answers are still owed for steps a correction has already
    /// voided.
    ///
    /// The shard answers every `0x02` it receives, one apiece — an ack or a
    /// refusal — and a `0x21` voids the steps *this end* had in flight without
    /// unsending them. Those bytes are already on the wire and their answers
    /// come back after the rollback, naming a sequence this end has forgotten.
    ///
    /// Reported as an [`UnexpectedAck`] once, which the window turned into a
    /// dropped connection: a wall and 150ms of latency was enough to close a
    /// client's own session. They are not a desync at all — the wire delivers in
    /// order, so while this is non-zero the next answer is one of the voided
    /// ones and belongs to nobody. Counted rather than guessed at, so a *real*
    /// desync is still reported.
    draining: usize,
    /// Whether the two ends have lost track of each other and nothing may be
    /// sent until the server says where this body is.
    ///
    /// See [`Walk::out_of_step`] for the whole of the rule.
    out_of_step: bool,
}

impl Walk {
    /// A character standing at `position`, facing `facing`, with nothing in
    /// flight and a fresh sequence.
    #[must_use]
    pub fn new(position: Point, facing: Facing) -> Self {
        Self {
            counter: StepCounter::new(),
            predicted: Predicted { position, facing },
            pending: VecDeque::new(),
            draining: 0,
            out_of_step: false,
        }
    }

    /// Whether the walk has lost track of the server and is waiting to be told
    /// where it is.
    ///
    /// Set by an answer this end cannot place — see [`UnexpectedAck`], and note
    /// that the ordinary answers to steps a rollback voided are *not* that. While
    /// it holds, [`Walk::step`] sends nothing: a `0x22` carries no position, so
    /// there is no local repair, and every step sent on top of the disagreement
    /// widens it.
    ///
    /// The caller is what asks — a [`ResyncRequest`](openshard_protocol::world::ResyncRequest)
    /// — and it must, because this is only safe if something is guaranteed to
    /// clear it. What clears it is the server saying where the body is: a `0x20`,
    /// a `0x21` or a second `0x1B`, all of which reset both sequences. The
    /// reference has the same pair and the same rule
    /// (`WalkerManager.WalkingFailed`, set beside `Send_Resync`, cleared by the
    /// `0x20` handler); a client that froze without asking would freeze for good.
    #[must_use]
    pub const fn out_of_step(&self) -> bool {
        self.out_of_step
    }

    /// Where this client believes it will end up. See [`Predicted`].
    #[must_use]
    pub const fn predicted(&self) -> Predicted {
        self.predicted
    }

    /// How many steps have been sent and not yet answered.
    ///
    /// Capped at [`MAX_IN_FLIGHT`], and *not* as a pace limit: the server's own
    /// budget (`openshard_movement::WalkPace`) is what stops a client walking
    /// faster than a body can, and a second rate invented on this side would only
    /// differ from it. The cap is what a client does when the shard stops
    /// answering — see [`NotSent::Backlogged`]. A caller that wants to walk in
    /// step waits for this to reach zero.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// Ask to take one step, and get the `0x02` to write to the socket.
    ///
    /// `facing` is a direction and whether to run, exactly as the packet carries
    /// it. Asking for a direction the character is not already facing turns it
    /// and moves it nowhere — that is a whole step in UO, it gets its own
    /// sequence number and its own ack, and this predicts it as one.
    ///
    /// `ground` answers the height to stand at on the target tile, given the
    /// whole point this end believes it is standing at — tile *and* height, not
    /// just the height, because the surface underfoot is what a step reaches
    /// from and a staircase cannot be climbed without it. It is what stops the
    /// prediction drifting up a hill — or into the water under a pier: [`intend`]
    /// carries `z` over unchanged because neither end of it has terrain, while
    /// the server *does* read the map and lands the step wherever the ground (or
    /// a static a body can stand on) is — and says nothing, since a `0x22`
    /// carries no position. A caller with no map passes `|_, _, _| None` and gets
    /// the old flat prediction, which is honest; a guessed height would be
    /// indistinguishable from a real one. `openshard_movement::MapTerrain`'s
    /// `predict_step` is the answer a caller with a map wants: it is the server's
    /// own step rule, so a stair climbs and a pier's deck is stood on rather than
    /// the water under it, and it never refuses (see below).
    ///
    /// Deliberately not `openshard_movement::Terrain::can_step`: this predicts a
    /// *height*, and it must not predict a *refusal*. Whether a step is allowed
    /// is the server's answer, arriving as a `0x21`, and a client that decided
    /// it here would need every rule about statics, doors and mounts to agree
    /// exactly — where a wrong height costs a body drawn a few pixels off until
    /// the next `0x20`.
    pub fn step(
        &mut self,
        facing: Facing,
        ground: impl Fn(Point, u16, u16) -> Option<i8>,
    ) -> Result<Vec<u8>, NotSent> {
        // Two refusals before anything is decided about the geometry, and the
        // reference checks the same two first thing in `PlayerMobile.Walk`.
        //
        // A walk that has lost track of the server has nothing to compute a step
        // *from*: `predicted` is derived from a chain of asks the server has
        // stopped agreeing with. Sending more only widens the disagreement.
        if self.out_of_step {
            return Err(NotSent::OutOfStep);
        }
        // And a shard that has stopped answering is not somewhere to keep
        // predicting into: every step past the cap is another tile of correction
        // when the link comes back.
        if self.pending.len() >= MAX_IN_FLIGHT {
            return Err(NotSent::Backlogged {
                in_flight: self.pending.len(),
            });
        }
        let (position, facing) = match intend(self.predicted.position, self.predicted.facing, facing) {
            Intent::Turned { facing } => (self.predicted.position, facing),
            Intent::Stepped { target, facing } => {
                // The surface under the target, if the caller knows one, reached
                // from where this end currently believes it is standing. A tile
                // the map has no cell for is left at the height it came with
                // rather than dropped to zero, which would put the body
                // underground.
                let landed = match ground(self.predicted.position, target.x, target.y) {
                    Some(z) => Point::new(target.x, target.y, z),
                    None => target,
                };
                (landed, facing)
            }
            Intent::OffTheMap => {
                return Err(NotSent::AtTheWorldEdge(AtTheWorldEdge {
                    from: self.predicted.position,
                    facing,
                }));
            }
        };

        let sequence = RawStepSequence(self.counter.take());
        self.predicted = Predicted { position, facing };
        self.pending.push_back(Pending {
            sequence: sequence.interpret(),
            position,
            facing,
        });
        Ok(WalkRequest {
            facing,
            sequence,
            // Never read by anything, on either side of the wire — see
            // `RawFastwalkKey`. Sending zero is what Sphere's own clients end up
            // doing once the key is ignored.
            fastwalk_key: RawFastwalkKey(0),
        }
        .encode())
    }

    /// Advance the walk with one packet from the server.
    ///
    /// Every decoded packet should come through here, not only the two walk
    /// answers: a `0x20` or a second `0x1B` moves the body without a step, and
    /// both ends put their sequence back to zero when that happens (the server
    /// does it in `WorldState::move_to`). A client that missed one would be
    /// refused on its next step and never find out why.
    pub fn on_packet(&mut self, packet: &ServerPacket) -> Result<Moved, UnexpectedAck> {
        match packet {
            ServerPacket::WalkAck(ack) => self.acked(ack.sequence, ack.notoriety),
            // A refusal for a step a previous refusal already voided. The wire
            // delivers in order, so it is one of the answers still owed and the
            // position it carries is older than the one already believed — the
            // shard did not move on a refusal. Swallowed rather than applied:
            // applying it would clear the steps sent *since* the rollback, which
            // is a second rollback nobody asked for.
            ServerPacket::WalkReject(_) if self.answered_from_before() => Ok(Moved::Idle),
            // A `0x21` is the answer to one of the steps in flight; a `0x20` or a
            // second `0x1B` is the answer to none of them.
            ServerPacket::WalkReject(reject) => Ok(self.snap(reject.position, reject.facing, 1)),
            ServerPacket::PlayerUpdate(update) => Ok(self.snap(update.position, update.facing, 0)),
            // A second `0x1B` restarts the session; the body it describes is
            // where this character now is, whatever was in flight.
            ServerPacket::PlayerStart(start) => Ok(self.snap(start.position, start.facing, 0)),
            _ => Ok(Moved::Idle),
        }
    }

    /// A `0x22`: the oldest step in flight is confirmed, and only that one.
    fn acked(&mut self, sequence: StepSequence, notoriety: Notoriety) -> Result<Moved, UnexpectedAck> {
        // A step the shard allowed and a rollback has since voided. It changes
        // nothing — the position it confirms was thrown away — and it is not a
        // desync, so it is counted off rather than reported.
        if self.answered_from_before() {
            return Ok(Moved::Idle);
        }
        let Some(pending) = self.pending.front().copied() else {
            return Err(self.lost_track(UnexpectedAck {
                got: sequence,
                expected: None,
            }));
        };
        if pending.sequence != sequence {
            // Left in flight on purpose. The mismatch is not something this end
            // can repair by guessing which step was meant, and dropping the
            // queue would turn a diagnosable desync into a silent one.
            return Err(self.lost_track(UnexpectedAck {
                got: sequence,
                expected: Some(pending.sequence),
            }));
        }
        self.pending.pop_front();
        Ok(Moved::Stepped {
            position: pending.position,
            facing: pending.facing,
            notoriety,
        })
    }

    /// The server put the body somewhere: believe it, and start over.
    ///
    /// `answered` is how many of the steps in flight this packet is itself the
    /// answer to — one for a `0x21`, which refuses a step that was asked for, and
    /// none for a `0x20` or a second `0x1B`, which nobody asked for. The rest are
    /// still coming back and are counted into [`Walk::draining`].
    fn snap(&mut self, position: Point, facing: Facing, answered: usize) -> Moved {
        self.predicted = Predicted { position, facing };
        self.draining += self.pending.len().saturating_sub(answered);
        self.pending.clear();
        self.counter.reset();
        // Whatever the two ends had lost track of, this is the server stating the
        // answer — and it is the only thing that ever does. See
        // [`Walk::out_of_step`].
        self.out_of_step = false;
        Moved::Snapped { position, facing }
    }

    /// Stop walking: this end can no longer say where the body is.
    ///
    /// The error is handed back through here rather than returned directly so
    /// that no arm can report a desync without also refusing to walk on top of
    /// it — the two are one decision.
    fn lost_track(&mut self, desync: UnexpectedAck) -> UnexpectedAck {
        self.out_of_step = true;
        desync
    }

    /// Whether the answer being handled belongs to a step a rollback has already
    /// voided, counting it off if so.
    ///
    /// The wire delivers in order and every `0x02` is answered exactly once, so
    /// while anything is owed from before the last correction, the next answer is
    /// one of those and nothing newer can have overtaken it.
    fn answered_from_before(&mut self) -> bool {
        if self.draining == 0 {
            return false;
        }
        self.draining -= 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use openshard_protocol::direction::Direction;
    use openshard_protocol::serial::Serial;
    use openshard_protocol::wire::{Graphic, Hue};
    use openshard_protocol::world::{MapSize, PlayerStart, PlayerUpdate, WalkAck, WalkReject};

    use super::*;

    fn walk() -> Walk {
        Walk::new(Point::new(100, 100, 0), Facing::walking(Direction::North))
    }

    /// A step onto ground lands at the ground's height, not at the height it
    /// started from.
    ///
    /// Which is the whole of the drift: `intend` has no terrain, so without this
    /// a character walking up Britain's hill stays at the height of the tile it
    /// left until a `0x20` corrects it — and a body standing below the terrain
    /// is drawn *behind* it, which looks exactly like a mobile that failed to
    /// draw. A turn is asserted alongside, because a turn moves nobody and must
    /// not be re-grounded: the ground under a body that did not move is the
    /// ground it is already on, and asking for it again would make a mounted or
    /// levitating body fall to the floor on every turn.
    #[test]
    fn a_step_lands_on_the_ground_and_a_turn_does_not() {
        let mut walk = Walk::new(Point::new(100, 100, 20), Facing::walking(Direction::North));
        let hill = |_from: Point, _x: u16, _y: u16| Some(25);

        walk.step(Facing::walking(Direction::East), hill).unwrap();
        assert_eq!(
            walk.predicted().position,
            Point::new(100, 100, 20),
            "a turn is a step that moves nobody, so nothing is re-grounded",
        );
        walk.step(Facing::walking(Direction::East), hill).unwrap();
        assert_eq!(walk.predicted().position, Point::new(101, 100, 25), "up the hill");

        // And a tile the caller knows nothing about keeps the height it had.
        // Dropping it to zero would put the body underground, where the terrain
        // in front of it hides it.
        walk.step(Facing::walking(Direction::East), |_, _, _| None)
            .unwrap();
        assert_eq!(walk.predicted().position, Point::new(102, 100, 25));
    }

    /// The `0x02` a `step` produced, taken apart: direction bits, sequence.
    fn sent(bytes: &[u8]) -> (u8, u8) {
        assert_eq!(bytes.len(), 7, "0x02 is seven bytes");
        assert_eq!(bytes[0], 0x02);
        (bytes[1], bytes[2])
    }

    #[test]
    fn the_first_step_of_a_session_carries_a_zero() {
        // The one sequence rule the server actually enforces: a fresh
        // connection opens at zero or the step is refused.
        let mut walk = walk();
        let bytes = walk
            .step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();
        assert_eq!(sent(&bytes).1, 0);
        assert_eq!(
            sent(
                &walk
                    .step(Facing::walking(Direction::North), |_, _, _| None)
                    .unwrap()
            )
            .1,
            1
        );
    }

    #[test]
    fn turning_costs_a_step_and_moves_nothing() {
        // Facing north and asked to go east: the first request only turns, and
        // the client must predict that or it puts itself a tile ahead of the
        // server for the rest of the session.
        let mut walk = walk();
        walk.step(Facing::walking(Direction::East), |_, _, _| None)
            .unwrap();
        assert_eq!(
            walk.predicted(),
            Predicted {
                position: Point::new(100, 100, 0),
                facing: Facing::walking(Direction::East),
            },
            "a turn moves nobody"
        );

        walk.step(Facing::walking(Direction::East), |_, _, _| None)
            .unwrap();
        assert_eq!(walk.predicted().position, Point::new(101, 100, 0));
        assert_eq!(walk.in_flight(), 2, "a turn is a step and gets its own ack");
    }

    #[test]
    fn a_position_is_only_believed_once_the_ack_arrives() {
        // The whole point of the type. Two steps go out; the view learns about
        // the first only when the first ack comes back, and about the second
        // only when the second does.
        let mut walk = walk();
        walk.step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();
        walk.step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();
        assert_eq!(walk.in_flight(), 2);

        assert_eq!(
            walk.on_packet(&ServerPacket::WalkAck(WalkAck {
                sequence: StepSequence(0),
                notoriety: Notoriety::Innocent,
            })),
            Ok(Moved::Stepped {
                position: Point::new(100, 99, 0),
                facing: Facing::walking(Direction::North),
                notoriety: Notoriety::Innocent,
            })
        );
        assert_eq!(walk.in_flight(), 1);

        assert_eq!(
            walk.on_packet(&ServerPacket::WalkAck(WalkAck {
                sequence: StepSequence(1),
                notoriety: Notoriety::Murderer,
            })),
            Ok(Moved::Stepped {
                position: Point::new(100, 98, 0),
                facing: Facing::walking(Direction::North),
                notoriety: Notoriety::Murderer,
            })
        );
        assert_eq!(walk.in_flight(), 0);
    }

    #[test]
    fn a_reject_snaps_back_and_restarts_the_sequence() {
        // Three steps in flight, all void. The next `0x02` has to carry a zero
        // or the server — which reset its own counter when it sent the 0x21 —
        // refuses that one too, and the client never walks again.
        let mut walk = walk();
        for _ in 0..3 {
            walk.step(Facing::walking(Direction::North), |_, _, _| None)
                .unwrap();
        }

        assert_eq!(
            walk.on_packet(&ServerPacket::WalkReject(WalkReject {
                sequence: StepSequence(1),
                position: Point::new(100, 100, 0),
                facing: Facing::walking(Direction::North),
            })),
            Ok(Moved::Snapped {
                position: Point::new(100, 100, 0),
                facing: Facing::walking(Direction::North),
            })
        );
        assert_eq!(walk.in_flight(), 0, "everything in flight is void");
        assert_eq!(walk.predicted().position, Point::new(100, 100, 0));

        let bytes = walk
            .step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();
        assert_eq!(sent(&bytes).1, 0, "both ends are fresh again");
    }

    #[test]
    fn a_jump_the_client_did_not_ask_for_resets_it_too() {
        // `WorldState::move_to` resets the server's sequence on a teleport and
        // sends a 0x20. A client that kept counting would be refused on its very
        // next step, because a fresh server takes nothing but zero.
        let mut walk = walk();
        walk.step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();
        walk.step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();

        let update = PlayerUpdate {
            serial: Serial::new(0x0000_002A).unwrap(),
            body: Graphic(0x0190),
            hue: Hue::NONE,
            flags: openshard_protocol::mobile::StatusFlags::NONE,
            position: Point::new(2000, 2000, -5),
            facing: Facing::walking(Direction::South),
        };
        assert_eq!(
            walk.on_packet(&ServerPacket::PlayerUpdate(update)),
            Ok(Moved::Snapped {
                position: Point::new(2000, 2000, -5),
                facing: Facing::walking(Direction::South),
            })
        );
        assert_eq!(
            sent(
                &walk
                    .step(Facing::walking(Direction::South), |_, _, _| None)
                    .unwrap()
            )
            .1,
            0
        );
        assert_eq!(walk.predicted().position, Point::new(2000, 2001, -5));
    }

    #[test]
    fn a_second_player_start_is_a_jump_as_well() {
        let mut walk = walk();
        walk.step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();
        let start = PlayerStart {
            serial: Serial::new(0x0000_002A).unwrap(),
            body: Graphic(0x0190),
            position: Point::new(1475, 1770, 20),
            facing: Facing::walking(Direction::West),
            map: MapSize::BRITANNIA,
        };
        assert_eq!(
            walk.on_packet(&ServerPacket::PlayerStart(start)),
            Ok(Moved::Snapped {
                position: Point::new(1475, 1770, 20),
                facing: Facing::walking(Direction::West),
            })
        );
        assert_eq!(walk.in_flight(), 0);
    }

    /// Two ways to be handed an answer that fits nothing, and neither is
    /// swallowed. A `Walk` apiece, because the first one stops the walk — see
    /// `a_desync_stops_the_walk_until_the_server_says_where_the_body_is` — so a
    /// second step through the same value would be refused for that reason
    /// instead, and this test would stop testing what it says.
    #[test]
    fn an_ack_for_a_step_nobody_sent_is_reported_not_swallowed() {
        let mut nothing_in_flight = walk();
        assert_eq!(
            nothing_in_flight.on_packet(&ServerPacket::WalkAck(WalkAck {
                sequence: StepSequence(0),
                notoriety: Notoriety::Innocent,
            })),
            Err(UnexpectedAck {
                got: StepSequence(0),
                expected: None,
            })
        );

        let mut walk = walk();
        walk.step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();
        assert_eq!(
            walk.on_packet(&ServerPacket::WalkAck(WalkAck {
                sequence: StepSequence(9),
                notoriety: Notoriety::Innocent,
            })),
            Err(UnexpectedAck {
                got: StepSequence(9),
                expected: Some(StepSequence(0)),
            })
        );
        assert_eq!(
            walk.in_flight(),
            1,
            "the step stays in flight: guessing which one was meant would hide the desync"
        );
    }

    #[test]
    fn the_world_edge_is_refused_here_rather_than_asked_about() {
        let mut walk = Walk::new(Point::new(0, 0, 0), Facing::walking(Direction::West));
        assert_eq!(
            walk.step(Facing::walking(Direction::West), |_, _, _| None),
            Err(NotSent::AtTheWorldEdge(AtTheWorldEdge {
                from: Point::new(0, 0, 0),
                facing: Facing::walking(Direction::West),
            }))
        );
        assert_eq!(walk.in_flight(), 0, "nothing was sent, so nothing is pending");
        assert_eq!(
            walk.step(Facing::walking(Direction::East), |_, _, _| None)
                .map(|bytes| sent(&bytes).1),
            Ok(0),
            "and the sequence was not spent on it"
        );
    }

    /// The race a wall and a slow link produce, and it used to close the
    /// window: a refusal voids the steps still in flight, the shard answers
    /// those steps anyway, and their answers name sequences this end has
    /// forgotten.
    ///
    /// Reported as an [`UnexpectedAck`] once, which `client/app`'s `link` turns
    /// into a lost connection. They are ordinary traffic — the shard owes one
    /// answer per `0x02` and is delivering them — so they are counted off, and
    /// only an answer owed to *nobody* is still a desync.
    #[test]
    fn the_answers_to_steps_a_rollback_voided_are_not_a_desync() {
        let mut walk = walk();
        for _ in 0..3 {
            walk.step(Facing::walking(Direction::North), |_, _, _| None)
                .unwrap();
        }

        // The shard refused the first of the three. That answers one of them;
        // the other two are still coming.
        let reject = ServerPacket::WalkReject(WalkReject {
            sequence: StepSequence(0),
            position: Point::new(100, 100, 0),
            facing: Facing::walking(Direction::North),
        });
        assert!(matches!(walk.on_packet(&reject), Ok(Moved::Snapped { .. })));
        assert_eq!(walk.in_flight(), 0);

        // A step asked for after the rollback, which is what makes this hurt: a
        // client that treated the stale answers as its own would confirm or
        // roll back *this* step with them.
        walk.step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();

        // Now the two answers from before arrive. Whatever they are — the shard
        // refuses out-of-sequence steps, so both are `0x21`s here — they change
        // nothing.
        for sequence in 1..=2 {
            let stale = ServerPacket::WalkReject(WalkReject {
                sequence: StepSequence(sequence),
                position: Point::new(100, 100, 0),
                facing: Facing::walking(Direction::North),
            });
            assert_eq!(walk.on_packet(&stale), Ok(Moved::Idle), "answer {sequence}");
        }
        assert_eq!(
            walk.in_flight(),
            1,
            "the step sent after the rollback is still waiting for its own answer"
        );

        // And it gets it: the walk carries on with the sequence it restarted at.
        assert_eq!(
            walk.on_packet(&ServerPacket::WalkAck(WalkAck {
                sequence: StepSequence(0),
                notoriety: Notoriety::Innocent,
            })),
            Ok(Moved::Stepped {
                position: Point::new(100, 99, 0),
                facing: Facing::walking(Direction::North),
                notoriety: Notoriety::Innocent,
            })
        );
    }

    /// An ack owed to nobody is still reported. The drain is a count and not a
    /// blanket "ignore what you do not recognise": swallowing everything would
    /// turn a real desync into a body walking somewhere the server disagrees
    /// with, silently and for ever.
    #[test]
    fn an_ack_owed_to_nobody_is_still_a_desync() {
        let mut walk = walk();
        walk.step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();
        let reject = ServerPacket::WalkReject(WalkReject {
            sequence: StepSequence(0),
            position: Point::new(100, 100, 0),
            facing: Facing::walking(Direction::North),
        });
        walk.on_packet(&reject).unwrap();
        // One step, one answer: nothing is owed from before.
        assert_eq!(
            walk.on_packet(&ServerPacket::WalkAck(WalkAck {
                sequence: StepSequence(7),
                notoriety: Notoriety::Innocent,
            })),
            Err(UnexpectedAck {
                got: StepSequence(7),
                expected: None,
            })
        );
    }

    /// A desync stops the walk, and the server's answer starts it again.
    ///
    /// The freeze is only safe because the caller asks — `link.rs` sends the
    /// `0x22` resync — and the shard answers with a position. Both halves are
    /// asserted here: that nothing is sent while it holds, and that the `0x20`
    /// clears it. A client that froze without the question would freeze for the
    /// rest of the session.
    #[test]
    fn a_desync_stops_the_walk_until_the_server_says_where_the_body_is() {
        let mut walk = walk();
        walk.step(Facing::walking(Direction::North), |_, _, _| None)
            .unwrap();
        assert!(!walk.out_of_step());

        // An ack for a step nobody sent, with nothing owed from a rollback: a
        // real disagreement.
        assert!(
            walk.on_packet(&ServerPacket::WalkAck(WalkAck {
                sequence: StepSequence(9),
                notoriety: Notoriety::Innocent,
            }))
            .is_err()
        );
        assert!(walk.out_of_step(), "the walk stopped rather than guessing");
        assert_eq!(
            walk.step(Facing::walking(Direction::North), |_, _, _| None),
            Err(NotSent::OutOfStep),
            "and nothing is sent on top of the disagreement"
        );

        // The answer to the resync: a `0x20` saying where the body really is.
        let update = PlayerUpdate {
            serial: Serial::new(0x0000_002A).unwrap(),
            body: Graphic(0x0190),
            hue: Hue::NONE,
            flags: openshard_protocol::mobile::StatusFlags::NONE,
            position: Point::new(100, 100, 0),
            facing: Facing::walking(Direction::North),
        };
        assert_eq!(
            walk.on_packet(&ServerPacket::PlayerUpdate(update)),
            Ok(Moved::Snapped {
                position: Point::new(100, 100, 0),
                facing: Facing::walking(Direction::North),
            })
        );
        assert!(!walk.out_of_step(), "and the walk is free again");
        assert_eq!(
            walk.step(Facing::walking(Direction::North), |_, _, _| None)
                .map(|bytes| sent(&bytes).1),
            Ok(0),
            "from a fresh sequence, which is what the server also went back to"
        );
    }

    /// A shard that stops answering stops this client walking, five steps later.
    ///
    /// Without the cap the prediction runs off on its own for as long as the
    /// outage lasts, and the correction when the link comes back is that whole
    /// distance — a body sliding back across half a screen. The reference's own
    /// number, checked first thing in `PlayerMobile.Walk`.
    #[test]
    fn a_silent_shard_stops_the_walk_after_five_steps() {
        let mut walk = Walk::new(Point::new(100, 1000, 0), Facing::walking(Direction::North));
        for step in 0..MAX_IN_FLIGHT {
            walk.step(Facing::walking(Direction::North), |_, _, _| None)
                .unwrap_or_else(|_| panic!("step {step} is within the cap"));
        }
        assert_eq!(
            walk.step(Facing::walking(Direction::North), |_, _, _| None),
            Err(NotSent::Backlogged {
                in_flight: MAX_IN_FLIGHT
            })
        );
        assert_eq!(
            walk.predicted().position,
            Point::new(100, 995, 0),
            "and the refused step predicted nothing: five tiles, not six"
        );

        // One answer is one step of room, and no more.
        walk.on_packet(&ServerPacket::WalkAck(WalkAck {
            sequence: StepSequence(0),
            notoriety: Notoriety::Innocent,
        }))
        .unwrap();
        assert!(
            walk.step(Facing::walking(Direction::North), |_, _, _| None)
                .is_ok()
        );
        assert!(
            walk.step(Facing::walking(Direction::North), |_, _, _| None)
                .is_err()
        );
    }

    #[test]
    fn two_hundred_and_sixty_steps_never_send_a_second_zero() {
        // A zero mid-session reads as a reconnect to the server. The wrap is
        // `StepCounter`'s rule, and this is the client-side proof that walking
        // through it does not break the session — the bug would show up once
        // every 256 steps and nowhere else.
        // Far enough north to have 260 tiles of room: the map edge is a
        // different refusal and would hide this one.
        let mut walk = Walk::new(Point::new(100, 1000, 0), Facing::walking(Direction::North));
        for step in 0..260 {
            let bytes = walk
                .step(Facing::walking(Direction::North), |_, _, _| None)
                .unwrap();
            let sequence = sent(&bytes).1;
            assert!(step == 0 || sequence != 0, "step {step} sent a second zero");
            walk.on_packet(&ServerPacket::WalkAck(WalkAck {
                sequence: StepSequence(sequence),
                notoriety: Notoriety::Innocent,
            }))
            .unwrap();
        }
    }
}
