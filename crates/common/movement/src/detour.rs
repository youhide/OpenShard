//! Getting past what is directly in the way, without planning a route.
//!
//! # What this is, and what `find_path` is
//!
//! A body walking a *heading* — an arrow key, a held mouse direction, a
//! creature told to close on something — has no route to fall back on when the
//! tile ahead is shut. `find_path` answers a different question, and answers it
//! at a different price: it needs a destination, and it searches. A heading has
//! neither. What it has is the ground it is standing next to, and the answer to
//! "can I still move at all" is in that ground alone.
//!
//! So this is not a planner and it is not a fallback for one. It is the rule a
//! body uses to brush past furniture: try the way you were going; failing that,
//! the nearest way round; failing that, do not move.
//!
//! # Four tiles decide it
//!
//! The scene is [`Around`], and it is exactly four tiles: the one being stood
//! on, the one the intent points at, and the two the body could slide onto
//! instead. Not the whole eight-neighbourhood, and not by accident — which two
//! flanks are candidates is fixed by the intent, and the other three neighbours
//! cannot change the answer:
//!
//! - **The intent is a cardinal** — a wall dead ahead. There is no diagonal
//!   past it *at all*: a diagonal step may not cut the corner where two
//!   blockers meet (see [`step_allowed`]), both cardinals flanking it must be
//!   open, and the blocked intent is unconditionally one of those two for
//!   either diagonal beside it. So neither diagonal can ever pass, and the
//!   candidates are the two cardinals at ninety degrees — a step along the
//!   wall's face, which is what a body hugging a wall actually does.
//! - **The intent is a diagonal** — a corner, not a wall. The two cardinals it
//!   splits into have no corner of their own to cut, so those are the
//!   candidates.
//!
//! Either way: two candidates, one intended tile, and where you stand. Four.
//! That is the whole input, which is why [`Around::new`] can state a scene
//! outright and every case of this can be enumerated rather than sampled.
//!
//! # How far a body may be turned, and by what
//!
//! Two things decide the answer besides the ground: [`Leeway`], how far off the
//! ask a body may be turned at all, and [`Lean`], which side of the ask the
//! pointing was actually on.
//!
//! The turn sizes are not a spectrum — there are exactly two, because the
//! flanks are fixed. An eighth (45°) is what a blocked diagonal splits onto,
//! and it is a body rounding a corner: it is always allowed, because refusing
//! it is a character stopping dead at the edge of a house it was walking past.
//! A quarter (90°) is the only thing a blocked cardinal has, and it puts the
//! body travelling at right angles to what was asked — defensible, and a
//! surprise, so it is what [`Leeway::Quarter`] opts into.
//!
//! The lean is what settles a tie the terrain cannot. Both flanks open, no
//! reason in the ground to prefer either — but the player pointing a little to
//! one side of the corner has already said which way round they mean to go,
//! and the eight sectors threw that away before this ever saw it. See
//! [`Detour::step`] for the order the three tie-breaks come in.
//!
//! # Three states: walking, sliding, standing
//!
//! [`Detour`] is the machine, and its states are what a body is doing about
//! what is in front of it. Two of them are moving — freely, or along the face
//! of something — and the third is not moving at all, which is a real thing a
//! body does and not an error. See [`Detour::Standing`] for why it is a state
//! rather than only an answer.
//!
//! The memory in [`Detour::Sliding`] — which flank got past the last obstacle —
//! exists because the tie-break between two open flanks has to be *stable
//! across tiles*, not merely deterministic. A fixed order — always the
//! clockwise one first — is deterministic and still loops: at a doorway or a
//! building corner the two flanks alternate which one is open from one tile to
//! the next, so tile A sends the body to tile B by its only open flank and tile
//! B sends it back to A by its only open flank, forever. A live corner did
//! exactly that for a second and a half before breaking out by chance.
//!
//! Remembering the flank that worked and preferring it again breaks the cycle
//! the moment either tile stops *requiring* the other one specifically — the
//! common case, since the two only disagree at a real pinch point. The memory
//! is dropped as soon as the intent stops being blocked, so an obstacle met
//! later is never biased by an unrelated one met before.

