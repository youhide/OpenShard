//! Where the player is asking to walk, and how often the ask may be sent.
//!
//! # Three idioms, one clock
//!
//! A held arrow says *which way* ([`Steering::press`]/[`release`](Steering::release)).
//! A held right button, with no modifier, says the same thing from the mouse —
//! *which way*, not *where to*: [`Steering::steer`] takes the compass direction
//! from the body to the cursor, recomputed every move, and drives it exactly
//! like a held arrow. How far from the body it is held is a question of its
//! own: close in, the same button asks the body to *face* that way and go
//! nowhere ([`Ask::Turn`], the classic client's ring — the only way a mouse can
//! turn a character on the spot). A held right button *with* Ctrl says *where to*: the
//! strategy game's move order, [`Steering::go_to`], a destination tile planned
//! and walked to on its own.
//!
//! These used to be one idiom stated twice — a drag restating a destination
//! every move looks the same as a heading restated every move, right up until
//! the cursor drags across something the plan cannot stand on. A destination
//! answers that by refusing to move at all (see the next section); a heading
//! has no destination to refuse, so it just keeps trying the way it is pointed
//! and discovers a wall exactly the way an arrow key does. Casual "run toward
//! the cursor" wants the second: it is a heading, not a survey of the ground
//! between here and there, and it is what a player reaching for the mouse
//! instead of a hand full of arrow keys actually means. Ctrl is the escape
//! hatch back to a real order, for the case that *does* want a plan — sending a
//! body around the far side of a building it cannot see from here.
//!
//! All three answer the same question a step at a time, so they are answered in
//! one place: whichever is asking, a step leaves every [`WALK_HOLD`] (or every
//! [`RUN_HOLD`] with shift down, which is what running is), and never two
//! at once. Separate timers, one per input, would take two steps a beat the
//! moment a player nudged an arrow while walking to a destination.
//!
//! The keyboard outranks the mouse, and either mouse mode outranks the other —
//! see [`Steering::asking`] for the order — and taking hold of a higher one
//! *drops* whatever a lower one was doing rather than queuing behind it: the
//! arrows going down is how a player says they no longer want to go where they
//! pointed or clicked.
//!
//! # The rate is the step's own length, not the anti-speedhack floor
//!
//! `common/movement`'s intervals are floors, deliberately half the real rate so
//! that jitter never trips the check (see `pace.rs`). Walking *at* the floor
//! would be moving twice as fast as a body moves, and the crowd — which glides a
//! step over its own length — would have a walker arrive half a tile before the
//! next step and stand there. The hold in `crowd` is already that real length,
//! for exactly this reason, so it is what is read here rather than a second
//! number that could disagree with it.
//!
//! # The first ask steps at once, and every other one waits its turn
//!
//! Waiting a whole step before the first one would put 400ms between the input
//! and the character. So a press that changes the direction, and a click that
//! names a new destination, are due immediately — *if the body is standing*. If
//! it is not, they are not.
//!
//! That second half is the queue rule, and it is the whole of this module's
//! contract with the player's eye:
//!
//! **An input joins the queue or rebuilds it. A step already begun ticks out.**
//!
//! A turn is the one step that is not a hold long. A body asked for a direction
//! it is not facing turns and covers no ground, which the shard takes without
//! charging its pace budget at all — so what the step behind a turn waits for is
//! this end's decision, and it is [`Turning`]'s. The default is the reference
//! client's: [`TURN_HOLD`], ClassicUO's `MovementSpeed.TurnDelay`. A click
//! sideways therefore squares the body up first and sets off a beat later, which
//! is the movement people remember; [`Turning::Immediate`] is the other answer,
//! where the pair leaves in one wake.
//!
//! An input never moves the deadline earlier. It changes which way the step the
//! walk already owes will go — [`Steering::take`] reads the keys at the moment
//! the step leaves and not at the moment they were pressed, so the queue is one
//! deep and is rebuilt by every press for free — and that step leaves when it
//! was always going to. The reasons are three, and each of them was a complaint:
//!
//! - **The picture.** The body is drawn crossing the tile its last step asked
//!   for, and `crowd.rs` starts each glide from the tile the previous step
//!   *ended* on. A step issued halfway through the last one therefore yanks the
//!   body to a tile it has not reached, which is half a tile in one frame — and
//!   the camera is locked to the body, so the whole world jumps with it. Pressing
//!   the opposite arrow mid-stride is how a player finds this in one second.
//! - **The pace.** The shard's `WalkPace` refuses a body that asks for steps
//!   faster than a body walks, and answers with a `0x21` that puts it back where
//!   it really is. A client that sent a step per keypress hands a key-masher a
//!   burst of steps, a rollback, and a body that flies off and is dragged back.
//! - **The wire.** The rollback races the steps still in flight, and their acks
//!   arrive for a sequence this end has already forgotten — which
//!   `client/net`'s [`Walk`](openshard_client_net::walk::Walk) reports as a
//!   desync it cannot repair. Not asking for those steps is what stops it.
//!
//! The rate floor is a floor and not a lockout: it *outlives the release*, so
//! letting go of the arrow and pressing it again does not buy a step, and it is
//! only ever a floor, so a walk that genuinely stopped sets off the instant the
//! arrow next goes down.
//!
//! # A heading never gives up; a destination does
//!
//! [`Steering::steer`] has no notion of arrival or of being stuck: it is a
//! direction, and a direction is either being asked for or it is not. It never
//! gives up on it and it never plans a route to it — that is still the
//! destination's alone, and a heading never touches `find_path` or a map. What
//! it *does* do, the one exception, is ask [`Detour`]: when the tile the held
//! direction leads to is blocked, that answers with the nearest direction still
//! legal past it — a cardinal step along a wall's face when the wall is dead
//! ahead, or the cardinal a blocked diagonal splits into when it is a corner
//! instead. An O(1) look at four tiles a body already has to check before
//! sending a step, not a search, and always a candidate the server's own corner
//! rule accepts. That is a runner sliding past an obstacle the way a body
//! brushing past furniture would, not a plan.
//!
//! The rule is `common/movement`'s and not this module's, and its own docs are
//! where the argument lives — which four tiles, why never a diagonal past a
//! wall, and why the flank last taken is remembered. What stays here is the
//! part that is about *input*: that only a held direction is answered this way
//! (a route replans instead), that the very first ask gets it and not only the
//! held retries, and what [`Step::Stuck`] means to a client that has a facing
//! and a clock — the paragraph below.
//!
//! When there is no legal way past at all — the inside corner of a building,
//! pushed at the corner — the heading is still not given up on, but nothing is
//! *sent*. A step this end has already proven the shard refuses is not a
//! harmless one: the answer is a `0x21`, which snaps the body back and resets
//! the walk sequence, a hold at a time, for as long as the player leans on the
//! key. The turn into it is still sent, because a mobile asked for a direction
//! it is not facing turns and moves nowhere and the shard takes that; it is
//! only once the body already faces the corner that there is nothing left to
//! ask for. The clock is armed either way, so the attempt repeats at the
//! walking pace rather than spinning on a deadline already passed, and the
//! walk resumes on its own the moment the door opens.
//!
//! A destination (Ctrl+drag) is answered differently, because it names a
//! specific tile rather than a heading: [`Steering::go_to`] plans a route with
//! `common/movement`'s `find_path`, and the steps taken toward the destination
//! are that route's, one per call.
//!
//! What it plans over is a [`Ground`] — the same map read twice, once with
//! everything the shard has put on it and once without (`clutter.rs`) — and the
//! two halves are asked in that order, which is the whole of how a shut door is
//! answered. The world as it stands answers first, and its route is the plan: a
//! door with a way round is a longer walk and not a barred one. Only when there
//! is no way through at all is the map asked on its own, where nothing the shard
//! placed is in the way and every door therefore stands open — and *that* route
//! is cut at the first step the real ground refuses, so the body walks up to
//! whatever is in the way and stops in front of it. [`plan`] is where both
//! halves live, and the client's own picture of the route is drawn from the same
//! call: what a player sees green up to the door and red past it is the plan
//! itself, not a second opinion about it.
//!
//! **A destination never walks at something this end can see is refused.** Where
//! neither half has a way through — a tile clicked on a wall, on the far bank, or
//! simply too far for the node budget — the plan is how far *toward* it the
//! ground goes ([`find_path_toward`]), and the body stops there. It used to walk
//! at the thing in a straight line instead, a step a hold until a patience ran
//! out, and every one of those steps was a `0x21`: the shard refuses it, rolls
//! the body back, and resets the walk sequence this end is counting. Walking up
//! to something and standing is also what the reference client does with a click
//! on a wall.
//!
//! Neither half can answer for what the shard knows and this end does not — a
//! view a beat out of date, a body standing in the way — so a route found in
//! good faith can still be refused, arriving as a `0x21` (see `client/net`'s
//! `walk`). A refusal — the body did not move where the last step asked —
//! replans from where it actually stands. And a destination that is not getting
//! anywhere after [`STUCK_STEPS`] steps is given up on — the walk standing at a
//! shut door included, which is what ends the order when nobody opens it — while
//! a heading never is; see the section above for why the two differ.
//!
//! The plan itself is lazy: [`Steering::go_to`] is called on the click *and*
//! again on every mouse-move while the button stays down, which is tens of raw
//! events a second while dragging, and `find_path` is an A* search — not
//! something to run that often. So `go_to` only ever restates *where*, never
//! *how*: it drops whatever was planned for the old destination and leaves the
//! new one unplanned, and [`Steering::take`] is the only place a search actually
//! runs, at most once per step, against whichever tile is current when that step
//! comes due.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use openshard_movement::{
    Around, CachedTerrain, Detour, Heading, Lean, Leeway, NavigationGraph, RUN_HOLD, Step, Terrain, Tile,
    WALK_HOLD, find_long_path, find_path, find_path_toward, step_allowed,
};
use openshard_protocol::direction::{Direction, Facing};
use openshard_protocol::world::Point;

use crate::keys::Held;

/// The node budget handed to [`find_path`] for a click-to-walk plan.
///
/// `common/movement`'s own doc calls "a few hundred" ample for a town, and this
/// stays inside that rather than reaching for the map's diagonal: `take` runs a
/// plan at most once per step (see its doc), but an unreachable destination —
/// across an unbroken wall, out over open water — pays the full budget on every
/// one of those, `STUCK_STEPS` times over and again on every re-click. A budget
/// sized for "ample" and not "generous" is what keeps that bounded.
pub const PLAN_BUDGET: usize = 600;

/// A coarse graph is counterproductive for a short failed search: joining an
/// endpoint to every portal in adjacent regions costs more than the local
/// answer, especially around a house with several doors.
const COARSE_MIN_DISTANCE: u32 = 8;

/// How long the step that follows a turn waits: ClassicUO's
/// `Constants.TURN_DELAY`, charged in `PlayerMobile.Walk` as
/// `MovementSpeed.TurnDelay` whenever the direction asked for is not the one
/// the body is already facing.
///
/// A turn is a step of its own in UO, and this is what makes a player *see* it:
/// the click turns the body, and the ground is only covered by the request
/// after it. See [`Turning`].
pub const TURN_HOLD: Duration = Duration::from_millis(80);

/// [`TURN_HOLD`] at ClassicUO's `FastRotation` setting
/// (`Constants.TURN_DELAY_FAST`) — the same behaviour, spun through quicker.
pub const TURN_HOLD_FAST: Duration = Duration::from_millis(45);

/// What the mouse is asking for, which is not always a walk.
///
/// The cursor's *distance* from the body is a question of its own, and the
/// classic client answers it with a ring: close in, the body turns to face the
/// cursor and stands there; further out, it walks that way. A player spinning a
/// character on the spot — to face a door, to face who they are talking to — is
/// using that ring, and a client without one cannot do it with the mouse at all:
/// every ask that changes the facing also sets the body walking.
///
/// So the zone is decided where the pixels are (`App::ask_to_cursor`) and
/// arrives here already resolved. What this module adds is what each one means
/// to the clock and the wire — see [`Steering::take`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ask {
    /// Face this way, and go nowhere: the cursor is inside the turn ring.
    ///
    /// One `0x02` while the body is not already facing that way — a mobile
    /// asked for a direction it is not facing turns and moves nowhere, which
    /// the shard answers with a `0x22` — and nothing at all once it is. The
    /// same shape as a body wedged in a corner, and for the same reason: a step
    /// this end knows will not be taken is not a harmless packet.
    Turn(Heading),
    /// Face this way and keep walking, which is the ordinary held-heading
    /// idiom.
    Walk(Heading),
}

impl Ask {
    /// Which way, whichever the ask is.
    pub const fn heading(self) -> Heading {
        match self {
            Ask::Turn(heading) | Ask::Walk(heading) => heading,
        }
    }
}

/// What a turn costs the walk it precedes.
///
/// A mobile asked for a direction it is not facing turns and covers no ground —
/// the shard answers that with a `0x22` and never touches the pace budget, so
/// *nothing on the wire* forces a gap before the step that follows. What decides
/// whether there is one is this end, and it is the difference between a body
/// that visibly squares up before it sets off and one that pivots and leaves in
/// the same frame. Both are playable; only one of them is what the reference
/// client does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Turning {
    /// ClassicUO's, and the default: the turn is a step of its own and the one
    /// it precedes waits [`TURN_HOLD`] for it.
    #[default]
    Deliberate,
    /// The same, at ClassicUO's `FastRotation` — [`TURN_HOLD_FAST`].
    Fast,
    /// The turn and the step it precedes leave in the same wake: no gap at all.
    ///
    /// Not the reference's behaviour, and kept because it is defensible on its
    /// own terms — a walk that starts on the frame the input arrived — and
    /// because the walk oracle in `dst.rs` is written against it: that harness
    /// is about the *cadence* of a walk under latency and wake jitter, and a
    /// turn tax in front of it would only be a constant it had to model twice.
    Immediate,
}

impl Turning {
    /// How long the step after a turn waits, or `None` when it does not wait at
    /// all.
    const fn hold(self) -> Option<Duration> {
        match self {
            Turning::Deliberate => Some(TURN_HOLD),
            Turning::Fast => Some(TURN_HOLD_FAST),
            Turning::Immediate => None,
        }
    }
}

