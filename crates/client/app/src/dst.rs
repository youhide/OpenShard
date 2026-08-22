//! A walk, held against an oracle, on a clock nobody sleeps on.
//!
//! # Two timelines
//!
//! Walking is answered in four places that never meet in a unit test: `steer.rs`
//! decides *when* a step is asked for, `client/net`'s [`Walk`] decides *where*
//! that step lands and predicts it, the shard decides whether it is allowed at
//! all, and `crowd.rs` decides where between two tiles the body is drawn this
//! frame. Each of the four has its own tests and each of them passed while the
//! walk on screen stuttered, because the bug was never in one of them — it was
//! in the *timing between* them. What is missing is an assertion about the thing
//! the player actually complains about: the position of the sprite, as a
//! function of the moment they pressed the key.
//!
//! So there are two timelines here.
//!
//! The **intent** timeline is the oracle: the body leaves the instant the key
//! goes down and crosses one tile per step, at a constant speed, for ever. It is
//! built from the script of inputs alone — it never sees a packet, a clock or a
//! `Crowd` — and it is deliberately the simplest kinematics that could be right.
//!
//! It has exactly one rule beyond that, and it is the queue rule the client
//! obeys (`steer.rs`, and `docs/client.md`): a press *while a step is under way*
//! moves no knot. It changes which way the step already owed will go, and that
//! step leaves at the deadline it always had. Without it the oracle would demand
//! a body that changes direction mid-tile, which is not a thing a grid walk can
//! do — the client that tried to give it one is what jumped the camera.
//!
//! The **event** timeline is everything below: a step is asked for when the
//! event loop happens to wake, crosses an mpsc to the net task, is predicted,
//! crosses back, and is glided over whatever [`crate::crowd::glide_time`] made
//! of the gap since the last one. Meanwhile the `0x02` crosses a wire with
//! latency and jitter on it and the shard answers a round trip later.
//!
//! The claim under test is that the second reproduces the first. That is not a
//! tautology dressed as a test: the oracle's knots are at `k * WALK_HOLD` from
//! the *press*, and the system's are at whenever the loop woke and whatever the
//! wire did. Every walking bug this client has had was a divergence between
//! those two sets of knots.
//!
//! # Not even the turn is a delay
//!
//! Turning is a whole step in UO: a mobile asked for a direction it is not
//! facing turns, moves nowhere, and gets its own `0x22`
//! (`openshard_movement::intend`). It is tempting to write that into the oracle
//! as a hold of standing still, and it would be wrong — the shard answers a turn
//! *before* it charges the pace budget (`Walker::request`), so the step the turn
//! precedes is legal in the same instant. `steer.rs` sends both in one wake and
//! the body sets off on the frame the key went down. So this oracle is constant
//! velocity from the moment of the ask and nothing else: no turn tax, no ramp,
//! no easing.
//!
//! # What is deliberately not covered
//!
//! The loop below is a copy of the ten lines in `App::about_to_wait` and
//! `App::user_event` that drive the walk, because those live inside a `winit`
//! handler that needs a window and a real clock. The copy is the known weakness
//! of this harness: a divergence introduced *into the copy* would be invisible
//! here. If one of these tests ever needs a rule that is only in `App`, the
//! answer is to lift that loop into a headless unit both can drive, not to grow
//! the copy — see `docs/client.md`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use openshard_client_net::walk::{Moved, Predicted, Walk};
use openshard_client_render::bench::{Cadence, Metrics, Sample};
use openshard_client_render::camera::{Camera, TILE_HEIGHT, TILE_WIDTH};
use openshard_client_render::chart;
use openshard_client_render::control::Control;
use openshard_client_render::follow::Gaze;
use openshard_client_render::follow::Rig;
use openshard_client_render::mobiles::{self, Mobile};
use openshard_movement::{
    OpenWorld, Terrain, Tile, WALK_HOLD, Walk as Handled, Walker, step_hold, step_progress,
};
use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::mobile::Notoriety;
use openshard_protocol::packet::{FramedClientPacket, decode_packet};
use openshard_protocol::serial::Serial;
use openshard_protocol::server_packet::ServerPacket;
use openshard_protocol::version::ClientVersion;
use openshard_protocol::wire::{Graphic, Hue};
use openshard_protocol::world::{Point, WalkAck, WalkReject, WalkRequest};

use crate::GLIDE_INTERVAL;
use crate::crowd::{Crowd, Ease, Who};
use crate::link::{self, Body};
use crate::net_command::project_motion;
use crate::steer::Ground;
use crate::world::PlayerMotion;

/// The body every scenario walks.
const BODY: Graphic = Graphic(0x0190);

/// Where every scenario starts. Far from the map's edges, so that no step is
/// refused for leaving the coordinate space.
const START: Point = Point::new(1000, 1000, 0);

/// A player-shaped `ClientVersion`, for the one packet that is decoded here.
fn version() -> ClientVersion {
    ClientVersion::new(7, 0, 45, 65)
}

/// Our own serial, as a `0x1B` would have named it.
fn me() -> Who {
    Some(Serial::new(0x0000_0001).unwrap())
}

// --- The oracle -----------------------------------------------------------

/// One thing the player did.
///
/// Scripts here hold one arrow at a time on purpose: which of several held
/// arrows wins is `keys.rs`'s rule, it has its own tests, and restating it in
/// the oracle would make the oracle a second copy of the thing under test.
#[derive(Clone, Copy, Debug)]
enum Input {
    /// An arrow went down.
    Press(Direction),
    /// An arrow came up.
    Release(Direction),
    /// Shift went down or came up.
    Running(bool),
}

/// A scripted input and the moment it happens.
#[derive(Clone, Copy, Debug)]
struct Act {
    /// How long after the scenario started.
    at: Duration,
    /// What the player did.
    input: Input,
}

/// A press at `millis`.
const fn press(millis: u64, direction: Direction) -> Act {
    Act {
        at: Duration::from_millis(millis),
        input: Input::Press(direction),
    }
}

/// A release at `millis`.
const fn release(millis: u64, direction: Direction) -> Act {
    Act {
        at: Duration::from_millis(millis),
        input: Input::Release(direction),
    }
}

/// One crossing on the intent timeline: where the body goes, from when, over
/// how long.
#[derive(Clone, Copy, Debug)]
struct Knot {
    /// When the crossing starts.
    at: Duration,
    /// How long it takes — the hold of the pace being walked at.
    takes: Duration,
    /// The tile it leaves, in tiles.
    from: (f64, f64),
    /// The tile it arrives at.
    to: (f64, f64),
}

/// Where the body would be, if the ask reached the screen with nothing in
/// between.
#[derive(Clone, Debug)]
struct Oracle {
    /// Where it stood before the first knot.
    start: (f64, f64),
    /// Every crossing, in order.
    knots: Vec<Knot>,
}

impl Oracle {
    /// Replay a script into crossings.
    ///
    /// The rules, all of them:
    ///
    /// - a press *while the body is standing* takes a step now, and the next is
    ///   due a hold later — waiting a whole step before the first one would put
    ///   400ms between the player and their character, which is the thing this
    ///   whole file exists to refuse;
    /// - a press *while a step is under way* takes no step at all. It changes
    ///   which way the step already owed will go, and that step still leaves at
    ///   the deadline the walk already had. This is the queue rule: an input
    ///   rebuilds what is asked for next and never cuts short what is being
    ///   walked — see `docs/client.md`;
    /// - every step crosses one tile over one hold at a constant speed,
    ///   whichever way the body was facing when it was asked for — a turn is a
    ///   packet, not a delay (see the module docs);
    /// - shift changes the pace of the *next* step and does not re-time the one
    ///   already due.
    fn build(start: Point, script: &[Act], until: Duration) -> Self {
        let mut knots = Vec::new();
        let mut position = (f64::from(start.x), f64::from(start.y));
        let mut running = false;
        let mut held: Option<Direction> = None;
        let mut due: Option<Duration> = None;
        let mut acts = script.iter().copied().peekable();
        let mut now;

        loop {
            // Whichever comes first: the player doing something, or the step
            // already owed falling due.
            let next_act = acts.peek().map(|act| act.at);
            now = match (next_act, due) {
                (Some(act), Some(step)) => act.min(step),
                (Some(act), None) => act,
                (None, Some(step)) => step,
                (None, None) => break,
            };
            if now > until {
                break;
            }

            if next_act == Some(now) {
                match acts.next().unwrap().input {
                    Input::Press(direction) => {
                        held = Some(direction);
                        // The queue rule: a step already under way is not cut
                        // short. The press says which way the step the walk
                        // already owes will go, and that one leaves when it was
                        // always going to.
                        if due.is_some_and(|step| step > now) {
                            continue;
                        }
                    }
                    Input::Release(_) => {
                        held = None;
                        due = None;
                        continue;
                    }
                    Input::Running(shift) => {
                        running = shift;
                        continue;
                    }
                }
            } else if held.is_none() {
                due = None;
                continue;
            }

            let Some(direction) = held else { continue };
            let takes = step_hold(running);
            // A turn costs nothing. It is a `0x02` of its own — turning is a
            // step in UO and the shard acks it — but it is not a *delay*: the
            // shard answers a turn before it charges the pace budget, so the
            // step it precedes leaves in the same instant, and the body sets off
            // on the frame the key went down. So the direction asked for is the
            // direction walked, from the first millisecond, and this oracle is
            // constant velocity and nothing else.
            let (dx, dy) = direction.step();
            let arrives = (position.0 + f64::from(dx), position.1 + f64::from(dy));
            knots.push(Knot {
                at: now,
                takes,
                from: position,
                to: arrives,
            });
            position = arrives;
            due = Some(now + takes);
        }

        Self {
            start: (f64::from(start.x), f64::from(start.y)),
            knots,
        }
    }

    /// Where the body should be at `when`.
    fn at(&self, when: Duration) -> (f64, f64) {
        let mut position = self.start;
        for knot in &self.knots {
            if when < knot.at {
                break;
            }
            let elapsed = when - knot.at;
            if elapsed >= knot.takes {
                position = knot.to;
                continue;
            }
            let progress = step_progress(elapsed, knot.takes);
            position = (
                knot.from.0 + (knot.to.0 - knot.from.0) * progress,
                knot.from.1 + (knot.to.1 - knot.from.1) * progress,
            );
            break;
        }
        position
    }
}

// --- The simulated world --------------------------------------------------

/// Everything between the key and the sprite that is not the client: how long a
/// packet takes, and how late the event loop wakes.
///
/// All four are deliberately separate knobs. Latency the client is supposed to
/// hide entirely — that is what predicting the step is *for* — and wake jitter
/// it cannot hide at all, so a test that mixed them could not say which one a
/// divergence came from.
#[derive(Clone, Copy, Debug, Default)]
struct Net {
    /// One way. A `0x02` takes this to reach the shard and its answer takes it
    /// to come back.
    latency: Duration,
    /// Added to each crossing, uniformly in `[0, jitter]`.
    jitter: Duration,
    /// How late the event loop wakes, uniformly in `[0, wake_jitter]`. A real
    /// one is never early.
    wake_jitter: Duration,
}

/// A seeded generator, so a failing scenario is a seed and not a story.
///
/// SplitMix64: four lines, no dependency, and the sequence is pinned by the
/// tests that use it.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    /// A uniform duration in `[0, span]`.
    fn upto(&mut self, span: Duration) -> Duration {
        if span.is_zero() {
            return Duration::ZERO;
        }
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        Duration::from_nanos(z % (span.as_nanos() as u64 + 1))
    }
}