use openshard_protocol::direction::Direction;
use openshard_protocol::world::Point;

use crate::footing::Footing;
use crate::walk::{
    Heading,
    Lean,
    step_allowed,
};

/// What is open around a body, as far as one step in one intended direction can
/// tell: the intended tile and the two flanks that could take its place.
///
/// See the module docs for why those two flanks, and why nothing else about the
/// neighbourhood is here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Around {
    /// What is being asked for: the sector, and where inside it the ask
    /// actually pointed. The lean is only ever read to break a tie between two
    /// flanks that are both open — see [`Detour::step`].
    intent: Heading,
    /// Whether the tile [`Around::intent`] points at can be stepped onto.
    ahead:  bool,
    /// The two directions the body could slide onto instead, clockwise of the
    /// intent first, and whether each is open. Which two they are is
    /// [`flanks`]'s answer, and it depends on the intent.
    flanks: [(Direction, bool); 2],
}

impl Around {
    /// Read the four tiles from the world.
    ///
    /// [`step_allowed`] and not [`can_step`](crate::can_step), for every one of
    /// them: that answers for the destination tile alone, and a diagonal that cuts
    /// a wall's corner is refused on top of that. Asking the terrain directly
    /// here is what once had a client believing a corner-cutting diagonal was
    /// open, sending it, and being rolled back for as long as the player held
    /// the key.
    #[must_use]
    pub fn read(footing: &Footing<'_>, from: Point, intent: Heading) -> Self {
        let open = |direction| step_allowed(footing, from, direction).is_some();
        Self {
            intent,
            ahead: open(intent.direction),
            flanks: flanks(intent.direction).map(|flank| (flank, open(flank))),
        }
    }

    /// The same scene stated outright, for a caller that already knows what is
    /// around it — and for enumerating every scene there is, which is what
    /// makes this rule testable exhaustively rather than at a handful of
    /// hand-drawn walls.
    ///
    /// `clockwise` and `counter` are the two flanks in the order [`flanks`]
    /// gives them; which directions those are is the intent's business, not the
    /// caller's.
    #[must_use]
    pub const fn new(intent: Heading, ahead: bool, clockwise: bool, counter: bool) -> Self {
        let [cw, ccw] = flanks(intent.direction);
        Self {
            intent,
            ahead,
            flanks: [(cw, clockwise), (ccw, counter)],
        }
    }
}

/// Where a body actually goes, given where it wanted to go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing was in the way. The direction is the intent itself.
    Ahead(Direction),
    /// The intent was blocked and this flank was not: one step to the side,
    /// along the face of whatever is in the way.
    Aside(Direction),
    /// Neither the intent nor either flank is open — the inside corner of a
    /// building, with the body pushed at the corner. There is no step.
    ///
    /// Which is not the same as "ask anyway and let the server sort it out". A
    /// step the caller has already proven will be refused is answered with a
    /// rollback, and a rollback a hold is a body shuddering in a corner rather
    /// than standing in one. What a caller may still do with this is *turn*:
    /// turning costs no ground and no shard refuses it.
    Stuck,
}