/// The ground a route is planned over, read twice: as the world stands, and as
/// it would be with every shut door standing open.
///
/// The two differ by exactly one thing — the tiles `clutter.rs` keeps as
/// *potentially passable and currently closed*. A crate is in the way in both
/// readings, because nobody can open a crate; a shut door is in the way in the
/// first and not the second, because a player can walk up to one and open it.
///
/// A step is always decided against [`Ground::real`]. The second half exists so
/// that a plan can say *why* there is no route — the way through is a doorway
/// with a shut leaf in it, rather than a wall — and so a body can be walked up to
/// that leaf instead of at nothing. See [`plan`].
///
/// The server keeps the same pair under the same name: `state::obstruct`'s
/// `Obstacle::door` and `LiveTerrain::through_doors`, for the creature that
/// plans a route it means to open its way along. That both ends draw the line in
/// the same place, from the same door table, is the property worth having.
#[derive(Clone, Copy)]
pub struct Ground<'a> {
    /// What a step is allowed by — the map with everything on it, doors shut.
    pub real: &'a dyn Terrain,
    /// The same ground with every shut door opened. Never what decides a step:
    /// walking on its word is a step the shard refuses.
    pub through_doors: &'a dyn Terrain,
    /// The bare static map the coarse graph was built from. Unlike the two
    /// live readings above it never contains a door, crate or mobile, so those
    /// can reject a proposed corridor without rewriting its topology.
    pub guide: &'a dyn Terrain,
    /// The map-only connectivity cache, absent in mapless/test callers.
    pub coarse: Option<&'a NavigationGraph>,
}

impl Ground<'_> {
    /// Try the ordinary bounded A* first, then use the static coarse graph to
    /// divide a long answer into the same exact, live-aware hops. `terrain` is
    /// chosen by the caller: real ground for a player's open half, or the
    /// existing doors-open reading for the route that is later cut at a leaf.
    fn path(&self, terrain: &CachedTerrain<'_>, from: Point, to: Point) -> Option<Vec<Direction>> {
        let local = find_path(terrain, from, to, PLAN_BUDGET);
        if local.is_some() {
            return local;
        }
        let distance = i32::from(from.x)
            .abs_diff(i32::from(to.x))
            .max(i32::from(from.y).abs_diff(i32::from(to.y)));
        if distance <= COARSE_MIN_DISTANCE {
            return None;
        }
        self.coarse.and_then(|coarse| {
            // Graph and endpoint joins are both the bare map. Live terrain
            // only approves or rejects the resulting exact steps.
            find_long_path(self.guide, terrain, coarse, from, to, PLAN_BUDGET)
        })
    }
}

/// A world with no doors in it, where the two readings are one terrain.
///
/// Test-only, the same way `Clutter::placed` is: every shipping caller has the
/// shard's items laid over the map and must say which reading it is asking for.
/// Not a shortcut for "the other half is not handy here" — the two being equal is
/// a statement about the world, and a caller with a shut door that made it anyway
/// would plan every route as though the door were a wall.
#[cfg(test)]
impl<'a> Ground<'a> {
    pub const fn plain(terrain: &'a dyn Terrain) -> Self {
        Self {
            real: terrain,
            through_doors: terrain,
            guide: terrain,
            coarse: None,
        }
    }
}

/// A route to a destination, in the two halves [`plan`] finds it in.
///
/// Both are steps from the body's own tile, in the order they would be walked:
/// [`Plan::barred`] carries on from where [`Plan::open`] stops.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The part of the way the world as it stands allows. What the walk takes,
    /// and what a picture of the route draws as passable.
    pub open: Vec<Direction>,
    /// What is left of the way past the first thing standing in it — empty for
    /// a route that reaches the destination, which is the ordinary answer.
    ///
    /// Never walked: the first of these steps is one this end has already proven
    /// the shard would refuse. It is a *reason*, and the thing to draw so a
    /// player can see where the way stopped and why.
    pub barred: Vec<Direction>,
    pub(crate) open_points: Vec<Point>,
    pub(crate) barred_points: Vec<Point>,
}

#[derive(Clone, Debug)]
struct CachedPlan {
    from: Point,
    goal: Tile,
    plan: Option<Plan>,
    /// A preview has already performed this frame's query.  The next walk may
    /// consume it once, but successful plans are never retained across live
    /// terrain changes.
    preview: bool,
    /// A failed coarse search is expensive and cannot become more successful
    /// without a terrain update. Keep it from being retried every frame until
    /// the app explicitly invalidates the cache.
    suppress_retry: bool,
}

/// How many steps in a row may leave the body exactly where it was before a walk
/// to a destination gives up.
///
/// More than one, because a step that only turns the body is a legitimate one —
/// UO makes turning a whole step — and because the position this is measured
/// against is the server's last word, which lags whatever is in flight. Four
/// steps is a second and a half of walking on the spot, which is long enough to
/// be sure and short enough that nobody watches it happen twice.
const STUCK_STEPS: u8 = 4;

/// Which way the player is asking to walk, from every input that can ask.
#[derive(Clone, Debug, Default)]
pub struct Steering {
    /// The arrows, and shift.
    keys: Held,
    /// What a held right button (with no modifier) is asking for, recomputed
    /// from the body to the cursor on every move — see [`Steering::steer`].
    /// `None` when the mouse is not steering, which is not the same as
    /// [`Steering::goal`] being absent: this is a direction, not a destination,
    /// and has no "arrived" of its own.
    ///
    /// An [`Ask`] rather than a direction, twice over. The cursor's distance
    /// says whether the body walks or only turns; and its heading says more
    /// than one of eight sectors, since which side of the sector it is on is
    /// what decides a tie between two ways round an obstacle (see
    /// [`Detour::step`]).
    mouse: Option<Ask>,
    /// The tile a Ctrl-held right button last asked for — a destination, not
    /// a heading; see [`Steering::go_to`].
    ///
    /// Absent means absent: nobody has clicked, or the body has arrived. Not a
    /// "no destination yet" — a body standing where it was told to stand has
    /// genuinely nowhere left to go.
    goal: Option<Tile>,
    /// The route planned to [`Steering::goal`], most-recent-plan first.
    ///
    /// Consumed one direction per step; emptied on a refusal and on every
    /// fresh [`Steering::go_to`], which invalidate it without replanning —
    /// [`Steering::take`] is the only place that actually calls [`plan`], and
    /// only when this is empty and `goal` is not.
    ///
    /// Empty *after* that replan is the one state worth naming: there is
    /// nowhere left to walk from here — the body is standing at whatever is in
    /// the way, or as close to the destination as the ground ever gets. Then
    /// nothing is sent at all and the destination's patience is what ends the
    /// order; see [`Steering::take`].
    route: VecDeque<Direction>,
    /// The last complete plan is shared by movement and the route preview.
    /// Both consumers ask the same question for the same world snapshot; do
    /// not make the expensive real/doors-open searches twice.
    cached_plan: Option<CachedPlan>,
    /// The earliest the next step may leave: the deadline of the step in flight.
    ///
    /// The rate floor, and the queue rule's whole mechanism. Armed by every step
    /// that costs time and cleared by *nothing* — not a release, not a lost
    /// focus, not a new destination — because everything that clears it is a way
    /// for a player to step over it. `None` only before the first step of a
    /// session.
    ///
    /// It is not a "the walk is running" flag, which is what it used to be:
    /// whether anything is being asked for is [`Steering::asking_for_anything`],
    /// and that is what decides whether the event loop is woken for it.
    due: Option<Instant>,
    /// Where the body stood when the last step was sent, for [`STUCK_STEPS`].
    was: Option<Point>,
    /// How many steps in a row have left it there.
    stalled: u8,
    /// Whether [`Steering::due`] is the deadline of a walk still under way,
    /// rather than one that has since stopped.
    ///
    /// The two are the same instant and mean opposite things to the cadence. A
    /// deadline a step was taken at is what the *next* one is measured from, so
    /// that a late wake does not push the whole walk back (see
    /// [`Steering::next_due`]). A deadline that came and went with the arrows up
    /// is not a cadence at all — the player pressed again some time later, and
    /// measuring from it would make the step after that one due a fraction of a
    /// hold away, which cuts the glide short and jumps the body.
    walking: bool,
    /// The direction of the last step sent, once one has been.
    ///
    /// Which way the body is *going* to face, which is a step ahead of the way
    /// it is drawn facing: the caller's facing comes back through the shard
    /// thread, and a second step decided from it would turn twice. Absent until
    /// this has asked for anything, and then the caller's facing is the only
    /// answer there is.
    asked: Option<Direction>,
    /// Whether the free turn a direction change buys has been spent since the
    /// clock last actually armed.
    ///
    /// A turn costs no time so the step it precedes can leave in the same
    /// wake — see [`Steering::charge`] — but that is only sound for *one*
    /// direction change per wake, the pattern `about_to_wait`'s "twice at
    /// most" loop enforces for [`Steering::due`]. [`Steering::steer`] and
    /// [`Steering::press`] answer their own immediate ask directly, with no
    /// such ceiling, and a raw `CursorMoved` fires far faster than a hold —
    /// so a heading whose resolved direction keeps changing call to call
    /// (which [`detour`] does while sliding around an obstacle, by design)
    /// found every one of those calls judged a fresh, free turn, since
    /// nothing had armed [`Steering::due`] in between to make
    /// [`Steering::free`] refuse the next one. Two, ten, however many raw
    /// events arrived in the time one real step should have taken, every one
    /// bought a step — a fastwalk the shard's own pace bucket has slack
    /// enough to absorb without ever answering `0x21`, so it reads as the
    /// walk itself running fast rather than as a rejected packet. This is
    /// the guard: the first direction change after the clock last armed is
    /// still free, and every one after it — until a real, clock-arming step
    /// or turn-then-step pair actually leaves — is paced exactly like an
    /// ordinary step instead.
    turned: bool,
    /// Getting past whatever is directly in the way of a held direction.
    ///
    /// The rule itself is `common/movement`'s [`Detour`], not this module's:
    /// it is a pure function of a four-tile scene and an intent, with one bit
    /// of memory (which flank is being slid along), and it belongs beside the
    /// terrain rather than beside the input handling. What *is* this module's
    /// is when to ask it — only for a held direction, never for a planned
    /// route, which answers for its own obstacles by replanning — and what to
    /// do with [`Step::Stuck`], which needs the facing this module tracks.
    detour: Detour,
    /// How far a body may be turned off the way it was pointed to keep it
    /// moving — see [`Leeway`], and [`Steering::set_leeway`] for where this
    /// comes from.
    leeway: Leeway,
    /// What a turn costs the step it precedes — see [`Turning`], and
    /// [`Steering::set_turning`].
    turning: Turning,
}

impl Steering {
    /// An arrow went down. Answers the step to send now, if any.
    ///
    /// The destination goes with it: see the module docs.
    ///
    /// `None` is the ordinary answer mid-walk and not a refusal — the press has
    /// already changed which way the step the walk owes will go, and that step
    /// leaves at its own deadline. See the queue rule in the module docs.
    ///
    /// `from` and `ground` exist for [`detour`] — routed through [`Steering::take`]
    /// rather than answered here directly, so the very first step of a press
    /// gets the same corner-legal detour every step after it does. Without
    /// this, a player mashing the arrows at a wall would have every *held*
    /// retry detour and every *fresh* press walk straight at it — the
    /// opposite of what mashing a direction into a corner is trying to do.
    /// Only [`Ground::real`] is read on this path: a held direction never plans,
    /// so the map's door-free half has nothing here to answer.
    pub fn press(
        &mut self,
        direction: Direction,
        from: Point,
        now: Instant,
        facing: Direction,
        ground: Ground<'_>,
    ) -> Option<Facing> {
        if !self.keys.press(direction) {
            // The operating system repeating a key that is already the one being
            // obeyed. Its rate is not a walking speed.
            return None;
        }
        self.mouse = None;
        self.goal = None;
        self.stalled = 0;
        self.was = None;
        if !self.free(now) {
            return None;
        }
        self.take(from, now, facing, ground)
    }

    /// An arrow came up.
    ///
    /// The rate floor stays armed: a player who lets go of an arrow and presses
    /// it again 60ms later has not earned a step, and a release that disarmed the
    /// clock is exactly how the old cadence was stepped over.
    pub fn release(&mut self, direction: Direction) {
        self.keys.release(direction);
        if self.keys.asking().is_none() && self.mouse.is_none() && self.goal.is_none() {
            self.stand();
        }
    }

    /// Shift went down or came up.
    ///
    /// Deliberately not re-timed: a walker that starts running mid-step keeps
    /// the deadline it already had, and the next one is a run's. Re-arming here
    /// would let a player tapping shift send a step per tap.
    pub fn set_running(&mut self, running: bool) {
        self.keys.set_running(running);
    }

    /// Choose whether an unmodified movement key runs. Shift temporarily
    /// reverses this preference, as it does in ClassicUO.
    pub fn set_always_running(&mut self, always_running: bool) {
        self.keys.set_always_running(always_running);
    }

    /// How far a body may be turned off the way it was pointed, to keep it
    /// moving past something in the way: an eighth of the compass
    /// ([`Leeway::Eighth`], the default — round a corner, stop at a wall) or a
    /// quarter ([`Leeway::Quarter`] — also slide along the wall's face).
    ///
    /// A player's preference and not a rule — see [`Leeway`]. There is no
    /// client config to read it from yet; when there is, this is the one line
    /// it sets, and nothing else about the walk has to learn about it. Takes
    /// effect on the next step, mid-walk included: the setting is read where
    /// the decision is made, so there is no state to reset when it changes.
    pub fn set_leeway(&mut self, leeway: Leeway) {
        self.leeway = leeway;
    }

    /// What a turn costs the step it precedes — see [`Turning`]. The default is
    /// the reference client's, [`Turning::Deliberate`].
    ///
    /// The same kind of seam as [`Steering::set_leeway`]: a player's setting,
    /// read where the decision is made, so it takes effect on the next step and
    /// there is no state to reset when it changes.
    pub fn set_turning(&mut self, turning: Turning) {
        self.turning = turning;
    }

    /// Drop the plan that was made against an older terrain snapshot.
    pub(crate) fn clear_plan_cache(&mut self) {
        self.cached_plan = None;
    }

    /// Discard a route made before dynamic courtesy obstacles changed. The
    /// destination remains, so the next due step plans from the current tile.
    pub(crate) fn clear_route(&mut self) {
        self.route.clear();
        self.clear_plan_cache();
    }

    /// Start a new render frame. The movement step and the HUD may share one
    /// plan during that frame. A remembered coarse failure is kept separately
    /// so an impossible expensive query is not retried every frame.
    pub(crate) fn begin_frame(&mut self) {
        if !self
            .cached_plan
            .as_ref()
            .is_some_and(|cached| cached.suppress_retry)
        {
            self.clear_plan_cache();
        }
    }