/// A world with a wall in it, or none.
///
/// `can_step` is the whole `Terrain` contract, and the only reason there is one
/// here rather than [`OpenWorld`] is the rollback scenario: a refusal is the one
/// event the oracle cannot predict, and the client's behaviour around it is
/// worth pinning anyway.
struct Field {
    /// Tiles no step may land on. Empty is an open field.
    walls: Vec<Tile>,
}

impl Terrain for Field {
    fn can_step(&self, from: Point, to: Point) -> Option<Point> {
        match self.walls.contains(&Tile::new(to.x, to.y)) {
            true => None,
            false => OpenWorld.can_step(from, to),
        }
    }
}

/// The client, the wire and a shard, on one virtual clock.
struct Sim {
    /// The virtual clock: everything here is an offset from the start.
    now: Duration,
    /// One arbitrary real instant, because [`crate::steer::Steering`] and
    /// `WalkPace` take `Instant`s. Neither *reads* one — they only ever
    /// subtract two — so a base plus the virtual clock is a real clock as far as
    /// they can tell, and nothing in this file sleeps.
    base: Instant,

    steering: crate::steer::Steering,
    /// The client's prediction. On the net task in the real client, which is
    /// why the hop to it is modelled below rather than called inline.
    walk: Walk,
    crowd: Crowd,
    /// The body as `App` holds it: the tile, and the glide it was last drawn
    /// with.
    player: Mobile,

    /// The shard's side of the same walk — the real rules, sequence check and
    /// anti-speedhack bucket included.
    shard: Walker,
    field: Field,

    /// `0x02`s crossing to the shard, and answers crossing back, by arrival.
    to_shard: VecDeque<(Duration, FramedClientPacket)>,
    to_client: VecDeque<(Duration, ServerPacket)>,
    /// The prediction crossing the mpsc back into the window — `link::Update`.
    to_window: VecDeque<(Duration, Predicted, bool)>,

    net: Net,
    rng: Rng,

    /// The window's own clocks, exactly as `App` keeps them.
    next_tick: Duration,
    last_advance: Duration,

    /// How many steps the shard refused, for the assertions.
    refused: u32,
    /// When each step that *moved the body* was asked for.
    ///
    /// Turns are deliberately not counted: a turn covers no ground, costs the
    /// shard nothing, and leaves in the same wake as the step it precedes — so a
    /// cadence measured over both would read every direction change as two steps
    /// in one instant. What the pace budget and the player's eye both care about
    /// is the gap between two *crossings*.
    stepped_at: Vec<Duration>,
    /// Steps this end refused to send, and why. Empty in every scenario where
    /// the shard is answering.
    not_sent: Vec<(Duration, openshard_client_net::walk::NotSent)>,
    /// Where the body was *drawn*, every frame.
    trace: Vec<(Duration, (f64, f64))>,

    /// The camera, driven exactly as `App::draw` drives it.
    ///
    /// Here because the eye is the one thing a walking bug shows up in that the
    /// body's own trace cannot: the body is drawn against a world that the
    /// camera is moving underneath it, so a defect in *either* is a jump on
    /// screen and only one of them is what the trace above records.
    control: Control,
    /// Every frame of the camera, in the shape the bench measures.
    ///
    /// The same [`Sample`] `crates/client/render/src/bench.rs` produces from a
    /// scripted body, so the same [`Metrics`] can be run over both — which is
    /// the only thing that says the bench's synthetic walk is not a scene the
    /// rigs are being fitted to.
    eyes: Vec<Sample>,
}

impl Sim {
    /// A client standing at [`START`], facing `facing`, connected to a shard
    /// that agrees.
    fn new(facing: Direction, net: Net, seed: u64, walls: Vec<Tile>) -> Self {
        // The reference camera and no ease: every assertion in this file wants
        // a divergence to be the walk's, and both of those are a filter that
        // would answer for it.
        Self::flying(Rig::HARD, Ease::NONE, facing, net, seed, walls)
    }

    /// The same, under a rig of the caller's choosing.
    ///
    /// [`Sim::new`] is the reference camera because that is what every
    /// *assertion* here wants: under `HARD` the eye is the body, so a divergence
    /// is the walk's and not a filter's. A rig is a parameter for the dump that
    /// looks at what a filter does to this walk — the same argument the bench
    /// makes offline, on the body the client actually draws.
    fn flying(rig: Rig, ease: Ease, facing: Direction, net: Net, seed: u64, walls: Vec<Tile>) -> Self {
        let facing = Facing::walking(facing);
        let mut crowd = Crowd::default();
        crowd.set_ease(ease);
        crowd.commanding(me());
        let player = crowd.see(me(), START, BODY, facing, Hue::NONE, false);
        Self {
            now: Duration::ZERO,
            base: Instant::now(),
            steering: {
                // The oracle below is constant velocity from the moment of the
                // ask, and that is the point of it: what this harness is about
                // is the *cadence* of a walk under latency and wake jitter.
                // A turn is not part of that question — it covers no ground,
                // costs the shard's pace budget nothing, and the client's own
                // default delay in front of it (`Turning::Deliberate`, the
                // reference client's) would only be a constant the oracle had
                // to model twice to say the same thing. So the harness states
                // the turn away, and `steer.rs`'s own unit tests are what pin
                // what a turn costs.
                let mut steering = crate::steer::Steering::default();
                steering.set_turning(crate::steer::Turning::Immediate);
                steering
            },
            walk: Walk::new(START, facing),
            crowd,
            player,
            shard: Walker::new(START, facing),
            field: Field { walls },
            to_shard: VecDeque::new(),
            to_client: VecDeque::new(),
            to_window: VecDeque::new(),
            net,
            rng: Rng(seed),
            next_tick: Duration::ZERO,
            last_advance: Duration::ZERO,
            refused: 0,
            stepped_at: Vec::new(),
            not_sent: Vec::new(),
            trace: Vec::new(),
            // A viewport of some size and a device that will hold anything: what
            // is under test here is where the eye goes, and neither the zoom
            // ladder nor the texture limit has a say in that.
            control: Control::new(Camera::new(START, 800, 600), 1 << 20, rig),
            eyes: Vec::new(),
        }
    }

    /// The virtual clock as an `Instant`, for the two units that take one.
    fn instant(&self) -> Instant {
        self.base + self.now
    }

    /// How long one crossing of the wire takes this time.
    fn hop(&mut self) -> Duration {
        self.net.latency + self.rng.upto(self.net.jitter)
    }

    /// Run the script, waking the way the event loop wakes, until `until`.
    fn run(&mut self, script: &[Act], until: Duration) {
        let mut acts = script.iter().copied().peekable();
        loop {
            // Every reason to come back, exactly as `about_to_wait` computes the
            // deadline: the animation clock, the step a held key is owed, a
            // packet arriving — and, here only, the player doing something.
            let mut wake = self.next_tick.min(until);
            if let Some(step) = self.steering.deadline() {
                wake = wake.min(step - self.base);
            }
            for arrival in [
                self.to_shard.front().map(|(at, _)| *at),
                self.to_client.front().map(|(at, _)| *at),
                self.to_window.front().map(|(at, ..)| *at),
                acts.peek().map(|act| act.at),
            ]
            .into_iter()
            .flatten()
            {
                wake = wake.min(arrival);
            }
            // A loop that wakes on time is the easy case; a real one is woken by
            // the operating system whenever it gets round to it, and never early.
            let late = self.rng.upto(self.net.wake_jitter);
            self.now = (wake + late).max(self.now);
            if self.now > until {
                self.now = until;
            }

            while acts.peek().is_some_and(|act| act.at <= self.now) {
                let act = acts.next().unwrap();
                match act.input {
                    Input::Press(direction) => {
                        // `OpenWorld`, not `self.field` — see the note on
                        // `about_to_wait`.
                        if let Some(facing) = self.steering.press(
                            direction,
                            self.player.at,
                            self.instant(),
                            self.player.facing,
                            Ground::plain(&OpenWorld),
                        ) {
                            self.send(facing);
                        }
                    }
                    Input::Release(direction) => self.steering.release(direction),
                    Input::Running(shift) => self.steering.set_running(shift),
                }
            }
            self.deliver();
            self.about_to_wait();

            if self.now >= until {
                break;
            }
        }
    }

    /// Everything whose time has come, in the order it would really arrive.
    fn deliver(&mut self) {
        while self.to_shard.front().is_some_and(|(at, _)| *at <= self.now) {
            let (_, packet) = self.to_shard.pop_front().unwrap();
            // The one place the wrapper is opened, and it is the simulated
            // socket read — the same seam `link::play` unwraps at.
            let request: WalkRequest = decode_packet(packet.bytes(), version()).unwrap();
            let sequence = request.sequence.interpret();
            let answer = match self.shard.request(request, &self.field, self.instant(), false) {
                Handled::Turned { .. } | Handled::Moved { .. } => ServerPacket::WalkAck(WalkAck {
                    sequence,
                    notoriety: Notoriety::Innocent,
                }),
                Handled::Refused => {
                    self.refused += 1;
                    ServerPacket::WalkReject(WalkReject {
                        sequence,
                        position: self.shard.position,
                        facing: self.shard.facing,
                    })
                }
            };
            let at = self.now + self.hop();
            self.to_client.push_back((at, answer));
        }

        while self.to_client.front().is_some_and(|(at, _)| *at <= self.now) {
            let (_, packet) = self.to_client.pop_front().unwrap();
            // The net task's fold: `link.rs`'s `fold`, minus the `WorldView`,
            // which holds nothing our own body is drawn from.
            let moved = self.walk.on_packet(&packet).unwrap();
            // Only a correction is worth publishing. A `0x22` confirms a
            // position the screen already has — see `link::Body`.
            if let Moved::Snapped { .. } = moved {
                let at = self.now;
                self.to_window.push_back((at, self.walk.predicted(), true));
            }
        }

        while self.to_window.front().is_some_and(|(at, ..)| *at <= self.now) {
            let (_, predicted, corrected) = self.to_window.pop_front().unwrap();
            // `App::user_event`: the crowd's clock is brought up to date before
            // the packet is folded in, or the step is timestamped at the last
            // frame and the *next* crossing is measured from there.
            self.crowd.advance(self.now - self.last_advance);
            self.last_advance = self.now;
            // `App::entered`: a rollback is what makes the steering's idea of
            // the facing it asked for a lie, and it is told so.
            if corrected {
                self.steering.corrected(predicted.facing.direction);
            }
            // `App::entered`, for our own body.
            self.player = match corrected {
                true => self
                    .crowd
                    .snap(me(), predicted.position, BODY, predicted.facing, Hue::NONE, false),
                false => self
                    .crowd
                    .see(me(), predicted.position, BODY, predicted.facing, Hue::NONE, false),
            };
            // `App::user_event`: a step arriving while nobody was moving finds
            // the animation clock armed at the standing rate, and the first
            // 80ms of the glide would be drawn frozen.
            let soon = self.now + GLIDE_INTERVAL;
            if self.crowd.anyone_gliding() && self.next_tick > soon {
                self.next_tick = soon;
            }
        }
    }