/// How far a body may turn off the way it was pointed, to keep moving past
/// something in the way.
///
/// A preference and not a rule — both answers are correct play, and which one a
/// shard wants is not something this crate can know. What is *not* a preference
/// is that some turn is allowed: a body that stops dead at every obstacle is a
/// body that cannot get round a barrel, and a heading is an ask to keep moving.
///
/// The two sizes are the only two there are, because the flanks a blocked
/// direction can be answered with are fixed (see the module docs): an eighth of
/// the compass for a blocked diagonal, a quarter for a blocked cardinal.
///
/// It is passed to [`Detour::step`] per call rather than kept anywhere, so
/// whoever owns the setting owns it — today a default, and a shard's config
/// when there is one to read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Leeway {
    /// An eighth of the compass, and no more: 45°.
    ///
    /// A diagonal blocked by a corner still resolves onto the cardinal it
    /// splits into — that is the small correction a body makes rounding a
    /// corner, and refusing it is a character stopping dead at the edge of a
    /// house it was walking past. A wall dead ahead has no eighth-turn past it
    /// at all (there is no legal diagonal past a blocked cardinal, see the
    /// module docs), so walking straight into a wall stops the body, which is
    /// what the classic client does.
    ///
    /// The default: the smallest turn that still lets a body walk round
    /// things, and no larger one that could take it somewhere its player never
    /// pointed.
    #[default]
    Eighth,
    /// A quarter, 90°: also slide along the face of a wall walked into head-on.
    ///
    /// Keeps a runner running along a building rather than stopping at it. It
    /// is the bigger surprise of the two, though — the body ends up travelling
    /// at right angles to the ask — so it is the one a player opts into.
    Quarter,
}

/// How a body is getting along with what is in front of it: walking freely,
/// sliding along something, or standing because there is nowhere to go.
///
/// Three states, and the third is not bookkeeping. **Not moving is one of the
/// things a body does**, and it is a different thing from moving freely — a
/// machine that says `Clear` for both is telling the caller that nothing was in
/// the way while the body is wedged in the corner of a building. It was written
/// that way first, and every question worth asking of it afterwards ("is this
/// walk getting anywhere", "why was nothing sent") had to be answered by
/// re-deriving the scene, because the state had thrown the answer away.
///
/// The transitions are decided entirely by the scene handed to
/// [`Detour::step`]; see the module docs for why the memory in
/// [`Detour::Sliding`] is here at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detour {
    /// Walking freely: nothing was in the way the last time this was asked, so
    /// nothing is owed to what got past it.
    #[default]
    Clear,
    /// Sliding along something, on the flank that got past it — preferred again
    /// while the obstacle lasts.
    Sliding(Direction),
    /// Standing: the intent is blocked and so is every flank of it. The inside
    /// corner of a building, with the body pushed at the corner.
    ///
    /// A state and not just an answer, because it *persists* — the player is
    /// still leaning on the key and every beat asks again — and because what a
    /// caller does about it differs from what it does about a step. It is left
    /// the moment the scene offers anything, which is what makes a walk pick
    /// itself back up when a door opens with no fresh input.
    Standing,
}