    /// Get the plan shared by the walk and its HUD preview for this frame.
    /// `begin_frame` has already dropped any successful plan from the previous
    /// frame, so a matching plan may have been produced by movement earlier in
    /// the current frame even though it is no longer marked as a preview.
    pub(crate) fn plan_for(&mut self, ground: Ground<'_>, from: Point, goal: Tile) -> Option<Plan> {
        if let Some(cached) = self.cached_plan.as_ref() {
            if cached.from == from && cached.goal == goal {
                return cached.plan.clone();
            }
        }
        let planned = plan(ground, from, goal);
        self.cached_plan = Some(CachedPlan {
            from,
            goal,
            preview: true,
            suppress_retry: planned.is_none() && ground.coarse.is_some(),
            plan: planned.clone(),
        });
        planned
    }

    /// Walk to `tile`, from wherever the body is standing now, or as close to it
    /// as the ground allows — see [`plan`]. Answers the step to send this
    /// instant, if the clock is free for one and there is one worth sending:
    /// `None` also means a body already standing as close as it can get.
    ///
    /// Called on a click and again on every mouse move while the button is held,
    /// which is what makes dragging steer: the destination is replaced and the
    /// cadence is untouched — and, on purpose, so is the plan. `find_path` does
    /// not run here: a drag restates the destination on every raw mouse-move
    /// event, tens of times a second, and a fresh A* search on each of those is
    /// a freeze, not a feature. Only the stale route is dropped; [`Steering::take`]
    /// plans the new one lazily, at most once per step, against whichever tile
    /// is current when a step actually comes due.
    pub fn go_to(
        &mut self,
        tile: Tile,
        from: Point,
        now: Instant,
        facing: Direction,
        ground: Ground<'_>,
    ) -> Option<Facing> {
        self.mouse = None;
        if self.goal != Some(tile) {
            self.route.clear();
        }
        self.goal = Some(tile);
        self.stalled = 0;
        self.was = None;
        // Only when the step in flight has run its course — otherwise a drag
        // across the ground would send a step per mouse event, and a click
        // mid-stride would cut the stride short. The same queue rule the
        // keyboard obeys: the destination is rebuilt now and walked toward at
        // the next deadline.
        if !self.free(now) {
            return None;
        }
        self.take(from, now, facing, ground)
    }

    /// The mouse is asking for `ask` — or, at `None`, has nothing to ask (the
    /// cursor left the map, or the button came up: see
    /// [`Steering::mouse_up`]). The default right-hold idiom: not an order to
    /// reach a tile, a compass heading recomputed from the cursor on every
    /// move and driven exactly like a held arrow key — see the module docs for
    /// why. Answers the step to send now, if any, the same as [`press`](Self::press).
    ///
    /// [`Ask::Turn`] is the same thing at a cursor held close in: the heading
    /// is answered with a facing and no ground — see [`Ask`].
    ///
    /// A [`Heading`] rather than a direction, and measured on the screen from
    /// where the body is drawn: see `App::ask_to_cursor`, which is the one
    /// place that knows what the projection is. What the extra half of it buys
    /// is the tie at a corner — with two ways round and no reason in the
    /// terrain to prefer either, the cursor's own side of the sector is the
    /// reason, and rounding to one of eight would have thrown it away before
    /// anything here could read it.
    ///
    /// The restated-ask gate below is against the whole heading, lean and all,
    /// so a cursor drifting *within* one sector past a corner is a fresh ask
    /// and is answered. It costs nothing when nothing is in the way: the
    /// resolved direction is the same, and the rate floor is what stops a step
    /// leaving early either way.
    ///
    /// A held key still outranks this: called while an arrow is down, this
    /// still updates `direction` for whenever the keyboard lets go, but the
    /// step answered (if any) is the keyboard's, from [`Steering::asking`].
    ///
    /// `from` and `ground` are [`detour`]'s, the same as [`Steering::press`]'s
    /// — and matter more here than there: this is called on every raw
    /// mouse-move, so a player actively steering around an obstacle by
    /// adjusting the cursor sends a fresh ask on nearly every move. Answering
    /// those without [`Steering::take`] would only ever detour the ask a held,
    /// unmoving heading repeats at the next hold — the one case a player
    /// working a corner with the mouse almost never hits.
    pub fn steer(
        &mut self,
        ask: Option<Ask>,
        from: Point,
        now: Instant,
        facing: Direction,
        ground: Ground<'_>,
    ) -> Option<Facing> {
        if self.mouse == ask {
            // The same ask restated — most mouse-move events while the cursor
            // sits still relative to the body, and not a fresh ask any more
            // than the operating system repeating a held key is. The zone is
            // part of it: a cursor crossing the turn ring at an unchanged
            // bearing is a different ask and is answered as one.
            return None;
        }
        self.mouse = ask;
        self.goal = None;
        self.route.clear();
        if ask.is_none() {
            if self.keys.asking().is_none() {
                self.stand();
            }
            return None;
        }
        if !self.free(now) {
            return None;
        }
        self.take(from, now, facing, ground)
    }

    /// The button driving [`Steering::steer`] came up.
    ///
    /// Only the heading lets go — unlike a destination, which keeps walking
    /// itself there after the click that gave it is over, a heading has
    /// nothing behind it once nobody is pointing it any more, so it stops the
    /// instant the button does. The keyboard is untouched: releasing the mouse
    /// must never cut off an arrow held at the same time.
    pub fn mouse_up(&mut self) {
        self.mouse = None;
        if self.keys.asking().is_none() && self.goal.is_none() {
            self.stand();
        }
    }

    /// Everything is up and nowhere is asked for.
    ///
    /// The window losing focus is a key release that never arrives, and a
    /// character that keeps walking while its player is in another window is not
    /// what any of these inputs meant.
    pub fn clear(&mut self) {
        self.keys.clear();
        self.mouse = None;
        self.goal = None;
        self.stand();
    }

    /// The server put the body somewhere this end did not walk it to.
    ///
    /// A `0x21` refusing a step, or a `0x20` moving the body — `link::Body`'s
    /// `corrected`. What it invalidates here is [`Steering::asked`]: the facing
    /// this walk believed it had asked for is void, and the next step decided
    /// against it would be mis-timed as a turn or, worse, mis-timed as not one
    /// and sent a hold late. The server's word replaces it.
    ///
    /// The rate floor is deliberately left alone. A rollback is not a step and
    /// does not buy one, and a client that re-armed its clock on every refusal
    /// would walk faster into a wall than away from one.
    pub fn corrected(&mut self, facing: Direction) {
        self.asked = Some(facing);
    }

    /// The tile being walked to, for the marker the HUD draws on it.
    pub const fn goal(&self) -> Option<Tile> {
        self.goal
    }

    /// The step due by now, if one is.
    ///
    /// `from` is where the body stands — the server's last word on it, which is
    /// also what a destination is steered from. Called from the wait loop, so it
    /// charges one step per call rather than catching up on a stall: a window
    /// that was minimised for a minute has not banked a hundred and fifty steps,
    /// and sending them would be the flood this pacing exists to prevent.
    /// `ground` is what a stalled destination is replanned against — see
    /// [`Steering::take`].
    pub fn due(
        &mut self,
        now: Instant,
        from: Point,
        facing: Direction,
        ground: Ground<'_>,
    ) -> Option<Facing> {
        match self.free(now) {
            true => self.take(from, now, facing, ground),
            false => None,
        }
    }

    /// When the next step is due, for the event loop's deadline.
    ///
    /// Only while something is asking for one. The floor outlives the asking
    /// (see [`Steering::due`]'s field) and a loop woken by a floor nobody is
    /// waiting on would wake for a step it then declines to take, over and over.
    pub fn deadline(&self) -> Option<Instant> {
        match self.asking_for_anything() {
            true => self.due,
            false => None,
        }
    }

    /// Take the step that is due: work out which way, arm the next one, and give
    /// up on a destination that is not getting any closer.
    fn take(&mut self, from: Point, now: Instant, facing: Direction, ground: Ground<'_>) -> Option<Facing> {
        // The stall check is the destination's alone. An arrow held against a
        // wall is the player's own doing and stops when they let go.
        if self.keys.asking().is_none() && self.goal.is_some() {
            self.stalled = match self.was {
                Some(was) if was == from => self.stalled + 1,
                _ => 0,
            };
            if self.stalled >= STUCK_STEPS {
                self.goal = None;
                self.route.clear();
            } else if self.stalled > 0 {
                // The step just taken did not move the body: the plan's guess
                // about the dynamic half of the world (a shut door, a placed
                // crate) was wrong, and replaying the same refused step for
                // three more tries buys nothing. Drop it — replanned below,
                // from where the body actually stands rather than where the
                // old route assumed.
                self.route.clear();
            }
        }

        // Plan lazily: only when there is a destination, nothing of the last
        // plan left to walk, and the body is not already standing on it. This
        // is the only place a search runs — `go_to` never calls one — so a plan
        // costs at most once per step, never once per mouse-move.
        if let Some(tile) = self.goal {
            if Tile::new(from.x, from.y) == tile {
                // Arrived — the ordinary way this ends.
                self.goal = None;
                self.route.clear();
            } else if self.route.is_empty() {
                // `plan` answers for every way this can end — through, up to
                // what is in the way, or as close as the ground gets — so an
                // empty route after it means one thing only: there is nowhere
                // left to walk from here. That is the branch below.
                // The route preview may already have asked the same question
                // during this frame.  Consume that plan once rather than
                // searching live terrain a second time.  A plan built by a
                // previous walk is not reusable: a door can have opened since
                // then. Failed coarse searches remain held until invalidation.
                let preview = self.cached_plan.as_ref().and_then(|cached| {
                    (cached.from == from && cached.goal == tile && (cached.preview || cached.suppress_retry))
                        .then(|| cached.plan.clone())
                });
                let planned = match preview {
                    Some(plan) => {
                        if self.cached_plan.as_ref().is_some_and(|cached| cached.preview) {
                            self.clear_plan_cache();
                        }
                        plan
                    }
                    None => {
                        let planned = plan(ground, from, tile);
                        self.cached_plan = Some(CachedPlan {
                            from,
                            goal: tile,
                            preview: false,
                            suppress_retry: planned.is_none() && ground.coarse.is_some(),
                            plan: planned.clone(),
                        });
                        planned
                    }
                };
                self.route = planned.map(|plan| plan.open).unwrap_or_default().into();
            }
        }

        // A destination with nothing walkable left: the body is standing at
        // whatever is in the way — a shut door, most often, which this end can
        // neither open nor walk through — or as close to the destination as the
        // ground ever gets. Nothing is sent, for the reason a heading wedged in
        // a corner sends nothing: the shard answers a step it refuses with a
        // `0x21`, which rolls the body back and resets the walk sequence this
        // end is counting. The clock is armed anyway so the retry comes at a
        // walking pace — which is also what picks the walk straight back up the
        // moment somebody opens the door — and `was` is set so the patience
        // above is what ends the order.
        //
        // The keyboard is exempt, as it is for the stall check: an arrow held
        // while a destination is still set outranks it and is answered below.
        if self.keys.asking().is_none() && self.goal.is_some() && self.route.is_empty() {
            self.was = Some(from);
            self.arm(self.interval(), now);
            return None;
        }

        let asking = self.asking();
        match asking {
            Some((step, lean)) => {
                self.was = Some(from);
                // The cursor is inside the turn ring: the ask is a facing and
                // nothing else, so no ground is being covered and there is
                // nothing for the terrain to have an opinion about — a turn
                // into a wall is as legal as a turn into a field. The one
                // thing that decides whether a packet leaves is whether the
                // body is already facing that way, exactly as at the corner
                // below: a turn it is already at is a `0x02` the shard would
                // answer by telling this end what it already knows.
                if matches!(self.mouse, Some(Ask::Turn(_))) {
                    let facing_it = step.direction == self.asked.unwrap_or(facing);
                    // Charged either way — the clock is what keeps a held
                    // button from re-asking on a deadline already passed.
                    self.charge(Some(step), now, facing);
                    return (!facing_it).then_some(step);
                }
                // A route already answers for what is in its way — replanned
                // above, on its own patience. Only a held direction (keys or
                // the mouse heading) reaches here with nothing between it and
                // the terrain, so only that case gets the flanking check.
                let step = match self.goal {
                    Some(_) => step,
                    None => match self.detour(
                        ground.real,
                        from,
                        Heading {
                            direction: step.direction,
                            lean,
                        },
                    ) {
                        Step::Ahead(direction) | Step::Aside(direction) => Facing { direction, ..step },
                        // Nowhere legal to go: the direction is blocked and so
                        // is every flank of it — a body wedged into the inside
                        // corner of a building, pushed at the corner itself.
                        //
                        // A step there is one this end has already proven the
                        // shard will refuse, and sending it anyway is not a
                        // no-op: the shard answers `0x21`, which puts the body
                        // back where it was and, on the way, resets the walk
                        // sequence this end is counting — a rollback a hold, for
                        // as long as the player leans on the key. So it is not
                        // sent. What is *not* suppressed is the turn: the body
                        // may not be facing the corner yet, and a mobile asked
                        // for a direction it is not facing turns and moves
                        // nowhere, which the shard accepts (`Walk::Turned`) and
                        // which is the feedback a player pressing into a wall
                        // expects to see. Only once the body already faces it is
                        // there nothing left to ask for.
                        Step::Stuck => match step.direction == self.asked.unwrap_or(facing) {
                            // Charged as if a step had left, and it is the
                            // clock that makes this a refusal rather than a
                            // spin: nothing here clears the asking, so the wait
                            // loop would wake on a deadline already passed and
                            // re-ask immediately, over and over, until the
                            // player let go. Armed, the retry comes at the pace
                            // a walk would have had — which is also what picks
                            // the walk straight back up the moment whatever was
                            // in the way (a door, another body) is gone.
                            true => {
                                self.charge(Some(step), now, facing);
                                return None;
                            }
                            false => step,
                        },
                    },
                };
                self.charge(Some(step), now, facing);
                Some(step)
            }
            // Nothing left to ask for: the arrows are up and the body is
            // standing where it was sent. Nothing is woken for it any more —
            // `deadline` sees that nothing is asking — and the event loop goes
            // back to sleeping on the animation.
            None => {
                self.stand();
                None
            }
        }
    }