    /// The ten lines of `App::about_to_wait` that walking goes through.
    fn about_to_wait(&mut self) {
        // `OpenWorld`, not `self.field`: `field` models the wall the *shard*
        // enforces (see its own doc), and the client's own static map is a
        // separate, empty one in every scenario this harness runs — the
        // point of the rollback scenarios below is a wall this end finds out
        // about only from a `0x21`. Handing `Steering::due` the shard's own
        // `field` would let `Steering::detour` route around exactly the
        // obstacle those scenarios exist to walk blindly into.
        //
        // Twice at most, exactly as `App::about_to_wait` does it: a turn costs
        // no time, so the step it precedes leaves in the same wake.
        for _ in 0..2 {
            let Some(facing) = self.steering.due(
                self.instant(),
                self.player.at,
                self.player.facing,
                Ground::plain(&OpenWorld),
            ) else {
                break;
            };
            self.send(facing);
        }
        if self.now >= self.next_tick {
            let elapsed = self.now - self.last_advance;
            self.crowd.advance(elapsed);
            self.last_advance = self.now;
            self.next_tick = self.now + self.redraw_interval();
            self.sample(elapsed);
        }
    }

    /// `App::redraw_interval` — the *timer*, which in the window is now only the
    /// fallback for a window nobody is watching (`App::pacing`).
    ///
    /// Deliberately the timer and not the display: this harness has no surface
    /// to block on, so "the display asks for the next frame as soon as the last
    /// is queued" has no meaning here, and the fastest cadence it can model is
    /// the glide clock's 16ms. That makes it the *coarser* of the two — a walk
    /// that is smooth under this harness is smooth at 60Hz, and not the other
    /// way round.
    fn redraw_interval(&self) -> Duration {
        match self.crowd.anyone_gliding() {
            true => GLIDE_INTERVAL,
            false => openshard_client_render::animation::FRAME_DELAY,
        }
    }

    /// `App::walk` with a link: ask, predict, and publish the prediction.
    ///
    /// Two hops, both of them an mpsc in the real client: the command out to
    /// the net task, and the snapshot back. Modelled with no delay of their own
    /// — they are a channel between two threads of one process — but modelled,
    /// because the *order* they impose is real.
    fn send(&mut self, facing: Facing) {
        let before = self.walk.predicted().position;
        // No map, so no height: `|_, _| None` is what a caller without one
        // passes, and the flat prediction is the honest answer.
        //
        // `link.rs` logs a refusal and sends nothing, and so does this: a step
        // past the cap on unanswered ones is a shard that has gone quiet, and the
        // body waits where it is.
        let packet = match self.walk.step(facing, |_, _| None) {
            Ok(packet) => packet,
            Err(refusal) => {
                self.not_sent.push((self.now, refusal));
                return;
            }
        };
        if self.walk.predicted().position != before {
            self.stepped_at.push(self.now);
        }
        let at = self.now + self.hop();
        self.to_shard.push_back((at, packet));
        self.to_window.push_back((self.now, self.walk.predicted(), false));
    }

    /// Where the body is drawn this frame, in tiles, and where the eye went.
    ///
    /// `App::draw` reads the glide from the crowd every frame rather than from
    /// the `Mobile` it last stored, and so does this: the stored one is as old
    /// as the last packet. `elapsed` is the span the crowd's clock was just
    /// advanced by, which is the same value `App::draw` hands the camera.
    fn sample(&mut self, elapsed: Duration) {
        self.player.drawn = self.crowd.drawn_for(me()).expect("the crowd knows our body");
        // `App::follow_player`, with the same gaze the sprite is placed from.
        let gaze = mobiles::gaze(&self.player);
        // And the trace is that same gaze read back in tiles, rather than a
        // second interpolation beside it: the oracle speaks in tiles and the
        // renderer in pixels, and the conversion is exact both ways. A trace
        // computed alongside the drawing is a trace of something nobody sees.
        self.trace.push((self.now, tiles_of(gaze)));
        self.control.follow_body(gaze, elapsed);
        self.eyes.push(Sample {
            at: self.now,
            gaze,
            eye: self.control.camera().eye(),
            state: self.control.eye_exact().expect("the eye was just placed"),
        });
    }

    /// The worst the drawn body ever was from where the oracle says it should
    /// have been, in tiles, and when.
    fn worst(&self, oracle: &Oracle) -> (Duration, f64) {
        let mut worst = (Duration::ZERO, 0.0);
        for (when, drawn) in &self.trace {
            let want = oracle.at(*when);
            let off = ((drawn.0 - want.0).powi(2) + (drawn.1 - want.1).powi(2)).sqrt();
            if off > worst.1 {
                worst = (*when, off);
            }
        }
        worst
    }
}

/// The player-only part of the event timeline, without a window, map assets,
/// or a live connection.  Unlike [`Sim`], this deliberately drives the same
/// app-thread movement core and render projection as `App::on_update`.
///
/// It exists for packet-order regressions that are too small to need the walk
/// oracle's shard and steering model: a double-click can make an ordinary
/// container packet arrive between the local prediction and its acknowledgement.
struct MotionKernel {
    motion: PlayerMotion,
    crowd: Crowd,
    player: Mobile,
}

impl MotionKernel {
    fn new(at: Point, facing: Facing) -> Self {
        let mut crowd = Crowd::default();
        crowd.commanding(me());
        let player = crowd.see(me(), at, BODY, facing, Hue::NONE, false);
        Self {
            motion: PlayerMotion::new(at, facing),
            crowd,
            player,
        }
    }

    fn predict(&mut self, body: Body, sequence: openshard_protocol::world::StepSequence) {
        self.motion.accept_local(body, sequence);
        self.project();
    }

    fn mutation(&mut self, movement: Option<link::Movement>) {
        self.motion.accept_network(movement);
        self.project();
    }

    fn frame(&mut self, elapsed: Duration) {
        self.motion.advance(elapsed);
        self.crowd.advance(elapsed);
        self.project();
    }

    fn project(&mut self) {
        project_motion(
            &mut self.crowd,
            me(),
            &mut self.player,
            self.motion.render_state(),
            false,
        );
        // This is the same final local-player projection the frame builder
        // performs after Crowd has supplied the animation group and frame.
        self.player.drawn = self.motion.drawn();
    }
}

/// Assert the drawn walk never left a corridor of `tiles` around the oracle.
#[track_caller]
fn tracks(sim: &Sim, oracle: &Oracle, tiles: f64) {
    let (when, off) = sim.worst(oracle);
    assert!(
        off <= tiles,
        "the body was {off:.4} tiles from where it should have been at {when:?}, \
         which is more than the {tiles} this scenario allows"
    );
    assert!(
        sim.trace.len() > 100,
        "only {} frames were drawn: a corridor nothing walked down is not an assertion",
        sim.trace.len()
    );
}

/// Assert the drawn body never moved faster than a body walks.
///
/// The complaint this exists for is a *jump*: the camera is locked to the drawn
/// body, so a body that changes tile between two frames without walking there
/// takes the whole world with it. A corridor around the oracle does not catch
/// one on its own — a jump forwards and a jump back can both sit inside it — and
/// this does: a walk covers one tile per `hold`, so between two frames `dt`
/// apart no axis may move further than `dt / hold`.
///
/// Per axis rather than as a distance, because a diagonal step covers a whole
/// tile on both axes in one hold and a Euclidean bound would have to be widened
/// by `sqrt(2)` for it — which is exactly enough slack to hide a jump on a
/// straight one.
///
/// Not for the rollback scenarios: a correction *is* a jump, deliberately, and
/// it has its own assertions.
#[track_caller]
fn continuous(sim: &Sim, hold: Duration) {
    for pair in sim.trace.windows(2) {
        let ((before, was), (when, now)) = (pair[0], pair[1]);
        let dt = (when - before).as_secs_f64();
        // A frame's worth of walking, and a fiftieth of a tile for the arithmetic.
        let allowed = dt / hold.as_secs_f64() + 0.02;
        let moved = (now.0 - was.0).abs().max((now.1 - was.1).abs());
        assert!(
            moved <= allowed,
            "the body jumped {moved:.4} tiles between {before:?} and {when:?}, \
             which is more than the {allowed:.4} a walk covers in that time"
        );
    }
}

/// Assert no two crossings were asked for closer together than `hold`.
///
/// The other half of the queue rule, and the one the shard sees: a step that
/// leaves early is one the pace budget has not paid for, and enough of them is a
/// `0x21` and a body yanked backwards. Measured on the asks rather than on the
/// acks, because this is the client's own cadence and it must be right before
/// the wire is involved.
#[track_caller]
fn paced(sim: &Sim, hold: Duration) {
    for pair in sim.stepped_at.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap + Duration::from_millis(1) >= hold,
            "two steps left {gap:?} apart, which is faster than the {hold:?} a body walks"
        );
    }
}

/// Assert the drawn body never *outran* a walk, over a whole scenario.
///
/// [`continuous`] is the same claim per frame pair, and it is not the same test:
/// it allows a fiftieth of a tile of arithmetic slack per frame, which at sixty
/// frames a second is a tile and a half a second of unnoticed burst. This one
/// takes the worst frame in the run against the nominal step and reports the
/// ratio, so a regression is a number that moved rather than a threshold that
/// happened to still hold.
///
/// The bound comes from a mutation and not from the measurement's own headroom.
/// With a crossing that starts on the tile boundary and runs for the nominal
/// time from the arrival — which is what this repository shipped until the walk
/// dump was written — ten steps under eight milliseconds of wake jitter peaked
/// between 1.28 and 1.6 times a walk, one frame per tile. With the two rules
/// that replaced it (`crowd::crossing`, and a step that starts from where the
/// body is drawn) the worst frame over eight seeds is 1.036, which is the two
/// per cent the lateness now costs in *speed* instead of in position. So 1.10 is
/// a factor above what the fix produces and far under what the defect did.
///
/// Either rule alone suppresses the burst and neither is redundant: the schedule
/// is what keeps the body from parking on its tile, and starting from the drawn
/// position is what makes any arrival at all — a rollback, an NPC on a wire —
/// continuous rather than merely well timed.
#[track_caller]
fn never_outran_a_walk(sim: &Sim, hold: Duration, times: f64) {
    let (mut worst, mut when) = (0.0f64, Duration::ZERO);
    for pair in sim.trace.windows(2) {
        let ((before, was), (at, now)) = (pair[0], pair[1]);
        let dt = (at - before).as_secs_f64();
        // A frame of no elapsed time is not a speed, and the walk is over the
        // axis a straight step moves along — see [`continuous`] on why not the
        // distance.
        if dt <= 0.0 {
            continue;
        }
        let moved = (now.0 - was.0).abs().max((now.1 - was.1).abs());
        let ratio = moved / (dt / hold.as_secs_f64());
        if ratio > worst {
            (worst, when) = (ratio, at);
        }
    }
    assert!(
        sim.trace.len() > 100,
        "only {} frames: a ceiling nothing walked under is not an assertion",
        sim.trace.len(),
    );
    assert!(
        worst <= times,
        "the body covered {worst:.2} times a walk's ground in one frame at {when:?}, \
         which is a body yanked rather than a body walking"
    );
}