impl Detour {
    /// Where to actually step, given what is around, what was wanted, and how
    /// far this shard's players have said a body may be turned to keep it
    /// moving.
    ///
    /// The transitions, in full. An open intent goes to [`Detour::Clear`] —
    /// whatever was being slid along is behind the body now, and biasing the
    /// next obstacle by it would be memory of the wrong thing. A blocked intent
    /// with an open flank goes to [`Detour::Sliding`] on that flank, when the
    /// turn onto it is one `leeway` allows. Nothing open, or nothing allowed,
    /// goes to [`Detour::Standing`]: there is no slide to remember, and no
    /// pretending there was nothing in the way either.
    ///
    /// # Which flank, when both are open
    ///
    /// Three tie-breaks, in this order, and the order is the point:
    ///
    /// 1. **Where the ask actually pointed** — [`Lean`]. A player holding the
    ///    cursor a little to one side of a corner has *said* which way round it
    ///    they mean to go, and the eight sectors threw that away before this
    ///    ever saw it. It is the freshest and most specific thing there is, so
    ///    it wins.
    /// 2. **The flank already being slid along** — [`Detour::Sliding`]. Nothing
    ///    was said, so keep doing what was working; see the module docs for the
    ///    two-tile loop this exists to break.
    /// 3. **Clockwise**, arbitrarily, because something has to be.
    ///
    /// A lean beating the memory is not a hole in the loop-breaking: the loop
    /// needs both flanks open at both tiles, and a lean that says the same
    /// thing at both — which is a player pointing steadily one way round an
    /// obstacle, and going that way is obeying them, not looping.
    ///
    /// `leeway` is a parameter and not a field on purpose: it is a setting, the
    /// state is a state, and putting a preference inside a machine makes "what
    /// is this body doing" and "what has its player asked for" one value that
    /// cannot be reasoned about separately.
    pub fn step(&mut self, around: &Around, leeway: Leeway) -> Step {
        if around.ahead {
            *self = Self::Clear;
            return Step::Ahead(around.intent.direction);
        }
        // A quarter turn is what a blocked *cardinal*'s flanks are, and a body
        // held to an eighth may not take one. A blocked diagonal's flanks are
        // eighths and are always allowed — that is a body rounding a corner,
        // not a body being sent somewhere else.
        if leeway == Leeway::Eighth && !around.intent.direction.is_diagonal() {
            *self = Self::Standing;
            return Step::Stuck;
        }
        let [cw, ccw] = around.flanks;
        let ordered = match (around.intent.lean, *self) {
            (Lean::Clockwise, _) => [cw, ccw],
            (Lean::Counter, _) => [ccw, cw],
            (Lean::Centred, Self::Sliding(preferred)) if preferred == ccw.0 => [ccw, cw],
            (Lean::Centred, _) => [cw, ccw],
        };
        for (direction, open) in ordered {
            if open {
                *self = Self::Sliding(direction);
                return Step::Aside(direction);
            }
        }
        *self = Self::Standing;
        Step::Stuck
    }

    /// The walk stopped — the key came up, the window lost focus. Forget both
    /// the flank and the corner, so a heading picked up later somewhere else
    /// starts from the fixed order and from no assumption about being stuck.
    pub fn forget(&mut self) {
        *self = Self::Clear;
    }
}

/// The two directions a blocked `intent` may be answered with, clockwise first.
///
/// A cardinal's are the cardinals at ninety degrees and a diagonal's are the
/// two cardinals it splits into — never a diagonal in either case. The module
/// docs have the argument; the short of it is that there is no diagonal past a
/// wall dead ahead, because the blocked tile is itself a flank of both
/// diagonals beside it and a diagonal needs both of its flanks open.
const fn flanks(intent: Direction) -> [Direction; 2] {
    let bits = intent.to_bits();
    let turn = match intent.is_diagonal() {
        true => 1,
        false => 2,
    };
    [
        Direction::from_bits(bits + turn),
        Direction::from_bits(bits + 8 - turn),
    ]
}

#[cfg(test)]
mod tests {
    use openshard_map::grid::Tile;
    use openshard_map::overlay::{
        Cover,
        Doors,
        Overlay,
    };

    use super::*;

    /// Every state the machine can be in, for the enumerations below: the two
    /// slides are both flanks, because which one is remembered is exactly what
    /// the tie-break turns on.
    fn every_state(intent: Direction) -> [Detour; 4] {
        let [cw, ccw] = flanks(intent);
        [
            Detour::Clear,
            Detour::Sliding(cw),
            Detour::Sliding(ccw),
            Detour::Standing,
        ]
    }