    /// Arm the clock for whatever comes after the step just sent, and remember
    /// which way that step went.
    ///
    /// # A turn is a step, and what it costs is a setting
    ///
    /// Turning is a whole step in UO — a mobile asked for a direction it is not
    /// facing turns and moves nowhere, and the shard answers it with its own
    /// `0x22`. What it is *not* is a step against the pace budget: the reference
    /// returns a turn before the bucket is touched, and so does ours
    /// (`openshard_movement::Walker::request`), because spinning on the spot is
    /// something clients genuinely do and throttling it would be absurd.
    ///
    /// So nothing on the wire decides how long the step behind a turn waits;
    /// this end does, and [`Turning`] is the setting. At the default it waits
    /// [`TURN_HOLD`], which is ClassicUO's `MovementSpeed.TurnDelay` and is
    /// what makes a click *square the body up* before it sets off — the
    /// reference client's feel, and what a player who has played one remembers.
    /// At [`Turning::Immediate`] the pair leaves in one wake instead: the clock
    /// is left exactly where it was and the *step* is what charges it, so the
    /// two `0x02`s go out together and the body moves on the frame the input
    /// arrived.
    fn charge(&mut self, asking: Option<Facing>, now: Instant, facing: Direction) {
        let Some(step) = asking else {
            self.stand();
            return;
        };
        // What the body will be facing, which is a step ahead of what the caller
        // can see: our own last ask has not been round the shard thread yet. A
        // rollback is what makes this stale, and `corrected` is where it is put
        // right.
        let facing = self.asked.unwrap_or(facing);
        self.asked = Some(step.direction);
        if step.direction == facing {
            // Ground is being covered: the real thing, paced at the real rate.
            self.arm(self.interval(), now);
            return;
        }
        if let Some(hold) = self.turning.hold() {
            // A turn of its own, and the step it was for waits it out. Nothing
            // special is needed to *make* it wait — the deadline is the queue
            // rule's whole mechanism, and arming it for a shorter interval than
            // a step's is the entire difference between this and the branch
            // below.
            self.arm(hold, now);
            return;
        }
        // A second direction change with no step between is not the "turn
        // precedes its step" pattern the free ride exists for — it is exactly
        // what a heading whose resolved direction keeps changing (`detour`,
        // sliding around an obstacle) produces call after call, and the free
        // ride has already been spent this cadence. Pace it like the real step
        // it is instead of letting it through as another turn — see the field's
        // own doc for what letting it through cost.
        if self.turned {
            self.arm(self.interval(), now);
            return;
        }
        self.walking = true;
        self.turned = true;
        // Where the clock was is either a deadline that has just passed —
        // charging from `now` instead would fold this wake's lateness into the
        // cadence, which is the drift `next_due` exists to refuse — or nothing
        // at all, which is the first ask of a walk and is due this instant.
        self.due = Some(self.due.unwrap_or(now));
    }

    /// Arm the clock for a step of length `interval`, and declare the walk under
    /// way.
    fn arm(&mut self, interval: Duration, now: Instant) {
        // Read before the walk is declared under way: what `next_due` needs to
        // know is whether the deadline it is chaining from belongs to a walk
        // that was still going.
        let due = self.next_due(now, interval);
        self.walking = true;
        self.turned = false;
        self.due = Some(due);
    }

    /// Whether the step in flight has run its course, so that the next one may
    /// leave.
    ///
    /// The queue rule, in one line. Everything that asks for a step goes through
    /// here, and nothing anywhere clears [`Steering::due`] — so this is false for
    /// exactly as long as a step is being walked, however the asking arrived.
    fn free(&self, now: Instant) -> bool {
        self.due.is_none_or(|due| now >= due)
    }

    /// Whether any input is asking to walk at all.
    fn asking_for_anything(&self) -> bool {
        self.keys.asking().is_some() || self.mouse.is_some() || self.goal.is_some()
    }

    /// Which way to step, keyboard first, then the mouse heading, then the
    /// planned route's own next step.
    ///
    /// `goal` and `mouse` are never both set — [`Steering::go_to`] and
    /// [`Steering::steer`] each clear the other — so their order below never
    /// actually competes; it exists for the one input that outranks both. A
    /// `goal` with nothing in `route` to answer from has already been resolved
    /// one way or another by [`Steering::take`] before this runs — arrived, or
    /// standing where the ground ran out and returned from there — so a
    /// defensive `None` here reads as "nothing to ask for", the same as
    /// arriving.
    /// The lean rides along, because only one of the three asks has one: an
    /// arrow key and a planned route point at a sector and nothing finer, and
    /// saying so with [`Heading::centred`] is the honest way to say it. Making
    /// one up — reconstructing a bearing from the direction they named — would
    /// hand the tie-break a preference nobody expressed.
    fn asking(&mut self) -> Option<(Facing, Lean)> {
        if let Some(facing) = self.keys.asking() {
            return Some((facing, Lean::Centred));
        }
        let pace = |direction| match self.keys.running() {
            true => Facing::running(direction),
            false => Facing::walking(direction),
        };
        if let Some(ask) = self.mouse {
            // A turn's pace is nobody's business — it covers no ground — so
            // the running flag rides along on both and means something on
            // only one of them.
            let heading = ask.heading();
            return Some((pace(heading.direction), heading.lean));
        }
        self.goal?;
        match self.route.pop_front() {
            Some(direction) => Some((pace(direction), Lean::Centred)),
            None => {
                self.goal = None;
                None
            }
        }
    }

    /// When the step after this one is due.
    ///
    /// Measured from the deadline that has just passed and not from the moment
    /// the event loop got round to it. The loop is woken by the operating
    /// system whenever it gets round to it and never early, so arming from
    /// `now` folds every wake's lateness into the cadence — where it
    /// *accumulates*: a handful of milliseconds a step is a body a fifth of a
    /// tile behind after ten and a whole tile behind after fifty, and nothing
    /// ever gives it back. Found by the walk oracle in `dst.rs`, which is
    /// exactly the divergence it exists to see: every unit involved was right
    /// about its own rate and the body still fell behind the player's hand.
    ///
    /// A wake later than a whole step is not jitter — the window was minimised
    /// or the machine asleep — and those steps are deliberately not banked (see
    /// [`Steering::due`]), so the cadence starts again from `now`.
    ///
    /// `interval` is what the step being charged actually takes: a walk's, a
    /// run's, or a turn's ([`Turning`]) — the chaining is the same for all
    /// three, and the interval is the only thing that differs.
    fn next_due(&self, now: Instant, interval: Duration) -> Instant {
        match self.due {
            Some(due) if self.walking && now < due + interval => due + interval,
            _ => now + interval,
        }
    }

    /// Nothing is being asked for any more: forget what the destination's
    /// patience was measuring.
    ///
    /// Deliberately not a reset. [`Steering::due`] stays — it is the rate floor
    /// and a walk that ended does not refund it — and so does
    /// [`Steering::asked`], which is still the truth about which way the body
    /// was last sent. Only a rollback makes that false, and only
    /// [`Steering::corrected`] says so.
    fn stand(&mut self) {
        self.was = None;
        self.stalled = 0;
        self.walking = false;
        self.turned = false;
        self.detour.forget();
    }

    /// How long a step takes at the pace being asked for.
    fn interval(&self) -> Duration {
        match self.keys.running() {
            true => RUN_HOLD,
            false => WALK_HOLD,
        }
    }

    /// Where to actually step, given a held `direction` the terrain may or may
    /// not allow — `common/movement`'s [`Detour`] over the four tiles that
    /// decide it, and [`Around::read`] is what reads them.
    ///
    /// Only a *held* direction comes here. A planned route answers for what is
    /// in its way by replanning, on its own patience; a heading has no route
    /// and no destination, so this local look is the whole of what it can do.
    fn detour(&mut self, terrain: &dyn Terrain, from: Point, intent: Heading) -> Step {
        let around = Around::read(terrain, from, intent);
        let step = self.detour.step(&around, self.leeway);
        debug_detour(from, &around, step);
        step
    }
}

/// Plan a route from `from` to `tile`, in the two halves a walk and a picture of
/// it both need.
///
/// **The world as it stands is asked first, and its route is the whole plan.** A
/// shut door with a way round is a longer walk and not a barred one, and that
/// longer walk is what the body takes — nothing here prefers a door to a detour.
///
/// Only when there is no way through at all is [`Ground::through_doors`] asked:
/// the same ground with the shut doors opened. That route is then cut at the
/// first step the real ground refuses — and because the two readings differ by
/// nothing else, the cut lands on a shut door every time. What is left before it
/// is a walk the body can really take; what is left after it is what that door
/// is in the way of. The body walks the first half and stops in front of the
/// leaf, which is what a player asking to go through a door expects of a client
/// that cannot open it for them; the second half is what tells them *why* it
/// stopped there.
///
/// **And where neither half has a way through, the answer is still a walk.** A
/// destination nothing can reach — clicked on a wall, on the far bank, on a tile
/// too far for [`PLAN_BUDGET`] — is planned as far toward it as the world as it
/// stands allows ([`find_path_toward`]), and the body stops there. *Nothing here
/// ever asks for a step this end can already see refused.* Walking at a wall in
/// a straight line until a patience runs out is what this used to do, and every
/// one of those steps was a `0x21`, a rollback, and a walk sequence reset.
///
/// **The one place either question is answered.** [`Steering::take`] walks the
/// open half and `App`'s HUD draws both, so the green line a player sees is the
/// route being walked rather than a second opinion that happens to agree with
/// it. A cut written for the picture alone would be exactly the shape of bug
/// `docs/parity.md` is about.
///
/// A `barred` half that comes back empty is a route with nothing standing in it:
/// one that arrives, or one that stops where the ground itself does. It stays
/// empty in the one case where the two readings disagree the other way — the real
/// ground allows every step of a route only the doors-open one managed to *find*
/// within [`PLAN_BUDGET`] — because a plan is barred by what stops a step, not by
/// which search found it.
///
/// `None` means there is nowhere to go at all: the body already stands as close
/// to the destination as the ground allows, or is walled in where it is.
/// [`Steering::take`] stands on that rather than sending anything, and the
/// destination's own patience is what ends the order. A plan with nothing in
/// either half says the same for the one case that is not a refusal — `from`
/// already standing on `tile`, a body that has arrived.
pub fn plan(ground: Ground<'_>, from: Point, tile: Tile) -> Option<Plan> {
    let started = Instant::now();
    let goal = Point::new(tile.x, tile.y, from.z);
    // Each half gets its own cache.  The real and doors-open readings are
    // different terrain snapshots, so sharing even a successful transition
    // between them could walk through a door that is still shut.
    let real = CachedTerrain::new(ground.real);
    if let Some(open) = ground.path(&real, from, goal) {
        let result = Some(Plan {
            open_points: replay(&real, from, &open),
            open,
            barred: Vec::new(),
            barred_points: Vec::new(),
        });
        debug_plan_cache(from, goal, started.elapsed(), &real, None, result.as_ref());
        return result;
    }
    let doors_open = CachedTerrain::new(ground.through_doors);
    let Some(through) = ground.path(&doors_open, from, goal) else {
        // Not even with the doors open, so there is nothing to say about the
        // far side of anything: no route through this destination's own tile is
        // known, and drawing one would be inventing it. What is left is how
        // close the world as it stands can get, which is a walk and not a guess.
        let Some(open) = find_path_toward(&real, from, goal, PLAN_BUDGET) else {
            debug_plan_cache(from, goal, started.elapsed(), &real, Some(&doors_open), None);
            return None;
        };
        let result = Some(Plan {
            open_points: replay(&real, from, &open),
            open,
            barred: Vec::new(),
            barred_points: Vec::new(),
        });
        debug_plan_cache(
            from,
            goal,
            started.elapsed(),
            &real,
            Some(&doors_open),
            result.as_ref(),
        );
        return result;
    };
    let mut open = Vec::new();
    let mut barred = Vec::new();
    let mut open_points = Vec::new();
    // Walked over the ground as it really is until it refuses a step: `at` going
    // absent is the cut, and everything from there on belongs to the far side of
    // the door. `step_allowed` and not `can_step`, because the corner rule is
    // half of what refuses a step and the walk this is a claim about obeys both.
    let mut at = Some(from);
    for direction in through {
        match at.and_then(|point| step_allowed(&real, point, direction)) {
            Some(next) => {
                at = Some(next);
                open.push(direction);
                open_points.push(next);
            }
            None => {
                at = None;
                barred.push(direction);
            }
        }
    }
    let mut barred_points = Vec::new();
    let mut barred_at = open_points.last().copied().unwrap_or(from);
    for &direction in &barred {
        let Some(next) = step_allowed(&doors_open, barred_at, direction) else {
            break;
        };
        barred_at = next;
        barred_points.push(next);
    }
    let result = Some(Plan {
        open,
        barred,
        open_points,
        barred_points,
    });
    debug_plan_cache(
        from,
        goal,
        started.elapsed(),
        &real,
        Some(&doors_open),
        result.as_ref(),
    );
    result
}

/// Replay a direction list while the query-local cache still owns its terrain
/// snapshot. The returned points are immutable plan output, not a live query.
fn replay(terrain: &CachedTerrain<'_>, from: Point, directions: &[Direction]) -> Vec<Point> {
    let mut at = from;
    directions
        .iter()
        .filter_map(|&direction| {
            let next = step_allowed(terrain, at, direction)?;
            at = next;
            Some(next)
        })
        .collect()
}

/// Emit one compact summary for the two query-local terrain snapshots.  This
/// remains opt-in: these paths run on the render thread and per-transition
/// logging would both obscure the useful numbers and distort them.
fn debug_plan_cache(
    from: Point,
    goal: Point,
    elapsed: Duration,
    real: &CachedTerrain<'_>,
    doors_open: Option<&CachedTerrain<'_>>,
    result: Option<&Plan>,
) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var_os("OPENSHARD_PATH_DEBUG").is_some()) {
        return;
    }
    let real = real.stats();
    let doors_open = doors_open.map(CachedTerrain::stats).unwrap_or_default();
    let (open_steps, barred_steps) = result
        .map(|plan| (plan.open.len(), plan.barred.len()))
        .unwrap_or((0, 0));
    eprintln!(
        "path-debug kind=plan from=({}, {}, {}) to=({}, {}, {}) elapsed_ms={:.3} result={} open_steps={open_steps} barred_steps={barred_steps} real_calls={} real_hits={} real_misses={} real_entries={} doors_calls={} doors_hits={} doors_misses={} doors_entries={}",
        from.x,
        from.y,
        from.z,
        goal.x,
        goal.y,
        goal.z,
        elapsed.as_secs_f64() * 1_000.0,
        result.is_some(),
        real.hits + real.misses,
        real.hits,
        real.misses,
        real.entries,
        doors_open.hits + doors_open.misses,
        doors_open.hits,
        doors_open.misses,
        doors_open.entries,
    );
}