/// Assert the reference rig put the eye exactly on the drawn body, every frame.
///
/// `Rig::HARD` *is* that sentence — it is every time constant at zero — so what
/// this holds is not the arithmetic, which the two share by construction. It is
/// the wiring: that the camera is advanced on every frame the body is, from the
/// same gaze the sprite is placed from, with nothing accumulated between frames
/// and nothing a frame late. That is the whole of what C0 transplanted, and
/// every one of those is a way to be wrong without any test in
/// `client/render` noticing.
#[track_caller]
fn eye_is_the_body(sim: &Sim) {
    for sample in &sim.eyes {
        assert_eq!(
            sample.eye,
            sample.gaze.eye().pixel(),
            "the eye was not on the body at {:?}",
            sample.at,
        );
    }
    // A corridor nothing walked down is not an assertion — the same companion
    // `tracks` carries, and for the same reason: an eye that never moved sits
    // exactly on a body that never moved.
    // Travelled rather than spanned, so a scenario that walks back and forth
    // between two tiles counts as much as one that walks in a straight line.
    // The bench's own measure, over the bench's own type, for the same reason
    // the samples are that type: two harnesses that counted differently could
    // not be compared.
    let metrics = Metrics::of(&sim.eyes);
    assert!(metrics.frames > 100, "only {} frames were drawn", metrics.frames);
    // Two hundred pixels is six tiles of screen. Ten steps east is 311 and the
    // shortest scenario here is a walk into a wall, so the bar is under all of
    // them and nowhere near a scene where nothing happened.
    assert!(
        metrics.travel > 200.0,
        "the eye travelled {:.0} pixels across the whole run, \
         so it was never really asked to follow anything",
        metrics.travel,
    );
}

/// Ten steps east, held from the first millisecond.
fn ten_steps_east() -> Vec<Act> {
    vec![press(0, Direction::East), release(4_000, Direction::East)]
}

// --- The scenarios ---------------------------------------------------------

/// A double-click can cause `OpenContainer` to arrive while the step it was
/// made during is still gliding.  That packet is a world mutation, but it is
/// not a movement event: accepting its generic `Body` used to overwrite the
/// visual source with the server's old tile and leave the sprite behind the
/// predicted/HUD position.
///
/// This runs the actual packet fold, both app-side movement cores, and the
/// player projection on a virtual clock. No window or client process is
/// required to reproduce the packet ordering.
#[test]
fn double_click_container_packet_cannot_desynchronise_a_predicted_glide() {
    let facing = Facing::walking(Direction::East);
    let mut wire = Walk::new(START, facing);
    wire.step(facing, |_, _| None).expect("the first step is valid");
    let sequence = wire
        .newest_pending_sequence()
        .expect("the predicted step has protocol identity");
    let predicted = wire.predicted();
    let mut kernel = MotionKernel::new(START, facing);
    kernel.predict(
        Body {
            predicted,
            corrected: false,
        },
        sequence,
    );
    kernel.frame(WALK_HOLD / 2);
    let halfway = kernel.motion.drawn();
    assert_ne!(halfway, Gaze::on(START), "the local prediction is gliding");
    let motion_before_container = kernel.motion.clone();

    let container = ServerPacket::OpenContainer(openshard_protocol::containers::OpenContainer {
        container: Serial::new(0x4000_0001).unwrap(),
        gump: Graphic(0x003C),
    });
    let folded = link::fold(&mut wire, &container).expect("a container does not disturb Walk");
    assert!(
        folded.movement.is_none(),
        "the double-click packet has no movement fact"
    );
    kernel.mutation(folded.movement);
    assert_eq!(
        kernel.motion, motion_before_container,
        "opening a container cannot change any movement-core field"
    );
    assert_eq!(kernel.motion.planning_state(), predicted);
    assert_eq!(kernel.motion.pending_steps(), 1);
    assert_eq!(
        kernel.motion.route_origin(),
        START,
        "the HUD begins at the active transition source until the delayed acknowledgement"
    );
    assert_eq!(kernel.motion.transition_from(), Some(START));
    assert_eq!(kernel.crowd.stepping_from(me()), Some(START));
    assert_eq!(
        kernel.motion.drawn(),
        halfway,
        "rebuilding the container presentation cannot restart or snap the glide"
    );

    let ack = ServerPacket::WalkAck(WalkAck {
        sequence,
        notoriety: Notoriety::Innocent,
    });
    let folded = link::fold(&mut wire, &ack).expect("the pending step is acknowledged");
    kernel.mutation(folded.movement);
    assert_eq!(kernel.motion.drawn(), halfway);

    kernel.frame(WALK_HOLD / 2);
    assert_eq!(kernel.motion.confirmed_state(), predicted);
    assert_eq!(kernel.motion.planning_state(), predicted);
    assert_eq!(kernel.motion.pending_steps(), 0);
    assert_eq!(kernel.motion.transition_from(), None);
    assert_eq!(kernel.motion.drawn(), Gaze::on(predicted.position));
    assert_eq!(kernel.crowd.stepping_from(me()), None);
}

/// The trace that originally exposed the bug was not a pending online walk:
/// `confirmed` and the route advanced with `pending=0` while `Crowd` remained
/// on the replay's start tile.  A replay/offline step is trusted immediately,
/// but it must still publish the same transition to the render clock before
/// the next frame asks for the HUD and sprite.
#[test]
fn trusted_replay_steps_cannot_advance_the_hud_past_the_drawn_body() {
    let facing = Facing::walking(Direction::East);
    let first = Point::new(START.x + 1, START.y, START.z);
    let second = Point::new(START.x + 2, START.y, START.z);
    let mut kernel = MotionKernel::new(START, facing);

    kernel.motion.accept_trusted_step(first, facing);
    kernel.project();
    kernel.frame(WALK_HOLD);
    assert_eq!(kernel.motion.drawn(), Gaze::on(first));
    assert_eq!(kernel.motion.route_origin(), first);

    kernel.motion.accept_trusted_step(second, facing);
    kernel.project();
    kernel.frame(WALK_HOLD / 2);

    assert_eq!(
        kernel.motion.route_origin(),
        first,
        "HUD starts at the glide source"
    );
    assert_eq!(kernel.motion.transition_from(), Some(first));
    assert_eq!(kernel.crowd.stepping_from(me()), Some(first));
    assert_ne!(kernel.motion.drawn(), Gaze::on(first));
    assert_ne!(kernel.motion.drawn(), Gaze::on(second));

    kernel.frame(WALK_HOLD / 2);
    assert_eq!(kernel.motion.route_origin(), second);
    assert_eq!(kernel.motion.transition_from(), None);
    assert_eq!(kernel.motion.drawn(), Gaze::on(second));
}

/// A blocked window can age several motion-core transitions in one frame.
/// Crowd receives only the newly active transition afterwards, so it must
/// rebase its animation clock at that explicit source instead of assuming it
/// saw every intermediate command.
#[test]
fn a_stalled_frame_rebases_crowd_at_the_active_motion_source() {
    let facing = Facing::walking(Direction::East);
    let first = Point::new(START.x + 1, START.y, START.z);
    let second = Point::new(START.x + 2, START.y, START.z);
    let third = Point::new(START.x + 3, START.y, START.z);
    let mut kernel = MotionKernel::new(START, facing);

    for (position, sequence) in [(first, 1), (second, 2), (third, 3)] {
        kernel.predict(
            Body {
                predicted: openshard_client_net::walk::Predicted { position, facing },
                corrected: false,
            },
            openshard_protocol::world::StepSequence(sequence),
        );
    }

    kernel.frame(WALK_HOLD * 2);

    assert_eq!(kernel.motion.transition_from(), Some(second));
    assert_eq!(kernel.crowd.stepping_from(me()), Some(second));
    assert_eq!(kernel.player.drawn, kernel.motion.drawn());
}

/// The local core owns the whole smooth chain while acknowledgements are in
/// flight.  The shard arbitrates those numbered steps, rather than pacing the
/// picture: only a refusal is allowed to discard the unconfirmed suffix.
#[test]
fn refusing_the_oldest_step_snaps_and_discards_the_entire_local_chain() {
    let facing = Facing::walking(Direction::East);
    let first = Point::new(START.x + 1, START.y, START.z);
    let second = Point::new(START.x + 2, START.y, START.z);
    let mut kernel = MotionKernel::new(START, facing);

    for (position, sequence) in [(first, 41), (second, 42)] {
        kernel.predict(
            Body {
                predicted: Predicted { position, facing },
                corrected: false,
            },
            openshard_protocol::world::StepSequence(sequence),
        );
    }
    kernel.frame(WALK_HOLD / 2);
    assert!(
        kernel.motion.is_gliding(),
        "the local core continues before a server answer"
    );
    assert_eq!(kernel.motion.pending_steps(), 2);
    assert_eq!(kernel.motion.planning_state().position, second);
    assert_eq!(kernel.motion.route_origin(), START);

    kernel.mutation(Some(link::Movement::Reject {
        sequence: openshard_protocol::world::StepSequence(41),
        confirmed: Predicted {
            position: START,
            facing,
        },
    }));

    assert!(
        !kernel.motion.is_gliding(),
        "a refusal ends the local chain at once"
    );
    assert_eq!(kernel.motion.confirmed_state().position, START);
    assert_eq!(kernel.motion.planning_state().position, START);
    assert_eq!(kernel.motion.pending_steps(), 0);
    assert_eq!(kernel.motion.route_origin(), START);
    assert_eq!(kernel.motion.transition_from(), None);
    assert_eq!(kernel.motion.drawn(), Gaze::on(START));
    assert_eq!(kernel.player.drawn, Gaze::on(START));
    assert_eq!(kernel.crowd.stepping_from(me()), None);
}