    /// Every scene there is, at every intent, from every state the machine can
    /// be in: 8 directions x 8 open/shut combinations x 4 states. The claim is
    /// the one that matters on the wire — **what comes back is never a
    /// direction the scene says is shut** — and it is checked by enumeration
    /// rather than by drawing walls and hoping the interesting one was drawn.
    #[test]
    fn no_scene_at_any_intent_is_ever_answered_with_a_shut_direction() {
        for &intent in &Direction::ALL {
            let [cw, ccw] = flanks(intent);
            for scene in 0..8u8 {
                let (ahead, clockwise, counter) = (scene & 1 != 0, scene & 2 != 0, scene & 4 != 0);
                let around = Around::new(Heading::centred(intent), ahead, clockwise, counter);
                for mut detour in every_state(intent) {
                    let open = |direction| {
                        match direction {
                            d if d == intent => ahead,
                            d if d == cw => clockwise,
                            _ => counter,
                        }
                    };
                    match detour.step(&around, Leeway::Quarter) {
                        Step::Ahead(direction) => {
                            assert_eq!(direction, intent, "{intent:?}/{scene}: not the intent");
                            assert!(ahead, "{intent:?}/{scene}: walked into a shut tile");
                        }
                        Step::Aside(direction) => {
                            assert!(
                                direction == cw || direction == ccw,
                                "{intent:?}/{scene}: {direction:?} is not a flank of the intent"
                            );
                            assert!(open(direction), "{intent:?}/{scene}: slid onto a shut tile");
                            assert!(!ahead, "{intent:?}/{scene}: slid aside with the way open");
                        }
                        Step::Stuck => {
                            assert!(
                                !ahead && !clockwise && !counter,
                                "{intent:?}/{scene}: gave up with somewhere to go"
                            )
                        }
                    }
                }
            }
        }
    }

    /// And the state it is left in says which of those three happened, from
    /// every state, at every scene, under either preference. Not bookkeeping: a
    /// machine that answered `Stuck` and then called itself `Clear` was
    /// claiming nothing had been in the way of a body wedged in a corner — the
    /// one question the state exists to answer, answered wrong. Standing is a
    /// state a body is *in*, for as long as the player leans on the key.
    #[test]
    fn the_state_left_behind_says_which_of_the_three_the_body_is_doing() {
        for &intent in &Direction::ALL {
            for scene in 0..8u8 {
                let (ahead, clockwise, counter) = (scene & 1 != 0, scene & 2 != 0, scene & 4 != 0);
                let around = Around::new(Heading::centred(intent), ahead, clockwise, counter);
                for when in [Leeway::Quarter, Leeway::Eighth] {
                    for mut detour in every_state(intent) {
                        let was = detour;
                        let step = detour.step(&around, when);
                        let expected = match step {
                            Step::Ahead(_) => Detour::Clear,
                            Step::Aside(direction) => Detour::Sliding(direction),
                            Step::Stuck => Detour::Standing,
                        };
                        assert_eq!(
                            detour, expected,
                            "{intent:?}/{scene}/{when:?} from {was:?}: answered {step:?} and calls itself {detour:?}"
                        );
                    }
                }
            }
        }
    }

    /// The default leeway, whole: an eighth of the compass and not one degree
    /// more, at every scene, from every state.
    ///
    /// Which is two claims, and the split between them is the whole of what
    /// this setting means. A **diagonal** is a body rounding a corner: the
    /// flank is 45° off, so it is taken whenever it is open — refusing it
    /// would be a character stopping dead at the edge of a house it was
    /// walking past, which is what "stops too aggressively" was. A
    /// **cardinal** is a wall dead ahead: its only flanks are 90° off, the
    /// body would end up travelling at right angles to the ask, and nobody
    /// asked for that — so it stops.
    #[test]
    fn an_eighth_of_leeway_rounds_a_corner_and_stops_at_a_wall() {
        for &intent in &Direction::ALL {
            let [cw, ccw] = flanks(intent);
            for scene in 0..8u8 {
                let (ahead, clockwise, counter) = (scene & 1 != 0, scene & 2 != 0, scene & 4 != 0);
                let around = Around::new(Heading::centred(intent), ahead, clockwise, counter);
                for mut detour in every_state(intent) {
                    let was = detour;
                    let step = detour.step(&around, Leeway::Eighth);
                    // Which of two open flanks is taken is the tie-break's
                    // business and has its own tests; what this one claims is
                    // whether a flank may be taken at all.
                    let taken = match step {
                        Step::Aside(cw_or_ccw) => Some(cw_or_ccw),
                        _ => None,
                    };
                    let expected = match (ahead, intent.is_diagonal()) {
                        (true, _) => (Some(Step::Ahead(intent)), None),
                        // The corner: an eighth turn onto whichever cardinal
                        // the diagonal splits into is open.
                        (false, true) if clockwise => (None, Some(cw)),
                        (false, true) if counter => (None, Some(ccw)),
                        // The wall: a quarter turn is the only thing on offer,
                        // and it is more than was asked for.
                        (false, _) => (Some(Step::Stuck), None),
                    };
                    match expected {
                        (Some(exact), _) => {
                            assert_eq!(
                                step, exact,
                                "{intent:?}/{scene} from {was:?}: an eighth of leeway is a corner, not a wall"
                            )
                        }
                        (None, flank) => {
                            assert!(
                                taken == flank || taken == Some(if flank == Some(cw) { ccw } else { cw }),
                                "{intent:?}/{scene} from {was:?}: {step:?} is not a flank of the corner"
                            )
                        }
                    }
                    // And whichever it was, it was open.
                    if let Some(direction) = taken {
                        let open = if direction == cw { clockwise } else { counter };
                        assert!(open, "{intent:?}/{scene}: {direction:?} is shut");
                    }
                }
            }
        }
    }