/// Temporary: `OPENSHARD_DETOUR_DEBUG=1` prints every ask that met something
/// in the way — the four-tile scene as it was read, and what came back — to
/// stderr. For chasing corner reports live, against a real map this end has no
/// fixture for; pull once those stay resolved.
fn debug_detour(from: Point, around: &Around, step: Step) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("OPENSHARD_DETOUR_DEBUG").is_some()) {
        if let Step::Ahead(_) = step {
            return;
        }
        static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let elapsed = T0.get_or_init(Instant::now).elapsed().as_millis();
        eprintln!("detour: t={elapsed}ms from={from:?} around={around:?} step={step:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openshard_movement::{OpenWorld, step_from};

    /// The clock is a parameter here as it is in `WalkPace`, so a rate can be
    /// tested without sleeping through one.
    fn at(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    /// Where the body stands in the tests below. Nothing here reads a map, so
    /// the height is only carried through.
    fn here() -> Point {
        Point::new(100, 100, 0)
    }

    #[test]
    fn a_press_steps_at_once_and_then_at_the_walking_rate() {
        let start = Instant::now();
        let mut steering = Steering::default();

        assert_eq!(
            steering.press(
                Direction::NorthWest,
                here(),
                start,
                Direction::NorthWest,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::NorthWest))
        );
        // Nothing is due until a whole step has passed.
        assert_eq!(
            steering.due(
                at(start, 399),
                here(),
                Direction::NorthWest,
                Ground::plain(&OpenWorld)
            ),
            None
        );
        assert_eq!(
            steering.due(
                at(start, 400),
                here(),
                Direction::NorthWest,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::NorthWest))
        );
        assert_eq!(
            steering.due(
                at(start, 401),
                here(),
                Direction::NorthWest,
                Ground::plain(&OpenWorld)
            ),
            None
        );
    }

    /// The operating system repeats a held key at its own rate, and that rate is
    /// not a walking speed — so a repeat neither sends a step nor re-arms the
    /// clock.
    #[test]
    fn the_operating_systems_repeat_is_not_a_step() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .press(
                Direction::NorthWest,
                here(),
                start,
                Direction::NorthWest,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        for repeat in 1..30 {
            assert_eq!(
                steering.press(
                    Direction::NorthWest,
                    here(),
                    at(start, repeat * 10),
                    Direction::NorthWest,
                    Ground::plain(&OpenWorld)
                ),
                None
            );
        }
        assert_eq!(steering.deadline(), Some(at(start, 400)), "the first press's");
    }

    #[test]
    fn shift_is_the_running_flag_and_halves_the_gap() {
        let start = Instant::now();
        let mut steering = Steering::default();
        steering.set_running(true);

        assert_eq!(
            steering.press(
                Direction::SouthEast,
                here(),
                start,
                Direction::SouthEast,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::running(Direction::SouthEast))
        );
        assert_eq!(
            steering.due(
                at(start, 199),
                here(),
                Direction::SouthEast,
                Ground::plain(&OpenWorld)
            ),
            None
        );
        assert_eq!(
            steering.due(
                at(start, 200),
                here(),
                Direction::SouthEast,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::running(Direction::SouthEast))
        );
    }

    /// Shift pressed mid-walk does not itself send anything: the deadline in
    /// flight is kept and the pace changes from the next step on.
    #[test]
    fn shift_mid_step_does_not_send_a_step_of_its_own() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .press(
                Direction::North,
                here(),
                start,
                Direction::North,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        steering.set_running(true);
        assert_eq!(
            steering.due(
                at(start, 200),
                here(),
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            None,
            "the walk's deadline stands"
        );
        assert_eq!(
            steering.due(
                at(start, 400),
                here(),
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::running(Direction::North))
        );
        // And from there it is a runner's.
        assert_eq!(
            steering.due(
                at(start, 600),
                here(),
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::running(Direction::North))
        );
    }

    #[test]
    fn nothing_asked_for_is_nothing_due() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .press(
                Direction::West,
                here(),
                start,
                Direction::West,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        steering.release(Direction::West);
        assert_eq!(steering.deadline(), None);
        assert_eq!(
            steering.due(
                at(start, 10_000),
                here(),
                Direction::West,
                Ground::plain(&OpenWorld)
            ),
            None
        );
    }

    /// A click walks toward the tile, a step at a time, and stops on it.
    #[test]
    fn a_destination_is_walked_to_and_let_go_of_on_arrival() {
        let start = Instant::now();
        let mut steering = Steering::default();

        // Three tiles east, at the same row.
        assert_eq!(
            steering.go_to(
                Tile::new(103, 100),
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East)),
            "the first step leaves at once"
        );
        let mut now = start;
        for x in 101..=102 {
            now = at(start, 400 * u64::from(x - 100));
            assert_eq!(
                steering.due(
                    now,
                    Point::new(x, 100, 0),
                    Direction::East,
                    Ground::plain(&OpenWorld)
                ),
                Some(Facing::walking(Direction::East)),
                "still short of it"
            );
        }
        // Standing on it: nothing more is asked for, and the clock stops with
        // the asking.
        assert_eq!(
            steering.due(
                at(now, 400),
                Point::new(103, 100, 0),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            None
        );
        assert_eq!(steering.goal(), None);
        assert_eq!(steering.deadline(), None);
    }

    /// A diagonal is one step, so a destination off both axes is walked to
    /// diagonally rather than in two moves.
    #[test]
    fn a_destination_off_both_axes_is_stepped_diagonally() {
        let start = Instant::now();
        let mut steering = Steering::default();
        assert_eq!(
            steering.go_to(
                Tile::new(105, 105),
                here(),
                start,
                Direction::SouthEast,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::SouthEast))
        );
    }

    /// A wall directly on the straight line to a destination, with a way around
    /// it — the whole point of planning a route rather than walking greedily at
    /// the goal and refusing at the wall. Blocks a single tile east of the body,
    /// which is exactly where the old `direction_toward` step would have gone
    /// and stalled.
    struct Wall {
        blocked: Tile,
    }

    impl openshard_movement::Terrain for Wall {
        fn can_step(&self, from: Point, to: Point) -> Option<Point> {
            match Tile::new(to.x, to.y) == self.blocked {
                true => None,
                false => OpenWorld.can_step(from, to),
            }
        }
    }

    #[test]
    fn a_click_to_walk_destination_routes_around_a_wall_in_its_way() {
        let start = Instant::now();
        let mut steering = Steering::default();
        let wall = Wall {
            blocked: Tile::new(101, 100),
        };

        // Straight east into the blocked tile, then three more east. The old
        // greedy step would ask for East every time, walk into (101, 100) and
        // stall on it; the plan detours around it instead.
        let first = steering
            .go_to(
                Tile::new(104, 100),
                here(),
                start,
                Direction::East,
                Ground::plain(&wall),
            )
            .expect("a route around the wall exists");
        assert_ne!(
            first.direction,
            Direction::East,
            "east is the blocked tile; the plan must step around it, not into it"
        );

        // Walk it out and check it actually lands on the destination rather
        // than stalling on STUCK_STEPS: apply each step the plan asks for to a
        // real position (unlike the cadence tests above, which pin a hard-coded
        // sequence because they are not exercising the planner) and feed that
        // position back in, the way the server's `0x22` would.
        let (dx, dy) = first.direction.step();
        let mut pos = Point::new(
            (i32::from(here().x) + dx) as u16,
            (i32::from(here().y) + dy) as u16,
            here().z,
        );
        assert!(
            wall.can_step(here(), pos).is_some(),
            "the first step is not onto the wall"
        );

        let mut now = start;
        for step in 1..10 {
            now = at(now, WALK_HOLD.as_millis() as u64);
            let Some(facing) = steering.due(now, pos, Direction::East, Ground::plain(&wall)) else {
                break;
            };
            let (dx, dy) = facing.direction.step();
            let next = Point::new(
                (i32::from(pos.x) + dx) as u16,
                (i32::from(pos.y) + dy) as u16,
                pos.z,
            );
            assert!(
                wall.can_step(pos, next).is_some(),
                "step {step} walked onto the blocked tile"
            );
            pos = next;
        }
        assert_eq!(
            (pos.x, pos.y),
            (104, 100),
            "the destination is reached despite the wall"
        );
        assert_eq!(steering.goal(), None, "and let go of on arrival");
    }

    /// A destination nothing can stand on — clicked squarely on a wall, on a
    /// pillar, on a crate — is walked *up to* and stopped at. The one thing that
    /// must not happen is a step into it: this end can already see the tile is
    /// refused, and asking anyway buys a `0x21`, a rollback and a walk sequence
    /// reset, once a hold, for as long as the patience lasts. That is what this
    /// used to do.
    #[test]
    fn a_destination_that_cannot_be_stood_on_is_walked_up_to_and_stopped_at() {
        let start = Instant::now();
        let mut steering = Steering::default();
        // Four tiles east, and the clicked tile itself is the blocked one, so
        // no search finds a route to it however it looks.
        let wall = Wall {
            blocked: Tile::new(104, 100),
        };

        let mut pos = here();
        let mut now = start;
        assert_eq!(
            steering.go_to(
                Tile::new(104, 100),
                pos,
                now,
                Direction::East,
                Ground::plain(&wall)
            ),
            Some(Facing::walking(Direction::East)),
            "as close as the ground allows is still somewhere worth walking"
        );
        // Walk what it asks for, feeding the position back the way a `0x22`
        // would, and check every step is one the ground actually allows.
        for _ in 0..3 {
            pos = Point::new(pos.x + 1, pos.y, pos.z);
            now = at(now, 400);
            let step = steering.due(now, pos, Direction::East, Ground::plain(&wall));
            if step.is_none() {
                break;
            }
            assert!(
                wall.can_step(pos, Point::new(pos.x + 1, pos.y, pos.z)).is_some(),
                "a step was asked for onto the blocked tile at {}, {}",
                pos.x + 1,
                pos.y
            );
        }
        assert_eq!(
            (pos.x, pos.y),
            (103, 100),
            "the body stops on the last tile before the one it was sent to"
        );
        // And there it stands: nothing is sent, the order is held for the usual
        // patience, and then let go of.
        for step in 1..u64::from(STUCK_STEPS) {
            assert_eq!(
                steering.due(at(now, 400 * step), pos, Direction::East, Ground::plain(&wall)),
                None,
                "beat {step} against the wall must send nothing"
            );
            assert_eq!(
                steering.goal(),
                Some(Tile::new(104, 100)),
                "and still hold the order"
            );
        }
        assert_eq!(
            steering.due(
                at(now, 400 * u64::from(STUCK_STEPS)),
                pos,
                Direction::East,
                Ground::plain(&wall)
            ),
            None
        );
        assert_eq!(steering.goal(), None, "and the destination is let go of");
    }

    /// The same, from a body that is already as close as it can get: there is
    /// nothing to walk at all, so the order is held for its patience and ends
    /// without a single packet.
    #[test]
    fn a_destination_with_nowhere_closer_to_stand_sends_nothing_at_all() {
        let start = Instant::now();
        let mut steering = Steering::default();
        let wall = Wall {
            blocked: Tile::new(101, 100),
        };

        assert_eq!(
            steering.go_to(
                Tile::new(101, 100),
                here(),
                start,
                Direction::East,
                Ground::plain(&wall)
            ),
            None,
            "the body is already against it; a step would only be refused"
        );
        assert_eq!(
            steering.goal(),
            Some(Tile::new(101, 100)),
            "the order stands for now"
        );
        for step in 1..u64::from(STUCK_STEPS) {
            assert_eq!(
                steering.due(
                    at(start, 400 * step),
                    here(),
                    Direction::East,
                    Ground::plain(&wall)
                ),
                None,
                "beat {step} still has nothing to send"
            );
        }
        assert_eq!(
            steering.due(
                at(start, 400 * u64::from(STUCK_STEPS)),
                here(),
                Direction::East,
                Ground::plain(&wall)
            ),
            None
        );
        assert_eq!(steering.goal(), None, "and the destination is let go of");
    }

    /// A wall clean across the way with one doorway in it, and — in the world as
    /// it stands — a shut leaf standing in that doorway.
    ///
    /// The shape [`Ground`]'s two readings exist for: with the door open there
    /// is a way through and as the world stands there is none at all, so what
    /// "no route" means can only be answered by asking both. `shut` stands in
    /// for `clutter.rs`'s own list of doors, which is where a real client gets
    /// the same two answers from one index.
    struct Doorwall {
        shut: bool,
    }

    /// The one gap in [`Doorwall`]'s line, three tiles east of the body.
    const DOORWAY: Tile = Tile::new(103, 100);

    impl openshard_movement::Terrain for Doorwall {
        fn can_step(&self, from: Point, to: Point) -> Option<Point> {
            let barred = to.x == DOORWAY.x && (Tile::new(to.x, to.y) != DOORWAY || self.shut);
            match barred {
                true => None,
                false => OpenWorld.can_step(from, to),
            }
        }
    }

    struct MutableDoor {
        shut: std::cell::Cell<bool>,
    }

    impl openshard_movement::Terrain for MutableDoor {
        fn can_step(&self, from: Point, to: Point) -> Option<Point> {
            (to.x != DOORWAY.x || !self.shut.get())
                .then(|| OpenWorld.can_step(from, to))
                .flatten()
        }
    }

    /// The same door contract at a distance that exceeds [`PLAN_BUDGET`]. The
    /// coarse graph is built from the open map; the `shut` reading is what must
    /// still keep the executable half from crossing the leaf.
    struct LongDoor {
        shut: bool,
        x: u16,
    }

    impl openshard_movement::Terrain for LongDoor {
        fn can_step(&self, from: Point, to: Point) -> Option<Point> {
            (!self.shut || to.x != self.x)
                .then(|| OpenWorld.can_step(from, to))
                .flatten()
        }
    }

    /// Two tiles past the doorway: a destination only reachable through it.
    const BEYOND: Tile = Tile::new(105, 100);

    /// With the door open there is nothing to cut: the world as it stands
    /// answers, and its answer is the whole plan.
    #[test]
    fn a_way_through_the_world_as_it_stands_is_the_whole_plan() {
        let open = Doorwall { shut: false };
        let plan = plan(Ground::plain(&open), here(), BEYOND).expect("the doorway is open");
        assert_eq!(plan.open, vec![Direction::East; 5]);
        assert!(
            plan.barred.is_empty(),
            "an open doorway is a route, and a route has nothing barred about it"
        );
    }

    /// The consumer this cache exists for: a destination beyond the ordinary
    /// plan budget is still a real route, assembled from bounded exact hops.
    /// Keeping this here, above `Ground::plain`, proves the client actually
    /// reaches for the graph rather than merely storing one at startup.
    #[test]
    fn a_far_destination_uses_the_coarse_route_after_the_ordinary_budget_ends() {
        let router = NavigationGraph::build(&OpenWorld, 704, 32).expect("a representable map");
        let from = Point::new(1, 1, 0);
        let goal = Tile::new(702, 1);
        assert!(
            find_path(&OpenWorld, from, Point::new(goal.x, goal.y, 0), PLAN_BUDGET).is_none(),
            "the flat plan is intentionally too short"
        );
        let plan = plan(
            Ground {
                real: &OpenWorld,
                through_doors: &OpenWorld,
                guide: &OpenWorld,
                coarse: Some(&router),
            },
            from,
            goal,
        )
        .expect("the coarse graph reaches across the facet");
        assert_eq!(plan.open.len(), 701);
        assert!(plan.barred.is_empty());
    }

    #[test]
    fn a_far_shut_door_is_still_a_cut_coarse_route_not_a_walk_through_it() {
        let open = LongDoor { shut: false, x: 400 };
        let shut = LongDoor { shut: true, x: 400 };
        let router = NavigationGraph::build(&open, 704, 32).expect("a representable map");
        let from = Point::new(1, 1, 0);
        let goal = Tile::new(702, 1);
        let plan = plan(
            Ground {
                real: &shut,
                through_doors: &open,
                guide: &open,
                coarse: Some(&router),
            },
            from,
            goal,
        )
        .expect("the doors-open map still has a long route");
        assert!(!plan.barred.is_empty(), "the shut leaf remains a visible refusal");

        let mut at = from;
        for &direction in &plan.open {
            at = step_allowed(&shut, at, direction).expect("the open half is actually walkable");
        }
        assert!(
            step_allowed(&shut, at, plan.barred[0]).is_none(),
            "the red half starts at the first step the real terrain rejects"
        );
        for &direction in &plan.barred {
            at = step_allowed(&open, at, direction).expect("the doors-open half continues the same route");
        }
        assert_eq!((at.x, at.y), (goal.x, goal.y));
    }

    /// The whole point: a destination behind a shut door is planned up to the
    /// door and named barred past it, rather than answered "no route" — which is
    /// what the doors-open reading is asked for, and what makes the walk go
    /// somewhere useful and the drawn line change colour at the door.
    #[test]
    fn a_shut_door_plans_up_to_it_and_names_the_rest_barred() {
        let shut = Doorwall { shut: true };
        let open = Doorwall { shut: false };
        assert!(
            find_path(&shut, here(), Point::new(BEYOND.x, BEYOND.y, 0), PLAN_BUDGET).is_none(),
            "the premise: as the world stands there is no way through at all"
        );
        let plan = plan(
            Ground {
                real: &shut,
                through_doors: &open,
                guide: &open,
                coarse: None,
            },
            here(),
            BEYOND,
        )
        .expect("the map itself has a doorway");
        assert_eq!(
            plan.open,
            vec![Direction::East; 2],
            "the walk stops on the tile before the doorway, not in it"
        );
        assert_eq!(
            plan.barred,
            vec![Direction::East; 3],
            "and the rest of the way is what the shut leaf is in the way of"
        );
    }

    #[test]
    fn plan_replay_is_snapshot_data_after_terrain_mutates() {
        let terrain = MutableDoor {
            shut: std::cell::Cell::new(true),
        };
        let plan = plan(
            Ground {
                real: &terrain,
                through_doors: &OpenWorld,
                guide: &OpenWorld,
                coarse: None,
            },
            here(),
            BEYOND,
        )
        .expect("the open snapshot provides a route");
        assert_eq!(plan.open_points.len(), 2);
        assert_eq!(plan.barred_points.len(), 3);
        terrain.shut.set(false);
        assert_eq!(
            plan.open_points.len(),
            2,
            "replay does not query the new snapshot"
        );
        assert_eq!(plan.barred_points.len(), 3);
    }

    /// Something in the way with a route round it is a *longer walk*, never a
    /// barred plan: the world as it stands is asked first, and its detour is the
    /// answer. Getting this backwards would draw half the town red every time a
    /// crate stood on the straight line.
    #[test]
    fn a_thing_in_the_way_with_a_route_round_it_is_not_barred() {
        let wall = Wall {
            blocked: Tile::new(101, 100),
        };
        let plan = plan(
            Ground {
                real: &wall,
                through_doors: &OpenWorld,
                guide: &OpenWorld,
                coarse: None,
            },
            here(),
            Tile::new(104, 100),
        )
        .expect("there is a way round a single tile");
        assert!(
            plan.barred.is_empty(),
            "a crate with a way round was planned as a barred route instead of a detour"
        );
        assert_ne!(
            plan.open.first(),
            Some(&Direction::East),
            "east is the blocked tile; the plan must step around it"
        );
    }

    /// The walk itself, end to end: it goes as far as the door, sends nothing
    /// into it, and holds the order for [`STUCK_STEPS`] before giving it up.
    ///
    /// Nothing is sent at the door for the reason nothing is sent in a corner: a
    /// step this end has already proven the shard refuses comes back as a
    /// `0x21`, which rolls the body back and resets the walk sequence — a
    /// rollback a hold, for as long as the player waits.
    #[test]
    fn a_destination_behind_a_shut_door_is_walked_up_to_and_no_further() {
        let start = Instant::now();
        let mut steering = Steering::default();
        let shut = Doorwall { shut: true };
        let open = Doorwall { shut: false };
        let ground = Ground {
            real: &shut,
            through_doors: &open,
            guide: &open,
            coarse: None,
        };

        assert_eq!(
            steering.go_to(BEYOND, here(), start, Direction::East, ground),
            Some(Facing::walking(Direction::East)),
            "the way to the door is a way worth walking"
        );
        assert_eq!(
            steering.due(at(start, 400), Point::new(101, 100, 0), Direction::East, ground),
            Some(Facing::walking(Direction::East))
        );
        // On the tile before the doorway, which is as far as the ground goes.
        let waiting = Point::new(102, 100, 0);
        assert_eq!(
            steering.due(at(start, 800), waiting, Direction::East, ground),
            None,
            "a step into the shut leaf is one the shard would refuse"
        );
        assert_eq!(
            steering.goal(),
            Some(BEYOND),
            "the order is not given up on the moment it stops moving"
        );
        assert_eq!(
            steering.deadline(),
            Some(at(start, 1200)),
            "and the retry is paced like a step rather than spun on a deadline already past"
        );
        for step in 1..u64::from(STUCK_STEPS) {
            assert_eq!(
                steering.due(at(start, 800 + 400 * step), waiting, Direction::East, ground),
                None,
                "beat {step} at the door still has nothing to send"
            );
            assert_eq!(steering.goal(), Some(BEYOND), "and still holds the order");
        }
        assert_eq!(
            steering.due(
                at(start, 800 + 400 * u64::from(STUCK_STEPS)),
                waiting,
                Direction::East,
                ground
            ),
            None
        );
        assert_eq!(
            steering.goal(),
            None,
            "a body that has stood at the door for four beats has been given up on"
        );
    }

    /// And the other half of standing there: the walk picks itself back up the
    /// moment somebody opens the door, with no fresh click. The clock armed at
    /// the door is what makes that happen at a walking pace.
    #[test]
    fn the_walk_resumes_the_moment_the_door_opens() {
        let start = Instant::now();
        let mut steering = Steering::default();
        let shut = Doorwall { shut: true };
        let open = Doorwall { shut: false };

        let waiting = Point::new(102, 100, 0);
        steering
            .go_to(
                BEYOND,
                here(),
                start,
                Direction::East,
                Ground {
                    real: &shut,
                    through_doors: &open,
                    guide: &open,
                    coarse: None,
                },
            )
            .unwrap();
        steering.due(
            at(start, 400),
            Point::new(101, 100, 0),
            Direction::East,
            Ground {
                real: &shut,
                through_doors: &open,
                guide: &open,
                coarse: None,
            },
        );
        assert_eq!(
            steering.due(
                at(start, 800),
                waiting,
                Direction::East,
                Ground {
                    real: &shut,
                    through_doors: &open,
                    guide: &open,
                    coarse: None,
                }
            ),
            None,
            "the premise: the body is standing at the shut door"
        );
        // The leaf swings: the same tile, now part of the world a step is
        // allowed by.
        assert_eq!(
            steering.due(at(start, 1200), waiting, Direction::East, Ground::plain(&open)),
            Some(Facing::walking(Direction::East)),
            "the door opened and nothing asked again — the walk must carry on by itself"
        );
    }

    /// Dragging the mouse across the ground restates the destination on every
    /// move, and must not send a step on every one of them.
    #[test]
    fn restating_a_destination_does_not_restart_the_cadence() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .go_to(
                Tile::new(110, 100),
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        for tick in 1..20 {
            assert_eq!(
                steering.go_to(
                    Tile::new(110, 100 + tick as u16),
                    here(),
                    at(start, tick * 10),
                    Direction::East,
                    Ground::plain(&OpenWorld)
                ),
                None
            );
        }
        assert_eq!(steering.deadline(), Some(at(start, 400)));
    }

    /// A terrain that counts how many times it was asked, so a test can prove
    /// no search ran rather than only that the right answer came back.
    #[derive(Default)]
    struct CountingTerrain {
        can_step_calls: std::cell::Cell<u32>,
    }

    impl openshard_movement::Terrain for CountingTerrain {
        fn can_step(&self, from: Point, to: Point) -> Option<Point> {
            self.can_step_calls.set(self.can_step_calls.get() + 1);
            OpenWorld.can_step(from, to)
        }
    }

    /// A drag restates the destination on every raw mouse-move event — tens a
    /// second — and `find_path` is an A* search, not something to run that
    /// often. `go_to` must not touch the terrain at all when the step cadence
    /// has not freed up a step to plan for; only `take`, gated to once per step,
    /// may.
    #[test]
    fn restating_a_destination_mid_step_does_not_search_the_terrain() {
        let start = Instant::now();
        let mut steering = Steering::default();
        let terrain = CountingTerrain::default();

        steering
            .go_to(
                Tile::new(110, 100),
                here(),
                start,
                Direction::East,
                Ground::plain(&terrain),
            )
            .unwrap();
        let after_click = terrain.can_step_calls.get();
        assert!(after_click > 0, "the click itself plans a route");

        for tick in 1..20 {
            steering.go_to(
                Tile::new(110, 100 + tick as u16),
                here(),
                at(start, tick * 10),
                Direction::East,
                Ground::plain(&terrain),
            );
        }
        assert_eq!(
            terrain.can_step_calls.get(),
            after_click,
            "restating the destination between steps must not run a search"
        );
    }

    /// A wall this end cannot see is discovered by walking into it, and the walk
    /// gives up rather than shuffling against it for ever.
    #[test]
    fn a_destination_that_never_gets_closer_is_given_up_on() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .go_to(
                Tile::new(200, 100),
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        // The body never moves: every step is refused by the server, which
        // snaps it back to where it was. The click's own step is the first of
        // the four, so three more are tried after it.
        for step in 1..u64::from(STUCK_STEPS) {
            assert!(
                steering
                    .due(
                        at(start, 400 * step),
                        here(),
                        Direction::East,
                        Ground::plain(&OpenWorld)
                    )
                    .is_some(),
                "step {step} is still worth trying"
            );
        }
        assert_eq!(
            steering.due(
                at(start, 400 * u64::from(STUCK_STEPS)),
                here(),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            None
        );
        assert_eq!(steering.goal(), None, "and the destination is let go of");
    }

    /// A body that *is* making progress keeps its destination however long the
    /// walk takes.
    #[test]
    fn progress_resets_the_patience() {
        let start = Instant::now();
        let mut steering = Steering::default();
        steering
            .go_to(
                Tile::new(100, 130),
                here(),
                start,
                Direction::South,
                Ground::plain(&OpenWorld),
            )
            .unwrap();

        for y in 101..=120u16 {
            let now = at(start, 400 * u64::from(y - 100));
            // Every other step is refused, which is what a body squeezing past
            // furniture looks like — it must not add up to a stall.
            let position = Point::new(100, y - u16::from(y % 2 == 0), 0);
            assert!(
                steering
                    .due(now, position, Direction::South, Ground::plain(&OpenWorld))
                    .is_some(),
                "row {y}"
            );
        }
        assert_eq!(steering.goal(), Some(Tile::new(100, 130)));
    }

    /// Taking hold of the arrows is how a player says they no longer want to go
    /// where they clicked — and it takes effect at the next step, not at the
    /// press. The step already under way is the destination's last.
    #[test]
    fn the_keyboard_takes_over_from_a_destination() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .go_to(
                Tile::new(200, 100),
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        assert_eq!(
            steering.press(
                Direction::NorthWest,
                here(),
                at(start, 50),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            None,
            "the step under way is not cut short"
        );
        assert_eq!(steering.goal(), None, "but the destination is dropped at once");
        assert_eq!(
            steering.due(at(start, 399), here(), Direction::East, Ground::plain(&OpenWorld)),
            None
        );
        assert_eq!(
            steering.due(
                at(start, 400),
                here(),
                Direction::NorthWest,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::NorthWest)),
            "the keyboard's step, not the destination's"
        );
    }

    /// The queue rule, stated on its own: a press mid-step changes which way the
    /// step already owed will go, and moves nothing else.
    ///
    /// The complaint it comes from is a jump — `crowd.rs` glides from the tile
    /// the last step ended on, so a step issued half a hold early yanks the body
    /// to a tile it has not reached and takes the camera with it. The DST in
    /// `dst.rs` is what holds the picture to that; this is the rule at the unit.
    #[test]
    fn a_press_mid_step_waits_for_the_step_to_tick_out() {
        let start = Instant::now();
        let mut steering = Steering::default();

        assert_eq!(
            steering.press(
                Direction::East,
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East))
        );
        // Halfway across the tile, the player asks for the opposite direction.
        assert_eq!(
            steering.press(
                Direction::West,
                here(),
                at(start, 200),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            None
        );
        assert_eq!(
            steering.deadline(),
            Some(at(start, 400)),
            "the deadline the step already had, not an earlier one"
        );
        // And it is the *new* direction that leaves at it: the queue is one step
        // deep and every press rebuilds it.
        assert_eq!(
            steering.due(at(start, 400), here(), Direction::East, Ground::plain(&OpenWorld)),
            Some(Facing::walking(Direction::West))
        );
    }

    /// The mash: pressing every direction in turn, faster than a body walks,
    /// buys exactly one step per hold.
    ///
    /// A turn used to be the way through the floor — it costs the shard nothing,
    /// so it was sent the instant it was asked for and the step behind it went
    /// with it. Spinning through four arrows was therefore four steps in one
    /// frame, which the shard's `WalkPace` answers with a `0x21` and a body
    /// dragged back to where it really is.
    #[test]
    fn spinning_through_the_arrows_does_not_buy_a_step_each() {
        let start = Instant::now();
        let mut steering = Steering::default();
        let arrows = [
            Direction::East,
            Direction::South,
            Direction::West,
            Direction::North,
        ];

        steering
            .press(
                arrows[0],
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        for (tick, direction) in arrows.iter().cycle().take(30).enumerate() {
            let now = at(start, 10 * tick as u64 + 10);
            assert_eq!(
                steering.press(
                    *direction,
                    here(),
                    now,
                    Direction::East,
                    Ground::plain(&OpenWorld)
                ),
                None,
                "at {now:?}"
            );
            assert_eq!(
                steering.due(now, here(), Direction::East, Ground::plain(&OpenWorld)),
                None,
                "nor by asking the clock at {now:?}"
            );
        }
        assert_eq!(steering.deadline(), Some(at(start, 400)));
    }

    /// The mouse-heading counterpart to the mash above, and the regression a
    /// live corner exposed: a heading whose *resolved* direction keeps
    /// changing call to call — exactly what [`detour`] produces while
    /// sliding around an obstacle, delivered by a raw `CursorMoved` stream
    /// far faster than a hold — must not buy a step per change either.
    ///
    /// `steer`'s own `self.mouse == direction` gate does not catch this the
    /// way a held arrow key's repeat does, because the direction genuinely
    /// is different every call; only [`Steering::turned`] does. Before it
    /// existed, every one of the 30 calls below bought a step — a real body
    /// slid around a real corner noticeably faster than a straight walk.
    ///
    /// Stated at [`Turning::Immediate`], because that is the mode the free
    /// turn — and so the guard against a second one — exists in at all. The
    /// default charges every turn its own delay, which cannot buy a step by
    /// construction; `spinning_the_cursor_never_covers_ground_faster_than_a_walk`
    /// is that half.
    #[test]
    fn a_heading_that_keeps_changing_direction_does_not_buy_a_step_each() {
        let start = Instant::now();
        let mut steering = Steering::default();
        steering.set_turning(Turning::Immediate);
        // None of these is the facing the body is standing at below, so the
        // very first call is a genuine turn — the body starting to walk,
        // same as the arrow mash above — and not, by coincidence, already
        // the real step `next_due` would have paced on its own.
        let headings = [Direction::East, Direction::North, Direction::West];

        let mut sent = 0;
        for (tick, &direction) in headings.iter().cycle().take(30).enumerate() {
            let now = at(start, 5 * tick as u64);
            if steering
                .steer(
                    Some(Ask::Walk(Heading::centred(direction))),
                    here(),
                    now,
                    Direction::South,
                    Ground::plain(&OpenWorld),
                )
                .is_some()
            {
                sent += 1;
            }
        }
        // 30 changes in 145ms — well under one 400ms hold. The first is the
        // free turn every fresh ask gets; the second is the one direction
        // change `turned` still lets through paced like a real step, the
        // same "twice at most" ceiling `about_to_wait`'s loop holds `due` to.
        // Every change after that is refused until the hold it armed passes.
        assert_eq!(
            sent, 2,
            "a direction that kept changing bought steps past the ceiling"
        );
    }

    /// Letting go of the arrow does not refund the step being walked: a tapped
    /// key is a held key as far as the cadence is concerned.
    ///
    /// This is the other half of the mash. The clock used to be disarmed on the
    /// last release, so press-release-press was read as the first ask of a fresh
    /// walk and stepped at once — a step per tap, at whatever rate a finger can
    /// manage.
    #[test]
    fn a_release_does_not_refund_the_step_in_flight() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .press(
                Direction::East,
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        for tap in 1..8 {
            let now = at(start, 40 * tap);
            steering.release(Direction::East);
            assert_eq!(
                steering.press(
                    Direction::East,
                    here(),
                    now,
                    Direction::East,
                    Ground::plain(&OpenWorld)
                ),
                None,
                "tap {tap} bought a step"
            );
        }
        // And the floor is a floor: a walk that has genuinely stopped sets off on
        // the next press, in that instant.
        steering.release(Direction::East);
        assert_eq!(
            steering.press(
                Direction::East,
                here(),
                at(start, 2_000),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East))
        );
        assert_eq!(
            steering.deadline(),
            Some(at(start, 2_400)),
            "and the step after it is a whole hold away, measured from the press"
        );
    }

    /// The click that turns before it steps, which is the reference client's
    /// movement and the default here: a body facing north, asked for east,
    /// sends the turn now and covers the ground a `TURN_HOLD` later.
    ///
    /// ClassicUO's `PlayerMobile.Walk` is where the shape comes from — a
    /// request whose direction is not the one the body faces leaves `x`, `y`
    /// and `z` alone and charges `MovementSpeed.TurnDelay`, so the ground is
    /// only covered by the request after it.
    #[test]
    fn a_turn_is_a_step_of_its_own_and_the_walk_waits_it_out() {
        let start = Instant::now();
        let mut steering = Steering::default();
        let turn = TURN_HOLD.as_millis() as u64;

        // Facing north, asking east: this is the turn, and it is all it is.
        assert_eq!(
            steering.steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East))
        );
        assert_eq!(
            steering.deadline(),
            Some(at(start, turn)),
            "a turn's delay, not a whole hold and not nothing"
        );
        assert_eq!(
            steering.due(
                at(start, turn - 1),
                here(),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            None,
            "the body is squaring up; nothing else has come due"
        );
        // Facing east now — the shard has acked the turn — so this one is the
        // step the turn was for.
        assert_eq!(
            steering.due(
                at(start, turn),
                here(),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East))
        );
        assert_eq!(
            steering.deadline(),
            Some(at(start, turn + 400)),
            "and from there it is an ordinary walk"
        );
    }

    /// The turn ring, at this end of it: the body faces the way it was pointed
    /// and stays where it is, however long the button is held.
    ///
    /// One `0x02` — the turn itself, which the shard answers by turning the
    /// body and moving it nowhere — and then nothing at all. Not "nothing
    /// because a step was refused": a step is never asked for, so the terrain
    /// is never even consulted, and a turn toward a wall is as good as a turn
    /// toward a field.
    #[test]
    fn a_cursor_in_the_turn_ring_turns_the_body_and_walks_it_nowhere() {
        let start = Instant::now();
        let mut steering = Steering::default();
        // A wall exactly where the body would step if this were a walk: what
        // pins that no step is even considered.
        let wall = Wall {
            blocked: Tile::new(101, 100),
        };

        assert_eq!(
            steering.steer(
                Some(Ask::Turn(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::North,
                Ground::plain(&wall)
            ),
            Some(Facing::walking(Direction::East)),
            "the turn is sent; the body is facing north and was asked to face east"
        );
        // Held, at the walking pace, for a good while: the body is facing east
        // now and there is nothing left to say.
        for step in 1..20u64 {
            assert_eq!(
                steering.due(
                    at(start, 400 * step),
                    here(),
                    Direction::East,
                    Ground::plain(&wall)
                ),
                None,
                "step {step}: a turn-only ask sent something once it was already facing"
            );
        }
    }

    /// And the ring is not a stop: pushing the cursor out past it walks, and
    /// pulling it back in turns and stands again. The zone is part of the ask,
    /// so crossing it is a fresh one even when the bearing never changed.
    #[test]
    fn crossing_the_turn_ring_at_an_unchanged_bearing_is_a_fresh_ask() {
        let start = Instant::now();
        let mut steering = Steering::default();
        let east = Heading::centred(Direction::East);

        steering
            .steer(
                Some(Ask::Turn(east)),
                here(),
                start,
                Direction::North,
                Ground::plain(&OpenWorld),
            )
            .expect("the turn");
        // Facing east and still inside the ring: nothing.
        assert_eq!(
            steering.steer(
                Some(Ask::Turn(east)),
                here(),
                at(start, 400),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            None,
            "the same ask restated"
        );
        // The cursor is pushed out past the ring. Same bearing, different ask.
        assert_eq!(
            steering.steer(
                Some(Ask::Walk(east)),
                here(),
                at(start, 500),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East)),
            "out past the ring, the same bearing is a walk"
        );
    }

    /// The other mode, stated: the turn and the step it precedes leave in one
    /// wake, which is what `dst.rs`'s oracle is written against and what this
    /// client did before the reference's own delay was put back.
    #[test]
    fn an_immediate_turn_takes_its_step_in_the_same_wake() {
        let start = Instant::now();
        let mut steering = Steering::default();
        steering.set_turning(Turning::Immediate);

        assert_eq!(
            steering.steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East)),
            "the turn"
        );
        assert_eq!(
            steering.due(start, here(), Direction::East, Ground::plain(&OpenWorld)),
            Some(Facing::walking(Direction::East)),
            "and the step it was for, in the same instant"
        );
        assert_eq!(steering.deadline(), Some(at(start, 400)));
    }

    /// Fast rotation is the same rule at ClassicUO's own faster number, and
    /// nothing else about the walk changes with it.
    #[test]
    fn fast_rotation_only_shortens_the_turn() {
        let start = Instant::now();
        let mut steering = Steering::default();
        steering.set_turning(Turning::Fast);

        steering
            .steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::North,
                Ground::plain(&OpenWorld),
            )
            .expect("the turn");
        let turn = TURN_HOLD_FAST.as_millis() as u64;
        assert_eq!(steering.deadline(), Some(at(start, turn)));
        assert_eq!(
            steering.due(
                at(start, turn),
                here(),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East))
        );
        assert_eq!(steering.deadline(), Some(at(start, turn + 400)));
    }

    /// A turn is a step and is paced like one, so spinning the cursor round the
    /// body cannot be a way to move faster than a body walks: the ground is
    /// only ever covered by an ask that keeps the facing, and each of those is
    /// a whole hold apart.
    #[test]
    fn spinning_the_cursor_never_covers_ground_faster_than_a_walk() {
        let start = Instant::now();
        let mut steering = Steering::default();
        let headings = [
            Direction::East,
            Direction::South,
            Direction::West,
            Direction::North,
        ];

        // Facing north throughout: the shard is never told otherwise, so every
        // ask below that is not north is a turn and covers nothing.
        let mut ground = 0;
        for (tick, &direction) in headings.iter().cycle().take(200).enumerate() {
            let now = at(start, 5 * tick as u64);
            if let Some(step) = steering.steer(
                Some(Ask::Walk(Heading::centred(direction))),
                here(),
                now,
                Direction::North,
                Ground::plain(&OpenWorld),
            ) {
                // What the body was facing when this left — `asked` is set to
                // this step's own direction by then, so the caller's facing is
                // the honest one to measure against.
                if step.direction == Direction::North {
                    ground += 1;
                }
            }
        }
        // A second of spinning at 5ms an event. Two and a half walking steps is
        // the ceiling, and the run above is nowhere near even that.
        assert!(
            ground <= 2,
            "spinning the cursor covered ground {ground} times in a second of walking"
        );
    }

    /// A rollback makes the facing this end believed it had asked for a lie, and
    /// the shard's word replaces it. Without that, the step after a `0x21` is
    /// decided against a direction nobody is facing: it is timed as a turn when
    /// it is a step, or as a step when it is a turn.
    #[test]
    fn a_correction_replaces_the_facing_this_end_believed_in() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .press(
                Direction::East,
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        // The shard refuses it and says the body is still facing north.
        steering.corrected(Direction::North);
        // Facing north, asking east: a turn, so what leaves at the deadline is
        // the turn and the step it precedes is a turn's delay behind it — not a
        // whole hold, which is what it would be if the facing this end believed
        // in had survived the rollback.
        assert_eq!(
            steering.due(
                at(start, 400),
                here(),
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East))
        );
        assert_eq!(
            steering.due(
                at(start, 400),
                here(),
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            None,
            "the turn is a step of its own; the wake it left in owes nothing more"
        );
        assert_eq!(
            steering.due(
                at(start, 400 + TURN_HOLD.as_millis() as u64),
                here(),
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East)),
            "the step the turn was for"
        );
    }

    /// Losing focus lets go of everything, keyboard and destination alike.
    #[test]
    fn losing_focus_stops_the_walk() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .press(
                Direction::South,
                here(),
                start,
                Direction::South,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        steering.go_to(
            Tile::new(200, 200),
            here(),
            start,
            Direction::South,
            Ground::plain(&OpenWorld),
        );
        steering.clear();
        assert_eq!(steering.goal(), None);
        assert_eq!(steering.deadline(), None);
        assert_eq!(
            steering.due(
                at(start, 10_000),
                here(),
                Direction::South,
                Ground::plain(&OpenWorld)
            ),
            None
        );
    }

    /// The mouse heading is driven exactly like a held key: the first ask
    /// steps at once, and a restated-but-unchanged heading — most mouse-move
    /// events while the cursor sits still relative to the body — is not a
    /// fresh ask, the same as the operating system repeating a held key.
    #[test]
    fn a_heading_steps_at_once_and_a_restated_one_does_not_repeat() {
        let start = Instant::now();
        let mut steering = Steering::default();

        assert_eq!(
            steering.steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East))
        );
        assert_eq!(
            steering.steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                at(start, 10),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            None,
            "the same heading restated is not a fresh ask"
        );
        assert_eq!(steering.deadline(), Some(at(start, 400)));
    }

    /// A blocked cardinal has no legal diagonal past it at all (see `detour`'s
    /// doc), so a real trap needs the direction itself and both of its
    /// flanking cardinals blocked, not the diagonals — the inside corner of a
    /// building, with the body pushed at the corner.
    struct Boxed;
    impl openshard_movement::Terrain for Boxed {
        fn can_step(&self, from: Point, to: Point) -> Option<Point> {
            match (to.x, to.y) {
                (101, 100) | (100, 99) | (100, 101) => None,
                _ => OpenWorld.can_step(from, to),
            }
        }
    }

    /// Wedged in a corner and leaning on the key: the body turns to face it
    /// once, and after that nothing goes to the shard at all.
    ///
    /// It used to keep asking for the blocked direction every hold — "a
    /// heading never gives up", which is true about the *asking* and was
    /// wrong about the packet. Every one of those was a step this end had
    /// already proven the shard refuses, and the answer is a `0x21`: the body
    /// snapped back and the walk sequence reset, a hold at a time, for as
    /// long as the player leaned on the key. What a player sees is the
    /// character shuddering against the corner rather than standing in it.
    #[test]
    fn a_heading_into_a_corner_turns_once_and_then_sends_nothing() {
        let start = Instant::now();
        let mut steering = Steering::default();

        // Facing north, so the first ask is a genuine turn — which the shard
        // accepts, moves nothing, and is the feedback a player pressing into a
        // wall expects.
        assert_eq!(
            steering.steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::North,
                Ground::plain(&Boxed)
            ),
            Some(Facing::walking(Direction::East)),
            "the turn into the corner is legal and is what the body is drawn doing"
        );
        for step in 1..20u64 {
            assert_eq!(
                steering.due(
                    at(start, 400 * step),
                    here(),
                    Direction::East,
                    Ground::plain(&Boxed)
                ),
                None,
                "step {step}: facing it already, there is no step left that the shard would take"
            );
        }
    }

    /// And the asking itself is not given up on, which is what still separates
    /// a heading from a destination: nothing is *sent* while the corner is
    /// there, but the clock stays armed at the walking pace — one attempt a
    /// hold, not a spin on a deadline already passed — and the walk picks
    /// straight back up the moment the way opens, with no fresh input.
    #[test]
    fn a_heading_held_in_a_corner_walks_the_instant_the_way_opens() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::North,
                Ground::plain(&Boxed),
            )
            .expect("the turn");
        // The turn is a step of its own, so the retries are measured from the
        // end of it and not from the ask.
        let turn = TURN_HOLD.as_millis() as u64;
        assert_eq!(steering.deadline(), Some(at(start, turn)));
        for step in 0..3u64 {
            assert_eq!(
                steering.due(
                    at(start, turn + 400 * step),
                    here(),
                    Direction::East,
                    Ground::plain(&Boxed)
                ),
                None
            );
            assert_eq!(
                steering.deadline(),
                Some(at(start, turn + 400 * (step + 1))),
                "step {step}: paced like a walk, so the retry is not a spin"
            );
        }
        // The door opens, the crate is moved, the body in the way walks off.
        assert_eq!(
            steering.due(
                at(start, turn + 400 * 3),
                here(),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::East)),
            "nothing was asked for again; the heading was held the whole time"
        );
    }

    /// The whole point: a body running straight at a single-tile obstacle
    /// slides past it along the wall's face rather than stopping dead
    /// against it — the diagonal past a wall dead ahead is never legal (see
    /// `detour`'s doc), so this is the cardinal sidestep, not a diagonal.
    ///
    /// Right from the very first ask: [`Steering::steer`] and
    /// [`Steering::press`] take a `terrain` for exactly this — a player
    /// steering the mouse toward an obstacle sends a fresh ask on nearly
    /// every move as the cursor moves, and the *first* ask at a new heading
    /// is by far the common case while working a corner, not the rare one.
    #[test]
    fn a_held_heading_detours_around_a_single_tile_obstacle() {
        let start = Instant::now();
        let mut steering = Steering::default();
        // The wider leeway, stated: the default turns no more than an
        // eighth, and this scene is about the quarter turn.
        steering.set_leeway(Leeway::Quarter);
        // Directly in the heading's path; both cardinal sidesteps are open.
        let wall = Wall {
            blocked: Tile::new(101, 100),
        };

        let detoured = steering
            .steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::East,
                Ground::plain(&wall),
            )
            .expect("a sidestep is open");
        assert!(
            matches!(detoured.direction, Direction::North | Direction::South),
            "east is blocked; a cardinal sidestep is taken instead of standing against it, \
             got {:?}",
            detoured.direction
        );
        assert!(
            wall.can_step(here(), step_from(here(), detoured.direction).unwrap())
                .is_some(),
            "the direction taken must actually be open"
        );
    }

    /// The corner case proper: a diagonal held direction blocked by the
    /// building corner it points into slides onto whichever cardinal it
    /// splits into that is open — not a diagonal, which has nothing to flank
    /// here since `direction` already is one.
    #[test]
    fn a_held_diagonal_heading_detours_onto_an_open_cardinal() {
        let start = Instant::now();
        let mut steering = Steering::default();
        // The wider leeway, stated: the default turns no more than an
        // eighth, and this scene is about the quarter turn.
        steering.set_leeway(Leeway::Quarter);
        // North-east is blocked; north and east, the cardinals it splits
        // into, are both open — the detour must take one of them.
        let corner = Wall {
            blocked: Tile::new(101, 99),
        };

        let detoured = steering
            .steer(
                Some(Ask::Walk(Heading::centred(Direction::NorthEast))),
                here(),
                start,
                Direction::NorthEast,
                Ground::plain(&corner),
            )
            .expect("a flanking cardinal is open");
        assert!(
            matches!(detoured.direction, Direction::North | Direction::East),
            "north-east is blocked; a flanking cardinal is taken, got {:?}",
            detoured.direction
        );
        assert!(
            corner
                .can_step(here(), step_from(here(), detoured.direction).unwrap())
                .is_some(),
            "the direction taken must actually be open"
        );
    }

    /// The same single-tile obstacle, at the setting a fresh `Steering` has:
    /// the body stops against it and stays stopped, and — the part that
    /// matters on the wire — the step it will not take is not sent either.
    /// Standing against something is standing, not a refusal a hold.
    ///
    /// **This is what pins the default.** Deliberately without a
    /// `set_leeway` call of its own: a body only ever goes where it was
    /// pointed unless a player asks for otherwise, and a default that flipped
    /// by accident would be every walk in the game changing character with
    /// nothing to catch it. The sliding tests above state their preference
    /// outright for the same reason.
    ///
    /// `Steering::set_leeway` is the seam a client config will set, and
    /// the reason the preference is threaded at all rather than settled once
    /// in `common/movement`: both answers are correct play.
    #[test]
    fn a_heading_stops_at_an_obstacle_by_default_and_slides_only_when_asked() {
        let start = Instant::now();
        let mut steering = Steering::default();
        // Directly in the heading's path, with both sidesteps wide open — the
        // scene `Leeway::Quarter` answers with a sidestep.
        let wall = Wall {
            blocked: Tile::new(101, 100),
        };

        assert_eq!(
            steering.steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::North,
                Ground::plain(&wall)
            ),
            Some(Facing::walking(Direction::East)),
            "the turn to face what it walked into is still legal and still sent"
        );
        for step in 1..6u64 {
            assert_eq!(
                steering.due(
                    at(start, 400 * step),
                    here(),
                    Direction::East,
                    Ground::plain(&wall)
                ),
                None,
                "step {step}: stopped means stopped, and a refused step is not sent to say so"
            );
        }
        // And the setting is the only thing standing in the way: the same
        // heading, with the sidestep allowed, walks.
        steering.set_leeway(Leeway::Quarter);
        let slid = steering
            .due(at(start, 400 * 6), here(), Direction::East, Ground::plain(&wall))
            .expect("a sidestep is open");
        assert!(
            matches!(slid.direction, Direction::North | Direction::South),
            "got {:?}",
            slid.direction
        );
    }

    /// The corner the reports were actually about, and the one the two tests
    /// above could not see: the diagonal tile itself is perfectly steppable,
    /// and it is the *corner* that makes the step illegal — one of the two
    /// cardinals flanking it is a wall, so the body would be cutting the
    /// corner where that wall ends.
    ///
    /// `MapTerrain`, which is the terrain the client plans against, does not
    /// answer for that: `can_step` looks at the destination tile alone. So
    /// `detour` used to see the way ahead as open, keep asking for the
    /// diagonal, and have the shard — whose `LiveTerrain` *does* apply the
    /// corner rule — refuse every one of them. That is the stick: a body
    /// pressed against a building corner sending the same rejected diagonal
    /// every hold, never once trying the sidestep that walks straight past it.
    /// [`open`] asks [`step_allowed`] now, so the diagonal reads as blocked
    /// here exactly as it does on the wire, and the ordinary detour takes
    /// over.
    #[test]
    fn a_diagonal_that_cuts_a_wall_corner_sidesteps_instead_of_asking_for_it() {
        let start = Instant::now();
        let mut steering = Steering::default();
        // The wider leeway, stated: the default turns no more than an
        // eighth, and this scene is about the quarter turn.
        steering.set_leeway(Leeway::Quarter);
        // The south-east tile is open ground. Due east is the wall's last
        // tile, so a step south-east clips the corner where it ends —
        // refused on the wire, and `Wall` alone cannot tell.
        let corner = Wall {
            blocked: Tile::new(101, 100),
        };
        assert!(
            corner
                .can_step(here(), step_from(here(), Direction::SouthEast).unwrap())
                .is_some(),
            "the tile itself is steppable — the corner rule is the only thing refusing it"
        );

        let detoured = steering
            .steer(
                Some(Ask::Walk(Heading::centred(Direction::SouthEast))),
                here(),
                start,
                Direction::SouthEast,
                Ground::plain(&corner),
            )
            .expect("south is open");
        assert_eq!(
            detoured.direction,
            Direction::South,
            "the corner is cut by south-east and east is the wall: one step to the side is \
             what gets past it"
        );
    }

    /// The doorway found live: a fixed rotation order alone flip-flops
    /// between two tiles forever when the tie-break's default flank is only
    /// open going backward at one of them — `Steering::last_detour` is what
    /// stops it from re-trying the flank it already found doesn't lead
    /// anywhere new, and keeps taking the one that does.
    #[test]
    fn a_repeated_detour_prefers_the_flank_it_already_took() {
        let start = Instant::now();
        let mut steering = Steering::default();
        // The wider leeway, stated: the default turns no more than an
        // eighth, and this scene is about the quarter turn.
        steering.set_leeway(Leeway::Quarter);
        // East is walled the whole way; south of the start tile is also
        // blocked, so the very first detour is forced onto north — the
        // non-default flank. From there, both south (back to the start) and
        // north (onward) are open: a fixed South-first order would try south
        // every single time and bounce between the two tiles forever, which
        // is exactly the doorway this was found against.
        struct Doorway;
        impl openshard_movement::Terrain for Doorway {
            fn can_step(&self, from: Point, to: Point) -> Option<Point> {
                match (to.x, to.y) {
                    (101, _) | (100, 101) => None,
                    _ => OpenWorld.can_step(from, to),
                }
            }
        }

        let first = steering
            .steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::East,
                Ground::plain(&Doorway),
            )
            .expect("north is open");
        assert_eq!(
            first.direction,
            Direction::North,
            "south is blocked at the start tile; north is the only way out"
        );

        let mut pos = step_from(here(), first.direction).unwrap();
        let mut now = start;
        for step in 1..4u32 {
            now = at(now, u64::from(step) * WALK_HOLD.as_millis() as u64);
            let facing = steering
                .due(now, pos, Direction::East, Ground::plain(&Doorway))
                .unwrap_or_else(|| panic!("step {step}: north keeps being open"));
            assert_eq!(
                facing.direction,
                Direction::North,
                "step {step}: having taken north once, it is preferred over re-trying south"
            );
            pos = step_from(pos, facing.direction).unwrap();
        }
    }

    /// Every one of the eight held directions, run at every shape of
    /// single-tile obstacle directly ahead of it, either takes a legal
    /// sidestep or — when genuinely boxed in — keeps asking the blocked
    /// direction rather than ever proposing a step the server's corner rule
    /// would refuse. This is the regression a hand-picked wall tile cannot
    /// promise on its own: every direction gets its own geometry, not one
    /// direction exercised eight times over.
    #[test]
    fn every_direction_detours_legally_or_stands_never_cutting_a_corner() {
        let start = Instant::now();

        for &direction in &Direction::ALL {
            let (dx, dy) = direction.step();
            let ahead = Tile::new(
                (i32::from(here().x) + dx) as u16,
                (i32::from(here().y) + dy) as u16,
            );
            struct AheadBlocked {
                ahead: Tile,
            }
            impl openshard_movement::Terrain for AheadBlocked {
                fn can_step(&self, from: Point, to: Point) -> Option<Point> {
                    match Tile::new(to.x, to.y) == self.ahead {
                        true => None,
                        false => OpenWorld.can_step(from, to),
                    }
                }
            }
            let terrain = AheadBlocked { ahead };

            let mut steering = Steering::default();
            // The wider leeway, stated: the default turns no more than an
            // eighth, and this scene is about the quarter turn.
            steering.set_leeway(Leeway::Quarter);
            let answer = steering
                .steer(
                    Some(Ask::Walk(Heading::centred(direction))),
                    here(),
                    start,
                    direction,
                    Ground::plain(&terrain),
                )
                .unwrap_or_else(|| panic!("{direction:?}: a heading never gives up, even on the first ask"));

            let to = step_from(here(), answer.direction)
                .unwrap_or_else(|| panic!("{direction:?}: {:?} left the map", answer.direction));
            assert!(
                terrain.can_step(here(), to).is_some(),
                "{direction:?}: proposed {:?}, which the terrain (and so the server) refuses",
                answer.direction
            );
        }
    }

    /// Letting go of the mouse stops the heading at once — unlike a
    /// destination, which keeps walking itself there after the click that gave
    /// it is over.
    #[test]
    fn releasing_the_mouse_stops_the_heading_but_not_the_keyboard() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .press(
                Direction::North,
                here(),
                start,
                Direction::North,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        // Queued behind the keyboard's own step, same as any other input
        // mid-step — see the queue rule in the module docs.
        assert_eq!(
            steering.steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                at(start, 10),
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            None
        );
        steering.mouse_up();
        assert_eq!(
            steering.due(
                at(start, 400),
                here(),
                Direction::North,
                Ground::plain(&OpenWorld)
            ),
            Some(Facing::walking(Direction::North)),
            "the keyboard is untouched by the mouse letting go"
        );
    }

    /// The keyboard outranks the mouse: pressing an arrow drops a heading in
    /// flight the same way it drops a destination.
    #[test]
    fn the_keyboard_takes_over_from_a_heading() {
        let start = Instant::now();
        let mut steering = Steering::default();

        steering
            .steer(
                Some(Ask::Walk(Heading::centred(Direction::East))),
                here(),
                start,
                Direction::East,
                Ground::plain(&OpenWorld),
            )
            .unwrap();
        assert_eq!(
            steering.press(
                Direction::West,
                here(),
                at(start, 50),
                Direction::East,
                Ground::plain(&OpenWorld)
            ),
            None,
            "the step under way is not cut short"
        );
        assert_eq!(
            steering.due(at(start, 400), here(), Direction::West, Ground::plain(&OpenWorld)),
            Some(Facing::walking(Direction::West)),
            "the keyboard's heading, not the mouse's"
        );
    }
}