/// The fixed DST scripts pin known timing regressions; this property search
/// covers the crossings between them.  It deliberately mixes packet outcomes
/// with long presentation gaps, because a queue that is correct event-by-event
/// can still fail when one frame consumes several queued transitions.
#[test]
fn fuzzed_motion_events_keep_the_core_and_crowd_projection_consistent() {
    use proptest::prelude::*;

    proptest!(ProptestConfig::with_cases(1_024), |(
        events in prop::collection::vec(
            (
                0_u8..6,
                -1_i8..=1,
                -1_i8..=1,
                -3_i8..=3,
                0_u16..=2_000,
            ),
            1..96,
        ),
    )| {
        let facing = Facing::walking(Direction::East);
        let mut kernel = MotionKernel::new(START, facing);
        let mut outstanding = VecDeque::new();
        let mut next_sequence = 0_u8;
        let mut confirmed = Predicted {
            position: START,
            facing,
        };

        let offset = |value: u16, delta: i8| {
            (i32::from(value) + i32::from(delta)).clamp(0, i32::from(u16::MAX)) as u16
        };

        for (kind, dx, dy, dz, elapsed) in events {
            match kind {
                // Locally accepted protocol step, including a turn or a
                // height-only move: all are useful boundary cases for the
                // motion queue even when a real terrain rule would reject
                // some of them before the network layer.
                0 => {
                    let from = kernel.motion.planning_state();
                    let predicted = Predicted {
                        position: Point::new(
                            offset(from.position.x, dx),
                            offset(from.position.y, dy),
                            from.position.z.saturating_add(dz),
                        ),
                        facing,
                    };
                    let sequence = openshard_protocol::world::StepSequence(next_sequence);
                    next_sequence = next_sequence.wrapping_add(1);
                    kernel.predict(
                        Body {
                            predicted,
                            corrected: false,
                        },
                        sequence,
                    );
                    outstanding.push_back((sequence, predicted));
                }
                // A display or event-loop stall: this is what can consume more
                // than one queued transition before Crowd sees the next one.
                1 => kernel.frame(Duration::from_millis(u64::from(elapsed))),
                // Acknowledge exactly the oldest outstanding step, as the walk
                // protocol requires.
                2 => {
                    if let Some((sequence, accepted)) = outstanding.pop_front() {
                        confirmed = accepted;
                        kernel.mutation(Some(link::Movement::Ack {
                            sequence,
                            confirmed: accepted,
                        }));
                    }
                }
                // Reject the oldest step and discard the rest, matching
                // `Walk::snap`'s rollback semantics.
                3 => {
                    if let Some((sequence, _)) = outstanding.pop_front() {
                        let from = kernel.motion.planning_state();
                        confirmed = Predicted {
                            position: Point::new(
                                offset(from.position.x, dx),
                                offset(from.position.y, dy),
                                from.position.z.saturating_add(dz),
                            ),
                            facing,
                        };
                        kernel.mutation(Some(link::Movement::Reject {
                            sequence,
                            confirmed,
                        }));
                        outstanding.clear();
                    }
                }
                // A server relocation is not paired with a sequence and
                // invalidates every outstanding local prediction.
                4 => {
                    let from = kernel.motion.planning_state();
                    confirmed = Predicted {
                        position: Point::new(
                            offset(from.position.x, dx),
                            offset(from.position.y, dy),
                            from.position.z.saturating_add(dz),
                        ),
                        facing,
                    };
                    kernel.mutation(Some(link::Movement::Relocation { confirmed }));
                    outstanding.clear();
                }
                // Any ordinary packet is a movement no-op.
                _ => {
                    let before = kernel.motion.clone();
                    kernel.mutation(None);
                    prop_assert_eq!(&kernel.motion, &before);
                }
            }

            let snapshot = kernel.motion.snapshot();
            prop_assert_eq!(snapshot.confirmed, confirmed);
            prop_assert_eq!(snapshot.predicted, kernel.motion.planning_state());
            prop_assert_eq!(snapshot.pending_steps, outstanding.len());
            match snapshot.transition {
                Some((from, to)) => {
                    prop_assert_ne!(from, to);
                    prop_assert_eq!(snapshot.route_origin, from);
                }
                None => {
                    prop_assert_eq!(snapshot.route_origin, snapshot.predicted.position);
                    prop_assert_eq!(snapshot.rendered, Gaze::on(snapshot.predicted.position));
                }
            }
            prop_assert_eq!(kernel.player.drawn, kernel.motion.drawn());
        }
    });
}

/// The oracle is worth exactly as much as its own arithmetic, so pin it first:
/// ten steps, four seconds, one tile per 400ms, and nothing in between.
#[test]
fn the_oracle_walks_ten_tiles_in_ten_holds() {
    let oracle = Oracle::build(START, &ten_steps_east(), Duration::from_millis(4_000));
    assert_eq!(oracle.at(Duration::ZERO), (1000.0, 1000.0));
    assert_eq!(oracle.at(Duration::from_millis(200)), (1000.5, 1000.0));
    assert_eq!(oracle.at(WALK_HOLD), (1001.0, 1000.0));
    assert_eq!(oracle.at(WALK_HOLD * 10), (1010.0, 1000.0));
    // And the speed is constant across every one of the ten, which is the whole
    // claim: no dwell at a tile boundary, no catch-up after one.
    for step in 0..10 {
        let quarter = WALK_HOLD * step + WALK_HOLD / 4;
        assert!((oracle.at(quarter).0 - (1000.25 + f64::from(step))).abs() < 1e-9);
    }
}

/// A body facing north, asked to go east, is walking east this instant — it does
/// not stand still for a hold first. See the module docs.
#[test]
fn the_oracle_charges_nothing_for_the_turn() {
    let oracle = Oracle::build(START, &ten_steps_east(), Duration::from_millis(4_000));
    // The same walk as above, and the body was facing the other way when the
    // key went down. It makes no difference: the turn is a packet, not a wait.
    assert_eq!(oracle.at(WALK_HOLD / 2), (1000.5, 1000.0));
    assert_eq!(oracle.at(WALK_HOLD * 10), (1010.0, 1000.0));
}

/// The headline: on a perfect wire and a punctual loop, the drawn body *is* the
/// oracle.
#[test]
fn ten_steps_on_a_perfect_wire_are_the_oracle() {
    let script = ten_steps_east();
    let until = Duration::from_millis(4_000);
    let oracle = Oracle::build(START, &script, until);
    let mut sim = Sim::new(Direction::East, Net::default(), 1, Vec::new());
    sim.run(&script, until);

    tracks(&sim, &oracle, 0.02);
    continuous(&sim, WALK_HOLD);
    paced(&sim, WALK_HOLD);
    assert_eq!(sim.refused, 0, "an open field refuses nothing");
    assert_eq!(sim.shard.position, Point::new(1010, 1000, 0));
}

/// The one that pays for the prediction: a third of a second of latency, and
/// jitter on top of it, must not reach the screen at all. The corridor is the
/// same as the perfect wire's.
#[test]
fn latency_and_jitter_do_not_reach_the_screen() {
    let script = ten_steps_east();
    let until = Duration::from_millis(4_000);
    let oracle = Oracle::build(START, &script, until);
    for seed in 0..8 {
        let net = Net {
            latency: Duration::from_millis(150),
            jitter: Duration::from_millis(60),
            wake_jitter: Duration::ZERO,
        };
        let mut sim = Sim::new(Direction::East, net, seed, Vec::new());
        sim.run(&script, until);
        tracks(&sim, &oracle, 0.02);
        assert_eq!(
            sim.refused, 0,
            "seed {seed}: a walk at the hold is not a speedhack"
        );
    }
}

/// The pace the client asks at is the shard's business, and the shard here is
/// the real `Walker` with the real bucket. Running is the fast end of what a
/// player can legitimately ask for, so it is what the anti-speedhack floor is
/// tested against.
#[test]
fn running_the_whole_way_is_never_refused_as_a_speedhack() {
    let script = vec![
        Act {
            at: Duration::ZERO,
            input: Input::Running(true),
        },
        press(0, Direction::East),
        release(4_000, Direction::East),
    ];
    let until = Duration::from_millis(4_000);
    let oracle = Oracle::build(START, &script, until);
    let net = Net {
        latency: Duration::from_millis(80),
        jitter: Duration::from_millis(40),
        wake_jitter: Duration::ZERO,
    };
    let mut sim = Sim::new(Direction::East, net, 7, Vec::new());
    sim.run(&script, until);

    tracks(&sim, &oracle, 0.02);
    assert_eq!(sim.refused, 0, "twenty steps at RUN_HOLD");
    assert_eq!(sim.shard.position, Point::new(1020, 1000, 0));
}

/// The complaint this rule came from: the body is facing one way and the player
/// presses another. There is a turn in front of the walk, and it must cost
/// nothing — the oracle is moving from the first millisecond, and so is the body.
#[test]
fn a_walk_that_starts_with_a_turn_leaves_at_once() {
    let script = ten_steps_east();
    let until = Duration::from_millis(4_000);
    let oracle = Oracle::build(START, &script, until);
    let net = Net {
        latency: Duration::from_millis(120),
        jitter: Duration::from_millis(40),
        wake_jitter: Duration::ZERO,
    };
    // Facing north, asked for east.
    let mut sim = Sim::new(Direction::North, net, 5, Vec::new());
    sim.run(&script, until);

    tracks(&sim, &oracle, 0.02);
    assert_eq!(sim.refused, 0, "a turn is not charged to the pace budget");
    assert_eq!(
        sim.shard.position,
        Point::new(1010, 1000, 0),
        "ten tiles, not nine"
    );
}

/// A walk that changes direction: five east, then five south-east. The turn
/// costs its hold on both timelines, and the diagonal is one step like any
/// other.
#[test]
fn a_walk_that_turns_tracks_the_oracle_through_the_turn() {
    let script = vec![
        press(0, Direction::East),
        release(2_000, Direction::East),
        press(2_000, Direction::SouthEast),
        release(4_800, Direction::SouthEast),
    ];
    let until = Duration::from_millis(4_800);
    let oracle = Oracle::build(START, &script, until);
    let net = Net {
        latency: Duration::from_millis(120),
        jitter: Duration::from_millis(30),
        wake_jitter: Duration::ZERO,
    };
    let mut sim = Sim::new(Direction::East, net, 3, Vec::new());
    sim.run(&script, until);

    tracks(&sim, &oracle, 0.02);
    assert_eq!(sim.refused, 0);
}

/// The event loop woken late is the one thing prediction cannot hide: the step
/// is asked for when the loop wakes, so a loop that is 20ms late is a body 20ms
/// behind. What must not happen is that the lateness *accumulates* — ten steps
/// each armed from a late wake would leave the body a fifth of a tile behind
/// for ever, and a hundred steps two whole tiles.
#[test]
fn wake_up_jitter_does_not_accumulate() {
    // Forty steps rather than ten, because accumulation is the whole question:
    // the few milliseconds a late wake costs are invisible in one step and a
    // whole tile by the fortieth. A corridor a walk of ten fits down is not an
    // assertion about drift.
    let until = Duration::from_millis(16_000);
    let script = vec![press(0, Direction::East), release(16_000, Direction::East)];
    let oracle = Oracle::build(START, &script, until);
    let late = Duration::from_millis(20);
    for seed in 0..8 {
        let net = Net {
            latency: Duration::from_millis(50),
            jitter: Duration::ZERO,
            wake_jitter: late,
        };
        let mut sim = Sim::new(Direction::East, net, seed, Vec::new());
        sim.run(&script, until);
        // Three late wakes' worth of tile and no more, ever. Three because the
        // lateness lands in three places and none of them is the client's to
        // fix: the loop wakes late to *ask* for the step, it wakes late again to
        // fold the prediction the net task sent back, and what is left is the
        // difference between the hold the body is glided over and the interval
        // the steps actually left at. What matters is that the three do not add
        // up over forty steps, which is what arming the next step from the
        // deadline rather than from the wake is for.
        let corridor = 3.0 * late.as_secs_f64() / WALK_HOLD.as_secs_f64() + 0.02;
        tracks(&sim, &oracle, corridor);
    }
}

/// And the other half of what a late wake must not do: reach the *speed*.
///
/// A corridor around the oracle bounds where the body is and says nothing about
/// how it got there — a body that parks for a frame and then covers two frames
/// of ground sits inside every corridor this file draws, and it is the jerk
/// people actually report. It was real: a crossing timestamped at the arrival
/// re-randomises its phase every tile, so the body's position stepped by the
/// difference of two wake latenesses at every tile boundary, and one frame in
/// four hundred milliseconds covered 1.6 times a walk's ground.
///
/// So this scenario asserts the ceiling directly, over the same walk at the
/// same jitter, and the companion — the body did walk the whole way — is inside
/// [`never_outran_a_walk`] and [`tracks`] both. `docs/camera.md` C4 has the
/// picture this came out of; `dst::dump_the_walk` draws it.
#[test]
fn wake_up_jitter_does_not_reach_the_speed() {
    let until = Duration::from_millis(4_400);
    let script = vec![press(0, Direction::East), release(4_000, Direction::East)];
    let oracle = Oracle::build(START, &script, until);
    for seed in 0..8 {
        let net = Net {
            latency: Duration::from_millis(60),
            jitter: Duration::from_millis(20),
            wake_jitter: Duration::from_millis(8),
        };
        let mut sim = Sim::new(Direction::East, net, seed, Vec::new());
        sim.run(&script, until);
        never_outran_a_walk(&sim, WALK_HOLD, 1.10);
        // A tile and a half of corridor would pass a body that never moved, so
        // the position is held too — tightly, because this wire is a tenth of
        // the jitter the drift scenario above runs at.
        tracks(&sim, &oracle, 0.1);
        assert_eq!(
            sim.refused, 0,
            "seed {seed}: a walk at the hold is not a speedhack"
        );
    }
}