    /// The two leeways differ in exactly one place — a blocked cardinal — and
    /// agree everywhere else. Stated as its own claim because it is easy to
    /// widen a setting by accident and hard to notice: every scene, both
    /// answers, and the only permitted disagreement is the wall.
    #[test]
    fn the_leeways_differ_only_at_a_wall_dead_ahead() {
        for &intent in &Direction::ALL {
            for scene in 0..8u8 {
                let (ahead, clockwise, counter) = (scene & 1 != 0, scene & 2 != 0, scene & 4 != 0);
                let around = Around::new(Heading::centred(intent), ahead, clockwise, counter);
                for state in every_state(intent) {
                    let (mut eighth, mut quarter) = (state, state);
                    let a = eighth.step(&around, Leeway::Eighth);
                    let b = quarter.step(&around, Leeway::Quarter);
                    let wall = !ahead && !intent.is_diagonal();
                    assert_eq!(
                        a == b,
                        !(wall && (clockwise || counter)),
                        "{intent:?}/{scene} from {state:?}: {a:?} against {b:?}"
                    );
                }
            }
        }
    }

    /// The sub-sector detail the eight directions throw away, put back: a
    /// player holding the cursor a little to one side of a corner has said
    /// which way round it they mean to go, and with both ways open that is the
    /// only thing that knows.
    ///
    /// The scene is deliberately symmetric — a diagonal blocked, both cardinals
    /// it splits into wide open — so nothing in the terrain can decide it and
    /// the answer is the lean or nothing.
    #[test]
    fn a_lean_past_a_corner_picks_the_side_it_leans_to() {
        let intent = Direction::SouthEast;
        let [cw, ccw] = flanks(intent);
        let corner = |lean| {
            Around::new(
                Heading {
                    direction: intent,
                    lean,
                },
                false,
                true,
                true,
            )
        };

        assert_eq!(
            Detour::default().step(&corner(Lean::Clockwise), Leeway::Eighth),
            Step::Aside(cw),
            "pointing past the corner clockwise is asking to go round it that way"
        );
        assert_eq!(
            Detour::default().step(&corner(Lean::Counter), Leeway::Eighth),
            Step::Aside(ccw)
        );

        // And it outranks the memory: a body that slid one way and is then
        // pointed the other goes the way it is being pointed. The memory is
        // for when nothing was said (a held arrow key, a cursor squarely on
        // the diagonal), not for overruling what was.
        let mut detour = Detour::Sliding(ccw);
        assert_eq!(
            detour.step(&corner(Lean::Clockwise), Leeway::Eighth),
            Step::Aside(cw)
        );
        // Nothing said: the memory is what is left, and it holds.
        let mut detour = Detour::Sliding(ccw);
        assert_eq!(
            detour.step(&corner(Lean::Centred), Leeway::Eighth),
            Step::Aside(ccw)
        );
    }