/// A wall the client cannot see is walked into, and the refusal is a rollback:
/// the body is *put* back rather than glided back, and the walk goes on.
///
/// The oracle has nothing to say here — it does not know about the wall — so
/// what is asserted is the two things a rollback must not do: draw the body
/// walking backwards, and leave it anywhere but where the shard says.
#[test]
fn a_refusal_puts_the_body_back_without_walking_it_back() {
    let script = ten_steps_east();
    let until = Duration::from_millis(4_000);
    let net = Net {
        latency: Duration::from_millis(100),
        jitter: Duration::ZERO,
        wake_jitter: Duration::ZERO,
    };
    let mut sim = Sim::new(Direction::East, net, 11, vec![Tile::new(1004, 1000)]);
    sim.run(&script, until);

    assert!(sim.refused > 0, "the wall is on the way");
    assert_eq!(
        sim.shard.position,
        Point::new(1003, 1000, 0),
        "the shard stopped it at the wall"
    );
    let (_, last) = *sim.trace.last().unwrap();
    assert!(
        (last.0 - 1003.0).abs() < 0.01 && (last.1 - 1000.0).abs() < 0.01,
        "the screen agrees with the shard: {last:?}"
    );
    // A held arrow against a wall keeps asking, so the body keeps leaning into
    // the tile it cannot have and keeps being put back — that is the player's
    // own doing and it is what the 2D client does too. The two things that must
    // not happen are that it is drawn *past* the wall, and that the way back is
    // *walked*: a rollback is a snap, and gliding into it would draw the
    // character strolling backwards a whole tile. Backwards in one frame is the
    // snap; backwards in two consecutive frames is a glide.
    let mut back_to_back = 0;
    for pair in sim.trace.windows(2) {
        let ((_, before), (when, after)) = (pair[0], pair[1]);
        let moved = after.0 - before.0;
        assert!(moved > -1.01, "jumped a whole tile backwards at {when:?}");
        back_to_back = match moved < -0.001 {
            true => back_to_back + 1,
            false => 0,
        };
        assert!(back_to_back < 2, "walked backwards at {when:?}");
    }
    assert!(
        sim.trace.iter().all(|(_, drawn)| drawn.0 <= 1004.0),
        "drawn past the wall"
    );
}

// --- The queue rule --------------------------------------------------------
//
// Two complaints, one rule. Walking east and pressing west mid-stride jumped the
// camera, and mashing the arrows sent the body flying off its own position and
// being dragged back. Both are the same defect: an input was allowed to take a
// step *now*, whenever it arrived, cutting short the step already being walked
// and paying nothing into the pace budget for the privilege.
//
// The rule, and the thing these scenarios assert: an input goes into the queue
// or rebuilds it, and a step already begun ticks out. See `docs/client.md`.

/// The reversal, exactly as reported: walking east, west pressed halfway
/// through the second step.
///
/// What must happen is one smooth step east and then one smooth step west: the
/// step under way finishes on its own tile, and the reversal leaves at the
/// deadline that step always had. What must not happen — and what did — is the
/// body being yanked to the tile it had not reached yet so that the new step can
/// start from there, which moves it half a tile in one frame and takes the
/// camera with it.
#[test]
fn a_reversal_lets_the_step_under_way_finish() {
    let script = vec![
        press(0, Direction::East),
        press(600, Direction::West),
        release(2_400, Direction::West),
    ];
    let until = Duration::from_millis(2_400);
    let oracle = Oracle::build(START, &script, until);
    let net = Net {
        latency: Duration::from_millis(120),
        jitter: Duration::from_millis(30),
        wake_jitter: Duration::ZERO,
    };
    let mut sim = Sim::new(Direction::East, net, 2, Vec::new());
    sim.run(&script, until);

    tracks(&sim, &oracle, 0.02);
    continuous(&sim, WALK_HOLD);
    paced(&sim, WALK_HOLD);
    assert_eq!(
        sim.refused, 0,
        "a reversal at the walking rate is not a speedhack"
    );
}

/// The same reversal, back and forth, over and over — which is what a player
/// does when they are testing the feel of it, and what produced the complaint.
///
/// Every press lands at a different phase of the step it interrupts, so this is
/// the phase sweep of the scenario above: a rule that only holds when the press
/// happens to arrive near a tile boundary fails here.
#[test]
fn walking_back_and_forth_never_jumps_the_camera() {
    let mut script = Vec::new();
    let mut direction = Direction::East;
    // 270ms: deliberately not a divisor of the 400ms hold, so the presses walk
    // through every phase of a step rather than landing on the same one.
    for tick in 0..20 {
        script.push(press(270 * tick, direction));
        direction = match direction {
            Direction::East => Direction::West,
            _ => Direction::East,
        };
    }
    let until = Duration::from_millis(270 * 20);
    let oracle = Oracle::build(START, &script, until);
    let net = Net {
        latency: Duration::from_millis(90),
        jitter: Duration::from_millis(20),
        wake_jitter: Duration::ZERO,
    };
    let mut sim = Sim::new(Direction::East, net, 4, Vec::new());
    sim.run(&script, until);

    tracks(&sim, &oracle, 0.02);
    continuous(&sim, WALK_HOLD);
    paced(&sim, WALK_HOLD);
    eye_is_the_body(&sim);
    assert_eq!(sim.refused, 0);
}

// --- The camera ------------------------------------------------------------
//
// The eye was `App`'s business until C0 and is `client/render`'s now: a `Rig`
// of parameters, a `Follower` that holds where the eye has got to, and one
// pipeline that every camera this client grows will be a value of. See
// `docs/camera.md`.
//
// What that transplant must not have done is change anything, and the only
// place that can be said honestly is here — the four units the walk is spread
// across, on one clock, with a wire between two of them.

/// C0's gate: the reference camera is still the reference camera.
///
/// Four scenarios rather than one, because the three ways the wiring could be
/// wrong each need a different one to show: a frame that is never advanced
/// needs a body that moves between packets (the glide), a frame that is a
/// frame late needs a body that changes direction (the reversal), and a state
/// that survives when it should not needs a body that is put back somewhere it
/// never walked (the rollback).
#[test]
fn the_reference_rig_puts_the_eye_on_the_body_every_frame() {
    let perfect = Net::default();
    let real = Net {
        latency: Duration::from_millis(90),
        jitter: Duration::from_millis(20),
        wake_jitter: Duration::from_millis(9),
    };

    // Ten steps east on a perfect wire, and the same over a real one.
    for (net, seed) in [(perfect, 1), (real, 7)] {
        let mut sim = Sim::new(Direction::East, net, seed, Vec::new());
        sim.run(&ten_steps_east(), Duration::from_millis(4_000));
        eye_is_the_body(&sim);
    }

    // A wall three tiles along: the shard refuses, and the body is put back on
    // a tile it never walked to. The eye goes with it — it is the reference
    // camera, and relaying a rollback whole is exactly what it does.
    let mut refused = Sim::new(Direction::East, real, 11, vec![Tile::new(1004, 1000)]);
    refused.run(&ten_steps_east(), Duration::from_millis(4_000));
    assert!(refused.refused > 0, "the wall is on the way");
    eye_is_the_body(&refused);

    // And a reversal every 270ms, which lands a press at every phase of a step.
    let mut script = Vec::new();
    let mut direction = Direction::East;
    for tick in 0..20 {
        script.push(press(270 * tick, direction));
        direction = match direction {
            Direction::East => Direction::West,
            _ => Direction::East,
        };
    }
    let mut kiting = Sim::new(Direction::East, real, 4, Vec::new());
    kiting.run(&script, Duration::from_millis(270 * 20));
    eye_is_the_body(&kiting);
}

/// The bench's synthetic walk is the walk this client actually does.
///
/// The bench flies a rig over a scripted body with no wire, no prediction and
/// no shard behind it — which is what makes it fast enough to sweep, and what
/// would make a rig fitted to it worthless if the script were not the real
/// kinematics. So the two are held against each other on the one scenario they
/// share: ten steps east, the same pace, the same frame interval, the reference
/// rig. If the synthetic body ever stops moving the way the real one does, this
/// is what says so — and it fails long before anybody notices that a rig tuned
/// on the bench feels wrong in the window.
#[test]
fn the_benchs_synthetic_walk_is_the_walk_this_client_does() {
    let mut sim = Sim::new(Direction::East, Net::default(), 5, Vec::new());
    sim.run(&ten_steps_east(), Duration::from_millis(4_000));
    let real = Metrics::of(&sim.eyes);

    let script = openshard_client_render::bench::scripts()
        .into_iter()
        .find(|script| script.name == "ten_east")
        .expect("the baseline walk is one of the scripts");
    let synthetic = Metrics::of(
        &openshard_client_render::bench::run(Rig::HARD, &script, Cadence::steady(GLIDE_INTERVAL)).samples,
    );

    // How fast the eye moves is the whole of the kinematics: a tile in a hold,
    // at a constant speed, whatever produced the steps. Five per cent, because
    // the real loop wakes on a grid and a step's glide is measured from the gap
    // between two wakes rather than from the nominal hold.
    let apart = (real.speed_max - synthetic.speed_max).abs() / synthetic.speed_max;
    assert!(
        apart < 0.05,
        "the real walk peaks at {:.1} px/s and the scripted one at {:.1}",
        real.speed_max,
        synthetic.speed_max,
    );
    // And both were really asked to walk.
    assert!(real.travel > 300.0 && synthetic.travel > 300.0);
}

/// The height reaches the camera as its own number, which is what a rig that
/// smooths a stair away will filter and what a projected pixel cannot say.
///
/// Held here rather than in `client/render` because the value under test is the
/// one that actually arrives — through the crowd, the glide and the packet that
/// moved the body — and not one a unit test wrote by hand.
#[test]
fn the_camera_is_told_the_height_apart_from_the_ground() {
    let mut sim = Sim::new(Direction::East, Net::default(), 3, Vec::new());
    sim.run(&ten_steps_east(), Duration::from_millis(1_200));
    // The field is flat, so every frame's lift is zero and the ground is the
    // whole of the gaze — which is the case that would pass just as well if the
    // two were still one number, so the assertion is the other way round: the
    // eye is the plane, exactly, with nothing of `z` folded into it.
    let gaze = mobiles::gaze(&sim.player);
    assert_eq!(gaze.lift, 0.0, "a flat field lifts nothing");
    assert_eq!(gaze.eye().pixel().y, gaze.y.round() as i32);

    // And a body standing twenty units up: the ground is unchanged and the lift
    // is the whole of the difference.
    let standing = Point::new(sim.player.at.x, sim.player.at.y, 20);
    let raised = Mobile {
        at: standing,
        drawn: openshard_client_render::follow::Gaze::on(standing),
        ..sim.player
    };
    let up = mobiles::gaze(&raised);
    assert_eq!((up.x, up.y), (gaze.x, gaze.y), "the ground did not move");
    assert_eq!(up.lift, 80.0, "twenty units, four pixels each");
    assert_eq!(up.eye().pixel().y, gaze.eye().pixel().y - 80);
}