    /// The lean itself, against the flanks it decides between: leaning toward
    /// a flank must name *that* flank, on every direction, both ways round.
    ///
    /// The two are derived separately — [`Lean::of`] from a cross product,
    /// [`flanks`] from the direction's bits — and a sign convention that
    /// disagreed between them would send a body round the far side of every
    /// obstacle, which is a bug no scene-shaped test would call wrong: both
    /// answers are legal steps.
    #[test]
    fn leaning_toward_a_flank_names_that_flank() {
        for &intent in &Direction::ALL {
            let [cw, ccw] = flanks(intent);
            for (flank, expected) in [(cw, Lean::Clockwise), (ccw, Lean::Counter)] {
                // A vector a long way along the intent and a little way along
                // the flank: unambiguously in the intent's sector, and leaning.
                let (ix, iy) = intent.step();
                let (fx, fy) = flank.step();
                let (dx, dy) = (ix * 8 + fx, iy * 8 + fy);
                assert_eq!(
                    Lean::of(ix, iy, dx, dy),
                    expected,
                    "{intent:?}: a bearing pulled toward {flank:?} must lean that way"
                );
            }
            // And squarely along the intent leans neither way — exactly, which
            // is what `Lean::of`'s integer arithmetic is for.
            let (ix, iy) = intent.step();
            assert_eq!(Lean::of(ix, iy, ix * 9, iy * 9), Lean::Centred);
        }
    }

    /// And the preference is read at the call, not baked in when the machine
    /// was made: a shard that changes the setting under a walk in progress —
    /// which is what a config reload is — takes effect on the next step, with
    /// no state to reset.
    #[test]
    fn the_preference_takes_effect_on_the_next_step() {
        let wall = Around::new(Heading::centred(Direction::East), false, true, true);
        let mut detour = Detour::default();

        assert!(matches!(detour.step(&wall, Leeway::Quarter), Step::Aside(_)));
        assert_eq!(detour.step(&wall, Leeway::Eighth), Step::Stuck);
        assert_eq!(detour, Detour::Standing);
        assert!(matches!(detour.step(&wall, Leeway::Quarter), Step::Aside(_)));
    }

    /// A cardinal intent is never answered with a diagonal, whatever the scene.
    /// This is the one that is not merely "a legal tile": the tile beyond a
    /// wall's corner can be perfectly good ground, and the step onto it is
    /// still refused because of the corner it cuts. A rule that read the tiles
    /// alone would offer it.
    #[test]
    fn a_wall_dead_ahead_is_never_answered_with_a_diagonal() {
        for intent in Direction::ALL.iter().filter(|d| !d.is_diagonal()) {
            for flank in flanks(*intent) {
                assert!(
                    !flank.is_diagonal(),
                    "{intent:?} offered {flank:?}, which would cut the corner it is walled against"
                );
            }
        }
        // And the diagonal case is the mirror image: its flanks are the two
        // cardinals it splits into, which have no corner of their own to cut.
        for intent in Direction::ALL.iter().filter(|d| d.is_diagonal()) {
            for flank in flanks(*intent) {
                assert!(!flank.is_diagonal(), "{intent:?} offered another diagonal");
            }
        }
    }