/// The mash: the arrows hammered at thirty presses a second, which is faster
/// than any walk and is what a player does to a client they suspect.
///
/// The oracle has nothing to say about which tile the body ends on — that
/// depends on which way the last press before each deadline pointed — so what is
/// asserted is what the complaint was: the body never outruns a walk, never
/// jumps, and is never refused. A refusal here would be the shard's pace budget
/// catching the client asking for more steps than a body can take, and the
/// rollback that follows it is the "flying away and being dragged back" that was
/// reported.
#[test]
fn mashing_the_arrows_never_outruns_a_walk() {
    let mut script = Vec::new();
    let mut direction = Direction::East;
    for tick in 0..90 {
        script.push(press(33 * tick, direction));
        direction = match direction {
            Direction::East => Direction::SouthEast,
            Direction::SouthEast => Direction::South,
            _ => Direction::East,
        };
    }
    let until = Duration::from_millis(4_000);
    let net = Net {
        latency: Duration::from_millis(100),
        jitter: Duration::from_millis(40),
        wake_jitter: Duration::ZERO,
    };
    for seed in 0..4 {
        let mut sim = Sim::new(Direction::East, net, seed, Vec::new());
        sim.run(&script, until);

        continuous(&sim, WALK_HOLD);
        paced(&sim, WALK_HOLD);
        assert_eq!(
            sim.refused, 0,
            "seed {seed}: the client asked for more steps than a body can take"
        );
        // Ten holds in four seconds, and a body covers one tile in each. The
        // Chebyshev distance is the map's own — a diagonal is one step — so a
        // walk that stayed inside its cadence cannot be further than that from
        // where it started, however the presses were ordered.
        let travelled = i64::from(sim.shard.position.x)
            .abs_diff(i64::from(START.x))
            .max(i64::from(sim.shard.position.y).abs_diff(i64::from(START.y)));
        assert!(
            travelled <= 10,
            "seed {seed}: {travelled} tiles in four seconds, which is more than ten holds of walking"
        );
    }
}

/// One arrow tapped rather than held: press, release, press, release, faster
/// than the walk.
///
/// A separate scenario because a release used to *disarm the clock*, so the tap
/// after it was treated as the first ask of a fresh walk and left at once — the
/// rate floor was there and a player could step over it by letting go of the
/// key. The floor has to outlive the release; what it must not do is outlive the
/// walk itself, so the last assertion is that a body which stops for a second
/// sets off immediately when the arrow goes down again.
#[test]
fn tapping_one_arrow_is_not_a_step_per_tap() {
    let mut script = Vec::new();
    for tick in 0..24 {
        script.push(press(120 * tick, Direction::East));
        script.push(release(120 * tick + 60, Direction::East));
    }
    let until = Duration::from_millis(4_000);
    let mut sim = Sim::new(Direction::East, Net::default(), 6, Vec::new());
    sim.run(&script, until);

    continuous(&sim, WALK_HOLD);
    paced(&sim, WALK_HOLD);
    assert_eq!(sim.refused, 0, "a tapped arrow is not a speedhack either");

    // And the floor is a floor and not a lockout: a walk that has genuinely
    // stopped leaves on the next press, in the same millisecond.
    let mut sim = Sim::new(Direction::East, Net::default(), 6, Vec::new());
    let script = vec![
        press(0, Direction::East),
        release(60, Direction::East),
        press(2_000, Direction::East),
        release(2_060, Direction::East),
    ];
    sim.run(&script, Duration::from_millis(2_400));
    assert_eq!(
        sim.stepped_at.len(),
        2,
        "two presses two seconds apart are two steps"
    );
    assert_eq!(
        sim.stepped_at[1],
        Duration::from_millis(2_000),
        "and the second is not held back"
    );
}

/// The wire's half of the rollback, and it used to close the window: a refusal
/// voids the steps still in flight, the shard answers those steps anyway, and
/// their answers name sequences this end has forgotten.
///
/// Latency is what makes it appear — a third of a second is two steps in flight
/// when the wall is hit, so two answers arrive with nothing left to match — and
/// `Sim::deliver` unwraps `Walk::on_packet` exactly as `link.rs` used to treat
/// it as fatal, so a regression here is a panic and not a soft assertion. What
/// the scenario asserts beyond surviving is that the walk *recovers*: the body
/// ends where the shard says, having kept asking the whole way.
#[test]
fn the_answers_a_rollback_left_on_the_wire_do_not_end_the_session() {
    // Into the wall, and then away from it — which is what makes the race
    // *fatal* rather than merely ugly. Leaning on the wall produces refusals and
    // nothing else, and a refusal for a step already voided is only a second
    // rollback. Turning away means the step sent after the first rollback is one
    // the shard *allows*, so its `0x22` comes back behind the refusals still on
    // the wire — and that ack, arriving with nothing left to match, is what
    // `link.rs` used to close the window over.
    let script = vec![
        press(0, Direction::East),
        // The arrows are a stack (`keys.rs`), so east is let go of as north is
        // taken: leaving it down would resume the walk into the wall the moment
        // north came up, which is correct behaviour and not this scenario.
        release(2_000, Direction::East),
        press(2_000, Direction::North),
        release(4_000, Direction::North),
    ];
    // Well past the last input: a round trip is 1.4 seconds here, so a run that
    // stopped at the release would end with steps still in flight and the drawn
    // body legitimately ahead of the shard — which is what predicting *is*, and
    // not something to assert against.
    let until = Duration::from_millis(6_500);
    let net = Net {
        latency: Duration::from_millis(700),
        jitter: Duration::from_millis(60),
        wake_jitter: Duration::ZERO,
    };
    for seed in 0..6 {
        let mut sim = Sim::new(Direction::East, net, seed, vec![Tile::new(1003, 1000)]);
        sim.run(&script, until);

        assert!(sim.refused > 0, "seed {seed}: the wall is on the way");
        assert_eq!(
            sim.shard.position.x, 1002,
            "seed {seed}: the wall stopped it a tile short and it never got past"
        );
        // How far past the wall the body is drawn before the refusal arrives is
        // the prediction's business, not this scenario's — nothing here predicts
        // walkability, which is its own backlog item. What matters is where it
        // ends up.
        //
        // Where the shard says, and this is the assertion the stale answers used
        // to break in the other direction: a rollback applied twice puts the body
        // back on a tile it had already walked away from.
        let (_, last) = *sim.trace.last().unwrap();
        assert!(
            (last.0 - f64::from(sim.shard.position.x)).abs() < 0.01
                && (last.1 - f64::from(sim.shard.position.y)).abs() < 0.01,
            "seed {seed}: the screen says {last:?} and the shard says {:?}",
            sim.shard.position
        );
        assert!(
            sim.not_sent.is_empty(),
            "seed {seed}: a shard that is answering never backs the client up: {:?}",
            sim.not_sent
        );
    }
}

/// A shard that stops answering stops the walk five steps later, and the body
/// waits rather than running off on its own.
///
/// The other half of the same debt. Without a cap the prediction walks for as
/// long as the outage lasts and the correction when the link comes back is that
/// whole distance — a body sliding backwards across half a screen, which is the
/// worst picture in this file. The reference caps it at five
/// (`Constants.MAX_STEP_COUNT`) and so do we.
#[test]
fn a_shard_that_goes_quiet_stops_the_body_rather_than_the_prediction() {
    let script = vec![press(0, Direction::East), release(8_000, Direction::East)];
    let until = Duration::from_millis(8_000);
    // A wire that swallows everything: the `0x02`s arrive an hour from now, so
    // nothing is ever answered.
    let net = Net {
        latency: Duration::from_secs(3_600),
        jitter: Duration::ZERO,
        wake_jitter: Duration::ZERO,
    };
    let mut sim = Sim::new(Direction::East, net, 12, Vec::new());
    sim.run(&script, until);

    assert_eq!(
        sim.walk.in_flight(),
        openshard_client_net::walk::MAX_IN_FLIGHT,
        "five steps went out and none was answered"
    );
    assert!(
        !sim.not_sent.is_empty(),
        "and the sixth was refused by this end rather than sent"
    );
    // Twenty steps' worth of asking, five tiles of walking. The drawn body is
    // one of those five and not twenty, which is the whole point.
    let (_, last) = *sim.trace.last().unwrap();
    assert!(
        (last.0 - 1005.0).abs() < 0.01,
        "the body walked {last:?}, which is past the cap"
    );
    continuous(&sim, WALK_HOLD);
}

// --- The instrument --------------------------------------------------------

/// One frame of a walk, in the numbers a curve is drawn from.
///
/// Both bodies and both derivatives in one row on purpose: the complaint the
/// dump exists for is "the camera jerks", and the only way to answer it is to
/// see whether the body under the camera jerked first.
#[derive(Clone, Copy, Debug)]
struct WalkFrame {
    /// When, from the press.
    at: Duration,
    /// Where the oracle says the body should be, in world pixels.
    want: (f64, f64),
    /// Where it was drawn, in world pixels — the sprite's own gaze.
    body: (f64, f64),
    /// Where the eye was put, unrounded.
    eye: (f64, f64),
}

/// Every frame of a run, against the oracle it is held to.
fn walk_frames(sim: &Sim, oracle: &Oracle) -> Vec<WalkFrame> {
    sim.eyes
        .iter()
        .map(|sample| {
            let want = oracle.at(sample.at);
            WalkFrame {
                at: sample.at,
                want: tile_pixels(want),
                body: sample.gaze.exact(),
                eye: sample.state.exact(),
            }
        })
        .collect()
}

/// A fractional tile, projected the way `camera::project` projects a whole one.
///
/// The oracle speaks in tiles and everything else here in world pixels, and the
/// comparison has to happen in one of the two.
fn tile_pixels(tile: (f64, f64)) -> (f64, f64) {
    let half = f64::from(TILE_WIDTH) / 2.0;
    ((tile.0 - tile.1) * half, (tile.0 + tile.1) * half)
}

/// And back: which fractional tile a gaze's ground position falls on.
///
/// The inverse of [`tile_pixels`] and exact — the projection is a linear map
/// with a determinant of half a tile squared, so nothing is lost either way.
/// `camera::unproject` is the same inverse rounded to a whole tile, which is
/// what picking wants and what a trace must not do: rounding the body to a tile
/// is exactly the teleport the glide exists to remove.
///
/// The *ground* position, so `lift` is not read: a body raised by its height has
/// not moved along the map, and folding the two would put a walk up a stair in
/// the wrong tile.
fn tiles_of(gaze: openshard_client_render::follow::Gaze) -> (f64, f64) {
    (
        (gaze.x + gaze.y) / f64::from(TILE_WIDTH),
        (gaze.y - gaze.x) / f64::from(TILE_HEIGHT),
    )
}

/// The speed between consecutive frames, in world pixels a second.
fn speeds(frames: &[WalkFrame], of: fn(&WalkFrame) -> (f64, f64)) -> Vec<(Duration, f64)> {
    frames
        .windows(2)
        .filter_map(|pair| {
            let dt = (pair[1].at - pair[0].at).as_secs_f64();
            if dt <= 0.0 {
                return None;
            }
            let (was, now) = (of(&pair[0]), of(&pair[1]));
            Some((pair[1].at, (now.0 - was.0).hypot(now.1 - was.1) / dt))
        })
        .collect()
}