    /// The doorway, as a scene rather than as a map: the same intent blocked at
    /// two tiles in a row, each of which opens the *other* flank. A fixed order
    /// takes the clockwise one at the first and the clockwise one at the second
    /// — which walks straight back — and repeats forever. The memory takes the
    /// flank that worked and keeps taking it.
    #[test]
    fn a_pinch_point_that_alternates_its_flanks_does_not_flip_flop() {
        let intent = Direction::East;
        let [cw, ccw] = flanks(intent);
        let mut detour = Detour::default();

        // First tile: only the counter-clockwise flank is open, so that is
        // forced whatever the tie-break would have preferred.
        assert_eq!(
            detour.step(
                &Around::new(Heading::centred(intent), false, false, true),
                Leeway::Quarter
            ),
            Step::Aside(ccw)
        );
        assert_eq!(detour, Detour::Sliding(ccw));
        // Second tile: *both* flanks are open. The fixed order would take the
        // clockwise one, which is the way back.
        for tile in 0..4 {
            assert_eq!(
                detour.step(
                    &Around::new(Heading::centred(intent), false, true, true),
                    Leeway::Quarter
                ),
                Step::Aside(ccw),
                "tile {tile}: the flank that worked is preferred over the way back"
            );
        }
        // The way ahead opens: the slide is over and nothing is owed to it.
        assert_eq!(
            detour.step(
                &Around::new(Heading::centred(intent), true, true, true),
                Leeway::Quarter
            ),
            Step::Ahead(intent)
        );
        assert_eq!(
            detour,
            Detour::Clear,
            "an unrelated obstacle is not biased by this one"
        );
        assert_ne!(cw, ccw);
    }

    /// Boxed in: nothing to send, the flank that is no longer working is not
    /// kept — and the body is *standing*, which is what it stays until the
    /// scene offers something, however long the player leans on the key.
    #[test]
    fn nothing_open_is_stuck_and_stands_there() {
        let corner = Around::new(Heading::centred(Direction::East), false, false, false);
        let mut detour = Detour::Sliding(Direction::North);

        assert_eq!(detour.step(&corner, Leeway::Quarter), Step::Stuck);
        assert_eq!(detour, Detour::Standing);
        for beat in 0..10 {
            assert_eq!(detour.step(&corner, Leeway::Quarter), Step::Stuck, "beat {beat}");
            assert_eq!(detour, Detour::Standing, "beat {beat}: still nowhere to go");
        }
        // A door opens somewhere in front of it, with nothing else asked for:
        // the walk resumes and the standing is over.
        assert_eq!(
            detour.step(
                &Around::new(Heading::centred(Direction::East), true, false, false),
                Leeway::Quarter
            ),
            Step::Ahead(Direction::East)
        );
        assert_eq!(detour, Detour::Clear, "the standing is over");
    }

    /// Letting go is not the same as being stuck: a heading picked up later,
    /// somewhere else, must not start out believing it is in a corner.
    #[test]
    fn forgetting_leaves_the_corner_behind() {
        let mut detour = Detour::Standing;
        detour.forget();
        assert_eq!(detour, Detour::Clear);
        assert_eq!(detour, Detour::Clear, "the standing is over");
    }

    /// The scene read from a world agrees with the scene stated outright — the
    /// two constructors are one rule, or the exhaustive test above proves
    /// something about a fiction.
    #[test]
    fn a_scene_read_from_the_world_is_a_scene_stated_outright() {
        // East is walled, and so is the tile north of the body — which is the
        // counter-clockwise flank of East. South, the clockwise one, is open.
        // No map at all under it, so the two walls are the only thing that can
        // refuse anything.
        let mut corner = Overlay::default();
        corner.set(Tile::new(101, 100), vec![Cover::blocking(0, 20)]);
        corner.set(Tile::new(100, 99), vec![Cover::blocking(0, 20)]);
        let corner = Footing::new(None, &corner, Doors::AsTheyStand);

        let from = Point::new(100, 100, 0);
        assert_eq!(
            Around::read(&corner, from, Heading::centred(Direction::East)),
            Around::new(Heading::centred(Direction::East), false, true, false)
        );
        // And a diagonal whose tile is open ground but whose corner is cut:
        // read must refuse it, which `can_step` alone would not.
        assert!(crate::can_step(&corner, from, Point::new(101, 101, 0)).is_some());
        assert_eq!(
            Around::read(&corner, from, Heading::centred(Direction::SouthEast)),
            Around::new(Heading::centred(Direction::SouthEast), false, true, false)
        );
    }
}