/// A run of the ten-step walk under one wire and one rig, for the dumps below.
fn walked(rig: Rig, ease: Ease, net: Net, seed: u64) -> (Sim, Oracle) {
    let script = ten_steps_east();
    let until = Duration::from_millis(4_400);
    let oracle = Oracle::build(START, &script, until);
    let mut sim = Sim::flying(rig, ease, Direction::East, net, seed, Vec::new());
    sim.run(&script, until);
    (sim, oracle)
}

/// A wire with a quiet desktop's wake jitter on it and a plausible shard behind
/// it. What the dumps look at, because a perfect one has nothing to show.
const LIVE: Net = Net {
    latency: Duration::from_millis(60),
    jitter: Duration::from_millis(20),
    wake_jitter: Duration::from_millis(8),
};

/// A run, drawn: where the drawn body and the eye were against where the
/// oracle says they should have been, and how fast each was going.
///
/// Two panels rather than one, and the first is a *deviation* rather than a
/// position: ten steps east is 220 pixels of ramp, and a chart of that ramp
/// hides the one-pixel discontinuities the whole complaint is made of. The
/// second is the speed, where the oracle is a flat line and every departure
/// from it is a frame the world moved at the wrong rate.
fn chart_of(name: &str, frames: &[WalkFrame]) -> String {
    let offset = |of: fn(&WalkFrame) -> (f64, f64)| chart::Series {
        name: String::new(),
        points: frames
            .iter()
            .map(|frame| {
                let (x, y) = of(frame);
                (frame.at.as_secs_f64(), (x - frame.want.0).hypot(y - frame.want.1))
            })
            .collect(),
    };
    let speed = |of: fn(&WalkFrame) -> (f64, f64)| chart::Series {
        name: String::new(),
        points: speeds(frames, of)
            .into_iter()
            .map(|(at, speed)| (at.as_secs_f64(), speed))
            .collect(),
    };
    let named = |series: chart::Series, called: &str| chart::Series {
        name: called.to_string(),
        ..series
    };
    // What the oracle walks at: one tile per hold, and a tile is 44 pixels
    // across its diagonal — `sqrt(22² + 22²)` per `WALK_HOLD`.
    let nominal = 22.0f64.hypot(22.0) / WALK_HOLD.as_secs_f64();
    let panels = vec![
        chart::Panel {
            title: "how far from where the oracle says, pixels".to_string(),
            series: vec![
                named(offset(|frame| frame.body), "body"),
                named(offset(|frame| frame.eye), "eye"),
            ],
            baseline: Some(0.0),
        },
        chart::Panel {
            title: "speed, pixels per second".to_string(),
            series: vec![
                named(speed(|frame| frame.body), "body"),
                named(speed(|frame| frame.eye), "eye"),
            ],
            baseline: Some(nominal),
        },
    ];
    let seconds = frames.last().map_or(1.0, |frame| frame.at.as_secs_f64());
    chart::svg(&format!("ten steps east — {name}"), seconds, &panels)
}

/// Where a dump goes: `OPENSHARD_CAMERA_DUMP`, or a directory of our own under
/// the system temp.
///
/// Never the source tree — this writes a file per wire and none of them belongs
/// in a diff. A unit test is handed no pointer to `target/`, which is why this
/// is not the integration test's `CARGO_TARGET_TMPDIR` trick.
fn dump_dir() -> std::path::PathBuf {
    std::env::var_os("OPENSHARD_CAMERA_DUMP")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("openshard-camera"))
}

/// Ten steps east, measured rather than asserted: what the body's speed and the
/// eye's speed actually do, frame by frame.
///
/// Asserts nothing and is ignored for it. The oracle says both should be flat at
/// one tile per hold from the press to the last step, and every departure from
/// flat is a jerk somebody can see.
#[test]
#[ignore = "writes a table and charts for a person, and asserts nothing"]
fn dump_the_walk() {
    let wires = [
        ("perfect", Net::default()),
        (
            "wake_8ms",
            Net {
                latency: Duration::ZERO,
                jitter: Duration::ZERO,
                wake_jitter: Duration::from_millis(8),
            },
        ),
        (
            "live",
            Net {
                latency: Duration::from_millis(60),
                jitter: Duration::from_millis(20),
                wake_jitter: Duration::from_millis(8),
            },
        ),
    ];
    let dir = dump_dir();
    std::fs::create_dir_all(&dir).expect("a directory to write into");
    println!(
        "\n{:<10} {:>6} {:>9} {:>8} {:>8} {:>9}",
        "wire", "frames", "mean px/s", "min", "max", "worst px"
    );
    for (name, net) in wires {
        let (sim, oracle) = walked(Rig::HARD, Ease::NONE, net, 3);
        let frames = walk_frames(&sim, &oracle);
        let body = speeds(&frames, |frame| frame.body);
        // The walk itself, without the stand at either end: the first frame has
        // nothing behind it and the last four are a body that has arrived, and
        // averaging those in is how a still scene reports as a smooth one.
        let walking: Vec<f64> = body
            .iter()
            .filter(|(at, _)| *at > Duration::from_millis(100) && *at < Duration::from_millis(3_900))
            .map(|(_, speed)| *speed)
            .collect();
        let mean = walking.iter().sum::<f64>() / walking.len() as f64;
        let worst = frames.iter().fold(0.0f64, |worst, frame| {
            worst.max((frame.body.0 - frame.want.0).hypot(frame.body.1 - frame.want.1))
        });
        println!(
            "{name:<10} {:>6} {mean:>9.1} {:>8.1} {:>8.1} {worst:>9.2}",
            frames.len(),
            walking.iter().copied().fold(f64::INFINITY, f64::min),
            walking.iter().copied().fold(0.0f64, f64::max),
        );
        std::fs::write(dir.join(format!("walk-{name}.svg")), chart_of(name, &frames))
            .expect("writing a chart");
        // The same frames as numbers. The picture is what a shape is read from
        // and the table is what a millisecond is read from, and every finding so
        // far has needed both.
        let mut csv = String::from("at_us,want_x,want_y,body_x,body_y,eye_x,eye_y\n");
        for frame in &frames {
            csv.push_str(&format!(
                "{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}\n",
                frame.at.as_micros(),
                frame.want.0,
                frame.want.1,
                frame.body.0,
                frame.body.1,
                frame.eye.0,
                frame.eye.1,
            ));
        }
        std::fs::write(dir.join(format!("walk-{name}.csv")), csv).expect("writing a table");
    }
    println!("\nwrote {}", dir.display());
}

/// What a filter does to the start and the stop of a real walk, rig by rig.
///
/// `docs/camera.md` C3 is the milestone this is the instrument for, and D9 is
/// why it is a dump and not a preset: no camera is chosen until one has been
/// looked at. The reference rig is in the table as the row with no ramp at all —
/// under `HARD` the eye *is* the body, so its start is the body's, which is
/// instantaneous by construction and always will be. A body crosses one tile per
/// hold at a constant speed because that is what the wire says it does; the only
/// thing that can ease into a walk is the eye.
///
/// The two numbers a time constant is chosen between are printed side by side,
/// because picking on either alone picks wrong. **Ramp** is how long the eye
/// takes to reach nine tenths of the walk's speed — the ease-in somebody asked
/// for. **Lag** is how far behind the body the eye then sits for the whole of
/// the walk, which is that same constant times the speed and is the price:
/// the character walks off centre and stays there until it stops.
#[test]
#[ignore = "writes a table and charts for a person, and asserts nothing"]
fn dump_the_ramp() {
    let rigs = [
        ("none", Rig::HARD, Ease::NONE),
        ("body_0.08", Rig::HARD, Ease::WALK),
        ("body_0.15", Rig::HARD, Ease { tau: 0.15 }),
        ("eye_0.08", plane(0.08), Ease::NONE),
    ];
    let dir = dump_dir();
    std::fs::create_dir_all(&dir).expect("a directory to write into");
    println!(
        "\n{:<10} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "rig", "ramp ms", "slide px", "trail px", "stop ms", "peak px/s"
    );
    let mut series = Vec::new();
    for (name, rig, ease) in rigs {
        let (sim, oracle) = walked(rig, ease, LIVE, 3);
        let frames = walk_frames(&sim, &oracle);
        let speed = speeds(&frames, |frame| frame.eye);
        // The walk's own speed, which every rig is ramping towards: a tile's
        // diagonal per hold.
        let nominal = 22.0f64.hypot(22.0) / WALK_HOLD.as_secs_f64();
        let ramp = speed
            .iter()
            .find(|(_, at)| *at >= nominal * 0.9)
            .map_or(Duration::MAX, |(at, _)| *at);
        // And how long after the last step the eye is still moving. The stop is
        // the half of the shape a ramp-in metric cannot see.
        let walked_until = Duration::from_millis(4_000);
        let stop = speed
            .iter()
            .filter(|(at, _)| *at > walked_until)
            .filter(|(_, at)| *at > 1.0)
            .map(|(at, _)| *at)
            .next_back()
            .map_or(Duration::ZERO, |at| at.saturating_sub(walked_until));
        let lag = frames
            .iter()
            .filter(|frame| frame.at > Duration::from_millis(1_000) && frame.at < walked_until)
            .fold(0.0f64, |worst, frame| {
                worst.max((frame.eye.0 - frame.body.0).hypot(frame.eye.1 - frame.body.1))
            });
        // The two lags, and they are the point of the table. **slide** is the eye
        // against the sprite — how far the character drifts from where it is
        // drawn, which is what the player sees as the body sliding around the
        // screen. **trail** is the sprite against the walk the oracle says it is
        // doing, which is where an eased body's lag goes instead: invisible,
        // because nothing on screen marks the tile.
        let trail = frames
            .iter()
            .filter(|frame| frame.at > Duration::from_millis(1_000) && frame.at < walked_until)
            .fold(0.0f64, |worst, frame| {
                worst.max((frame.body.0 - frame.want.0).hypot(frame.body.1 - frame.want.1))
            });
        println!(
            "{name:<10} {:>9} {lag:>9.1} {trail:>9.1} {:>9} {:>9.1}",
            ramp.as_millis(),
            stop.as_millis(),
            speed.iter().map(|(_, at)| *at).fold(0.0f64, f64::max),
        );
        series.push(chart::Series {
            name: name.to_string(),
            points: speed
                .into_iter()
                .map(|(at, speed)| (at.as_secs_f64(), speed))
                .collect(),
        });
        std::fs::write(dir.join(format!("ramp-{name}.svg")), chart_of(name, &frames))
            .expect("writing a chart");
    }
    // And every rig on one axis, which is the picture the choice is made from.
    let nominal = 22.0f64.hypot(22.0) / WALK_HOLD.as_secs_f64();
    let panels = vec![chart::Panel {
        title: "the eye's speed into and out of a ten-tile walk, pixels per second".to_string(),
        series,
        baseline: Some(nominal),
    }];
    std::fs::write(dir.join("ramp.svg"), chart::svg("plane_tau", 4.4, &panels))
        .expect("writing the comparison");
    println!("\nwrote {}", dir.display());
}

/// A rig that filters the ground plane and nothing else.
///
/// The height is left at the reference's zero deliberately: a climbed stair
/// arrives through the glide already spread over its step, and `Rig::LIFT`'s own
/// note is that filtering it again makes it marginally worse. What is under the
/// eye here is the walk.
fn plane(tau: f32) -> Rig {
    Rig {
        plane_tau: tau,
        ..Rig::HARD
    }
}
